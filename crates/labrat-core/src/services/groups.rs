use crate::model::{Namespace, User, access, visibility};
use crate::services::paths;
use crate::{Error, Result, time};
use rusqlite::{Connection, params};

/// Project creation levels (GitLab's `Gitlab::Access` project creation constants).
pub mod creation_level {
    pub const NO_ONE: i64 = 0;
    pub const MAINTAINER: i64 = 1;
    pub const DEVELOPER: i64 = 2;
    pub const ADMINISTRATOR: i64 = 3;
    pub const OWNER: i64 = 4;

    pub fn project_name(level: i64) -> &'static str {
        match level {
            NO_ONE => "noone",
            MAINTAINER => "maintainer",
            ADMINISTRATOR => "administrator",
            OWNER => "owner",
            _ => "developer",
        }
    }

    pub fn parse_project(s: &str) -> Option<i64> {
        match s {
            "noone" => Some(NO_ONE),
            "maintainer" => Some(MAINTAINER),
            "developer" => Some(DEVELOPER),
            "administrator" => Some(ADMINISTRATOR),
            "owner" => Some(OWNER),
            _ => None,
        }
    }

    /// Subgroup creation: 0 owner, 1 maintainer.
    pub fn subgroup_name(level: i64) -> &'static str {
        if level == 0 { "owner" } else { "maintainer" }
    }

    pub fn parse_subgroup(s: &str) -> Option<i64> {
        match s {
            "owner" => Some(0),
            "maintainer" => Some(1),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct NewGroup {
    pub name: String,
    pub path: String,
    pub parent_id: Option<i64>,
    pub description: Option<String>,
    pub visibility: Option<i64>,
    pub project_creation_level: Option<i64>,
    pub subgroup_creation_level: Option<i64>,
    pub request_access_enabled: Option<bool>,
}

/// `Groups::CreateService`.
pub fn create(c: &Connection, actor: &User, new: NewGroup) -> Result<Namespace> {
    let parent = match new.parent_id {
        Some(pid) => Some(
            Namespace::find(c, pid)?
                .filter(Namespace::is_group)
                .ok_or(Error::not_found("Parent group"))?,
        ),
        None => None,
    };
    if let Some(p) = &parent {
        let perms = crate::perms::GroupPerms::load(c, Some(actor), p)?;
        let needed = if p.subgroup_creation_level.unwrap_or(1) == 0 {
            access::OWNER
        } else {
            access::MAINTAINER
        };
        if !(perms.admin || perms.level >= needed) {
            return Err(Error::Forbidden(Some(
                "You don't have permission to create a subgroup in this group.".into(),
            )));
        }
    } else if !actor.can_create_group && !actor.admin {
        return Err(Error::Forbidden(Some(
            "You don't have permission to create groups.".into(),
        )));
    }

    let mut errs = vec![];
    if new.name.trim().is_empty() {
        errs.push(("name".to_string(), "can't be blank".to_string()));
    }
    if !paths::valid_namespace_path(&new.path) {
        errs.push(("path".into(), paths::NAMESPACE_PATH_MESSAGE.into()));
    }
    if parent.is_none() && paths::reserved_top_level(&new.path) {
        errs.push(("path".into(), format!("{} is a reserved name", new.path)));
    }
    let full_path = match &parent {
        Some(p) => format!("{}/{}", p.full_path, new.path),
        None => new.path.clone(),
    };
    if errs.is_empty() && paths::route_taken(c, &full_path)? {
        errs.push(("path".into(), "has already been taken".into()));
    }
    let vis = new.visibility.unwrap_or(visibility::PRIVATE);
    if let Some(p) = &parent
        && vis > p.visibility_level {
            errs.push((
                "visibility_level".into(),
                format!(
                    "{} is not allowed since the parent group has a {} visibility.",
                    visibility::name(vis),
                    visibility::name(p.visibility_level)
                ),
            ));
        }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }

    let now = time::now();
    c.execute(
        "INSERT INTO namespaces (type, name, path, parent_id, owner_id, description, visibility_level, request_access_enabled,
             project_creation_level, subgroup_creation_level, created_at, updated_at)
         VALUES ('Group', ?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            new.name,
            new.path,
            new.parent_id,
            new.description,
            vis,
            new.request_access_enabled.unwrap_or(true),
            new.project_creation_level.unwrap_or(creation_level::DEVELOPER),
            new.subgroup_creation_level.unwrap_or(1),
            now
        ],
    )?;
    let id = c.last_insert_rowid();
    let full_name = match &parent {
        Some(p) => format!("{} / {}", p.full_name, new.name),
        None => new.name.clone(),
    };
    paths::insert_route(c, "Namespace", id, &full_path, &full_name)?;
    // The creator becomes an owner unless they already own an ancestor... GitLab always adds them.
    crate::services::members::add(c, None, "Namespace", id, actor.id, access::OWNER, None)?;
    Namespace::find(c, id)?.ok_or(Error::not_found("Group"))
}

