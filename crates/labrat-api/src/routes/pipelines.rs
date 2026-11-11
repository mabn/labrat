//! Commit statuses (`lib/api/commit_statuses.rb`) and the pipelines they create
//! (`lib/api/ci/pipelines.rb`, list and show only).

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::user::{self as user_ent, Ctx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::{get, post};
use labrat_core::model::{Project, User};
use labrat_core::services::ci::{self, Pipeline, Status, StatusInput};
use labrat_core::{App, Config, Error, time};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/statuses/{sha}", post(create_status))
        .route("/projects/{id}/repository/commits/{sha}/statuses", get(list_statuses))
        .route("/projects/{id}/pipelines", get(list_pipelines))
        .route("/projects/{id}/pipeline", axum::routing::post(create_pipeline))
        .route("/projects/{id}/pipelines/latest", get(latest_pipeline))
        .route("/projects/{id}/pipelines/{pipeline_id}", get(show_pipeline).delete(delete_pipeline))
        .route("/projects/{id}/pipelines/{pipeline_id}/jobs", get(pipeline_jobs))
        .route("/projects/{id}/pipelines/{pipeline_id}/bridges", get(pipeline_bridges))
        .route("/projects/{id}/pipelines/{pipeline_id}/variables", get(pipeline_variables))
        .route("/projects/{id}/pipelines/{pipeline_id}/test_report", get(pipeline_test_report))
        .route("/projects/{id}/pipelines/{pipeline_id}/test_report_summary", get(pipeline_test_report_summary))
        .route("/projects/{id}/pipelines/{pipeline_id}/metadata", axum::routing::put(pipeline_metadata))
        .route("/projects/{id}/pipelines/{pipeline_id}/cancel", post(cancel_pipeline))
        .route("/projects/{id}/pipelines/{pipeline_id}/retry", post(retry_pipeline))
}

fn user_basic(c: &Connection, req: &Req, id: Option<i64>) -> Result<Value, ApiError> {
    let ctx = Ctx { base_url: &req.app.config.external_url, current_user: req.user.as_ref() };
    Ok(match id.map(|i| User::find(c, i)).transpose()?.flatten() {
        Some(u) => user_ent::basic_value(&u, &ctx),
        None => Value::Null,
    })
}

/// `Entities::CommitStatus`.
pub fn status_value(c: &Connection, req: &Req, s: &Status) -> Result<Value, ApiError> {
    let mut m = Map::new();
    m.insert("id".into(), json!(s.id));
    m.insert("sha".into(), json!(s.sha));
    m.insert("ref".into(), json!(s.ref_));
    m.insert("status".into(), json!(s.status));
    m.insert("name".into(), json!(s.name));
    m.insert("target_url".into(), json!(s.target_url));
    m.insert("description".into(), json!(s.description));
    m.insert("created_at".into(), json!(time::iso(s.created_at)));
    m.insert("started_at".into(), json!(s.started_at.map(time::iso)));
    m.insert("finished_at".into(), json!(s.finished_at.map(time::iso)));
    m.insert("allow_failure".into(), json!(false));
    m.insert("coverage".into(), json!(s.coverage));
    m.insert("pipeline_id".into(), json!(s.pipeline_id));
    m.insert("author".into(), user_basic(c, req, s.user_id)?);
    Ok(Value::Object(m))
}

/// `Entities::Ci::PipelineBasic`.
pub fn pipeline_basic(config: &Config, p: &Project, pl: &Pipeline) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(pl.id));
    m.insert("iid".into(), json!(pl.iid));
    m.insert("project_id".into(), json!(pl.project_id));
    m.insert("sha".into(), json!(pl.sha));
    m.insert("ref".into(), json!(pl.ref_));
    m.insert("status".into(), json!(pl.status));
    m.insert("source".into(), json!(pl.source));
    m.insert("created_at".into(), json!(time::iso(pl.created_at)));
    m.insert("updated_at".into(), json!(time::iso(pl.updated_at)));
    m.insert("web_url".into(), json!(config.url(&format!("/{}/-/pipelines/{}", p.full_path, pl.id))));
    m
}

