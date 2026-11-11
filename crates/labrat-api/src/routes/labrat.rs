//! labrat-only test endpoints under `/-/labrat/`. They need no authentication: labrat is a
//! test tool that only listens where tests can reach it.

use crate::ctx::{ApiResult, Req, Resp};
use axum::Router;
use axum::routing::{get, post};
use labrat_core::App;
use serde_json::json;
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/-/labrat/reset", post(reset))
        .route("/-/labrat/snapshot", post(snapshot))
        .route("/-/labrat/restore", post(restore))
        .route("/-/labrat/idle", get(idle))
        .route("/-/labrat/time", post(set_time))
        .route("/-/labrat/requests.json", get(requests).delete(clear_requests))
}

/// The request log as JSON, newest first (`?limit=` to cap it).
async fn requests(req: Req) -> ApiResult {
    let limit = req.params.int("limit")?.unwrap_or(100).max(0) as usize;
    let list: Vec<serde_json::Value> = crate::request_log::entries(&req.app).iter().take(limit).map(crate::request_log::to_json).collect();
    Ok(Resp::ok(serde_json::Value::Array(list)))
}

async fn clear_requests(req: Req) -> ApiResult {
    crate::request_log::clear(&req.app);
    Ok(Resp::no_content())
}

/// Wipes all data and recreates the root user (and its token, if configured).
async fn reset(req: Req) -> ApiResult {
    crate::request_log::clear(&req.app);
    req.app.db.reset()?;
    crate::cache::clear();
    crate::bootstrap(&req.app)?;
    Ok(Resp::ok(json!({"status": "ok"})))
}

async fn snapshot(req: Req) -> ApiResult {
    let path = req
        .params
        .str("path")
        .unwrap_or("labrat-snapshot.sqlite".into());
    req.app.db.snapshot(&path)?;
    Ok(Resp::ok(json!({"path": path})))
}

async fn restore(req: Req) -> ApiResult {
    let path = req
        .params
        .str("path")
        .unwrap_or("labrat-snapshot.sqlite".into());
    req.app.db.restore(&path)?;
    Ok(Resp::ok(json!({"path": path})))
}

/// Returns once all background work has finished.
async fn idle(req: Req) -> ApiResult {
    crate::jobs::wait_idle(&req.app).await;
    Ok(Resp::ok(json!({"idle": true})))
}

/// Shifts this instance's clock: `offset_ms` is added to the system time.
async fn set_time(req: Req) -> ApiResult {
    let offset = req.params.int_or("offset_ms", 0)?;
    req.app.db.set_offset(offset);
    // Moving the clock forward lets the time pass for background work too: jobs that became
    // due run before the next request, as Sidekiq would have run them meanwhile.
    crate::jobs::wait_due(&req.app).await;
    Ok(Resp::ok(json!({"offset_ms": offset, "now": labrat_core::time::iso(req.app.db.now())})))
}
