//! CI jobs and their artifacts (`lib/api/ci/jobs.rb`, `lib/api/ci/job_artifacts.rb`), for the
//! pipelines labrat's test endpoints describe and its fake CI creates.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::user::{self as user_ent, Ctx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use labrat_core::model::{Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::ci::Pipeline;
use labrat_core::services::pipelines::{self, Artifact, Job};
use labrat_core::{App, Config, Error, repo, time};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/jobs", get(list_jobs))
        .route("/projects/{id}/jobs/artifacts/{*rest}", get(artifacts_by_ref))
        .route("/projects/{id}/jobs/{job_id}", get(show_job))
        .route("/projects/{id}/jobs/{job_id}/trace", get(job_trace))
        .route("/projects/{id}/jobs/{job_id}/cancel", post(cancel_job))
        .route("/projects/{id}/jobs/{job_id}/retry", post(retry_job))
        .route("/projects/{id}/jobs/{job_id}/erase", post(erase_job))
        .route("/projects/{id}/jobs/{job_id}/play", post(play_job))
        .route("/projects/{id}/jobs/{job_id}/artifacts", get(download_artifacts).delete(delete_artifacts))
        .route("/projects/{id}/jobs/{job_id}/artifacts/tree", get(artifacts_tree))
        .route("/projects/{id}/jobs/{job_id}/artifacts/keep", post(keep_artifacts))
        .route("/projects/{id}/jobs/{job_id}/artifacts/{*artifact_path}", get(artifact_file))
        .route("/projects/{id}/artifacts", axum::routing::delete(delete_project_artifacts))
}

/// The user who registered labrat's fake runner: the oldest admin other than root (the
/// reference instances' runners were registered by their admin), else root.
fn runner_owner(c: &Connection) -> labrat_core::Result<Option<User>> {
    let id: Option<i64> = c
        .query_row(
            "SELECT id FROM users WHERE admin = 1 AND state = 'active' ORDER BY (username = 'root'), id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(id.map(|i| User::find(c, i)).transpose()?.flatten())
}

/// `Entities::Ci::Runner` and `RunnerManager` for labrat's fake runner.
fn runner_values(c: &Connection, config: &Config) -> labrat_core::Result<(Value, Value)> {
    let busy: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM commit_statuses WHERE build = 1 AND status IN ('running', 'canceling') AND runner_id IS NOT NULL)", [], |r| r.get(0))?;
    let exec = if busy { "active" } else { "idle" };
    let owner = runner_owner(c)?;
    let ctx = Ctx { base_url: &config.external_url, ee: config.is_ee(), current_user: None };
    let created = owner.as_ref().map(|u| u.created_at).unwrap_or(0);
    let runner = json!({
        "id": pipelines::FAKE_RUNNER_ID,
        "description": "labrat-fake-runner",
        "ip_address": null,
        "active": true,
        "paused": false,
        "is_shared": true,
        "runner_type": "instance_type",
        "name": null,
        "online": true,
        "created_by": owner.as_ref().map(|u| user_ent::basic_value(u, &ctx)),
        "created_at": time::iso(created),
        "status": "online",
        "job_execution_status": exec,
    });
    let manager = json!({
        "id": pipelines::FAKE_RUNNER_ID,
        "system_id": "<legacy>",
        "version": "18.11.0",
        "revision": "fake",
        "platform": "linux",
        "architecture": "amd64",
        "created_at": time::iso(created),
        "contacted_at": time::iso(time::now()),
        "ip_address": "127.0.0.1",
        "status": "online",
        "job_execution_status": exec,
    });
    Ok((runner, manager))
}

