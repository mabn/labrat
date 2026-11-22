//! `lib/api/projects.rb`.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::project::{self as project_entity, ProjectCtx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::{get, post};
use labrat_core::model::{Namespace, Project, User, access, feature, visibility};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::projects::{self, FEATURES, ProjectAttrs};
use labrat_core::{App, Error, time};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects", get(list).post(create))
        .route("/projects/user/{user_id}", post(create_for_user))
        .route("/projects/{id}", get(show).put(update).delete(destroy))
        .route("/projects/{id}/archive", post(archive))
        .route("/projects/{id}/unarchive", post(unarchive))
        .route("/users/{user_id}/projects", get(user_projects))
}

/// How a project is being presented.
#[derive(Clone, Copy, PartialEq)]
pub enum View {
    /// In a list (`Entities::Project`).
    List,
    /// `GET /projects/:id` (`ProjectWithAccess`, with `user_can_admin_project`).
    Show,
    /// Create/update responses (`Entities::Project`, with `user_can_admin_project`).
    Single,
}

/// Renders a project the way list and detail endpoints do.
pub fn project_value(
    c: &Connection,
    req: &Req,
    p: &Project,
    perms: &ProjectPerms,
    view: View,
) -> Result<Value, ApiError> {
    let with_access = view == View::Show;
    let ns = Namespace::find(c, p.namespace_id)?.ok_or(Error::not_found("Namespace"))?;
    let owner = match ns.owner_id {
        Some(id) => User::find(c, id)?,
        None => None,
    };
    let ctx = ProjectCtx {
        config: &req.app.config,
        user: req.user.as_ref(),
        single: view != View::List,
    };
    if with_access && req.user.is_some() {
        Ok(project_entity::with_access(
            c,
            p,
            &ns,
            owner.as_ref(),
            perms,
            &ctx,
        )?)
    } else {
        Ok(Value::Object(project_entity::project(
            c,
            p,
            &ns,
            owner.as_ref(),
            perms,
            &ctx,
        )?))
    }
}

/// `BasicProjectSimple`-like shape used with `simple=true`.
fn simple_value(
    c: &Connection,
    req: &Req,
    p: &Project,
    perms: &ProjectPerms,
) -> Result<Value, ApiError> {
    let ns = Namespace::find(c, p.namespace_id)?.ok_or(Error::not_found("Namespace"))?;
    let ctx = ProjectCtx {
        config: &req.app.config,
        user: req.user.as_ref(),
        single: false,
    };
    Ok(Value::Object(project_entity::basic_details(
        c, p, &ns, perms, &ctx,
    )?))
}

const LIST_DEFAULTS: &[(&str, &str)] = &[
    ("imported", "false"),
    ("membership", "false"),
    ("order_by", "created_at"),
    ("owned", "false"),
    ("simple", "false"),
    ("sort", "desc"),
    ("starred", "false"),
    ("statistics", "false"),
    ("with_custom_attributes", "false"),
    ("with_issues_enabled", "false"),
    ("with_merge_requests_enabled", "false"),
];

