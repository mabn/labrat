//! `lib/api/labels.rb` for projects (`Entities::ProjectLabel`).

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::find;
use crate::markdown;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::{get, post};
use labrat_core::model::Project;
use labrat_core::perms::ProjectPerms;
use labrat_core::services::labels::{self, Label};
use labrat_core::services::subscriptions;
use labrat_core::{App, Error, time};
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/labels", get(list).post(create).put(update_by_param).delete(destroy_by_param))
        .route("/projects/{id}/labels/{name}", get(show).put(update).delete(destroy))
        .route("/projects/{id}/labels/{name}/subscribe", post(subscribe))
        .route("/projects/{id}/labels/{name}/promote", axum::routing::put(promote))
        .route("/groups/{id}/labels", get(group_list).post(create).put(update_by_param).delete(destroy_by_param))
        .route("/groups/{id}/labels/{name}", get(show).put(update).delete(destroy))
        .route("/groups/{id}/labels/{name}/subscribe", post(subscribe))
        .route("/groups/{id}/labels/{name}/unsubscribe", post(unsubscribe))
        .route("/projects/{id}/labels/{name}/unsubscribe", post(unsubscribe))
}

/// CSS color names GitLab accepts (`Gitlab::Color::Constants::COLOR_NAME_TO_HEX`, common subset).
const NAMED: &[(&str, &str)] = &[
    ("black", "#000000"), ("white", "#FFFFFF"), ("red", "#FF0000"), ("green", "#008000"), ("blue", "#0000FF"),
    ("yellow", "#FFFF00"), ("orange", "#FFA500"), ("purple", "#800080"), ("gray", "#808080"), ("grey", "#808080"),
    ("pink", "#FFC0CB"), ("brown", "#A52A2A"), ("cyan", "#00FFFF"), ("magenta", "#FF00FF"), ("lime", "#00FF00"),
    ("navy", "#000080"), ("teal", "#008080"), ("olive", "#808000"), ("maroon", "#800000"), ("silver", "#C0C0C0"),
];

/// `Gitlab::Color.of`: `#RGB`/`#RRGGBB` (kept as given) or a CSS color name.
pub(crate) fn color(s: &str) -> Option<String> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix('#') {
        if (hex.len() == 3 || hex.len() == 6) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(t.to_string());
        }
        return None;
    }
    NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(t)).map(|(_, h)| h.to_string())
}

/// Where a label route operates: a project (`lib/api/labels.rb`) or a group
/// (`lib/api/group_labels.rb`).
enum Scope {
    Project(Project, ProjectPerms),
    Group(labrat_core::model::Namespace, bool),
}

impl Scope {
    fn load(c: &Connection, req: &Req) -> Result<Scope, ApiError> {
        if req.path.contains("/groups/") {
            let (g, perms) = find::group(c, req, req.path_param("id"))?;
            let admin = perms.admin || perms.level >= 20;
            Ok(Scope::Group(g, admin))
        } else {
            let (p, perms) = find::project(c, req, req.path_param("id"))?;
            Ok(Scope::Project(p, perms))
        }
    }

    /// `admin_label`: Reporter or above.
    fn admin(&self) -> Result<(), ApiError> {
        let ok = match self {
            Scope::Project(_, perms) => perms.reporter(),
            Scope::Group(_, admin) => *admin,
        };
        if ok { Ok(()) } else { Err(Error::forbidden().into()) }
    }

    fn is_group(&self) -> bool {
        matches!(self, Scope::Group(..))
    }

    /// Labels visible here: the project's or group's own plus its ancestor groups'.
    fn available(&self, c: &Connection) -> labrat_core::Result<Vec<Label>> {
        match self {
            Scope::Project(p, _) => labels::available(c, p),
            Scope::Group(g, _) => {
                let ids = labrat_core::model::Namespace::self_and_ancestor_ids(c, g.id)?.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
                let mut st = c.prepare(&format!("SELECT id FROM labels WHERE group_id IN ({ids}) ORDER BY title, id"))?;
                let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                Ok(ids.into_iter().filter_map(|id| Label::find(c, id).ok().flatten()).collect())
            }
        }
    }

    fn owns(&self, l: &Label) -> bool {
        match self {
            Scope::Project(p, _) => l.project_id == Some(p.id),
            Scope::Group(g, _) => l.group_id == Some(g.id),
        }
    }

