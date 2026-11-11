//! Project milestones (`lib/api/project_milestones.rb`) and MR resource events
//! (`lib/api/resource_{label,state,milestone}_events.rb`).

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::merge_request as mr_ent;
use crate::entities::user::{self as user_ent, Ctx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::labels::Label;
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::{App, Error, time};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    let mr = "/projects/{id}/merge_requests/{eventable_id}";
    Router::new()
        .route("/projects/{id}/milestones", get(list).post(create))
        .route("/groups/{id}/milestones", get(group_list).post(group_create))
        .route("/groups/{id}/milestones/{milestone_id}", get(group_show).put(update).delete(destroy))
        .route("/groups/{id}/milestones/{milestone_id}/merge_requests", get(merge_requests))
        .route("/projects/{id}/milestones/{milestone_id}", get(show).put(update).delete(destroy))
        .route("/projects/{id}/milestones/{milestone_id}/merge_requests", get(merge_requests))
        .route("/projects/{id}/milestones/{milestone_id}/issues", get(milestone_issues))
        .route("/projects/{id}/milestones/{milestone_id}/promote", axum::routing::post(promote))
        .route(&format!("{mr}/resource_label_events"), get(label_events))
        .route(&format!("{mr}/resource_label_events/{{event_id}}"), get(label_event))
        .route(&format!("{mr}/resource_state_events"), get(state_events))
        .route(&format!("{mr}/resource_state_events/{{event_id}}"), get(state_event))
        .route(&format!("{mr}/resource_milestone_events"), get(milestone_events))
        .route(&format!("{mr}/resource_milestone_events/{{event_id}}"), get(milestone_event))
}

fn project(c: &Connection, req: &Req) -> Result<(Project, ProjectPerms), ApiError> {
    find::project(c, req, req.path_param("id"))
}

fn milestone_id(req: &Req) -> Result<i64, ApiError> {
    req.path_param("milestone_id").parse().map_err(|_| Error::Param("milestone_id is invalid".into()).into())
}

fn find_milestone(c: &Connection, p: &Project, id: i64) -> Result<i64, ApiError> {
    let found: Option<i64> = c
        .query_row("SELECT id FROM milestones WHERE id = ?1 AND project_id = ?2", params![id, p.id], |r| r.get(0))
        .ok();
    found.ok_or_else(|| Error::Api(404, json!("404 Not found")).into())
}

fn valid_date(s: &str) -> bool {
    time::parse_date(s).is_some()
}

async fn list(req: Req) -> ApiResult {
    let state = req.params.one_of("state", &["active", "closed", "all"])?;
    let title = req.params.str("title");
    let search = req.params.str("search").map(|s| s.to_lowercase());
    let iids = req.params.int_list("iids")?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = project(c, &req)?;
        let mut st = c.prepare(
            "SELECT id, iid, title, description, state FROM milestones WHERE project_id = ?1 ORDER BY due_date IS NULL, due_date, id DESC",
        )?;
        let rows: Vec<(i64, i64, String, Option<String>, String)> = st
            .query_map([p.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let ids: Vec<i64> = rows
            .into_iter()
            .filter(|(_, iid, t, d, s)| {
                state.as_deref().is_none_or(|x| x == "all" || x == s)
                    && title.as_ref().is_none_or(|x| x == t)
                    && iids.as_ref().is_none_or(|v| v.contains(iid))
                    && search.as_ref().is_none_or(|q| {
                        t.to_lowercase().contains(q) || d.as_deref().unwrap_or("").to_lowercase().contains(q)
                    })
            })
            .map(|r| r.0)
            .collect();
        paginate(&req, ids, &[("include_ancestors", "false"), ("state", "all")], |id| Ok(mr_ent::milestone(c, &req.app.config, *id)?))
    })
}

async fn show(req: Req) -> ApiResult {
    let id = milestone_id(&req)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = project(c, &req)?;
        let id = find_milestone(c, &p, id)?;
        Ok::<_, ApiError>(mr_ent::milestone(c, &req.app.config, id)?)
    })?;
    Ok(Resp::ok(body))
}

