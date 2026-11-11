use crate::model::{Namespace, Project, User, access, feature, visibility};
use crate::perms::GroupPerms;
use crate::services::{groups::creation_level, paths};
use crate::{Error, Result, time};
use rusqlite::{Connection, params};
use serde_json::{Map, Value};

/// Project feature names stored in `projects.features`.
pub const FEATURES: &[&str] = &[
    "issues",
    "repository",
    "merge_requests",
    "forking",
    "wiki",
    "builds",
    "snippets",
    "pages",
    "analytics",
    "container_registry",
    "security_and_compliance",
    "releases",
    "environments",
    "feature_flags",
    "infrastructure",
    "monitor",
    "model_experiments",
    "model_registry",
    "package_registry",
    "requirements",
];

#[derive(Debug, Default, Clone)]
pub struct ProjectAttrs {
    pub name: Option<String>,
    pub path: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<i64>,
    pub default_branch: Option<String>,
    pub merge_method: Option<String>,
    pub squash_option: Option<String>,
    pub only_allow_merge_if_pipeline_succeeds: Option<bool>,
    pub allow_merge_on_skipped_pipeline: Option<bool>,
    pub only_allow_merge_if_all_discussions_are_resolved: Option<bool>,
    pub remove_source_branch_after_merge: Option<bool>,
    pub printing_merge_request_link_enabled: Option<bool>,
    pub resolve_outdated_diff_discussions: Option<bool>,
    pub merge_commit_template: Option<Option<String>>,
    pub squash_commit_template: Option<Option<String>>,
    pub suggestion_commit_message: Option<Option<String>>,
    pub issue_branch_template: Option<Option<String>>,
    pub request_access_enabled: Option<bool>,
    pub archived: Option<bool>,
    pub topics: Option<Vec<String>>,
    /// Feature access levels by feature name.
    pub features: Vec<(String, i64)>,
    /// Settings labrat only stores and returns.
    pub settings: Vec<(String, Value)>,
}

/// Can `actor` create projects in `ns`?
pub fn can_create_in(c: &Connection, actor: &User, ns: &Namespace) -> Result<bool> {
    if actor.admin {
        return Ok(true);
    }
    if ns.is_user() {
        return Ok(ns.owner_id == Some(actor.id) && actor.projects_limit > 0);
    }
    let perms = GroupPerms::load(c, Some(actor), ns)?;
    let level = ns
        .project_creation_level
        .unwrap_or(creation_level::DEVELOPER);
    Ok(match level {
        creation_level::NO_ONE => false,
        creation_level::MAINTAINER => perms.level >= access::MAINTAINER,
        creation_level::DEVELOPER => perms.level >= access::DEVELOPER,
        creation_level::OWNER => perms.level >= access::OWNER,
        _ => false,
    })
}

