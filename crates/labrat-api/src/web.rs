//! The web UI: a single-page app (built from `web/` and embedded in the binary) that talks to
//! labrat only through the GitLab-compatible APIs, plus the few non-API pieces it needs.
//!
//! - `POST /users/sign_in` and `POST /users/sign_out` manage a cookie session, as GitLab's web
//!   sign-in does. The API accepts that session (see `ctx::Req::authenticate`) like GitLab's
//!   frontend does, with a CSRF token on every write.
//! - `/uploads/...` serves files attached to descriptions and comments.
//! - `/assets/spa/...` serves the app's static files; every other path serves `index.html`, so
//!   GitLab's URL paths (and `web_url`s from the API) open the right page.

use crate::params::parse_query;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use labrat_core::App;
use labrat_core::model::{Project, User};
use labrat_core::services::users;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// The session cookie (HttpOnly).
pub const SESSION_COOKIE: &str = "_labrat_session";
/// The CSRF token cookie. The app reads it and sends it back as `X-CSRF-Token` on writes.
pub const CSRF_COOKIE: &str = "_labrat_csrf";

mod assets {
    include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
}

struct Session {
    app: usize,
    user_id: i64,
    csrf: String,
}

/// Signed-in sessions by cookie value. Sessions do not survive a restart.
static SESSIONS: LazyLock<Mutex<HashMap<String, Session>>> = LazyLock::new(Default::default);

fn app_key(app: &Arc<App>) -> usize {
    Arc::as_ptr(app) as usize
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .find_map(|c| c.strip_prefix(name)?.strip_prefix('='))
}

/// Whether the request carries a session cookie (used to keep the app's own calls out of the
/// request log).
pub fn has_session_cookie(headers: &HeaderMap) -> bool {
    cookie(headers, SESSION_COOKIE).is_some()
}

/// The active user of the request's session. Writes must also carry the session's CSRF token
/// in `X-CSRF-Token`; like GitLab, a write without it is treated as anonymous.
pub fn session_user(app: &Arc<App>, method: &Method, headers: &HeaderMap) -> Option<User> {
    let sid = cookie(headers, SESSION_COOKIE)?;
    let user_id = {
        let sessions = SESSIONS.lock();
        let s = sessions.get(sid)?;
        if s.app != app_key(app) {
            return None;
        }
        if !matches!(*method, Method::GET | Method::HEAD) {
            let sent = headers.get("x-csrf-token").and_then(|v| v.to_str().ok());
            if sent != Some(s.csrf.as_str()) {
                return None;
            }
        }
        s.user_id
    };
    app.db
        .read(|c| User::find(c, user_id))
        .ok()
        .flatten()
        .filter(|u| u.state == "active")
}

fn redirect(to: &str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, to.to_string())]).into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "404 Not Found").into_response()
}

/// Only same-site relative paths, so the sign-in form can't redirect elsewhere.
fn safe_redirect(to: &str) -> &str {
    if to.starts_with('/') && !to.starts_with("//") && !to.starts_with("/\\") { to } else { "/" }
}

/// A form field; `user[login]` style names arrive nested (`{"user": {"login": …}}`).
fn form_str(form: &Map<String, Value>, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| match k.split_once('[') {
            Some((head, rest)) => form.get(head)?.get(rest.trim_end_matches(']'))?.as_str(),
            None => form.get(*k)?.as_str(),
        })
        .unwrap_or_default()
        .to_string()
}

fn sign_in(app: &Arc<App>, form: &Map<String, Value>) -> Response {
    let login = form_str(form, &["user[login]", "login"]);
    let password = form_str(form, &["user[password]", "password"]);
    let back = form_str(form, &["redirect"]);
    let user = app.db.read(|c| {
        let u = match User::find_by_username(c, &login)? {
            Some(u) => Some(u),
            None => {
                let id: Option<i64> = c
                    .query_row("SELECT id FROM users WHERE lower(email) = lower(?1)", [&login], |r| r.get(0))
                    .ok();
                id.map(|i| User::find(c, i)).transpose()?.flatten()
            }
        };
        Ok::<_, labrat_core::Error>(u)
    });
    match user {
        Ok(Some(u)) if u.state == "active" && users::check_password(&u, &password) => {
            let sid = hex::encode(rand::random::<[u8; 20]>());
            let csrf = hex::encode(rand::random::<[u8; 20]>());
            SESSIONS
                .lock()
                .insert(sid.clone(), Session { app: app_key(app), user_id: u.id, csrf: csrf.clone() });
            let mut r = redirect(safe_redirect(&back));
            let h = r.headers_mut();
            h.append(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!("{SESSION_COOKIE}={sid}; Path=/; HttpOnly; SameSite=Lax")).unwrap(),
            );
            h.append(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!("{CSRF_COOKIE}={csrf}; Path=/; SameSite=Strict")).unwrap(),
            );
            r
        }
        _ => {
            let mut to = "/users/sign_in?error=invalid".to_string();
            if !back.is_empty() {
                to.push_str("&redirect=");
                to.push_str(&percent_encoding::utf8_percent_encode(&back, percent_encoding::NON_ALPHANUMERIC).to_string());
            }
            redirect(&to)
        }
    }
}