/// Shared project listing with GitLab's filters. `namespaces` restricts to those namespaces.
pub async fn list_projects(
    req: Req,
    namespaces: Option<Vec<i64>>,
    extra_defaults: &[(&str, &str)],
) -> ApiResult {
    let p = &req.params;
    let user = req.maybe_user()?.cloned();
    let order_by = p
        .one_of(
            "order_by",
            &[
                "id",
                "name",
                "path",
                "created_at",
                "updated_at",
                "last_activity_at",
                "similarity",
                "star_count",
            ],
        )?
        .unwrap_or("created_at".into());
    let desc = p.one_of("sort", &["asc", "desc"])?.as_deref() != Some("asc");
    let membership = p.bool_or("membership", false)?;
    let owned = p.bool_or("owned", false)?;
    let simple = p.bool_or("simple", false)?;
    let search = p.str("search").map(|s| s.to_lowercase());
    let vis_filter = match p.one_of("visibility", &["private", "internal", "public"])? {
        Some(v) => visibility::parse(&v),
        None => None,
    };
    let archived = p.bool("archived")?;
    let min_access = p.int("min_access_level")?;
    let with_mrs = p.bool_or("with_merge_requests_enabled", false)?;
    let topic = p.list("topic");
    let id_after = p.int("id_after")?;
    let id_before = p.int("id_before")?;
    let app = req.app.clone();
    app.db.read(|c| {
        let all = match &namespaces {
            Some(ids) => {
                let list = ids
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                Project::list_where(c, &format!("p.namespace_id IN ({list})"), [])?
            }
            None => Project::list_where(c, "1 = 1", [])?,
        };
        let mut rows: Vec<(Project, ProjectPerms)> = vec![];
        for proj in all {
            if proj
                .setting("marked_for_deletion_at")
                .is_some_and(|v| !v.is_null())
                && false
            {
                continue;
            }
            let perms = ProjectPerms::load(c, user.as_ref(), &proj)?;
            if !perms.read_project() {
                continue;
            }
            if (membership || owned || min_access.is_some()) && perms.level == 0 {
                continue;
            }
            if owned && perms.level < access::OWNER {
                continue;
            }
            if let Some(m) = min_access
                && perms.level < m {
                    continue;
                }
            if let Some(s) = &search
                && !proj.name.to_lowercase().contains(s)
                    && !proj.path.to_lowercase().contains(s)
                    && !proj
                        .description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(s)
                {
                    continue;
                }
            if let Some(v) = vis_filter
                && proj.visibility_level != v {
                    continue;
                }
            if let Some(a) = archived
                && proj.archived != a {
                    continue;
                }
            if with_mrs && proj.feature_level("merge_requests") == feature::DISABLED {
                continue;
            }
            if let Some(t) = &topic {
                let topics = proj.topics(c)?;
                if !t
                    .iter()
                    .all(|x| topics.iter().any(|y| y.eq_ignore_ascii_case(x)))
                {
                    continue;
                }
            }
            if id_after.is_some_and(|a| proj.id <= a) || id_before.is_some_and(|b| proj.id >= b) {
                continue;
            }
            rows.push((proj, perms));
        }
        rows.sort_by(|(a, _), (b, _)| {
            let o = match order_by.as_str() {
                "name" => a.name.cmp(&b.name),
                "path" => a.path.cmp(&b.path),
                "updated_at" => a.updated_at.cmp(&b.updated_at),
                "last_activity_at" => a.last_activity_at.cmp(&b.last_activity_at),
                _ => a.id.cmp(&b.id),
            }
            .then(a.id.cmp(&b.id));
            if desc { o.reverse() } else { o }
        });
        let defaults: Vec<(&str, &str)> = if extra_defaults.is_empty() {
            LIST_DEFAULTS.to_vec()
        } else {
            extra_defaults.to_vec()
        };
        paginate(&req, rows, &defaults, |(proj, perms)| {
            if simple {
                simple_value(c, &req, proj, perms)
            } else {
                project_value(c, &req, proj, perms, View::List)
            }
        })
    })
}

async fn list(req: Req) -> ApiResult {
    list_projects(req, None, &[]).await
}

async fn user_projects(req: Req) -> ApiResult {
    let uid = req.path_param("user_id").to_string();
    let app = req.app.clone();
    let ns = app.db.read(|c| {
        let u = find::user_by_ref(c, &uid)?.ok_or(Error::not_found("User"))?;
        Ok::<_, ApiError>(u.namespace_id.unwrap_or(-1))
    })?;
    list_projects(req, Some(vec![ns]), &[]).await
}

async fn show(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, perms) = find::project(c, &req, &id)?;
        project_value(c, &req, &p, &perms, View::Show)
    })?;
    Ok(Resp::ok(body))
}