/// Ruby's `errors.messages.inspect`: `{:title=>["has already been taken"]}`.
fn messages_inspect(errs: &[(&str, &str)]) -> String {
    let parts: Vec<String> = errs.iter().map(|(k, v)| format!(":{k}=>[\"{v}\"]")).collect();
    format!("{{{}}}", parts.join(", "))
}

async fn create(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["title"])?;
    let title = req.params.str("title").unwrap_or_default();
    let description = req.params.str("description");
    let due = req.params.str("due_date");
    let start = req.params.str("start_date");
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = project(c, &req)?;
        if !perms.reporter() {
            return Err(Error::forbidden().into());
        }
        let mut errs = vec![];
        if title.trim().is_empty() {
            errs.push(("title", "can't be blank"));
        }
        let taken: i64 = c.query_row("SELECT count(*) FROM milestones WHERE project_id = ?1 AND title = ?2", params![p.id, title.trim()], |r| r.get(0))?;
        if taken > 0 {
            errs.push(("title", "already being used for another group or project milestone."));
        }
        if due.as_deref().is_some_and(|d| !d.is_empty() && !valid_date(d)) || start.as_deref().is_some_and(|d| !d.is_empty() && !valid_date(d)) {
            errs.push(("base", "Date is not valid"));
        }
        if let (Some(s), Some(d)) = (&start, &due)
            && valid_date(s) && valid_date(d) && d < s {
                errs.push(("due_date", "must be greater than start date"));
            }
        if !errs.is_empty() {
            return Err(Error::message(400, format!("Failed to create milestone {}", messages_inspect(&errs))).into());
        }
        let iid = labrat_core::services::merge_requests::next_iid(c, "milestones", p.id)?;
        let now = time::now();
        c.execute(
            "INSERT INTO milestones (iid, title, description, project_id, state, due_date, start_date, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7, ?7)",
            params![iid, title.trim(), description, p.id, due.filter(|d| !d.is_empty()), start.filter(|d| !d.is_empty()), now],
        )?;
        Ok::<_, ApiError>(mr_ent::milestone(c, &req.app.config, c.last_insert_rowid())?)
    })?;
    Ok(Resp::created(body))
}

/// The milestone of a `/projects/:id/milestones/:milestone_id` or
/// `/groups/:id/milestones/:milestone_id` route, whether the user may change it (Reporter),
/// and the project for project routes.
fn target(c: &Connection, req: &Req) -> Result<(i64, bool, Option<(Project, ProjectPerms)>), ApiError> {
    let id = milestone_id(req)?;
    if req.path.contains("/groups/") {
        let (g, perms) = group_of(c, req)?;
        let found: Option<i64> = c.query_row("SELECT id FROM milestones WHERE id = ?1 AND group_id = ?2", params![id, g.id], |r| r.get(0)).ok();
        let id = found.ok_or_else(|| ApiError::from(Error::Api(404, json!("404 Not found"))))?;
        Ok((id, perms.admin || perms.level >= labrat_core::model::access::REPORTER, None))
    } else {
        let (p, perms) = project(c, req)?;
        let id = find_milestone(c, &p, id)?;
        Ok((id, perms.reporter(), Some((p, perms))))
    }
}

