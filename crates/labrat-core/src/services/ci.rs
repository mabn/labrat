//! Commit statuses and the external pipelines they create (`Ci::CreateCommitStatusService`), and
//! how pipelines affect merge requests: `head_pipeline`, the CI merge check and auto-merge.

use crate::db::OptionalExt;
use crate::model::{Project, User};
use crate::services::merge_requests::{MergeRequest, next_iid};
use crate::{Config, Error, Result, jobs, repo, time};
use labrat_git::Oid;
use rusqlite::{Connection, Row, params};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub id: i64,
    pub iid: i64,
    pub project_id: i64,
    pub sha: String,
    pub ref_: Option<String>,
    pub tag: bool,
    pub status: String,
    pub source: String,
    pub user_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration: Option<i64>,
    pub queued_duration: Option<i64>,
    pub name: Option<String>,
    pub before_sha: Option<String>,
    /// Merge request pipelines: the MR, and for merged results and train pipelines the MR
    /// head and the target commit.
    pub merge_request_id: Option<i64>,
    pub source_sha: Option<String>,
    pub target_sha: Option<String>,
}

const PIPELINE_COLS: &str = "id, iid, project_id, sha, ref, tag, status, source, user_id, created_at, updated_at, started_at, finished_at, duration, queued_duration, name, before_sha, merge_request_id, source_sha, target_sha";

fn pipeline_row(r: &Row) -> rusqlite::Result<Pipeline> {
    Ok(Pipeline {
        id: r.get(0)?,
        iid: r.get(1)?,
        project_id: r.get(2)?,
        sha: r.get(3)?,
        ref_: r.get(4)?,
        tag: r.get(5)?,
        status: r.get(6)?,
        source: r.get(7)?,
        user_id: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
        started_at: r.get(11)?,
        finished_at: r.get(12)?,
        duration: r.get(13)?,
        queued_duration: r.get(14)?,
        name: r.get(15)?,
        before_sha: r.get(16)?,
        merge_request_id: r.get(17)?,
        source_sha: r.get(18)?,
        target_sha: r.get(19)?,
    })
}

impl Pipeline {
    pub fn find(c: &Connection, id: i64) -> Result<Option<Pipeline>> {
        c.query_row(&format!("SELECT {PIPELINE_COLS} FROM ci_pipelines WHERE id = ?1"), [id], pipeline_row).optional()
    }

    pub fn list(c: &Connection, project_id: i64) -> Result<Vec<Pipeline>> {
        let mut st = c.prepare(&format!("SELECT {PIPELINE_COLS} FROM ci_pipelines WHERE project_id = ?1 ORDER BY id DESC"))?;
        let v = st.query_map([project_id], pipeline_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "running" | "preparing" | "waiting_for_resource")
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.status.as_str(), "success" | "failed" | "canceled" | "skipped")
    }

    /// Still to run or running (a pipeline being canceled no longer counts).
    pub fn is_unfinished(&self) -> bool {
        matches!(self.status.as_str(), "created" | "pending" | "running" | "preparing" | "waiting_for_resource" | "scheduled")
    }

    /// Merged results and merge train pipelines: built on a merge with the target.
    pub fn is_merged_result(&self) -> bool {
        self.source == "merge_request_event" && self.target_sha.is_some()
    }

    /// `matches_sha_or_source_sha?`
    pub fn matches_sha_or_source_sha(&self, sha: &str) -> bool {
        self.sha == sha || self.source_sha.as_deref() == Some(sha)
    }
}

