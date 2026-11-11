//! Loading resources by `:id` with GitLab's visibility rules (invisible means 404).

use crate::ctx::{ApiError, Req};
use labrat_core::model::{Namespace, Project, User};
use labrat_core::perms::{GroupPerms, ProjectPerms};
use labrat_core::{Error, Result};
use rusqlite::Connection;

fn is_int(s: &str) -> bool {
    !s.is_empty()
        && s.trim_start_matches('-')
            .chars()
            .all(|c| c.is_ascii_digit())
}

/// Finds a project by numeric id or full path, without permission checks.
pub fn project_by_ref(c: &Connection, id: &str) -> Result<Option<Project>> {
    if is_int(id) {
        Project::find(c, id.parse().unwrap_or(0))
    } else {
        Project::find_by_full_path(c, id)
    }
}

/// A project the current user can read, plus their permissions.
pub fn project(c: &Connection, req: &Req, id: &str) -> Result<(Project, ProjectPerms), ApiError> {
    let user = req.maybe_user()?;
    let project = project_by_ref(c, id)?.ok_or(Error::not_found("Project"))?;
    let perms = ProjectPerms::load(c, user, &project)?;
    if !perms.read_project() {
        return Err(Error::not_found("Project").into());
    }
    Ok((project, perms))
}

pub fn group_by_ref(c: &Connection, id: &str) -> Result<Option<Namespace>> {
    let ns = if is_int(id) {
        Namespace::find(c, id.parse().unwrap_or(0))?
    } else {
        Namespace::find_by_full_path(c, id)?
    };
    Ok(ns.filter(Namespace::is_group))
}

/// A group the current user can read, plus their permissions.
pub fn group(c: &Connection, req: &Req, id: &str) -> Result<(Namespace, GroupPerms), ApiError> {
    let user = req.maybe_user()?;
    let group = group_by_ref(c, id)?.ok_or(Error::not_found("Group"))?;
    let perms = GroupPerms::load(c, user, &group)?;
    if !perms.read_group() {
        return Err(Error::not_found("Group").into());
    }
    Ok((group, perms))
}

/// A user by numeric id (`:id` params on /users are integers in GitLab).
pub fn user_by_id(c: &Connection, id: &str) -> Result<User> {
    let id: i64 = id
        .parse()
        .map_err(|_| Error::Param("id is invalid".into()))?;
    User::find(c, id)?.ok_or(Error::not_found("User"))
}

/// A user by id or username (`:user_id` params that accept both).
pub fn user_by_ref(c: &Connection, id: &str) -> Result<Option<User>> {
    if is_int(id) {
        User::find(c, id.parse().unwrap_or(0))
    } else {
        User::find_by_username(c, id)
    }
}