/// `Entities::Ci::JobBasic` fields, in order, up to `web_url` and `project`.
pub fn job_basic(c: &Connection, config: &Config, p: &Project, j: &Job) -> labrat_core::Result<Map<String, Value>> {
    let pl = Pipeline::find(c, j.pipeline_id)?.ok_or(Error::not_found("Pipeline"))?;
    let mut m = Map::new();
    m.insert("id".into(), json!(j.id));
    m.insert("status".into(), json!(j.status));
    m.insert("stage".into(), json!(j.stage));
    m.insert("name".into(), json!(j.name));
    m.insert("ref".into(), json!(j.ref_));
    m.insert("tag".into(), json!(pl.tag));
    m.insert("coverage".into(), json!(j.coverage));
    m.insert("allow_failure".into(), json!(j.allow_failure));
    m.insert("created_at".into(), json!(time::iso(j.created_at)));
    m.insert("started_at".into(), json!(j.started_at.map(time::iso)));
    m.insert("finished_at".into(), json!(j.finished_at.map(time::iso)));
    m.insert("erased_at".into(), json!(j.erased_at.map(time::iso)));
    m.insert("duration".into(), json!(j.duration()));
    m.insert("queued_duration".into(), json!(j.queued_duration()));
    // Jobs are presented without the current user.
    let ctx = Ctx { base_url: &config.external_url, ee: config.is_ee(), current_user: None };
    let user = match j.user_id.map(|i| User::find(c, i)).transpose()?.flatten() {
        Some(u) => Value::Object(user_ent::user(&u, &ctx)),
        None => Value::Null,
    };
    m.insert("user".into(), user);
    let r = repo::repo(c, p);
    let commit = match labrat_git::Oid::from_hex(&j.sha).ok().and_then(|o| r.commit(&o).ok().map(|cm| (o, cm))) {
        Some((o, cm)) => Value::Object(crate::entities::repository::commit(config, p, &o, &cm)),
        None => Value::Null,
    };
    m.insert("commit".into(), commit);
    m.insert("pipeline".into(), Value::Object(crate::routes::pipelines::pipeline_basic(config, p, &pl)));
    if j.status == "failed" {
        m.insert("failure_reason".into(), json!(j.failure_reason.clone().unwrap_or_else(|| "unknown_failure".into())));
    }
    m.insert("web_url".into(), json!(config.url(&format!("/{}/-/jobs/{}", p.full_path, j.id))));
    m.insert("project".into(), json!({"ci_job_token_scope_enabled": false}));
    Ok(m)
}

/// `Entities::Ci::Job`.
pub fn job_value(c: &Connection, config: &Config, p: &Project, j: &Job) -> labrat_core::Result<Value> {
    job_value_with(c, config, p, j, None)
}

/// `Entities::Ci::Job`, with the archive the job had before an erase (GitLab presents the
/// erased job with its cached `artifacts_file`).
fn job_value_with(c: &Connection, config: &Config, p: &Project, j: &Job, old_archive: Option<(String, i64)>) -> labrat_core::Result<Value> {
    let mut m = job_basic(c, config, p, j)?;
    let arts = Artifact::for_job(c, j.id)?;
    if let Some(a) = arts.iter().find(|a| a.file_type == "archive") {
        m.insert("artifacts_file".into(), json!({"filename": a.filename, "size": a.size}));
    } else if let Some((f, size)) = old_archive {
        m.insert("artifacts_file".into(), json!({"filename": f, "size": size}));
    }
    m.insert(
        "artifacts".into(),
        Value::Array(arts.iter().map(|a| json!({"file_type": a.file_type, "size": a.size, "filename": a.filename, "file_format": a.file_format})).collect()),
    );
    let (runner, manager) = match j.runner_id {
        Some(_) => runner_values(c, config)?,
        None => (Value::Null, Value::Null),
    };
    m.insert("runner".into(), runner);
    m.insert("runner_manager".into(), manager);
    m.insert("artifacts_expire_at".into(), json!(j.artifacts_expire_at.map(time::iso)));
    m.insert("archived".into(), json!(false));
    m.insert("tag_list".into(), json!(j.tags));
    Ok(Value::Object(m))
}

fn not_found(what: &'static str) -> ApiError {
    Error::not_found(what).into()
}

/// `authorize_read_builds!`: reporters; guests and anyone who can see the project too while
/// its jobs are public (`public_jobs`, the default).
fn read_builds(c: &Connection, req: &Req) -> Result<(Project, ProjectPerms), ApiError> {
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    let public_jobs = p.setting("public_jobs").and_then(Value::as_bool).unwrap_or(true);
    if !(perms.reporter() || req.is_admin() || (public_jobs && perms.read_project())) {
        return Err(Error::forbidden().into());
    }
    Ok((p, perms))
}

