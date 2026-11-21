//! `lib/api/members.rb` for groups and projects.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::{member as member_entity, user::Ctx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Member, Namespace, User, access};
use labrat_core::perms::{GroupPerms, ProjectPerms};
use labrat_core::services::members;
use labrat_core::{App, Error, time};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/groups/{id}/members", get(list_group).post(add_group))
        .route("/groups/{id}/members/all", get(list_all_group))
        .route(
            "/groups/{id}/members/{user_id}",
            get(show_group).put(update_group).delete(remove_group),
        )
        .route("/groups/{id}/members/all/{user_id}", get(show_all_group))
        .route(
            "/projects/{id}/members",
            get(list_project).post(add_project),
        )
        .route("/projects/{id}/members/all", get(list_all_project))
        .route(
            "/projects/{id}/members/{user_id}",
            get(show_project).put(update_project).delete(remove_project),
        )
        .route(
            "/projects/{id}/members/all/{user_id}",
            get(show_all_project),
        )
        .route("/projects/{id}/users", get(project_users))
}

/// `GET /projects/:id/users`: the project's team (direct and inherited members) as
/// `UserBasic`, ordered by user id.
async fn project_users(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let s = source(c, &req, false)?;
        let mut users = vec![];
        for m in all_members(c, &s)? {
            users.push(User::get(c, m.user_id)?);
        }
        users.sort_by_key(|u| u.id);
        if let Some(q) = req.params.str("search") {
            let q = q.to_lowercase();
            users.retain(|u| u.username.to_lowercase().contains(&q) || u.name.to_lowercase().contains(&q) || u.email.to_lowercase() == q);
        }
        if let Some(skip) = req.params.str("skip_users") {
            let skip: Vec<i64> = skip.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            users.retain(|u| !skip.contains(&u.id));
        }
        let ctx = Ctx { base_url: &req.app.config.external_url, ee: req.app.config.is_ee(), current_user: req.user.as_ref() };
        paginate(&req, users, &[], |u| Ok(crate::entities::user::basic_value(u, &ctx)))
    })
}

/// The membership source behind a `/groups/:id` or `/projects/:id` route.
struct Source {
    kind: &'static str,
    id: i64,
    /// Namespace whose ancestors provide inherited memberships.
    inherit_from: Option<i64>,
    /// Caller's access level and whether they are an admin.
    level: i64,
    admin: bool,
    /// Owner-level permission (`admin_group_member` for groups, `manage_owners` for projects).
    owner: bool,
    /// Can manage members at all.
    manage: bool,
}

fn source(c: &Connection, req: &Req, group: bool) -> Result<Source, ApiError> {
    req.user()?;
    let id = req.path_param("id").to_string();
    if group {
        let (g, perms): (Namespace, GroupPerms) = find::group(c, req, &id)?;
        Ok(Source {
            kind: "Namespace",
            id: g.id,
            inherit_from: g.parent_id,
            level: perms.level,
            admin: perms.admin,
            owner: perms.owner(),
            manage: perms.owner(),
        })
    } else {
        let (p, perms): (_, ProjectPerms) = find::project(c, req, &id)?;
        let ns = Namespace::find(c, p.namespace_id)?;
        Ok(Source {
            kind: "Project",
            id: p.id,
            inherit_from: ns.filter(|n| n.is_group()).map(|n| n.id),
            level: perms.level,
            admin: perms.admin,
            owner: perms.owner(),
            manage: perms.admin_project(),
        })
    }
}

fn render(c: &Connection, req: &Req, m: &Member) -> Result<Value, ApiError> {
    let u = User::get(c, m.user_id)?;
    let created_by = match m.created_by_id {
        Some(id) => User::find(c, id)?,
        None => None,
    };
    let ctx = Ctx {
        base_url: &req.app.config.external_url, ee: req.app.config.is_ee(),
        current_user: req.user.as_ref(),
    };
    Ok(member_entity::member(
        m,
        &u,
        &ctx,
        created_by.as_ref(),
        true,
    ))
}

