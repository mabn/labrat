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
        .route("/-/labrat/projects/{id}/ci", get(get_ci).put(put_ci))
        .route("/-/labrat/projects/{id}/ci/jobs", get(ci_jobs))
        .route("/-/labrat/ci/pick", post(ci_pick))
        .route("/-/labrat/ci/finish", post(ci_finish))
}

fn ci_project(c: &rusqlite::Connection, id: &str) -> Result<labrat_core::model::Project, crate::ctx::ApiError> {
    Ok(crate::find::project_by_ref(c, id)?.ok_or(labrat_core::Error::not_found("Project"))?)
}

/// The project's fake CI (see `labrat_core::services::fake_ci`).
async fn get_ci(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let body = req.app.db.read(|c| {
        let p = ci_project(c, &id)?;
        Ok::<_, crate::ctx::ApiError>(match labrat_core::services::fake_ci::config(c, p.id)? {
            Some(f) => f.to_json(),
            None => json!({"enabled": false}),
        })
    })?;
    Ok(Resp::ok(body))
}

/// Switches the project's fake CI on or off: `{"enabled": true, "jobs": ["test"], "runner":
/// {"mode": "manual" | "auto", "status": "success", "duration_ms": 0, "overrides": {}}}`.
async fn put_ci(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let cfg = labrat_core::services::fake_ci::parse_config(&req.params.raw_all()).map_err(|m| labrat_core::Error::message(400, m))?;
    let body = req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        labrat_core::services::fake_ci::set_config(c, p.id, cfg.as_ref())?;
        Ok::<_, crate::ctx::ApiError>(cfg.as_ref().map(|f| f.to_json()).unwrap_or(json!({"enabled": false})))
    })?;
    Ok(Resp::ok(body))
}

async fn ci_jobs(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let body = req.app.db.read(|c| {
        let p = ci_project(c, &id)?;
        Ok::<_, crate::ctx::ApiError>(labrat_core::services::fake_ci::jobs_json(c, p.id)?)
    })?;
    Ok(Resp::ok(body))
}

fn project_param(req: &Req) -> Result<String, crate::ctx::ApiError> {
    match req.params.raw("project_id").unwrap_or(&serde_json::Value::Null) {
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::String(s) => Ok(s.clone()),
        _ => Err(labrat_core::Error::message(400, "project_id is missing").into()),
    }
}

/// The fake runner picks up a project's pending jobs (`{"project_id": 1}`, optionally with
/// `"ref"`): they start running.
async fn ci_pick(req: Req) -> ApiResult {
    crate::jobs::wait_idle(&req.app).await;
    let id = project_param(&req)?;
    let r#ref = req.params.raw_all()["ref"].as_str().map(String::from);
    let config = req.app.config.clone();
    let body = req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let picked = labrat_core::services::fake_ci::pick(c, &config, p.id, r#ref.as_deref())?;
        Ok::<_, crate::ctx::ApiError>(json!({"picked": picked.iter().map(|(id, r)| json!({"id": id, "ref": r})).collect::<Vec<_>>()}))
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

/// The fake runner finishes jobs: `{"project_id": 1, "ref": "refs/merge-requests/1/merge",
/// "status": "success" | "failed" | "canceled"}`. Running jobs on that ref finish; with
/// `"include_pending": true` pending ones too.
async fn ci_finish(req: Req) -> ApiResult {
    let id = project_param(&req)?;
    let body = req.params.raw_all();
    let r#ref = body["ref"].as_str().unwrap_or("").to_string();
    let status = body["status"].as_str().unwrap_or("success").to_string();
    let pending = body["include_pending"].as_bool().unwrap_or(false);
    let config = req.app.config.clone();
    let body = req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let n = labrat_core::services::fake_ci::finish(c, &config, p.id, &r#ref, &status, pending)?;
        Ok::<_, crate::ctx::ApiError>(json!({"finished": n}))
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
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
