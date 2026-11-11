//! `API::Entities::Group` and `GroupDetail`.

use labrat_core::model::{Namespace, visibility};
use labrat_core::perms::GroupPerms;
use labrat_core::services::groups::creation_level;
use labrat_core::{Config, time};
use serde_json::{Map, Value, json};

pub fn group(g: &Namespace, config: &Config) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(g.id));
    m.insert(
        "web_url".into(),
        json!(config.url(&format!("/groups/{}", g.full_path))),
    );
    m.insert("name".into(), json!(g.name));
    m.insert("path".into(), json!(g.path));
    m.insert("description".into(), json!(g.description));
    m.insert(
        "visibility".into(),
        json!(visibility::name(g.visibility_level)),
    );
    m.insert(
        "share_with_group_lock".into(),
        json!(g.setting_bool("share_with_group_lock", false)),
    );
    m.insert(
        "require_two_factor_authentication".into(),
        json!(g.setting_bool("require_two_factor_authentication", false)),
    );
    m.insert(
        "two_factor_grace_period".into(),
        g.settings
            .get("two_factor_grace_period")
            .cloned()
            .unwrap_or(json!(48)),
    );
    m.insert(
        "project_creation_level".into(),
        json!(creation_level::project_name(
            g.project_creation_level
                .unwrap_or(creation_level::DEVELOPER)
        )),
    );
    m.insert(
        "auto_devops_enabled".into(),
        g.settings
            .get("auto_devops_enabled")
            .cloned()
            .unwrap_or(Value::Null),
    );
    m.insert(
        "subgroup_creation_level".into(),
        json!(creation_level::subgroup_name(
            g.subgroup_creation_level.unwrap_or(1)
        )),
    );
    m.insert("emails_disabled".into(), json!(false));
    m.insert("emails_enabled".into(), json!(true));
    m.insert("show_diff_preview_in_email".into(), json!(true));
    m.insert(
        "mentions_disabled".into(),
        g.settings
            .get("mentions_disabled")
            .cloned()
            .unwrap_or(Value::Null),
    );
    m.insert(
        "lfs_enabled".into(),
        json!(g.setting_bool("lfs_enabled", true)),
    );
    m.insert("archived".into(), json!(false));
    m.insert("math_rendering_limits_enabled".into(), json!(true));
    m.insert("lock_math_rendering_limits_enabled".into(), json!(false));
    m.insert(
        "default_branch".into(),
        g.settings
            .get("default_branch")
            .cloned()
            .unwrap_or(Value::Null),
    );
    m.insert("default_branch_protection".into(), json!(2));
    m.insert(
        "default_branch_protection_defaults".into(),
        json!({
            "allowed_to_push": [{"access_level": 40}],
            "allow_force_push": false,
            "allowed_to_merge": [{"access_level": 40}],
            "developer_can_initial_push": false,
        }),
    );
    m.insert("avatar_url".into(), Value::Null);
    m.insert(
        "request_access_enabled".into(),
        json!(g.request_access_enabled),
    );
    m.insert("full_name".into(), json!(g.full_name));
    m.insert("full_path".into(), json!(g.full_path));
    m.insert("created_at".into(), json!(time::iso(g.created_at)));
    m.insert("parent_id".into(), json!(g.parent_id));
    m.insert("organization_id".into(), json!(1));
    m.insert("shared_runners_setting".into(), json!("enabled"));
    m.insert("max_artifacts_size".into(), Value::Null);
    m.insert("marked_for_deletion_on".into(), Value::Null);
    m
}

/// `GroupDetail`: the shape of `GET /groups/:id`. Projects are inserted by the caller.
pub fn detail(
    g: &Namespace,
    perms: &GroupPerms,
    config: &Config,
    user_can_admin_group: bool,
) -> Map<String, Value> {
    let mut m = group(g, config);
    m.insert("shared_with_groups".into(), json!([]));
    let admin = user_can_admin_group && perms.admin_group();
    if admin {
        use sha2::{Digest, Sha256};
        let h = hex::encode(Sha256::digest(
            format!("labrat-group-runners-{}", g.id).as_bytes(),
        ));
        m.insert(
            "runners_token".into(),
            json!(format!("GR1348941{}", &h[..20])),
        );
    }
    if g.parent_id.is_none() && admin {
        m.insert("enabled_git_access_protocol".into(), json!("all"));
    }
    if g.parent_id.is_none() {
        m.insert(
            "prevent_sharing_groups_outside_hierarchy".into(),
            json!(false),
        );
    }
    m
}