/// Reads the project attributes shared by create and update.
fn attrs_from_params(req: &Req) -> Result<ProjectAttrs, ApiError> {
    let p = &req.params;
    let mut a = ProjectAttrs {
        name: p.str("name"),
        path: p.str("path"),
        description: p.str("description"),
        default_branch: p.str("default_branch"),
        merge_method: p.one_of("merge_method", &["ff", "rebase_merge", "merge"])?,
        squash_option: p.one_of(
            "squash_option",
            &["never", "always", "default_on", "default_off"],
        )?,
        only_allow_merge_if_pipeline_succeeds: p.bool("only_allow_merge_if_pipeline_succeeds")?,
        allow_merge_on_skipped_pipeline: p.bool("allow_merge_on_skipped_pipeline")?,
        only_allow_merge_if_all_discussions_are_resolved: p
            .bool("only_allow_merge_if_all_discussions_are_resolved")?,
        remove_source_branch_after_merge: p.bool("remove_source_branch_after_merge")?,
        printing_merge_request_link_enabled: p.bool("printing_merge_request_link_enabled")?,
        resolve_outdated_diff_discussions: p.bool("resolve_outdated_diff_discussions")?,
        merge_commit_template: p.nullable_str("merge_commit_template"),
        squash_commit_template: p.nullable_str("squash_commit_template"),
        suggestion_commit_message: p.nullable_str("suggestion_commit_message"),
        issue_branch_template: p.nullable_str("issue_branch_template"),
        request_access_enabled: p.bool("request_access_enabled")?,
        topics: p.list("topics").or_else(|| p.list("tag_list")),
        ..Default::default()
    };
    if let Some(v) = p.one_of("visibility", &["private", "internal", "public"])? {
        a.visibility = visibility::parse(&v);
    }
    for f in FEATURES {
        let key = format!("{f}_access_level");
        if let Some(v) = p.one_of(&key, &["disabled", "private", "enabled", "public"])? {
            a.features
                .push((f.to_string(), feature::parse(&v).unwrap()));
        }
    }
    // Legacy boolean toggles.
    for (param, f) in [
        ("issues_enabled", "issues"),
        ("merge_requests_enabled", "merge_requests"),
        ("wiki_enabled", "wiki"),
        ("jobs_enabled", "builds"),
        ("snippets_enabled", "snippets"),
        ("container_registry_enabled", "container_registry"),
    ] {
        if let Some(b) = p.bool(param)?
            && !a.features.iter().any(|(k, _)| k == f) {
                a.features.push((
                    f.to_string(),
                    if b {
                        feature::ENABLED
                    } else {
                        feature::DISABLED
                    },
                ));
            }
    }
    for key in [
        "packages_enabled",
        "lfs_enabled",
        "shared_runners_enabled",
        "group_runners_enabled",
        "public_jobs",
        "public_builds",
        "warn_about_potentially_unwanted_characters",
        "enforce_auth_checks_on_uploads",
        "autoclose_referenced_issues",
        "keep_latest_artifact",
        "ci_forward_deployment_enabled",
        "ci_forward_deployment_rollback_allowed",
        "ci_separated_caches",
        "restrict_user_defined_variables",
        "auto_devops_enabled",
        "protect_merge_request_pipelines",
        "ci_display_pipeline_variables",
        "ci_push_repository_for_job_token_allowed",
        "ci_allow_fork_pipelines_to_run_in_parent_project",
    ] {
        if let Some(b) = p.bool(key)? {
            let k = if key == "public_builds" {
                "public_jobs"
            } else {
                key
            };
            a.settings.push((k.to_string(), Value::Bool(b)));
        }
    }
    for key in [
        "ci_default_git_depth",
        "build_timeout",
        "ci_delete_pipelines_in_seconds",
        "max_artifacts_size",
    ] {
        if let Some(n) = p.int(key)? {
            a.settings.push((key.to_string(), json!(n)));
        }
    }
    for key in [
        "ci_config_path",
        "build_git_strategy",
        "auto_cancel_pending_pipelines",
        "auto_devops_deploy_strategy",
        "resource_group_default_process_mode",
        "ci_pipeline_variables_minimum_override_role",
    ] {
        if let Some(s) = p.str(key) {
            a.settings.push((key.to_string(), json!(s)));
        }
    }
    if req.app.config.is_ee() {
        // EE's project params: merge settings, templates and the approval settings that the
        // approvals endpoints also change.
        for key in [
            "merge_pipelines_enabled",
            "merge_trains_enabled",
            "merge_trains_skip_train_allowed",
            "only_allow_merge_if_all_status_checks_passed",
            "reset_approvals_on_push",
            "merge_requests_author_approval",
            "merge_requests_disable_committers_approval",
            "disable_overriding_approvers_per_merge_request",
        ] {
            if let Some(b) = p.bool(key)? {
                a.settings.push((key.to_string(), Value::Bool(b)));
            }
        }
        if let Some(n) = p.int("approvals_before_merge")? {
            a.settings.push(("approvals_before_merge".to_string(), json!(n)));
        }
        for key in ["merge_requests_template", "issues_template", "merge_request_title_regex", "merge_request_title_regex_description"] {
            if let Some(v) = p.nullable_str(key) {
                a.settings.push((key.to_string(), json!(v)));
            }
        }
    }
    Ok(a)
}

fn create_in(
    c: &Connection,
    req: &Req,
    actor: &User,
    ns: &Namespace,
    mut attrs: ProjectAttrs,
) -> Result<Value, ApiError> {
    if attrs.name.is_none() {
        attrs.name = attrs.path.clone();
    }
    if attrs.path.is_none() {
        attrs.path = attrs
            .name
            .as_deref()
            .map(labrat_core::services::paths::path_from_name);
    }
    let initialize_with_readme = req.params.bool_or("initialize_with_readme", false)?;
    let default_branch = attrs.default_branch.take();
    let p = projects::create(c, actor, ns, attrs)?;
    if initialize_with_readme {
        crate::routes::repo_init::initialize_with_readme(
            c,
            &req.app.config,
            &p,
            actor,
            default_branch.as_deref().unwrap_or("main"),
        )?;
    }
    let p = Project::find(c, p.id)?.expect("created");
    let perms = ProjectPerms::load(c, Some(actor), &p)?;
    project_value(c, req, &p, &perms, View::Single)
}

