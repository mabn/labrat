//! `lib/api/todos.rb`.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::merge_request::{self as mr_ent, Opts};
use crate::entities::project as project_ent;
use crate::entities::user::{self as user_ent, Ctx};
use crate::pagination::paginate;
use axum::Router;
use axum::routing::{get, post};
use labrat_core::model::{Namespace, Project, User};
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::todos::action;
use labrat_core::{App, Error, time};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/todos", get(list))
        .route("/todos/{id}/mark_as_done", post(mark_done))
        .route("/todos/mark_as_done", post(mark_all_done))
}

type Row = (
    i64,
    i64,
    i64,
    Option<i64>,
    String,
    i64,
    i64,
    String,
    i64,
    i64,
);

fn load(c: &Connection, id: i64) -> Result<Option<Row>, ApiError> {
    Ok(c
        .query_row(
            "SELECT id, user_id, author_id, project_id, target_type, target_id, action, state, created_at, updated_at FROM todos WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)),
        )
        .ok())
}

/// `Entities::Todo`.
/// `Entities::Todo`. The target is a full `Entities::MergeRequest`; lists leave out `subscribed`.
pub fn todo_value(c: &Connection, req: &Req, id: i64) -> Result<Value, ApiError> {
    todo_value_with(c, req, id, true)
}