#[derive(Debug, Default, Clone)]
pub struct GroupUpdate {
    pub name: Option<String>,
    pub path: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<i64>,
    pub project_creation_level: Option<i64>,
    pub subgroup_creation_level: Option<i64>,
    pub request_access_enabled: Option<bool>,
    pub settings: Vec<(String, serde_json::Value)>,
}

pub fn update(c: &Connection, group: &Namespace, upd: GroupUpdate) -> Result<Namespace> {
    let mut errs = vec![];
    if let Some(n) = &upd.name
        && n.trim().is_empty() {
            errs.push(("name".to_string(), "can't be blank".to_string()));
        }
    if let Some(p) = &upd.path
        && !paths::valid_namespace_path(p) {
            errs.push(("path".into(), paths::NAMESPACE_PATH_MESSAGE.into()));
        }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }
    let now = time::now();
    let new_name = upd.name.clone().unwrap_or(group.name.clone());
    let new_path = upd.path.clone().unwrap_or(group.path.clone());
    if new_path != group.path || new_name != group.name {
        let prefix = match group.full_path.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/"),
            None => String::new(),
        };
        let name_prefix = match group.full_name.rsplit_once(" / ") {
            Some((parent, _)) => format!("{parent} / "),
            None => String::new(),
        };
        let new_full = format!("{prefix}{new_path}");
        if new_path != group.path && paths::route_taken(c, &new_full)? {
            return Err(Error::validation("path", "has already been taken"));
        }
        paths::rename_routes(
            c,
            &group.full_path,
            &new_full,
            &group.full_name,
            &format!("{name_prefix}{new_name}"),
        )?;
        c.execute(
            "UPDATE namespaces SET name = ?1, path = ?2 WHERE id = ?3",
            params![new_name, new_path, group.id],
        )?;
    }
    if let Some(d) = &upd.description {
        c.execute(
            "UPDATE namespaces SET description = ?1 WHERE id = ?2",
            params![d, group.id],
        )?;
    }
    if let Some(v) = upd.visibility {
        c.execute(
            "UPDATE namespaces SET visibility_level = ?1 WHERE id = ?2",
            params![v, group.id],
        )?;
    }
    if let Some(v) = upd.project_creation_level {
        c.execute(
            "UPDATE namespaces SET project_creation_level = ?1 WHERE id = ?2",
            params![v, group.id],
        )?;
    }
    if let Some(v) = upd.subgroup_creation_level {
        c.execute(
            "UPDATE namespaces SET subgroup_creation_level = ?1 WHERE id = ?2",
            params![v, group.id],
        )?;
    }
    if let Some(v) = upd.request_access_enabled {
        c.execute(
            "UPDATE namespaces SET request_access_enabled = ?1 WHERE id = ?2",
            params![v, group.id],
        )?;
    }
    if !upd.settings.is_empty() {
        let mut s = group.settings.clone();
        for (k, v) in upd.settings {
            s.insert(k, v);
        }
        c.execute(
            "UPDATE namespaces SET settings = ?1 WHERE id = ?2",
            params![serde_json::Value::Object(s).to_string(), group.id],
        )?;
    }
    c.execute(
        "UPDATE namespaces SET updated_at = ?1 WHERE id = ?2",
        params![now, group.id],
    )?;
    Namespace::find(c, group.id)?.ok_or(Error::not_found("Group"))
}

/// Deletes a group with its subgroups and projects.
pub fn delete(c: &Connection, group: &Namespace) -> Result<()> {
    let ids = Namespace::self_and_descendant_ids(c, group.id)?;
    for ns in ids.iter().rev() {
        let projects: Vec<i64> = c
            .prepare("SELECT id FROM projects WHERE namespace_id = ?1")?
            .query_map([ns], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for p in projects {
            crate::services::projects::delete_by_id(c, p)?;
        }
        c.execute(
            "DELETE FROM members WHERE source_type = 'Namespace' AND source_id = ?1",
            [ns],
        )?;
        c.execute(
            "DELETE FROM routes WHERE source_type = 'Namespace' AND source_id = ?1",
            [ns],
        )?;
        c.execute("DELETE FROM namespaces WHERE id = ?1", [ns])?;
    }
    Ok(())
}
