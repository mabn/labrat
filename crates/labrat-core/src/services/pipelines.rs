//! CI pipelines and jobs (roadmap phase 7): the jobs (`Ci::Build`) of pipelines that the labrat
//! test endpoints describe (`/-/labrat/projects/:id/pipelines`) or the fake CI creates.
//!
//! labrat runs nothing. A test creates a pipeline run with its stages and jobs, and moves the
//! jobs on (running, a log, artifacts, a result). Everything else follows GitLab:
//!
//! - processing (`Ci::PipelineProcessing::AtomicProcessingService`): a created job is queued
//!   once the stages before it are complete and its `when` allows it (`manual` jobs wait to be
//!   played), and skipped otherwise;
//! - statuses (`Gitlab::Ci::Status::Composite`) of stages and pipelines, with failures allowed
//!   to fail counting as warnings and manual jobs allowed to fail ignored;
//! - job hooks when a job is created, queued, started and finished;
//! - job actions: cancel, retry, play and erase.

use crate::db::OptionalExt;
use crate::model::{Project, User};
use crate::services::ci::{self, Pipeline};
use crate::services::merge_requests::{MergeRequest, next_iid};
use crate::{Config, Error, Result, repo, time};
use rusqlite::{Connection, Row, params};
use serde_json::{Map, Value, json};

/// The id of the runner labrat reports for jobs that ran.
pub const FAKE_RUNNER_ID: i64 = 1;

/// Whether `user` may change the pipeline's jobs (`update_build`, `cancel_build`, `play_job`,
/// `retry_job`) and the pipeline (`update_pipeline`, `cancel_pipeline`): a developer who can
/// also update its ref. For a branch that means pushing or merging to it; for a tag, creating
/// it (Maintainer when the tag isn't protected). Other refs (merge request refs) aren't
/// protected.
pub fn can_update(c: &Connection, user: &User, p: &Project, pl: &Pipeline) -> Result<bool> {
    let perms = crate::perms::ProjectPerms::load(c, Some(user), p)?;
    if !(user.admin || perms.developer()) {
        return Ok(false);
    }
    let Some(r) = pl.ref_.as_deref() else { return Ok(true) };
    if pl.tag {
        return crate::services::protected_tags::can_create(c, user, p, r);
    }
    Ok(repo::can_push_to_branch(c, user, p, r)? || repo::can_merge_to_branch(c, user, p, r)?)
}

/// `erase_build`: maintainers (`admin_build`), or a user who can update the job, started it and
/// whose ref isn't protected.
pub fn can_erase(c: &Connection, user: &User, p: &Project, pl: &Pipeline, j: &Job) -> Result<bool> {
    let perms = crate::perms::ProjectPerms::load(c, Some(user), p)?;
    if user.admin || perms.admin_project() {
        return Ok(true);
    }
    if j.user_id != Some(user.id) || !can_update(c, user, p, pl)? {
        return Ok(false);
    }
    let Some(r) = j.ref_.as_deref() else { return Ok(true) };
    Ok(!if pl.tag { crate::services::protected_tags::is_protected(c, p.id, r)? } else { repo::is_protected(c, p, r)? })
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub pipeline_id: i64,
    pub project_id: i64,
    pub name: String,
    pub ref_: Option<String>,
    pub sha: String,
    pub status: String,
    pub stage: String,
    pub stage_idx: i64,
    pub when: String,
    pub allow_failure: bool,
    pub failure_reason: Option<String>,
    pub exit_code: Option<i64>,
    pub tags: Vec<String>,
    pub coverage: Option<f64>,
    pub user_id: Option<i64>,
    pub retried: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub queued_at: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub erased_at: Option<i64>,
    pub erased_by_id: Option<i64>,
    pub runner_id: Option<i64>,
    pub artifacts_expire_at: Option<i64>,
    pub build: bool,
    pub manual_confirmation: Option<String>,
}

const JOB_COLS: &str = "id, pipeline_id, project_id, name, ref, sha, status, coalesce(stage, 'test'), stage_idx, job_when, allow_failure, failure_reason, exit_code, tag_list, coverage, user_id, retried, created_at, updated_at, queued_at, started_at, finished_at, erased_at, erased_by_id, runner_id, artifacts_expire_at, build, manual_confirmation";

fn job_row(r: &Row) -> rusqlite::Result<Job> {
    let tags: String = r.get(13)?;
    Ok(Job {
        id: r.get(0)?,
        pipeline_id: r.get(1)?,
        project_id: r.get(2)?,
        name: r.get(3)?,
        ref_: r.get(4)?,
        sha: r.get(5)?,
        status: r.get(6)?,
        stage: r.get(7)?,
        stage_idx: r.get(8)?,
        when: r.get(9)?,
        allow_failure: r.get(10)?,
        failure_reason: r.get(11)?,
        exit_code: r.get(12)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        coverage: r.get(14)?,
        user_id: r.get(15)?,
        retried: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
        queued_at: r.get(19)?,
        started_at: r.get(20)?,
        finished_at: r.get(21)?,
        erased_at: r.get(22)?,
        erased_by_id: r.get(23)?,
        runner_id: r.get(24)?,
        artifacts_expire_at: r.get(25)?,
        build: r.get(26)?,
        manual_confirmation: r.get(27)?,
    })
}

impl Job {
    pub fn find(c: &Connection, id: i64) -> Result<Option<Job>> {
        c.query_row(&format!("SELECT {JOB_COLS} FROM commit_statuses WHERE id = ?1"), [id], job_row).optional()
    }

    /// A CI job (not a commit status) of the project.
    pub fn find_in(c: &Connection, project_id: i64, id: i64) -> Result<Option<Job>> {
        Ok(Job::find(c, id)?.filter(|j| j.build && j.project_id == project_id))
    }

