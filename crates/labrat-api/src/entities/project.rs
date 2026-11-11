//! `API::Entities::Project`, `BasicProjectDetails`, `ProjectWithAccess`, `NamespaceBasic`.

use labrat_core::model::{Namespace, Project, User, feature, visibility};
use labrat_core::perms::ProjectPerms;
use labrat_core::{Config, time};
use rusqlite::Connection;
use serde_json::{Map, Value, json};

pub struct ProjectCtx<'a> {
    pub config: &'a Config,
    pub user: Option<&'a User>,
    /// Rendering a single project (GET/PUT), where GitLab passes `user_can_admin_project`.
    pub single: bool,
}

fn opt_str(v: &Option<String>) -> Value {
    json!(v)
}

/// `NamespaceBasic`.
pub fn namespace_basic(ns: &Namespace, config: &Config) -> Value {
    let web_url = if ns.is_user() {
        config.url(&format!("/{}", ns.path))
    } else {
        config.url(&format!("/groups/{}", ns.full_path))
    };
    json!({
        "id": ns.id,
        "name": ns.name,
        "path": ns.path,
        "kind": ns.kind_name(),
        "full_path": ns.full_path,
        "parent_id": ns.parent_id,
        "avatar_url": null,
        "web_url": web_url,
    })
}

/// `ProjectIdentity`.
fn identity(p: &Project, m: &mut Map<String, Value>) {
    m.insert("id".into(), json!(p.id));
    m.insert("description".into(), opt_str(&p.description));
    m.insert("name".into(), json!(p.name));
    m.insert("name_with_namespace".into(), json!(p.full_name));
    m.insert("path".into(), json!(p.path));
    m.insert("path_with_namespace".into(), json!(p.full_path));
    m.insert("created_at".into(), json!(time::iso(p.created_at)));
}

/// `BasicProjectDetails` (the shape of project lists with `simple=false` for non-members).
pub fn basic_details(
    c: &Connection,
    p: &Project,
    ns: &Namespace,
    perms: &ProjectPerms,
    ctx: &ProjectCtx,
) -> labrat_core::Result<Map<String, Value>> {
    let mut m = Map::new();
    identity(p, &mut m);
    let read_code = perms.read_code(p);
    if read_code {
        m.insert(
            "default_branch".into(),
            json!(p.default_branch.clone().unwrap_or("main".into())),
        );
    }
    let topics = p.topics(c)?;
    m.insert("tag_list".into(), json!(topics));
    m.insert("topics".into(), json!(topics));
    m.insert(
        "ssh_url_to_repo".into(),
        json!(ctx.config.ssh_url(&p.full_path)),
    );
    m.insert(
        "http_url_to_repo".into(),
        json!(ctx.config.url(&format!("/{}.git", p.full_path))),
    );
    m.insert(
        "web_url".into(),
        json!(ctx.config.url(&format!("/{}", p.full_path))),
    );
    if read_code {
        let readme = readme_url(c, p, ctx.config)?;
        m.insert("readme_url".into(), readme);
        m.insert("forks_count".into(), json!(0));
    }
    m.insert("avatar_url".into(), Value::Null);
    m.insert("star_count".into(), json!(0));
    m.insert(
        "last_activity_at".into(),
        json!(time::iso(p.last_activity_at)),
    );
    m.insert(
        "visibility".into(),
        json!(visibility::name(p.visibility_level)),
    );
    m.insert("namespace".into(), namespace_basic(ns, ctx.config));
    if perms.admin {
        m.insert("repository_storage".into(), json!("default"));
    }
    Ok(m)
}

fn readme_url(c: &Connection, p: &Project, config: &Config) -> labrat_core::Result<Value> {
    let Some(branch) = &p.default_branch else {
        return Ok(Value::Null);
    };
    match crate::entities::repo_hooks::readme_path(c, p, branch)? {
        Some(path) => Ok(json!(
            config.url(&format!("/{}/-/blob/{}/{}", p.full_path, branch, path))
        )),
        None => Ok(Value::Null),
    }
}

fn setting_bool(p: &Project, key: &str, default: bool) -> Value {
    json!(p.setting(key).and_then(Value::as_bool).unwrap_or(default))
}