async fn update(req: Req) -> ApiResult {
    req.user()?;
    let state_event = req.params.one_of("state_event", &["close", "activate"])?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (id, admin, _) = target(c, &req)?;
        if !admin {
            return Err(Error::forbidden().into());
        }
        let now = time::now();
        let (cur_start, cur_due): (Option<String>, Option<String>) =
            c.query_row("SELECT start_date, due_date FROM milestones WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let date = |k: &str, cur: Option<String>| if req.params.has(k) { req.params.str(k).filter(|s| !s.is_empty()) } else { cur };
        let (start, due) = (date("start_date", cur_start), date("due_date", cur_due));
        if let (Some(s), Some(d)) = (&start, &due)
            && valid_date(s)
            && valid_date(d)
            && d < s
        {
            return Err(Error::message(400, format!("Failed to update milestone {}", messages_inspect(&[("due_date", "must be greater than start date")]))).into());
        }
        for (k, col) in [("title", "title"), ("description", "description"), ("due_date", "due_date"), ("start_date", "start_date")] {
            if req.params.has(k) {
                let v = req.params.str(k).filter(|s| !(col.ends_with("date") && s.is_empty()));
                c.execute(&format!("UPDATE milestones SET {col} = ?1, updated_at = ?2 WHERE id = ?3"), params![v, now, id])?;
            }
        }
        if let Some(ev) = &state_event {
            let s = if ev == "close" { "closed" } else { "active" };
            c.execute("UPDATE milestones SET state = ?1, updated_at = ?2 WHERE id = ?3", params![s, now, id])?;
        }
        Ok::<_, ApiError>(mr_ent::milestone(c, &req.app.config, id)?)
    })?;
    Ok(Resp::ok(body))
}

async fn destroy(req: Req) -> ApiResult {
    req.user()?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (id, admin, _) = target(c, &req)?;
        if !admin {
            return Err(Error::forbidden().into());
        }
        c.execute("UPDATE merge_requests SET milestone_id = NULL WHERE milestone_id = ?1", [id])?;
        c.execute("DELETE FROM resource_milestone_events WHERE milestone_id = ?1", [id])?;
        c.execute("DELETE FROM milestones WHERE id = ?1", [id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn merge_requests(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (id, _, proj) = target(c, &req)?;
        let list = MergeRequest::list_where(c, "milestone_id = ?1 ORDER BY id DESC", [id])?;
        let mut rows: Vec<(Project, MergeRequest)> = vec![];
        match proj {
            Some((p, perms)) => {
                if !perms.read_merge_request(&p) {
                    return Err(Error::forbidden().into());
                }
                rows = list.into_iter().map(|m| (p.clone(), m)).collect();
            }
            None => {
                for m in list {
                    let Some(p) = Project::find(c, m.target_project_id)? else { continue };
                    if ProjectPerms::load(c, req.user.as_ref(), &p)?.read_merge_request(&p) {
                        rows.push((p, m));
                    }
                }
            }
        }
        crate::routes::merge_requests::render_basic_list(c, &req, rows)
    })
}

// ---------------------------------------------------------------------------------------------
// Resource events

fn eventable(c: &Connection, req: &Req) -> Result<(Project, MergeRequest), ApiError> {
    let (p, perms) = project(c, req)?;
    if !perms.read_merge_request(&p) {
        return Err(Error::forbidden().into());
    }
    let iid: i64 = req.path_param("eventable_id").parse().map_err(|_| Error::Param("eventable_id is invalid".into()))?;
    let mr = MergeRequest::find_by_iid(c, p.id, iid)?.ok_or(Error::not_found("Merge Request"))?;
    Ok((p, mr))
}

fn user_value(c: &Connection, req: &Req, id: Option<i64>) -> Result<Value, ApiError> {
    let ctx = Ctx { base_url: &req.app.config.external_url, current_user: req.user.as_ref() };
    Ok(match id.map(|i| User::find(c, i)).transpose()?.flatten() {
        Some(u) => user_ent::basic_value(&u, &ctx),
        None => Value::Null,
    })
}

type Row = (i64, Option<i64>, i64, Value);

fn events(c: &Connection, req: &Req, table: &str, mr: &MergeRequest) -> Result<Vec<Row>, ApiError> {
    let sql = match table {
        "resource_label_events" => "SELECT id, user_id, created_at, label_id, action, NULL FROM resource_label_events WHERE merge_request_id = ?1 ORDER BY id",
        "resource_state_events" => "SELECT id, user_id, created_at, source_commit, state, source_merge_request_id FROM resource_state_events WHERE merge_request_id = ?1 ORDER BY id",
        _ => "SELECT id, user_id, created_at, milestone_id, action, state FROM resource_milestone_events WHERE merge_request_id = ?1 ORDER BY id",
    };
    let mut st = c.prepare(sql)?;
    let raw: Vec<(i64, Option<i64>, i64, Value, Value, Value)> = st
        .query_map([mr.id], |r| {
            let v = |i: usize| -> rusqlite::Result<Value> {
                Ok(match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    _ => Value::Null,
                })
            };
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, v(3)?, v(4)?, v(5)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for (id, user, created, a, b, d) in raw {
        let mut m = serde_json::Map::new();
        m.insert("id".into(), json!(id));
        m.insert("user".into(), user_value(c, req, user)?);
        m.insert("created_at".into(), json!(time::iso(created)));
        m.insert("resource_type".into(), json!("MergeRequest"));
        m.insert("resource_id".into(), json!(mr.id));
        match table {
            "resource_label_events" => {
                let label = a.as_i64().map(|i| Label::find(c, i)).transpose()?.flatten();
                m.insert("label".into(), label.map(|l| mr_ent::label_basic(&l)).unwrap_or(Value::Null));
                m.insert("action".into(), b);
            }
            "resource_state_events" => {
                m.insert("source_commit".into(), a);
                m.insert("source_merge_request_id".into(), d);
                m.insert("state".into(), b);
            }
            _ => {
                let ms = match a.as_i64() {
                    Some(i) => mr_ent::milestone(c, &req.app.config, i)?,
                    None => Value::Null,
                };
                m.insert("milestone".into(), ms);
                m.insert("action".into(), b);
                m.insert("state".into(), d);
            }
        }
        out.push((id, user, created, Value::Object(m)));
    }
    Ok(out)
}

async fn list_events(req: Req, table: &'static str) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (_, mr) = eventable(c, &req)?;
        let rows = events(c, &req, table, &mr)?;
        paginate(&req, rows, &[], |r| Ok(r.3.clone()))
    })
}