/// Asset digests of GitLab 18.11's CI favicons.
pub(crate) fn favicon_digest(icon: &str) -> &'static str {
    match icon {
        "status_canceled" => "ca35321a6cda9943ebdf6631c8057ffd54064f3ba20cfe0ebc4b0b992041c430",
        "status_failed" => "41304d7f7e3828808b0c26771f0309e55296819a9beea3ea9fbf6689d9857c12",
        "status_manual" => "829a0804612cef47d9efc1618dba38325483657c847dba0546c3b9f0295bb36c",
        "status_pending" => "5bdf338420e5221ca24353b6bff1c9367189588750632e9a871b7af09ff6a2ae",
        "status_preparing" => "ecc5e3748a16f6189600e528e1602789ad967d2eff13ee440b8e3003d46aa8dd",
        "status_running" => "9c635b2419a8e1ec991c993061b89cc5aefc0743bb238ecd0c381e7741a70e8c",
        "status_scheduled" => "2349748ddbcc7b5e7bd112ed374ffa754a2d3f9c937fa1c709890734475b2f8f",
        "status_skipped" => "0b9c5e543588945e8c4ca57786bbf2d0c56631959c9f853300392d0315be829b",
        "status_success" => "8451333011eee8ce9f2ab25dc487fe24a8758c694827a582f17f42b0a90446a2",
        _ => "4b975aa976d24e5a3ea7cd9a5713e6ce2cd9afd08b910415e96675de35f64955",
    }
}

/// `DetailedStatusEntity` for a pipeline status.
fn detailed_status(p: &Project, pl: &Pipeline, has_details: bool) -> Value {
    let (icon, text, label, group) = match pl.status.as_str() {
        "pending" => ("status_pending", "Pending", "pending", "pending"),
        "running" => ("status_running", "Running", "running", "running"),
        "success" => ("status_success", "Passed", "passed", "success"),
        "failed" => ("status_failed", "Failed", "failed", "failed"),
        "canceled" => ("status_canceled", "Canceled", "canceled", "canceled"),
        "skipped" => ("status_skipped", "Skipped", "skipped", "skipped"),
        _ => ("status_created", "Created", "created", "created"),
    };
    json!({
        "icon": icon,
        "text": text,
        "label": label,
        "group": group,
        "tooltip": label,
        "has_details": has_details,
        "details_path": format!("/{}/-/pipelines/{}", p.full_path, pl.id),
        "illustration": null,
        "favicon": format!("/assets/ci_favicons/favicon_{icon}-{}.png", favicon_digest(icon)),
    })
}

/// `Entities::Ci::Pipeline` (detailed).
pub fn pipeline_detailed(c: &Connection, req: &Req, p: &Project, pl: &Pipeline) -> Result<Map<String, Value>, ApiError> {
    Ok(pipeline_detailed_for(c, &req.app.config, req.user.as_ref(), p, pl, false)?)
}