    /// `(project_id, group_id)` of a label created here.
    fn ids(&self) -> (Option<i64>, Option<i64>) {
        match self {
            Scope::Project(p, _) => (Some(p.id), None),
            Scope::Group(g, _) => (None, Some(g.id)),
        }
    }
}

fn entity(c: &Connection, req: &Req, scope: &Scope, l: &Label, with_counts: bool) -> Result<Value, ApiError> {
    let mut v = label_entity(c, req, l, with_counts)?;
    if scope.is_group()
        && let Some(m) = v.as_object_mut()
    {
        m.shift_remove("priority");
        m.shift_remove("is_project_label");
    }
    Ok(v)
}

fn label_entity(c: &Connection, req: &Req, l: &Label, with_counts: bool) -> Result<Value, ApiError> {
    let mut m = Map::new();
    m.insert("id".into(), json!(l.id));
    m.insert("name".into(), json!(l.title));
    m.insert("description".into(), json!(l.description));
    m.insert("text_color".into(), json!(l.text_color()));
    let html = match &l.description {
        Some(d) if !d.is_empty() => markdown::render_inline(d),
        _ => String::new(),
    };
    m.insert("description_html".into(), json!(html));
    m.insert("color".into(), json!(l.color));
    m.insert("archived".into(), json!(l.archived));
    if with_counts {
        m.insert("open_issues_count".into(), json!(0));
        m.insert("closed_issues_count".into(), json!(0));
        let n: i64 = c.query_row(
            "SELECT count(*) FROM label_links ll JOIN merge_requests m ON m.id = ll.target_id
             WHERE ll.target_type = 'MergeRequest' AND ll.label_id = ?1 AND m.state = 'opened'",
            [l.id],
            |r| r.get(0),
        )?;
        m.insert("open_merge_requests_count".into(), json!(n));
    }
    let subscribed = match &req.user {
        Some(u) => subscriptions::explicit(c, u.id, "Label", l.id)?.unwrap_or(false),
        None => false,
    };
    m.insert("subscribed".into(), json!(subscribed));
    if l.group_id.is_none() {
        m.insert("priority".into(), json!(l.priority));
    } else {
        m.insert("priority".into(), Value::Null);
    }
    m.insert("is_project_label".into(), json!(l.project_id.is_some()));
    Ok(Value::Object(m))
}

fn project(c: &Connection, req: &Req) -> Result<(Project, ProjectPerms), ApiError> {
    find::project(c, req, req.path_param("id"))
}

fn admin(perms: &ProjectPerms) -> Result<(), ApiError> {
    if perms.reporter() { Ok(()) } else { Err(Error::forbidden().into()) }
}

fn find_label(c: &Connection, scope: &Scope, id_or_title: &str, ancestors: bool) -> Result<Label, ApiError> {
    let all = if ancestors {
        scope.available(c)?
    } else {
        scope.available(c)?.into_iter().filter(|l| scope.owns(l)).collect()
    };
    let by_id = id_or_title.parse::<i64>().ok().and_then(|id| all.iter().find(|l| l.id == id).cloned());
    by_id
        .or_else(|| all.iter().find(|l| l.title == id_or_title).cloned())
        .ok_or_else(|| Error::not_found("Label").into())
}

async fn list(req: Req) -> ApiResult {
    let with_counts = req.params.bool("with_counts")?.unwrap_or(false);
    let ancestors = req.params.bool("include_ancestor_groups")?.unwrap_or(true);
    let search = req.params.str("search");
    let archived = req.params.bool("archived")?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms) = project(c, &req)?;
        let scope = Scope::Project(p.clone(), perms);
        let mut all = labels::available(c, &p)?;
        if !ancestors {
            all.retain(|l| l.project_id == Some(p.id));
        }
        if let Some(q) = &search {
            let q = q.to_lowercase();
            all.retain(|l| l.title.to_lowercase().contains(&q) || l.description.as_deref().unwrap_or("").to_lowercase().contains(&q));
        }
        if let Some(a) = archived {
            all.retain(|l| l.archived == a);
        }
        paginate(&req, all, &[("include_ancestor_groups", "true"), ("with_counts", "false")], |l| entity(c, &req, &scope, l, with_counts))
    })
}

