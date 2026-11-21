//! `lib/api/merge_requests.rb`, `merge_request_diffs.rb`, subscriptions, todos, time tracking
//! and resource events for merge requests.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::merge_request::{self as ent, Opts};
use crate::entities::repository as repo_ent;
use crate::entities::user::{self as user_ent, Ctx};
use crate::find;
use crate::pagination::{Page, paginate};
use axum::Router;
use axum::routing::{get, post, put};
use labrat_core::model::{Namespace, Project, User, access};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::merge_requests::{
    self as mrs, MergeRequest, MrParams, diffs::Diff, merge, mergeability,
};
use labrat_core::{App, Error, repo, time};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    let mr = "/projects/{id}/merge_requests/{merge_request_iid}";
    Router::new()
        .route("/merge_requests", get(list_global))
        .route("/groups/{id}/merge_requests", get(list_group))
        .route(
            "/projects/{id}/merge_requests",
            get(list_project).post(create),
        )
        .route(mr, get(show).put(update).delete(destroy))
        .route(&format!("{mr}/participants"), get(participants))
        .route(&format!("{mr}/reviewers"), get(reviewers))
        .route(&format!("{mr}/commits"), get(commits))
        .route(&format!("{mr}/changes"), get(changes))
        .route(&format!("{mr}/diffs"), get(diffs))
        .route(&format!("{mr}/raw_diffs"), get(raw_diffs))
        .route(&format!("{mr}/pipelines"), get(pipelines).post(crate::routes::pipelines::create_mr_pipeline))
        .route(&format!("{mr}/merge"), put(do_merge))
        .route(&format!("{mr}/merge_ref"), get(merge_ref))
        .route(
            &format!("{mr}/cancel_merge_when_pipeline_succeeds"),
            post(cancel_auto_merge),
        )
        .route(&format!("{mr}/rebase"), put(rebase))
        .route(&format!("{mr}/closes_issues"), get(empty_list))
        .route(&format!("{mr}/related_issues"), get(empty_list))
        .route(&format!("{mr}/versions"), get(versions))
        .route(&format!("{mr}/versions/{{version_id}}"), get(version))
        .route(&format!("{mr}/subscribe"), post(subscribe))
        .route(&format!("{mr}/unsubscribe"), post(unsubscribe))
        .route(&format!("{mr}/todo"), post(todo))
        .route(&format!("{mr}/time_estimate"), post(time_estimate))
        .route(
            &format!("{mr}/reset_time_estimate"),
            post(reset_time_estimate),
        )
        .route(&format!("{mr}/add_spent_time"), post(add_spent_time))
        .route(&format!("{mr}/reset_spent_time"), post(reset_spent_time))
        .route(&format!("{mr}/time_stats"), get(time_stats))
}

/// The project and MR for `:id/:iid`, requiring read access to merge requests.
pub fn load(c: &Connection, req: &Req) -> Result<(Project, ProjectPerms, MergeRequest), ApiError> {
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    let (name, raw) = if req.path_params.iter().any(|(k, _)| k == "eventable_id") {
        ("eventable_id", req.path_param("eventable_id"))
    } else {
        ("merge_request_iid", req.path_param("merge_request_iid"))
    };
    let iid: i64 = raw
        .parse()
        .map_err(|_| Error::Param(format!("{name} is invalid")))?;
    let mr = MergeRequest::find_by_iid(c, p.id, iid)?
        .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
    // `MergeRequestsFinder` only finds MRs the user may read.
    if !perms.read_merge_request(&p) {
        return Err(Error::Api(404, json!("404 Not found")).into());
    }
    Ok((p, perms, mr))
}

fn ectx<'a>(req: &'a Req) -> Ctx<'a> {
    Ctx {
        base_url: &req.app.config.external_url, ee: req.app.config.is_ee(),
        current_user: req.user.as_ref(),
    }
}

/// Renders a single MR. Like GitLab's entity, reading `merge_status` as a user who can update
/// the MR first queues an async mergeability recheck.
fn single(
    c: &Connection,
    req: &Req,
    p: &Project,
    mr: &MergeRequest,
    o: Opts,
) -> Result<Value, ApiError> {
    let mut mr = mr.clone();
    if let Some(u) = &req.user
        && mrs::can_update(c, p, &mr, u)? && mergeability::check_async(c, &mr)? {
            mr = mr.reload(c)?;
        }
    Ok(ent::single(
        c,
        &req.app.config,
        req.user.as_ref(),
        p,
        &mr,
        &o,
    )?)
}

// ---------------------------------------------------------------------------------------------
// Listing

const LIST_DEFAULTS: &[(&str, &str)] = &[
    ("order_by", "created_at"),
    ("sort", "desc"),
    ("state", "all"),
    ("with_labels_details", "false"),
    ("with_merge_status_recheck", "false"),
];

fn user_ref(c: &Connection, v: &str) -> Result<Option<i64>, ApiError> {
    Ok(find::user_by_ref(c, v)?.map(|u| u.id))
}

/// Grape's `CommaSeparatedToArray`: strings split on commas, each element trimmed, blanks dropped.
fn comma_list(v: Option<&Value>) -> Option<Vec<String>> {
    let items: Vec<String> = match v? {
        Value::Array(a) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect(),
        Value::String(s) => vec![s.clone()],
        other => vec![other.to_string()],
    };
    Some(items.iter().flat_map(|s| s.split(',').map(|p| p.trim().to_string()).collect::<Vec<_>>()).filter(|s| !s.is_empty()).collect())
}

/// Ruby's `String#to_i`.
fn ruby_to_i(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let d: String = digits.chars().take_while(char::is_ascii_digit).collect();
    let n: i64 = d.parse().unwrap_or(0);
    if neg { -n } else { n }
}

/// `Gitlab::SQL::Pattern.split_query_to_search_terms`: whitespace-separated, quoted phrases kept.
fn search_terms(q: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = q;
    while let Some(i) = rest.find('"') {
        let (before, after) = rest.split_at(i);
        out.extend(before.split_whitespace().map(str::to_string));
        match after[1..].find('"') {
            Some(j) => {
                out.push(after[1..1 + j].to_string());
                rest = &after[j + 2..];
            }
            None => {
                rest = &after[1..];
            }
        }
    }
    out.extend(rest.split_whitespace().map(str::to_string));
    out.retain(|t| !t.is_empty());
    out
}

/// Rewrites list query params the way Grape coerces them, so pagination links match GitLab's
/// (`labels=a,b` becomes `labels[]=a&labels[]=b`, blank labels disappear, `iids` are integers).
/// `project_list`: only the project list declares `iids` (an Array[Integer]).
fn coerce_list_query(req: &mut Req, project_list: bool) {
    let mut pairs: Vec<(String, String)> = vec![];
    let mut arrays: Vec<(String, Vec<String>)> = vec![];
    for (k, v) in url::form_urlencoded::parse(req.query.as_bytes()) {
        let (k, v) = (k.into_owned(), v.into_owned());
        let base = k.trim_end_matches("[]").to_string();
        match base.as_str() {
            // `assignee_username` is an Array param: a single value becomes `[value]`.
            "assignee_username" if !k.ends_with("[]") => pairs.push(("assignee_username[]".to_string(), v)),
            "iids" if !project_list => pairs.push((k, v)),
            "labels" | "not[labels]" | "iids" => {
                let mut vals: Vec<String> = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if base == "iids" {
                    vals = vals.iter().map(|s| ruby_to_i(s).to_string()).collect();
                }
                match arrays.iter_mut().find(|(n, _)| *n == base) {
                    Some((_, list)) => list.extend(vals),
                    None => arrays.push((base, vals)),
                }
            }
            // Grape coerces these to DateTime; GitLab's Link header shows the coerced value.
            "created_after" | "created_before" | "updated_after" | "updated_before" | "merged_after" | "merged_before"
                if v.len() == 10 && labrat_core::time::parse_date(&v).is_some() =>
            {
                pairs.push((k, format!("{v}T00:00:00+00:00")))
            }
            _ => pairs.push((k, v)),
        }
    }
    for (name, vals) in arrays {
        for v in vals {
            pairs.push((format!("{name}[]"), v));
        }
    }
    req.query = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
}