/// `Projects::CreateService`.
pub fn create(
    c: &Connection,
    actor: &User,
    ns: &Namespace,
    attrs: ProjectAttrs,
) -> Result<Project> {
    if !can_create_in(c, actor, ns)? {
        return Err(Error::Validation(vec![(
            "namespace".into(),
            "is not valid".into(),
        )]));
    }
    let name = attrs.name.clone().unwrap_or_default();
    let path = attrs.path.clone().unwrap_or_default();
    let mut errs: Vec<(String, String)> = vec![];
    if name.trim().is_empty() {
        errs.push(("name".into(), "can't be blank".into()));
    } else if !valid_project_name(&name) {
        errs.push((
            "name".into(),
            "can contain only letters, digits, emoji, '_', '.', '+', dashes, or spaces. It must start with a letter, digit, emoji, or '_'.".into(),
        ));
    }
    if path.is_empty() {
        errs.push(("path".into(), "can't be blank".into()));
    } else if !paths::valid_project_path(&path) {
        errs.push(("path".into(), paths::PROJECT_PATH_MESSAGE.into()));
    } else if paths::reserved_in_namespace(&path) {
        errs.push(("path".into(), format!("{path} is a reserved name")));
    }
    let full_path = format!("{}/{}", ns.full_path, path);
    if errs.is_empty() {
        let name_taken: i64 = c.query_row(
            "SELECT count(*) FROM projects WHERE namespace_id = ?1 AND lower(name) = lower(?2)",
            params![ns.id, name],
            |r| r.get(0),
        )?;
        if name_taken > 0 {
            errs.push((
                "project_namespace.name".into(),
                "has already been taken".into(),
            ));
            errs.push(("name".into(), "has already been taken".into()));
        }
        if paths::route_taken(c, &full_path)? {
            errs.push(("path".into(), "has already been taken".into()));
        }
    }
    // `Projects::CreateService`: a visibility the group doesn't allow becomes the group's.
    let mut vis = attrs.visibility.unwrap_or(visibility::PRIVATE);
    if ns.is_group() && vis > ns.visibility_level {
        vis = ns.visibility_level;
    }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }

    let now = time::now();
    c.execute(
        "INSERT INTO namespaces (type, name, path, parent_id, owner_id, visibility_level, created_at, updated_at)
         VALUES ('Project', ?1, ?2, ?3, NULL, ?4, ?5, ?5)",
        params![name, path, ns.id, vis, now],
    )?;
    let project_ns = c.last_insert_rowid();
    c.execute(
        "INSERT INTO projects (name, path, namespace_id, project_namespace_id, description, visibility_level, creator_id,
             created_at, updated_at, last_activity_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)",
        params![name, path, ns.id, project_ns, attrs.description, vis, actor.id, now],
    )?;
    let id = c.last_insert_rowid();
    let full_name = format!("{} / {}", ns.full_name, name);
    paths::insert_route(c, "Project", id, &full_path, &full_name)?;
    let project = Project::find(c, id)?.expect("just inserted");
    let rest = ProjectAttrs {
        name: None,
        path: None,
        visibility: None,
        ..attrs
    };
    apply(c, &project, rest)?;
    if !ns.is_group()
        && let Some(owner) = ns.owner_id {
            crate::services::members::add(
                c,
                Some(actor),
                "Project",
                id,
                owner,
                access::OWNER,
                None,
            )?;
        }
    crate::services::events::record(c, Some(id), actor.id, "created", Some("Project"), Some(id), None)?;
    Ok(Project::find(c, id)?.expect("just inserted"))
}

fn valid_project_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    let ok_first = first.is_alphanumeric() || first == '_' || !first.is_ascii();
    ok_first
        && name.chars().all(|ch| {
            ch.is_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-' | ' ') || !ch.is_ascii()
        })
}

/// `Projects::UpdateService` for the attributes labrat models.
pub fn update(c: &Connection, project: &Project, attrs: ProjectAttrs) -> Result<Project> {
    let mut errs: Vec<(String, String)> = vec![];
    if let Some(n) = &attrs.name
        && n.trim().is_empty() {
            errs.push(("name".into(), "can't be blank".into()));
        }
    if let Some(p) = &attrs.path
        && !paths::valid_project_path(p) {
            errs.push(("path".into(), paths::PROJECT_PATH_MESSAGE.into()));
        }
    if let Some(m) = &attrs.merge_method
        && !matches!(m.as_str(), "merge" | "rebase_merge" | "ff") {
            return Err(Error::Param(
                "merge_method does not have a valid value".into(),
            ));
        }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }
    let ns = Namespace::find(c, project.namespace_id)?.ok_or(Error::not_found("Namespace"))?;
    let new_name = attrs.name.clone().unwrap_or(project.name.clone());
    let new_path = attrs.path.clone().unwrap_or(project.path.clone());
    if new_name != project.name || new_path != project.path {
        let new_full = format!("{}/{}", ns.full_path, new_path);
        if new_path != project.path && paths::route_taken(c, &new_full)? {
            return Err(Error::validation("path", "has already been taken"));
        }
        c.execute(
            "UPDATE routes SET path = ?1, name = ?2 WHERE source_type = 'Project' AND source_id = ?3",
            params![new_full, format!("{} / {}", ns.full_name, new_name), project.id],
        )?;
        c.execute(
            "UPDATE projects SET name = ?1, path = ?2 WHERE id = ?3",
            params![new_name, new_path, project.id],
        )?;
        c.execute(
            "UPDATE namespaces SET name = ?1, path = ?2 WHERE id = ?3",
            params![new_name, new_path, project.project_namespace_id],
        )?;
    }
    if let Some(v) = attrs.visibility {
        c.execute(
            "UPDATE projects SET visibility_level = ?1 WHERE id = ?2",
            params![v, project.id],
        )?;
    }
    apply(
        c,
        project,
        ProjectAttrs {
            name: None,
            path: None,
            visibility: None,
            ..attrs
        },
    )?;
    c.execute(
        "UPDATE projects SET updated_at = ?1 WHERE id = ?2",
        params![time::now(), project.id],
    )?;
    Ok(Project::find(c, project.id)?.expect("exists"))
}