fn sign_out(headers: &HeaderMap, form: &Map<String, Value>) -> Response {
    if let Some(sid) = cookie(headers, SESSION_COOKIE) {
        let mut sessions = SESSIONS.lock();
        // Only a form carrying the session's CSRF token may end it.
        let token = form_str(form, &["authenticity_token"]);
        if sessions.get(sid).is_some_and(|s| s.csrf == token) {
            sessions.remove(sid);
        }
    }
    let mut r = redirect("/users/sign_in");
    let h = r.headers_mut();
    for name in [SESSION_COOKIE, CSRF_COOKIE] {
        h.append(
            header::SET_COOKIE,
            HeaderValue::from_str(&format!("{name}=; Path=/; Max-Age=0")).unwrap(),
        );
    }
    r
}

/// Entry point from the router's fallback for every path that isn't the API or git.
pub async fn handle(app: Arc<App>, method: Method, path: &str, _query: &str, headers: HeaderMap, body: &[u8]) -> Response {
    if method == Method::POST {
        let mut form = Map::new();
        parse_query(&String::from_utf8_lossy(body), &mut form);
        return match path {
            "/users/sign_in" => sign_in(&app, &form),
            "/users/sign_out" => sign_out(&headers, &form),
            _ => not_found(),
        };
    }
    if method != Method::GET && method != Method::HEAD {
        return not_found();
    }
    let user = session_user(&app, &method, &headers);
    if let Some(resp) = upload_file(&app, user.as_ref(), path) {
        return resp;
    }
    if let Some(rel) = path.strip_prefix("/assets/spa/") {
        return match asset(&format!("assets/spa/{rel}")) {
            Some((data, ctype)) => (
                [
                    (header::CONTENT_TYPE, ctype),
                    // File names carry a content hash.
                    (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                ],
                data,
            )
                .into_response(),
            None => not_found(),
        };
    }
    if let Some((data, ctype)) = asset(path.trim_start_matches('/')).filter(|_| path != "/" && !path.ends_with(".html")) {
        // Top-level files from `web/public` (the favicon).
        return ([(header::CONTENT_TYPE, ctype)], data).into_response();
    }
    match asset("index.html") {
        Some((data, _)) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
            data,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "The web UI was not built into this binary. Run `npm ci && npm run build` in `web/`, then rebuild labrat.\n",
        )
            .into_response(),
    }
}

fn asset(rel: &str) -> Option<(&'static [u8], &'static str)> {
    let data = assets::ASSETS.iter().find(|(p, _)| *p == rel)?.1;
    let ext = rel.rsplit_once('.').map(|(_, e)| e).unwrap_or_default();
    let ctype = match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "json" => "application/json",
        _ => "application/octet-stream",
    };
    Some((data, ctype))
}

/// Uploaded files: `/<project path>/uploads/<secret>/<name>` and
/// `/-/project/<id>/uploads/<secret>/<name>`, for users who can read the project.
///
/// labrat also answers `/uploads/<secret>/<name>` (the link stored in Markdown), so images in
/// rendered notes work without rewriting links per project as GitLab's renderer does.
fn upload_file(app: &Arc<App>, user: Option<&User>, path: &str) -> Option<Response> {
    let (head, tail) = match path.strip_prefix("/uploads/") {
        Some(t) => ("", t),
        None => path.trim_start_matches('/').split_once("/uploads/")?,
    };
    let (secret, name) = tail.split_once('/')?;
    if secret.len() != 32 || name.contains('/') {
        return None;
    }
    let found = app.db.read(|c| {
        let p = if head.is_empty() {
            let pid: Option<i64> = c.query_row("SELECT project_id FROM uploads WHERE secret = ?1", [secret], |r| r.get(0)).ok();
            match pid {
                Some(id) => Project::find(c, id)?,
                None => None,
            }
        } else {
            match head.strip_prefix("-/project/") {
                Some(id) => Project::find(c, id.parse().unwrap_or(-1))?,
                None => Project::find_by_full_path(c, head)?,
            }
        };
        let Some(p) = p else { return Ok::<_, labrat_core::Error>(None) };
        if !labrat_core::perms::ProjectPerms::load(c, user, &p)?.read_project() {
            return Ok(None);
        }
        let data: Option<Vec<u8>> = c
            .query_row("SELECT data FROM uploads WHERE project_id = ?1 AND secret = ?2 AND filename = ?3", rusqlite::params![p.id, secret, name], |r| r.get(0))
            .ok();
        Ok(data)
    });
    let Ok(Some(data)) = found else { return Some(not_found()) };
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let ctype = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain; charset=utf-8",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    };
    Some(([(header::CONTENT_TYPE, ctype.to_string())], data).into_response())
}