/// Applies the MR list filters of `merge_requests_base_params`.
pub(crate) fn filter(
    c: &Connection,
    req: &Req,
    list: Vec<(Project, MergeRequest)>,
    scope_default: Option<&str>,
) -> Result<Vec<(Project, MergeRequest)>, ApiError> {
    let p = &req.params;
    let me = req.user.clone();
    // Typed params that are empty are dropped by Grape (`declared_params` keeps nothing).
    let present = |k: &str| p.str(k).is_some_and(|v| !v.is_empty()) || matches!(p.raw(k), Some(Value::Array(_)));
    p.one_of("approved", &["yes", "no"])?;
    p.one_of("view", &["simple"])?;
    if p.has("assignee_id") && p.has("assignee_username") {
        return Err(Error::Param("assignee_id, assignee_username are mutually exclusive".into()).into());
    }
    for k in ["assignee_id", "reviewer_id"] {
        if let Some(v) = p.str(k)
            && !v.is_empty()
            && v.parse::<i64>().is_err()
            && !v.eq_ignore_ascii_case("none")
            && !v.eq_ignore_ascii_case("any")
        {
            return Err(Error::Param(format!("{k} should be an integer, none, or any, however got {v}")).into());
        }
    }
    // labrat has no deployments, so the deployment filters match nothing.
    let no_deployments = present("environment")
        || ["deployed_before", "deployed_after"].iter().any(|k| p.str(k).is_some_and(|v| labrat_core::time::parse_time(&v).is_some()));
    let state = p.one_of("state", &["opened", "closed", "locked", "merged", "all"])?;
    let order_by = p
        .one_of(
            "order_by",
            &[
                "created_at",
                "label_priority",
                "milestone_due",
                "popularity",
                "priority",
                "title",
                "updated_at",
                "merged_at",
            ],
        )?
        .unwrap_or("created_at".into());
    let desc = p.one_of("sort", &["asc", "desc"])?.as_deref() != Some("asc");
    let scope = p
        .one_of(
            "scope",
            &[
                "created-by-me",
                "assigned-to-me",
                "created_by_me",
                "assigned_to_me",
                "reviews_for_me",
                "all",
            ],
        )?
        .map(|s| s.replace('-', "_"))
        .or(scope_default.map(str::to_string));
    if p.has("author_id") && p.has("author_username") {
        return Err(
            Error::Param("author_id, author_username are mutually exclusive".into()).into(),
        );
    }
    if p.has("reviewer_id") && p.has("reviewer_username") {
        return Err(
            Error::Param("reviewer_id, reviewer_username are mutually exclusive".into()).into(),
        );
    }
    if let Some(Value::Array(a)) = p.raw("assignee_username")
        && a.len() > 1 {
            let names: Vec<String> = a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            return Err(Error::Param(format!(
                "assignee_username allows one value, but found {}: {}",
                names.len(),
                names.join(", ")
            ))
            .into());
        }
    let author_id = match (if present("author_id") { p.int("author_id")? } else { None }, p.str("author_username").filter(|u| !u.is_empty())) {
        (Some(id), _) => Some(Some(id)),
        (None, Some(u)) => Some(user_ref(c, &u)?),
        _ => None,
    };
    // `assignee_username` is one literal name (no comma splitting).
    let assignee = p.str("assignee_id").filter(|v| !v.is_empty()).or_else(|| {
        match p.raw("assignee_username") {
            Some(Value::Array(a)) => a.first().and_then(Value::as_str).map(str::to_string),
            _ => p.str("assignee_username"),
        }
        .filter(|u| !u.is_empty())
        .map(|u| format!("@{u}"))
    });
    let reviewer = p
        .str("reviewer_id")
        .or_else(|| p.str("reviewer_username").map(|u| format!("@{u}")));
    let labels = comma_list(p.raw("labels"));
    let milestone = p.str("milestone").filter(|v| !v.is_empty());
    let source_branch = p.str("source_branch").filter(|v| !v.is_empty());
    let target_branch = p.str("target_branch").filter(|v| !v.is_empty());
    let search = p.str("search").map(|s| search_terms(&s.to_lowercase()));
    let search_in = {
        let given: Vec<String> = p.str("in").unwrap_or_default().split(',').map(|s| s.trim().to_string()).collect();
        let fields: Vec<String> = given.into_iter().filter(|f| f == "title" || f == "description").collect();
        if fields.is_empty() { vec!["title".to_string(), "description".to_string()] } else { fields }
    };
    let wip = p.one_of("wip", &["yes", "no"])?;
    let time_of = |k: &str| -> Result<Option<i64>, ApiError> { if present(k) { Ok(p.time(k)?) } else { Ok(None) } };
    let created_after = time_of("created_after")?;
    let created_before = time_of("created_before")?;
    let updated_after = time_of("updated_after")?;
    let updated_before = time_of("updated_before")?;
    let iids: Option<Vec<i64>> = comma_list(p.raw("iids")).map(|v| v.iter().map(|s| ruby_to_i(s)).collect());
    let not = p.raw("not").cloned();
    let my_reaction = p.str("my_reaction_emoji");
    let source_project_id = p.int("source_project_id")?;
    if p.has("merge_user_id") && p.has("merge_user_username") {
        return Err(Error::Param("merge_user_id, merge_user_username are mutually exclusive".into()).into());
    }
    let merge_user = match (if present("merge_user_id") { p.int("merge_user_id")? } else { None }, p.str("merge_user_username").filter(|u| !u.is_empty())) {
        (Some(id), _) => Some(Some(id)),
        (None, Some(u)) => Some(User::find_by_username(c, &u)?.map(|u| u.id)),
        _ => None,
    };

    // `Milestone.started` / `upcoming`: by start date against today.
    let today = labrat_core::time::today();
    let milestone_dates = |id: i64| -> Option<(Option<String>, Option<String>, String)> {
        c.query_row("SELECT start_date, due_date, title FROM milestones WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok()
    };
    let started = |id: i64| milestone_dates(id).is_some_and(|(sd, _, _)| sd.is_some_and(|d| d <= today));
    let upcoming = |id: i64| milestone_dates(id).is_some_and(|(sd, _, _)| sd.is_some_and(|d| d > today));
    let mut out = vec![];
    if no_deployments {
        return Ok(out);
    }
    for (proj, mr) in list {
        if let Some(s) = &state
            && s != "all" && mr.state != *s {
                continue;
            }
        match scope.as_deref() {
            Some("created_by_me") => {
                if me.as_ref().is_none_or(|u| u.id != mr.author_id) {
                    continue;
                }
            }
            Some("assigned_to_me") => {
                if me
                    .as_ref()
                    .is_none_or(|u| !mr.assignee_ids(c).unwrap_or_default().contains(&u.id))
                {
                    continue;
                }
            }
            Some("reviews_for_me")
                if me
                    .as_ref()
                    .is_none_or(|u| !mr.reviewer_ids(c).unwrap_or_default().contains(&u.id))
                => {
                    continue;
                }
            _ => {}
        }
        if let Some(a) = &author_id
            && *a != Some(mr.author_id) {
                continue;
            }
        if let Some(a) = &assignee {
            let ids = mr.assignee_ids(c)?;
            let ok = match a.to_lowercase().as_str() {
                "none" => ids.is_empty(),
                "any" => !ids.is_empty(),
                v if v.starts_with('@') => user_ref(c, &v[1..])?.is_some_and(|u| ids.contains(&u)),
                v => v
                    .parse::<i64>()
                    .map(|id| ids.contains(&id))
                    .map_err(|_| Error::Param("assignee_id is invalid".into()))?,
            };
            if !ok {
                continue;
            }
        }
        if let Some(rv) = &reviewer {
            let ids = mr.reviewer_ids(c)?;
            let ok = match rv.to_lowercase().as_str() {
                "none" => ids.is_empty(),
                "any" => !ids.is_empty(),
                v if v.starts_with('@') => user_ref(c, &v[1..])?.is_some_and(|u| ids.contains(&u)),
                v => v
                    .parse::<i64>()
                    .map(|id| ids.contains(&id))
                    .map_err(|_| Error::Param("reviewer_id is invalid".into()))?,
            };
            if !ok {
                continue;
            }
        }
        if let Some(ls) = labels.as_ref().filter(|l| !l.is_empty()) {
            let titles: Vec<String> = mr.labels(c)?.into_iter().map(|l| l.title).collect();
            // `Issuables::LabelFilter`: a `None`/`Any` sentinel anywhere wins.
            let ok = if ls.iter().any(|l| l.eq_ignore_ascii_case("none")) {
                titles.is_empty()
            } else if ls.iter().any(|l| l.eq_ignore_ascii_case("any")) {
                !titles.is_empty()
            } else {
                ls.iter().all(|l| titles.contains(l))
            };
            if !ok {
                continue;
            }
        }
        if let Some(ms) = &milestone {
            let title: Option<String> = match mr.milestone_id {
                Some(id) => c
                    .query_row("SELECT title FROM milestones WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })
                    .ok(),
                None => None,
            };
            let ok = match ms.as_str() {
                m if m.eq_ignore_ascii_case("none") || m == "No Milestone" => title.is_none(),
                m if m.eq_ignore_ascii_case("any") || m == "Any Milestone" => title.is_some(),
                "#started" => mr.milestone_id.is_some_and(started),
                "#upcoming" => mr.milestone_id.is_some_and(upcoming),
                _ => title.as_deref() == Some(ms.as_str()),
            };
            if !ok {
                continue;
            }
        }
        if source_branch
            .as_ref()
            .is_some_and(|b| *b != mr.source_branch)
            || target_branch
                .as_ref()
                .is_some_and(|b| *b != mr.target_branch)
        {
            continue;
        }
        if let Some(terms) = &search {
            // Every term must match within the same column; columns are OR'ed. The search API
            // (`__min_chars`) uses `fuzzy_arel_match` with the minimum character limit: terms under
            // three characters are dropped, and with none left the whole query must equal the column.
            let long: Vec<&String> = terms.iter().filter(|t| t.chars().count() >= 3).collect();
            let whole = p.str("search").unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
            let min_chars = p.has("__min_chars");
            let all_in = |text: &str| {
                let t = text.to_lowercase();
                if min_chars {
                    if long.is_empty() { t == whole } else { long.iter().all(|term| t.contains(term.as_str())) }
                } else {
                    terms.iter().all(|term| t.contains(term.as_str()))
                }
            };
            let in_title = search_in.iter().any(|f| f == "title") && all_in(&mr.title);
            let in_desc = search_in.iter().any(|f| f == "description")
                && all_in(mr.description.as_deref().unwrap_or(""));
            if !in_title && !in_desc {
                continue;
            }
        }
        if let Some(w) = &wip {
            let l = mr.title.to_lowercase();
            let is = l.starts_with("draft - ")
                || l.starts_with("draft:")
                || l.starts_with("[draft]")
                || l.starts_with("(draft)");
            if (w == "yes") != is {
                continue;
            }
        }
        if created_after.is_some_and(|t| mr.created_at < t)
            || created_before.is_some_and(|t| mr.created_at > t)
            || updated_after.is_some_and(|t| mr.updated_at < t)
            || updated_before.is_some_and(|t| mr.updated_at > t)
        {
            continue;
        }
        if let Some(ids) = &iids
            && !ids.contains(&mr.iid) {
                continue;
            }
        if source_project_id.is_some_and(|id| Some(id) != mr.source_project_id) {
            continue;
        }
        if let Some(want) = &merge_user
            && (mr.merge_user_id != *want || want.is_none()) {
                continue;
            }
        if let Some(Value::Object(n)) = &not {
            let int_of = |v: &Value| v.as_str().and_then(|s| s.parse::<i64>().ok()).or(v.as_i64());
            if let Some(name) = n.get("author_username").and_then(Value::as_str)
                && User::find(c, mr.author_id)?.is_some_and(|u| u.username == name) {
                    continue;
                }
            if let Some(id) = n.get("assignee_id").and_then(int_of)
                && mr.assignee_ids(c)?.contains(&id) {
                    continue;
                }
            if let Some(names) = comma_list(n.get("assignee_username")) {
                let mut hit = false;
                for id in mr.assignee_ids(c)? {
                    if User::find(c, id)?.is_some_and(|u| names.contains(&u.username)) {
                        hit = true;
                    }
                }
                if hit {
                    continue;
                }
            }
            // A negated reviewer that doesn't exist matches no MR at all.
            if let Some(id) = n.get("reviewer_id").and_then(int_of)
                && (mr.reviewer_ids(c)?.contains(&id) || User::find(c, id)?.is_none()) {
                    continue;
                }
            if let Some(name) = n.get("reviewer_username").and_then(Value::as_str) {
                if User::find_by_username(c, name)?.is_none() {
                    continue;
                }
                let mut hit = false;
                for id in mr.reviewer_ids(c)? {
                    if User::find(c, id)?.is_some_and(|u| u.username == name) {
                        hit = true;
                    }
                }
                if hit {
                    continue;
                }
            }
            // `not[milestone]=#started`: MRs whose milestone hasn't started.
            if n.get("milestone").and_then(Value::as_str) == Some("#started") {
                if !mr.milestone_id.is_some_and(upcoming) {
                    continue;
                }
            } else if let Some(title) = n.get("milestone").and_then(Value::as_str) {
                let mt: Option<String> = match mr.milestone_id {
                    Some(id) => c.query_row("SELECT title FROM milestones WHERE id = ?1", [id], |r| r.get(0)).ok(),
                    None => None,
                };
                if mt.as_deref() == Some(title) {
                    continue;
                }
            }
            if let (Some(e), Some(u)) = (n.get("my_reaction_emoji").and_then(Value::as_str), &me) {
                let k: i64 = c.query_row(
                    "SELECT count(*) FROM award_emoji WHERE awardable_type = 'MergeRequest' AND awardable_id = ?1 AND user_id = ?2 AND name = ?3",
                    params![mr.id, u.id, e],
                    |r| r.get(0),
                )?;
                if k > 0 {
                    continue;
                }
            }
            if let Some(v) = n.get("author_id").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<i64>().ok())
                    .or(v.as_i64())
            })
                && v == mr.author_id {
                    continue;
                }
            if let Some(v) = comma_list(n.get("labels")) {
                let titles: Vec<String> = mr.labels(c)?.into_iter().map(|l| l.title).collect();
                if v.iter().any(|l| titles.iter().any(|t| t == l)) {
                    continue;
                }
            }
        }
        if let (Some(e), Some(u)) = (&my_reaction, &me) {
            // `None` / `Any` (any case): the user's reactions of any name.
            let wildcard = e.eq_ignore_ascii_case("none") || e.eq_ignore_ascii_case("any");
            let n: i64 = c.query_row(
                "SELECT count(*) FROM award_emoji WHERE awardable_type = 'MergeRequest' AND awardable_id = ?1 AND user_id = ?2 AND (?4 OR name = ?3)",
                params![mr.id, u.id, e, wildcard],
                |r| r.get(0),
            )?;
            let ok = if e.eq_ignore_ascii_case("none") {
                n == 0
            } else {
                n > 0
            };
            if !ok {
                continue;
            }
        }
        out.push((proj, mr));
    }
    // Keys for the orderings that need more than MR columns.
    let mut extra: std::collections::HashMap<i64, (Option<i64>, Option<String>, i64)> = std::collections::HashMap::new();
    if matches!(order_by.as_str(), "label_priority" | "priority" | "milestone_due" | "popularity") {
        for (_, m) in &out {
            let label_prio: Option<i64> = m.labels(c)?.iter().filter_map(|l| l.priority).min();
            let due: Option<String> = match m.milestone_id {
                Some(id) => c.query_row("SELECT due_date FROM milestones WHERE id = ?1", [id], |r| r.get(0)).ok().flatten(),
                None => None,
            };
            let ups: i64 = c.query_row(
                "SELECT count(*) FROM award_emoji WHERE awardable_type = 'MergeRequest' AND awardable_id = ?1 AND name = 'thumbsup'",
                [m.id],
                |r| r.get(0),
            )?;
            extra.insert(m.id, (label_prio, due, ups));
        }
    }
    out.sort_by(|(_, a), (_, b)| {
        let (ea, eb) = (extra.get(&a.id), extra.get(&b.id));
        match order_by.as_str() {
            // `order_labels_priority` / `order_due_date_and_labels_priority`: `sort` sets the
            // direction, NULLs always last, ties id-descending.
            "label_priority" | "priority" => {
                let dir = |o: std::cmp::Ordering| if desc { o.reverse() } else { o };
                let nl = |x: Option<std::cmp::Ordering>, a_none: bool, b_none: bool| match (a_none, b_none) {
                    (false, false) => dir(x.unwrap()),
                    (false, true) => std::cmp::Ordering::Less,
                    (true, false) => std::cmp::Ordering::Greater,
                    (true, true) => std::cmp::Ordering::Equal,
                };
                let (pa, pb) = (ea.and_then(|e| e.0), eb.and_then(|e| e.0));
                let by_label = nl(pa.zip(pb).map(|(x, y)| x.cmp(&y)), pa.is_none(), pb.is_none());
                let o = if order_by == "priority" {
                    let (da, db) = (ea.and_then(|e| e.1.clone()), eb.and_then(|e| e.1.clone()));
                    let by_due = nl(da.as_ref().zip(db.as_ref()).map(|(x, y)| x.cmp(y)), da.is_none(), db.is_none());
                    by_due.then(by_label)
                } else {
                    by_label
                };
                return o.then(b.id.cmp(&a.id));
            }
            "milestone_due" => {
                let o = match (ea.and_then(|e| e.1.clone()), eb.and_then(|e| e.1.clone())) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                };
                let o = if desc { o.reverse() } else { o };
                return o.then(b.id.cmp(&a.id));
            }
            "popularity" => {
                let o = ea.map(|e| e.2).unwrap_or(0).cmp(&eb.map(|e| e.2).unwrap_or(0));
                let o = if desc { o.reverse() } else { o };
                return o.then(b.id.cmp(&a.id));
            }
            _ => {}
        }
        let o = match order_by.as_str() {
            "updated_at" => a.updated_at.cmp(&b.updated_at),
            // `order_title_asc/desc.with_order_id_desc`: ties stay id-descending.
            "title" => {
                let o = a.title.to_lowercase().cmp(&b.title.to_lowercase());
                let o = if desc { o.reverse() } else { o };
                return o.then(b.id.cmp(&a.id));
            }
            // NULLs last in both directions.
            "merged_at" => {
                let o = match (a.merged_at, b.merged_at) {
                    (Some(x), Some(y)) => if desc { y.cmp(&x) } else { x.cmp(&y) },
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                return o.then(b.id.cmp(&a.id));
            }
            _ => a.created_at.cmp(&b.created_at),
        };
        let o = if desc { o.reverse() } else { o };
        o.then(b.id.cmp(&a.id))
    });
    Ok(out)
}

