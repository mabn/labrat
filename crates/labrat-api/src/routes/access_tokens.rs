//! `lib/api/resource_access_tokens.rb`: project and group access tokens.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::token as token_ent;
use crate::find;
use axum::Router;
use axum::routing::{get, post};
use labrat_core::model::Token;
use labrat_core::services::access_tokens::{self, Resource};
use labrat_core::{App, Error, time};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/access_tokens", get(list).post(create))
        .route("/projects/{id}/access_tokens/{token_id}", get(show).delete(revoke))
        .route("/projects/{id}/access_tokens/{token_id}/rotate", post(rotate))
        .route("/groups/{id}/access_tokens", get(list).post(create))
        .route("/groups/{id}/access_tokens/{token_id}", get(show).delete(revoke))
        .route("/groups/{id}/access_tokens/{token_id}/rotate", post(rotate))
}

/// `ResourceAccessToken`: the token entity plus its access level and resource.
fn entity(c: &Connection, t: &Token, value: Option<&str>) -> Result<Value, ApiError> {
    let (level, rid): (Option<i64>, Option<i64>) = c.query_row("SELECT access_level, resource_id FROM personal_access_tokens WHERE id = ?1", [t.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut m = token_ent::token(t);
    m.insert("access_level".into(), json!(level));
    let kind: Option<String> = c.query_row("SELECT resource_type FROM personal_access_tokens WHERE id = ?1", [t.id], |r| r.get(0))?;
    m.insert("resource_type".into(), json!(kind));
    m.insert("resource_id".into(), json!(rid));
    if let Some(v) = value {
        m.insert("token".into(), json!(v));
    }
    Ok(Value::Object(m))
}

/// The project or group of the path, for a user who may manage its tokens
/// (`admin_resource_access_tokens`: Maintainer of the project, Owner of the group).
fn load(c: &Connection, req: &Req) -> Result<(String, i64), ApiError> {
    let admin = req.user.as_ref().is_some_and(|u| u.admin);
    if req.path.contains("/groups/") {
        let (g, perms) = find::group(c, req, req.path_param("id"))?;
        if !admin && perms.level < 50 {
            return Err(Error::forbidden().into());
        }
        Ok(("group".into(), g.id))
    } else {
        let (p, perms) = find::project(c, req, req.path_param("id"))?;
        if !perms.admin_project() && !admin {
            return Err(Error::forbidden().into());
        }
        Ok(("project".into(), p.id))
    }
}

fn token_of(c: &Connection, req: &Req, res: &(String, i64)) -> Result<Token, ApiError> {
    let id: i64 = req.path_param("token_id").parse().map_err(|_| Error::Param("token_id is invalid".into()))?;
    if !access_tokens::ids_for(c, Resource { kind: &res.0, id: res.1 })?.contains(&id) {
        return Err(Error::Api(404, json!(format!("404 Could not find {} access token with token_id: {id}", res.0))).into());
    }
    Ok(Token::find(c, id)?.ok_or(Error::not_found("Token"))?)
}

async fn create(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let p = &req.params;
    let mut missing = vec![];
    if !p.has("name") {
        missing.push("name is missing".to_string());
    }
    if p.list("scopes").is_none_or(|s| s.is_empty()) {
        missing.push("scopes is missing, scopes does not have a valid value".to_string());
    }
    if !missing.is_empty() {
        return Err(Error::Param(missing.join(", ")).into());
    }
    let level = p.int("access_level")?.unwrap_or(40);
    if ![10, 15, 20, 30, 40, 50].contains(&level) {
        return Err(Error::Param("access_level does not have a valid value".into()).into());
    }
    let expires_at = p.date("expires_at")?;
    let limit = time::date(time::now() + 365 * 86_400_000);
    if expires_at.as_deref().is_some_and(|e| e > limit.as_str()) {
        return Err(Error::BadRequest(Some(format!("Expiration date must be before {limit}"))).into());
    }
    let name = p.str("name").unwrap_or_default();
    let description = p.str("description");
    let scopes = p.list("scopes").unwrap_or_default();
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let res = load(c, &req)?;
        let (id, value) = access_tokens::create_for(
            c,
            &req.app.config,
            Resource { kind: &res.0, id: res.1 },
            &me,
            access_tokens::NewProjectToken { name: &name, description: description.as_deref(), scopes: &scopes, access_level: level, expires_at: expires_at.as_deref() },
        )?;
        let t = Token::find(c, id)?.ok_or(Error::not_found("Token"))?;
        entity(c, &t, Some(&value))
    })?;
    Ok(Resp::created(body))
}

async fn list(req: Req) -> ApiResult {
    let state = req.params.one_of("state", &["active", "inactive"])?;
    let app = req.app.clone();
    app.db.read(|c| {
        let res = load(c, &req)?;
        let mut list = vec![];
        for id in access_tokens::ids_for(c, Resource { kind: &res.0, id: res.1 })? {
            let Some(t) = Token::find(c, id)? else { continue };
            if state.as_deref().is_some_and(|s| (s == "active") != t.active()) {
                continue;
            }
            list.push(t);
        }
        crate::pagination::paginate(&req, list, &[], |t| entity(c, t, None))
    })
}

async fn show(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let res = load(c, &req)?;
        let t = token_of(c, &req, &res)?;
        Ok(Resp::ok(entity(c, &t, None)?))
    })
}

async fn revoke(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let res = load(c, &req)?;
        let t = token_of(c, &req, &res)?;
        labrat_core::services::tokens::revoke(c, &t)?;
        Ok(Resp { status: axum::http::StatusCode::NO_CONTENT, body: None, headers: vec![] })
    })
}

async fn rotate(req: Req) -> ApiResult {
    let expires_at = req.params.date("expires_at")?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let res = load(c, &req)?;
        let t = token_of(c, &req, &res)?;
        let expires = expires_at.clone().unwrap_or_else(|| time::date(time::now() + 7 * 86_400_000));
        let (id, value) = access_tokens::rotate(c, t.id, Some(&expires))?;
        let new = Token::find(c, id)?.ok_or(Error::not_found("Token"))?;
        Ok(Resp::ok(entity(c, &new, Some(&value))?))
    })
}
