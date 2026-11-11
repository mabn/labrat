//! Project templates (`lib/api/project_templates.rb`): issue and merge request description
//! templates from `.gitlab/<type>_templates/*.md` on the default branch.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::get;
use labrat_core::model::Project;
use labrat_core::{App, Error, repo};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/templates/{type}", get(list))
        .route("/projects/{id}/templates/{type}/{name}", get(show))
}

const TYPES: &[&str] = &["dockerfiles", "gitignores", "gitlab_ci_ymls", "licenses", "issues", "merge_requests"];

fn kind(req: &Req) -> Result<String, ApiError> {
    let t = req.path_param("type").to_string();
    if !TYPES.contains(&t.as_str()) {
        return Err(Error::Param("type does not have a valid value".into()).into());
    }
    Ok(t)
}

/// `(name, content)` of the repository's templates of that type, sorted by name.
fn templates(c: &rusqlite::Connection, p: &Project, kind: &str) -> Result<Vec<(String, Vec<u8>)>, ApiError> {
    let dir = match kind {
        "issues" => ".gitlab/issue_templates",
        "merge_requests" => ".gitlab/merge_request_templates",
        _ => return Ok(vec![]),
    };
    let r = repo::repo(c, p);
    let Some(branch) = p.default_branch.clone() else { return Ok(vec![]) };
    let Some(head) = r.branch(&branch)? else { return Ok(vec![]) };
    let tree = r.commit(&head)?.tree;
    let Some(entry) = r.entry_at(&tree, dir)? else { return Ok(vec![]) };
    let mut out = vec![];
    for e in r.tree(&entry.oid).unwrap_or_default() {
        if let Some(name) = e.name.strip_suffix(".md")
            && e.mode != 0o40000
        {
            out.push((name.to_string(), r.blob(&e.oid)?));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

async fn list(req: Req) -> ApiResult {
    let kind = kind(&req)?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.read_code(&p) {
            return Err(Error::forbidden().into());
        }
        let list = templates(c, &p, &kind)?;
        paginate(&req, list, &[], |(name, _)| Ok(json!({"key": name, "name": name})))
    })
}

async fn show(req: Req) -> ApiResult {
    let kind = kind(&req)?;
    let name = req.path_param("name").to_string();
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.read_code(&p) {
            return Err(Error::forbidden().into());
        }
        let found = templates(c, &p, &kind)?.into_iter().find(|(n, _)| *n == name);
        match found {
            Some((n, content)) => Ok::<_, ApiError>(json!({"name": n, "content": String::from_utf8_lossy(&content)})),
            None => Err(Error::not_found("Template").into()),
        }
    })?;
    let _ = Value::Null;
    Ok(Resp::ok(body))
}
