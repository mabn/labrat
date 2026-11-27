//! labrat's fake CI: merge request pipelines without CI configuration or runners.
//!
//! labrat doesn't read `.gitlab-ci.yml`. A project gets merge request pipelines once its fake
//! CI is switched on (`PUT /-/labrat/projects/:id/ci`), as if its configuration had these jobs
//! run for `merge_request_event` only:
//!
//! - an MR gets a pipeline when it is created and when its source branch gets commits
//!   (`MergeRequests::CreatePipelineService`): a merged results pipeline on
//!   `refs/merge-requests/:iid/merge` when the project has merged results pipelines on, else a
//!   detached one on `refs/merge-requests/:iid/head`;
//! - merge trains run their car pipelines on `refs/merge-requests/:iid/train`.
//!
//! Each pipeline has one job per configured name. The fake runner finishes them:
//!
//! - in `manual` mode jobs stay pending until `/-/labrat/ci/pick` starts them and
//!   `/-/labrat/ci/finish` finishes them;
//! - in `auto` mode each job starts at once and finishes after `duration_ms` with `status`
//!   (or the status `overrides` gives for its ref).
//!
//! Retrying a pipeline (`POST /projects/:id/pipelines/:pipeline_id/retry`) gives its failed and
//! canceled jobs pending copies in the same pipeline.

use crate::model::{Project, User};
use crate::services::ci::{self, Pipeline};
use crate::services::merge_requests::{MergeRequest, next_iid};
use crate::{Config, Error, Result, jobs, repo, time};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct FakeCi {
    pub jobs: Vec<String>,
    pub mode: String,
    pub status: String,
    pub duration_ms: i64,
    pub overrides: serde_json::Map<String, Value>,
}

impl Default for FakeCi {
    fn default() -> Self {
        FakeCi { jobs: vec!["test".into()], mode: "manual".into(), status: "success".into(), duration_ms: 0, overrides: Default::default() }
    }
}

impl FakeCi {
    pub fn to_json(&self) -> Value {
        json!({
            "enabled": true,
            "jobs": self.jobs,
            "runner": {"mode": self.mode, "status": self.status, "duration_ms": self.duration_ms, "overrides": self.overrides},
        })
    }
}

/// The project's fake CI, when it is on.
pub fn config(c: &Connection, project_id: i64) -> Result<Option<FakeCi>> {
    Ok(c
        .query_row(
            "SELECT jobs, mode, status, duration_ms, overrides FROM fake_ci WHERE project_id = ?1",
            [project_id],
            |r| {
                let jobs: String = r.get(0)?;
                let overrides: String = r.get(4)?;
                Ok(FakeCi {
                    jobs: serde_json::from_str(&jobs).unwrap_or_default(),
                    mode: r.get(1)?,
                    status: r.get(2)?,
                    duration_ms: r.get(3)?,
                    overrides: serde_json::from_str(&overrides).unwrap_or_default(),
                })
            },
        )
        .optional()?)
}

/// Switches the project's fake CI on (with `Some`) or off.
pub fn set_config(c: &Connection, project_id: i64, cfg: Option<&FakeCi>) -> Result<()> {
    c.execute("DELETE FROM fake_ci WHERE project_id = ?1", [project_id])?;
    if let Some(f) = cfg {
        c.execute(
            "INSERT INTO fake_ci (project_id, jobs, mode, status, duration_ms, overrides) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project_id, json!(f.jobs).to_string(), f.mode, f.status, f.duration_ms, Value::Object(f.overrides.clone()).to_string()],
        )?;
    }
    Ok(())
}

/// Parses the body of `PUT /-/labrat/projects/:id/ci`; `None` switches fake CI off.
pub fn parse_config(body: &Value) -> std::result::Result<Option<FakeCi>, String> {
    if body["enabled"] == json!(false) {
        return Ok(None);
    }
    let mut f = FakeCi::default();
    if let Some(a) = body["jobs"].as_array() {
        f.jobs = a.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        if f.jobs.is_empty() {
            return Err("jobs must name at least one job".into());
        }
    }
    let r = &body["runner"];
    if let Some(m) = r["mode"].as_str() {
        if !matches!(m, "manual" | "auto") {
            return Err("runner.mode must be manual or auto".into());
        }
        f.mode = m.into();
    }
    let valid = |s: &str| matches!(s, "success" | "failed" | "canceled");
    if let Some(s) = r["status"].as_str() {
        if !valid(s) {
            return Err("runner.status must be success, failed or canceled".into());
        }
        f.status = s.into();
    }
    if let Some(d) = r["duration_ms"].as_i64() {
        f.duration_ms = d.max(0);
    }
    if let Some(o) = r["overrides"].as_object() {
        for (k, v) in o {
            if !v.as_str().is_some_and(valid) {
                return Err(format!("runner.overrides.{k} must be success, failed or canceled"));
            }
        }
        f.overrides = o.clone();
    }
    Ok(Some(f))
}

