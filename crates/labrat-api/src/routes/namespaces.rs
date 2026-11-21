//! `lib/api/namespaces.rb`.

use crate::ctx::{ApiResult, Req, Resp};
use crate::entities::project::namespace_basic;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::get;
use labrat_core::model::Namespace;
use labrat_core::{App, Error};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/namespaces", get(list))
        .route("/namespaces/{id}", get(show))
        .route("/namespaces/{id}/exists", get(exists))
}

/// Whether the user is an Owner of the group (`admin_group`) or an administrator.
fn admin_group(c: &Connection, req: &Req, ns: &Namespace) -> labrat_core::Result<bool> {
    let Some(u) = &req.user else { return Ok(false) };
    if u.admin {
        return Ok(true);
    }
    for id in Namespace::self_and_ancestor_ids(c, ns.id)? {
        let level: Option<i64> = c
            .query_row("SELECT access_level FROM members WHERE source_type = 'Namespace' AND source_id = ?1 AND user_id = ?2", rusqlite::params![id, u.id], |r| r.get(0))
            .ok();
        if level.is_some_and(|l| l >= 50) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `Entities::Namespace`.
fn entity(c: &Connection, req: &Req, ns: &Namespace) -> labrat_core::Result<Value> {
    let mut v = namespace_basic(ns, &req.app.config);
    let m = v.as_object_mut().unwrap();
    let admin = ns.is_group() && admin_group(c, req, ns)?;
    let tree = Namespace::self_and_descendant_ids(c, ns.id)?;
    let in_list = tree.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    if admin {
        let members: i64 = c.query_row(
            &format!("SELECT COUNT(DISTINCT user_id) FROM members WHERE source_type = 'Namespace' AND source_id IN ({in_list})"),
            [],
            |r| r.get(0),
        )?;
        let projects: i64 = c.query_row(&format!("SELECT COUNT(*) FROM projects WHERE namespace_id IN ({in_list})"), [], |r| r.get(0))?;
        m.insert("members_count_with_descendants".into(), json!(members));
        // GitLab's aggregated on-disk repository size; labrat stores objects shared across
        // projects, so it reports 0 (the conformance normalizer masks the value).
        m.insert("root_repository_size".into(), json!(0));
        m.insert("projects_count".into(), json!(projects));
        if req.app.config.is_ee() {
            // EE's namespace fields: billable members are the distinct users with a
            // role above Guest in the group tree or its projects (Ultimate doesn't bill Guests).
            let billable: i64 = c.query_row(
                &format!(
                    "SELECT COUNT(DISTINCT m.user_id) FROM members m JOIN users u ON u.id = m.user_id WHERE m.access_level > 10 AND u.user_type = 0 AND (
                        (m.source_type = 'Namespace' AND m.source_id IN ({in_list}))
                        OR (m.source_type = 'Project' AND m.source_id IN (SELECT id FROM projects WHERE namespace_id IN ({in_list}))))"
                ),
                [],
                |r| r.get(0),
            )?;
            m.insert("billable_members_count".into(), json!(billable));
            m.insert("plan".into(), json!("default"));
            m.insert("trial_ends_on".into(), Value::Null);
            m.insert("trial".into(), json!(false));
        }
    }
    Ok(v)
}

/// Namespaces the user may see: all for an administrator, else their own and their groups'.
fn visible(c: &Connection, req: &Req) -> labrat_core::Result<Vec<Namespace>> {
    let me = req.user.as_ref().expect("authenticated");
    let all = Namespace::list_where(c, "n.type != 'Project' ORDER BY n.id", [])?;
    if me.admin {
        return Ok(all);
    }
    let mut ids = vec![];
    let mut st = c.prepare("SELECT source_id FROM members WHERE source_type = 'Namespace' AND user_id = ?1")?;
    for g in st.query_map([me.id], |r| r.get::<_, i64>(0))? {
        ids.extend(Namespace::self_and_descendant_ids(c, g?)?);
    }
    Ok(all.into_iter().filter(|n| ids.contains(&n.id) || (n.is_user() && n.owner_id == Some(me.id))).collect())
}

async fn list(req: Req) -> ApiResult {
    req.user()?;
    let app = req.app.clone();
    app.db.read(|c| {
        let mut list = visible(c, &req)?;
        if req.params.bool_or("top_level_only", false)? {
            list.retain(|n| n.parent_id.is_none());
        }
        if let Some(s) = req.params.str("search") {
            let s = s.to_lowercase();
            let full = req.params.bool_or("full_path_search", false)?;
            list.retain(|n| n.name.to_lowercase().contains(&s) || n.path.to_lowercase().contains(&s) || (full && n.full_path.to_lowercase().contains(&s)));
        }
        paginate(&req, list, &[("full_path_search", "false"), ("top_level_only", "false")], |n| Ok(entity(c, &req, n)?))
    })
}

async fn show(req: Req) -> ApiResult {
    req.user()?;
    let id = req.path_param("id").to_string();
    let body = req.app.db.read(|c| {
        let ns = match id.parse::<i64>() {
            Ok(n) => Namespace::find(c, n)?,
            Err(_) => Namespace::find_by_full_path(c, &id)?,
        };
        let ns = ns.filter(|n| visible(c, &req).map(|v| v.iter().any(|x| x.id == n.id)).unwrap_or(false));
        let ns = ns.ok_or(Error::not_found("Namespace"))?;
        Ok::<_, crate::ctx::ApiError>(entity(c, &req, &ns)?)
    })?;
    Ok(Resp::ok(body))
}

async fn exists(req: Req) -> ApiResult {
    req.user()?;
    let path = req.path_param("id").to_string();
    let parent = req.params.int("parent_id")?;
    let body = req.app.db.read(|c| {
        let siblings: Vec<String> = Namespace::list_where(c, "n.type != 'Project' ORDER BY n.id", [])?
            .into_iter()
            .filter(|n| n.parent_id == parent)
            .map(|n| n.path.to_lowercase())
            .collect();
        let taken = |p: &str| siblings.contains(&p.to_lowercase());
        let exists = taken(&path);
        // `Namespace.clean_path`: append the first free counter.
        let suggests = if exists {
            let mut n = 1;
            while taken(&format!("{path}{n}")) {
                n += 1;
            }
            vec![format!("{path}{n}")]
        } else {
            vec![]
        };
        Ok::<_, Error>(json!({"exists": exists, "suggests": suggests}))
    })?;
    Ok(Resp::ok(body))
}
