//! Access levels and abilities, following GitLab's policies.

use crate::Result;
use crate::model::{Namespace, Project, User, access, feature, visibility};
use rusqlite::{Connection, params};

/// Highest non-expired membership level of `user_id` in the namespace or its ancestors.
pub fn group_access(c: &Connection, user_id: i64, namespace_id: i64) -> Result<i64> {
    let ids = Namespace::self_and_ancestor_ids(c, namespace_id)?;
    let today = crate::time::today();
    let mut best = access::NO_ACCESS;
    for id in ids {
        let level: Option<i64> = c
            .query_row(
                "SELECT access_level FROM members WHERE source_type = 'Namespace' AND source_id = ?1
                 AND user_id = ?2 AND (expires_at IS NULL OR expires_at > ?3)",
                params![id, user_id, today],
                |r| r.get(0),
            )
            .ok();
        best = best.max(level.unwrap_or(0));
    }
    Ok(best)
}

/// Highest access level of `user` in `project`: direct membership or inherited from groups.
pub fn project_access(c: &Connection, user: &User, project: &Project) -> Result<i64> {
    let today = crate::time::today();
    let direct: i64 = c
        .query_row(
            "SELECT access_level FROM members WHERE source_type = 'Project' AND source_id = ?1
             AND user_id = ?2 AND (expires_at IS NULL OR expires_at > ?3)",
            params![project.id, user.id, today],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let ns = Namespace::find(c, project.namespace_id)?;
    let inherited = match ns {
        Some(ns) if ns.is_group() => group_access(c, user.id, ns.id)?,
        Some(ns) if ns.owner_id == Some(user.id) => access::OWNER,
        _ => 0,
    };
    Ok(direct.max(inherited))
}

/// What a (possibly anonymous) user may do with a project.
#[derive(Debug, Clone, Copy)]
pub struct ProjectPerms {
    pub level: i64,
    pub admin: bool,
    pub authenticated: bool,
    pub external: bool,
    pub visibility: i64,
}

impl ProjectPerms {
    pub fn load(c: &Connection, user: Option<&User>, project: &Project) -> Result<ProjectPerms> {
        let level = match user {
            Some(u) => project_access(c, u, project)?,
            None => 0,
        };
        Ok(ProjectPerms {
            level,
            admin: user.is_some_and(|u| u.admin),
            authenticated: user.is_some(),
            external: user.is_some_and(|u| u.external),
            visibility: project.visibility_level,
        })
    }

    fn at_least(&self, level: i64) -> bool {
        self.admin || self.level >= level
    }

    pub fn read_project(&self) -> bool {
        if self.admin || self.level >= access::GUEST {
            return true;
        }
        match self.visibility {
            visibility::PUBLIC => true,
            visibility::INTERNAL => self.authenticated && !self.external,
            _ => false,
        }
    }

    /// Access to a project feature such as merge requests or the repository.
    pub fn feature(&self, project: &Project, name: &str, min_member_level: i64) -> bool {
        if !self.read_project() {
            return false;
        }
        match project.feature_level(name) {
            feature::DISABLED => false,
            feature::PRIVATE => self.at_least(min_member_level),
            _ => {
                if self.visibility == visibility::PRIVATE {
                    self.at_least(min_member_level)
                } else {
                    true
                }
            }
        }
    }

    pub fn read_code(&self, project: &Project) -> bool {
        self.feature(project, "repository", access::REPORTER)
    }

    pub fn read_merge_request(&self, project: &Project) -> bool {
        self.feature(project, "merge_requests", access::REPORTER) && self.read_code(project)
    }

    pub fn create_merge_request_in(&self, project: &Project) -> bool {
        !project.archived
            && self.at_least(access::DEVELOPER)
            && project.feature_level("merge_requests") != feature::DISABLED
            && project.feature_level("repository") != feature::DISABLED
    }

    pub fn admin_project(&self) -> bool {
        self.at_least(access::MAINTAINER)
    }

    pub fn owner(&self) -> bool {
        self.at_least(access::OWNER)
    }

    pub fn developer(&self) -> bool {
        self.at_least(access::DEVELOPER)
    }

    pub fn reporter(&self) -> bool {
        self.at_least(access::REPORTER)
    }

    pub fn guest(&self) -> bool {
        self.at_least(access::GUEST)
    }
}

/// What a (possibly anonymous) user may do with a group.
#[derive(Debug, Clone, Copy)]
pub struct GroupPerms {
    pub level: i64,
    pub admin: bool,
    pub authenticated: bool,
    pub external: bool,
    pub visibility: i64,
    pub has_visible_projects: bool,
}

impl GroupPerms {
    pub fn load(c: &Connection, user: Option<&User>, group: &Namespace) -> Result<GroupPerms> {
        let level = match user {
            Some(u) => group_access(c, u.id, group.id)?,
            None => 0,
        };
        // Members of projects or subgroups inside a private group can see the group.
        let mut has_visible_projects = false;
        if let Some(u) = user
            && level == 0 && !u.admin {
                let ids = Namespace::self_and_descendant_ids(c, group.id)?;
                let list = ids
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let n: i64 = c.query_row(
                    &format!(
                        "SELECT count(*) FROM members m WHERE m.user_id = ?1 AND (
                           (m.source_type = 'Namespace' AND m.source_id IN ({list})) OR
                           (m.source_type = 'Project' AND m.source_id IN (SELECT id FROM projects WHERE namespace_id IN ({list}))))"
                    ),
                    [u.id],
                    |r| r.get(0),
                )?;
                has_visible_projects = n > 0;
            }
        Ok(GroupPerms {
            level,
            admin: user.is_some_and(|u| u.admin),
            authenticated: user.is_some(),
            external: user.is_some_and(|u| u.external),
            visibility: group.visibility_level,
            has_visible_projects,
        })
    }

    pub fn read_group(&self) -> bool {
        if self.admin || self.level >= access::GUEST || self.has_visible_projects {
            return true;
        }
        match self.visibility {
            visibility::PUBLIC => true,
            visibility::INTERNAL => self.authenticated && !self.external,
            _ => false,
        }
    }

    fn at_least(&self, level: i64) -> bool {
        self.admin || self.level >= level
    }

    pub fn admin_group(&self) -> bool {
        self.at_least(access::OWNER)
    }

    pub fn maintainer(&self) -> bool {
        self.at_least(access::MAINTAINER)
    }

    pub fn developer(&self) -> bool {
        self.at_least(access::DEVELOPER)
    }

    pub fn owner(&self) -> bool {
        self.at_least(access::OWNER)
    }
}