async fn show(req: Req) -> ApiResult {
    let ancestors = req.params.bool("include_ancestor_groups")?.unwrap_or(true);
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let scope = Scope::load(c, &req)?;
        let l = find_label(c, &scope, req.path_param("name"), ancestors)?;
        entity(c, &req, &scope, &l, false)
    })?;
    Ok(Resp::ok(body))
}

async fn create(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["name", "color"])?;
    let name = req.params.str("name").unwrap_or_default();
    let raw_color = req.params.str("color").unwrap_or_default();
    let description = req.params.str("description");
    let priority = req.params.int("priority")?;
    let archived = req.params.bool("archived")?.unwrap_or(false);
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let scope = Scope::load(c, &req)?;
        scope.admin()?;
        if scope.available(c)?.iter().any(|l| l.title == name.trim()) {
            return Err(Error::Api(409, json!("Label already exists")).into());
        }
        let mut errs = vec![];
        let title = name.trim().to_string();
        if title.is_empty() {
            errs.push(("title".to_string(), "can't be blank".to_string()));
        } else if title.contains(',') {
            errs.push(("title".to_string(), "is invalid".to_string()));
        } else if title.chars().count() > 255 {
            errs.push(("title".to_string(), "is too long (maximum is 255 characters)".to_string()));
        }
        let col = color(&raw_color);
        if col.is_none() {
            errs.push(("color".to_string(), "must be a valid color code".to_string()));
        }
        if !errs.is_empty() {
            return Err(Error::Validation(errs).into());
        }
        let (pid, gid) = scope.ids();
        let priority = if scope.is_group() { None } else { priority };
        let l = labels::create(c, pid, gid, &title, &col.unwrap(), description.as_deref(), priority)?;
        if archived {
            c.execute("UPDATE labels SET archived = 1 WHERE id = ?1", [l.id])?;
        }
        let l = Label::find(c, l.id)?.unwrap();
        entity(c, &req, &scope, &l, false)
    })?;
    Ok(Resp::created(body))
}

async fn do_update(req: Req, key: String) -> ApiResult {
    req.user()?;
    let new_name = req.params.str("new_name");
    let raw_color = req.params.str("color");
    let description = req.params.str("description");
    let has_desc = req.params.has("description");
    let has_priority = req.params.has("priority");
    let priority = req.params.int("priority")?;
    let archived = req.params.bool("archived")?;
    let group = req.path.contains("/groups/");
    let has_priority = has_priority && !group;
    if new_name.is_none() && raw_color.is_none() && !has_desc && !has_priority && archived.is_none() {
        let names = if group { "new_name, color, description, archived" } else { "new_name, color, description, priority, archived" };
        return Err(Error::Param(format!("{names} are missing, at least one parameter must be provided")).into());
    }
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let scope = Scope::load(c, &req)?;
        scope.admin()?;
        let l = find_label(c, &scope, &key, false)?;
        let mut errs = vec![];
        if let Some(n) = &new_name {
            let t = n.trim();
            if t.is_empty() {
                errs.push(("title".to_string(), "can't be blank".to_string()));
            } else if scope.available(c)?.iter().any(|o| o.title == t && o.id != l.id) {
                errs.push(("title".to_string(), "has already been taken".to_string()));
            }
        }
        let col = match &raw_color {
            Some(rc) => match color(rc) {
                Some(cc) => Some(cc),
                None => {
                    errs.push(("color".to_string(), "must be a valid color code".to_string()));
                    None
                }
            },
            None => None,
        };
        if !errs.is_empty() {
            return Err(Error::Validation(errs).into());
        }
        let now = time::now();
        if let Some(n) = &new_name {
            c.execute("UPDATE labels SET title = ?1, updated_at = ?2 WHERE id = ?3", params![n.trim(), now, l.id])?;
        }
        if let Some(cc) = &col {
            c.execute("UPDATE labels SET color = ?1, updated_at = ?2 WHERE id = ?3", params![cc, now, l.id])?;
        }
        if has_desc {
            c.execute("UPDATE labels SET description = ?1, updated_at = ?2 WHERE id = ?3", params![description, now, l.id])?;
        }
        if has_priority {
            c.execute("UPDATE labels SET priority = ?1 WHERE id = ?2", params![priority, l.id])?;
        }
        if let Some(a) = archived {
            c.execute("UPDATE labels SET archived = ?1, updated_at = ?2 WHERE id = ?3", params![a, now, l.id])?;
        }
        let l = Label::find(c, l.id)?.unwrap();
        entity(c, &req, &scope, &l, false)
    })?;
    Ok(Resp::ok(body))
}