pub fn todo_value_with(c: &Connection, req: &Req, id: i64, include_subscribed: bool) -> Result<Value, ApiError> {
    let Some((id, _uid, author_id, project_id, ttype, tid, act, state, created, updated)) =
        load(c, id)?
    else {
        return Err(Error::not_found("Todo").into());
    };
    let ctx = Ctx {
        base_url: &req.app.config.external_url,
        current_user: req.user.as_ref(),
    };
    let project = match project_id {
        Some(pid) => Project::find(c, pid)?,
        None => None,
    };
    let project_v = match &project {
        Some(p) => {
            let ns = Namespace::find(c, p.namespace_id)?.unwrap();
            json!({"id": p.id, "description": p.description, "name": p.name, "name_with_namespace": p.full_name,
                   "path": p.path, "path_with_namespace": p.full_path, "created_at": time::iso(p.created_at)})
                .as_object()
                .cloned()
                .map(Value::Object)
                .unwrap_or(Value::Null)
                .clone()
                .to_owned()
                .pipe_ns(&ns)
        }
        None => Value::Null,
    };
    let author = User::find(c, author_id)?
        .map(|u| user_ent::basic_value(&u, &ctx))
        .unwrap_or(Value::Null);
    // `Todo#body` / `#target_url`: a to-do from a note shows the note.
    let note: Option<(i64, String)> = c
        .query_row("SELECT n.id, n.note FROM todos t JOIN notes n ON n.id = t.note_id WHERE t.id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok();
    let (target, target_url, body) = match (ttype.as_str(), &project) {
        ("MergeRequest", Some(p)) => match MergeRequest::find(c, tid)? {
            Some(mr) => (
                {
                    // The target is rendered without the current user or project.
                    let mut v = mr_ent::single(c, &req.app.config, None, p, &mr, &Opts { without_project: true, ..Opts::default() })?;
                    if !include_subscribed {
                        v.as_object_mut().map(|m| m.shift_remove("subscribed"));
                    }
                    v
                },
                json!(match &note {
                    Some((nid, _)) => format!("{}#note_{nid}", mr.web_url(&req.app.config, p)),
                    None => mr.web_url(&req.app.config, p),
                }),
                json!(note.as_ref().map(|n| n.1.clone()).unwrap_or(mr.title.clone())),
            ),
            None => (Value::Null, Value::Null, Value::Null),
        },
        ("Commit", Some(p)) => {
            let sha: Option<String> = c.query_row("SELECT commit_id FROM todos WHERE id = ?1", [id], |r| r.get(0)).ok().flatten();
            let r = labrat_core::repo::repo(c, p);
            match sha.and_then(|s| labrat_git::Oid::from_hex(&s).ok()) {
                Some(oid) => {
                    let cm = r.commit(&oid)?;
                    let url = req.app.config.url(&format!("/{}/-/commit/{}", p.full_path, oid.hex()));
                    (
                        Value::Object(crate::entities::repository::commit(&req.app.config, p, &oid, &cm)),
                        json!(match &note {
                            Some((nid, _)) => format!("{url}#note_{nid}"),
                            None => url,
                        }),
                        json!(note.as_ref().map(|n| n.1.clone()).unwrap_or_else(|| cm.message.lines().next().unwrap_or("").to_string())),
                    )
                }
                None => (Value::Null, Value::Null, Value::Null),
            }
        }
        _ => (Value::Null, Value::Null, Value::Null),
    };
    let _ = project_ent::namespace_basic;
    Ok(json!({
        "id": id,
        "project": project_v,
        "author": author,
        "action_name": action::name(act),
        "target_type": ttype,
        "target": target,
        "target_url": target_url,
        "body": body,
        "state": state,
        "created_at": time::iso(created),
        "updated_at": time::iso(updated),
    }))
}

trait PipeNs {
    fn pipe_ns(self, ns: &Namespace) -> Value;
}

impl PipeNs for Value {
    fn pipe_ns(self, _ns: &Namespace) -> Value {
        self
    }
}

async fn list(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let p = &req.params;
    let state = p
        .one_of("state", &["pending", "done"])?
        .unwrap_or("pending".into());
    // `Todo.action_names` and `TodosFinder::TODO_TYPES`.
    let action_filter = p.one_of(
        "action",
        &[
            "assigned", "review_requested", "mentioned", "build_failed", "marked", "approval_required", "unmergeable", "directly_addressed",
            "member_access_requested", "review_submitted", "ssh_key_expired", "ssh_key_expiring_soon",
        ],
    )?;
    let ttype = p.one_of(
        "type",
        &["Commit", "Issue", "WorkItem", "MergeRequest", "DesignManagement::Design", "AlertManagement::Alert", "Namespace", "Project", "Key", "WikiPage::Meta"],
    )?;
    let project_id = p.int("project_id")?;
    let author_id = p.int("author_id")?;
    let app = req.app.clone();
    app.db.read(|c| {
        // Pending lists leave out snoozed to-dos (`not_snoozed`).
        let mut st = c.prepare(
            "SELECT id, action, target_type, project_id, author_id FROM todos WHERE user_id = ?1 AND state = ?2
             AND (?2 != 'pending' OR snoozed_until IS NULL OR snoozed_until <= ?3) ORDER BY id DESC",
        )?;
        let rows: Vec<(i64, i64, String, Option<i64>, i64)> = st
            .query_map(params![me.id, state, time::now()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let ids: Vec<i64> = rows
            .into_iter()
            .filter(|(_, a, t, pid, aid)| {
                action_filter.as_deref().is_none_or(|f| action::name(*a) == f)
                    && ttype.as_deref().is_none_or(|x| x == t)
                    && project_id.is_none_or(|x| Some(x) == *pid)
                    && author_id.is_none_or(|x| x == *aid)
            })
            .map(|r| r.0)
            .collect();
        paginate(&req, ids, &[], |id| todo_value_with(c, &req, *id, false))
    })
}

async fn mark_done(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id: i64 = req
        .path_param("id")
        .parse()
        .map_err(|_| Error::Param("id is invalid".into()))?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let n = c.execute(
            "UPDATE todos SET state = 'done', updated_at = ?1 WHERE id = ?2 AND user_id = ?3",
            params![time::now(), id, me.id],
        )?;
        if n == 0 {
            return Err(Error::Api(404, json!("404 Not found")).into());
        }
        todo_value(c, &req, id)
    })?;
    Ok(Resp::created(body))
}

async fn mark_all_done(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    app.db.tx(|c| {
        c.execute("UPDATE todos SET state = 'done', updated_at = ?1 WHERE user_id = ?2 AND state = 'pending'", params![time::now(), me.id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

