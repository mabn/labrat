//! `lib/api/groups.rb`.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::group as group_entity;
use crate::find;
use crate::pagination::paginate;
use crate::routes::projects::{View, list_projects, project_value};
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Namespace, Project, access, visibility};
use labrat_core::perms::{GroupPerms, ProjectPerms};
use labrat_core::services::groups::{self, GroupUpdate, NewGroup, creation_level};
use labrat_core::{App, Error, time};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/groups", get(list).post(create))
        .route("/groups/{id}", get(show).put(update).delete(destroy))
        .route("/groups/{id}/subgroups", get(subgroups))
        .route("/groups/{id}/descendant_groups", get(descendants))
        .route("/groups/{id}/projects", get(projects))
}

const LIST_DEFAULTS: &[(&str, &str)] = &[
    ("order_by", "name"),
    ("owned", "false"),
    ("sort", "asc"),
    ("statistics", "false"),
    ("with_custom_attributes", "false"),
];

/// Groups visible in lists: members see their groups (and ancestors); admins see all.
fn visible_groups(
    c: &Connection,
    req: &Req,
    parent: Option<&Namespace>,
    all_levels: bool,
) -> Result<Vec<(Namespace, GroupPerms)>, ApiError> {
    let p = &req.params;
    let user = req.maybe_user()?;
    let all_available = p
        .bool("all_available")?
        .unwrap_or(user.is_some_and(|u| u.admin));
    let groups = Namespace::list_where(c, "n.type = 'Group'", [])?;
    let mut out = vec![];
    let descendants = match parent {
        Some(pg) => Some(Namespace::self_and_descendant_ids(c, pg.id)?),
        None => None,
    };
    for g in groups {
        if let (Some(pg), Some(desc)) = (parent, &descendants) {
            if all_levels {
                if g.id == pg.id || !desc.contains(&g.id) {
                    continue;
                }
            } else if g.parent_id != Some(pg.id) {
                continue;
            }
        }
        let perms = GroupPerms::load(c, user, &g)?;
        let member_or_related = perms.level > 0 || perms.has_visible_projects || perms.admin;
        let visible = if parent.is_some() || all_available {
            perms.read_group()
        } else if user.is_some() {
            member_or_related || is_ancestor_of_member_group(c, user.unwrap().id, g.id)?
        } else {
            g.visibility_level == visibility::PUBLIC
        };
        if visible {
            out.push((g, perms));
        }
    }
    if p.bool_or("top_level_only", false)? {
        out.retain(|(g, _)| g.parent_id.is_none());
    }
    if let Some(s) = p.str("search") {
        let s = s.to_lowercase();
        out.retain(|(g, _)| {
            g.name.to_lowercase().contains(&s) || g.path.to_lowercase().contains(&s)
        });
    }
    if let Some(v) = p.one_of("visibility", &["private", "internal", "public"])? {
        let level = visibility::parse(&v).unwrap();
        out.retain(|(g, _)| g.visibility_level == level);
    }
    if p.bool_or("owned", false)? {
        out.retain(|(_, perms)| perms.level >= access::OWNER);
    }
    if let Some(l) = p.int("min_access_level")? {
        out.retain(|(_, perms)| perms.level >= l);
    }
    if let Some(ids) = p.int_list("skip_groups")? {
        out.retain(|(g, _)| !ids.contains(&g.id));
    }
    let order_by = p
        .one_of("order_by", &["name", "path", "id", "similarity"])?
        .unwrap_or("name".into());
    let desc = p.one_of("sort", &["asc", "desc"])?.as_deref() == Some("desc");
    out.sort_by(|(a, _), (b, _)| {
        let o = match order_by.as_str() {
            "path" => a.path.cmp(&b.path),
            "id" => a.id.cmp(&b.id),
            _ => a.name.cmp(&b.name),
        };
        let o = if desc { o.reverse() } else { o };
        o.then(a.id.cmp(&b.id))
    });
    Ok(out)
}

