//! To-do items (`TodoService`) for merge requests.

use crate::model::{Project, User};
use crate::services::merge_requests::MergeRequest;
use crate::{Result, time};
use rusqlite::{Connection, params};

pub mod action {
    pub const ASSIGNED: i64 = 1;
    pub const MENTIONED: i64 = 2;
    pub const BUILD_FAILED: i64 = 3;
    pub const MARKED: i64 = 4;
    pub const APPROVAL_REQUIRED: i64 = 5;
    pub const UNMERGEABLE: i64 = 6;
    pub const DIRECTLY_ADDRESSED: i64 = 7;
    pub const REVIEW_REQUESTED: i64 = 9;
    pub const MEMBER_ACCESS_REQUESTED: i64 = 10;
    pub const REVIEW_SUBMITTED: i64 = 11;

    pub fn name(a: i64) -> &'static str {
        match a {
            ASSIGNED => "assigned",
            MENTIONED => "mentioned",
            BUILD_FAILED => "build_failed",
            MARKED => "marked",
            APPROVAL_REQUIRED => "approval_required",
            UNMERGEABLE => "unmergeable",
            DIRECTLY_ADDRESSED => "directly_addressed",
            REVIEW_REQUESTED => "review_requested",
            MEMBER_ACCESS_REQUESTED => "member_access_requested",
            REVIEW_SUBMITTED => "review_submitted",
            _ => "unknown",
        }
    }
}