    /// The pipeline's CI jobs, oldest first.
    /// The pipeline's CI jobs and commit statuses (`GenericCommitStatus`), oldest first.
    pub fn statuses_for_pipeline(c: &Connection, pipeline_id: i64, include_retried: bool) -> Result<Vec<Job>> {
        let mut st = c.prepare(&format!("SELECT {JOB_COLS} FROM commit_statuses WHERE pipeline_id = ?1 AND (?2 OR retried = 0) ORDER BY id"))?;
        let v = st.query_map(params![pipeline_id, include_retried], job_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn for_pipeline(c: &Connection, pipeline_id: i64, include_retried: bool) -> Result<Vec<Job>> {
        let mut st = c.prepare(&format!("SELECT {JOB_COLS} FROM commit_statuses WHERE pipeline_id = ?1 AND build = 1 AND (?2 OR retried = 0) ORDER BY id"))?;
        let v = st.query_map(params![pipeline_id, include_retried], job_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// The project's CI jobs, oldest first.
    pub fn for_project(c: &Connection, project_id: i64) -> Result<Vec<Job>> {
        let mut st = c.prepare(&format!("SELECT {JOB_COLS} FROM commit_statuses WHERE project_id = ?1 AND build = 1 ORDER BY id"))?;
        let v = st.query_map([project_id], job_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// The job log, if any was sent (and the job wasn't erased).
    pub fn trace(&self, c: &Connection) -> Result<Option<Vec<u8>>> {
        Ok(c.query_row("SELECT trace FROM commit_statuses WHERE id = ?1", [self.id], |r| r.get::<_, Option<Vec<u8>>>(0))?)
    }

    /// `duration`: how long it ran (so far, while running), in seconds.
    pub fn duration(&self) -> Option<f64> {
        let s = self.started_at?;
        Some((self.finished_at.unwrap_or_else(time::now) - s) as f64 / 1000.0)
    }

    /// `queued_duration`: how long it waited for a runner (so far, while pending; until it was
    /// canceled, if it never ran).
    pub fn queued_duration(&self) -> Option<f64> {
        let q = self.queued_at?;
        Some((self.started_at.or(self.finished_at).unwrap_or_else(time::now) - q) as f64 / 1000.0)
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.status.as_str(), "success" | "failed" | "canceled" | "skipped")
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "running" | "preparing" | "waiting_for_resource")
    }

    /// `cancelable?` of a job on its own (never for a commit status; cancelling a pipeline
    /// cancels them too, see `pipeline_cancelable`).
    pub fn is_cancelable(&self) -> bool {
        // `(active? || created?) && !canceling?`
        self.build && matches!(self.status.as_str(), "created" | "pending" | "running" | "preparing" | "waiting_for_resource" | "waiting_for_callback")
    }

    /// `retryable?`: finished (or canceled) jobs, never commit statuses.
    pub fn is_retryable(&self) -> bool {
        self.build && !self.retried && matches!(self.status.as_str(), "success" | "failed" | "canceled" | "canceling")
    }

    /// `manual_confirmation_message`: a playable manual job's confirmation question.
    pub fn manual_confirmation_message(&self) -> Option<&str> {
        (self.is_manual() && self.is_playable()).then_some(self.manual_confirmation.as_deref()).flatten()
    }

    /// `playable?`: a manual job waiting to be played (or one that ran, to be played again).
    pub fn is_playable(&self) -> bool {
        // `playable?`: an action that is manual or could be retried (a skipped one is neither).
        self.when == "manual" && (self.status == "manual" || self.is_retryable())
    }

    pub fn is_manual(&self) -> bool {
        self.when == "manual"
    }

    /// `erasable?`: a finished job that still has a log or artifacts.
    pub fn is_erasable(&self, c: &Connection) -> Result<bool> {
        Ok(matches!(self.status.as_str(), "success" | "failed" | "canceled")
            && (self.trace(c)?.is_some() || !Artifact::for_job(c, self.id)?.is_empty()))
    }

    /// The number of earlier attempts of this job (`retries_count`).
    pub fn retries_count(&self, c: &Connection) -> Result<i64> {
        Ok(c.query_row(
            "SELECT count(*) FROM commit_statuses WHERE pipeline_id = ?1 AND name = ?2 AND build = 1 AND id < ?3",
            params![self.pipeline_id, self.name, self.id],
            |r| r.get(0),
        )?)
    }
}

/// A job artifact: the archive, its metadata, or a report.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub id: i64,
    pub job_id: i64,
    pub file_type: String,
    pub file_format: Option<String>,
    pub filename: String,
    pub size: i64,
    pub expire_at: Option<i64>,
    pub created_at: i64,
}

impl Artifact {
    /// The job's artifacts, in upload order.
    pub fn for_job(c: &Connection, job_id: i64) -> Result<Vec<Artifact>> {
        let mut st = c.prepare("SELECT id, job_id, file_type, file_format, filename, size, expire_at, created_at FROM ci_job_artifacts WHERE job_id = ?1 ORDER BY id")?;
        let v = st
            .query_map([job_id], |r| {
                Ok(Artifact {
                    id: r.get(0)?,
                    job_id: r.get(1)?,
                    file_type: r.get(2)?,
                    file_format: r.get(3)?,
                    filename: r.get(4)?,
                    size: r.get(5)?,
                    expire_at: r.get(6)?,
                    created_at: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn data(c: &Connection, id: i64) -> Result<Vec<u8>> {
        Ok(c.query_row("SELECT data FROM ci_job_artifacts WHERE id = ?1", [id], |r| r.get(0))?)
    }
}

/// The job's artifact of `file_type` and its content.
pub fn artifact_of(c: &Connection, job_id: i64, file_type: &str) -> Result<Option<(Artifact, Vec<u8>)>> {
    match Artifact::for_job(c, job_id)?.into_iter().find(|a| a.file_type == file_type) {
        Some(a) => {
            let d = Artifact::data(c, a.id)?;
            Ok(Some((a, d)))
        }
        None => Ok(None),
    }
}

/// `Gitlab::Ci::Status::Composite` over `(status, allow_failure)` pairs; `None` for no jobs.
pub fn composite(jobs: &[(String, bool)]) -> Option<&'static str> {
    use std::collections::BTreeSet;
    if jobs.is_empty() {
        return None;
    }
    let set: BTreeSet<&str> = jobs
        .iter()
        .map(|(s, af)| match (s.as_str(), af) {
            ("failed" | "canceled", true) => "success_with_warnings",
            ("manual", true) => "ignored",
            (s, _) => s,
        })
        .collect();
    let only = |allowed: &[&str]| set.iter().all(|s| allowed.contains(s));
    let any = |names: &[&str]| names.iter().any(|n| set.contains(n));
    Some(if only(&["skipped", "ignored"]) {
        "skipped"
    } else if only(&["success", "skipped", "success_with_warnings", "ignored"]) {
        "success"
    } else if only(&["created", "success_with_warnings", "ignored"]) {
        "created"
    } else if only(&["preparing", "success_with_warnings", "ignored"]) {
        "preparing"
    } else if only(&["canceled", "success", "skipped", "success_with_warnings", "ignored"]) {
        "canceled"
    } else if only(&["pending", "created", "skipped", "success_with_warnings", "ignored"]) {
        "pending"
    } else if any(&["running", "pending"]) {
        "running"
    } else if any(&["waiting_for_resource"]) {
        "waiting_for_resource"
    } else if any(&["manual"]) {
        "manual"
    } else if any(&["scheduled"]) {
        "scheduled"
    } else if any(&["preparing"]) {
        "preparing"
    } else if any(&["created"]) {
        "running"
    } else if any(&["canceling"]) {
        "canceling"
    } else {
        "failed"
    })
}

/// Whether any job failed but was allowed to (`warnings?`).
pub fn has_warnings(jobs: &[Job]) -> bool {
    jobs.iter().any(|j| j.allow_failure && matches!(j.status.as_str(), "failed" | "canceled"))
}

fn pairs(jobs: &[&Job]) -> Vec<(String, bool)> {
    jobs.iter().map(|j| (j.status.clone(), j.allow_failure)).collect()
}

/// A stage of a pipeline.
#[derive(Debug, Clone)]
pub struct Stage {
    pub id: i64,
    pub pipeline_id: i64,
    pub name: String,
    pub position: i64,
    pub status: String,
}

impl Stage {
    /// The pipeline's stages that have jobs, in order.
    pub fn for_pipeline(c: &Connection, pipeline_id: i64) -> Result<Vec<Stage>> {
        let mut st = c.prepare("SELECT id, pipeline_id, name, position, status FROM ci_stages WHERE pipeline_id = ?1 ORDER BY position, id")?;
        let v = st
            .query_map([pipeline_id], |r| Ok(Stage { id: r.get(0)?, pipeline_id: r.get(1)?, name: r.get(2)?, position: r.get(3)?, status: r.get(4)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }
}

/// Makes sure each stage the pipeline's jobs use has a row, and updates the stage statuses.
pub fn sync_stages(c: &Connection, pipeline_id: i64) -> Result<()> {
    let jobs = Job::statuses_for_pipeline(c, pipeline_id, false)?;
    let project_id: i64 = c.query_row("SELECT project_id FROM ci_pipelines WHERE id = ?1", [pipeline_id], |r| r.get(0))?;
    let mut seen: Vec<(String, i64)> = vec![];
    for j in &jobs {
        if !seen.iter().any(|(n, _)| n == &j.stage) {
            seen.push((j.stage.clone(), j.stage_idx));
        }
    }
    for (name, pos) in seen {
        let in_stage: Vec<&Job> = jobs.iter().filter(|j| j.stage == name).collect();
        let status = composite(&pairs(&in_stage)).unwrap_or("created");
        let id: Option<i64> = c.query_row("SELECT id FROM ci_stages WHERE pipeline_id = ?1 AND name = ?2", params![pipeline_id, name], |r| r.get(0)).optional()?;
        let current: Option<String> = match id {
            Some(id) => Some(c.query_row("SELECT status FROM ci_stages WHERE id = ?1", [id], |r| r.get(0))?),
            None => None,
        };
        let status = match &current {
            Some(cur) => match next_status(cur, status) {
                Some(s) => s,
                None => cur.as_str(),
            },
            None => status,
        };
        match id {
            Some(id) => c.execute("UPDATE ci_stages SET status = ?1, position = ?2 WHERE id = ?3", params![status, pos, id])?,
            None => c.execute(
                "INSERT INTO ci_stages (pipeline_id, project_id, name, position, status) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![pipeline_id, project_id, name, pos, status],
            )?,
        };
    }
    Ok(())
}

/// `set_status` of pipelines and stages: the state a composite status moves them to from
/// `current`, or `None` when it doesn't move them (nothing goes back to `created`; `enqueue`
/// from a finished or canceling state, or while running, is `running`).
pub fn next_status(current: &str, target: &'static str) -> Option<&'static str> {
    match target {
        "created" => None,
        "pending" => match current {
            "created" | "manual" | "waiting_for_resource" | "preparing" | "skipped" | "scheduled" => Some("pending"),
            "success" | "failed" | "canceling" | "canceled" | "running" => Some("running"),
            _ => None,
        },
        "canceling" if current == "canceled" => None,
        t => Some(t),
    }
}

/// Moves a job to `to`, stamping its times as GitLab does, and sends the job hooks GitLab
/// sends for that transition (queued, started from pending, finished).
pub fn transition(c: &Connection, config: &Config, job_id: i64, to: &str) -> Result<()> {
    let Some(j) = Job::find(c, job_id)? else { return Ok(()) };
    if j.status == to {
        return Ok(());
    }
    let now = time::now();
    let from = j.status.clone();
    match to {
        "pending" => c.execute(
            "UPDATE commit_statuses SET status = 'pending', queued_at = ?1, started_at = NULL, finished_at = NULL, updated_at = ?1 WHERE id = ?2",
            params![now, job_id],
        )?,
        "running" => c.execute(
            "UPDATE commit_statuses SET status = 'running', queued_at = coalesce(queued_at, ?1), started_at = ?1, runner_id = ?3, updated_at = ?1 WHERE id = ?2",
            params![now, job_id, FAKE_RUNNER_ID],
        )?,
        "success" | "failed" | "canceled" => c.execute(
            "UPDATE commit_statuses SET status = ?1, finished_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![to, now, job_id],
        )?,
        _ => c.execute("UPDATE commit_statuses SET status = ?1, updated_at = ?2 WHERE id = ?3", params![to, now, job_id])?,
    };
    if to == "failed" && j.failure_reason.is_none() {
        c.execute("UPDATE commit_statuses SET failure_reason = 'script_failure' WHERE id = ?1", [job_id])?;
    }
    // `BuildFinishedWorker`: the log is archived as a `trace` artifact two minutes later.
    if j.build && matches!(to, "success" | "failed" | "canceled") {
        crate::jobs::enqueue_after(c, "ci_archive_trace", json!({"job_id": job_id}), 120_000)?;
    }
    if j.build {
        let hook = match to {
            "pending" | "success" | "failed" | "canceled" => true,
            "running" => from == "pending",
            _ => false,
        };
        if hook {
            job_hook(c, config, job_id)?;
        }
    }
    Ok(())
}

/// Sends the job hook for the job as it is now.
pub fn job_hook(c: &Connection, config: &Config, job_id: i64) -> Result<()> {
    let Some(j) = Job::find(c, job_id)? else { return Ok(()) };
    let Some(p) = Project::find(c, j.project_id)? else { return Ok(()) };
    crate::services::hooks::job_event(c, config, &p, &j)
}

/// `AtomicProcessingService`: queues (or skips) created jobs whose earlier stages are
/// complete, then updates the stages and the pipeline (`ci::recompute`).
pub fn process(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    process_jobs(c, config, pipeline_id)?;
    sync_stages(c, pipeline_id)?;
    ci::recompute(c, config, pipeline_id)
}

/// The job half of `process`: created jobs are queued, made manual or skipped.
fn process_jobs(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    let mut positions: Vec<i64> = Job::for_pipeline(c, pipeline_id, false)?.iter().map(|j| j.stage_idx).collect();
    positions.sort();
    positions.dedup();
    for pos in positions {
        let jobs = Job::for_pipeline(c, pipeline_id, false)?;
        let Some(prev) = prior_status(&jobs, pos) else { continue };
        for j in jobs.iter().filter(|j| j.stage_idx == pos && j.status == "created") {
            let to = if !runs_after(&j.when, prev) {
                "skipped"
            } else if j.when == "manual" {
                "manual"
            } else {
                "pending"
            };
            transition(c, config, j.id, to)?;
        }
    }
    Ok(())
}

/// `Ci::ArchiveTraceWorker`: the job's log becomes its `trace` artifact (`job.log`).
pub fn archive_trace(c: &Connection, job_id: i64) -> Result<()> {
    let Some(j) = Job::find(c, job_id)? else { return Ok(()) };
    let Some(trace) = j.trace(c)? else { return Ok(()) };
    if Artifact::for_job(c, job_id)?.iter().any(|a| a.file_type == "trace") {
        return Ok(());
    }
    c.execute(
        "INSERT INTO ci_job_artifacts (job_id, project_id, file_type, file_format, filename, size, data, expire_at, created_at) VALUES (?1, ?2, 'trace', NULL, 'job.log', ?3, ?4, NULL, ?5)",
        params![job_id, j.project_id, trace.len() as i64, trace, time::now()],
    )?;
    Ok(())
}

/// The status of the stages before `pos`, once they are all complete (`None` before that).
fn prior_status(jobs: &[Job], pos: i64) -> Option<&'static str> {
    let prior: Vec<&Job> = jobs.iter().filter(|j| j.stage_idx < pos).collect();
    let prev = composite(&pairs(&prior)).unwrap_or("success");
    matches!(prev, "success" | "failed" | "canceled" | "skipped").then_some(prev)
}

/// `ProcessBuildService#valid_statuses_for_processable`: whether a job with this `when` runs
/// after earlier stages ending in `prev`, or is skipped.
fn runs_after(when: &str, prev: &str) -> bool {
    let valid: &[&str] = match when {
        "on_success" | "manual" | "delayed" => &["success", "skipped"],
        "on_failure" => &["failed"],
        "always" => &["success", "failed", "skipped"],
        _ => &[],
    };
    valid.contains(&prev)
}

/// Processes the pipeline in a background job, once (`PipelineProcessWorker`).
pub fn process_later(c: &Connection, pipeline_id: i64) -> Result<()> {
    let pending: i64 = c.query_row(
        "SELECT count(*) FROM jobs WHERE kind = 'fake_ci_process' AND running = 0 AND json_extract(payload, '$.pipeline_id') = ?1",
        [pipeline_id],
        |r| r.get(0),
    )?;
    if pending == 0 {
        crate::jobs::enqueue(c, "fake_ci_process", json!({"pipeline_id": pipeline_id}))?;
    }
    Ok(())
}

/// `ResetSkippedJobsService`: after a retry or a play, skipped jobs of later stages are
/// created again, to be processed.
fn reset_skipped_after(c: &Connection, pipeline_id: i64, stage_idx: i64) -> Result<()> {
    let now = time::now();
    c.execute(
        "UPDATE commit_statuses SET status = 'created', updated_at = ?1 WHERE pipeline_id = ?2 AND build = 1 AND retried = 0 AND stage_idx > ?3 AND status = 'skipped'",
        params![now, pipeline_id, stage_idx],
    )?;
    Ok(())
}

/// `Ci::RetryJobService`: a new attempt of the job, created and then processed; the old one
/// is marked retried. Returns the new job.
pub fn retry_job(c: &Connection, config: &Config, job: &Job, user: &User) -> Result<Job> {
    let now = time::now();
    c.execute("UPDATE commit_statuses SET retried = 1, updated_at = ?1 WHERE id = ?2", params![now, job.id])?;
    c.execute(
        "INSERT INTO commit_statuses (pipeline_id, project_id, name, ref, sha, status, user_id, created_at, updated_at, build, stage, stage_idx, job_when, allow_failure, tag_list, artifacts_expire_at, manual_confirmation)
         SELECT pipeline_id, project_id, name, ref, sha, 'created', ?1, ?2, ?2, build, stage, stage_idx, job_when, allow_failure, tag_list, NULL, manual_confirmation FROM commit_statuses WHERE id = ?3",
        params![user.id, now, job.id],
    )?;
    let id = c.last_insert_rowid();
    job_hook(c, config, id)?;
    reset_skipped_after(c, job.pipeline_id, job.stage_idx)?;
    // A retried manual job is played (`enqueue_immediately`): once processed, it is queued
    // rather than waiting again, unless the earlier stages skip it.
    let new = Job::find(c, id)?.expect("just inserted");
    if new.when == "manual" {
        let jobs = Job::for_pipeline(c, job.pipeline_id, false)?;
        if prior_status(&jobs, new.stage_idx).is_some_and(|prev| runs_after(&new.when, prev)) {
            transition(c, config, id, "pending")?;
        }
    }
    process(c, config, job.pipeline_id)?;
    Ok(Job::find(c, id)?.expect("just inserted"))
}

/// `Ci::PlayBuildService`: a manual job is queued, as the user who played it.
pub fn play_job(c: &Connection, config: &Config, job: &Job, user: &User) -> Result<Job> {
    // `Ci::PlayBuildService`: the job is enqueued if it can be (`created`, `skipped`, `manual`,
    // `scheduled`); otherwise it is retried, if it can be, or stays as it is.
    if !matches!(job.status.as_str(), "created" | "skipped" | "manual" | "scheduled") {
        return if job.is_retryable() { retry_job(c, config, job, user) } else { Ok(job.clone()) };
    }
    c.execute("UPDATE commit_statuses SET user_id = ?1 WHERE id = ?2", params![user.id, job.id])?;
    transition(c, config, job.id, "pending")?;
    reset_skipped_after(c, job.pipeline_id, job.stage_idx)?;
    // The pipeline is processed in the background (`PipelineProcessWorker`).
    process_later(c, job.pipeline_id)?;
    Ok(Job::find(c, job.id)?.expect("exists"))
}

/// `build.cancel`: a running job is `canceling` until the runner acknowledges it;
/// any other unfinished job is canceled at once.
pub fn cancel_job(c: &Connection, config: &Config, job: &Job) -> Result<Job> {
    // The `cancel` event: any cancelable status, manual and scheduled jobs included.
    if job.status == "running" {
        transition(c, config, job.id, "canceling")?;
    } else if is_cancelable_status(&job.status) {
        transition(c, config, job.id, "canceled")?;
    }
    // The pipeline is processed in the background.
    process_later(c, job.pipeline_id)?;
    Ok(Job::find(c, job.id)?.expect("exists"))
}

/// `Ci::BuildEraseService`: the log and artifacts go.
pub fn erase_job(c: &Connection, config: &Config, job: &Job, user: &User) -> Result<Job> {
    let now = time::now();
    c.execute("DELETE FROM ci_job_artifacts WHERE job_id = ?1", [job.id])?;
    c.execute(
        "UPDATE commit_statuses SET trace = NULL, artifacts_expire_at = NULL, erased_at = ?1, erased_by_id = ?2, updated_at = ?1 WHERE id = ?3",
        params![now, user.id, job.id],
    )?;
    let _ = config;
    Ok(Job::find(c, job.id)?.expect("exists"))
}

/// Cancels the pipeline's unfinished jobs (`Ci::CancelPipelineService`).
/// `HasStatus::CANCELABLE_STATUSES`: every unfinished status but `canceling`.
pub fn is_cancelable_status(status: &str) -> bool {
    matches!(status, "created" | "pending" | "running" | "preparing" | "waiting_for_callback" | "waiting_for_resource" | "manual" | "scheduled")
}

/// `Ci::Pipeline#cancelable?`: an unfinished pipeline that labrat runs (not one made of
/// commit statuses alone). Cancelling any other does nothing.
pub fn pipeline_cancelable(pl: &Pipeline) -> bool {
    is_cancelable_status(&pl.status) && pl.source != "external"
}

pub fn cancel_pipeline(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    // `cancelable_statuses`: CI jobs and commit statuses. A running CI job waits for its
    // runner (`canceling`); a commit status has none and is canceled at once.
    for j in Job::statuses_for_pipeline(c, pipeline_id, false)? {
        if j.build && j.status == "running" {
            transition(c, config, j.id, "canceling")?;
        } else if is_cancelable_status(&j.status) {
            transition(c, config, j.id, "canceled")?;
        }
    }
    // The pipeline is processed in the background.
    process_later(c, pipeline_id)
}

/// `Ci::RetryPipelineService`: each failed or canceled job gets a new attempt; skipped jobs
/// after them are reset. Returns whether anything was retried.
pub fn retry_pipeline(c: &Connection, config: &Config, pipeline_id: i64, user: &User) -> Result<bool> {
    let jobs: Vec<Job> = Job::for_pipeline(c, pipeline_id, false)?.into_iter().filter(|j| matches!(j.status.as_str(), "failed" | "canceled")).collect();
    if jobs.is_empty() {
        return Ok(false);
    }
    let now = time::now();
    let min_stage = jobs.iter().map(|j| j.stage_idx).min().unwrap_or(0);
    for j in &jobs {
        c.execute("UPDATE commit_statuses SET retried = 1, updated_at = ?1 WHERE id = ?2", params![now, j.id])?;
        c.execute(
            "INSERT INTO commit_statuses (pipeline_id, project_id, name, ref, sha, status, user_id, created_at, updated_at, build, stage, stage_idx, job_when, allow_failure, tag_list, manual_confirmation)
             SELECT pipeline_id, project_id, name, ref, sha, 'created', ?1, ?2, ?2, build, stage, stage_idx, job_when, allow_failure, tag_list, manual_confirmation FROM commit_statuses WHERE id = ?3",
            params![user.id, now, j.id],
        )?;
        job_hook(c, config, c.last_insert_rowid())?;
    }
    let skipped: i64 = c.query_row(
        "SELECT count(*) FROM commit_statuses WHERE pipeline_id = ?1 AND build = 1 AND retried = 0 AND stage_idx > ?2 AND status = 'skipped'",
        params![pipeline_id, min_stage],
        |r| r.get(0),
    )?;
    reset_skipped_after(c, pipeline_id, min_stage)?;
    // Reprocessing skipped jobs starts background processing that takes the pipeline's lease
    // first: the pipeline is then processed after the request (as the reference showed).
    let finished: Option<i64> = c.query_row("SELECT finished_at FROM ci_pipelines WHERE id = ?1", [pipeline_id], |r| r.get(0))?;
    c.execute("UPDATE ci_pipelines SET finished_at = NULL WHERE id = ?1", [pipeline_id])?;
    if skipped > 0 {
        crate::jobs::enqueue(c, "fake_ci_process", json!({"pipeline_id": pipeline_id, "retried": true, "finished_at": finished}))?;
    } else {
        process_retried(c, config, pipeline_id, finished)?;
    }
    Ok(true)
}

/// `process` after a pipeline retry. The queued jobs' hooks show the pipeline as GitLab loaded
/// it before the retry cleared its `finished_at`, so they still have the old one.
pub fn process_retried(c: &Connection, config: &Config, pipeline_id: i64, finished_at: Option<i64>) -> Result<()> {
    c.execute("UPDATE ci_pipelines SET finished_at = ?1 WHERE id = ?2", params![finished_at, pipeline_id])?;
    process_jobs(c, config, pipeline_id)?;
    c.execute("UPDATE ci_pipelines SET finished_at = NULL WHERE id = ?1", [pipeline_id])?;
    sync_stages(c, pipeline_id)?;
    ci::recompute(c, config, pipeline_id)
}

/// Parses an `expire_in` like `1 week`, `3 days 4 hours` or `never` into milliseconds
/// (`None` for never).
pub fn parse_expire_in(s: &str) -> std::result::Result<Option<i64>, String> {
    let s = s.trim().to_lowercase();
    if s == "never" {
        return Ok(None);
    }
    let re = regex::Regex::new(r"(\d+(?:\.\d+)?)\s*([a-z]*)").unwrap();
    let mut total = 0f64;
    let mut any = false;
    for cap in re.captures_iter(&s) {
        any = true;
        let n: f64 = cap[1].parse().map_err(|_| format!("invalid expire_in: {s}"))?;
        let unit = match cap[2].trim_end_matches('s') {
            "" | "sec" | "second" | "s" => 1.0,
            "min" | "minute" | "m" => 60.0,
            "hr" | "hour" | "h" => 3600.0,
            "day" | "d" => 86400.0,
            "wk" | "week" | "w" => 7.0 * 86400.0,
            "mo" | "month" | "mon" => 30.0 * 86400.0,
            "yr" | "year" | "y" => 365.0 * 86400.0,
            other => return Err(format!("invalid expire_in unit: {other}")),
        };
        total += n * unit;
    }
    if !any {
        return Err(format!("invalid expire_in: {s}"));
    }
    Ok(Some((total * 1000.0) as i64))
}

/// The default artifact expiry (`default_artifacts_expire_in`): 30 days.
pub const DEFAULT_EXPIRE_MS: i64 = 30 * 86_400_000;

/// Stores an artifact for a job, replacing one of the same type. An `archive` also gets its
/// `metadata` (a gzipped listing, as GitLab Workhorse makes one).
pub fn add_artifact(c: &Connection, job: &Job, file_type: &str, filename: &str, data: Vec<u8>, expire_ms: Option<i64>) -> Result<()> {
    let now = time::now();
    let expire_at = expire_ms.map(|ms| now + ms);
    let format = match file_type {
        "archive" => "zip",
        "metadata" | "junit" | "cobertura" | "dotenv" | "metrics_referee" => "gzip",
        _ => "raw",
    };
    let types: Vec<&str> = if file_type == "archive" { vec!["archive", "metadata"] } else { vec![file_type] };
    for t in &types {
        c.execute("DELETE FROM ci_job_artifacts WHERE job_id = ?1 AND file_type = ?2", params![job.id, t])?;
    }
    c.execute(
        "INSERT INTO ci_job_artifacts (job_id, project_id, file_type, file_format, filename, size, data, expire_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![job.id, job.project_id, file_type, format, filename, data.len() as i64, data, expire_at, now],
    )?;
    if file_type == "archive" {
        let entries = crate::zip::read(&data).ok_or_else(|| Error::message(400, "artifacts archive is not a zip file"))?;
        let mut listing = String::from("GitLab Build Artifacts Metadata 0.0.2\n");
        for e in &entries {
            listing.push_str(&format!("{}\t{}\n", e.name, e.size));
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, listing.as_bytes()).map_err(|e| Error::message(500, e.to_string()))?;
        let meta = gz.finish().map_err(|e| Error::message(500, e.to_string()))?;
        c.execute(
            "INSERT INTO ci_job_artifacts (job_id, project_id, file_type, file_format, filename, size, data, expire_at, created_at) VALUES (?1, ?2, 'metadata', 'gzip', 'metadata.gz', ?3, ?4, ?5, ?6)",
            params![job.id, job.project_id, meta.len() as i64, meta, expire_at, now],
        )?;
        c.execute("UPDATE commit_statuses SET artifacts_expire_at = ?1 WHERE id = ?2", params![expire_at, job.id])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The test endpoints: describing pipeline runs.

fn spec_err(msg: impl Into<String>) -> Error {
    Error::message(400, msg.into())
}

const JOB_STATUSES: &[&str] = &["created", "pending", "running", "success", "failed", "canceled", "canceling", "skipped", "manual", "scheduled", "preparing", "waiting_for_resource"];

/// The user a spec names (`user`: a username or id), or `default`.
fn spec_user(c: &Connection, v: &Value, default: &User) -> Result<User> {
    match v {
        Value::Null => Ok(default.clone()),
        Value::Number(n) => User::find(c, n.as_i64().unwrap_or(0))?.ok_or_else(|| spec_err("user not found")),
        Value::String(s) => User::find_by_username(c, s)?.ok_or_else(|| spec_err(format!("user {s} not found"))),
        _ => Err(spec_err("user must be a username or an id")),
    }
}

/// `POST /-/labrat/projects/:id/pipelines`: creates a pipeline run as `spec` describes it (see
/// `docs/PLAN.md`, "CI pipelines and jobs").
pub fn create_run(c: &Connection, config: &Config, p: &Project, default_user: &User, spec: &Value) -> Result<Pipeline> {
    let user = spec_user(c, &spec["user"], default_user)?;
    let r = repo::repo(c, p);
    let mr = match spec["merge_request_iid"].as_i64() {
        Some(iid) => Some(MergeRequest::find_by_iid(c, p.id, iid)?.ok_or_else(|| spec_err(format!("merge request !{iid} not found")))?),
        None => None,
    };
    let (ref_, tag, default_sha, source) = match &mr {
        Some(m) => {
            let head = m.diff_head_sha(c, p)?.ok_or_else(|| spec_err("the merge request has no commits"))?;
            (format!("refs/merge-requests/{}/head", m.iid), false, head, "merge_request_event")
        }
        None => {
            let name = spec["ref"].as_str().map(str::to_string).or(p.default_branch.clone()).ok_or_else(|| spec_err("ref is missing"))?;
            if let Some(o) = r.branch(&name)? {
                (name, false, o.hex(), "push")
            } else if let Some((_, o)) = r.tags()?.into_iter().find(|(n, _)| *n == name) {
                (name, true, r.peel(&o)?.hex(), "push")
            } else {
                return Err(spec_err(format!("ref {name} not found")));
            }
        }
    };
    let sha = match spec["sha"].as_str() {
        Some(s) => {
            let o = r.resolve(s)?.filter(|o| r.kind(o).ok().flatten() == Some(labrat_git::Kind::Commit)).ok_or_else(|| spec_err(format!("commit {s} not found")))?;
            o.hex()
        }
        None => default_sha,
    };
    let source = spec["source"].as_str().unwrap_or(source).to_string();
    let jobs = spec["jobs"].as_array().cloned().unwrap_or_default();
    if jobs.is_empty() {
        return Err(spec_err("jobs must list at least one job"));
    }
    // Stage order: as listed, then in order of first use.
    let mut stages: Vec<String> = spec["stages"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default();
    for j in &jobs {
        let s = j["stage"].as_str().unwrap_or("test").to_string();
        if !stages.contains(&s) {
            stages.push(s);
        }
    }
    let now = time::now();
    // A run described as having gone on for a while ends now: it was created that long ago.
    let secs = |j: &Value, k: &str, default: i64| j[k].as_f64().map(|s| (s * 1000.0) as i64).unwrap_or(default);
    let span = {
        let mut t = 0i64;
        for st in &stages {
            let mut end = t;
            for j in jobs.iter().filter(|j| j["stage"].as_str().unwrap_or("test") == st) {
                let started = t + secs(j, "queued_duration", 1000);
                end = end.max(match j["status"].as_str() {
                    Some("success" | "failed") => started + secs(j, "duration", 0),
                    Some("running") => started,
                    _ => t,
                });
            }
            t = end;
        }
        t
    };
    let created_at = match spec["created_at"].as_str() {
        Some(t) => time::parse_time(t).ok_or_else(|| spec_err("created_at is not a time"))?,
        None => now - span,
    };
    let before = spec["before_sha"].as_str().map(str::to_string).unwrap_or_else(|| labrat_git::Oid::ZERO.hex());
    let iid = next_iid(c, "ci_pipelines", p.id)?;
    c.execute(
        "INSERT INTO ci_pipelines (iid, project_id, sha, ref, tag, status, source, user_id, created_at, updated_at, name, before_sha, merge_request_id)
         VALUES (?1, ?2, ?3, ?4, ?5, 'created', ?6, ?7, ?8, ?8, ?9, ?10, ?11)",
        params![iid, p.id, sha, ref_, tag, source, user.id, created_at, spec["name"].as_str(), before, mr.as_ref().map(|m| m.id)],
    )?;
    let pid = c.last_insert_rowid();
    let mut ids = vec![];
    for j in &jobs {
        let name = j["name"].as_str().ok_or_else(|| spec_err("each job needs a name"))?;
        let stage = j["stage"].as_str().unwrap_or("test");
        let pos = stages.iter().position(|s| s == stage).unwrap_or(0) as i64;
        let when = j["when"].as_str().unwrap_or("on_success");
        if !matches!(when, "on_success" | "on_failure" | "always" | "manual") {
            return Err(spec_err(format!("job {name}: when must be on_success, on_failure, always or manual")));
        }
        let allow_failure = j["allow_failure"].as_bool().unwrap_or(when == "manual");
        let tags = j["tags"].as_array().cloned().unwrap_or_default();
        c.execute(
            "INSERT INTO commit_statuses (pipeline_id, project_id, name, ref, sha, status, user_id, created_at, updated_at, build, stage, stage_idx, job_when, allow_failure, tag_list, manual_confirmation)
             VALUES (?1, ?2, ?3, ?4, ?5, 'created', ?6, ?7, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![pid, p.id, name, ref_, sha, user.id, created_at, stage, pos, when, allow_failure, Value::Array(tags).to_string(), j["manual_confirmation"].as_str()],
        )?;
        let id = c.last_insert_rowid();
        job_hook(c, config, id)?;
        ids.push(id);
    }
    sync_stages(c, pid)?;
    // Jobs given a status take it (with their times); the rest are processed as GitLab does.
    let explicit = jobs.iter().any(|j| j["status"].is_string());
    if explicit {
        let mut t = created_at;
        for pos in 0..stages.len() as i64 {
            let mut stage_end = t;
            for (j, id) in jobs.iter().zip(&ids) {
                let job = Job::find(c, *id)?.expect("inserted");
                if job.stage_idx != pos {
                    continue;
                }
                let end = apply_job(c, config, &job, j, Some(t))?;
                stage_end = stage_end.max(end);
            }
            t = stage_end;
        }
    }
    process(c, config, pid)?;
    if explicit {
        fix_times(c, pid)?;
    }
    let pl = Pipeline::find(c, pid)?.expect("inserted");
    for m in ci::all_merge_requests(c, p, &pl)? {
        ci::update_head_pipeline(c, p, &m)?;
    }
    cancel_redundant(c, config, p, &pl)?;
    Ok(Pipeline::find(c, pid)?.expect("inserted"))
}

/// `Ci::PipelineCreation::CancelRedundantPipelinesService` (the project's
/// `auto_cancel_pending_pipelines`, and the default `auto_cancel: on_new_commit: conservative`):
/// a new pipeline cancels the ref's older unfinished pipelines (of the last 7 days, for another
/// commit than the new one and the ref's head) where no job has started, commit statuses
/// included.
pub fn cancel_redundant(c: &Connection, config: &Config, p: &Project, pl: &Pipeline) -> Result<()> {
    if p.setting("auto_cancel_pending_pipelines").and_then(Value::as_str) == Some("disabled") {
        return Ok(());
    }
    let Some(r) = pl.ref_.as_deref() else { return Ok(()) };
    let head = if r.starts_with("refs/") {
        repo::repo(c, p).ref_oid(r)?
    } else {
        repo::repo(c, p).ref_oid(&format!("refs/{}/{r}", if pl.tag { "tags" } else { "heads" }))?
    }
    .map(|o| o.hex());
    for other in Pipeline::list(c, p.id)? {
        let redundant = other.id != pl.id
            && other.ref_.as_deref() == Some(r)
            && is_cancelable_status(&other.status)
            && other.sha != pl.sha
            && Some(&other.sha) != head.as_ref()
            && other.created_at < pl.created_at
            && other.created_at >= pl.created_at - 7 * 24 * 3600 * 1000;
        if !redundant {
            continue;
        }
        // `conservative_interruptible`: no job (none is interruptible) has started.
        let started = Job::for_pipeline(c, other.id, true)?.iter().any(|j| matches!(j.status.as_str(), "running" | "success" | "failed"));
        if !started {
            cancel_pipeline(c, config, other.id)?;
        }
    }
    Ok(())
}

/// A run created as having run: its times follow its jobs' (started with the first, finished
/// with the last, lasting as long as they ran).
fn fix_times(c: &Connection, pipeline_id: i64) -> Result<()> {
    let jobs = Job::for_pipeline(c, pipeline_id, false)?;
    let Some(pl) = Pipeline::find(c, pipeline_id)? else { return Ok(()) };
    let started = jobs.iter().filter_map(|j| j.started_at).min();
    let finished = if pl.is_complete() { jobs.iter().filter_map(|j| j.finished_at).max() } else { None };
    let mut periods: Vec<(i64, i64)> = jobs.iter().filter_map(|j| Some((j.started_at?, j.finished_at.unwrap_or_else(time::now)))).collect();
    periods.sort();
    let (mut total, mut cur): (i64, Option<(i64, i64)>) = (0, None);
    for (s, f) in periods {
        cur = match cur {
            Some((cs, cf)) if s <= cf => Some((cs, cf.max(f))),
            Some((cs, cf)) => {
                total += cf - cs;
                Some((s, f))
            }
            None => Some((s, f)),
        };
    }
    if let Some((cs, cf)) = cur {
        total += cf - cs;
    }
    let duration = finished.map(|_| total / 1000);
    let queued = started.map(|s| (s - pl.created_at) / 1000);
    c.execute(
        "UPDATE ci_pipelines SET started_at = ?1, finished_at = ?2, duration = ?3, queued_duration = ?4 WHERE id = ?5",
        params![started, finished, duration, queued, pipeline_id],
    )?;
    Ok(())
}

/// `PUT /-/labrat/projects/:id/pipelines/:pipeline_id`: moves the run on. Jobs are matched by
/// `id` or by `name` (the latest attempt); unknown names are added to the pipeline.
pub fn update_run(c: &Connection, config: &Config, pl: &Pipeline, spec: &Value) -> Result<Pipeline> {
    if let Some(name) = spec.get("name") {
        c.execute("UPDATE ci_pipelines SET name = ?1 WHERE id = ?2", params![name.as_str(), pl.id])?;
    }
    for j in spec["jobs"].as_array().cloned().unwrap_or_default() {
        let all = Job::for_pipeline(c, pl.id, true)?;
        let found = match (j["id"].as_i64(), j["name"].as_str()) {
            (Some(id), _) => all.into_iter().find(|x| x.id == id),
            (None, Some(n)) => all.into_iter().rev().find(|x| x.name == n),
            _ => return Err(spec_err("each job needs a name or an id")),
        };
        let job = match found {
            Some(job) => job,
            None => {
                let name = j["name"].as_str().ok_or_else(|| spec_err("job not found"))?;
                let stages = Stage::for_pipeline(c, pl.id)?;
                let stage = j["stage"].as_str().unwrap_or("test");
                let pos = match stages.iter().find(|s| s.name == stage) {
                    Some(s) => s.position,
                    None => stages.iter().map(|s| s.position + 1).max().unwrap_or(0),
                };
                let when = j["when"].as_str().unwrap_or("on_success");
                let now = time::now();
                c.execute(
                    "INSERT INTO commit_statuses (pipeline_id, project_id, name, ref, sha, status, user_id, created_at, updated_at, build, stage, stage_idx, job_when, allow_failure)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'created', ?6, ?7, ?7, 1, ?8, ?9, ?10, ?11)",
                    params![pl.id, pl.project_id, name, pl.ref_, pl.sha, pl.user_id, now, stage, pos, when, j["allow_failure"].as_bool().unwrap_or(when == "manual")],
                )?;
                let id = c.last_insert_rowid();
                job_hook(c, config, id)?;
                sync_stages(c, pl.id)?;
                Job::find(c, id)?.expect("inserted")
            }
        };
        apply_job(c, config, &job, &j, None)?;
    }
    process(c, config, pl.id)?;
    Ok(Pipeline::find(c, pl.id)?.expect("exists"))
}

/// Applies one job's spec: attributes, log, artifacts, then its status. With `at` (creating a
/// pipeline that already ran), the job's times are laid out from `at`: queued then, started
/// after `queued_duration` and finished after `duration` (seconds). Returns when it finished
/// (or started).
fn apply_job(c: &Connection, config: &Config, job: &Job, j: &Value, at: Option<i64>) -> Result<i64> {
    let id = job.id;
    if let Some(a) = j["allow_failure"].as_bool() {
        c.execute("UPDATE commit_statuses SET allow_failure = ?1 WHERE id = ?2", params![a, id])?;
    }
    if let Some(m) = j["manual_confirmation"].as_str() {
        c.execute("UPDATE commit_statuses SET manual_confirmation = ?1 WHERE id = ?2", params![m, id])?;
    }
    if let Some(t) = j["tags"].as_array() {
        c.execute("UPDATE commit_statuses SET tag_list = ?1 WHERE id = ?2", params![Value::Array(t.clone()).to_string(), id])?;
    }
    if let Some(cv) = j["coverage"].as_f64() {
        c.execute("UPDATE commit_statuses SET coverage = ?1 WHERE id = ?2", params![cv, id])?;
    }
    // A runner finishing a job being canceled cancels it, whatever it reports.
    let canceling = job.status == "canceling" && matches!(j["status"].as_str(), Some("success" | "failed"));
    if let Some(r) = j["failure_reason"].as_str().filter(|_| !canceling) {
        c.execute("UPDATE commit_statuses SET failure_reason = ?1 WHERE id = ?2", params![r, id])?;
    }
    // Only a failure records the exit code.
    if let Some(e) = j["exit_code"].as_i64().filter(|_| j["status"] == "failed" && !canceling) {
        c.execute("UPDATE commit_statuses SET exit_code = ?1 WHERE id = ?2", params![e, id])?;
    }
    if let Some(log) = j["log"].as_str() {
        c.execute("UPDATE commit_statuses SET trace = ?1 WHERE id = ?2", params![log.as_bytes(), id])?;
    }
    if let Some(more) = j["log_append"].as_str() {
        let mut t = job.trace(c)?.unwrap_or_default();
        t.extend_from_slice(more.as_bytes());
        c.execute("UPDATE commit_statuses SET trace = ?1 WHERE id = ?2", params![t, id])?;
    }
    for a in j["artifacts"].as_array().cloned().unwrap_or_default() {
        let file_type = a["file_type"].as_str().unwrap_or("archive");
        let data = if let Some(files) = a["files"].as_object() {
            let list: Vec<(String, Vec<u8>)> = files.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().as_bytes().to_vec())).collect();
            if file_type == "archive" { crate::zip::write(&list) } else { list.into_iter().next().map(|(_, d)| d).unwrap_or_default() }
        } else if let Some(b) = a["data_base64"].as_str() {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(b).map_err(|_| spec_err("data_base64 is not base64"))?
        } else if let Some(t) = a["content"].as_str() {
            t.as_bytes().to_vec()
        } else {
            return Err(spec_err("an artifact needs files, content or data_base64"));
        };
        let default_name = match file_type {
            "archive" => "artifacts.zip".to_string(),
            "junit" => "junit.xml.gz".to_string(),
            t => format!("{t}.txt"),
        };
        let filename = a["filename"].as_str().map(str::to_string).unwrap_or(default_name);
        let expire = match a["expire_in"].as_str() {
            Some(e) => parse_expire_in(e).map_err(spec_err)?,
            None => Some(DEFAULT_EXPIRE_MS),
        };
        let job = Job::find(c, id)?.expect("exists");
        add_artifact(c, &job, file_type, &filename, data, expire)?;
    }
    let mut end = at.unwrap_or_else(time::now);
    if let Some(status) = j["status"].as_str().map(|s| if canceling { "canceled" } else { s }) {
        if !JOB_STATUSES.contains(&status) {
            return Err(spec_err(format!("job {}: unknown status {status}", job.name)));
        }
        let cur = Job::find(c, id)?.expect("exists");
        if cur.status != status {
            let secs = |k: &str| j[k].as_f64().map(|s| (s * 1000.0) as i64);
            match at {
                // A pipeline described as having run: walk the job through its states, then
                // set its times.
                Some(t0) => {
                    let path: &[&str] = match status {
                        "pending" => &["pending"],
                        "running" => &["pending", "running"],
                        "success" | "failed" => &["pending", "running", status],
                        "canceled" => &["pending", "canceled"],
                        s => &[s][..],
                    };
                    for s in path {
                        transition(c, config, id, s)?;
                    }
                    let queued = t0;
                    let started = queued + secs("queued_duration").unwrap_or(1000);
                    let finished = started + secs("duration").unwrap_or(0);
                    match status {
                        "pending" => {
                            c.execute("UPDATE commit_statuses SET queued_at = ?1 WHERE id = ?2", params![queued, id])?;
                        }
                        "running" => {
                            c.execute("UPDATE commit_statuses SET queued_at = ?1, started_at = ?2 WHERE id = ?3", params![queued, started, id])?;
                            end = started;
                        }
                        "success" | "failed" => {
                            c.execute("UPDATE commit_statuses SET queued_at = ?1, started_at = ?2, finished_at = ?3 WHERE id = ?4", params![queued, started, finished, id])?;
                            end = finished;
                        }
                        _ => {}
                    }
                }
                None => {
                    // Moving straight from created to running or a result passes through the
                    // states in between, as a runner would.
                    if matches!(status, "running" | "success" | "failed") && matches!(cur.status.as_str(), "created" | "manual" | "scheduled") {
                        transition(c, config, id, "pending")?;
                    }
                    let cur = Job::find(c, id)?.expect("exists");
                    if matches!(status, "success" | "failed") && cur.status == "pending" {
                        transition(c, config, id, "running")?;
                    }
                    transition(c, config, id, status)?;
                    // A given duration sets when it started, counted back from now.
                    if let (Some(d), true) = (secs("duration"), matches!(status, "success" | "failed" | "canceled")) {
                        let now = time::now();
                        c.execute("UPDATE commit_statuses SET started_at = ?1 WHERE id = ?2", params![now - d, id])?;
                    }
                }
            }
        }
    }
    Ok(end)
}

/// A summary of a run for the test endpoints' responses.
pub fn run_json(c: &Connection, pl: &Pipeline) -> Result<Value> {
    let jobs: Vec<Value> = Job::for_pipeline(c, pl.id, true)?
        .iter()
        .map(|j| json!({"id": j.id, "name": j.name, "stage": j.stage, "status": j.status, "retried": j.retried}))
        .collect();
    let mut m = Map::new();
    m.insert("id".into(), json!(pl.id));
    m.insert("iid".into(), json!(pl.iid));
    m.insert("ref".into(), json!(pl.ref_));
    m.insert("sha".into(), json!(pl.sha));
    m.insert("status".into(), json!(pl.status));
    m.insert("jobs".into(), Value::Array(jobs));
    Ok(Value::Object(m))
}
