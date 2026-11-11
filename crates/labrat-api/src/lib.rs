//! HTTP layer: GitLab's REST API v4 and labrat's test endpoints.

pub mod cache;
pub mod ctx;
pub mod entities;
pub mod find;
pub mod git_http;
pub mod hooks;
pub mod graphql;
pub mod jobs;
pub mod markdown;
pub mod pagination;
pub mod params;
pub mod request_log;
pub mod routes;
pub mod ui;

use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use labrat_core::App;
use labrat_core::model::User;
use labrat_core::services::{tokens, users};
use serde_json::json;
use std::sync::Arc;

/// Creates the root user (and its fixed token, if configured) on an empty database.
pub fn bootstrap(app: &App) -> labrat_core::Result<()> {
    labrat_core::init();
    app.db.tx(|c| {
        let root = users::ensure_root(c, &app.config.root_password)?;
        if let Some(value) = &app.config.root_token
            && tokens::find_by_value(c, value)?.is_none() {
                tokens::create(
                    c,
                    &root,
                    tokens::NewToken {
                        name: "labrat-root",
                        description: None,
                        scopes: &[
                            "api".to_string(),
                            "sudo".to_string(),
                            "admin_mode".to_string(),
                        ],
                        expires_at: None,
                        impersonation: false,
                        value: Some(value),
                    },
                )?;
            }
        let _: Option<User> = None;
        Ok(())
    })
}

async fn api_not_found() -> impl IntoResponse {
    ctx::json_response(StatusCode::NOT_FOUND, &json!({"error": "404 Not Found"}))
}

/// Everything outside `/api/v4`: git over HTTP, then (later) the web UI.
async fn fallback(
    axum::extract::State(app): axum::extract::State<Arc<App>>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().unwrap_or("").to_string();
    let bytes = axum::body::to_bytes(body, 1 << 30)
        .await
        .unwrap_or_default();
    if git_http::split_git_path(&path).is_some() {
        return git_http::handle(app, parts.method, &path, &query, parts.headers, bytes).await;
    }
    ui::handle(app, parts.method, &path, &query, parts.headers, &bytes).await
}

pub fn router(app: Arc<App>) -> Router {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(jobs::scheduled_merges(Arc::downgrade(&app)));
    }
    let api = Router::new()
        .merge(routes::meta::routes())
        .merge(routes::users::routes())
        .merge(routes::access_tokens::routes())
        .merge(routes::namespaces::routes())
        .merge(routes::repo_extras::routes())
        .merge(routes::groups::routes())
        .merge(routes::projects::routes())
        .merge(routes::members::routes())
        .merge(routes::repository::routes())
        .merge(routes::merge_requests::routes())
        .merge(routes::todos::routes())
        .merge(routes::notes::routes())
        .merge(routes::hooks::routes())
        .merge(routes::labels::routes())
        .merge(routes::milestones::routes())
        .merge(routes::commit_notes::routes())
        .merge(routes::pipelines::routes())
        .merge(routes::events::routes())
        .merge(routes::context_commits::routes())
        .merge(routes::search::routes())
        .merge(routes::uploads::routes())
        .merge(routes::templates::routes())
        .fallback(api_not_found)
        // GitLab's catch-all API route answers a method mismatch with 404, not 405.
        .method_not_allowed_fallback(api_not_found);
    Router::new()
        .nest("/api/v4", api)
        .merge(routes::labrat::routes())
        .merge(graphql::routes())
        .fallback(fallback)
        .layer(axum::middleware::from_fn_with_state(app.clone(), kick_jobs))
        .layer(axum::middleware::from_fn_with_state(app.clone(), request_log::record))
        .with_state(app)
}

/// Wakes the job worker after every write request, so queued work (webhooks, MR jobs) runs.
async fn kick_jobs(
    axum::extract::State(app): axum::extract::State<Arc<App>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let write = req.method() != axum::http::Method::GET && req.method() != axum::http::Method::HEAD;
    let resp = next.run(req).await;
    if write {
        jobs::kick(&app);
    }
    resp
}
