//! MR context commits (`/projects/:id/merge_requests/:iid/context_commits`): commits from outside
//! the MR shown with it, stored by SHA like `MergeRequestContextCommit`.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::repository as repo_ent;
use crate::entities::user::{self as user_ent, Ctx};
use crate::pagination::paginate;
use crate::routes::merge_requests::load;
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Project, User};
use labrat_core::services::merge_requests as mrs;
use labrat_core::{App, Error, repo};
use labrat_git::{Kind, Oid};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route(
        "/projects/{id}/merge_requests/{merge_request_iid}/context_commits",
        get(list).post(create).delete(destroy),
    )
}

/// `requires :commits, type: Array[String], coerce_with: CommaSeparatedToArray, allow_blank: false`.
fn commits_param(req: &Req) -> Result<Vec<String>, ApiError> {
    let list: Vec<String> = match req.params.raw("commits") {
        None => return Err(Error::Param("commits is missing, commits is empty".into()).into()),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect(),
        Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        Some(other) => vec![other.to_string()],
    };
    if list.is_empty() {
        return Err(Error::Param("commits is empty".into()).into());
    }
    Ok(list)
}

/// `repository.commits_by(oids:)`: the commits found, possibly fewer than asked for.
fn find_commits(r: &labrat_git::Repo, ids: &[String]) -> Result<Vec<Oid>, ApiError> {
    let mut out = vec![];
    for id in ids {
        if let Some(o) = r.resolve(id).ok().flatten().filter(|o| r.kind(o).ok().flatten() == Some(Kind::Commit))
            && !out.contains(&o)
        {
            out.push(o);
        }
    }
    Ok(out)
}

fn stored(c: &Connection, mr_id: i64) -> Result<Vec<String>, ApiError> {
    let mut st = c.prepare("SELECT sha FROM merge_request_context_commits WHERE merge_request_id = ?1 ORDER BY id")?;
    let rows = st.query_map([mr_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// `Entities::CommitWithLink` (`type: :full`) of a stored context commit (`to_commit` has no
/// parents or extended trailers).
fn commit_with_link(c: &Connection, req: &Req, p: &Project, oid: &Oid) -> Result<Value, ApiError> {
    let r = repo::repo(c, p);
    let cm = r.commit(oid)?;
    let config = &req.app.config;
    let mut m = repo_ent::commit(config, p, oid, &cm);
    m.insert("parent_ids".into(), Value::Null);
    m.insert("extended_trailers".into(), Value::Null);
    let ctx = Ctx { base_url: &config.external_url, current_user: req.user.as_ref() };
    let author: Option<User> = c
        .query_row("SELECT id FROM users WHERE lower(email) = lower(?1)", [&cm.author.email], |r| r.get::<_, i64>(0))
        .ok()
        .map(|id| User::find(c, id))
        .transpose()?
        .flatten();
    m.insert(
        "author".into(),
        match author {
            Some(u) => {
                let mut a = user_ent::basic(&u, &ctx);
                a.insert("show_status".into(), json!(false));
                a.insert("path".into(), json!(format!("/{}", u.username)));
                Value::Object(a)
            }
            None => Value::Null,
        },
    );
    m.insert("author_gravatar_url".into(), json!(user_ent::avatar_url(&cm.author.email)));
    let base = config.external_url.trim_end_matches('/');
    let path = format!("/{}/-/commit/{}", p.full_path, oid.hex());
    m.insert("commit_url".into(), json!(format!("{base}{path}")));
    m.insert("commit_path".into(), json!(path));
    let mut lines = cm.message.splitn(2, '\n');
    let title = lines.next().unwrap_or("").trim_end().to_string();
    let description = lines.next().unwrap_or("").trim();
    m.insert(
        "description_html".into(),
        json!(if description.is_empty() { String::new() } else { crate::markdown::render_inline(description) }),
    );
    m.insert("title_html".into(), json!(crate::markdown::render_inline(&title)));
    m.insert("signature_html".into(), Value::Null);
    m.insert("prev_commit_id".into(), Value::Null);
    m.insert("next_commit_id".into(), Value::Null);
    m.insert("pipeline_status_path".into(), Value::Null);
    Ok(Value::Object(m))
}

async fn list(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _, mr) = load(c, &req)?;
        let shas = stored(c, mr.id)?;
        paginate(&req, shas, &[], |sha| commit_with_link(c, &req, &p, &Oid::from_hex(sha)?))
    })
}

async fn create(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let ids = commits_param(&req)?;
    // `CONTEXT_COMMITS_POST_LIMIT`, checked before the MR is looked up.
    if ids.len() > 20 {
        return Err(Error::message(400, "Context commits array size should not be more than 20").into());
    }
    let app = req.app.clone();
    let out = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        if !mrs::can_update(c, &p, &mr, &user)? {
            return Err(Error::message(403, "You are not allowed to access the requested resource").into());
        }
        let existing = stored(c, mr.id)?;
        let dups: Vec<String> = existing.into_iter().filter(|e| ids.iter().any(|i| e.starts_with(i.as_str()))).collect();
        if !dups.is_empty() {
            return Err(Error::message(400, format!("Context commits: {dups:?} are already created")).into());
        }
        let r = repo::repo(c, &p);
        let found = find_commits(&r, &ids)?;
        if found.len() != ids.len() {
            return Err(Error::message(400, "One or more context commits' sha is not valid.").into());
        }
        let mut out = vec![];
        for (i, o) in found.iter().enumerate() {
            c.execute(
                "INSERT INTO merge_request_context_commits (merge_request_id, sha, relative_order) VALUES (?1, ?2, ?3)",
                params![mr.id, o.hex(), i as i64],
            )?;
            out.push(Value::Object(repo_ent::commit(&req.app.config, &p, o, &r.commit(o)?)));
        }
        Ok::<_, ApiError>(Value::Array(out))
    })?;
    Ok(Resp::created(out))
}

async fn destroy(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let ids = commits_param(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms, mr) = load(c, &req)?;
        if !(user.admin || perms.level >= labrat_core::model::access::OWNER) {
            return Err(Error::forbidden().into());
        }
        let r = repo::repo(c, &p);
        let found = find_commits(&r, &ids)?;
        if found.len() != ids.len() {
            return Err(Error::message(400, "One or more context commits' sha is not valid.").into());
        }
        for o in found {
            c.execute("DELETE FROM merge_request_context_commits WHERE merge_request_id = ?1 AND sha = ?2", params![mr.id, o.hex()])?;
        }
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}
