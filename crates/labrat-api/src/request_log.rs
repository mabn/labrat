//! A log of recent API calls (REST and GraphQL) with their requests and responses, shown in the
//! UI at `/-/labrat/requests` and available as JSON with `Accept: application/json`.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use labrat_core::{App, time};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};

const KEEP: usize = 500;
const BODY_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub struct Entry {
    pub id: u64,
    pub at: i64,
    pub method: String,
    pub uri: String,
    pub user: Option<String>,
    pub status: u16,
    pub duration_ms: f64,
    pub request_headers: Vec<(String, String)>,
    pub request_body: String,
    pub response_body: String,
}

struct Log {
    next: u64,
    entries: VecDeque<Entry>,
}

static LOGS: LazyLock<Mutex<HashMap<usize, Log>>> = LazyLock::new(Default::default);

fn key(app: &Arc<App>) -> usize {
    Arc::as_ptr(app) as usize
}

fn text(bytes: &[u8]) -> String {
    let cut = &bytes[..bytes.len().min(BODY_LIMIT)];
    let mut s = String::from_utf8_lossy(cut).into_owned();
    if bytes.len() > BODY_LIMIT {
        s.push_str(&format!("\n… ({} bytes)", bytes.len()));
    }
    s
}

fn token_user(app: &Arc<App>, headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers
        .get("private-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::to_string))?;
    app.db
        .read(|c| {
            let t = labrat_core::services::tokens::find_by_value(c, &raw)?;
            Ok::<_, labrat_core::Error>(t.and_then(|t| labrat_core::model::User::find(c, t.user_id).ok().flatten()).map(|u| u.username))
        })
        .ok()
        .flatten()
}

/// Middleware recording API calls.
pub async fn record(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if !(path.starts_with("/api/v4/") || path == "/api/graphql") {
        return next.run(req).await;
    }
    let start = std::time::Instant::now();
    let method = req.method().to_string();
    let uri = req.uri().to_string();
    let user = token_user(&app, req.headers());
    let request_headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "private-token" | "authorization" | "cookie"))
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let request_body = text(&bytes);
    let resp = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let (rparts, rbody) = resp.into_parts();
    let rbytes = axum::body::to_bytes(rbody, usize::MAX).await.unwrap_or_default();
    let entry = Entry {
        id: 0,
        at: time::now(),
        method,
        uri,
        user,
        status: rparts.status.as_u16(),
        duration_ms: start.elapsed().as_secs_f64() * 1000.0,
        request_headers,
        request_body,
        response_body: text(&rbytes),
    };
    {
        let mut logs = LOGS.lock();
        let log = logs.entry(key(&app)).or_insert(Log { next: 1, entries: VecDeque::new() });
        let mut e = entry;
        e.id = log.next;
        log.next += 1;
        log.entries.push_front(e);
        log.entries.truncate(KEEP);
    }
    Response::from_parts(rparts, Body::from(rbytes))
}

/// The newest entries first.
pub fn entries(app: &Arc<App>) -> Vec<Entry> {
    LOGS.lock().get(&key(app)).map(|l| l.entries.iter().cloned().collect()).unwrap_or_default()
}

pub fn clear(app: &Arc<App>) {
    LOGS.lock().remove(&key(app));
}

pub fn to_json(e: &Entry) -> Value {
    json!({
        "id": e.id,
        "at": time::iso(e.at),
        "method": e.method,
        "uri": e.uri,
        "user": e.user,
        "status": e.status,
        "duration_ms": e.duration_ms,
        "request_headers": e.request_headers.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "request_body": e.request_body,
        "response_body": e.response_body,
    })
}