fn job_id(req: &Req) -> Result<i64, ApiError> {
    req.path_param("job_id").parse().map_err(|_| Error::Param("job_id is invalid".into()).into())
}

/// `find_build!`
fn load_job(c: &Connection, req: &Req, p: &Project) -> Result<Job, ApiError> {
    Job::find_in(c, p.id, job_id(req)?)?.ok_or_else(|| Error::Api(404, json!("404 Not found")).into())
}

/// `filter_builds_by_scope`: `scope` is one status or a list of them.
pub fn scope_filter(req: &Req) -> Result<Option<Vec<String>>, ApiError> {
    const SCOPES: &[&str] = &["created", "pending", "running", "failed", "success", "canceled", "skipped", "waiting_for_resource", "manual", "canceling", "preparing", "scheduled", "waiting_for_callback"];
    let raw = req.params.raw("scope").or(req.params.raw("scope[]"));
    let list: Vec<String> = match raw {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(_) => return Err(Error::Param("scope does not have a valid value".into()).into()),
    };
    if list.iter().any(|s| !SCOPES.contains(&s.as_str())) {
        return Err(Error::Param("scope does not have a valid value".into()).into());
    }
    Ok(Some(list))
}

async fn list_jobs(req: Req) -> ApiResult {
    let scope = scope_filter(&req)?;
    let ref_ = req.params.str("ref");
    let app = req.app.clone();
    app.db.read(|c| {
        req.user()?;
        let (p, _) = read_builds(c, &req)?;
        let mut jobs = Job::for_project(c, p.id)?;
        jobs.reverse();
        jobs.retain(|j| scope.as_ref().is_none_or(|s| s.contains(&j.status)) && ref_.as_ref().is_none_or(|r| j.ref_.as_ref() == Some(r)));
        let lreq = req.with_array_query("scope");
        crate::pagination::paginate_without_count(&lreq, jobs, &[], |j| Ok(job_value(c, &req.app.config, &p, j)?))
    })
}

/// `GET /projects/:id/pipelines/:pipeline_id/jobs`.
pub fn pipeline_jobs(c: &Connection, req: &Req, p: &Project, pl: &Pipeline) -> ApiResult {
    let scope = scope_filter(req)?;
    let include_retried = req.params.bool_or("include_retried", false)?;
    let mut jobs = Job::for_pipeline(c, pl.id, include_retried)?;
    jobs.reverse();
    jobs.retain(|j| scope.as_ref().is_none_or(|s| s.contains(&j.status)));
    let lreq = req.with_array_query("scope");
    paginate(&lreq, jobs, &[("include_retried", "false")], |j| Ok(job_value(c, &req.app.config, p, j)?))
}

async fn show_job(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        req.user()?;
        let (p, _) = read_builds(c, &req)?;
        let j = load_job(c, &req, &p)?;
        Ok(Resp::ok(job_value(c, &req.app.config, &p, &j)?))
    })
}

async fn job_trace(req: Req) -> Response {
    let app = req.app.clone();
    let res: Result<(i64, Vec<u8>), ApiError> = app.db.read(|c| {
        req.user()?;
        let (p, _) = read_builds(c, &req)?;
        let j = load_job(c, &req, &p)?;
        Ok((j.id, j.trace(c)?.unwrap_or_default()))
    });
    match res {
        Ok((id, data)) => Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .header("Content-Disposition", format!("infile; filename=\"{id}.log\""))
            .body(axum::body::Body::from(data))
            .unwrap(),
        Err(e) => e.into_response(),
    }
}

/// The job a user changes (cancel, retry, play, erase, keep artifacts): developers (and admins)
/// who can also update the job's ref (`update_build`; see `pipelines::can_update`).
fn update_job(c: &Connection, req: &Req) -> Result<(Project, User, Job), ApiError> {
    let u = req.user()?.clone();
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !(perms.developer() || u.admin) {
        return Err(Error::forbidden().into());
    }
    let j = load_job(c, req, &p)?;
    let pl = Pipeline::find(c, j.pipeline_id)?;
    if !pl.map(|pl| pipelines::can_update(c, &u, &p, &pl)).transpose()?.unwrap_or(false) {
        return Err(Error::forbidden().into());
    }
    Ok((p, u, j))
}