pub fn pipeline_detailed_for(
    c: &Connection,
    config: &Config,
    current: Option<&User>,
    p: &Project,
    pl: &Pipeline,
    with_user: bool,
) -> labrat_core::Result<Map<String, Value>> {
    let mut m = pipeline_basic(config, p, pl);
    m.insert("before_sha".into(), json!(pl.before_sha.clone().unwrap_or_else(|| "0000000000000000000000000000000000000000".into())));
    m.insert("tag".into(), json!(pl.tag));
    m.insert("yaml_errors".into(), Value::Null);
    let ctx = Ctx { base_url: &config.external_url, current_user: current };
    let user = match pl.user_id.map(|i| User::find(c, i)).transpose()?.flatten() {
        Some(u) => user_ent::basic_value(&u, &ctx),
        None => Value::Null,
    };
    m.insert("user".into(), user);
    m.insert("started_at".into(), json!(pl.started_at.map(time::iso)));
    m.insert("finished_at".into(), json!(pl.finished_at.map(time::iso)));
    m.insert("committed_at".into(), Value::Null);
    m.insert("duration".into(), json!(pl.duration));
    m.insert("queued_duration".into(), json!(pl.queued_duration));
    let cov: Option<f64> = {
        let mut st = c.prepare("SELECT coverage FROM commit_statuses WHERE pipeline_id = ?1 AND retried = 0 AND coverage IS NOT NULL")?;
        let v: Vec<f64> = st.query_map([pl.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        if v.is_empty() { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) }
    };
    m.insert("coverage".into(), json!(cov.map(|x| format!("{x:.2}"))));
    // The pipelines API builds the status without a user, so it reports no details page.
    m.insert("detailed_status".into(), detailed_status(p, pl, with_user));
    m.insert("archived".into(), json!(false));
    Ok(m)
}

async fn create_status(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let sha = req.path_param("sha").to_string();
    let state = req.params.one_of("state", &["pending", "running", "success", "failed", "canceled", "skipped"])?;
    let Some(state) = state else {
        return Err(Error::Param("state is missing, state does not have a valid value".into()).into());
    };
    let coverage = match req.params.raw("coverage") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) if !s.is_empty() => Some(s.parse::<f64>().map_err(|_| Error::Param("coverage is invalid".into()))?),
        _ => None,
    };
    let input = StatusInput {
        state,
        ref_: req.params.str("ref"),
        name: req.params.str("name").or(req.params.str("context")),
        target_url: req.params.str("target_url"),
        description: req.params.str("description"),
        coverage,
        pipeline_id: req.params.int("pipeline_id")?,
    };
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.developer() {
            return Err(Error::forbidden().into());
        }
        let s = ci::post_status(c, &req.app.config, &p, &user, &sha, input)?;
        status_value(c, &req, &s)
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::created(body))
}

async fn list_statuses(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let all = req.params.bool_or("all", false)?;
    let order_by = req.params.one_of("order_by", &["id", "pipeline_id"])?.unwrap_or("id".into());
    let desc = req.params.one_of("sort", &["asc", "desc"])?.as_deref() == Some("desc");
    let ref_ = req.params.str("ref");
    let name = req.params.str("name");
    let stage = req.params.str("stage");
    let pipeline_id = req.params.int("pipeline_id")?;
    let app = req.app.clone();
    app.db.read(|c| {
        req.user()?;
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.reporter() {
            return Err(Error::forbidden().into());
        }
        let r = labrat_core::repo::repo(c, &p);
        r.resolve(&sha)?.filter(|o| r.kind(o).ok().flatten() == Some(labrat_git::Kind::Commit)).ok_or(Error::not_found("Commit"))?;
        let mut list = Status::for_sha(c, p.id, &sha, all)?;
        list.retain(|s| {
            ref_.as_ref().is_none_or(|x| s.ref_.as_ref() == Some(x))
                && name.as_ref().is_none_or(|x| &s.name == x)
                && stage.as_ref().is_none_or(|x| x == "external")
                && pipeline_id.is_none_or(|x| s.pipeline_id == x)
        });
        list.sort_by_key(|s| if order_by == "pipeline_id" { s.pipeline_id } else { s.id });
        if desc {
            list.reverse();
        }
        paginate(&req, list, &[("order_by", "id")], |s| status_value(c, &req, s))
    })
}

async fn list_pipelines(req: Req) -> ApiResult {
    let status = req.params.str("status");
    let ref_ = req.params.str("ref");
    let sha = req.params.str("sha");
    let source = req.params.str("source");
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.reporter() && !(p.visibility_level > 0 && perms.read_project()) {
            return Err(Error::forbidden().into());
        }
        let mut list = Pipeline::list(c, p.id)?;
        let refs = match &ref_ {
            Some(r) => Some(labrat_core::services::ci::refs_for_filter(c, &p, r, source.as_deref())?),
            None => None,
        };
        list.retain(|pl| {
            status.as_ref().is_none_or(|x| &pl.status == x)
                && refs.as_ref().is_none_or(|rs| pl.ref_.as_ref().is_some_and(|r| rs.contains(r)))
                && sha.as_ref().is_none_or(|x| &pl.sha == x)
                && source.as_ref().is_none_or(|x| &pl.source == x)
        });
        paginate(&req, list, &[("order_by", "id"), ("sort", "desc")], |pl| {
            let mut m = pipeline_basic(&req.app.config, &p, pl);
            m.insert("name".into(), Value::Null);
            Ok(Value::Object(m))
        })
    })
}

