//! Approvals, reviewer states and draft notes (pending reviews).

use crate::db::OptionalExt;
use crate::model::{Project, User, access};
use crate::perms::ProjectPerms;
use crate::services::comments::{self, CreateParams};
use crate::services::merge_requests::MergeRequest;
use crate::services::notes::{self, Note};
use crate::services::positions::Position;
use crate::{Config, Error, Result, time};
use rusqlite::{Connection, Row, params};
use serde_json::json;

pub mod state {
    pub const UNREVIEWED: i64 = 0;
    pub const REVIEWED: i64 = 1;
    pub const REQUESTED_CHANGES: i64 = 2;
    pub const APPROVED: i64 = 3;
    pub const UNAPPROVED: i64 = 4;
    pub const REVIEW_STARTED: i64 = 5;

    pub fn parse(s: &str) -> Option<i64> {
        Some(match s {
            "unreviewed" => UNREVIEWED,
            "reviewed" => REVIEWED,
            "requested_changes" => REQUESTED_CHANGES,
            "approved" => APPROVED,
            "unapproved" => UNAPPROVED,
            "review_started" => REVIEW_STARTED,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Approvals

/// `:approve_merge_request`: can update the MR and is a project member.
pub fn can_approve(c: &Connection, p: &Project, mr: &MergeRequest, user: &User) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    // The update rights without the archive rule: REST approvals still work on archived
    // projects (recorded on 18.11), although GraphQL `canApprove` says false.
    let can_update = perms.level >= access::DEVELOPER || perms.admin || mr.author_id == user.id || mr.assignee_ids(c)?.contains(&user.id);
    Ok(perms.level >= access::GUEST && can_update)
}

/// EE: someone has requested changes and not approved since.
pub fn changes_requested(c: &Connection, mr: &MergeRequest) -> Result<bool> {
    let n: i64 = c.query_row("SELECT count(*) FROM merge_request_requested_changes WHERE merge_request_id = ?1", [mr.id], |r| r.get(0))?;
    Ok(n > 0)
}

pub fn has_approved(c: &Connection, mr: &MergeRequest, user: &User) -> Result<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM approvals WHERE merge_request_id = ?1 AND user_id = ?2",
        params![mr.id, user.id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Approvals as (user id, approved_at), in approval id order.
pub fn approvals(c: &Connection, mr: &MergeRequest) -> Result<Vec<(i64, i64)>> {
    let mut st = c.prepare(
        "SELECT user_id, created_at FROM approvals WHERE merge_request_id = ?1 ORDER BY id",
    )?;
    let v = st
        .query_map([mr.id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// `MergeRequests::ApprovalService`. Returns false when the user can't approve (API: 401).
pub fn approve(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<bool> {
    if mr.state == "merged" || has_approved(c, mr, user)? || !can_approve(c, p, mr, user)? {
        return Ok(false);
    }
    if config.is_ee() && crate::services::approvals::eligibility(c, p, mr, user)? != crate::services::approvals::Eligibility::Yes {
        return Ok(false);
    }
    let now = time::now();
    // EE: approving withdraws the user's requested changes.
    c.execute("DELETE FROM merge_request_requested_changes WHERE merge_request_id = ?1 AND user_id = ?2", params![mr.id, user.id])?;
    c.execute(
        "INSERT INTO approvals (merge_request_id, user_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![mr.id, user.id, now],
    )?;
    set_reviewer_state(c, mr, user, state::APPROVED)?;
    // `MergeRequests::ApprovedEvent` subscribers run asynchronously in GitLab.
    crate::jobs::enqueue(
        c,
        "approval_events",
        json!({"merge_request_id": mr.id, "user_id": user.id}),
    )?;
    Ok(true)
}

/// Approval note, to-do resolution and webhook (`CreateApprovalNoteWorker` & co).
pub fn approval_events_job(
    c: &Connection,
    config: &Config,
    mr_id: i64,
    user_id: i64,
) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else {
        return Ok(());
    };
    let p = Project::find(c, mr.target_project_id)?.expect("project exists");
    let user = User::get(c, user_id)?;
    crate::services::events::record(c, Some(p.id), user.id, "approved", Some("MergeRequest"), Some(mr.id), None)?;
    notes::system_note(
        c,
        mr.id,
        p.id,
        &user,
        "approved this merge request",
        "approved",
    )?;
    crate::services::todos::resolve_for_target(c, &mr, Some(user.id))?;
    // EE reports a partial approval as `approval` and the final one as `approved`.
    let action = if config.is_ee() && !crate::services::approvals::state(c, &p, &mr)?.approved() { "approval" } else { "approved" };
    crate::services::hooks::merge_request_event(c, config, &p, &mr, Some(&user), action, None)
}

/// `MergeRequests::RemoveApprovalService`. Returns false when there was nothing to remove.
pub fn unapprove(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<bool> {
    if mr.state == "merged" || !has_approved(c, mr, user)? {
        return Ok(false);
    }
    // EE: `unapproved` when this takes away a full approval, `unapproval` otherwise.
    let action = if !config.is_ee() || crate::services::approvals::state(c, p, mr)?.approved() { "unapproved" } else { "unapproval" };
    c.execute(
        "DELETE FROM approvals WHERE merge_request_id = ?1 AND user_id = ?2",
        params![mr.id, user.id],
    )?;
    set_reviewer_state(c, mr, user, state::UNAPPROVED)?;
    // The synchronous system note's (throttled) touch is the only change the hook reports.
    let before = crate::services::merge_requests::snapshot(c, mr)?;
    notes::system_note(
        c,
        mr.id,
        p.id,
        user,
        "unapproved this merge request",
        "unapproved",
    )?;
    let mr = mr.reload(c)?;
    crate::services::hooks::merge_request_event(c, config, p, &mr, Some(user), action, Some(&before))?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Reviewer states

fn reviewer_row(c: &Connection, mr: &MergeRequest, user: &User) -> Result<Option<(i64, i64)>> {
    c.query_row(
        "SELECT id, state FROM merge_request_reviewers WHERE merge_request_id = ?1 AND user_id = ?2",
        params![mr.id, user.id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

/// Sets a reviewer's state if the user is a reviewer (errors are ignored as in GitLab).
fn set_reviewer_state(
    c: &Connection,
    mr: &MergeRequest,
    user: &User,
    new_state: i64,
) -> Result<()> {
    if let Some((id, current)) = reviewer_row(c, mr, user)? {
        if current == state::APPROVED
            && new_state != state::REQUESTED_CHANGES
            && new_state != state::UNAPPROVED
            && new_state != state::APPROVED
        {
            return Ok(());
        }
        // `UpdateReviewerStateService`: the assignees and the reviewer.
        crate::services::user_counts::invalidate_assignees_and(c, mr, &[user.id])?;
        c.execute(
            "UPDATE merge_request_reviewers SET state = ?1 WHERE id = ?2",
            params![new_state, id],
        )?;
    }
    Ok(())
}

/// `MergeRequests::UpdateReviewerStateService` with its system notes. Returns the service's error
/// message, if any (the requested-changes note is made before the reviewer check).
pub fn update_reviewer_state(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    name: &str,
) -> Result<Option<String>> {
    let Some(new_state) = state::parse(name) else {
        return Ok(None);
    };
    if new_state == state::REQUESTED_CHANGES {
        notes::system_note(c, mr.id, p.id, user, "requested changes", "requested_changes")?;
        if config.is_ee() {
            c.execute("INSERT OR IGNORE INTO merge_request_requested_changes (merge_request_id, user_id) VALUES (?1, ?2)", params![mr.id, user.id])?;
        }
    } else if new_state == state::REVIEWED {
        notes::system_note(c, mr.id, p.id, user, "left review comments", "reviewed")?;
    }
    let Some((id, current)) = reviewer_row(c, mr, user)? else {
        return Ok(Some("Reviewer not found".into()));
    };
    if current == state::APPROVED && new_state != state::REQUESTED_CHANGES && new_state != state::UNAPPROVED {
        return Ok(Some("Reviewer has approved".into()));
    }
    c.execute("UPDATE merge_request_reviewers SET state = ?1 WHERE id = ?2", params![new_state, id])?;
    crate::services::user_counts::invalidate_assignees_and(c, mr, &[user.id])?;
    // `RemoveApprovalService` leaves merged merge requests alone.
    if new_state == state::REQUESTED_CHANGES && mr.state != "merged" && has_approved(c, mr, user)? {
        c.execute("DELETE FROM approvals WHERE merge_request_id = ?1 AND user_id = ?2", params![mr.id, user.id])?;
    }
    Ok(None)
}

/// `MergeRequests::RequestReviewService`: re-request a review from an existing reviewer.
pub fn rerequest(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    reviewer: &User,
) -> Result<bool> {
    let Some((id, _)) = reviewer_row(c, mr, reviewer)? else {
        return Ok(false);
    };
    let mut before = crate::services::merge_requests::snapshot(c, mr)?;
    before.rerequested = Some(reviewer.id);
    // `RemoveApprovalService` leaves merged merge requests alone.
    let had_approval = mr.state != "merged" && has_approved(c, mr, reviewer)?;
    if had_approval {
        c.execute(
            "DELETE FROM approvals WHERE merge_request_id = ?1 AND user_id = ?2",
            params![mr.id, reviewer.id],
        )?;
    }
    c.execute(
        "UPDATE merge_request_reviewers SET state = ?1 WHERE id = ?2",
        params![state::UNREVIEWED, id],
    )?;
    // `RequestReviewService`: the reviewer and the requester.
    crate::services::user_counts::invalidate(c, &[reviewer.id, user.id])?;
    // No touch of its own: only the system note's throttled touch can move `updated_at`.
    let body = if had_approval {
        format!(
            "requested review from @{} and removed approval",
            reviewer.username
        )
    } else {
        format!("requested review from @{}", reviewer.username)
    };
    notes::system_note(c, mr.id, p.id, user, &body, "reviewer")?;
    crate::services::todos::create(
        c,
        reviewer.id,
        user,
        p,
        mr,
        crate::services::todos::action::REVIEW_REQUESTED,
        None,
    )?;
    crate::services::hooks::merge_request_event(c, config, p, mr, Some(user), "update", Some(&before))?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Draft notes

#[derive(Debug, Clone)]
pub struct Draft {
    pub id: i64,
    pub merge_request_id: i64,
    pub author_id: i64,
    pub note: String,
    pub discussion_id: Option<String>,
    pub resolve_discussion: bool,
    pub position: Option<String>,
    pub line_code: Option<String>,
    pub commit_id: Option<String>,
}

const DRAFT_COLS: &str = "id, merge_request_id, author_id, note, discussion_id, resolve_discussion, position, line_code, commit_id";

fn draft_row(r: &Row) -> rusqlite::Result<Draft> {
    Ok(Draft {
        id: r.get(0)?,
        merge_request_id: r.get(1)?,
        author_id: r.get(2)?,
        note: r.get(3)?,
        discussion_id: r.get(4)?,
        resolve_discussion: r.get(5)?,
        position: r.get(6)?,
        line_code: r.get(7)?,
        commit_id: r.get(8)?,
    })
}

pub fn drafts(c: &Connection, mr: &MergeRequest, user: &User) -> Result<Vec<Draft>> {
    let mut st = c.prepare(&format!("SELECT {DRAFT_COLS} FROM draft_notes WHERE merge_request_id = ?1 AND author_id = ?2 ORDER BY id"))?;
    let v = st
        .query_map(params![mr.id, user.id], draft_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn has_drafts(c: &Connection, mr: &MergeRequest, user: &User) -> Result<bool> {
    Ok(!drafts(c, mr, user)?.is_empty())
}

pub fn find_draft(
    c: &Connection,
    mr: &MergeRequest,
    user: &User,
    id: i64,
) -> Result<Option<Draft>> {
    c.query_row(
        &format!("SELECT {DRAFT_COLS} FROM draft_notes WHERE id = ?1 AND merge_request_id = ?2 AND author_id = ?3"),
        params![id, mr.id, user.id],
        draft_row,
    )
    .optional()
}

/// `DraftNotes::CreateService`. Validation errors are returned as (field, message) pairs.
pub fn create_draft(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    note: &str,
    reply_to: Option<&str>,
    resolve: bool,
    position: Option<Position>,
    commit_id: Option<&str>,
) -> Result<std::result::Result<Draft, Vec<(String, String)>>> {
    let mut discussion_id = None;
    if let Some(did) = reply_to {
        let Some(first) = comments::first_note(c, did)? else {
            return Ok(Err(vec![(
                "base".into(),
                "Thread to reply to cannot be found".into(),
            )]));
        };
        if first.system {
            return Ok(Err(vec![(
                "base".into(),
                "Replies to system notes are not allowed".into(),
            )]));
        }
        if resolve && !comments::can_resolve(c, user, p, mr, &first)? {
            return Ok(Err(vec![(
                "base".into(),
                "User is not allowed to resolve thread".into(),
            )]));
        }
        let dup: i64 = c.query_row(
            "SELECT count(*) FROM draft_notes WHERE merge_request_id = ?1 AND author_id = ?2 AND discussion_id = ?3",
            params![mr.id, user.id, did],
            |r| r.get(0),
        )?;
        if dup > 0 {
            return Ok(Err(vec![(
                "author_id".into(),
                "has already been taken".into(),
            )]));
        }
        discussion_id = Some(did.to_string());
    } else if resolve {
        return Ok(Err(vec![(
            "base".into(),
            "User is not allowed to resolve thread".into(),
        )]));
    }
    if note.len() > 1_048_576 {
        return Ok(Err(vec![(
            "note".into(),
            "is too long. The maximum size is 1 MiB.".into(),
        )]));
    }
    let r = crate::repo::repo(c, p);
    let line_code = match &position {
        Some(pos) if pos.on_text() => pos.line_code(&r)?,
        _ => None,
    };
    let now = time::now();
    c.execute(
        "INSERT INTO draft_notes (merge_request_id, author_id, note, discussion_id, resolve_discussion, position, original_position, line_code, commit_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, ?9)",
        params![mr.id, user.id, note, discussion_id, resolve, position.as_ref().map(|p| p.store()), line_code, commit_id, now],
    )?;
    let id = c.last_insert_rowid();
    if drafts(c, mr, user)?.len() == 1 {
        set_reviewer_state(c, mr, user, state::REVIEW_STARTED)?;
    }
    Ok(Ok(find_draft(c, mr, user, id)?.expect("just inserted")))
}

pub fn update_draft(
    c: &Connection,
    d: &Draft,
    note: &str,
    position: Option<Position>,
) -> Result<()> {
    c.execute(
        "UPDATE draft_notes SET note = ?1, position = ?2, updated_at = ?3 WHERE id = ?4",
        params![note, position.map(|p| p.store()), time::now(), d.id],
    )?;
    Ok(())
}

/// `DraftNotes::DestroyService`.
pub fn delete_draft(c: &Connection, mr: &MergeRequest, user: &User, d: &Draft) -> Result<()> {
    c.execute("DELETE FROM draft_notes WHERE id = ?1", [d.id])?;
    if drafts(c, mr, user)?.is_empty() {
        set_reviewer_state(c, mr, user, state::UNREVIEWED)?;
    }
    Ok(())
}

fn publish_one(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    d: &Draft,
    review_id: Option<i64>,
) -> Result<()> {
    let position = d
        .position
        .as_deref()
        .and_then(Position::parse)
        .filter(|pos| pos.complete());
    let prm = CreateParams {
        body: d.note.clone(),
        thread: true,
        position: if d.discussion_id.is_none() {
            position
        } else {
            None
        },
        reply_to: d.discussion_id.clone(),
        review_id,
        commit_id: d.commit_id.clone(),
        publishing: true,
        ..Default::default()
    };
    // Invalid drafts are dropped silently, like GitLab does.
    let _ = comments::create(c, config, p, mr, user, prm);
    if let Some(did) = &d.discussion_id {
        let first = comments::first_note(c, did)?;
        let can = match &first {
            Some(n) => comments::can_resolve(c, user, p, mr, n)?,
            None => false,
        };
        if d.resolve_discussion && can {
            comments::resolve_discussion(c, config, p, mr, user, did, true)?;
        } else {
            c.execute(
                "UPDATE notes SET resolved_at = NULL, resolved_by_id = NULL WHERE discussion_id = ?1 AND resolvable = 1",
                [did],
            )?;
        }
    }
    c.execute("DELETE FROM draft_notes WHERE id = ?1", [d.id])?;
    Ok(())
}

/// `DraftNotes::PublishService` for one draft.
pub fn publish_draft(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    d: &Draft,
) -> Result<()> {
    if !comments::can_create_note(c, user, p, mr)? {
        return Err(Error::message(500, "Not allowed to create notes"));
    }
    publish_one(c, config, p, mr, user, d, None)
}

thread_local! {
    /// Set while the drafts of a review are turned into notes (see `comments::create`).
    pub static PUBLISHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `DraftNotes::PublishService` for all drafts (a review).
pub fn publish_drafts(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<()> {
    let list = drafts(c, mr, user)?;
    if list.is_empty() {
        return Ok(());
    }
    c.execute(
        "INSERT INTO reviews (merge_request_id, author_id, created_at) VALUES (?1, ?2, ?3)",
        params![mr.id, user.id, time::now()],
    )?;
    let review_id = c.last_insert_rowid();
    crate::services::user_counts::invalidate_assignees_and(c, mr, &[])?;
    for d in &list {
        if d.note.trim().is_empty() {
            c.execute("DELETE FROM draft_notes WHERE id = ?1", [d.id])?;
            continue;
        }
        PUBLISHING.with(|f| f.set(true));
        let published = publish_one(c, config, p, mr, user, d, Some(review_id));
        PUBLISHING.with(|f| f.set(false));
        published?;
    }
    crate::services::todos::resolve_for_target(c, mr, Some(user.id))?;
    // `(assignees | [author]) - reviewer`.
    let mut recipients = mr.assignee_ids(c)?;
    if !recipients.contains(&mr.author_id) {
        recipients.push(mr.author_id);
    }
    for uid in recipients {
        if uid != user.id {
            crate::services::todos::create(
                c,
                uid,
                user,
                p,
                mr,
                crate::services::todos::action::REVIEW_SUBMITTED,
                None,
            )?;
        }
    }
    let _ = json!(null);
    let _: Option<Note> = None;
    Ok(())
}