async fn cancel_job(req: Req) -> ApiResult {
    let app = req.app.clone();
    let r = app.db.tx(|c| {
        let (p, _, j) = update_job(c, &req)?;
        let j = pipelines::cancel_job(c, &req.app.config, &j)?;
        Ok::<_, ApiError>(Resp::created(job_value(c, &req.app.config, &p, &j)?))
    })?;
    crate::jobs::kick(&req.app);
    Ok(r)
}

async fn retry_job(req: Req) -> ApiResult {
    let app = req.app.clone();
    let r = app.db.tx(|c| {
        let (p, u, j) = update_job(c, &req)?;
        if !j.is_retryable() {
            return Err(Error::Api(403, json!("403 Forbidden - Job is not retryable")).into());
        }
        let j = pipelines::retry_job(c, &req.app.config, &j, &u)?;
        Ok::<_, ApiError>(Resp::created(job_value(c, &req.app.config, &p, &j)?))
    })?;
    crate::jobs::kick(&req.app);
    Ok(r)
}

async fn erase_job(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, u, j) = update_job(c, &req)?;
        let pl = Pipeline::find(c, j.pipeline_id)?;
        if !pl.map(|pl| pipelines::can_erase(c, &u, &p, &pl, &j)).transpose()?.unwrap_or(false) {
            return Err(Error::forbidden().into());
        }
        if !j.is_erasable(c)? {
            return Err(Error::Api(403, json!("403 Forbidden - Job is not erasable!")).into());
        }
        let archive = Artifact::for_job(c, j.id)?.into_iter().find(|a| a.file_type == "archive").map(|a| (a.filename, a.size));
        let j = pipelines::erase_job(c, &req.app.config, &j, &u)?;
        Ok(Resp::created(job_value_with(c, &req.app.config, &p, &j, archive)?))
    })
}

async fn play_job(req: Req) -> ApiResult {
    let app = req.app.clone();
    let r = app.db.tx(|c| {
        let (p, u, j) = update_job(c, &req)?;
        if !j.is_playable() {
            return Err(Error::Api(400, json!("400 Bad request - Unplayable Job")).into());
        }
        let j = pipelines::play_job(c, &req.app.config, &j, &u)?;
        Ok::<_, ApiError>(Resp::ok(job_value(c, &req.app.config, &p, &j)?))
    })?;
    crate::jobs::kick(&req.app);
    Ok(r)
}

fn file_response(data: Vec<u8>, filename: &str, content_type: &str) -> Response {
    Response::builder()
        .status(200)
        .header("Content-Type", content_type)
        .header("Content-Disposition", format!("attachment; filename=\"{filename}\"; filename*=UTF-8''{filename}"))
        .body(axum::body::Body::from(data))
        .unwrap()
}

/// `present_artifacts_file!`: the archive.
async fn download_artifacts(req: Req) -> Response {
    let app = req.app.clone();
    let res: Result<(Artifact, Vec<u8>), ApiError> = app.db.read(|c| {
        let (p, _) = read_builds(c, &req)?;
        let j = load_job(c, &req, &p)?;
        let file_type = req.params.str("file_type").unwrap_or("archive".into());
        pipelines::artifact_of(c, j.id, &file_type)?.ok_or_else(not_found_plain)
    });
    match res {
        Ok((a, data)) => file_response(data, &a.filename, "application/octet-stream"),
        Err(e) => e.into_response(),
    }
}

/// `not_found!` without a resource.
fn not_found_plain() -> ApiError {
    Error::Api(404, json!("404 Not Found")).into()
}

/// `ActiveRecord::RecordNotFound`.
fn record_not_found() -> ApiError {
    Error::Api(404, json!("404 Not found")).into()
}

/// The content type GitLab Workhorse sends for an archive entry, by its extension (the
/// reference instance's MIME table).
fn entry_content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "txt" | "log" | "text" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "xml" => "text/xml; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        _ => "application/octet-stream",
    }
}