fn filter_query(c: &Connection, req: &Req, list: Vec<Member>) -> Result<Vec<Member>, ApiError> {
    let query = req.params.str("query").map(|q| q.to_lowercase());
    let user_ids = req.params.int_list("user_ids")?;
    let skip = req.params.int_list("skip_users")?;
    let mut out = vec![];
    for m in list {
        if user_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(&m.user_id))
        {
            continue;
        }
        if skip.as_ref().is_some_and(|ids| ids.contains(&m.user_id)) {
            continue;
        }
        if let Some(q) = &query {
            let u = User::get(c, m.user_id)?;
            if !u.username.to_lowercase().contains(q) && !u.name.to_lowercase().contains(q) {
                continue;
            }
        }
        out.push(m);
    }
    Ok(out)
}

async fn list(req: Req, group: bool) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let s = source(c, &req, group)?;
        let list = filter_query(c, &req, members::list(c, s.kind, s.id)?)?;
        paginate(&req, list, &[], |m| render(c, &req, m))
    })
}

/// Direct plus inherited members, one row per user with the highest level.
fn all_members(c: &Connection, s: &Source) -> labrat_core::Result<Vec<Member>> {
    let mut all = members::list(c, s.kind, s.id)?;
    if let Some(ns) = s.inherit_from {
        for id in Namespace::self_and_ancestor_ids(c, ns)? {
            all.extend(members::list(c, "Namespace", id)?);
        }
    }
    let today = time::today();
    all.retain(|m| m.expires_at.as_deref().is_none_or(|e| e > today.as_str()));
    let mut best: Vec<Member> = vec![];
    for m in all {
        match best.iter_mut().find(|b| b.user_id == m.user_id) {
            Some(b) if m.access_level > b.access_level => *b = m,
            Some(_) => {}
            None => best.push(m),
        }
    }
    best.sort_by_key(|m| m.id);
    Ok(best)
}

async fn list_all(req: Req, group: bool) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let s = source(c, &req, group)?;
        let list = filter_query(c, &req, all_members(c, &s)?)?;
        paginate(&req, list, &[], |m| render(c, &req, m))
    })
}

fn user_id_param(req: &Req) -> Result<i64, ApiError> {
    req.path_param("user_id")
        .parse()
        .map_err(|_| Error::Param("user_id is invalid".into()).into())
}

fn not_found_member() -> ApiError {
    Error::Api(404, json!("404 Not found")).into()
}

async fn show(req: Req, group: bool, all: bool) -> ApiResult {
    let uid = user_id_param(&req)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let s = source(c, &req, group)?;
        let m = if all {
            all_members(c, &s)?.into_iter().find(|m| m.user_id == uid)
        } else {
            Member::find(c, s.kind, s.id, uid)?
        };
        let m = m.ok_or_else(not_found_member)?;
        render(c, &req, &m)
    })?;
    Ok(Resp::ok(body))
}

fn check_assign(s: &Source, level: i64) -> Result<(), ApiError> {
    if s.admin {
        return Ok(());
    }
    if level == access::OWNER && !s.owner {
        return Err(Error::forbidden().into());
    }
    if level > s.level || (level == access::PLANNER && !s.owner) {
        return Err(Error::forbidden().into());
    }
    Ok(())
}