fn setting(p: &Project, key: &str, default: Value) -> Value {
    p.setting(key).cloned().unwrap_or(default)
}

/// `API::Entities::Project`.
pub fn project(
    c: &Connection,
    p: &Project,
    ns: &Namespace,
    owner: Option<&User>,
    perms: &ProjectPerms,
    ctx: &ProjectCtx,
) -> labrat_core::Result<Map<String, Value>> {
    let mut m = basic_details(c, p, ns, perms, ctx)?;
    let api = |path: &str| {
        ctx.config
            .url(&format!("/api/v4/projects/{}{}", p.id, path))
    };
    let mut links = Map::new();
    links.insert("self".into(), json!(api("")));
    if perms.feature(p, "issues", labrat_core::model::access::GUEST) {
        links.insert("issues".into(), json!(api("/issues")));
    }
    if perms.read_merge_request(p) {
        links.insert("merge_requests".into(), json!(api("/merge_requests")));
    }
    links.insert("repo_branches".into(), json!(api("/repository/branches")));
    links.insert("labels".into(), json!(api("/labels")));
    links.insert("events".into(), json!(api("/events")));
    links.insert("members".into(), json!(api("/members")));
    links.insert("cluster_agents".into(), json!(api("/cluster_agents")));
    m.insert("_links".into(), Value::Object(links));
    let marked = p
        .setting("marked_for_deletion_at")
        .cloned()
        .unwrap_or(Value::Null);
    m.insert("marked_for_deletion_at".into(), marked.clone());
    m.insert("marked_for_deletion_on".into(), marked.clone());
    m.insert(
        "packages_enabled".into(),
        setting_bool(p, "packages_enabled", true),
    );
    m.insert(
        "empty_repo".into(),
        json!(crate::entities::repo_hooks::empty_repo(c, p)?),
    );
    m.insert("archived".into(), json!(p.archived));
    if !ns.is_group()
        && let Some(o) = owner {
            m.insert(
                "owner".into(),
                crate::entities::user::basic_value(
                    o,
                    &crate::entities::user::Ctx {
                        base_url: &ctx.config.external_url,
                        current_user: ctx.user,
                    },
                ),
            );
        }
    m.insert(
        "resolve_outdated_diff_discussions".into(),
        json!(p.resolve_outdated_diff_discussions),
    );
    m.insert(
        "container_expiration_policy".into(),
        json!({
            "cadence": "1d",
            "enabled": false,
            "keep_n": 10,
            "older_than": "90d",
            "name_regex": ".*",
            "name_regex_keep": null,
            "next_run_at": time::iso(p.created_at + 86_400_000 + 29),
        }),
    );
    m.insert("repository_object_format".into(), json!("sha1"));
    let enabled = |f: &str| json!(perms.feature(p, f, labrat_core::model::access::REPORTER));
    m.insert("issues_enabled".into(), enabled("issues"));
    m.insert("merge_requests_enabled".into(), enabled("merge_requests"));
    m.insert("wiki_enabled".into(), enabled("wiki"));
    m.insert("jobs_enabled".into(), enabled("builds"));
    m.insert("snippets_enabled".into(), enabled("snippets"));
    m.insert(
        "container_registry_enabled".into(),
        enabled("container_registry"),
    );
    m.insert("service_desk_enabled".into(), json!(false));
    // `admin_issue` (needed for the service desk address) is read-only in archived projects.
    if perms.reporter() && marked.is_null() && !p.archived {
        m.insert("service_desk_address".into(), Value::Null);
    }
    m.insert(
        "can_create_merge_request_in".into(),
        json!(perms.create_merge_request_in(p) && marked.is_null() && !p.archived),
    );
    for f in [
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
    ] {
        m.insert(
            format!("{f}_access_level"),
            json!(feature::name(p.feature_level(f))),
        );
    }
    m.insert("emails_disabled".into(), json!(false));
    m.insert("emails_enabled".into(), json!(true));
    m.insert("show_diff_preview_in_email".into(), json!(true));
    m.insert(
        "shared_runners_enabled".into(),
        setting_bool(p, "shared_runners_enabled", true),
    );
    m.insert("lfs_enabled".into(), setting_bool(p, "lfs_enabled", true));
    m.insert("creator_id".into(), json!(p.creator_id));
    let admin = perms.admin_project();
    if admin {
        m.insert("import_url".into(), Value::Null);
        m.insert("import_type".into(), Value::Null);
    }
    m.insert("import_status".into(), json!("none"));
    if admin && ctx.single {
        m.insert("import_error".into(), Value::Null);
    }
    if perms.feature(p, "issues", labrat_core::model::access::GUEST) {
        m.insert("open_issues_count".into(), json!(0));
    }
    m.insert(
        "description_html".into(),
        json!(crate::markdown::render(
            p.description.as_deref().unwrap_or("")
        )),
    );
    m.insert("updated_at".into(), json!(time::iso(p.updated_at)));
    if admin {
        m.insert(
            "ci_default_git_depth".into(),
            setting(p, "ci_default_git_depth", json!(20)),
        );
        m.insert(
            "ci_delete_pipelines_in_seconds".into(),
            setting(p, "ci_delete_pipelines_in_seconds", Value::Null),
        );
        m.insert(
            "ci_forward_deployment_enabled".into(),
            setting_bool(p, "ci_forward_deployment_enabled", true),
        );
        m.insert(
            "ci_forward_deployment_rollback_allowed".into(),
            setting_bool(p, "ci_forward_deployment_rollback_allowed", true),
        );
        m.insert(
            "ci_job_token_scope_enabled".into(),
            setting_bool(p, "ci_job_token_scope_enabled", false),
        );
        m.insert(
            "ci_separated_caches".into(),
            setting_bool(p, "ci_separated_caches", true),
        );
        m.insert(
            "ci_allow_fork_pipelines_to_run_in_parent_project".into(),
            setting_bool(p, "ci_allow_fork_pipelines_to_run_in_parent_project", true),
        );
        m.insert(
            "ci_id_token_sub_claim_components".into(),
            setting(
                p,
                "ci_id_token_sub_claim_components",
                json!(["project_path", "ref_type", "ref"]),
            ),
        );
        m.insert(
            "build_git_strategy".into(),
            setting(p, "build_git_strategy", json!("fetch")),
        );
        m.insert(
            "keep_latest_artifact".into(),
            setting_bool(p, "keep_latest_artifact", true),
        );
        m.insert(
            "restrict_user_defined_variables".into(),
            setting_bool(p, "restrict_user_defined_variables", false),
        );
        m.insert(
            "ci_pipeline_variables_minimum_override_role".into(),
            setting(
                p,
                "ci_pipeline_variables_minimum_override_role",
                json!("developer"),
            ),
        );
        m.insert(
            "runner_token_expiration_interval".into(),
            setting(p, "runner_token_expiration_interval", Value::Null),
        );
        m.insert(
            "group_runners_enabled".into(),
            setting_bool(p, "group_runners_enabled", true),
        );
        m.insert(
            "resource_group_default_process_mode".into(),
            setting(p, "resource_group_default_process_mode", json!("unordered")),
        );
        m.insert(
            "auto_cancel_pending_pipelines".into(),
            setting(p, "auto_cancel_pending_pipelines", json!("enabled")),
        );
        m.insert(
            "build_timeout".into(),
            setting(p, "build_timeout", json!(3600)),
        );
        m.insert(
            "auto_devops_enabled".into(),
            setting_bool(p, "auto_devops_enabled", true),
        );
        m.insert(
            "auto_devops_deploy_strategy".into(),
            setting(p, "auto_devops_deploy_strategy", json!("continuous")),
        );
        m.insert(
            "ci_push_repository_for_job_token_allowed".into(),
            setting_bool(p, "ci_push_repository_for_job_token_allowed", false),
        );
        m.insert(
            "protect_merge_request_pipelines".into(),
            setting_bool(p, "protect_merge_request_pipelines", true),
        );
        m.insert(
            "ci_display_pipeline_variables".into(),
            setting_bool(p, "ci_display_pipeline_variables", false),
        );
        m.insert("runners_token".into(), json!(runners_token(p)));
    }
    if read_code_value(perms, p) {
        m.insert(
            "ci_config_path".into(),
            setting(p, "ci_config_path", Value::Null),
        );
    }
    m.insert("public_jobs".into(), setting_bool(p, "public_jobs", true));
    m.insert("shared_with_groups".into(), json!([]));
    m.insert(
        "only_allow_merge_if_pipeline_succeeds".into(),
        json!(p.only_allow_merge_if_pipeline_succeeds),
    );
    m.insert(
        "allow_merge_on_skipped_pipeline".into(),
        json!(p.allow_merge_on_skipped_pipeline),
    );
    m.insert(
        "request_access_enabled".into(),
        json!(p.request_access_enabled),
    );
    m.insert(
        "only_allow_merge_if_all_discussions_are_resolved".into(),
        json!(p.only_allow_merge_if_all_discussions_are_resolved),
    );
    m.insert(
        "remove_source_branch_after_merge".into(),
        json!(p.remove_source_branch_after_merge),
    );
    m.insert(
        "printing_merge_request_link_enabled".into(),
        json!(p.printing_merge_request_link_enabled),
    );
    m.insert("merge_method".into(), json!(p.merge_method));
    m.insert("squash_option".into(), json!(p.squash_option));
    m.insert(
        "enforce_auth_checks_on_uploads".into(),
        setting_bool(p, "enforce_auth_checks_on_uploads", true),
    );
    m.insert(
        "suggestion_commit_message".into(),
        json!(p.suggestion_commit_message),
    );
    m.insert(
        "merge_commit_template".into(),
        json!(p.merge_commit_template),
    );
    m.insert(
        "squash_commit_template".into(),
        json!(p.squash_commit_template),
    );
    m.insert(
        "issue_branch_template".into(),
        json!(p.issue_branch_template),
    );
    m.insert(
        "warn_about_potentially_unwanted_characters".into(),
        setting_bool(p, "warn_about_potentially_unwanted_characters", true),
    );
    m.insert(
        "autoclose_referenced_issues".into(),
        setting_bool(p, "autoclose_referenced_issues", true),
    );
    m.insert(
        "max_artifacts_size".into(),
        setting(p, "max_artifacts_size", Value::Null),
    );
    Ok(m)
}