/// Applies simple column, feature and setting changes.
fn apply(c: &Connection, project: &Project, a: ProjectAttrs) -> Result<()> {
    let id = project.id;
    let set = |col: &str, v: &dyn rusqlite::ToSql| -> Result<()> {
        c.execute(
            &format!("UPDATE projects SET {col} = ?1 WHERE id = ?2"),
            params![v, id],
        )?;
        Ok(())
    };
    if let Some(v) = &a.description {
        set("description", v)?;
    }
    if let Some(v) = &a.default_branch {
        set("default_branch", v)?;
    }
    if let Some(v) = &a.merge_method {
        set("merge_method", v)?;
    }
    if let Some(v) = &a.squash_option {
        set("squash_option", v)?;
    }
    let bools = [
        (
            "only_allow_merge_if_pipeline_succeeds",
            a.only_allow_merge_if_pipeline_succeeds,
        ),
        (
            "allow_merge_on_skipped_pipeline",
            a.allow_merge_on_skipped_pipeline,
        ),
        (
            "only_allow_merge_if_all_discussions_are_resolved",
            a.only_allow_merge_if_all_discussions_are_resolved,
        ),
        (
            "remove_source_branch_after_merge",
            a.remove_source_branch_after_merge,
        ),
        (
            "printing_merge_request_link_enabled",
            a.printing_merge_request_link_enabled,
        ),
        (
            "resolve_outdated_diff_discussions",
            a.resolve_outdated_diff_discussions,
        ),
        ("request_access_enabled", a.request_access_enabled),
        ("archived", a.archived),
    ];
    for (col, v) in bools {
        if let Some(v) = v {
            set(col, &v)?;
        }
    }
    let texts = [
        ("merge_commit_template", &a.merge_commit_template),
        ("squash_commit_template", &a.squash_commit_template),
        ("suggestion_commit_message", &a.suggestion_commit_message),
        ("issue_branch_template", &a.issue_branch_template),
    ];
    for (col, v) in texts {
        if let Some(v) = v {
            set(col, v)?;
        }
    }
    if let Some(topics) = &a.topics {
        c.execute("DELETE FROM topics WHERE project_id = ?1", [id])?;
        for t in topics {
            let t = t.trim();
            if !t.is_empty() {
                c.execute(
                    "INSERT OR IGNORE INTO topics (project_id, name) VALUES (?1, ?2)",
                    params![id, t],
                )?;
            }
        }
    }
    if !a.features.is_empty() {
        let mut f: Map<String, Value> = project.features.clone();
        for (k, v) in a.features {
            f.insert(k, Value::from(v));
        }
        set("features", &Value::Object(f).to_string())?;
    }
    if !a.settings.is_empty() {
        let mut s = project.settings.clone();
        for (k, v) in a.settings {
            s.insert(k, v);
        }
        set("settings", &Value::Object(s).to_string())?;
    }
    let _ = feature::ENABLED;
    Ok(())
}

pub fn delete_by_id(c: &Connection, id: i64) -> Result<()> {
    let ns: Option<i64> = c
        .query_row(
            "SELECT project_namespace_id FROM projects WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .ok();
    c.execute(
        "DELETE FROM members WHERE source_type = 'Project' AND source_id = ?1",
        [id],
    )?;
    c.execute(
        "DELETE FROM routes WHERE source_type = 'Project' AND source_id = ?1",
        [id],
    )?;
    c.execute("DELETE FROM projects WHERE id = ?1", [id])?;
    if let Some(ns) = ns {
        c.execute("DELETE FROM namespaces WHERE id = ?1", [ns])?;
    }
    Ok(())
}