/// `MergeRequestBasic` list with GitLab's default list params (used by milestone MR lists).
pub fn render_basic_list(c: &Connection, req: &Req, rows: Vec<(Project, MergeRequest)>) -> ApiResult {
    render_list(c, req, rows, Opts::default(), &[])
}

fn render_list(
    c: &Connection,
    req: &Req,
    rows: Vec<(Project, MergeRequest)>,
    base_opts: Opts,
    defaults: &[(&str, &str)],
) -> ApiResult {
    let simple = req.params.str("view").as_deref() == Some("simple");
    let with_labels = req.params.bool_or("with_labels_details", false)?;
    let recheck = req.params.bool_or("with_merge_status_recheck", false)?;
    if recheck
        && let Some(u) = &req.user {
            // Group lists hand the rechecks to `MergeRequests::MergeabilityCheckBatchWorker` and
            // present the stored status (`skip_merge_status_recheck`).
            let batch = base_opts.relative_to_group.is_some();
            for (p, mr) in &rows {
                if mrs::can_update(c, p, mr, u)? {
                    if batch {
                        if matches!(mr.merge_status.as_str(), "unchecked" | "cannot_be_merged_recheck") {
                            labrat_core::jobs::enqueue(c, "mr_check_mergeability", serde_json::json!({"merge_request_id": mr.id}))?;
                        }
                    } else {
                        mergeability::check_async(c, mr)?;
                    }
                }
            }
        }
    let opts = Opts {
        with_labels_details: with_labels,
        ..base_opts
    };
    paginate(req, rows, defaults, |(p, mr)| {
        let mr = mr.reload(c)?;
        if simple {
            Ok(ent::simple(&req.app.config, p, &mr))
        } else {
            Ok(Value::Object(ent::basic(
                c,
                &req.app.config,
                req.user.as_ref(),
                p,
                &mr,
                &opts,
            )?))
        }
    })
}