/// `MergeRequests::CreatePipelineService`, run as a job after the MR is created or its source
/// branch gets commits: a merged results pipeline when the project has them on and the MR
/// merges cleanly, else a detached one.
pub fn create_mr_pipeline(c: &Connection, config: &Config, mr_id: i64, user_id: i64) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else { return Ok(()) };
    let Some(p) = Project::find(c, mr.target_project_id)? else { return Ok(()) };
    if !mr.is_open() || self::config(c, p.id)?.is_none() {
        return Ok(());
    }
    let Some(user) = User::find(c, user_id)? else { return Ok(()) };
    let Some(d) = mr.latest_diff(c)? else { return Ok(()) };
    let Some(head) = d.head_commit_sha.clone() else { return Ok(()) };
    if d.commits_count == 0 {
        return Ok(());
    }
    if let Ok(oid) = labrat_git::Oid::from_hex(&head)
        && ci::skips_ci(c, &p, &oid)?
    {
        return Ok(());
    }
    let r = repo::repo(c, &p);
    let merged = if crate::services::merge_trains::merge_pipelines_enabled(&p) {
        // The merge ref comes from a mergeability check (`MergeToRefService`), which settles a
        // pending merge status too.
        if matches!(mr.merge_status.as_str(), "unchecked" | "checking") {
            crate::services::merge_requests::mergeability::check(c, config, mr.id)?;
        }
        let mr = mr.reload(c)?;
        let target = r.branch(&mr.target_branch)?;
        match (crate::services::merge_requests::mergeability::test_merge(c, config, &p, &mr)?, target) {
            (Some(oid), Some(t)) => Some((oid.hex(), t.hex())),
            _ => None,
        }
    } else {
        None
    };
    let pl = match merged {
        Some((sha, target_sha)) => create(
            c,
            config,
            &p,
            &user,
            &sha,
            &format!("refs/merge-requests/{}/merge", mr.iid),
            Some((&mr, &head, &target_sha)),
        )?,
        None => create(c, config, &p, &user, &head, &format!("refs/merge-requests/{}/head", mr.iid), Some((&mr, "", "")))?,
    };
    ci::update_head_pipeline(c, &p, &mr)?;
    let _ = pl;
    Ok(())
}

