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
        .route("/-/labrat/demo", post(demo))
        .route("/-/labrat/snapshot", post(snapshot))
        .route("/-/labrat/restore", post(restore))
        .route("/-/labrat/idle", get(idle))
        .route("/-/labrat/time", post(set_time))
        .route("/-/labrat/requests.json", get(requests).delete(clear_requests))
        .route("/-/labrat/projects/{id}/ci", get(get_ci).put(put_ci))
        .route("/-/labrat/projects/{id}/ci/jobs", get(ci_jobs))
        .route("/-/labrat/projects/{id}/pipelines", get(list_runs).post(create_run))
        .route("/-/labrat/projects/{id}/pipelines/{pipeline_id}", get(show_run).put(update_run).delete(delete_run))
        .route("/-/labrat/ci/pick", post(ci_pick))
        .route("/-/labrat/ci/finish", post(ci_finish))
        .route("/-/labrat/projects/{id}/code_owners", get(get_code_owners).put(put_code_owners).delete(delete_code_owners))
        .route("/-/labrat/projects/{id}/approval_policies", get(get_policies).put(put_policies).delete(delete_policies))
        .route("/-/labrat/projects/{id}/merge_requests/{merge_request_iid}/policy_results", axum::routing::put(put_policy_results))
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

fn code_owners_json(c: &rusqlite::Connection, p: &labrat_core::model::Project, branch: &str) -> labrat_core::Result<serde_json::Value> {
    use labrat_core::model::{Namespace, User};
    let Some(sections) = labrat_core::services::code_owners::config(c, p.id, branch)? else {
        return Ok(json!({"branch": branch, "sections": []}));
    };
    let mut out = vec![];
    for s in sections {
        let mut entries = vec![];
        for e in &s.entries {
            let mut owners = vec![];
            for u in &e.user_ids {
                if let Some(u) = User::find(c, *u)? {
                    owners.push(format!("@{}", u.username));
                }
            }
            for g in &e.group_ids {
                if let Some(g) = Namespace::find(c, *g)? {
                    owners.push(format!("@{}", g.full_path));
                }
            }
            entries.push(json!({"pattern": e.pattern, "owners": owners}));
        }
        let name = (s.name != labrat_core::services::code_owners::DEFAULT_SECTION).then_some(s.name.clone());
        out.push(json!({"name": name, "approvals": s.approvals, "optional": s.optional, "entries": entries}));
    }
    Ok(json!({"branch": branch, "sections": out}))
}

/// EE code owners (labrat doesn't read `CODEOWNERS` files). `GET` shows a branch's
/// (`?branch=`, default: the default branch).
async fn get_code_owners(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let branch = req.params.str("branch");
    let body = req.app.db.read(|c| {
        let p = ci_project(c, &id)?;
        let branch = branch.unwrap_or_else(|| labrat_core::repo::default_branch(&p));
        Ok::<_, crate::ctx::ApiError>(code_owners_json(c, &p, &branch)?)
    })?;
    Ok(Resp::ok(body))
}

/// Sets a branch's code owners, as the sections of a `CODEOWNERS` file: `{"branch": "main",
/// "sections": [{"name": null, "entries": [{"pattern": "*", "owners": ["@alice"]}]},
/// {"name": "Backend", "approvals": 2, "optional": false, "entries": [{"pattern": "/app/",
/// "owners": ["@bob", "@group/path"]}]}]}`. The open MRs targeting the branch get their code
/// owner rules synced at once.
async fn put_code_owners(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let raw = req.params.raw_all();
    let body = req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let branch = raw["branch"].as_str().map(str::to_string).unwrap_or_else(|| labrat_core::repo::default_branch(&p));
        let sections = labrat_core::services::code_owners::parse(c, &raw["sections"])?.map_err(|m| labrat_core::Error::message(400, m))?;
        labrat_core::services::code_owners::set_config(c, &p, &branch, Some(&sections))?;
        Ok::<_, crate::ctx::ApiError>(code_owners_json(c, &p, &branch)?)
    })?;
    Ok(Resp::ok(body))
}