async fn list_project(mut req: Req) -> ApiResult {
    coerce_list_query(&mut req, true);
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.read_merge_request(&p) {
            return Err(Error::forbidden().into());
        }
        let all = MergeRequest::list_where(c, "target_project_id = ?1", [p.id])?;
        let rows = filter(
            c,
            &req,
            all.into_iter().map(|m| (p.clone(), m)).collect(),
            None,
        )?;
        render_list(c, &req, rows, Opts::default(), LIST_DEFAULTS)
    });
    crate::jobs::kick(&req.app);
    result
}

/// MRs in all projects `user` can read merge requests in, optionally under `namespaces`.
pub(crate) fn visible_mrs(
    c: &Connection,
    req: &Req,
    namespaces: Option<&[i64]>,
) -> Result<Vec<(Project, MergeRequest)>, ApiError> {
    let projects = match namespaces {
        Some(ns) => {
            let list = ns
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            Project::list_where(c, &format!("p.namespace_id IN ({list})"), [])?
        }
        None => Project::list_where(c, "1 = 1", [])?,
    };
    let mut out = vec![];
    for p in projects {
        let perms = ProjectPerms::load(c, req.user.as_ref(), &p)?;
        if !perms.read_merge_request(&p) {
            continue;
        }
        for mr in MergeRequest::list_where(c, "target_project_id = ?1", [p.id])? {
            out.push((p.clone(), mr));
        }
    }
    Ok(out)
}

async fn list_group(mut req: Req) -> ApiResult {
    coerce_list_query(&mut req, false);
    let app = req.app.clone();
    app.db.tx(|c| {
        let (g, _) = find::group(c, &req, req.path_param("id"))?;
        let ns = Namespace::self_and_descendant_ids(c, g.id)?;
        // `non_archived` (default true) leaves out archived projects.
        let mut all = visible_mrs(c, &req, Some(&ns))?;
        if req.params.bool_or("non_archived", true)? {
            all.retain(|(p, _)| !p.archived);
        }
        // `iids` is declared on the project list only: it doesn't filter here (but stays in links).
        let mut m = req.params.map.clone();
        m.remove("iids");
        let sub = req.with_params(crate::params::Params { map: m });
        let rows = filter(c, &sub, all, None)?;
        let mut defaults: Vec<(&str, &str)> = LIST_DEFAULTS.to_vec();
        defaults.push(("non_archived", "true"));
        render_list(
            c,
            &req,
            rows,
            Opts {
                relative_to_group: Some(g.id),
                ..Opts::default()
            },
            &defaults,
        )
    })
}

