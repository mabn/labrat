//! Background jobs stored in the database, like GitLab's Sidekiq queues. Services enqueue
//! them inside their transaction; the HTTP layer runs them after the request (optionally after
//! a configured delay, see docs/PLAN.md "Background jobs").

use crate::{Config, Result, time};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::RwLock;

/// Delays in milliseconds: one default, per-kind overrides and random jitter.
#[derive(Debug, Clone, Default)]
pub struct Delays {
    pub default_ms: i64,
    pub per_kind_ms: HashMap<String, i64>,
    pub jitter_ms: i64,
}

static DELAYS: RwLock<Option<Delays>> = RwLock::new(None);

pub fn set_delays(d: Delays) {
    *DELAYS.write().unwrap() = Some(d);
}

pub fn delays() -> Delays {
    DELAYS.read().unwrap().clone().unwrap_or_default()
}

fn delay_for(kind: &str) -> i64 {
    let d = delays();
    let base = d.per_kind_ms.get(kind).copied().unwrap_or(d.default_ms);
    let jitter = if d.jitter_ms > 0 {
        (rand::random::<u64>() % (d.jitter_ms as u64 + 1)) as i64
    } else {
        0
    };
    base + jitter
}

pub fn enqueue(c: &Connection, kind: &str, payload: Value) -> Result<()> {
    c.execute(
        "INSERT INTO jobs (kind, payload, run_at) VALUES (?1, ?2, ?3)",
        params![kind, payload.to_string(), time::now() + delay_for(kind)],
    )?;
    Ok(())
}