fn key_param(req: &Req) -> Result<String, ApiError> {
    let id = req.params.str("label_id");
    let name = req.params.str("name");
    match (id, name) {
        (Some(i), None) => Ok(i),
        (None, Some(n)) => Ok(n),
        (Some(_), Some(_)) => Err(Error::Param("label_id, name are mutually exclusive".into()).into()),
        (None, None) => Err(Error::Param("label_id, name are missing, exactly one parameter must be provided".into()).into()),
    }
}

async fn update(req: Req) -> ApiResult {
    let key = req.path_param("name").to_string();
    do_update(req, key).await
}

async fn update_by_param(req: Req) -> ApiResult {
    let key = key_param(&req)?;
    do_update(req, key).await
}

async fn do_destroy(req: Req, key: String) -> ApiResult {
    req.user()?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let scope = Scope::load(c, &req)?;
        let l = find_label(c, &scope, &key, false)?;
        scope.admin()?;
        c.execute("DELETE FROM labels WHERE id = ?1", [l.id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn destroy(req: Req) -> ApiResult {
    let key = req.path_param("name").to_string();
    do_destroy(req, key).await
}

async fn destroy_by_param(req: Req) -> ApiResult {
    let key = key_param(&req)?;
    do_destroy(req, key).await
}

async fn set_subscription(req: Req, on: bool) -> ApiResult {
    let user = req.user()?.clone();
    let app = req.app.clone();
    let res = app.db.tx(|c| {
        let scope = Scope::load(c, &req)?;
        let l = find_label(c, &scope, req.path_param("name"), true)?;
        let current = subscriptions::explicit(c, user.id, "Label", l.id)?.unwrap_or(false);
        if current == on {
            return Ok::<_, ApiError>(None);
        }
        subscriptions::set(c, user.id, "Label", l.id, on)?;
        Ok(Some(entity(c, &req, &scope, &l, false)?))
    })?;
    match res {
        Some(v) => Ok(Resp::created(v)),
        None => Ok(Resp::status(304, Value::Null)),
    }
}

async fn subscribe(req: Req) -> ApiResult {
    set_subscription(req, true).await
}

async fn unsubscribe(req: Req) -> ApiResult {
    set_subscription(req, false).await
}

/// `GroupLabel` entity: no priority or project flag.
fn group_entity(c: &Connection, req: &Req, l: &Label) -> Result<Value, ApiError> {
    let mut v = label_entity(c, req, l, false)?;
    if let Some(m) = v.as_object_mut() {
        m.shift_remove("priority");
        m.shift_remove("is_project_label");
    }
    Ok(v)
}

/// `PUT /projects/:id/labels/:name/promote`: the label becomes a label of the project's group,
/// keeping its merge requests.
async fn promote(req: Req) -> ApiResult {
    req.user()?;
    let name = req.path_param("name").to_string();
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = project(c, &req)?;
        admin(&perms)?;
        let l = find_label(c, &Scope::Project(p.clone(), perms), &name, false)?;
        let ns = labrat_core::model::Namespace::find(c, p.namespace_id)?.ok_or(Error::not_found("Namespace"))?;
        if ns.kind != "Group" {
            return Err(Error::message(400, "Failed to promote label due to internal error. Please contact administrators.").into());
        }
        let g = labels::create(c, None, Some(ns.id), &l.title, &l.color, l.description.as_deref(), None)?;
        c.execute("UPDATE OR IGNORE label_links SET label_id = ?1 WHERE label_id = ?2", rusqlite::params![g.id, l.id])?;
        c.execute("DELETE FROM labels WHERE id = ?1", [l.id])?;
        group_entity(c, &req, &g)
    })?;
    Ok(Resp::ok(body))
}

async fn group_list(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (g, _) = find::group(c, &req, req.path_param("id"))?;
        let mut st = c.prepare("SELECT id FROM labels WHERE group_id = ?1 ORDER BY title, id")?;
        let ids: Vec<i64> = st.query_map([g.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let list: Vec<Label> = ids.into_iter().filter_map(|id| Label::find(c, id).ok().flatten()).collect();
        paginate(
            &req,
            list,
            &[("include_ancestor_groups", "true"), ("include_descendant_groups", "false"), ("only_group_labels", "true"), ("with_counts", "false")],
            |l| group_entity(c, &req, l),
        )
    })
}
