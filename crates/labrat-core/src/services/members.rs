use crate::model::{Member, User, access};
use crate::{Error, Result, time};
use rusqlite::{Connection, params};

/// Adds or replaces a membership. `source_type` is "Project" or "Namespace".
pub fn add(
    c: &Connection,
    actor: Option<&User>,
    source_type: &str,
    source_id: i64,
    user_id: i64,
    access_level: i64,
    expires_at: Option<&str>,
) -> Result<Member> {
    if !access::valid(access_level) {
        return Err(Error::Param(
            "access_level does not have a valid value".into(),
        ));
    }
    let now = time::now();
    let existed = Member::find(c, source_type, source_id, user_id)?.is_some();
    c.execute(
        "INSERT INTO members (source_type, source_id, user_id, access_level, expires_at, created_by_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT (source_type, source_id, user_id)
         DO UPDATE SET access_level = excluded.access_level, expires_at = excluded.expires_at, updated_at = excluded.updated_at",
        params![source_type, source_id, user_id, access_level, expires_at, actor.map(|a| a.id), now],
    )?;
    // `ProjectMember#post_create_hook`: a `joined` event for new project members.
    if source_type == "Project" && !existed {
        crate::services::events::record(c, Some(source_id), user_id, "joined", Some("Project"), Some(source_id), None)?;
    }
    Member::find(c, source_type, source_id, user_id)?.ok_or(Error::not_found("Member"))
}

pub fn update(
    c: &Connection,
    member: &Member,
    access_level: Option<i64>,
    expires_at: Option<Option<&str>>,
) -> Result<Member> {
    if let Some(level) = access_level {
        if !access::valid(level) {
            return Err(Error::Param(
                "access_level does not have a valid value".into(),
            ));
        }
        c.execute(
            "UPDATE members SET access_level = ?1 WHERE id = ?2",
            params![level, member.id],
        )?;
    }
    if let Some(exp) = expires_at {
        c.execute(
            "UPDATE members SET expires_at = ?1 WHERE id = ?2",
            params![exp, member.id],
        )?;
    }
    c.execute(
        "UPDATE members SET updated_at = ?1 WHERE id = ?2",
        params![time::now(), member.id],
    )?;
    Member::find(c, &member.source_type, member.source_id, member.user_id)?
        .ok_or(Error::not_found("Member"))
}

pub fn remove(c: &Connection, member: &Member) -> Result<()> {
    c.execute("DELETE FROM members WHERE id = ?1", [member.id])?;
    // `Members::DestroyService`: `user.invalidate_cache_counts`.
    crate::services::user_counts::invalidate_everything(c, member.user_id)?;
    Ok(())
}

/// Direct members of a source, ordered like GitLab's members API (by id).
pub fn list(c: &Connection, source_type: &str, source_id: i64) -> Result<Vec<Member>> {
    let mut st = c.prepare(&format!(
        "SELECT {} FROM members WHERE source_type = ?1 AND source_id = ?2 ORDER BY id",
        Member::COLUMNS
    ))?;
    let v = st
        .query_map(params![source_type, source_id], Member::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