async fn show_event(req: Req, table: &'static str) -> ApiResult {
    let eid: i64 = req.path_param("event_id").parse().map_err(|_| Error::Param("event_id is invalid".into()))?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (_, mr) = eventable(c, &req)?;
        let rows = events(c, &req, table, &mr)?;
        rows.into_iter().find(|r| r.0 == eid).map(|r| r.3).ok_or_else(|| ApiError::from(Error::Api(404, json!("404 Not found"))))
    })?;
    Ok(Resp::ok(body))
}

async fn label_events(req: Req) -> ApiResult {
    list_events(req, "resource_label_events").await
}
async fn label_event(req: Req) -> ApiResult {
    show_event(req, "resource_label_events").await
}
async fn state_events(req: Req) -> ApiResult {
    list_events(req, "resource_state_events").await
}
async fn state_event(req: Req) -> ApiResult {
    show_event(req, "resource_state_events").await
}
async fn milestone_events(req: Req) -> ApiResult {
    list_events(req, "resource_milestone_events").await
}
async fn milestone_event(req: Req) -> ApiResult {
    show_event(req, "resource_milestone_events").await
}

// ---------------------------------------------------------------------------------------------
// Group milestones (`lib/api/group_milestones.rb`).

fn group_of(c: &Connection, req: &Req) -> Result<(labrat_core::model::Namespace, labrat_core::perms::GroupPerms), ApiError> {
    crate::find::group(c, req, req.path_param("id"))
}

