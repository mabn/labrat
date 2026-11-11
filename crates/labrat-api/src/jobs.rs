//! Runs the database-backed job queue of an app (labrat's Sidekiq). Each app gets one worker
//! task while jobs are pending; webhook jobs are delivered over HTTP here.

use labrat_core::{App, jobs};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

/// Apps with a running worker, by address. The `Weak` tells a live entry from a stale one: a
/// worker whose runtime was dropped (a finished test) never removes its key, and a new app can
/// reuse the address.
static ACTIVE: LazyLock<Mutex<HashMap<usize, Weak<App>>>> = LazyLock::new(Default::default);

fn key(app: &Arc<App>) -> usize {
    Arc::as_ptr(app) as usize
}

/// Starts the worker for `app` if it isn't running.
pub fn kick(app: &Arc<App>) {
    {
        let mut active = ACTIVE.lock();
        let running = active.get(&key(app)).and_then(Weak::upgrade).is_some_and(|a| Arc::ptr_eq(&a, app));
        if running {
            return;
        }
        active.insert(key(app), Arc::downgrade(app));
    }
    let app = app.clone();
    tokio::spawn(async move {
        worker(app.clone()).await;
        ACTIVE.lock().remove(&key(&app));
        // A job may have been queued between the last check and the removal.
        let pending = app.db.read(jobs::pending).unwrap_or(0);
        if pending > 0 {
            kick(&app);
        }
    });
}

/// Runs a claimed job and removes it; failures are recorded in `job_failures`.
async fn run_claimed(app: &Arc<App>, job: jobs::Job) {
    if job.kind == "web_hook" {
        crate::hooks::deliver(app, &job.payload).await;
    } else {
        let app2 = app.clone();
        let job2 = job.clone();
        let res = tokio::task::spawn_blocking(move || app2.db.tx(|c| jobs::run(c, &app2.config, &job2))).await;
        let failure = match res {
            Ok(Err(e)) => Some(e.to_string()),
            Err(e) => Some(format!("panicked: {e}")),
            Ok(Ok(())) => None,
        };
        if let Some(e) = failure {
            tracing::error!("job {} failed: {e}", job.kind);
            let _ = app.db.tx(|c| {
                c.execute("INSERT INTO job_failures (kind, error) VALUES (?1, ?2)", rusqlite::params![job.kind, e])?;
                Ok::<_, labrat_core::Error>(())
            });
        }
    }
    let _ = app.db.tx(|c| jobs::finish(c, job.id));
}

async fn worker(app: Arc<App>) {
    loop {
        let job = match app.db.tx(jobs::claim) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("job claim failed: {e}");
                return;
            }
        };
        match job {
            Some(job) => run_claimed(&app, job).await,
            None => match app.db.read(jobs::next_due_in) {
                Ok(Some(ms)) => tokio::time::sleep(Duration::from_millis(ms.max(1) as u64)).await,
                _ => return,
            },
        }
    }
}

/// Runs every job that is due now, here rather than waiting for the worker (which may be
/// sleeping on a real-time delay computed before the clock moved), then waits for jobs the
/// worker is already running. Jobs scheduled later are left alone.
pub async fn wait_due(app: &Arc<App>) {
    loop {
        match app.db.tx(jobs::claim) {
            Ok(Some(job)) => run_claimed(app, job).await,
            _ => {
                if app.db.read(jobs::due_or_running).unwrap_or(0) == 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    }
}

/// Resolves when no job is pending or running.
pub async fn wait_idle(app: &Arc<App>) {
    loop {
        let pending = app.db.read(jobs::pending).unwrap_or(0);
        if pending == 0 {
            return;
        }
        kick(app);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// `MergeRequests::ProcessScheduledMergeWorker`: GitLab runs it every minute; labrat checks every
/// second (so a shifted clock takes effect at once) and retries a still-blocked MR once a minute.
pub async fn scheduled_merges(app: std::sync::Weak<App>) {
    let mut last: std::collections::HashMap<i64, i64> = Default::default();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(app) = app.upgrade() else { return };
        let now = app.db.now();
        let due = app.db.read(|c| labrat_core::services::merge_requests::due_scheduled_merges(c, now)).unwrap_or_default();
        last.retain(|id, _| due.contains(id));
        let fresh: Vec<i64> = due.into_iter().filter(|id| last.get(id).is_none_or(|t| now - t >= 60_000)).collect();
        if fresh.is_empty() {
            continue;
        }
        let res = app.db.tx(|c| {
            for id in &fresh {
                jobs::enqueue(c, "mr_auto_merge", serde_json::json!({"merge_request_id": id}))?;
            }
            Ok::<_, labrat_core::Error>(())
        });
        if res.is_ok() {
            for id in fresh {
                last.insert(id, now);
            }
            kick(&app);
        }
    }
}