/// Creates a `merge_request_event` pipeline with the fake CI's jobs, pending.
/// `mr` is `(mr, source_sha, target_sha)`; empty SHAs for a detached pipeline.
pub fn create(c: &Connection, config: &Config, p: &Project, user: &User, sha: &str, r#ref: &str, mr: Option<(&MergeRequest, &str, &str)>) -> Result<Pipeline> {
    let cfg = self::config(c, p.id)?.unwrap_or_default();
    let iid = next_iid(c, "ci_pipelines", p.id)?;
    let now = time::now();
    let (mr_id, source_sha, target_sha) = match mr {
        Some((m, s, t)) => (Some(m.id), (!s.is_empty()).then(|| s.to_string()), (!t.is_empty()).then(|| t.to_string())),
        None => (None, None, None),
    };
    c.execute(
        "INSERT INTO ci_pipelines (iid, project_id, sha, ref, tag, status, source, user_id, created_at, updated_at, before_sha, merge_request_id, source_sha, target_sha)
         VALUES (?1, ?2, ?3, ?4, 0, 'created', 'merge_request_event', ?5, ?6, ?6, ?7, ?8, ?9, ?10)",
        // Detached pipelines have no `before`; merged results and train pipelines get their own
        // SHA.
        params![iid, p.id, sha, r#ref, user.id, now, if target_sha.is_some() { sha.to_string() } else { labrat_git::Oid::ZERO.hex() }, mr_id, source_sha, target_sha],
    )?;
    let pid = c.last_insert_rowid();
    for name in &cfg.jobs {
        c.execute(
            "INSERT INTO commit_statuses (pipeline_id, project_id, name, ref, sha, status, user_id, created_at, updated_at, build, stage)
             VALUES (?1, ?2, ?3, ?4, ?5, 'created', ?6, ?7, ?7, 1, 'test')",
            params![pid, p.id, name, r#ref, sha, user.id, now],
        )?;
    }
    ci::recompute(c, config, pid)?;
    // The jobs are queued a moment later, as GitLab's pipeline processing does.
    jobs::enqueue(c, "fake_ci_enqueue", json!({"pipeline_id": pid}))?;
    Ok(Pipeline::find(c, pid)?.expect("just inserted"))
}

/// The pipeline's created jobs become pending (waiting for a runner); in auto mode the runner
/// takes them.
pub fn enqueue(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    let Some(pl) = Pipeline::find(c, pipeline_id)? else { return Ok(()) };
    let now = time::now();
    for id in job_ids(c, pipeline_id, &["created"])? {
        c.execute("UPDATE commit_statuses SET status = 'pending', queued_at = ?1, updated_at = ?1 WHERE id = ?2", params![now, id])?;
    }
    ci::recompute(c, config, pipeline_id)?;
    if self::config(c, pl.project_id)?.is_some_and(|f| f.mode == "auto") {
        jobs::enqueue(c, "fake_ci_run", json!({"pipeline_id": pipeline_id}))?;
    }
    Ok(())
}

/// The jobs of a pipeline.
fn job_ids(c: &Connection, pipeline_id: i64, status: &[&str]) -> Result<Vec<i64>> {
    let mut st = c.prepare("SELECT id, status FROM commit_statuses WHERE pipeline_id = ?1 AND build = 1 AND retried = 0 ORDER BY id")?;
    let rows: Vec<(i64, String)> = st.query_map([pipeline_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter(|(_, s)| status.contains(&s.as_str())).map(|(id, _)| id).collect())
}

fn set_job(c: &Connection, id: i64, status: &str) -> Result<()> {
    let now = time::now();
    match status {
        "running" => c.execute(
            "UPDATE commit_statuses SET status = 'running', started_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?,
        s => c.execute(
            "UPDATE commit_statuses SET status = ?1, finished_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![s, now, id],
        )?,
    };
    Ok(())
}

/// The fake runner picks up the project's pending jobs: they start running. Returns the
/// picked jobs as `(job id, ref)`.
/// With `ref`, only that ref's jobs.
pub fn pick(c: &Connection, config: &Config, project_id: i64, r#ref: Option<&str>) -> Result<Vec<(i64, String)>> {
    let mut st = c.prepare(
        "SELECT s.id, s.pipeline_id, coalesce(s.ref, '') FROM commit_statuses s WHERE s.project_id = ?1 AND s.build = 1 AND s.status = 'pending' AND s.retried = 0
         AND (?2 IS NULL OR s.ref = ?2) ORDER BY s.id",
    )?;
    let rows: Vec<(i64, i64, String)> = st.query_map(params![project_id, r#ref], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut pipelines = vec![];
    for (id, pl, _) in &rows {
        set_job(c, *id, "running")?;
        if !pipelines.contains(pl) {
            pipelines.push(*pl);
        }
    }
    for pl in pipelines {
        process_later(c, config, pl)?;
    }
    Ok(rows.into_iter().map(|(id, _, r)| (id, r)).collect())
}

/// Finishes the project's running (and, with `include_pending`, pending) jobs on `ref` with
/// `status`. Returns how many jobs finished.
pub fn finish(c: &Connection, config: &Config, project_id: i64, r#ref: &str, status: &str, include_pending: bool) -> Result<usize> {
    if !matches!(status, "success" | "failed" | "canceled") {
        return Err(Error::message(400, "status must be success, failed or canceled"));
    }
    let mut st = c.prepare(
        "SELECT id, pipeline_id, status FROM commit_statuses WHERE project_id = ?1 AND ref = ?2 AND build = 1 AND retried = 0 AND status IN ('pending', 'running', 'canceling') ORDER BY id",
    )?;
    let rows: Vec<(i64, i64, String)> = st.query_map(params![project_id, r#ref], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut n = 0;
    let mut pipelines = vec![];
    for (id, pl, s) in rows {
        if s == "pending" && !include_pending {
            continue;
        }
        if s == "pending" {
            set_job(c, id, "running")?;
        }
        // A job being canceled ends canceled, whatever the runner reports.
        set_job(c, id, if s == "canceling" { "canceled" } else { status })?;
        n += 1;
        if !pipelines.contains(&pl) {
            pipelines.push(pl);
        }
    }
    for pl in pipelines {
        process_later(c, config, pl)?;
    }
    Ok(n)
}

/// Cancels a pipeline's unfinished jobs (`Ci::CancelPipelineService`). Jobs a runner is
/// running become `canceling` until the runner reports back (`ci/finish`, or the auto runner).
pub fn cancel_pipeline(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    for id in job_ids(c, pipeline_id, &["created", "pending"])? {
        set_job(c, id, "canceled")?;
    }
    for id in job_ids(c, pipeline_id, &["running"])? {
        c.execute("UPDATE commit_statuses SET status = 'canceling', updated_at = ?1 WHERE id = ?2", params![time::now(), id])?;
    }
    ci::recompute(c, config, pipeline_id)
}

/// `POST /projects/:id/pipelines/:pipeline_id/retry` (`Ci::RetryPipelineService`): each failed
/// or canceled job is replaced by a pending copy in the same pipeline, and the old one is marked
/// retried. The pipeline is `running` again at once, with `finished_at` cleared; its
/// `started_at` stays. Returns whether any job was retried.
pub fn retry_pipeline(c: &Connection, config: &Config, pipeline_id: i64, user: &User) -> Result<bool> {
    let Some(pl) = Pipeline::find(c, pipeline_id)? else { return Ok(false) };
    let ids = job_ids(c, pipeline_id, &["failed", "canceled"])?;
    if ids.is_empty() {
        return Ok(false);
    }
    let now = time::now();
    for id in ids {
        c.execute("UPDATE commit_statuses SET retried = 1, updated_at = ?1 WHERE id = ?2", params![now, id])?;
        c.execute(
            "INSERT INTO commit_statuses (pipeline_id, project_id, name, ref, sha, status, user_id, created_at, updated_at, queued_at, build, stage)
             SELECT pipeline_id, project_id, name, ref, sha, 'pending', ?1, ?2, ?2, ?2, 1, stage FROM commit_statuses WHERE id = ?3",
            params![user.id, now, id],
        )?;
    }
    c.execute("UPDATE ci_pipelines SET finished_at = NULL WHERE id = ?1", [pipeline_id])?;
    ci::recompute(c, config, pipeline_id)?;
    if self::config(c, pl.project_id)?.is_some_and(|f| f.mode == "auto") {
        jobs::enqueue(c, "fake_ci_run", json!({"pipeline_id": pipeline_id}))?;
    }
    Ok(true)
}

/// Auto mode: the runner starts the pipeline's jobs, and finishes them after the duration.
pub fn auto_run(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    let Some(pl) = Pipeline::find(c, pipeline_id)? else { return Ok(()) };
    let Some(cfg) = self::config(c, pl.project_id)? else { return Ok(()) };
    let ids = job_ids(c, pipeline_id, &["pending"])?;
    if ids.is_empty() {
        return Ok(());
    }
    for id in &ids {
        set_job(c, *id, "running")?;
    }
    process_later(c, config, pipeline_id)?;
    jobs::enqueue_after(c, "fake_ci_finish", json!({"pipeline_id": pipeline_id}), cfg.duration_ms)
}

/// Auto mode: the pipeline's running jobs finish with the configured status.
pub fn auto_finish(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    let Some(pl) = Pipeline::find(c, pipeline_id)? else { return Ok(()) };
    let Some(cfg) = self::config(c, pl.project_id)? else { return Ok(()) };
    let r#ref = pl.ref_.clone().unwrap_or_default();
    let status = cfg.overrides.get(&r#ref).and_then(Value::as_str).unwrap_or(&cfg.status).to_string();
    for id in job_ids(c, pipeline_id, &["canceling"])? {
        set_job(c, id, "canceled")?;
    }
    let ids = job_ids(c, pipeline_id, &["running"])?;
    for id in &ids {
        set_job(c, *id, &status)?;
    }
    process_later(c, config, pipeline_id)
}

/// GitLab updates a pipeline's status from its jobs in a worker, a moment after a job
/// changed. Jobs that start and finish within that moment leave the pipeline going from
/// `pending` straight to its result, never `running` (no `started_at`, no duration).
fn process_later(c: &Connection, _config: &Config, pipeline_id: i64) -> Result<()> {
    let pending: i64 = c.query_row(
        "SELECT count(*) FROM jobs WHERE kind = 'fake_ci_process' AND running = 0 AND json_extract(payload, '$.pipeline_id') = ?1",
        [pipeline_id],
        |r| r.get(0),
    )?;
    if pending == 0 {
        jobs::enqueue(c, "fake_ci_process", json!({"pipeline_id": pipeline_id}))?;
    }
    Ok(())
}

/// The jobs of the project for `GET /-/labrat/ci/jobs`.
pub fn jobs_json(c: &Connection, project_id: i64) -> Result<Value> {
    let mut st = c.prepare(
        "SELECT id, pipeline_id, name, ref, sha, status FROM commit_statuses WHERE project_id = ?1 AND build = 1 ORDER BY id",
    )?;
    let rows: Vec<Value> = st
        .query_map([project_id], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "pipeline_id": r.get::<_, i64>(1)?, "name": r.get::<_, String>(2)?, "ref": r.get::<_, Option<String>>(3)?, "sha": r.get::<_, String>(4)?, "status": r.get::<_, String>(5)?}))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Value::Array(rows))
}
