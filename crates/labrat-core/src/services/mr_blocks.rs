//! EE merge request dependencies: a merge request can't merge while another one (in any project)
//! it depends on isn't merged. Closed blockers still block.
//!
//! The behaviour follows recordings of an EE 18.11.14 instance (`fixtures-ee/mr_dependencies`):
//! the system notes on both sides, the errors, and that merged blockers stay listed.

use crate::model::{Project, User};
use crate::services::merge_requests::MergeRequest;
use crate::services::notes;
use crate::{Result, jobs, time};
use rusqlite::{Connection, params};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct Block {
    pub id: i64,
    pub blocking_merge_request_id: i64,
    pub blocked_merge_request_id: i64,
    pub created_at: i64,
}

fn rows(c: &Connection, sql: &str, id: i64) -> Result<Vec<Block>> {
    let mut st = c.prepare(sql)?;
    let out = st
        .query_map([id], |r| {
            Ok(Block { id: r.get(0)?, blocking_merge_request_id: r.get(1)?, blocked_merge_request_id: r.get(2)?, created_at: r.get(3)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(out)
}

/// The blocks that keep `mr_id` from merging, oldest first.
pub fn blocking(c: &Connection, mr_id: i64) -> Result<Vec<Block>> {
    rows(
        c,
        "SELECT id, blocking_merge_request_id, blocked_merge_request_id, created_at FROM merge_request_blocks WHERE blocked_merge_request_id = ?1 ORDER BY id",
        mr_id,
    )
}

/// The blocks where `mr_id` is the blocker, oldest first.
pub fn blocked(c: &Connection, mr_id: i64) -> Result<Vec<Block>> {
    rows(
        c,
        "SELECT id, blocking_merge_request_id, blocked_merge_request_id, created_at FROM merge_request_blocks WHERE blocking_merge_request_id = ?1 ORDER BY id",
        mr_id,
    )
}

pub fn find(c: &Connection, id: i64) -> Result<Option<Block>> {
    Ok(rows(c, "SELECT id, blocking_merge_request_id, blocked_merge_request_id, created_at FROM merge_request_blocks WHERE id = ?1", id)?.pop())
}

/// Whether a blocker of `mr` isn't merged yet (the `merge_request_blocked` check).
pub fn is_blocked(c: &Connection, mr: &MergeRequest) -> Result<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM merge_request_blocks b JOIN merge_requests m ON m.id = b.blocking_merge_request_id
         WHERE b.blocked_merge_request_id = ?1 AND m.state != 'merged'",
        [mr.id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// GitLab's `to_reference(from)` for a merge request: `!1` in the same project, `project!1` in
/// another project of the same namespace, `group/project!1` otherwise.
pub fn reference(c: &Connection, mr: &MergeRequest, from: &Project) -> Result<String> {
    if mr.target_project_id == from.id {
        return Ok(format!("!{}", mr.iid));
    }
    let p = Project::find(c, mr.target_project_id)?.expect("project exists");
    if p.namespace_id == from.namespace_id {
        return Ok(format!("{}!{}", p.path, mr.iid));
    }
    Ok(format!("{}!{}", p.full_path, mr.iid))
}

/// Why a block can't be created.
pub enum CreateError {
    SelfReferential,
    Exists,
}

/// Makes `blocked` depend on `blocking`, with a system note on each.
pub fn create(c: &Connection, user: &User, blocking: &MergeRequest, blocked: &MergeRequest) -> Result<std::result::Result<Block, CreateError>> {
    if blocking.id == blocked.id {
        return Ok(Err(CreateError::SelfReferential));
    }
    let exists: bool = c.query_row(
        "SELECT count(*) FROM merge_request_blocks WHERE blocking_merge_request_id = ?1 AND blocked_merge_request_id = ?2",
        params![blocking.id, blocked.id],
        |r| r.get::<_, i64>(0),
    )? > 0;
    if exists {
        return Ok(Err(CreateError::Exists));
    }
    let now = time::now();
    c.execute(
        "INSERT INTO merge_request_blocks (blocking_merge_request_id, blocked_merge_request_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![blocking.id, blocked.id, now],
    )?;
    let id = c.last_insert_rowid();
    let (bp, dp) = projects(c, blocking, blocked)?;
    notes::system_note(c, blocked.id, dp.id, user, &format!("marked this merge request as blocked by {}", reference(c, blocking, &dp)?), "relate")?;
    notes::system_note(c, blocking.id, bp.id, user, &format!("marked this merge request as blocking {}", reference(c, blocked, &bp)?), "relate")?;
    Ok(Ok(Block { id, blocking_merge_request_id: blocking.id, blocked_merge_request_id: blocked.id, created_at: now }))
}

fn projects(c: &Connection, blocking: &MergeRequest, blocked: &MergeRequest) -> Result<(Project, Project)> {
    Ok((
        Project::find(c, blocking.target_project_id)?.expect("project exists"),
        Project::find(c, blocked.target_project_id)?.expect("project exists"),
    ))
}

/// Removes a block, with a system note on each side.
pub fn destroy(c: &Connection, user: &User, block: &Block) -> Result<()> {
    c.execute("DELETE FROM merge_request_blocks WHERE id = ?1", [block.id])?;
    let blocking = MergeRequest::get(c, block.blocking_merge_request_id)?;
    let blocked = MergeRequest::get(c, block.blocked_merge_request_id)?;
    let (bp, dp) = projects(c, &blocking, &blocked)?;
    notes::system_note(c, blocked.id, dp.id, user, &format!("removed the relation with {}", reference(c, &blocking, &dp)?), "unrelate")?;
    notes::system_note(c, blocking.id, bp.id, user, &format!("removed the relation with {}", reference(c, &blocked, &bp)?), "unrelate")?;
    Ok(())
}

/// After `mr` merged: the MRs it blocked may auto-merge now.
pub fn after_merge(c: &Connection, mr: &MergeRequest) -> Result<()> {
    for b in blocked(c, mr.id)? {
        let Some(other) = MergeRequest::find(c, b.blocked_merge_request_id)? else { continue };
        if other.is_open() && other.merge_when_pipeline_succeeds {
            jobs::enqueue(c, "mr_auto_merge", json!({"merge_request_id": other.id}))?;
        }
    }
    Ok(())
}