fn is_ancestor_of_member_group(
    c: &Connection,
    user_id: i64,
    group_id: i64,
) -> labrat_core::Result<bool> {
    let ids = Namespace::self_and_descendant_ids(c, group_id)?;
    let list = ids
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let n: i64 = c.query_row(
        &format!("SELECT count(*) FROM members WHERE user_id = ?1 AND source_type = 'Namespace' AND source_id IN ({list})"),
        [user_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

async fn list(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let groups = visible_groups(c, &req, None, false)?;
        paginate(&req, groups, LIST_DEFAULTS, |(g, _)| {
            Ok(Value::Object(group_entity::group(g, &req.app.config)))
        })
    })
}

async fn subgroups(req: Req) -> ApiResult {
    nested(req, false).await
}

async fn descendants(req: Req) -> ApiResult {
    nested(req, true).await
}

async fn nested(req: Req, all: bool) -> ApiResult {
    let id = req.path_param("id").to_string();
    let app = req.app.clone();
    app.db.read(|c| {
        let (g, _) = find::group(c, &req, &id)?;
        let groups = visible_groups(c, &req, Some(&g), all)?;
        paginate(&req, groups, LIST_DEFAULTS, |(g, _)| {
            Ok(Value::Object(group_entity::group(g, &req.app.config)))
        })
    })
}

/// `GroupDetail` with the projects block.
pub fn detail_value(
    c: &Connection,
    req: &Req,
    g: &Namespace,
    perms: &GroupPerms,
    with_projects: bool,
) -> Result<Value, ApiError> {
    let mut m = group_entity::detail(g, perms, &req.app.config, with_projects);
    if with_projects {
        let mut items = vec![];
        for p in Project::list_where(c, "p.namespace_id = ?1 ORDER BY p.id DESC", [g.id])? {
            let pp = ProjectPerms::load(c, req.user.as_ref(), &p)?;
            if pp.read_project() {
                items.push(project_value(c, req, &p, &pp, View::List)?);
            }
            if items.len() >= 100 {
                break;
            }
        }
        m.insert("projects".into(), Value::Array(items));
        m.insert("shared_projects".into(), json!([]));
    }
    Ok(Value::Object(m))
}

async fn show(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let with_projects = req.params.bool_or("with_projects", true)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (g, perms) = find::group(c, &req, &id)?;
        detail_value(c, &req, &g, &perms, with_projects)
    })?;
    Ok(Resp::ok(body))
}

fn ruby_hash(errs: &[(String, String)]) -> String {
    let mut fields: Vec<(String, Vec<String>)> = vec![];
    for (f, m) in errs {
        match fields.iter_mut().find(|(k, _)| k == f) {
            Some((_, v)) => v.push(m.clone()),
            None => fields.push((f.clone(), vec![m.clone()])),
        }
    }
    let inner: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            let msgs: Vec<String> = v.iter().map(|m| format!("{m:?}")).collect();
            format!(":{k}=>[{}]", msgs.join(", "))
        })
        .collect();
    format!("{{{}}}", inner.join(", "))
}

fn update_from_params(req: &Req) -> Result<GroupUpdate, ApiError> {
    let p = &req.params;
    let visibility = match p.one_of("visibility", &["private", "internal", "public"])? {
        Some(v) => visibility::parse(&v),
        None => None,
    };
    let pcl = match p.one_of(
        "project_creation_level",
        &["noone", "owner", "maintainer", "developer", "administrator"],
    )? {
        Some(v) => creation_level::parse_project(&v),
        None => None,
    };
    let scl = match p.one_of("subgroup_creation_level", &["owner", "maintainer"])? {
        Some(v) => creation_level::parse_subgroup(&v),
        None => None,
    };
    let mut settings = vec![];
    for key in [
        "share_with_group_lock",
        "require_two_factor_authentication",
        "lfs_enabled",
        "mentions_disabled",
        "auto_devops_enabled",
    ] {
        if let Some(b) = p.bool(key)? {
            settings.push((key.to_string(), Value::Bool(b)));
        }
    }
    if let Some(n) = p.int("two_factor_grace_period")? {
        settings.push(("two_factor_grace_period".into(), json!(n)));
    }
    if let Some(s) = p.str("default_branch") {
        settings.push(("default_branch".into(), json!(s)));
    }
    Ok(GroupUpdate {
        name: p.str("name"),
        path: p.str("path"),
        description: p.str("description"),
        visibility,
        project_creation_level: pcl,
        subgroup_creation_level: scl,
        request_access_enabled: p.bool("request_access_enabled")?,
        settings,
    })
}