#[derive(Debug, Clone)]
pub struct Status {
    pub id: i64,
    pub pipeline_id: i64,
    pub name: String,
    pub ref_: Option<String>,
    pub sha: String,
    pub status: String,
    pub target_url: Option<String>,
    pub description: Option<String>,
    pub coverage: Option<f64>,
    pub user_id: Option<i64>,
    pub retried: bool,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

const STATUS_COLS: &str = "id, pipeline_id, name, ref, sha, status, target_url, description, coverage, user_id, retried, created_at, started_at, finished_at";

fn status_row(r: &Row) -> rusqlite::Result<Status> {
    Ok(Status {
        id: r.get(0)?,
        pipeline_id: r.get(1)?,
        name: r.get(2)?,
        ref_: r.get(3)?,
        sha: r.get(4)?,
        status: r.get(5)?,
        target_url: r.get(6)?,
        description: r.get(7)?,
        coverage: r.get(8)?,
        user_id: r.get(9)?,
        retried: r.get(10)?,
        created_at: r.get(11)?,
        started_at: r.get(12)?,
        finished_at: r.get(13)?,
    })
}

impl Status {
    pub fn find(c: &Connection, id: i64) -> Result<Option<Status>> {
        c.query_row(&format!("SELECT {STATUS_COLS} FROM commit_statuses WHERE id = ?1"), [id], status_row).optional()
    }