async fn group_create(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["title"])?;
    let title = req.params.str("title").unwrap_or_default();
    let description = req.params.str("description");
    let due = req.params.str("due_date");
    let start = req.params.str("start_date");
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (g, perms) = group_of(c, &req)?;
        if !(perms.admin || perms.level >= labrat_core::model::access::REPORTER) {
            return Err(Error::forbidden().into());
        }
        let mut errs = vec![];
        if title.trim().is_empty() {
            errs.push(("title", "can't be blank"));
        }
        let taken: i64 = c.query_row("SELECT count(*) FROM milestones WHERE group_id = ?1 AND title = ?2", params![g.id, title.trim()], |r| r.get(0))?;
        if taken > 0 {
            errs.push(("title", "already being used for another group or project milestone."));
        }
        if due.as_deref().is_some_and(|d| !d.is_empty() && !valid_date(d)) || start.as_deref().is_some_and(|d| !d.is_empty() && !valid_date(d)) {
            errs.push(("base", "Date is not valid"));
        }
        if !errs.is_empty() {
            return Err(Error::message(400, format!("Failed to create milestone {}", messages_inspect(&errs))).into());
        }
        // Group milestone iids are counted per group (negative key keeps them apart from projects).
        let iid = labrat_core::services::merge_requests::next_iid(c, "group_milestones", -g.id)?;
        let now = time::now();
        c.execute(
            "INSERT INTO milestones (iid, title, description, group_id, state, due_date, start_date, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7, ?7)",
            params![iid, title.trim(), description, g.id, due.filter(|d| !d.is_empty()), start.filter(|d| !d.is_empty()), now],
        )?;
        Ok::<_, ApiError>(mr_ent::milestone(c, &req.app.config, c.last_insert_rowid())?)
    })?;
    Ok(Resp::created(body))
}

async fn group_list(req: Req) -> ApiResult {
    let state = req.params.one_of("state", &["active", "closed", "all"])?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (g, _) = group_of(c, &req)?;
        let mut st = c.prepare("SELECT id, state FROM milestones WHERE group_id = ?1 ORDER BY due_date IS NULL, due_date, id DESC")?;
        let rows: Vec<(i64, String)> = st.query_map([g.id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let ids: Vec<i64> = rows.into_iter().filter(|(_, s)| state.as_deref().is_none_or(|x| x == "all" || x == s)).map(|r| r.0).collect();
        paginate(&req, ids, &[("include_ancestors", "false"), ("state", "all")], |id| Ok(mr_ent::milestone(c, &req.app.config, *id)?))
    })
}

async fn group_show(req: Req) -> ApiResult {
    let id = milestone_id(&req)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (g, _) = group_of(c, &req)?;
        let found: i64 = c.query_row("SELECT count(*) FROM milestones WHERE id = ?1 AND group_id = ?2", params![id, g.id], |r| r.get(0))?;
        if found == 0 {
            return Err(Error::not_found("Milestone").into());
        }
        Ok::<_, ApiError>(mr_ent::milestone(c, &req.app.config, id)?)
    })?;
    Ok(Resp::ok(body))
}

/// Issues of a milestone: labrat has no issues.
async fn milestone_issues(req: Req) -> ApiResult {
    let id = milestone_id(&req)?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = project(c, &req)?;
        find_milestone(c, &p, id)?;
        paginate::<Value>(&req, vec![], &[], |v| Ok(v.clone()))
    })
}

/// `POST .../milestones/:id/promote`: the milestone moves to the project's group. GitLab answers
/// with a bare `200` body.
async fn promote(req: Req) -> ApiResult {
    req.user()?;
    let id = milestone_id(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms) = project(c, &req)?;
        if !perms.reporter() {
            return Err(Error::forbidden().into());
        }
        find_milestone(c, &p, id)?;
        let ns = labrat_core::model::Namespace::find(c, p.namespace_id)?.ok_or(Error::not_found("Namespace"))?;
        if ns.kind != "Group" {
            return Err(Error::message(400, "Project does not belong to a group.").into());
        }
        // `Milestones::PromoteService`: a new group milestone takes over, the old one goes away.
        let iid = labrat_core::services::merge_requests::next_iid(c, "group_milestones", -ns.id)?;
        let now = time::now();
        c.execute(
            "INSERT INTO milestones (iid, title, description, group_id, state, due_date, start_date, created_at, updated_at)
             SELECT ?1, title, description, ?2, state, due_date, start_date, ?3, ?3 FROM milestones WHERE id = ?4",
            params![iid, ns.id, now, id],
        )?;
        let new_id = c.last_insert_rowid();
        c.execute("UPDATE merge_requests SET milestone_id = ?1 WHERE milestone_id = ?2", params![new_id, id])?;
        c.execute("DELETE FROM milestones WHERE id = ?1", [id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::ok(json!(200)))
}