async fn list_global(mut req: Req) -> ApiResult {
    coerce_list_query(&mut req, false);
    if req.params.str("scope").as_deref() != Some("all") {
        req.user()?;
    }
    let app = req.app.clone();
    app.db.tx(|c| {
        // `non_archived` (default false here) leaves out archived projects.
        let mut all = visible_mrs(c, &req, None)?;
        if req.params.bool_or("non_archived", false)? {
            all.retain(|(p, _)| !p.archived);
        }
        let rows = filter(c, &req, all, Some("created_by_me"))?;
        let mut defaults: Vec<(&str, &str)> = LIST_DEFAULTS.to_vec();
        defaults.push(("non_archived", "false"));
        defaults.push(("scope", "created_by_me"));
        render_list(
            c,
            &req,
            rows,
            Opts {
                global_list: true,
                ..Opts::default()
            },
            &defaults,
        )
    })
}

// ---------------------------------------------------------------------------------------------
// Create, read, update, delete

fn mr_params(req: &Req) -> Result<MrParams, ApiError> {
    let p = &req.params;
    let mut assignees = p.int_list("assignee_ids")?;
    if let Some(a) = p.int("assignee_id")? {
        assignees = Some(vec![a]);
    }
    Ok(MrParams {
        title: p.str("title"),
        description: if p.has("description") {
            Some(p.str("description"))
        } else {
            None
        },
        source_branch: p.str("source_branch"),
        target_branch: p.str("target_branch"),
        assignee_ids: assignees,
        reviewer_ids: p.int_list("reviewer_ids")?,
        labels: p.list("labels"),
        add_labels: p.list("add_labels"),
        remove_labels: p.list("remove_labels"),
        milestone_id: if p.has("milestone_id") {
            Some(p.int("milestone_id")?)
        } else {
            None
        },
        remove_source_branch: p.bool("remove_source_branch")?,
        squash: p.bool("squash")?,
        discussion_locked: p.bool("discussion_locked")?,
        state_event: p.one_of("state_event", &["close", "reopen"])?,
        allow_collaboration: p
            .bool("allow_collaboration")?
            .or(p.bool("allow_maintainer_to_push")?),
        // A value that doesn't parse as a time clears the schedule, like an empty one.
        merge_after: if p.has("merge_after") {
            Some(p.str("merge_after").and_then(|v| labrat_core::time::parse_time(v.trim())))
        } else {
            None
        },
        spend: None,
        time_estimate: None,
    })
}

/// Quick actions in a description are run, and removed from the stored text.
pub(crate) fn take_quick_actions(prm: &mut MrParams, ee: bool) -> Vec<(String, String)> {
    let Some(Some(d)) = &prm.description else { return vec![] };
    // `merge_quick_actions_into_params!`: the extractor's text (no `\r`, trailing whitespace
    // stripped) replaces the description whenever it differs, commands or not.
    let ex = labrat_core::services::quick_actions::extract(d, ee);
    if ex.text != *d {
        prm.description = Some(Some(ex.text));
    }
    ex.commands
}

async fn create(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    req.params
        .require(&["title", "source_branch", "target_branch"])?;
    let mut prm = mr_params(&req)?;
    let cmds = take_quick_actions(&mut prm, req.app.config.is_ee());
    let approvals_before_merge = if req.app.config.is_ee() { req.params.int("approvals_before_merge")? } else { None };
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.developer() {
            return Err(Error::forbidden().into());
        }
        // Another target project is only possible for forks (out of scope): a readable project
        // isn't a fork target, an unknown or unreadable one is not found.
        if let Some(t) = req.params.int("target_project_id")?
            && t != p.id
        {
            let readable = match labrat_core::model::Project::find(c, t)? {
                Some(tp) => labrat_core::perms::ProjectPerms::load(c, Some(&me), &tp)?.read_merge_request(&tp),
                None => false,
            };
            if !readable {
                return Err(Error::Api(404, json!("404 Not found")).into());
            }
            return Err(Error::Api(422, json!(["Source project is not a fork of the target project"])).into());
        }
        let mr = mrs::create(c, &req.app.config, &p, &me, prm)?;
        // EE: `approvals_before_merge` becomes the MR's "All Members" rule. If the MR already
        // has an `any_approver` rule from the project, EE fails after creating the MR: a 500.
        let mut failed = false;
        if req.app.config.is_ee()
            && let Some(n) = approvals_before_merge {
                c.execute("UPDATE merge_requests SET approvals_before_merge = ?1 WHERE id = ?2", rusqlite::params![n, mr.id])?;
                if labrat_core::services::approvals::mr_rules(c, &mr)?.iter().any(|r| r.is_any_approver()) {
                    failed = true;
                    // The error comes before `AfterCreateService`: the MR is never prepared
                    // (no diff, no open hook) and reads as `cannot_be_merged`.
                    c.execute("UPDATE merge_requests SET creation_failed = 1, merge_status = 'cannot_be_merged' WHERE id = ?1", [mr.id])?;
                    c.execute(
                        "DELETE FROM jobs WHERE kind = 'mr_after_create' AND json_extract(payload, '$.merge_request_id') = ?1",
                        [mr.id],
                    )?;
                } else {
                    labrat_core::services::approvals::set_required(c, &mr, n)?;
                }
            }
        if !cmds.is_empty() {
            let ctx = labrat_core::services::comments::CreateParams { new_record: true, ..Default::default() };
            labrat_core::services::quick_actions::execute(c, &req.app.config, &p, &mr, &me, &cmds, &ctx)?;
        }
        let mr = mr.reload(c)?;
        Ok::<_, ApiError>((single(c, &req, &p, &mr, Opts::default())?, failed))
    })?;
    crate::jobs::kick(&req.app);
    let (body, failed) = body;
    if failed {
        return Err(Error::Api(500, json!("500 Internal Server Error")).into());
    }
    Ok(Resp::created(body))
}