/// Removes a branch's code owners (`?branch=`, default: the default branch).
async fn delete_code_owners(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let branch = req.params.str("branch");
    req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let branch = branch.unwrap_or_else(|| labrat_core::repo::default_branch(&p));
        labrat_core::services::code_owners::set_config(c, &p, &branch, None)?;
        Ok::<_, crate::ctx::ApiError>(())
    })?;
    Ok(Resp::no_content())
}

fn policies_json(c: &rusqlite::Connection, p: &labrat_core::model::Project) -> labrat_core::Result<serde_json::Value> {
    let list: Vec<serde_json::Value> = labrat_core::services::policies::project_policies(c, p.id)?.into_iter().map(|p| p.content).collect();
    Ok(json!({"approval_policy": list}))
}

/// EE approval policies (labrat links no security policy project): `GET` shows the project's.
async fn get_policies(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let body = req.app.db.read(|c| {
        let p = ci_project(c, &id)?;
        Ok::<_, crate::ctx::ApiError>(policies_json(c, &p)?)
    })?;
    Ok(Resp::ok(body))
}

/// Replaces the project's approval policies: `{"approval_policy": [...]}`, the list of a
/// security policy file in GitLab's schema (as JSON). Open MRs get their policy rules and are
/// evaluated at once.
async fn put_policies(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let raw = req.params.raw_all();
    let list = raw["approval_policy"].clone();
    labrat_core::services::policies::validate(&list).map_err(|m| labrat_core::Error::message(400, m))?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        labrat_core::services::policies::set_policies(c, &app.config, &p, &list)?;
        Ok::<_, crate::ctx::ApiError>(policies_json(c, &p)?)
    })?;
    Ok(Resp::ok(body))
}

/// Sets approval policies' results on one MR, by policy name (`null`: evaluated as labrat does
/// by default): `{"policies": {"No critical vulns": {"violated": true, "status": "FAILED",
/// "newScanFinding": [{"name": "SQL injection", "path": "app.rb", "reportType": "SAST",
/// "severity": "CRITICAL", "location": {}}]}, "Licenses": {"violated": false}}}`. The lists
/// take GraphQL's `PolicyViolationDetails` shapes (`newScanFinding`, `previousScanFinding`,
/// `licenseScanning`, `errors`, `comparisonPipelines`). A policy that isn't violated asks for
/// no approvals and isn't reported; `FAILED` and `RUNNING` policies ask for theirs.
async fn put_policy_results(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let iid: i64 = req.path_param("merge_request_iid").parse().map_err(|_| labrat_core::Error::message(400, "merge_request_iid is invalid"))?;
    let raw = req.params.raw_all();
    let Some(map) = raw["policies"].as_object() else {
        return Err(labrat_core::Error::message(400, "policies must be an object").into());
    };
    let results: Vec<(String, Option<serde_json::Value>)> = map.iter().map(|(k, v)| (k.clone(), (!v.is_null()).then(|| v.clone()))).collect();
    let app = req.app.clone();
    app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let mr = labrat_core::services::merge_requests::MergeRequest::find_by_iid(c, p.id, iid)?.ok_or(labrat_core::Error::not_found("Merge request"))?;
        labrat_core::services::policies::set_results(c, &app.config, &p, &mr, &results)?.map_err(|m| labrat_core::Error::message(400, m))?;
        Ok::<_, crate::ctx::ApiError>(())
    })?;
    Ok(Resp::no_content())
}

/// Removes the project's approval policies.
async fn delete_policies(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let app = req.app.clone();
    app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        labrat_core::services::policies::set_policies(c, &app.config, &p, &json!([]))?;
        Ok::<_, crate::ctx::ApiError>(())
    })?;
    Ok(Resp::no_content())
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