/// One file of the archive: Workhorse answers a plain `404 page not found` when it isn't there.
fn entry_response(archive: &[u8], path: &str, by_type: bool) -> Response {
    let found = labrat_core::zip::read(archive).and_then(|entries| entries.into_iter().find(|e| e.name == path && !e.is_dir())).and_then(|e| e.data());
    match found {
        Some(data) => {
            let name = path.rsplit('/').next().unwrap_or(path);
            // The job's file endpoint picks a type by extension; the ref one always sends bytes.
            let ctype = if by_type { entry_content_type(path) } else { "application/octet-stream" };
            file_response(data, name, ctype)
        }
        None => Response::builder()
            .status(404)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from("404 page not found\n"))
            .unwrap(),
    }
}

async fn artifact_file(req: Req) -> Response {
    let app = req.app.clone();
    let path = req.path_param("artifact_path").to_string();
    let res: Result<Response, ApiError> = app.db.read(|c| {
        let (p, _) = read_builds(c, &req)?;
        let j = load_job(c, &req, &p)?;
        let (_, data) = pipelines::artifact_of(c, j.id, "archive")?.ok_or_else(not_found_plain)?;
        Ok(entry_response(&data, &path, true))
    });
    res.unwrap_or_else(|e| e.into_response())
}

/// `latest_successful_build_for_ref!`: the job of that name in the ref's latest successful
/// pipeline.
fn latest_successful_build(c: &Connection, p: &Project, job: &str, ref_name: &str) -> Result<Job, ApiError> {
    let mut pls = Pipeline::list(c, p.id)?;
    pls.retain(|pl| pl.ref_.as_deref() == Some(ref_name) && pl.status == "success");
    let pl = pls.first().ok_or_else(record_not_found)?;
    Job::for_pipeline(c, pl.id, false)?.into_iter().rev().find(|j| j.name == job && j.status == "success").ok_or_else(record_not_found)
}

/// `GET .../jobs/artifacts/:ref_name/download?job=` and `.../raw/*artifact_path?job=`.
async fn artifacts_by_ref(req: Req) -> Response {
    let rest = req.path_param("rest").to_string();
    let app = req.app.clone();
    let res: Result<Response, ApiError> = app.db.read(|c| {
        let (p, _) = read_builds(c, &req)?;
        req.params.require(&["job"])?;
        let job = req.params.str("job").unwrap_or_default();
        if let Some(r) = rest.strip_suffix("/download") {
            let j = latest_successful_build(c, &p, &job, r)?;
            let (a, data) = pipelines::artifact_of(c, j.id, "archive")?.ok_or_else(record_not_found)?;
            return Ok(file_response(data, &a.filename, "application/octet-stream"));
        }
        if let Some(i) = rest.find("/raw/") {
            let (r, path) = (&rest[..i], &rest[i + 5..]);
            let j = latest_successful_build(c, &p, &job, r)?;
            let (_, data) = pipelines::artifact_of(c, j.id, "archive")?.ok_or_else(record_not_found)?;
            return Ok(entry_response(&data, path, false));
        }
        Err(Error::Api(404, json!({"error": "404 Not Found"})).into())
    });
    res.unwrap_or_else(|e| e.into_response())
}