/// `TodoService#todo_exist?`: the user has any pending to-do on the MR.
pub fn any_pending(c: &Connection, user_id: i64, mr: &MergeRequest) -> Result<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM todos WHERE user_id = ?1 AND target_type = 'MergeRequest' AND target_id = ?2 AND state = 'pending'",
        params![user_id, mr.id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn pending_exists(c: &Connection, user_id: i64, mr: &MergeRequest, action: i64) -> Result<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM todos WHERE user_id = ?1 AND target_type = 'MergeRequest' AND target_id = ?2 AND action = ?3 AND state = 'pending'",
        params![user_id, mr.id, action],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn create(
    c: &Connection,
    user_id: i64,
    author: &User,
    p: &Project,
    mr: &MergeRequest,
    action: i64,
    note_id: Option<i64>,
) -> Result<i64> {
    // `excluded_user_ids`: only mentions may pile up; other actions keep one pending to-do.
    if !matches!(action, action::MENTIONED | action::DIRECTLY_ADDRESSED | action::MEMBER_ACCESS_REQUESTED) {
        let existing: Option<i64> = c
            .query_row(
                "SELECT id FROM todos WHERE user_id = ?1 AND target_type = 'MergeRequest' AND target_id = ?2 AND action = ?3 AND state = 'pending' ORDER BY id DESC LIMIT 1",
                params![user_id, mr.id, action],
                |r| r.get(0),
            )
            .ok();
        if let Some(id) = existing {
            return Ok(id);
        }
    }
    let now = time::now();
    c.execute(
        "INSERT INTO todos (user_id, author_id, project_id, target_type, target_id, action, note_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'MergeRequest', ?4, ?5, ?6, ?7, ?7)",
        params![user_id, author.id, p.id, mr.id, action, note_id, now],
    )?;
    Ok(c.last_insert_rowid())
}

/// To-dos for a new MR: assignees and reviewers.
pub fn new_merge_request(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    author: &User,
) -> Result<()> {
    for uid in mr.assignee_ids(c)? {
        if !pending_exists(c, uid, mr, action::ASSIGNED)? {
            create(c, uid, author, p, mr, action::ASSIGNED, None)?;
        }
    }
    for uid in mr.reviewer_ids(c)? {
        if !pending_exists(c, uid, mr, action::REVIEW_REQUESTED)? {
            create(c, uid, author, p, mr, action::REVIEW_REQUESTED, None)?;
        }
    }
    // `create_mention_todos` for the title and description.
    crate::services::mentions::text_todos(c, p, mr, author, &mentionable_text(mr), None, &[])?;
    Ok(())
}

/// The MR text that can mention users.
pub fn mentionable_text(mr: &MergeRequest) -> String {
    format!("{}\n{}", mr.title, mr.description.as_deref().unwrap_or(""))
}

pub fn assigned(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    author: &User,
    old: &[i64],
    new: &[i64],
) -> Result<()> {
    for uid in new.iter().filter(|u| !old.contains(u)) {
        if !pending_exists(c, *uid, mr, action::ASSIGNED)? {
            create(c, *uid, author, p, mr, action::ASSIGNED, None)?;
        }
    }
    Ok(())
}

pub fn review_requested(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    author: &User,
    old: &[i64],
    new: &[i64],
) -> Result<()> {
    for uid in new.iter().filter(|u| !old.contains(u)) {
        if !pending_exists(c, *uid, mr, action::REVIEW_REQUESTED)? {
            create(c, *uid, author, p, mr, action::REVIEW_REQUESTED, None)?;
        }
    }
    Ok(())
}

/// Marks pending to-dos on the MR done (for one user, or everyone).
pub fn resolve_for_target(c: &Connection, mr: &MergeRequest, user_id: Option<i64>) -> Result<()> {
    match user_id {
        Some(u) => c.execute(
            "UPDATE todos SET state = 'done', snoozed_until = NULL, updated_at = ?1 WHERE target_type = 'MergeRequest' AND target_id = ?2 AND user_id = ?3 AND state = 'pending'",
            params![time::now(), mr.id, u],
        )?,
        None => c.execute(
            "UPDATE todos SET state = 'done', snoozed_until = NULL, updated_at = ?1 WHERE target_type = 'MergeRequest' AND target_id = ?2 AND state = 'pending'",
            params![time::now(), mr.id],
        )?,
    };
    Ok(())
}

/// `TodoService#mark_todo`: a "marked" to-do for `user`, unless one is already pending.
pub fn mark(c: &Connection, p: &Project, mr: &MergeRequest, user: &User) -> Result<Option<i64>> {
    if pending_exists(c, user.id, mr, action::MARKED)? {
        return Ok(None);
    }
    Ok(Some(create(c, user.id, user, p, mr, action::MARKED, None)?))
}

/// Sets the state of `user`'s to-dos among `ids`; returns the ones that changed, oldest first.
pub fn set_state(c: &Connection, user_id: i64, ids: &[i64], state: &str) -> Result<Vec<i64>> {
    let mut changed = vec![];
    for id in ids {
        let n = c.execute(
            // `resolve_todos` clears the snooze; a restore keeps it.
            "UPDATE todos SET state = ?1, updated_at = ?2,
                 snoozed_until = CASE WHEN ?1 = 'done' THEN NULL ELSE snoozed_until END
               WHERE id = ?3 AND user_id = ?4 AND state != ?1",
            params![state, time::now(), id, user_id],
        )?;
        if n > 0 {
            changed.push(*id);
        }
    }
    changed.sort();
    Ok(changed)
}

/// `resolve_todos_for_target(discussion, user)`: the user's to-dos on that thread's notes.
pub fn resolve_for_discussion(c: &Connection, mr: &MergeRequest, user_id: i64, discussion_id: &str) -> Result<()> {
    c.execute(
        "UPDATE todos SET state = 'done', snoozed_until = NULL, updated_at = ?1 WHERE target_type = 'MergeRequest' AND target_id = ?2 AND user_id = ?3
         AND state = 'pending' AND note_id IN (SELECT id FROM notes WHERE discussion_id = ?4)",
        params![time::now(), mr.id, user_id, discussion_id],
    )?;
    Ok(())
}

/// A to-do on a commit (mentions in commit comments).
pub fn create_for_commit(c: &Connection, user_id: i64, author: &User, p: &Project, sha: &str, action: i64, note_id: Option<i64>) -> Result<i64> {
    let now = time::now();
    c.execute(
        "INSERT INTO todos (user_id, author_id, project_id, target_type, target_id, commit_id, action, note_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'Commit', 0, ?4, ?5, ?6, ?7, ?7)",
        params![user_id, author.id, p.id, sha, action, note_id, now],
    )?;
    Ok(c.last_insert_rowid())
}