/// Adds demo projects, merge requests and users (see `crate::demo`).
async fn demo(req: Req) -> ApiResult {
    let body = crate::demo::generate(req.app.clone()).await?;
    Ok(Resp::created(body))
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

fn run_pipeline(c: &rusqlite::Connection, p: &labrat_core::model::Project, id: &str) -> Result<labrat_core::services::ci::Pipeline, crate::ctx::ApiError> {
    let id: i64 = id.parse().map_err(|_| labrat_core::Error::message(400, "pipeline_id is invalid"))?;
    Ok(labrat_core::services::ci::Pipeline::find(c, id)?.filter(|pl| pl.project_id == p.id).ok_or(labrat_core::Error::not_found("Pipeline"))?)
}

/// The user a run is created as when the spec names none: the project's creator.
fn run_user(c: &rusqlite::Connection, p: &labrat_core::model::Project) -> Result<labrat_core::model::User, crate::ctx::ApiError> {
    let id: Option<i64> = c.query_row("SELECT creator_id FROM projects WHERE id = ?1", [p.id], |r| r.get(0)).unwrap_or(None);
    let u = match id {
        Some(i) => labrat_core::model::User::find(c, i)?,
        None => None,
    };
    match u {
        Some(u) => Ok(u),
        None => Ok(labrat_core::model::User::find_by_username(c, "root")?.ok_or(labrat_core::Error::not_found("User"))?),
    }
}

/// The project's pipeline runs, newest first, as the test endpoints describe them.
async fn list_runs(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let body = req.app.db.read(|c| {
        let p = ci_project(c, &id)?;
        let mut out = vec![];
        for pl in labrat_core::services::ci::Pipeline::list(c, p.id)? {
            out.push(labrat_core::services::pipelines::run_json(c, &pl)?);
        }
        Ok::<_, crate::ctx::ApiError>(serde_json::Value::Array(out))
    })?;
    Ok(Resp::ok(body))
}

async fn show_run(req: Req) -> ApiResult {
    let (id, pid) = (req.path_param("id").to_string(), req.path_param("pipeline_id").to_string());
    let body = req.app.db.read(|c| {
        let p = ci_project(c, &id)?;
        let pl = run_pipeline(c, &p, &pid)?;
        Ok::<_, crate::ctx::ApiError>(labrat_core::services::pipelines::run_json(c, &pl)?)
    })?;
    Ok(Resp::ok(body))
}

/// Creates a pipeline run (see `labrat_core::services::pipelines::create_run`).
async fn create_run(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let spec = req.params.raw_all();
    let body = req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let user = run_user(c, &p)?;
        let pl = labrat_core::services::pipelines::create_run(c, &req.app.config, &p, &user, &spec)?;
        Ok::<_, crate::ctx::ApiError>(labrat_core::services::pipelines::run_json(c, &pl)?)
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::created(body))
}

/// Moves a pipeline run on (see `labrat_core::services::pipelines::update_run`).
async fn update_run(req: Req) -> ApiResult {
    let (id, pid) = (req.path_param("id").to_string(), req.path_param("pipeline_id").to_string());
    let spec = req.params.raw_all();
    let body = req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let pl = run_pipeline(c, &p, &pid)?;
        let pl = labrat_core::services::pipelines::update_run(c, &req.app.config, &pl, &spec)?;
        Ok::<_, crate::ctx::ApiError>(labrat_core::services::pipelines::run_json(c, &pl)?)
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

async fn delete_run(req: Req) -> ApiResult {
    let (id, pid) = (req.path_param("id").to_string(), req.path_param("pipeline_id").to_string());
    req.app.db.tx(|c| {
        let p = ci_project(c, &id)?;
        let pl = run_pipeline(c, &p, &pid)?;
        let mrs = labrat_core::services::ci::all_merge_requests(c, &p, &pl)?;
        c.execute("DELETE FROM ci_pipelines WHERE id = ?1", [pl.id])?;
        for mr in mrs {
            labrat_core::services::ci::update_head_pipeline(c, &p, &mr)?;
        }
        Ok::<_, crate::ctx::ApiError>(())
    })?;
    Ok(Resp::no_content())
}