async fn create(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    req.params.require(&["name", "path"])?;
    let upd = update_from_params(&req)?;
    let parent_id = req.params.int("parent_id")?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        if let Some(pid) = parent_id {
            let (parent, perms) = find::group(c, &req, &pid.to_string())?;
            let needed = if parent.subgroup_creation_level.unwrap_or(1) == 0 {
                access::OWNER
            } else {
                access::MAINTAINER
            };
            if !(perms.admin || perms.level >= needed) {
                return Err(Error::forbidden().into());
            }
        } else if !me.can_create_group && !me.admin {
            return Err(Error::forbidden().into());
        }
        let new = NewGroup {
            name: upd.name.clone().unwrap_or_default(),
            path: upd.path.clone().unwrap_or_default(),
            parent_id,
            description: upd.description.clone(),
            visibility: upd.visibility,
            project_creation_level: upd.project_creation_level,
            subgroup_creation_level: upd.subgroup_creation_level,
            request_access_enabled: upd.request_access_enabled,
        };
        let g = match groups::create(c, &me, new) {
            Ok(g) => g,
            Err(Error::Validation(errs)) => {
                return Err(Error::message(
                    400,
                    format!("Failed to save group {}", ruby_hash(&errs)),
                )
                .into());
            }
            Err(e) => return Err(e.into()),
        };
        let g = if upd.settings.is_empty() {
            g
        } else {
            groups::update(
                c,
                &g,
                GroupUpdate {
                    settings: upd.settings.clone(),
                    ..Default::default()
                },
            )?
        };
        let perms = GroupPerms::load(c, Some(&me), &g)?;
        detail_value(c, &req, &g, &perms, false)
    })?;
    Ok(Resp::created(body))
}

async fn update(req: Req) -> ApiResult {
    req.user()?;
    let id = req.path_param("id").to_string();
    let upd = update_from_params(&req)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (g, perms) = find::group(c, &req, &id)?;
        if !perms.admin_group() {
            return Err(Error::forbidden().into());
        }
        let g = groups::update(c, &g, upd)?;
        detail_value(c, &req, &g, &perms, true)
    })?;
    Ok(Resp::ok(body))
}

async fn destroy(req: Req) -> ApiResult {
    req.user()?;
    let id = req.path_param("id").to_string();
    let permanently = req.params.bool_or("permanently_remove", false)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (g, perms) = find::group(c, &req, &id)?;
        if !perms.owner() {
            return Err(Error::forbidden().into());
        }
        let marked = g
            .settings
            .get("marked_for_deletion_on")
            .is_some_and(|v| !v.is_null());
        if permanently {
            if g.parent_id.is_none() {
                return Err(Error::message(
                    400,
                    "`permanently_remove` option is only available for subgroups.",
                )
                .into());
            }
            if !marked {
                return Err(Error::message(400, "Group must be marked for deletion first.").into());
            }
            if req.params.str("full_path").as_deref() != Some(g.full_path.as_str()) {
                return Err(Error::message(
                    400,
                    "`full_path` is incorrect. You must enter the complete path for the subgroup.",
                )
                .into());
            }
            groups::delete(c, &g)?;
            return Ok(());
        }
        if marked {
            return Err(Error::message(400, "Group has already been marked for deletion").into());
        }
        let suffix = format!("-deletion_scheduled-{}", g.id);
        groups::update(
            c,
            &g,
            GroupUpdate {
                name: Some(format!("{}{suffix}", g.name)),
                path: Some(format!("{}{suffix}", g.path)),
                settings: vec![("marked_for_deletion_on".into(), json!(time::today()))],
                ..Default::default()
            },
        )?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::status(202, json!({"message": "202 Accepted"})))
}

async fn projects(req: Req) -> ApiResult {
    let id = req.path_param("id").to_string();
    let include_subgroups = req.params.bool_or("include_subgroups", false)?;
    let app = req.app.clone();
    let ns_ids = app.db.read(|c| {
        let (g, _) = find::group(c, &req, &id)?;
        let ids = if include_subgroups {
            Namespace::self_and_descendant_ids(c, g.id)?
        } else {
            vec![g.id]
        };
        Ok::<_, ApiError>(ids)
    })?;
    list_projects(
        req,
        Some(ns_ids),
        &[
            ("include_ancestor_groups", "false"),
            ("include_subgroups", "false"),
            ("order_by", "created_at"),
            ("owned", "false"),
            ("simple", "false"),
            ("sort", "desc"),
            ("starred", "false"),
            ("with_custom_attributes", "false"),
            ("with_issues_enabled", "false"),
            ("with_merge_requests_enabled", "false"),
            ("with_shared", "true"),
        ],
    )
    .await
}