async fn show_pipeline(req: Req) -> ApiResult {
    let id: i64 = req.path_param("pipeline_id").parse().map_err(|_| Error::Param("pipeline_id is invalid".into()))?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = find::project(c, &req, req.path_param("id"))?;
        let pl = Pipeline::find(c, id)?.filter(|pl| pl.project_id == p.id).ok_or_else(|| ApiError::from(Error::Api(404, json!("404 Not found"))))?;
        let mut m = pipeline_detailed(c, &req, &p, &pl)?;
        m.insert("name".into(), json!(pl.name));
        Ok::<_, ApiError>(Value::Object(m))
    })?;
    Ok(Resp::ok(body))
}

/// labrat runs no CI: a pipeline can never be created. GitLab answers this way for a project
/// without CI configuration (Auto DevOps's `workflow:rules` filter out every job).
const NO_PIPELINE: &str = "The pipeline did not run. Review the workflow:rules configuration for the pipeline.";

fn pipeline_error(msg: &str) -> ApiError {
    Error::Api(400, json!({"base": [msg]})).into()
}

/// `POST /projects/:id/pipeline`.
async fn create_pipeline(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    req.params.require(&["ref"])?;
    let r#ref = req.params.str("ref").unwrap_or_default();
    let app = req.app.clone();
    let res: Result<(), ApiError> = app.db.read(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !(user.admin || perms.level >= labrat_core::model::access::DEVELOPER) {
            return Err(Error::forbidden().into());
        }
        let r = labrat_core::repo::repo(c, &p);
        if r.branch(&r#ref)?.is_none() && !r.tags()?.iter().any(|(n, _)| *n == r#ref) {
            return Err(pipeline_error("Reference not found"));
        }
        Err(pipeline_error(NO_PIPELINE))
    });
    res.map(|_| Resp::no_content())
}

/// `POST /projects/:id/merge_requests/:iid/pipelines`.
pub async fn create_mr_pipeline(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let asynchronous = req.params.bool_or("async", false)?;
    let app = req.app.clone();
    let res: Result<(), ApiError> = app.db.read(|c| {
        let (_, perms, _) = crate::routes::merge_requests::load(c, &req)?;
        if !(user.admin || perms.level >= labrat_core::model::access::DEVELOPER) {
            return Err(Error::forbidden().into());
        }
        // `async: true` only enqueues `MergeRequests::CreatePipelineWorker`: `status :accepted`
        // with the status as the body.
        if asynchronous {
            return Ok(());
        }
        Err(pipeline_error(NO_PIPELINE))
    });
    res.map(|_| if asynchronous { Resp::status(202, json!(202)) } else { Resp::no_content() })
}

fn not_found_pipeline() -> ApiError {
    Error::Api(404, json!("404 Not found")).into()
}

/// The project and one of its pipelines from the path.
fn load_pipeline(c: &Connection, req: &Req) -> Result<(Project, Pipeline), ApiError> {
    let id: i64 = req.path_param("pipeline_id").parse().map_err(|_| Error::Param("pipeline_id is invalid".into()))?;
    let (p, _) = find::project(c, req, req.path_param("id"))?;
    let pl = Pipeline::find(c, id)?.filter(|pl| pl.project_id == p.id).ok_or_else(not_found_pipeline)?;
    Ok((p, pl))
}

fn detailed_with_name(c: &Connection, req: &Req, p: &Project, pl: &Pipeline) -> Result<Value, ApiError> {
    let mut m = pipeline_detailed(c, req, p, pl)?;
    m.insert("name".into(), json!(pl.name));
    Ok(Value::Object(m))
}