/// Like `enqueue`, due `after_ms` later than the kind's delay (fake CI durations).
pub fn enqueue_after(c: &Connection, kind: &str, payload: Value, after_ms: i64) -> Result<()> {
    c.execute(
        "INSERT INTO jobs (kind, payload, run_at) VALUES (?1, ?2, ?3)",
        params![kind, payload.to_string(), time::now() + delay_for(kind) + after_ms.max(0)],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: Value,
}

/// Claims the next job that is due, oldest first.
pub fn claim(c: &Connection) -> Result<Option<Job>> {
    let row: Option<(i64, String, String)> = c
        .query_row(
            "SELECT id, kind, payload FROM jobs WHERE running = 0 AND run_at <= ?1 ORDER BY run_at, id LIMIT 1",
            [time::now()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, kind, payload)) = row else {
        return Ok(None);
    };
    c.execute("UPDATE jobs SET running = 1 WHERE id = ?1", [id])?;
    Ok(Some(Job {
        id,
        kind,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
    }))
}

/// Runs the pending jobs of one kind right away, in the caller's transaction. Used where GitLab's
/// Sidekiq job reliably finishes while the request is still busy with later work.
pub fn run_kind_now(c: &Connection, config: &Config, kind: &str) -> Result<()> {
    let jobs: Vec<(i64, String)> = c
        .prepare("SELECT id, payload FROM jobs WHERE running = 0 AND kind = ?1 ORDER BY run_at, id")?
        .query_map([kind], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, payload) in jobs {
        c.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
        let job = Job { id, kind: kind.to_string(), payload: serde_json::from_str(&payload).unwrap_or(Value::Null) };
        run(c, config, &job)?;
    }
    Ok(())
}

pub fn finish(c: &Connection, id: i64) -> Result<()> {
    c.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
    Ok(())
}

/// Milliseconds until the next pending job is due, if any.
pub fn next_due_in(c: &Connection) -> Result<Option<i64>> {
    let at: Option<i64> = c
        .query_row("SELECT min(run_at) FROM jobs WHERE running = 0", [], |r| {
            r.get(0)
        })
        .optional()?
        .flatten();
    Ok(at.map(|t| (t - time::now()).max(0)))
}

/// Jobs that are due now or running.
pub fn due_or_running(c: &Connection) -> Result<i64> {
    Ok(c.query_row("SELECT count(*) FROM jobs WHERE running = 1 OR run_at <= ?1", [time::now()], |r| r.get(0))?)
}

/// Jobs GitLab schedules for later (Sidekiq's scheduled set, which isn't waited for): they
/// count as pending only once due.
const SCHEDULED_KINDS: &str = "'ci_archive_trace'";

/// Jobs still to run: everything queued, except scheduled jobs that aren't due yet.
pub fn pending(c: &Connection) -> Result<i64> {
    Ok(c.query_row(&format!("SELECT count(*) FROM jobs WHERE kind NOT IN ({SCHEDULED_KINDS}) OR running = 1 OR run_at <= ?1"), [time::now()], |r| r.get(0))?)
}

/// Runs one job. Unknown kinds are dropped.
pub fn run(c: &Connection, config: &Config, job: &Job) -> Result<()> {
    let p = &job.payload;
    let id = |k: &str| p[k].as_i64().unwrap_or(0);
    match job.kind.as_str() {
        "mr_after_create" => crate::services::merge_requests::after_create(
            c,
            config,
            id("merge_request_id"),
            id("user_id"),
        ),
        "commit_cross_references" => crate::services::cross_refs::process_commits(c, config, &p),
        "update_merge_requests" => crate::services::merge_requests::refresh::refresh_job(c, config, &p),
        "approvals_sync" => crate::services::approvals::finish_sync(c, id("merge_request_id")),
        "policies_evaluate" => {
            let Some(mr) = crate::services::merge_requests::MergeRequest::find(c, id("merge_request_id"))? else { return Ok(()) };
            let Some(project) = crate::model::Project::find(c, mr.target_project_id)? else { return Ok(()) };
            crate::services::policies::evaluate(c, config, &project, &mr)
        }
        "code_owners_after_push" => {
            let (Some(mr), Some(user)) = (crate::services::merge_requests::MergeRequest::find(c, id("merge_request_id"))?, crate::model::User::find(c, id("user_id"))?) else {
                return Ok(());
            };
            // It runs once the push's approvals sync is done.
            if crate::services::approvals::syncing(c, &mr)? {
                return enqueue_after(c, "code_owners_after_push", p.clone(), 50);
            }
            let Some(project) = crate::model::Project::find(c, mr.target_project_id)? else { return Ok(()) };
            let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
            crate::services::code_owners::after_push(c, config, &project, &mr, &user, &s("old"), &s("new"), &s("merge_status"))?;
            // The approval policies are evaluated again.
            crate::services::policies::sync_rules(c, &project, &mr)?;
            crate::services::policies::evaluate(c, config, &project, &mr.reload(c)?)
        }
        "merge_train_abort" => crate::services::merge_trains::abort_job(c, config, id("car_id"), id("user_id"), p["reason"].as_str().unwrap_or("")),
        "merge_train_refresh" | "merge_train_process" => crate::services::merge_trains::refresh(c, config, id("project_id"), p["target_branch"].as_str().unwrap_or("")),
        "merge_train_auto_add" => match crate::services::merge_requests::MergeRequest::find(c, id("merge_request_id"))? {
            Some(mr) => crate::services::merge_trains::auto_add(c, config, &mr).map(|_| ()),
            None => Ok(()),
        },
        "mr_create_pipeline" => crate::services::fake_ci::create_mr_pipeline(c, config, id("merge_request_id"), id("user_id")),
        "fake_ci_enqueue" => crate::services::fake_ci::enqueue(c, config, id("pipeline_id")),
        "fake_ci_process" if p["retried"] == true => crate::services::pipelines::process_retried(c, config, id("pipeline_id"), p["finished_at"].as_i64()),
        "fake_ci_process" => crate::services::pipelines::process(c, config, id("pipeline_id")),
        "ci_archive_trace" => crate::services::pipelines::archive_trace(c, id("job_id")),
        "fake_ci_run" => crate::services::fake_ci::auto_run(c, config, id("pipeline_id")),
        "fake_ci_finish" => crate::services::fake_ci::auto_finish(c, config, id("pipeline_id")),
        "cherry_pick_note" => {
            let user = crate::model::User::get(c, id("user_id"))?;
            crate::services::notes::system_note(c, id("merge_request_id"), id("project_id"), &user, p["body"].as_str().unwrap_or(""), "cherry_pick")
                .map(|_| ())
        }
        "mr_check_mergeability" | "mr_check_mergeability_closed" => {
            crate::services::merge_requests::mergeability::check(c, config, id("merge_request_id"))
        }
        "mr_rebase" => crate::services::merge_requests::merge::rebase_job(
            c,
            config,
            id("merge_request_id"),
            id("user_id"),
            p["skip_ci"].as_bool().unwrap_or(false),
        ),
        "note_event" => {
            let Some(n) = crate::services::notes::Note::find(c, id("note_id"))? else { return Ok(()) };
            crate::services::events::record(
                c,
                n.project_id,
                id("user_id"),
                "commented",
                Some(crate::services::events::note_type(n.kind.as_deref())),
                Some(n.id),
                None,
            )
        }
        "mr_merge" => {
            // `MergeWorker` behind `merge_async`.
            let Some(mr) = crate::services::merge_requests::MergeRequest::find(c, id("merge_request_id"))? else { return Ok(()) };
            let Some(user) = crate::model::User::find(c, id("user_id"))? else { return Ok(()) };
            let Some(proj) = crate::model::Project::find(c, mr.target_project_id)? else { return Ok(()) };
            let s = |k: &str| p[k].as_str().map(str::to_string);
            let mp = crate::services::merge_requests::merge::MergeParams {
                commit_message: s("commit_message"),
                squash_commit_message: s("squash_commit_message"),
                should_remove_source_branch: p["should_remove_source_branch"].as_bool(),
                sha: s("sha"),
                ..Default::default()
            };
            crate::services::merge_requests::merge::merge(c, config, &proj, &mr, &user, &mp)?;
            Ok(())
        }
        "mr_auto_merge" => crate::services::merge_requests::merge::auto_merge_process(
            c,
            config,
            id("merge_request_id"),
        ),
        "delete_source_branch" => crate::services::merge_requests::merge::delete_source_branch_job(
            c,
            config,
            id("merge_request_id"),
            id("user_id"),
            p["sha"].as_str().unwrap_or(""),
        ),
        "assignee_note" => {
            let ids = |k: &str| -> Vec<i64> {
                p[k].as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                    .unwrap_or_default()
            };
            crate::services::merge_requests::assignee_change_job(
                c,
                id("merge_request_id"),
                id("user_id"),
                &ids("old"),
                &ids("new"),
            )
        }
        "approval_events" => crate::services::reviews::approval_events_job(
            c,
            config,
            id("merge_request_id"),
            id("user_id"),
        ),
        "web_hook" => Ok(()), // delivered by the HTTP layer, which owns the HTTP client
        _ => Ok(()),
    }
}
