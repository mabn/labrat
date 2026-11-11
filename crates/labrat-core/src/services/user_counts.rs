//! The merge request counts of `/user_counts` (`User#assigned_open_merge_requests_count`,
//! `#review_requested_open_merge_requests_count`). GitLab caches them for
//! `COUNT_CACHE_VALIDITY_PERIOD` (24 hours) and only services that touch a user's MRs
//! invalidate them, so a count can stay stale (e.g. the author's when reviewers are removed).

use crate::Result;
use crate::services::merge_requests::MergeRequest;
use rusqlite::{Connection, params};

const VALIDITY_MS: i64 = 24 * 3600 * 1000;

const OPEN: &str = "SELECT COUNT(*) FROM merge_requests m JOIN projects p ON p.id = m.target_project_id WHERE m.state = 'opened' AND p.archived = 0";

fn cached(c: &Connection, user_id: i64, key: &str, compute: impl FnOnce() -> Result<i64>) -> Result<i64> {
    let now = crate::time::now();
    let hit: Option<i64> = c
        .query_row("SELECT value FROM user_count_cache WHERE user_id = ?1 AND key = ?2 AND expires_at > ?3", params![user_id, key, now], |r| r.get(0))
        .ok();
    if let Some(v) = hit {
        return Ok(v);
    }
    let v = compute()?;
    c.execute(
        "INSERT OR REPLACE INTO user_count_cache (user_id, key, value, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![user_id, key, v, now + VALIDITY_MS],
    )?;
    Ok(v)
}

/// Open MRs the user authored or is assigned to that are "with them": no reviewers, or a
/// reviewer has reviewed or requested changes.
pub fn assigned_open(c: &Connection, user_id: i64) -> Result<i64> {
    cached(c, user_id, "assigned_open_merge_requests_count", || {
        Ok(c.query_row(
            &format!(
                "{OPEN} AND (m.author_id = ?1 OR EXISTS (SELECT 1 FROM merge_request_assignees a WHERE a.merge_request_id = m.id AND a.user_id = ?1))
                 AND (NOT EXISTS (SELECT 1 FROM merge_request_reviewers r WHERE r.merge_request_id = m.id)
                      OR EXISTS (SELECT 1 FROM merge_request_reviewers r WHERE r.merge_request_id = m.id AND r.state IN (1, 2)))"
            ),
            [user_id],
            |r| r.get(0),
        )?)
    })
}

/// Open MRs where the user is a reviewer in state unreviewed, unapproved or review_started.
pub fn review_requested_open(c: &Connection, user_id: i64) -> Result<i64> {
    cached(c, user_id, "review_requested_open_merge_requests_count", || {
        Ok(c.query_row(
            &format!("{OPEN} AND EXISTS (SELECT 1 FROM merge_request_reviewers r WHERE r.merge_request_id = m.id AND r.user_id = ?1 AND r.state IN (0, 4, 5))"),
            [user_id],
            |r| r.get(0),
        )?)
    })
}

/// `User#todos_pending_count`: `TodosFinder` pending to-dos, without snoozed ones.
pub fn todos_pending(c: &Connection, user_id: i64) -> Result<i64> {
    let now = crate::time::now();
    cached(c, user_id, "todos_pending_count", || {
        Ok(c.query_row(
            "SELECT COUNT(*) FROM todos WHERE user_id = ?1 AND state = 'pending' AND (snoozed_until IS NULL OR snoozed_until <= ?2)",
            params![user_id, now],
            |r| r.get(0),
        )?)
    })
}

/// `User#invalidate_merge_request_cache_counts` for each user.
pub fn invalidate(c: &Connection, user_ids: &[i64]) -> Result<()> {
    for id in user_ids {
        c.execute("DELETE FROM user_count_cache WHERE user_id = ?1 AND key != 'todos_pending_count'", [id])?;
    }
    Ok(())
}

/// `User#invalidate_cache_counts`: every cached count.
pub fn invalidate_everything(c: &Connection, user_id: i64) -> Result<()> {
    c.execute("DELETE FROM user_count_cache WHERE user_id = ?1", [user_id])?;
    Ok(())
}

/// `invalidate_all_users_cache_count`: assignees, reviewers and the author.
pub fn invalidate_all(c: &Connection, mr: &MergeRequest) -> Result<()> {
    let mut ids = mr.assignee_ids(c)?;
    ids.extend(mr.reviewer_ids(c)?);
    ids.push(mr.author_id);
    invalidate(c, &ids)
}

/// Invalidation for the MR's assignees plus `extra` users.
pub fn invalidate_assignees_and(c: &Connection, mr: &MergeRequest, extra: &[i64]) -> Result<()> {
    let mut ids = mr.assignee_ids(c)?;
    ids.extend_from_slice(extra);
    invalidate(c, &ids)
}