async fn show(req: Req) -> ApiResult {
    let o = Opts {
        render_html: req.params.bool_or("render_html", false)?,
        include_first_contribution: true,
        include_rebase_in_progress: req.params.bool_or("include_rebase_in_progress", false)?,
        include_diverged_commits_count: req
            .params
            .bool_or("include_diverged_commits_count", false)?,
        ..Opts::default()
    };
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        single(c, &req, &p, &mr, o)
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

async fn update(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let keys = [
        "assignee_id",
        "assignee_ids",
        "reviewer_ids",
        "description",
        "labels",
        "add_labels",
        "remove_labels",
        "milestone_id",
        "remove_source_branch",
        "allow_collaboration",
        "allow_maintainer_to_push",
        "squash",
        "target_branch",
        "title",
        "state_event",
        "discussion_locked",
        "merge_after",
    ];
    req.params.require_any(&keys)?;
    for k in ["title", "target_branch"] {
        if req.params.str(k).is_some_and(|v| v.is_empty()) {
            return Err(Error::Param(format!("{k} is empty")).into());
        }
    }
    let mut prm = mr_params(&req)?;
    let cmds = take_quick_actions(&mut prm, req.app.config.is_ee());
    let state_event = prm.state_event.clone();
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        if !mrs::can_update(c, &p, &mr, &me)? {
            return Err(Error::forbidden().into());
        }
        // `handle_quick_actions`: the description's commands are interpreted first and their
        // attribute changes merged into this update's parameters (one update, one set of notes).
        let mut prm = prm;
        let mr = if cmds.is_empty() {
            mr
        } else {
            let ctx = labrat_core::services::comments::CreateParams { return_update: true, ..Default::default() };
            labrat_core::services::quick_actions::execute(c, &req.app.config, &p, &mr, &me, &cmds, &ctx)?;
            prm.merge_commands(labrat_core::services::quick_actions::take_update());
            mr.reload(c)?
        };
        // Clearing `merge_after` destroys the schedule after the MR (with its loaded
        // `merge_schedule`) is presented: the response still shows the old schedule.
        // (An explicit `null`; an unparsable or empty string clears it before.)
        let clear_schedule = matches!(prm.merge_after, Some(None))
            && req.params.raw("merge_after").is_some_and(Value::is_null)
            && mrs::merge_after(c, &mr)?.is_some();
        if clear_schedule {
            prm.merge_after = None;
        }
        let mr = mrs::update(c, &req.app.config, &p, &mr, &me, prm)?;
        let body = single(c, &req, &p, &mr, Opts::default())?;
        if clear_schedule {
            mrs::set_merge_after(c, &mr, None)?;
        }
        Ok::<_, ApiError>(body)
    });
    // GitLab commits the close/reopen before validating the rest of the update, so a failed
    // update still leaves the state change behind.
    if let (Err(e), Some(ev)) = (&result, &state_event)
        && (400..500).contains(&e.status) && e.status != 403 && e.status != 404 {
            let _ = app.db.tx(|c| {
                let (p, _, mr) = load(c, &req)?;
                mrs::apply_state_event(c, &req.app.config, &p, &mr, &me, Some(ev))?;
                Ok::<_, ApiError>(())
            });
        }
    let body = result?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

async fn destroy(req: Req) -> ApiResult {
    req.user()?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms, mr) = load(c, &req)?;
        if !perms.owner() {
            return Err(Error::forbidden().into());
        }
        req.check_unmodified_since(mr.updated_at)?;
        mrs::delete(c, &p, &mr)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn participants(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _, mr) = load(c, &req)?;
        let ids = labrat_core::services::subscriptions::participant_ids(c, &p, &mr, req.user.as_ref())?;
        let users: Vec<User> = ids.into_iter().filter_map(|id| User::find(c, id).ok().flatten()).collect();
        paginate(&req, users, &[], |u| Ok(user_ent::basic_value(u, &ectx(&req))))
    })
}

async fn reviewers(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        let rows = mr.reviewers(c)?;
        paginate(&req, rows, &[], |(_, uid, state, created)| {
            let u = User::get(c, *uid)?;
            Ok(json!({
                "user": user_ent::basic_value(&u, &ectx(&req)),
                "state": ent::reviewer_state_name(*state),
                "created_at": time::iso(*created),
            }))
        })
    })
}

async fn commits(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _, mr) = load(c, &req)?;
        let r = repo::repo(c, &p);
        let list = match mr.latest_diff(c)? {
            Some(d) => d.commits(&r)?,
            None => vec![],
        };
        paginate(&req, list, &[], |o| {
            let cm = r.commit(o)?;
            Ok(Value::Object(repo_ent::commit(&req.app.config, &p, o, &cm)))
        })
    })
}

async fn changes(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        let mut v = single(c, &req, &p, &mr, Opts::default())?;
        let r = repo::repo(c, &p);
        let changes: Vec<Value> = match mr.latest_diff(c)? {
            Some(d) => {
                let mut list = d.changes(&r)?;
                labrat_git::diff::sort_like_gitlab(&mut list);
                let uni = req.params.bool_or("unidiff", false)?;
                // `access_raw_diffs` reads the diffs without collapsing.
                let view = if req.params.bool_or("access_raw_diffs", false)? {
                    repo_ent::MrDiffView::Expanded
                } else {
                    repo_ent::MrDiffView::Collapsing
                };
                let mut rendered = repo_ent::mr_diff_list(&r, d.oids().1, &list, 0, list.len(), view)?;
                if uni {
                    for (v, ch) in rendered.iter_mut().zip(&list) {
                        repo_ent::unidiff(v, ch);
                    }
                }
                rendered
            }
            None => vec![],
        };
        let m = v.as_object_mut().unwrap();
        m.insert("changes".into(), Value::Array(changes));
        m.insert("overflow".into(), json!(false));
        Ok::<_, ApiError>(v)
    })?;
    Ok(Resp::ok(body))
}

async fn diffs(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _, mr) = load(c, &req)?;
        let r = repo::repo(c, &p);
        let latest = mr.latest_diff(c)?;
        let mut changes = match &latest {
            Some(d) => d.changes(&r)?,
            None => vec![],
        };
        labrat_git::diff::sort_like_gitlab(&mut changes);
        let uni = req.params.bool_or("unidiff", false)?;
        let page = crate::pagination::Page::from_req(&req)?;
        let target = latest.as_ref().and_then(|d| d.oids().1);
        let mut rendered = repo_ent::mr_diff_list(&r, target, &changes, page.offset(), page.per_page as usize, repo_ent::MrDiffView::Collapsing)?;
        if uni {
            for (v, ch) in rendered.iter_mut().zip(changes.iter().skip(page.offset())) {
                repo_ent::unidiff(v, ch);
            }
        }
        let total = changes.len();
        Ok(Resp::ok(Value::Array(rendered)).paginated(&req, &page, total, &[("unidiff", "false")]))
    })
}

async fn raw_diffs(req: Req) -> axum::response::Response {
    let app = req.app.clone();
    let result = app.db.read(|c| {
        let (p, _, mr) = load(c, &req)?;
        let r = repo::repo(c, &p);
        let out = match mr.latest_diff(c)? {
            Some(d) => labrat_git::diff::full_patch(&r, &d.changes(&r)?)?,
            None => String::new(),
        };
        Ok::<_, ApiError>(out)
    });
    match result {
        Ok(text) => axum::response::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain; charset=utf-8")
            .header("Content-Disposition", "inline")
            .body(axum::body::Body::from(text))
            .unwrap(),
        Err(e) => axum::response::IntoResponse::into_response(e),
    }
}

async fn pipelines(mut req: Req) -> ApiResult {
    // This endpoint pages by 15 by default.
    if !req.params.has("per_page") {
        req.params.map.insert("per_page".into(), json!("15"));
    }
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _, mr) = load(c, &req)?;
        let list = labrat_core::services::ci::mr_pipelines(c, &p, &mr)?;
        paginate(&req, list, &[], |pl| Ok(Value::Object(crate::routes::pipelines::pipeline_basic(&req.app.config, &p, pl))))
    })
}

async fn empty_list(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        load(c, &req)?;
        paginate(&req, Vec::<Value>::new(), &[], |v| Ok(v.clone()))
    })
}

// ---------------------------------------------------------------------------------------------
// Merge, rebase, auto-merge

