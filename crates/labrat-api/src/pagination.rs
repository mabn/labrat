//! GitLab's offset pagination: `page`/`per_page` params, `X-*` headers and the `Link` header.

use crate::ctx::{ApiError, Req, Resp};
use labrat_core::Error;
use serde_json::Value;

pub const DEFAULT_PER_PAGE: i64 = 20;
pub const MAX_PER_PAGE: i64 = 100;
/// Above this many rows GitLab omits `X-Total` and `X-Total-Pages`.
pub const COUNT_LIMIT: usize = 10_000;

pub struct Page {
    pub page: i64,
    pub per_page: i64,
}

impl Page {
    pub fn from_req(req: &Req) -> Result<Page, ApiError> {
        // Grape coerces an integer too big for 64 bits (a Bignum); the query then fails: 500.
        let big = |k: &str| {
            req.params.str(k).is_some_and(|v| {
                let d = v.strip_prefix('-').unwrap_or(&v);
                !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()) && v.parse::<i64>().is_err()
            })
        };
        if big("page") {
            return Err(Error::Api(500, serde_json::json!("500 Internal Server Error")).into());
        }
        let page = req.params.int("page")?.unwrap_or(1).max(1);
        let per_page = req.params.int("per_page")?.unwrap_or(DEFAULT_PER_PAGE);
        if per_page == 0 {
            return Err(Error::Param("per_page has a value not allowed".into()).into());
        }
        let per_page = if per_page < 1 {
            DEFAULT_PER_PAGE
        } else {
            per_page.min(MAX_PER_PAGE)
        };
        // The SQL `OFFSET` is a bigint: one that doesn't fit is a database error, so a 500.
        if (page - 1).checked_mul(per_page).is_none() {
            return Err(Error::Api(500, serde_json::json!("500 Internal Server Error")).into());
        }
        Ok(Page { page, per_page })
    }

    pub fn offset(&self) -> usize {
        usize::try_from((self.page - 1).saturating_mul(self.per_page)).unwrap_or(usize::MAX)
    }

    /// Gitaly's commit listing takes a 32-bit offset: a larger one fails, a 500.
    pub fn check_gitaly_offset(&self) -> Result<(), ApiError> {
        if (self.page - 1).saturating_mul(self.per_page) > i32::MAX as i64 {
            return Err(Error::Api(500, serde_json::json!("500 Internal Server Error")).into());
        }
        Ok(())
    }
}

/// Paginates an already filtered and sorted list and renders each item.
///
/// `defaults` are the endpoint's declared parameters with default values; GitLab includes
/// them in the `Link` header URLs.
pub fn paginate<T>(
    req: &Req,
    items: Vec<T>,
    defaults: &[(&str, &str)],
    mut render: impl FnMut(&T) -> Result<Value, ApiError>,
) -> Result<Resp, ApiError> {
    let page = Page::from_req(req)?;
    let total = items.len();
    let rows: Vec<Value> = items
        .iter()
        .skip(page.offset())
        .take(page.per_page as usize)
        .map(&mut render)
        .collect::<Result<_, _>>()?;
    Ok(Resp::ok(Value::Array(rows)).paginated(req, &page, total, defaults))
}

/// `GitalyKeysetPager`: like `paginate`, but when Gitaly serves the first page the page count
/// is unclamped, 0 for an empty list (and the `last` link points at page 0).
pub fn paginate_gitaly<T>(
    req: &Req,
    items: Vec<T>,
    first_page_via_gitaly: bool,
    defaults: &[(&str, &str)],
    mut render: impl FnMut(&T) -> Result<Value, ApiError>,
) -> Result<Resp, ApiError> {
    let page = Page::from_req(req)?;
    let total = items.len();
    let rows: Vec<Value> = items
        .iter()
        .skip(page.offset())
        .take(page.per_page as usize)
        .map(&mut render)
        .collect::<Result<_, _>>()?;
    Ok(Resp::ok(Value::Array(rows)).paginated_opts(req, &page, total, defaults, !first_page_via_gitaly))
}