async fn add(req: Req, group: bool) -> ApiResult {
    let me = req.user()?.clone();
    let p = &req.params;
    p.require(&["access_level"])?;
    if p.has("user_id") && p.has("username") {
        return Err(Error::Param("user_id, username are mutually exclusive".into()).into());
    }
    p.require_any(&["user_id", "username"])?;
    let level = p.int("access_level")?.unwrap();
    let expires_at = p.date("expires_at")?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let s = source(c, &req, group)?;
        if !s.manage {
            return Err(Error::forbidden().into());
        }
        let user = match p.str("user_id") {
            Some(id) => {
                find::user_by_ref(c, &id)?.filter(|_| id.chars().all(|ch| ch.is_ascii_digit()))
            }
            None => User::find_by_username(c, &p.str("username").unwrap())?,
        };
        let user = user.ok_or(Error::not_found("User"))?;
        if Member::find(c, s.kind, s.id, user.id)?.is_some() {
            return Err(Error::message(409, "Member already exists").into());
        }
        if !access::valid(level) || level == access::MINIMAL {
            return Err(Error::Validation(vec![(
                "access_level".into(),
                "is not included in the list".into(),
            )])
            .into());
        }
        check_assign(&s, level)?;
        if let Some(e) = &expires_at
            && e.as_str() <= time::today().as_str() {
                return Err(Error::Validation(vec![(
                    "expires_at".into(),
                    "cannot be a date in the past".into(),
                )])
                .into());
            }
        let m = members::add(
            c,
            Some(&me),
            s.kind,
            s.id,
            user.id,
            level,
            expires_at.as_deref(),
        )?;
        render(c, &req, &m)
    })?;
    Ok(Resp::created(body))
}

async fn update(req: Req, group: bool) -> ApiResult {
    req.user()?;
    let uid = user_id_param(&req)?;
    req.params.require(&["access_level"])?;
    let level = req.params.int("access_level")?.unwrap();
    let expires_at = if req.params.has("expires_at") {
        Some(req.params.date("expires_at")?)
    } else {
        None
    };
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let s = source(c, &req, group)?;
        let m = Member::find(c, s.kind, s.id, uid)?.ok_or_else(not_found_member)?;
        if !s.manage {
            return Err(Error::forbidden().into());
        }
        if !access::valid(level) || level == access::MINIMAL {
            return Err(Error::Validation(vec![(
                "access_level".into(),
                "is not included in the list".into(),
            )])
            .into());
        }
        check_assign(&s, level)?;
        if m.access_level == access::OWNER && !s.owner {
            return Err(Error::forbidden().into());
        }
        let m = members::update(
            c,
            &m,
            Some(level),
            expires_at.as_ref().map(|e| e.as_deref()),
        )?;
        render(c, &req, &m)
    })?;
    Ok(Resp::ok(body))
}

async fn remove(req: Req, group: bool) -> ApiResult {
    let me = req.user()?.clone();
    let uid = user_id_param(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let s = source(c, &req, group)?;
        let m = Member::find(c, s.kind, s.id, uid)?.ok_or_else(not_found_member)?;
        let self_removal = m.user_id == me.id;
        if !s.manage && !self_removal {
            return Err(Error::forbidden().into());
        }
        if m.access_level == access::OWNER && !s.owner && !self_removal {
            return Err(Error::forbidden().into());
        }
        if group && m.access_level == access::OWNER {
            let owners: i64 = c.query_row(
                "SELECT count(*) FROM members WHERE source_type = 'Namespace' AND source_id = ?1 AND access_level = 50",
                [s.id],
                |r| r.get(0),
            )?;
            if owners <= 1 {
                return Err(Error::forbidden().into());
            }
        }
        members::remove(c, &m)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn list_group(req: Req) -> ApiResult {
    list(req, true).await
}
async fn list_project(req: Req) -> ApiResult {
    list(req, false).await
}
async fn list_all_group(req: Req) -> ApiResult {
    list_all(req, true).await
}
async fn list_all_project(req: Req) -> ApiResult {
    list_all(req, false).await
}
async fn show_group(req: Req) -> ApiResult {
    show(req, true, false).await
}
async fn show_project(req: Req) -> ApiResult {
    show(req, false, false).await
}
async fn show_all_group(req: Req) -> ApiResult {
    show(req, true, true).await
}
async fn show_all_project(req: Req) -> ApiResult {
    show(req, false, true).await
}
async fn add_group(req: Req) -> ApiResult {
    add(req, true).await
}
async fn add_project(req: Req) -> ApiResult {
    add(req, false).await
}
async fn update_group(req: Req) -> ApiResult {
    update(req, true).await
}
async fn update_project(req: Req) -> ApiResult {
    update(req, false).await
}
async fn remove_group(req: Req) -> ApiResult {
    remove(req, true).await
}
async fn remove_project(req: Req) -> ApiResult {
    remove(req, false).await
}