/// `GET .../artifacts/tree`: the archive's entries under `path`.
async fn artifacts_tree(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = read_builds(c, &req)?;
        let j = load_job(c, &req, &p)?;
        let (_, data) = pipelines::artifact_of(c, j.id, "archive")?.ok_or_else(|| not_found("Artifacts"))?;
        let recursive = req.params.bool_or("recursive", false)?;
        let path = req.params.str("path").unwrap_or_default();
        let dir = if path.is_empty() { String::new() } else { format!("{}/", path.trim_end_matches('/')) };
        let _ = recursive;
        let entries = labrat_core::zip::read(&data).unwrap_or_default();
        // Every directory the files imply (without a mode), plus the files. GitLab 18.11 lists
        // one level even with `recursive`.
        let mut all: Vec<(String, Option<u64>, Option<u32>)> = vec![];
        for e in &entries {
            let parts: Vec<&str> = e.name.trim_end_matches('/').split('/').collect();
            for i in 1..parts.len() {
                let d = format!("{}/", parts[..i].join("/"));
                if !all.iter().any(|(n, _, _)| *n == d) {
                    all.push((d, None, None));
                }
            }
            if e.is_dir() {
                match all.iter_mut().find(|(n, _, _)| *n == e.name) {
                    Some(x) => x.2 = e.mode,
                    None => all.push((e.name.clone(), None, e.mode)),
                }
            } else {
                all.push((e.name.clone(), Some(e.size), Some(e.mode.unwrap_or(0o644))));
            }
        }
        if !dir.is_empty() && !all.iter().any(|(n, s, _)| *n == dir && s.is_none()) {
            return Err(not_found("Path"));
        }
        let mut dirs: Vec<&(String, Option<u64>, Option<u32>)> = all.iter().filter(|(n, s, _)| s.is_none() && is_child(n, &dir)).collect();
        let mut files: Vec<&(String, Option<u64>, Option<u32>)> = all.iter().filter(|(n, s, _)| s.is_some() && is_child(n, &dir)).collect();
        dirs.sort();
        files.sort();
        let list: Vec<&(String, Option<u64>, Option<u32>)> = dirs.into_iter().chain(files).collect();
        paginate(&req, list, &[("path", ""), ("recursive", "false")], |(n, size, mode)| {
            let name = n.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
            let mode = mode.map(|m| format!("{:06o}", m & 0o7777));
            Ok(match size {
                None => json!({"name": name, "path": n, "type": "directory", "mode": mode}),
                Some(s) => json!({"name": name, "path": n, "type": "file", "size": s, "mode": mode}),
            })
        })
    })
}

/// Whether `name` is directly inside the directory `dir` (`""` for the root).
fn is_child(name: &str, dir: &str) -> bool {
    name.len() > dir.len() && name.starts_with(dir) && !name[dir.len()..].trim_end_matches('/').contains('/')
}

async fn keep_artifacts(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _, j) = update_job(c, &req)?;
        if Artifact::for_job(c, j.id)?.is_empty() {
            // `not_found!(build)` prints the job object.
            return Err(Error::Api(404, json!("404 #<Ci::Build:0x00007f0000000000> Not Found")).into());
        }
        c.execute("UPDATE ci_job_artifacts SET expire_at = NULL WHERE job_id = ?1", [j.id])?;
        c.execute("UPDATE commit_statuses SET artifacts_expire_at = NULL WHERE id = ?1", [j.id])?;
        let j = Job::find(c, j.id)?.expect("exists");
        Ok(Resp::ok(job_value(c, &req.app.config, &p, &j)?))
    })
}

/// Maintainers delete artifacts.
fn delete_access(c: &Connection, req: &Req) -> Result<Project, ApiError> {
    let u = req.user()?;
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !(perms.admin_project() || u.admin) {
        return Err(Error::forbidden().into());
    }
    Ok(p)
}

async fn delete_artifacts(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let p = delete_access(c, &req)?;
        let j = load_job(c, &req, &p)?;
        // `Ci::JobArtifacts::DeleteService`: the erasable artifacts (all but the log) go; the
        // job keeps its `artifacts_expire_at`.
        c.execute("DELETE FROM ci_job_artifacts WHERE job_id = ?1 AND file_type != 'trace'", [j.id])?;
        Ok(Resp::no_content())
    })
}

/// `DeleteProjectArtifactsService`: every artifact expires now, except those of each ref's
/// latest successful pipeline, which are kept.
async fn delete_project_artifacts(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let p = delete_access(c, &req)?;
        let now = time::now();
        let mut kept: Vec<i64> = vec![];
        let mut seen: Vec<String> = vec![];
        for pl in Pipeline::list(c, p.id)? {
            if pl.status != "success" {
                continue;
            }
            let r = pl.ref_.clone().unwrap_or_default();
            if !seen.contains(&r) {
                seen.push(r);
                kept.push(pl.id);
            }
        }
        let kept_json = json!(kept).to_string();
        c.execute(
            "UPDATE ci_job_artifacts SET expire_at = ?1 WHERE project_id = ?2
             AND job_id NOT IN (SELECT id FROM commit_statuses WHERE pipeline_id IN (SELECT value FROM json_each(?3)))",
            rusqlite::params![now, p.id, kept_json],
        )?;
        Ok(Resp::status(202, json!({"message": "202 Accepted"})))
    })
}