    /// `GET .../commits/:sha/statuses`: statuses of the pipelines for `sha` (the raw parameter).
    pub fn for_sha(c: &Connection, project_id: i64, sha: &str, all: bool) -> Result<Vec<Status>> {
        let mut st = c.prepare(&format!(
            "SELECT {STATUS_COLS} FROM commit_statuses
             WHERE pipeline_id IN (SELECT id FROM ci_pipelines WHERE project_id = ?1 AND sha = ?2) AND (?3 OR retried = 0) ORDER BY id"
        ))?;
        let v = st.query_map(params![project_id, sha, all], status_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }
}

pub struct StatusInput {
    pub state: String,
    pub ref_: Option<String>,
    pub name: Option<String>,
    pub target_url: Option<String>,
    pub description: Option<String>,
    pub coverage: Option<f64>,
    pub pipeline_id: Option<i64>,
}

/// `POST /projects/:id/statuses/:sha`.
pub fn post_status(c: &Connection, config: &Config, p: &Project, user: &User, sha_param: &str, input: StatusInput) -> Result<Status> {
    let r = repo::repo(c, p);
    let commit = r
        .resolve(sha_param)?
        .filter(|o| r.kind(o).ok().flatten() == Some(labrat_git::Kind::Commit))
        .ok_or(Error::not_found("Commit"))?;
    let sha = commit.hex();
    let name = input.name.clone().unwrap_or("default".into());
    let matching = first_matching_pipeline(c, p, &sha, input.ref_.as_deref(), input.pipeline_id)?;
    if input.pipeline_id.is_some() && matching.is_none() {
        return Err(Error::not_found("Pipeline for pipeline_id, sha and ref"));
    }
    let ref_ = match input.ref_.clone().or(matching.as_ref().and_then(|m| m.ref_.clone())) {
        Some(r) => r,
        None => {
            let mut containing = vec![];
            for (b, head) in r.branches()? {
                if head == commit || labrat_git::walk::is_ancestor(&r, &commit, &head)? {
                    containing.push(b);
                }
            }
            containing.sort();
            containing.into_iter().next().ok_or(Error::not_found("References for commit"))?
        }
    };
    let pipeline = match matching {
        Some(pl) => pl,
        None => create_pipeline(c, config, p, user, &sha, &ref_)?,
    };
    let now = time::now();
    // A live job (pending or running) with the same name, ref and author is updated; anything
    // else starts a new job.
    let live: Option<Status> = c
        .query_row(
            &format!(
                "SELECT {STATUS_COLS} FROM commit_statuses WHERE pipeline_id = ?1 AND name = ?2 AND ref IS ?3 AND user_id = ?4
                 AND status IN ('pending', 'running') AND retried = 0 ORDER BY id DESC LIMIT 1"
            ),
            params![pipeline.id, name, ref_, user.id],
            status_row,
        )
        .optional()?;
    let (job_id, from) = match &live {
        Some(s) => (s.id, s.status.clone()),
        None => {
            c.execute(
                "INSERT INTO commit_statuses (pipeline_id, project_id, name, ref, sha, status, user_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'created', ?6, ?7, ?7)",
                params![pipeline.id, p.id, name, ref_, sha, user.id, now],
            )?;
            (c.last_insert_rowid(), "created".to_string())
        }
    };
    let invalid = |event: &str| {
        Error::message(400, format!("Cannot transition status via :{event} from :{from} (Reason(s): Status cannot transition via \"{event}\")"))
    };
    let (to, event) = match input.state.as_str() {
        "pending" => {
            if from != "created" {
                return Err(invalid("enqueue"));
            }
            ("pending", "enqueue")
        }
        "running" => {
            if from == "running" {
                return Err(invalid("run"));
            }
            ("running", "run")
        }
        "success" => ("success", "succeed"),
        "failed" => ("failed", "drop"),
        "canceled" => ("canceled", "cancel"),
        "skipped" => {
            if from == "running" {
                return Err(invalid("skip"));
            }
            ("skipped", "skip")
        }
        other => return Err(Error::message(400, format!("Cannot transition status via :{other} from :{from}"))),
    };
    let _ = event;
    match to {
        "pending" => {
            c.execute("UPDATE commit_statuses SET status = 'pending', queued_at = ?1, updated_at = ?1 WHERE id = ?2", params![now, job_id])?;
        }
        "running" => {
            c.execute(
                "UPDATE commit_statuses SET status = 'running', queued_at = coalesce(queued_at, ?1), started_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now, job_id],
            )?;
        }
        "skipped" => {
            c.execute("UPDATE commit_statuses SET status = 'skipped', updated_at = ?1 WHERE id = ?2", params![now, job_id])?;
        }
        terminal => {
            c.execute(
                "UPDATE commit_statuses SET status = ?1, finished_at = ?2, updated_at = ?2 WHERE id = ?3",
                params![terminal, now, job_id],
            )?;
        }
    }
    if let Some(u) = input.target_url.filter(|s| !s.is_empty()) {
        c.execute("UPDATE commit_statuses SET target_url = ?1 WHERE id = ?2", params![u, job_id])?;
    }
    if let Some(d) = input.description.filter(|s| !s.is_empty()) {
        c.execute("UPDATE commit_statuses SET description = ?1 WHERE id = ?2", params![d, job_id])?;
    }
    if let Some(cv) = input.coverage {
        c.execute("UPDATE commit_statuses SET coverage = ?1 WHERE id = ?2", params![cv, job_id])?;
    }
    if live.is_none() {
        // `update_older_statuses_retried!`
        c.execute(
            "UPDATE commit_statuses SET retried = 1 WHERE pipeline_id = ?1 AND name = ?2 AND id != ?3 AND retried = 0",
            params![pipeline.id, name, job_id],
        )?;
    }
    recompute(c, config, pipeline.id)?;
    Status::find(c, job_id)?.ok_or(Error::not_found("Commit Status"))
}

fn first_matching_pipeline(c: &Connection, p: &Project, sha: &str, ref_: Option<&str>, id: Option<i64>) -> Result<Option<Pipeline>> {
    c.query_row(
        &format!(
            "SELECT {PIPELINE_COLS} FROM ci_pipelines WHERE project_id = ?1 AND sha = ?2 AND (?3 IS NULL OR ref = ?3) AND (?4 IS NULL OR id = ?4)
             ORDER BY id DESC LIMIT 1"
        ),
        params![p.id, sha, ref_, id],
        pipeline_row,
    )
    .optional()
}

fn create_pipeline(c: &Connection, config: &Config, p: &Project, user: &User, sha: &str, ref_: &str) -> Result<Pipeline> {
    let r = repo::repo(c, p);
    let tag = r.ref_oid(&format!("refs/tags/{ref_}"))?.is_some();
    let iid = next_iid(c, "ci_pipelines", p.id)?;
    let now = time::now();
    c.execute(
        "INSERT INTO ci_pipelines (iid, project_id, sha, ref, tag, status, source, user_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'created', 'external', ?6, ?7, ?7)",
        params![iid, p.id, sha, ref_, tag, user.id, now],
    )?;
    let pl = Pipeline::find(c, c.last_insert_rowid())?.expect("just inserted");
    // `Ci::PipelineCreatedEvent` -> `UpdateHeadPipelineWorker`.
    for mr in all_merge_requests(c, p, &pl)? {
        update_head_pipeline(c, p, &mr)?;
    }
    let _ = config;
    Ok(pl)
}

/// `ref_and_associated_reserved_refs`: a `ref` filter also matches the reserved refs of MRs from
/// that branch, unless the source filter leaves out merge request pipelines.
pub fn refs_for_filter(c: &Connection, p: &Project, ref_: &str, source: Option<&str>) -> Result<Vec<String>> {
    let mut refs = vec![ref_.to_string()];
    if source.is_none_or(|s| s.eq_ignore_ascii_case("merge_request_event")) {
        let mut st = c.prepare("SELECT iid FROM merge_requests WHERE source_branch = ?1 AND (target_project_id = ?2 OR source_project_id = ?2)")?;
        let iids: Vec<i64> = st.query_map(params![ref_, p.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for iid in iids {
            for kind in ["head", "merge", "train"] {
                refs.push(format!("refs/merge-requests/{iid}/{kind}"));
            }
        }
    }
    Ok(refs)
}

thread_local! {
    /// Set while a push with the `ci.skip` push option (or a rebase with `skip_ci`) is processed.
    pub static PUSH_SKIPS_CI: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `Chain::Skip::SKIP_PATTERN`: `[ci skip]` / `[skip ci]` with ` `, `_` or `-`, any case.
pub fn message_skips_ci(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["ci", "skip"].iter().any(|first| {
        let second = if *first == "ci" { "skip" } else { "ci" };
        [' ', '_', '-'].iter().any(|sep| m.contains(&format!("[{first}{sep}{second}]")))
    })
}

/// Whether a pipeline for `sha` would be skipped (`Chain::Skip#skipped?`).
pub fn skips_ci(c: &Connection, p: &Project, sha: &Oid) -> Result<bool> {
    if PUSH_SKIPS_CI.with(|f| f.get()) {
        return Ok(true);
    }
    let r = repo::repo(c, p);
    Ok(r.commit(sha).map(|cm| message_skips_ci(&cm.message)).unwrap_or(false))
}

/// A pipeline the `Skip` chain step saved: `skipped`, no jobs (projects here have no CI
/// configuration, so only skipped pipelines are ever persisted by pushes).
pub fn create_skipped(c: &Connection, p: &Project, user: &User, sha: &str, ref_: &str, source: &str, before_sha: &str) -> Result<Pipeline> {
    let iid = next_iid(c, "ci_pipelines", p.id)?;
    let now = time::now();
    c.execute(
        "INSERT INTO ci_pipelines (iid, project_id, sha, ref, tag, status, source, user_id, created_at, updated_at, before_sha)
         VALUES (?1, ?2, ?3, ?4, 0, 'skipped', ?5, ?6, ?7, ?7, ?8)",
        params![iid, p.id, sha, ref_, source, user.id, now, before_sha],
    )?;
    Ok(Pipeline::find(c, c.last_insert_rowid())?.expect("just inserted"))
}

/// The branch hook: `Git::BranchHooksService#create_pipelines`, which only persists a pipeline
/// when the push skips CI.
pub fn on_push(c: &Connection, _config: &Config, p: &Project, user: &User, u: &labrat_git::protocol::RefUpdate) -> Result<()> {
    let Some(branch) = u.name.strip_prefix("refs/heads/") else { return Ok(()) };
    if u.new.is_zero() || !skips_ci(c, p, &u.new)? {
        return Ok(());
    }
    let pl = create_skipped(c, p, user, &u.new.hex(), branch, "push", &u.old.hex())?;
    crate::services::hooks::pipeline_event(c, _config, p, &pl)?;
    for mr in all_merge_requests(c, p, &pl)? {
        update_head_pipeline(c, p, &mr)?;
    }
    Ok(())
}

/// `MergeRequests::CreatePipelineService` for an MR whose head skips CI: a skipped detached
/// pipeline on `refs/merge-requests/:iid/head`, which becomes the head pipeline.
/// Returns the pipeline; its hook is the caller's (`fire_pipeline_hook`).
pub fn create_skipped_mr_pipeline(c: &Connection, p: &Project, mr: &MergeRequest, user: &User) -> Result<Option<Pipeline>> {
    let Some(d) = mr.latest_diff(c)? else { return Ok(None) };
    let (Some(head), Some(base)) = (d.head_commit_sha.clone(), d.base_commit_sha.clone()) else { return Ok(None) };
    let Ok(oid) = Oid::from_hex(&head) else { return Ok(None) };
    if d.commits_count == 0 || !skips_ci(c, p, &oid)? {
        return Ok(None);
    }
    // `MergeRequests::CreatePipelineService` passes no `before`: the blank SHA.
    let _ = base;
    let pl = create_skipped(c, p, user, &head, &format!("refs/merge-requests/{}/head", mr.iid), "merge_request_event", &Oid::ZERO.hex())?;
    update_head_pipeline(c, p, mr)?;
    Ok(Some(pl))
}

/// `Pipeline#all_merge_requests`: an MR pipeline's own MR; for branch pipelines, open MRs from
/// that branch whose latest diff contains the pipeline's commit.
pub fn all_merge_requests(c: &Connection, p: &Project, pl: &Pipeline) -> Result<Vec<MergeRequest>> {
    if pl.source == "merge_request_event" {
        return Ok(match pl.merge_request_id {
            Some(id) => MergeRequest::find(c, id)?.into_iter().collect(),
            None => vec![],
        });
    }
    let Some(branch) = &pl.ref_ else { return Ok(vec![]) };
    let r = repo::repo(c, p);
    let mut out = vec![];
    for mr in MergeRequest::list_where(c, "source_project_id = ?1 AND source_branch = ?2 AND state = 'opened'", params![p.id, branch])? {
        if let Some(d) = mr.latest_diff(c)?
            && d.commits(&r)?.iter().any(|o| o.hex() == pl.sha) {
                out.push(mr);
            }
    }
    Ok(out)
}

fn mr_row(c: &Connection, mr_id: i64) -> Result<(Option<i64>, Option<i64>, Option<i64>, Option<i64>)> {
    Ok(c
        .query_row(
            "SELECT head_pipeline_id, metrics_pipeline_id, latest_build_started_at, latest_build_finished_at FROM merge_request_pipelines WHERE merge_request_id = ?1",
            [mr_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .unwrap_or((None, None, None, None)))
}

/// The raw `head_pipeline` association.
pub fn head_pipeline(c: &Connection, mr: &MergeRequest) -> Result<Option<Pipeline>> {
    match mr_row(c, mr.id)?.0 {
        Some(id) => Pipeline::find(c, id),
        None => Ok(None),
    }
}

/// `merge_request_metrics.pipeline` and the build timestamps.
pub fn metrics(c: &Connection, mr: &MergeRequest) -> Result<(Option<Pipeline>, Option<i64>, Option<i64>)> {
    let (_, m, s, f) = mr_row(c, mr.id)?;
    Ok((m.map(|id| Pipeline::find(c, id)).transpose()?.flatten(), s, f))
}

/// `diff_head_pipeline`: the head pipeline only while it is for the MR's current head.
pub fn diff_head_pipeline(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Option<Pipeline>> {
    let head = mr.diff_head_sha(c, p)?;
    Ok(head_pipeline(c, mr)?.filter(|pl| head.as_deref().is_some_and(|h| pl.matches_sha_or_source_sha(h))))
}

/// Pipelines listed for the MR (`Ci::PipelinesForMergeRequestFinder`): its detached pipelines,
/// newest first, then branch pipelines of the source branch at any of its diff heads.
pub fn mr_pipelines(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Vec<Pipeline>> {
    let heads: Vec<String> = crate::services::merge_requests::diffs::Diff::for_mr(c, mr.id)?
        .into_iter()
        .filter_map(|d| d.head_commit_sha)
        .collect();
    let all = Pipeline::list(c, p.id)?;
    let mr_ref = format!("refs/merge-requests/{}/head", mr.iid);
    let mut out: Vec<Pipeline> = all
        .iter()
        .filter(|pl| pl.source == "merge_request_event" && (pl.merge_request_id == Some(mr.id) || pl.ref_.as_deref() == Some(mr_ref.as_str())))
        .cloned()
        .collect();
    out.extend(
        all.into_iter()
            .filter(|pl| pl.source != "merge_request_event" && pl.ref_.as_deref() == Some(mr.source_branch.as_str()) && !pl.tag && heads.contains(&pl.sha)),
    );
    Ok(out)
}

/// `MergeRequest#update_head_pipeline`.
pub fn update_head_pipeline(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<()> {
    let head = mr.diff_head_sha(c, p)?;
    let found = mr_pipelines(c, p, mr)?.into_iter().find(|pl| head.as_deref().is_some_and(|h| pl.matches_sha_or_source_sha(h)));
    if let Some(pl) = found {
        c.execute(
            "INSERT INTO merge_request_pipelines (merge_request_id, head_pipeline_id) VALUES (?1, ?2)
             ON CONFLICT (merge_request_id) DO UPDATE SET head_pipeline_id = excluded.head_pipeline_id",
            params![mr.id, pl.id],
        )?;
    }
    Ok(())
}

/// `Gitlab::Ci::Status::Composite` over the latest jobs.
fn composite(statuses: &[String]) -> &'static str {
    let set: std::collections::BTreeSet<&str> = statuses.iter().map(String::as_str).collect();
    let subset = |allowed: &[&str]| set.iter().all(|s| allowed.contains(s));
    if set.is_empty() {
        return "success";
    }
    // Jobs a runner still has to acknowledge as canceled.
    if set.contains("canceling") {
        return "canceling";
    }
    if subset(&["skipped"]) {
        "skipped"
    } else if subset(&["success", "skipped"]) {
        "success"
    } else if subset(&["created"]) {
        "created"
    } else if subset(&["canceled", "success", "skipped"]) {
        "canceled"
    } else if subset(&["pending", "created", "skipped"]) {
        "pending"
    } else if set.contains("running") || set.contains("pending") {
        "running"
    } else {
        "failed"
    }
}

/// `AtomicProcessingService`: recompute the pipeline status and apply transition side effects.
pub fn recompute(c: &Connection, config: &Config, pipeline_id: i64) -> Result<()> {
    let Some(pl) = Pipeline::find(c, pipeline_id)? else { return Ok(()) };
    let statuses: Vec<String> = {
        let mut st = c.prepare("SELECT status FROM commit_statuses WHERE pipeline_id = ?1 AND retried = 0")?;
        st.query_map([pipeline_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let mut target = composite(&statuses).to_string();
    // `enqueue` from a finished pipeline goes to `running`.
    if target == "pending" && pl.is_complete() {
        target = "running".into();
    }
    if target == pl.status {
        return Ok(());
    }
    let now = time::now();
    let started = if target == "running" && pl.started_at.is_none() { Some(now) } else { pl.started_at };
    let finished = if matches!(target.as_str(), "success" | "failed" | "canceled") { Some(now) } else { pl.finished_at };
    let duration = if matches!(target.as_str(), "success" | "failed" | "canceled") {
        // Fake CI pipelines last as long as their latest jobs ran (a retried pipeline counts
        // only the new jobs); external ones report 0.
        let mut st = c.prepare(
            "SELECT started_at, coalesce(finished_at, ?2) FROM commit_statuses WHERE pipeline_id = ?1 AND build = 1 AND retried = 0 AND started_at IS NOT NULL ORDER BY started_at",
        )?;
        let periods: Vec<(i64, i64)> = st.query_map(params![pl.id, now], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let builds: i64 = c.query_row("SELECT count(*) FROM commit_statuses WHERE pipeline_id = ?1 AND build = 1", [pl.id], |r| r.get(0))?;
        if builds > 0 && started.is_some() {
            // The union of the jobs' run periods (`Gitlab::Ci::Pipeline::Duration`).
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
            Some(total / 1000)
        } else {
            started.map(|_| 0)
        }
    } else {
        pl.duration
    };
    let queued = match started.or(finished) {
        Some(t) => {
            let s = (t - pl.created_at) / 1000;
            if s > 0 { Some(s) } else { None }
        }
        None => None,
    };
    c.execute(
        "UPDATE ci_pipelines SET status = ?1, started_at = ?2, finished_at = ?3, duration = ?4, queued_duration = ?5, updated_at = ?6 WHERE id = ?7",
        params![target, started, finished, duration, queued, now, pl.id],
    )?;
    let p = Project::find(c, pl.project_id)?.expect("project exists");
    let pl = Pipeline::find(c, pl.id)?.expect("exists");
    // `PipelineMetricsWorker` on `-> running` and `-> success`.
    if target == "running" || target == "success" {
        let mut st = c.prepare("SELECT merge_request_id FROM merge_request_pipelines WHERE head_pipeline_id = ?1")?;
        let ids: Vec<i64> = st.query_map([pl.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for id in ids {
            c.execute(
                "UPDATE merge_request_pipelines SET metrics_pipeline_id = ?1, latest_build_started_at = ?2, latest_build_finished_at = ?3 WHERE merge_request_id = ?4",
                params![pl.id, pl.started_at, if target == "success" { pl.finished_at } else { None }, id],
            )?;
        }
    }
    if target == "running"
        && let Some(mr_id) = pl.merge_request_id
    {
        crate::services::approvals::pipeline_running(c, mr_id)?;
    }
    crate::services::hooks::pipeline_event(c, config, &p, &pl)?;
    // EE merge trains: a car's pipeline moves its train on; a passing MR pipeline adds an MR
    // waiting to join.
    if pl.is_complete() && config.is_ee() {
        crate::services::merge_trains::on_pipeline_complete(c, config, &pl)?;
    }
    // Completed pipelines let armed auto-merges proceed.
    if pl.is_complete() {
        for mr in all_merge_requests(c, &p, &pl)? {
            if mr.merge_when_pipeline_succeeds {
                jobs::enqueue(c, "mr_auto_merge", json!({"merge_request_id": mr.id}))?;
            }
        }
    }
    Ok(())
}

/// `has_ci_enabled?` for an MR (a head pipeline or any pipeline for it).
pub fn has_ci(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<bool> {
    Ok(head_pipeline(c, mr)?.is_some() || !mr_pipelines(c, p, mr)?.is_empty())
}

/// `CheckCiStatusService` as seen by `detailed_merge_status`: `None` when it passes or is
/// inactive, else `ci_still_running` or `ci_must_pass`.
pub fn ci_check(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Option<&'static str>> {
    let setting = p.only_allow_merge_if_pipeline_succeeds;
    // EE merge train strategies run their own pipelines: they don't make CI a merge check.
    let auto_ci = mr.merge_when_pipeline_succeeds
        && !matches!(mr.merge_params.get("auto_merge_strategy").and_then(|v| v.as_str()), Some("merge_train" | "add_to_merge_train_when_checks_pass"));
    if !setting && !auto_ci {
        return Ok(None);
    }
    let must = setting || has_ci(c, p, mr)?;
    if !must {
        return Ok(None);
    }
    let dhp = diff_head_pipeline(c, p, mr)?;
    let ok = match &dhp {
        Some(pl) => pl.status == "success" || (pl.status == "skipped" && p.allow_merge_on_skipped_pipeline == Some(true)),
        None => false,
    };
    if ok {
        return Ok(None);
    }
    Ok(Some(if in_progress(p, dhp.as_ref()) { "ci_still_running" } else { "ci_must_pass" }))
}

/// `diff_head_pipeline_considered_in_progress?`.
pub fn in_progress(p: &Project, dhp: Option<&Pipeline>) -> bool {
    match dhp {
        Some(pl) if p.only_allow_merge_if_pipeline_succeeds => !pl.is_complete(),
        Some(pl) => pl.is_active() || pl.status == "created",
        None => false,
    }
}

pub fn head_pipeline_success(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<bool> {
    Ok(diff_head_pipeline(c, p, mr)?.is_some_and(|pl| pl.status == "success"))
}

#[allow(dead_code)]
fn _unused(_: Oid) {}
