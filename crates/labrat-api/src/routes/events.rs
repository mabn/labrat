//! Contribution events (`lib/api/events.rb`): `/events`, `/users/:id/events`, `/projects/:id/events`.

use crate::ctx::{ApiError, ApiResult, Req};
use crate::entities::user::{self as user_ent, Ctx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::notes::Note;
use labrat_core::{App, Error, time};
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/events", get(mine))
        .route("/users/{id}/events", get(for_user))
        .route("/projects/{id}/events", get(for_project))
}

struct Row {
    id: i64,
    project_id: Option<i64>,
    author_id: i64,
    action: String,
    target_type: Option<String>,
    target_id: Option<i64>,
    push_data: Option<String>,
    created_at: i64,
}

const ACTIONS: &[&str] = &["created", "updated", "closed", "reopened", "pushed", "commented", "merged", "joined", "left", "destroyed", "expired", "approved"];
const TARGETS: &[&str] = &["issue", "milestone", "merge_request", "note", "project", "snippet", "user", "wiki", "design"];

fn load(c: &Connection, where_: &str, arg: i64) -> Result<Vec<Row>, ApiError> {
    let mut st = c.prepare(&format!(
        "SELECT id, project_id, author_id, action, target_type, target_id, push_data, created_at FROM events WHERE {where_} ORDER BY id DESC"
    ))?;
    let rows = st
        .query_map([arg], |r| {
            Ok(Row {
                id: r.get(0)?,
                project_id: r.get(1)?,
                author_id: r.get(2)?,
                action: r.get(3)?,
                target_type: r.get(4)?,
                target_id: r.get(5)?,
                push_data: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Filters common to the three endpoints, and visibility of each event's project.
fn filter(c: &Connection, req: &Req, rows: Vec<Row>) -> Result<Vec<Row>, ApiError> {
    // `action` isn't validated: an unknown value just doesn't filter.
    let action = req.params.str("action").filter(|a| ACTIONS.contains(&a.as_str()));
    let target = req.params.one_of("target_type", TARGETS)?;
    let before = req.params.str("before").and_then(|d| time::parse_date(&d));
    let after = req.params.str("after").and_then(|d| time::parse_date(&d));
    let asc = req.params.one_of("sort", &["asc", "desc"])?.as_deref() == Some("asc");
    let mut out = vec![];
    for r in rows {
        if action.as_ref().is_some_and(|a| *a != r.action) {
            continue;
        }
        if let Some(t) = &target {
            let tt = r.target_type.as_deref().unwrap_or("");
            let ok = match t.as_str() {
                "merge_request" => tt == "MergeRequest",
                "note" => tt == "Note",
                "project" => tt == "Project",
                _ => false,
            };
            if !ok {
                continue;
            }
        }
        let day = chrono::DateTime::from_timestamp_millis(r.created_at).unwrap_or_default().date_naive();
        if before.is_some_and(|b| day >= b) || after.is_some_and(|a| day <= a) {
            continue;
        }
        if let Some(pid) = r.project_id {
            let Some(p) = Project::find(c, pid)? else { continue };
            if !ProjectPerms::load(c, req.user.as_ref(), &p)?.read_project() {
                continue;
            }
        }
        out.push(r);
    }
    if asc {
        out.reverse();
    }
    Ok(out)
}

fn entity(c: &Connection, req: &Req, r: &Row, with_note: bool) -> Result<Value, ApiError> {
    let ctx = Ctx { base_url: &req.app.config.external_url, ee: req.app.config.is_ee(), current_user: req.user.as_ref() };
    let author = User::find(c, r.author_id)?;
    let push: Option<Value> = r.push_data.as_deref().and_then(|s| serde_json::from_str(s).ok());
    let tt = r.target_type.clone();
    let note = match tt.as_deref() {
        Some("Note" | "DiffNote" | "DiscussionNote" | "LegacyDiffNote") => r.target_id.map(|id| Note::find(c, id)).transpose()?.flatten(),
        _ => None,
    };
    let mr = match (tt.as_deref(), &note) {
        (Some("MergeRequest"), _) => r.target_id.map(|id| MergeRequest::find(c, id)).transpose()?.flatten(),
        (_, Some(n)) if n.noteable_type == "MergeRequest" => MergeRequest::find(c, n.noteable_id)?,
        _ => None,
    };
    let project = r.project_id.map(|id| Project::find(c, id)).transpose()?.flatten();
    let action_name = match r.action.as_str() {
        "pushed" => {
            let a = push.as_ref().and_then(|p| p["action"].as_str()).unwrap_or("pushed");
            match a {
                "created" => "pushed new",
                "removed" => "deleted",
                _ => "pushed to",
            }
        }
        "closed" => "closed",
        "merged" => "accepted",
        "joined" => "joined",
        "left" => "left",
        "destroyed" => "destroyed",
        "commented" => "commented on",
        "approved" => "approved",
        "created" if tt.as_deref() == Some("Project") => "created",
        _ => "opened",
    };
    // Comments on commits: the title is the commit's.
    let commit_title: Option<String> = match (&note, &project) {
        (Some(n), Some(p)) if n.noteable_type == "Commit" => n
            .commit_id
            .as_deref()
            .and_then(|s| labrat_git::Oid::from_hex(s).ok())
            .and_then(|o| labrat_core::repo::repo(c, p).commit(&o).ok())
            .map(|cm| cm.message.lines().next().unwrap_or("").to_string()),
        _ => None,
    };
    let (target_iid, target_title) = match tt.as_deref() {
        Some(_) if commit_title.is_some() => (json!(r.target_id), json!(commit_title)),
        Some("MergeRequest") => (mr.as_ref().map(|m| json!(m.iid)).unwrap_or(json!(r.target_id)), json!(mr.as_ref().map(|m| m.title.clone()))),
        Some("Project") => (json!(r.target_id), json!(project.as_ref().map(|p| p.name.clone()))),
        Some(_) if note.is_some() => (json!(r.target_id), json!(mr.as_ref().map(|m| m.title.clone()))),
        _ => (json!(r.target_id), Value::Null),
    };
    let mut m = Map::new();
    m.insert("id".into(), json!(r.id));
    m.insert("project_id".into(), json!(r.project_id));
    m.insert("action_name".into(), json!(action_name));
    m.insert("target_id".into(), json!(r.target_id));
    m.insert("target_iid".into(), target_iid);
    m.insert("target_type".into(), json!(tt));
    m.insert("author_id".into(), json!(r.author_id));
    m.insert("target_title".into(), target_title);
    m.insert("created_at".into(), json!(time::iso(r.created_at)));
    let _ = with_note;
    if let (Some(n), Some(mr), Some(p)) = (&note, &mr, &project) {
        let v = crate::entities::note::note(c, &crate::routes::notes::ectx(req), p, mr, n)?;
        m.insert("note".into(), v);
    } else if let Some(n) = note.as_ref().filter(|n| n.noteable_type == "Commit") {
        m.insert("note".into(), crate::entities::note::commit_note(c, &crate::routes::notes::ectx(req), n)?);
    }
    if let Some(a) = &author {
        m.insert("author".into(), user_ent::basic_value(a, &ctx));
    }
    m.insert("imported".into(), json!(false));
    m.insert("imported_from".into(), json!("none"));
    if let Some(pd) = push {
        m.insert("push_data".into(), pd);
    }
    m.insert("author_username".into(), json!(author.map(|a| a.username)));
    Ok(Value::Object(m))
}

async fn mine(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let all = req.params.str("scope").as_deref() == Some("all");
    let app = req.app.clone();
    app.db.read(|c| {
        let rows = if all { load(c, "?1 = ?1", 0)? } else { load(c, "author_id = ?1", me.id)? };
        let rows = filter(c, &req, rows)?;
        paginate(&req, rows, &[("sort", "desc")], |r| entity(c, &req, r, false))
    })
}

async fn for_user(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let key = req.path_param("id").to_string();
        let user = match key.parse::<i64>() {
            Ok(id) => User::find(c, id)?,
            Err(_) => User::find_by_username(c, &key)?,
        }
        .ok_or(Error::not_found("User"))?;
        let rows = filter(c, &req, load(c, "author_id = ?1", user.id)?)?;
        paginate(&req, rows, &[("sort", "desc")], |r| entity(c, &req, r, false))
    })
}

async fn for_project(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = find::project(c, &req, req.path_param("id"))?;
        let rows = filter(c, &req, load(c, "project_id = ?1", p.id)?)?;
        let _ = params![];
        paginate(&req, rows, &[("sort", "desc")], |r| entity(c, &req, r, true))
    })
}