async fn do_merge(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let p = &req.params;
    let mp = merge::MergeParams {
        commit_message: p.str("merge_commit_message"),
        squash_commit_message: p.str("squash_commit_message"),
        should_remove_source_branch: p.bool("should_remove_source_branch")?,
        sha: p.str("sha"),
    };
    let auto =
        p.bool_or("merge_when_pipeline_succeeds", false)? || p.bool_or("auto_merge", false)?;
    let squash = p.bool("squash")?;
    let app = req.app.clone();
    // Permission and sha checks, then the `squash` flag, which GitLab saves before deciding
    // whether the MR can be merged (so it sticks even when the merge is refused).
    app.db.tx(|c| {
        let (proj, _, mr) = load(c, &req)?;
        if !mrs::can_be_merged_by(c, &proj, &mr, &me)? {
            return Err(Error::Unauthorized(None).into());
        }
        let head = mr.diff_head_sha(c, &proj)?;
        if let Some(sha) = &mp.sha
            && Some(sha) != head.as_ref() {
                return Err(Error::message(
                    409,
                    format!(
                        "SHA does not match HEAD of source branch: {}",
                        head.unwrap_or_default()
                    ),
                )
                .into());
            }
        if squash == Some(true) {
            c.execute(
                "UPDATE merge_requests SET squash = 1 WHERE id = ?1",
                [mr.id],
            )?;
        }
        Ok::<_, ApiError>(())
    })?;
    // A refused merge still commits what it recorded (e.g. `merge_error`).
    let outcome = app.db.tx(|c| {
        let (proj, _, mr) = load(c, &req)?;
        let cfg = &req.app.config;
        let can_merge_now = mergeability::mergeable(c, cfg, &proj, &mr)?;
        let mr = mr.reload(c)?;
        let not_allowed = || Error::message(405, "405 Method Not Allowed").into();
        let cannot = || Error::message(422, "Branch cannot be merged").into();
        if auto {
            // `MergeWhenChecksPassService#availability_details`: the non-CI checks pass (drafts and
            // threads may wait), and the MR either cannot merge yet or its pipeline is still
            // running.
            let blocking = mergeability::auto_merge_blocking(c, &proj, &mr)?;
            let dhp = labrat_core::services::ci::diff_head_pipeline(c, &proj, &mr)?;
            let in_progress = labrat_core::services::ci::in_progress(&proj, dhp.as_ref());
            let available = blocking.is_none() && !(can_merge_now && !in_progress);
            if available {
                merge::enable_auto_merge(c, &proj, &mr, &me, &mp)?;
            } else if dhp.as_ref().is_some_and(|pl| pl.status == "success") {
                // `pipeline_allows_merge?`: a green pipeline merges right away.
                if !can_merge_now {
                    return Ok(Err(cannot()));
                }
                merge::merge(c, cfg, &proj, &mr, &me, &mp)?;
                if mr.reload(c)?.state != "merged" {
                    return Ok(Err(cannot()));
                }
            } else {
                return Ok::<_, ApiError>(Err(not_allowed()));
            }
        } else if can_merge_now {
            merge::merge(c, cfg, &proj, &mr, &me, &mp)?;
            if mr.reload(c)?.state != "merged" {
                return Ok(Err(cannot()));
            }
        } else {
            return Ok(Err(not_allowed()));
        }
        let mr = mr.reload(c)?;
        Ok(Ok(single(c, &req, &proj, &mr, Opts::default())?))
    })?;
    let body: Value = outcome.map_err(|e: ApiError| e)?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

async fn merge_ref(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        if !mr.is_open() || mergeability::broken(c, &p, &mr)? {
            return Err(Error::message(400, "Merge request is not mergeable").into());
        }
        // `MergeRequests::MergeToRefService` only rewrites the ref when its parents moved.
        let r = repo::repo(c, &p);
        let target = r.branch(&mr.target_branch)?;
        let source = mr.diff_head_sha(c, &p)?.and_then(|h| labrat_git::Oid::from_hex(&h).ok());
        if let (Some(existing), Some(t), Some(s)) = (r.resolve(&format!("refs/merge-requests/{}/merge", mr.iid))?, target, source)
            && r.commit(&existing)?.parents == vec![t, s]
        {
            return Ok::<_, ApiError>(json!({"commit_id": existing.hex()}));
        }
        match mergeability::test_merge(c, &req.app.config, &p, &mr)? {
            Some(sha) => {
                c.execute(
                    "UPDATE merge_requests SET merge_status = 'can_be_merged' WHERE id = ?1",
                    [mr.id],
                )?;
                Ok::<_, ApiError>(json!({"commit_id": sha.hex()}))
            }
            None => {
                c.execute(
                    "UPDATE merge_requests SET merge_status = 'cannot_be_merged' WHERE id = ?1",
                    [mr.id],
                )?;
                Err(Error::message(400, "Merge request is not mergeable").into())
            }
        }
    })?;
    Ok(Resp::ok(body))
}

async fn cancel_auto_merge(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        if !(mrs::can_be_merged_by(c, &p, &mr, &me)? || mr.author_id == me.id) {
            return Err(Error::Unauthorized(None).into());
        }
        if merge::cancel_auto_merge(c, &p, &mr, &me)? {
            Ok::<_, ApiError>(json!({"status": "success"}))
        } else {
            Ok(json!({"message": "Can't cancel the automatic merge", "status": "error", "http_status": 406}))
        }
    })?;
    Ok(Resp::created(body))
}

async fn rebase(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let skip_ci = req.params.bool_or("skip_ci", false)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        let r = repo::repo(c, &p);
        if r.branch(&mr.source_branch)?.is_none() {
            return Err(Error::Forbidden(Some("Source branch does not exist".into())).into());
        }
        if !repo::can_push_to_branch(c, &me, &p, &mr.source_branch)? {
            return Err(Error::Forbidden(Some("Cannot push to source branch".into())).into());
        }
        let rules = repo::matching_rules(c, &p, &mr.source_branch)?;
        if !rules.is_empty() && !rules.iter().any(|x| x.allow_force_push) {
            return Err(Error::Forbidden(Some(
                "Source branch is protected from force push".into(),
            ))
            .into());
        }
        if !mr.is_open() || mr.rebase_in_progress {
            return Err(Error::message(409, "409 Conflict: Resource lock").into());
        }
        merge::request_rebase(c, &mr, &me, skip_ci)?;
        Ok::<_, ApiError>(json!({"rebase_in_progress": true}))
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::status(202, body))
}

// ---------------------------------------------------------------------------------------------
// Versions

async fn versions(req: Req) -> ApiResult {
    req.user()?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        let list = Diff::for_mr(c, mr.id)?;
        paginate(&req, list, &[], |d| Ok(Value::Object(ent::diff_version(d))))
    })
}

async fn version(req: Req) -> ApiResult {
    let vid: i64 = req
        .path_param("version_id")
        .parse()
        .map_err(|_| Error::Param("version_id is invalid".into()))?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _, mr) = load(c, &req)?;
        let d = Diff::find(c, vid)?
            .filter(|d| d.merge_request_id == mr.id)
            .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        let mut v = ent::diff_version_full(c, &req.app.config, &p, &d)?;
        if req.params.bool_or("unidiff", false)? {
            // Apply file headers to each diff, as `use :with_unidiff` does.
            let r = repo::repo(c, &p);
            let mut changes = d.changes(&r)?;
            labrat_git::diff::sort_like_gitlab(&mut changes);
            if let Some(list) = v["diffs"].as_array_mut() {
                for (item, ch) in list.iter_mut().zip(changes.iter()) {
                    repo_ent::unidiff(item, ch);
                }
            }
        }
        Ok::<_, ApiError>(v)
    })?;
    Ok(Resp::ok(body))
}

// ---------------------------------------------------------------------------------------------
// Subscriptions and todos

async fn set_subscription(req: Req, subscribe: bool) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        if !mrs::can_update(c, &p, &mr, &me)?
            && ProjectPerms::load(c, Some(&me), &p)?.level < access::REPORTER
        {
            return Err(Error::forbidden().into());
        }
        let current = labrat_core::services::subscriptions::subscribed_to_mr(c, me.id, &mr)?;
        if current == subscribe {
            return Ok::<_, ApiError>(None);
        }
        labrat_core::services::subscriptions::set(c, me.id, "MergeRequest", mr.id, subscribe)?;
        Ok(Some(single(c, &req, &p, &mr, Opts::default())?))
    })?;
    match result {
        Some(v) => Ok(Resp::created(v)),
        None => Ok(Resp {
            status: axum::http::StatusCode::NOT_MODIFIED,
            body: None,
            headers: vec![],
        }),
    }
}

