//! `/version` and `/metadata`.

use crate::ctx::{ApiResult, Req, Resp};
use axum::Router;
use axum::routing::get;
use labrat_core::App;
use serde_json::json;
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/version", get(version))
        .route("/metadata", get(version))
        .route("/markdown", axum::routing::post(markdown))
}

/// `POST /markdown`: the `:plain_markdown` pipeline, or the full GFM one with `gfm`.
async fn markdown(req: Req) -> ApiResult {
    let p = &req.params;
    let Some(text) = p.str("text") else {
        return Err(labrat_core::Error::Param("text is missing".into()).into());
    };
    let gfm = p.bool_or("gfm", false)?;
    let project = p.str("project");
    let html = req.app.db.read(|c| {
        let proj = match &project {
            Some(path) => {
                let found = labrat_core::model::Project::find_by_full_path(c, path)?;
                let readable = match found {
                    Some(pr) => crate::find::project(c, &req, &pr.id.to_string()).ok().map(|(pr, _)| pr),
                    None => None,
                };
                Some(readable.ok_or(labrat_core::Error::not_found("Project"))?)
            }
            None => None,
        };
        Ok::<_, crate::ctx::ApiError>(match (gfm, proj) {
            (false, _) => crate::markdown::render_plain(&text),
            // `only_path: false`: reference links are absolute.
            (true, Some(pr)) => crate::markdown::render_gfm(c, &pr, &text, labrat_core::time::now()).replace(" href=\"/", &format!(" href=\"{}/", req.app.config.url("").trim_end_matches('/'))),
            (true, None) => crate::markdown::render(&text),
        })
    })?;
    Ok(Resp::created(json!({ "html": html })))
}

async fn version(req: Req) -> ApiResult {
    req.user()?;
    Ok(Resp::ok(json!({
        "version": req.app.config.gitlab_version(),
        "revision": req.app.config.gitlab_revision(),
        "kas": {"enabled": false, "externalUrl": null, "externalK8sProxyUrl": null, "version": null},
        "enterprise": req.app.config.is_ee(),
    })))
}
