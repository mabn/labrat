//! Protected tags (`ProtectedTag`, `ProtectedTag::CreateAccessLevel`).

use crate::Result;
use crate::model::{Project, User};
use crate::perms::ProjectPerms;
use crate::repo::ref_matches;
use rusqlite::{Connection, params};

pub struct ProtectedTag {
    pub id: i64,
    pub name: String,
    /// `(id, access_level)` of each create access level.
    pub create_levels: Vec<(i64, i64)>,
}

pub fn list(c: &Connection, project_id: i64) -> Result<Vec<ProtectedTag>> {
    let mut st = c.prepare("SELECT id, name FROM protected_tags WHERE project_id = ?1 ORDER BY id")?;
    let rows: Vec<(i64, String)> = st.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for (id, name) in rows {
        let mut st = c.prepare("SELECT id, access_level FROM protected_tag_create_access_levels WHERE protected_tag_id = ?1 ORDER BY id")?;
        let create_levels = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        out.push(ProtectedTag { id, name, create_levels });
    }
    Ok(out)
}

pub fn find(c: &Connection, project_id: i64, name: &str) -> Result<Option<ProtectedTag>> {
    Ok(list(c, project_id)?.into_iter().find(|t| t.name == name))
}

pub fn create(c: &Connection, project_id: i64, name: &str, level: i64) -> Result<ProtectedTag> {
    c.execute("INSERT INTO protected_tags (project_id, name, created_at) VALUES (?1, ?2, ?3)", params![project_id, name, crate::time::now()])?;
    let id = c.last_insert_rowid();
    c.execute("INSERT INTO protected_tag_create_access_levels (protected_tag_id, access_level) VALUES (?1, ?2)", params![id, level])?;
    Ok(find(c, project_id, name)?.expect("just created"))
}

pub fn delete(c: &Connection, t: &ProtectedTag) -> Result<()> {
    c.execute("DELETE FROM protected_tag_create_access_levels WHERE protected_tag_id = ?1", [t.id])?;
    c.execute("DELETE FROM protected_tags WHERE id = ?1", [t.id])?;
    Ok(())
}

/// `ProtectedTag.protected?`: a rule's name (with `*` wildcards) matches the tag.
pub fn is_protected(c: &Connection, project_id: i64, tag: &str) -> Result<bool> {
    Ok(list(c, project_id)?.iter().any(|t| ref_matches(&t.name, tag)))
}

/// `UserAccess#can_create_tag?`: for a protected tag, some matching rule's access level must be
/// met ("No one" never is); otherwise `admin_tag` (Maintainer).
pub fn can_create(c: &Connection, user: &User, p: &Project, tag: &str) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    let rules: Vec<ProtectedTag> = list(c, p.id)?.into_iter().filter(|t| ref_matches(&t.name, tag)).collect();
    if rules.is_empty() {
        return Ok(perms.admin_project());
    }
    let level = if perms.admin { 60 } else { perms.level };
    Ok(rules.iter().any(|t| t.create_levels.iter().any(|(_, l)| *l != 0 && level >= *l)))
}