/// Kaminari's `without_count`: no totals and no `last` link; a next page exists when more items
/// follow this one.
pub fn paginate_without_count<T>(
    req: &Req,
    items: Vec<T>,
    defaults: &[(&str, &str)],
    mut render: impl FnMut(&T) -> Result<Value, ApiError>,
) -> Result<Resp, ApiError> {
    let page = Page::from_req(req)?;
    let rows: Vec<Value> = items
        .iter()
        .skip(page.offset())
        .take(page.per_page as usize)
        .map(&mut render)
        .collect::<Result<_, _>>()?;
    let more = items.len() > page.offset() + page.per_page as usize;
    let next = more.then_some(page.page.saturating_add(1));
    let prev = (page.page > 1).then_some(page.page - 1);
    let link = |p: i64| -> String { page_url(req, &page, p, defaults) };
    let mut links = vec![];
    if let Some(p) = prev {
        links.push(format!("<{}>; rel=\"prev\"", link(p)));
    }
    if let Some(n) = next {
        links.push(format!("<{}>; rel=\"next\"", link(n)));
    }
    links.push(format!("<{}>; rel=\"first\"", link(1)));
    Ok(Resp::ok(Value::Array(rows))
        .header("Link", links.join(", "))
        .header("X-Next-Page", next.map(|n| n.to_string()).unwrap_or_default())
        .header("X-Page", page.page.to_string())
        .header("X-Per-Page", page.per_page.to_string())
        .header("X-Prev-Page", prev.map(|n| n.to_string()).unwrap_or_default()))
}

impl Resp {
    pub fn paginated(self, req: &Req, page: &Page, total: usize, defaults: &[(&str, &str)]) -> Resp {
        self.paginated_opts(req, page, total, defaults, true)
    }

    /// `at_least_one_page`: `OffsetPagination` reports at least one page; Kaminari alone 0.
    pub fn paginated_opts(
        mut self,
        req: &Req,
        page: &Page,
        total: usize,
        defaults: &[(&str, &str)],
        at_least_one_page: bool,
    ) -> Resp {
        let total_pages = ((total as i64 + page.per_page - 1) / page.per_page).max(i64::from(at_least_one_page));
        let next = if page.page < total_pages {
            Some(page.page.saturating_add(1))
        } else {
            None
        };
        // Kaminari: no previous page on the first page or past the last one.
        let prev = if page.page > 1 && page.page <= total_pages {
            Some(page.page - 1)
        } else {
            None
        };
        let link = |p: i64| -> String { page_url(req, page, p, defaults) };
        let mut links = vec![];
        if let Some(p) = prev {
            links.push(format!("<{}>; rel=\"prev\"", link(p)));
        }
        if let Some(n) = next {
            links.push(format!("<{}>; rel=\"next\"", link(n)));
        }
        links.push(format!("<{}>; rel=\"first\"", link(1)));
        if total <= COUNT_LIMIT {
            links.push(format!("<{}>; rel=\"last\"", link(total_pages)));
        }
        self = self.header("Link", links.join(", "));
        self = self.header(
            "X-Next-Page",
            next.map(|n| n.to_string()).unwrap_or_default(),
        );
        self = self.header("X-Page", page.page.to_string());
        self = self.header("X-Per-Page", page.per_page.to_string());
        self = self.header(
            "X-Prev-Page",
            prev.map(|n| n.to_string()).unwrap_or_default(),
        );
        if total <= COUNT_LIMIT {
            self = self.header("X-Total", total.to_string());
            self = self.header("X-Total-Pages", total_pages.to_string());
        }
        self
    }
}

/// Builds a page URL: declared defaults plus given params, sorted by key as Rails' `to_query` does.
fn page_url(req: &Req, page: &Page, p: i64, defaults: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(String, String)> = defaults
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    for (k, v) in &req.path_params {
        pairs.retain(|(pk, _)| pk != k);
        pairs.push((k.clone(), v.clone()));
    }
    // Given params replace defaults; repeated array params (`labels[]`) are all kept.
    let mut given: Vec<(String, String)> = vec![];
    for (k, v) in url::form_urlencoded::parse(req.query.as_bytes()) {
        if k == "private_token" || k == "access_token" {
            continue;
        }
        if !k.ends_with("[]") {
            given.retain(|(gk, _)| gk != &k);
        }
        given.push((k.into_owned(), v.into_owned()));
    }
    for (k, _) in &given {
        pairs.retain(|(pk, _)| pk != k);
    }
    pairs.extend(given);
    pairs.retain(|(k, _)| k != "page" && k != "per_page");
    pairs.push(("page".into(), p.to_string()));
    pairs.push(("per_page".into(), page.per_page.to_string()));
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    format!("{}{}?{}", req.app.config.external_url, req.path, query)
}

pub fn bad_param(name: &str) -> ApiError {
    Error::Param(format!("{name} is invalid")).into()
}
