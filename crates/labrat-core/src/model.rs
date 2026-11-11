//! Plain data types loaded from the database.

use crate::db::OptionalExt;
use crate::{Error, Result};
use rusqlite::{Connection, Row, params};
use serde_json::{Map, Value};

pub mod access {
    pub const NO_ACCESS: i64 = 0;
    pub const MINIMAL: i64 = 5;
    pub const GUEST: i64 = 10;
    pub const PLANNER: i64 = 15;
    pub const REPORTER: i64 = 20;
    pub const DEVELOPER: i64 = 30;
    pub const MAINTAINER: i64 = 40;
    pub const OWNER: i64 = 50;

    pub fn valid(level: i64) -> bool {
        matches!(
            level,
            MINIMAL | GUEST | PLANNER | REPORTER | DEVELOPER | MAINTAINER | OWNER
        )
    }
}

pub mod visibility {
    pub const PRIVATE: i64 = 0;
    pub const INTERNAL: i64 = 10;
    pub const PUBLIC: i64 = 20;

    pub fn name(level: i64) -> &'static str {
        match level {
            PUBLIC => "public",
            INTERNAL => "internal",
            _ => "private",
        }
    }

    pub fn parse(s: &str) -> Option<i64> {
        match s {
            "private" => Some(PRIVATE),
            "internal" => Some(INTERNAL),
            "public" => Some(PUBLIC),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub name: String,
    pub email: String,
    pub public_email: Option<String>,
    pub commit_email: Option<String>,
    pub encrypted_password: Option<String>,
    pub state: String,
    pub admin: bool,
    pub user_type: i64,
    pub external: bool,
    pub private_profile: bool,
    pub projects_limit: i64,
    pub can_create_group: bool,
    pub bio: String,
    pub location: String,
    pub linkedin: String,
    pub twitter: String,
    pub discord: String,
    pub website_url: String,
    pub github: String,
    pub job_title: String,
    pub organization: String,
    pub pronouns: Option<String>,
    pub note: Option<String>,
    pub theme_id: i64,
    pub color_scheme_id: i64,
    pub preferred_language: String,
    pub timezone: Option<String>,
    pub namespace_id: Option<i64>,
    pub created_by_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub confirmed_at: Option<i64>,
    pub last_sign_in_at: Option<i64>,
    pub current_sign_in_at: Option<i64>,
    pub last_activity_on: Option<String>,
    pub sign_in_count: i64,
}

pub mod user_type {
    pub const HUMAN: i64 = 0;
    pub const PROJECT_BOT: i64 = 6;
    pub const GHOST: i64 = 5;
}

impl User {
    pub const COLUMNS: &'static str = "id, username, name, email, public_email, commit_email, encrypted_password, state, admin, user_type, external, private_profile, projects_limit, can_create_group, bio, location, linkedin, twitter, discord, website_url, github, job_title, organization, pronouns, note, theme_id, color_scheme_id, preferred_language, timezone, namespace_id, created_by_id, created_at, updated_at, confirmed_at, last_sign_in_at, current_sign_in_at, last_activity_on, sign_in_count";

    pub fn from_row(r: &Row) -> rusqlite::Result<User> {
        Ok(User {
            id: r.get(0)?,
            username: r.get(1)?,
            name: r.get(2)?,
            email: r.get(3)?,
            public_email: r.get(4)?,
            commit_email: r.get(5)?,
            encrypted_password: r.get(6)?,
            state: r.get(7)?,
            admin: r.get(8)?,
            user_type: r.get(9)?,
            external: r.get(10)?,
            private_profile: r.get(11)?,
            projects_limit: r.get(12)?,
            can_create_group: r.get(13)?,
            bio: r.get(14)?,
            location: r.get(15)?,
            linkedin: r.get(16)?,
            twitter: r.get(17)?,
            discord: r.get(18)?,
            website_url: r.get(19)?,
            github: r.get(20)?,
            job_title: r.get(21)?,
            organization: r.get(22)?,
            pronouns: r.get(23)?,
            note: r.get(24)?,
            theme_id: r.get(25)?,
            color_scheme_id: r.get(26)?,
            preferred_language: r.get(27)?,
            timezone: r.get(28)?,
            namespace_id: r.get(29)?,
            created_by_id: r.get(30)?,
            created_at: r.get(31)?,
            updated_at: r.get(32)?,
            confirmed_at: r.get(33)?,
            last_sign_in_at: r.get(34)?,
            current_sign_in_at: r.get(35)?,
            last_activity_on: r.get(36)?,
            sign_in_count: r.get(37)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<User>> {
        c.query_row(
            &format!("SELECT {} FROM users WHERE id = ?1", Self::COLUMNS),
            [id],
            Self::from_row,
        )
        .optional()
    }

    pub fn get(c: &Connection, id: i64) -> Result<User> {
        Self::find(c, id)?.ok_or(Error::not_found("User"))
    }

    pub fn find_by_username(c: &Connection, username: &str) -> Result<Option<User>> {
        c.query_row(
            &format!(
                "SELECT {} FROM users WHERE lower(username) = lower(?1)",
                Self::COLUMNS
            ),
            [username],
            Self::from_row,
        )
        .optional()
    }

    pub fn bot(&self) -> bool {
        self.user_type != user_type::HUMAN
    }

    pub fn active(&self) -> bool {
        self.state == "active"
    }

    pub fn blocked(&self) -> bool {
        self.state == "blocked"
            || self.state == "ldap_blocked"
            || self.state == "blocked_pending_approval"
    }
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub id: i64,
    pub kind: String, // 'User', 'Group', 'Project'
    pub name: String,
    pub path: String,
    pub parent_id: Option<i64>,
    pub owner_id: Option<i64>,
    pub description: Option<String>,
    pub visibility_level: i64,
    pub request_access_enabled: bool,
    pub project_creation_level: Option<i64>,
    pub subgroup_creation_level: Option<i64>,
    pub settings: Map<String, Value>,
    pub created_at: i64,
    pub updated_at: i64,
    /// From `routes`.
    pub full_path: String,
    pub full_name: String,
}

impl Namespace {
    const SELECT: &'static str = "SELECT n.id, n.type, n.name, n.path, n.parent_id, n.owner_id, n.description, n.visibility_level, n.request_access_enabled, n.project_creation_level, n.subgroup_creation_level, n.settings, n.created_at, n.updated_at, r.path, r.name FROM namespaces n JOIN routes r ON r.source_type = 'Namespace' AND r.source_id = n.id";

    fn from_row(r: &Row) -> rusqlite::Result<Namespace> {
        let settings: String = r.get(11)?;
        Ok(Namespace {
            id: r.get(0)?,
            kind: r.get(1)?,
            name: r.get(2)?,
            path: r.get(3)?,
            parent_id: r.get(4)?,
            owner_id: r.get(5)?,
            description: r.get(6)?,
            visibility_level: r.get(7)?,
            request_access_enabled: r.get(8)?,
            project_creation_level: r.get(9)?,
            subgroup_creation_level: r.get(10)?,
            settings: serde_json::from_str(&settings).unwrap_or_default(),
            created_at: r.get(12)?,
            updated_at: r.get(13)?,
            full_path: r.get(14)?,
            full_name: r.get(15)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<Namespace>> {
        c.query_row(
            &format!("{} WHERE n.id = ?1", Self::SELECT),
            [id],
            Self::from_row,
        )
        .optional()
    }

    pub fn find_by_full_path(c: &Connection, path: &str) -> Result<Option<Namespace>> {
        c.query_row(
            &format!("{} WHERE lower(r.path) = lower(?1)", Self::SELECT),
            [path],
            Self::from_row,
        )
        .optional()
    }

    pub fn list_where(
        c: &Connection,
        cond: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<Namespace>> {
        let mut st = c.prepare(&format!("{} WHERE {cond}", Self::SELECT))?;
        let rows = st
            .query_map(args, Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn is_group(&self) -> bool {
        self.kind == "Group"
    }

    pub fn is_user(&self) -> bool {
        self.kind == "User"
    }

    /// GitLab's `kind`: "user" or "group".
    pub fn kind_name(&self) -> &'static str {
        match self.kind.as_str() {
            "User" => "user",
            "Project" => "project",
            _ => "group",
        }
    }

    /// Ids of this namespace and its ancestors, nearest first.
    pub fn self_and_ancestor_ids(c: &Connection, id: i64) -> Result<Vec<i64>> {
        let mut st = c.prepare(
            "WITH RECURSIVE a(id, parent_id, depth) AS (
                 SELECT id, parent_id, 0 FROM namespaces WHERE id = ?1
                 UNION ALL
                 SELECT n.id, n.parent_id, a.depth + 1 FROM namespaces n JOIN a ON n.id = a.parent_id)
             SELECT id FROM a ORDER BY depth",
        )?;
        let ids = st
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    /// Ids of this namespace and all descendant groups.
    pub fn self_and_descendant_ids(c: &Connection, id: i64) -> Result<Vec<i64>> {
        let mut st = c.prepare(
            "WITH RECURSIVE d(id) AS (
                 SELECT id FROM namespaces WHERE id = ?1
                 UNION ALL
                 SELECT n.id FROM namespaces n JOIN d ON n.parent_id = d.id WHERE n.type = 'Group')
             SELECT id FROM d",
        )?;
        let ids = st
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        self.settings
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub namespace_id: i64,
    pub project_namespace_id: i64,
    pub description: Option<String>,
    pub visibility_level: i64,
    pub creator_id: Option<i64>,
    pub default_branch: Option<String>,
    pub archived: bool,
    pub merge_method: String,
    pub squash_option: String,
    pub only_allow_merge_if_pipeline_succeeds: bool,
    pub allow_merge_on_skipped_pipeline: Option<bool>,
    pub only_allow_merge_if_all_discussions_are_resolved: bool,
    pub remove_source_branch_after_merge: bool,
    pub printing_merge_request_link_enabled: bool,
    pub resolve_outdated_diff_discussions: bool,
    pub merge_commit_template: Option<String>,
    pub squash_commit_template: Option<String>,
    pub suggestion_commit_message: Option<String>,
    pub issue_branch_template: Option<String>,
    pub request_access_enabled: bool,
    pub features: Map<String, Value>,
    pub settings: Map<String, Value>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity_at: i64,
    pub full_path: String,
    pub full_name: String,
}

pub mod feature {
    pub const DISABLED: i64 = 0;
    pub const PRIVATE: i64 = 10;
    pub const ENABLED: i64 = 20;
    pub const PUBLIC: i64 = 30;

    pub fn name(level: i64) -> &'static str {
        match level {
            DISABLED => "disabled",
            PRIVATE => "private",
            PUBLIC => "public",
            _ => "enabled",
        }
    }

    pub fn parse(s: &str) -> Option<i64> {
        match s {
            "disabled" => Some(DISABLED),
            "private" => Some(PRIVATE),
            "enabled" => Some(ENABLED),
            "public" => Some(PUBLIC),
            _ => None,
        }
    }

    /// Default level of a project feature on creation.
    pub fn default_level(feature: &str) -> i64 {
        match feature {
            "pages" | "security_and_compliance" => PRIVATE,
            _ => ENABLED,
        }
    }
}

impl Project {
    const SELECT: &'static str = "SELECT p.id, p.name, p.path, p.namespace_id, p.project_namespace_id, p.description, p.visibility_level, p.creator_id, p.default_branch, p.archived, p.merge_method, p.squash_option, p.only_allow_merge_if_pipeline_succeeds, p.allow_merge_on_skipped_pipeline, p.only_allow_merge_if_all_discussions_are_resolved, p.remove_source_branch_after_merge, p.printing_merge_request_link_enabled, p.resolve_outdated_diff_discussions, p.merge_commit_template, p.squash_commit_template, p.suggestion_commit_message, p.issue_branch_template, p.request_access_enabled, p.features, p.settings, p.created_at, p.updated_at, p.last_activity_at, r.path, r.name FROM projects p JOIN routes r ON r.source_type = 'Project' AND r.source_id = p.id";

    fn from_row(r: &Row) -> rusqlite::Result<Project> {
        let features: String = r.get(23)?;
        let settings: String = r.get(24)?;
        Ok(Project {
            id: r.get(0)?,
            name: r.get(1)?,
            path: r.get(2)?,
            namespace_id: r.get(3)?,
            project_namespace_id: r.get(4)?,
            description: r.get(5)?,
            visibility_level: r.get(6)?,
            creator_id: r.get(7)?,
            default_branch: r.get(8)?,
            archived: r.get(9)?,
            merge_method: r.get(10)?,
            squash_option: r.get(11)?,
            only_allow_merge_if_pipeline_succeeds: r.get(12)?,
            allow_merge_on_skipped_pipeline: r.get(13)?,
            only_allow_merge_if_all_discussions_are_resolved: r.get(14)?,
            remove_source_branch_after_merge: r.get(15)?,
            printing_merge_request_link_enabled: r.get(16)?,
            resolve_outdated_diff_discussions: r.get(17)?,
            merge_commit_template: r.get(18)?,
            squash_commit_template: r.get(19)?,
            suggestion_commit_message: r.get(20)?,
            issue_branch_template: r.get(21)?,
            request_access_enabled: r.get(22)?,
            features: serde_json::from_str(&features).unwrap_or_default(),
            settings: serde_json::from_str(&settings).unwrap_or_default(),
            created_at: r.get(25)?,
            updated_at: r.get(26)?,
            last_activity_at: r.get(27)?,
            full_path: r.get(28)?,
            full_name: r.get(29)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<Project>> {
        c.query_row(
            &format!("{} WHERE p.id = ?1", Self::SELECT),
            [id],
            Self::from_row,
        )
        .optional()
    }

    pub fn find_by_full_path(c: &Connection, path: &str) -> Result<Option<Project>> {
        c.query_row(
            &format!("{} WHERE lower(r.path) = lower(?1)", Self::SELECT),
            [path],
            Self::from_row,
        )
        .optional()
    }

    pub fn list_where(
        c: &Connection,
        cond: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<Project>> {
        let mut st = c.prepare(&format!("{} WHERE {cond}", Self::SELECT))?;
        let rows = st
            .query_map(args, Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn feature_level(&self, feature: &str) -> i64 {
        self.features
            .get(feature)
            .and_then(Value::as_i64)
            .unwrap_or_else(|| feature::default_level(feature))
    }

    pub fn setting<'a>(&'a self, key: &str) -> Option<&'a Value> {
        self.settings.get(key)
    }

    pub fn topics(&self, c: &Connection) -> Result<Vec<String>> {
        let mut st = c.prepare("SELECT name FROM topics WHERE project_id = ?1 ORDER BY name")?;
        let v = st
            .query_map([self.id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(v)
    }
}

#[derive(Debug, Clone)]
pub struct Member {
    pub id: i64,
    pub source_type: String,
    pub source_id: i64,
    pub user_id: i64,
    pub access_level: i64,
    pub expires_at: Option<String>,
    pub created_by_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Member {
    pub const COLUMNS: &'static str = "id, source_type, source_id, user_id, access_level, expires_at, created_by_id, created_at, updated_at";

    pub fn from_row(r: &Row) -> rusqlite::Result<Member> {
        Ok(Member {
            id: r.get(0)?,
            source_type: r.get(1)?,
            source_id: r.get(2)?,
            user_id: r.get(3)?,
            access_level: r.get(4)?,
            expires_at: r.get(5)?,
            created_by_id: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
        })
    }

    pub fn find(
        c: &Connection,
        source_type: &str,
        source_id: i64,
        user_id: i64,
    ) -> Result<Option<Member>> {
        c.query_row(
            &format!(
                "SELECT {} FROM members WHERE source_type = ?1 AND source_id = ?2 AND user_id = ?3",
                Self::COLUMNS
            ),
            params![source_type, source_id, user_id],
            Self::from_row,
        )
        .optional()
    }

    pub fn expired(&self) -> bool {
        match &self.expires_at {
            Some(d) => d.as_str() <= crate::time::today().as_str(),
            None => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub scopes: Vec<String>,
    pub impersonation: bool,
    pub revoked: bool,
    pub expires_at: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
}

impl Token {
    pub const COLUMNS: &'static str = "id, user_id, name, description, scopes, impersonation, revoked, expires_at, created_at, updated_at, last_used_at";

    pub fn from_row(r: &Row) -> rusqlite::Result<Token> {
        let scopes: String = r.get(4)?;
        Ok(Token {
            id: r.get(0)?,
            user_id: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            scopes: serde_json::from_str(&scopes).unwrap_or_default(),
            impersonation: r.get(5)?,
            revoked: r.get(6)?,
            expires_at: r.get(7)?,
            created_at: r.get(8)?,
            updated_at: r.get(9)?,
            last_used_at: r.get(10)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<Token>> {
        c.query_row(
            &format!(
                "SELECT {} FROM personal_access_tokens WHERE id = ?1",
                Self::COLUMNS
            ),
            [id],
            Self::from_row,
        )
        .optional()
    }

    pub fn expired(&self) -> bool {
        match &self.expires_at {
            Some(d) => d.as_str() <= crate::time::today().as_str(),
            None => false,
        }
    }

    pub fn active(&self) -> bool {
        !self.revoked && !self.expired()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}