async fn subscribe(req: Req) -> ApiResult {
    set_subscription(req, true).await
}

async fn unsubscribe(req: Req) -> ApiResult {
    set_subscription(req, false).await
}

async fn todo(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        let pending: i64 = c.query_row(
            "SELECT count(*) FROM todos WHERE user_id = ?1 AND target_type = 'MergeRequest' AND target_id = ?2 AND action = 4 AND state = 'pending'",
            params![me.id, mr.id],
            |r| r.get(0),
        )?;
        if pending > 0 {
            return Ok::<_, ApiError>(None);
        }
        let id = labrat_core::services::todos::create(c, me.id, &me, &p, &mr, labrat_core::services::todos::action::MARKED, None)?;
        Ok(Some(crate::routes::todos::todo_value(c, &req, id)?))
    })?;
    match result {
        Some(v) => Ok(Resp::created(v)),
        None => Ok(Resp {
            status: axum::http::StatusCode::NOT_MODIFIED,
            body: None,
            headers: vec![],
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// Time tracking

fn parse_duration(s: &str) -> Option<i64> {
    labrat_core::chronic::parse_time(s, false).map(|n| n.to_i64())
}

/// GitLab's human duration (`1w 2d 3h`), or null for zero.
pub(crate) fn human_duration(s: i64) -> Value {
    labrat_core::chronic::human_value(s)
}

fn time_stats_value(mr: &MergeRequest) -> Value {
    let human = human_duration;
    json!({
        "time_estimate": mr.time_estimate,
        "total_time_spent": mr.total_time_spent,
        "human_time_estimate": human(mr.time_estimate),
        "human_total_time_spent": human(mr.total_time_spent),
    })
}

/// Sets an MR's time estimate with GitLab's system note.
pub(crate) fn set_time_estimate(c: &Connection, p: &Project, mr: &MergeRequest, me: &User, s: i64) -> labrat_core::Result<()> {
    // `time_estimate=` keeps at most `MAX_INT_VALUE`.
    let s = s.min(i32::MAX as i64);
    if s == mr.time_estimate {
        return Ok(());
    }
    c.execute(
        "UPDATE merge_requests SET updated_at = ?1, updated_by_id = ?2 WHERE id = ?3",
        params![labrat_core::time::now(), me.id, mr.id],
    )?;
    let fmt = |s: i64| -> String { labrat_core::chronic::human(s) };
    c.execute("UPDATE merge_requests SET time_estimate = ?1 WHERE id = ?2", params![s, mr.id])?;
    let body = if mr.time_estimate == 0 {
        format!("added time estimate of {}", fmt(s))
    } else if s == 0 {
        format!("removed time estimate of {}", fmt(mr.time_estimate))
    } else {
        format!("changed time estimate to {} from {}", fmt(s), fmt(mr.time_estimate))
    };
    labrat_core::services::notes::system_note(c, mr.id, p.id, me, &body, "time_tracking")?;
    Ok(())
}

pub(crate) fn parse_duration_pub(s: &str) -> Option<i64> {
    parse_duration(s)
}

async fn time_op(req: Req, op: &'static str) -> ApiResult {
    let me = req.user()?.clone();
    let duration = if op == "estimate" || op == "spend" {
        req.params.require(&["duration"])?;
        req.params.str("duration")
    } else {
        None
    };
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms, mr) = load(c, &req)?;
        if !perms.developer() {
            return Err(Error::forbidden().into());
        }
        // `TimeTrackingFormatter.parse`: the estimate keeps zero, spent time doesn't.
        let estimate_error = || -> ApiError {
            Error::BadRequest(Some("{:reason=>\"Time estimate must have a valid format and be greater than or equal to zero.\"}".into())).into()
        };
        let secs = match (&duration, op) {
            (Some(d), "estimate") => match labrat_core::chronic::parse_time(d, true) {
                Some(n) if !n.is_negative() => Some(n.to_i64()),
                _ => return Err(estimate_error()),
            },
            (Some(d), _) => Some(labrat_core::chronic::parse_time(d, false).map(|n| n.to_i64()).unwrap_or(0)),
            _ => None,
        };
        let mut before = labrat_core::services::merge_requests::snapshot(c, &mr)?;
        let note = |body: String| labrat_core::services::notes::system_note(c, mr.id, p.id, &me, &body, "time_tracking");
        let fmt = |s: i64| -> String { labrat_core::chronic::human(s) };
        match op {
            "estimate" => {
                // `time_estimate=` keeps at most `MAX_INT_VALUE`.
                let s = secs.unwrap().min(i32::MAX as i64);
                set_time_estimate(c, &p, &mr, &me, s)?;
            }
            "reset_estimate" => {
                if mr.time_estimate != 0 {
                    c.execute(
                        "UPDATE merge_requests SET time_estimate = 0, updated_at = ?1, updated_by_id = ?2 WHERE id = ?3",
                        params![labrat_core::time::now(), me.id, mr.id],
                    )?;
                    note(format!("removed time estimate of {}", fmt(mr.time_estimate)))?;
                }
            }
            "spend" => {
                let s = secs.unwrap();
                // An unparsable or zero duration changes nothing (`spend_time` returns early).
                if s == 0 {
                    return Ok(time_stats_value(&mr));
                }
                if let Some(errs) = labrat_core::services::merge_requests::spend_errors(&mr, s) {
                    return Err(Error::Validation(errs).into());
                }
                before.spend = true;
                before.time_change = Some((s, s));
                c.execute("UPDATE merge_requests SET total_time_spent = max(0, total_time_spent + ?1) WHERE id = ?2", params![s, mr.id])?;
                let now = labrat_core::time::now();
                c.execute(
                    "INSERT INTO timelogs (merge_request_id, user_id, time_spent, summary, spent_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                    params![mr.id, me.id, s, req.params.str("summary"), now],
                )?;
                let at = labrat_core::time::ruby_utc(labrat_core::time::now());
                let body = if s >= 0 {
                    format!("added {} of time spent at {}", fmt(s), at)
                } else {
                    format!("subtracted {} of time spent at {}", fmt(s), at)
                };
                note(body)?;
            }
            _ => {
                before.time_change = Some((0, -mr.total_time_spent));
                c.execute("UPDATE merge_requests SET total_time_spent = 0 WHERE id = ?1", [mr.id])?;
                c.execute("DELETE FROM timelogs WHERE merge_request_id = ?1", [mr.id])?;
                note("removed time spent".into())?;
            }
        }
        // Time tracking goes through `UpdateService` (or `AddSpentTimeService`): an `update` hook.
        let after = mr.reload(c)?;
        labrat_core::services::hooks::merge_request_event(c, &req.app.config, &p, &after, Some(&me), "update", Some(&before))?;
        Ok::<_, ApiError>(time_stats_value(&after))
    })?;
    // `add_spent_time` is a POST without an explicit status: 201.
    Ok(if op == "spend" { Resp::created(body) } else { Resp::ok(body) })
}

async fn time_estimate(req: Req) -> ApiResult {
    time_op(req, "estimate").await
}
async fn reset_time_estimate(req: Req) -> ApiResult {
    time_op(req, "reset_estimate").await
}
async fn add_spent_time(req: Req) -> ApiResult {
    time_op(req, "spend").await
}
async fn reset_spent_time(req: Req) -> ApiResult {
    time_op(req, "reset_spent").await
}

async fn time_stats(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        Ok::<_, ApiError>(time_stats_value(&mr))
    })?;
    Ok(Resp::ok(body))
}

// ---------------------------------------------------------------------------------------------
// Resource events

#[allow(dead_code)]
fn page_of<T: Clone>(req: &Req, v: &[T]) -> Result<Vec<T>, ApiError> {
    let page = Page::from_req(req)?;
    Ok(v.iter()
        .skip(page.offset())
        .take(page.per_page as usize)
        .cloned()
        .collect())
}