fn read_code_value(perms: &ProjectPerms, p: &Project) -> bool {
    perms.read_code(p)
}

/// A stable fake runner registration token per project.
fn runners_token(p: &Project) -> String {
    use sha2::{Digest, Sha256};
    let h = hex::encode(Sha256::digest(format!("labrat-runners-{}", p.id).as_bytes()));
    format!("GR1348941{}", &h[..20])
}

/// `ProjectWithAccess`: adds the `permissions` block.
pub fn with_access(
    c: &Connection,
    p: &Project,
    ns: &Namespace,
    owner: Option<&User>,
    perms: &ProjectPerms,
    ctx: &ProjectCtx,
) -> labrat_core::Result<Value> {
    let mut m = project(c, p, ns, owner, perms, ctx)?;
    let mut permissions = Map::new();
    let today = time::today();
    let project_access = match ctx.user {
        Some(u) => c
            .query_row(
                "SELECT access_level FROM members WHERE source_type = 'Project' AND source_id = ?1 AND user_id = ?2
                 AND (expires_at IS NULL OR expires_at > ?3)",
                rusqlite::params![p.id, u.id, today],
                |r| r.get::<_, i64>(0),
            )
            .ok(),
        None => None,
    };
    permissions.insert(
        "project_access".into(),
        match project_access {
            Some(l) => json!({"access_level": l, "notification_level": 3}),
            None => Value::Null,
        },
    );
    let group_access = match (ctx.user, ns.is_group()) {
        (Some(u), true) => {
            let level = labrat_core::perms::group_access(c, u.id, ns.id)?;
            if level > 0 {
                json!({"access_level": level, "notification_level": 3})
            } else {
                Value::Null
            }
        }
        _ => Value::Null,
    };
    permissions.insert("group_access".into(), group_access);
    m.insert("permissions".into(), Value::Object(permissions));
    Ok(Value::Object(m))
}