/// The pipeline user's permission level on the project.
fn level(c: &Connection, req: &Req, p: &Project) -> Result<(bool, bool), ApiError> {
    let u = req.user()?;
    let perms = labrat_core::perms::ProjectPerms::load(c, Some(u), p)?;
    Ok((perms.developer() || u.admin, perms.owner() || u.admin))
}

/// Jobs are `Ci::Build`s only; commit statuses never show up.
async fn pipeline_jobs(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        load_pipeline(c, &req)?;
        paginate(&req, Vec::<Value>::new(), &[("include_retried", "false")], |v| Ok(v.clone()))
    })
}

async fn pipeline_bridges(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        load_pipeline(c, &req)?;
        paginate(&req, Vec::<Value>::new(), &[], |v| Ok(v.clone()))
    })
}

async fn pipeline_variables(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        load_pipeline(c, &req)?;
        Ok(Resp::ok(json!([])))
    })
}

async fn pipeline_test_report(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        load_pipeline(c, &req)?;
        Ok(Resp::ok(json!({"total_time": 0, "total_count": 0, "success_count": 0, "failed_count": 0, "skipped_count": 0, "error_count": 0, "test_suites": []})))
    })
}

async fn pipeline_test_report_summary(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        load_pipeline(c, &req)?;
        Ok(Resp::ok(json!({"total": {"time": 0, "count": 0, "success": 0, "failed": 0, "skipped": 0, "error": 0, "suite_error": null}, "test_suites": []})))
    })
}

/// `GET .../pipelines/latest`: the newest pipeline for `ref` (default branch by default);
/// `authorize! :read_pipeline, nil` answers 403 when there is none.
async fn latest_pipeline(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = find::project(c, &req, req.path_param("id"))?;
        let r = req.params.str("ref").or(p.default_branch.clone()).unwrap_or_default();
        let pl = Pipeline::list(c, p.id)?.into_iter().filter(|pl| pl.ref_.as_deref() == Some(r.as_str())).max_by_key(|pl| pl.id);
        match pl {
            Some(pl) => Ok(Resp::ok(detailed_with_name(c, &req, &p, &pl)?)),
            None => Err(Error::forbidden().into()),
        }
    })
}

/// `PUT .../metadata` sets the pipeline name.
async fn pipeline_metadata(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, pl) = load_pipeline(c, &req)?;
        if !level(c, &req, &p)?.0 {
            return Err(Error::forbidden().into());
        }
        req.params.require(&["name"])?;
        c.execute("UPDATE ci_pipelines SET name = ?1 WHERE id = ?2", rusqlite::params![req.params.str("name"), pl.id])?;
        let pl = Pipeline::find(c, pl.id)?.ok_or_else(not_found_pipeline)?;
        Ok(Resp::ok(detailed_with_name(c, &req, &p, &pl)?))
    })
}

/// Commit statuses can't be cancelled or retried: both leave the pipeline as it is (and,
/// unlike `show`, answer without `name`).
async fn cancel_pipeline(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, pl) = load_pipeline(c, &req)?;
        if !level(c, &req, &p)?.0 {
            return Err(Error::forbidden().into());
        }
        Ok(Resp::ok(Value::Object(pipeline_detailed(c, &req, &p, &pl)?)))
    })
}

async fn retry_pipeline(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, pl) = load_pipeline(c, &req)?;
        if !level(c, &req, &p)?.0 {
            return Err(Error::forbidden().into());
        }
        Ok(Resp::created(Value::Object(pipeline_detailed(c, &req, &p, &pl)?)))
    })
}

/// `DELETE`: owners only; the MRs' head pipelines are recomputed.
async fn delete_pipeline(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, pl) = load_pipeline(c, &req)?;
        if !level(c, &req, &p)?.1 {
            return Err(Error::forbidden().into());
        }
        let mrs = ci::all_merge_requests(c, &p, &pl)?;
        c.execute("DELETE FROM ci_pipelines WHERE id = ?1", [pl.id])?;
        for mr in mrs {
            ci::update_head_pipeline(c, &p, &mr)?;
        }
        Ok(Resp { status: axum::http::StatusCode::NO_CONTENT, body: None, headers: vec![] })
    })
}