async fn create(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    req.params.require_any(&["name", "path"])?;
    let attrs = attrs_from_params(&req)?;
    let ns_id = req.params.int("namespace_id")?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let ns = match ns_id {
            Some(id) => Namespace::find(c, id)?.filter(|n| n.kind != "Project").ok_or(Error::not_found("Namespace"))?,
            None => Namespace::find(c, me.namespace_id.unwrap_or(-1))?.ok_or(Error::not_found("Namespace"))?,
        };
        if ns.is_user() && ns.owner_id == Some(me.id) && me.projects_limit == 0 && !me.admin {
            return Err(ApiError {
                status: 403,
                body: json!({"error": ["You've reached your limit of 0 projects created. Contact your GitLab administrator."]}),
                headers: vec![],
            });
        }
        create_in(c, &req, &me, &ns, attrs)
    })?;
    Ok(Resp::created(body))
}

async fn create_for_user(req: Req) -> ApiResult {
    req.admin()?;
    req.params.require(&["name"])?;
    let uid = req.path_param("user_id").to_string();
    let attrs = attrs_from_params(&req)?;
    let ns_id = req.params.int("namespace_id")?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let user = find::user_by_id(c, &uid)?;
        let ns = match ns_id {
            Some(id) => Namespace::find(c, id)?.ok_or(Error::not_found("Namespace"))?,
            None => Namespace::find(c, user.namespace_id.unwrap_or(-1))?
                .ok_or(Error::not_found("Namespace"))?,
        };
        create_in(c, &req, &user, &ns, attrs)
    })?;
    Ok(Resp::created(body))
}

async fn update(req: Req) -> ApiResult {
    req.user()?;
    let id = req.path_param("id").to_string();
    let attrs = attrs_from_params(&req)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, &id)?;
        if !perms.admin_project() {
            return Err(Error::forbidden().into());
        }
        if attrs.visibility.is_some()
            && attrs.visibility != Some(p.visibility_level)
            && !perms.owner()
        {
            return Err(Error::forbidden().into());
        }
        if let Some(b) = &attrs.default_branch {
            let exists = crate::routes::repo_init::branch_exists(c, &p, b)?;
            if !exists {
                return Err(Error::message(400, "Could not set the default branch").into());
            }
            // EE: a policy's `block_branch_modification` keeps the default branch; the update
            // is dropped and answered with `null`.
            if req.app.config.is_ee()
                && p.default_branch.as_deref() != Some(b.as_str())
                && let Some(cur) = &p.default_branch
                && labrat_core::services::policies::modification_blocked(c, &p, cur)?
            {
                return Ok(Value::Null);
            }
        }
        let p = projects::update(c, &p, attrs)?;
        let perms = ProjectPerms::load(c, req.user.as_ref(), &p)?;
        project_value(c, &req, &p, &perms, View::Single)
    })?;
    Ok(Resp::ok(body))
}

async fn destroy(req: Req) -> ApiResult {
    req.user()?;
    let id = req.path_param("id").to_string();
    let permanently = req.params.bool_or("permanently_remove", false)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, &id)?;
        if !perms.owner() {
            return Err(Error::forbidden().into());
        }
        let marked = p
            .setting("marked_for_deletion_at")
            .is_some_and(|v| !v.is_null());
        if permanently {
            if !marked {
                return Err(
                    Error::message(400, "Project must be marked for deletion first.").into(),
                );
            }
            if req.params.str("full_path").as_deref() != Some(p.full_path.as_str()) {
                return Err(Error::message(
                    400,
                    "`full_path` is incorrect. You must enter the complete path for the project.",
                )
                .into());
            }
            projects::delete_by_id(c, p.id)?;
            return Ok(());
        }
        if marked {
            return Err(Error::message(400, "Project has already been marked for deletion").into());
        }
        let suffix = format!("-deletion_scheduled-{}", p.id);
        projects::update(
            c,
            &p,
            ProjectAttrs {
                name: Some(format!("{}{suffix}", p.name)),
                path: Some(format!("{}{suffix}", p.path)),
                settings: vec![("marked_for_deletion_at".into(), json!(time::today()))],
                ..Default::default()
            },
        )?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::status(202, json!({"message": "202 Accepted"})))
}

async fn set_archived(req: Req, archived: bool) -> ApiResult {
    req.user()?;
    let id = req.path_param("id").to_string();
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, &id)?;
        if !perms.admin_project() {
            return Err(Error::forbidden().into());
        }
        let p = projects::update(
            c,
            &p,
            ProjectAttrs {
                archived: Some(archived),
                ..Default::default()
            },
        )?;
        project_value(c, &req, &p, &perms, View::List)
    })?;
    Ok(Resp::created(body))
}

async fn archive(req: Req) -> ApiResult {
    set_archived(req, true).await
}

async fn unarchive(req: Req) -> ApiResult {
    set_archived(req, false).await
}
