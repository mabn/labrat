//! `lib/api/users.rb`, `lib/api/personal_access_tokens.rb`.

use crate::ctx::{ApiResult, Req, Resp};
use crate::entities::{token as token_entity, user as user_entity};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::{get, post};
use labrat_core::model::{Token, User};
use labrat_core::services::{tokens, users};
use labrat_core::{App, Error, time};
use serde_json::Value;
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/user", get(current_user))
        .route("/users", get(list).post(create))
        .route("/users/{id}", get(show).put(update).delete(destroy))
        .route("/users/{id}/block", post(block))
        .route("/users/{id}/unblock", post(unblock))
        .route("/users/{id}/deactivate", post(deactivate))
        .route("/users/{id}/activate", post(activate))
        .route("/users/{id}/personal_access_tokens", post(create_pat))
        .route(
            "/users/{id}/impersonation_tokens",
            get(list_impersonation).post(create_impersonation),
        )
        .route(
            "/users/{id}/impersonation_tokens/{token_id}",
            get(show_impersonation).delete(revoke_impersonation),
        )
        .route("/user/personal_access_tokens", post(create_own_pat))
        .route("/personal_access_tokens", get(list_pats))
        .route(
            "/personal_access_tokens/self",
            get(pat_self).delete(revoke_self),
        )
        .route(
            "/personal_access_tokens/{id}",
            get(show_pat).delete(revoke_pat),
        )
        .route("/personal_access_tokens/{id}/rotate", post(rotate_pat))
        .route("/personal_access_tokens/self/rotate", post(rotate_self))
        .route("/user_counts", get(user_counts))
        .route("/user/status", get(own_status).put(set_status_put).patch(set_status_patch))
        .route("/users/{id}/status", get(user_status))
}

/// `PersonalAccessTokens::RotateService`: revokes the token and issues a copy expiring in a
/// week unless `expires_at` is given.
fn rotate(c: &rusqlite::Connection, req: &Req, t: &Token) -> Result<Value, crate::ctx::ApiError> {
    if t.revoked || !t.active() {
        return Err(Error::BadRequest(Some("Token already revoked".into())).into());
    }
    let expires = match req.params.str("expires_at") {
        Some(e) => e,
        None => time::date(time::now() + 7 * 86_400_000),
    };
    let owner = User::get(c, t.user_id)?;
    tokens::revoke(c, t)?;
    let (new, value) = tokens::create(
        c,
        &owner,
        tokens::NewToken { name: &t.name, description: t.description.as_deref(), scopes: &t.scopes, expires_at: Some(&expires), impersonation: false, value: None },
    )?;
    Ok(token_entity::with_token(&new, &value))
}

async fn rotate_pat(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id: i64 = req.path_param("id").parse().map_err(|_| Error::Param("id is invalid".into()))?;
    let body = req.app.db.tx(|c| {
        let t = Token::find(c, id)?
            .filter(|t| !t.impersonation && (me.admin || t.user_id == me.id))
            .ok_or(Error::not_found("Personal Access Token"))?;
        rotate(c, &req, &t)
    })?;
    Ok(Resp::ok(body))
}

async fn rotate_self(req: Req) -> ApiResult {
    req.user()?;
    let t = req.token.clone().ok_or(Error::Unauthorized(None))?;
    let body = req.app.db.tx(|c| {
        let t = Token::find(c, t.id)?.unwrap_or(t);
        rotate(c, &req, &t)
    })?;
    Ok(Resp::ok(body))
}

/// `Entities::UserCounts`, with the counts cached as GitLab caches them.
async fn user_counts(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let body = req.app.db.read(|c| {
        let assigned = labrat_core::services::user_counts::assigned_open(c, me.id)?;
        let review = labrat_core::services::user_counts::review_requested_open(c, me.id)?;
        let todos = labrat_core::services::user_counts::todos_pending(c, me.id)?;
        Ok::<_, Error>(serde_json::json!({
            "merge_requests": assigned,
            "assigned_issues": 0,
            "assigned_merge_requests": assigned,
            "review_requested_merge_requests": review,
            "todos": todos,
        }))
    })?;
    Ok(Resp::ok(body))
}

fn status_entity(c: &rusqlite::Connection, user_id: i64) -> labrat_core::Result<Value> {
    let row: Option<(String, Option<String>, String, Option<i64>)> = c
        .query_row(
            "SELECT emoji, message, availability, clear_status_at FROM user_statuses WHERE user_id = ?1 AND (clear_status_at IS NULL OR clear_status_at > ?2)",
            rusqlite::params![user_id, time::now()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    Ok(match row {
        Some((emoji, message, availability, clear)) => serde_json::json!({
            "emoji": emoji,
            "message_html": message.as_deref().map(crate::markdown::render_inline).unwrap_or_default(),
            "message": message,
            "availability": availability,
            "clear_status_at": clear.map(time::iso),
        }),
        None => serde_json::json!({"emoji": null, "message": null, "availability": null, "message_html": "", "clear_status_at": null}),
    })
}

/// Field order of `Entities::UserStatus`.
fn ordered_status(v: Value) -> Value {
    serde_json::json!({"emoji": v["emoji"], "message": v["message"], "availability": v["availability"], "message_html": v["message_html"], "clear_status_at": v["clear_status_at"]})
}

async fn own_status(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let v = req.app.db.read(|c| status_entity(c, me.id))?;
    Ok(Resp::ok(ordered_status(v)))
}

async fn user_status(req: Req) -> ApiResult {
    req.user()?;
    let id = req.path_param("id").to_string();
    let v = req.app.db.read(|c| {
        let u = match id.parse::<i64>() {
            Ok(n) => User::find(c, n)?,
            Err(_) => User::find_by_username(c, &id)?,
        };
        let u = u.ok_or(Error::not_found("User"))?;
        status_entity(c, u.id)
    })?;
    Ok(Resp::ok(ordered_status(v)))
}

/// `Users::SetStatusService`. PUT nullifies missing fields, PATCH keeps them.
async fn set_status(req: Req, include_missing: bool) -> ApiResult {
    let me = req.user()?.clone();
    let p = &req.params;
    let clear_after = p.one_of("clear_status_after", &["30_minutes", "3_hours", "8_hours", "1_day", "3_days", "7_days", "30_days"])?;
    let v = req.app.db.tx(|c| {
        let current: Option<(String, Option<String>, String, Option<i64>)> = c
            .query_row("SELECT emoji, message, availability, clear_status_at FROM user_statuses WHERE user_id = ?1", [me.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .ok();
        let keep = |new: Option<String>, old: Option<String>| if include_missing || new.is_some() { new } else { old };
        let emoji = keep(p.str("emoji"), current.as_ref().map(|c| c.0.clone()));
        let message = keep(p.str("message"), current.as_ref().and_then(|c| c.1.clone()));
        let availability = keep(p.str("availability"), current.as_ref().map(|c| c.2.clone())).filter(|a| a == "busy" || a == "not_set");
        let clear_at = match &clear_after {
            Some(k) => {
                let mins = match k.as_str() {
                    "30_minutes" => 30,
                    "3_hours" => 180,
                    "8_hours" => 480,
                    "1_day" => 1440,
                    "3_days" => 3 * 1440,
                    "7_days" => 7 * 1440,
                    _ => 30 * 1440,
                };
                Some(time::now() + mins * 60_000)
            }
            None if include_missing => None,
            None => current.as_ref().and_then(|c| c.3),
        };
        let blank = |s: &Option<String>| s.as_deref().is_none_or(|x| x.trim().is_empty());
        if blank(&emoji) && blank(&message) && availability.as_deref().is_none_or(|a| a == "not_set") {
            c.execute("DELETE FROM user_statuses WHERE user_id = ?1", [me.id])?;
        } else {
            let emoji = if blank(&emoji) { "speech_balloon".to_string() } else { emoji.unwrap() };
            c.execute(
                "INSERT OR REPLACE INTO user_statuses (user_id, emoji, message, availability, clear_status_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![me.id, emoji, message, availability.unwrap_or_else(|| "not_set".into()), clear_at],
            )?;
        }
        status_entity(c, me.id)
    })?;
    Ok(Resp::ok(ordered_status(v)))
}

async fn set_status_put(req: Req) -> ApiResult {
    set_status(req, true).await
}

async fn set_status_patch(req: Req) -> ApiResult {
    set_status(req, false).await
}

fn ectx<'a>(req: &'a Req) -> user_entity::Ctx<'a> {
    user_entity::Ctx {
        base_url: &req.app.config.external_url,
        current_user: req.user.as_ref(),
    }
}

/// The admin view of a user, with `created_by` resolved.
pub fn admin_view(c: &rusqlite::Connection, req: &Req, u: &User) -> labrat_core::Result<Value> {
    let created_by = match u.created_by_id {
        Some(id) => User::find(c, id)?,
        None => None,
    };
    Ok(Value::Object(user_entity::with_admin(
        u,
        &ectx(req),
        created_by.as_ref(),
    )))
}

async fn current_user(req: Req) -> ApiResult {
    let me = req.user()?;
    let body = req.app.db.read(|c| {
        if me.admin {
            admin_view(c, &req, me)
        } else {
            Ok(Value::Object(user_entity::public(me, &ectx(&req))))
        }
    })?;
    Ok(Resp::ok(body))
}

async fn list(req: Req) -> ApiResult {
    let me = req.maybe_user()?;
    let p = &req.params;
    if me.is_none() && p.str("username").is_none() {
        return Err(Error::Forbidden(Some("Not authorized to access /api/v4/users".into())).into());
    }
    let order_by = p.one_of(
        "order_by",
        &["id", "name", "username", "created_at", "updated_at"],
    )?;
    let sort = p.one_of("sort", &["asc", "desc"])?;
    let admin = req.is_admin();
    let mut users: Vec<User> = req.app.db.read(|c| {
        let mut st = c.prepare(&format!("SELECT {} FROM users", User::COLUMNS))?;
        Ok::<_, Error>(
            st.query_map([], User::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        )
    })?;
    if let Some(u) = p.str("username") {
        users.retain(|x| x.username.eq_ignore_ascii_case(&u));
    }
    if let Some(s) = p.str("search") {
        let s = s.to_lowercase();
        users.retain(|x| {
            x.username.to_lowercase().contains(&s)
                || x.name.to_lowercase().contains(&s)
                || (admin && x.email.to_lowercase() == s)
                || x.public_email
                    .as_deref()
                    .is_some_and(|e| e.to_lowercase() == s)
        });
    }
    if p.bool_or("active", false)? {
        users.retain(|x| x.active());
    }
    if p.bool_or("blocked", false)? {
        users.retain(|x| x.blocked());
    }
    if p.bool_or("external", false)? {
        users.retain(|x| x.external);
    }
    if p.bool_or("exclude_external", false)? {
        users.retain(|x| !x.external);
    }
    if p.bool_or("humans", false)? {
        users.retain(|x| !x.bot());
    }
    if p.bool_or("exclude_humans", false)? {
        users.retain(|x| x.bot());
    }
    if admin && p.bool_or("admins", false)? {
        users.retain(|x| x.admin);
    }
    if admin {
        if let Some(t) = p.time("created_after")? {
            users.retain(|x| x.created_at > t);
        }
        if let Some(t) = p.time("created_before")? {
            users.retain(|x| x.created_at < t);
        }
    }
    let order_by = if admin {
        order_by.unwrap_or("id".into())
    } else {
        "id".into()
    };
    let desc = if admin {
        sort.as_deref() != Some("asc")
    } else {
        true
    };
    users.sort_by(|a, b| {
        let o = match order_by.as_str() {
            "name" => a.name.cmp(&b.name),
            "username" => a.username.cmp(&b.username),
            "created_at" => a.created_at.cmp(&b.created_at),
            "updated_at" => a.updated_at.cmp(&b.updated_at),
            _ => a.id.cmp(&b.id),
        }
        .then(a.id.cmp(&b.id));
        if desc { o.reverse() } else { o }
    });
    let defaults: &[(&str, &str)] = &[
        ("active", "false"),
        ("admins", "false"),
        ("blocked", "false"),
        ("exclude_active", "false"),
        ("exclude_external", "false"),
        ("exclude_humans", "false"),
        ("exclude_internal", "false"),
        ("external", "false"),
        ("humans", "false"),
        ("order_by", "id"),
        ("sort", "desc"),
        ("with_custom_attributes", "false"),
        ("without_project_bots", "false"),
        ("without_projects", "false"),
    ];
    let app = req.app.clone();
    app.db.read(|c| {
        paginate(&req, users, defaults, |u| {
            if admin {
                Ok(admin_view(c, &req, u)?)
            } else {
                Ok(user_entity::basic_value(u, &ectx(&req)))
            }
        })
    })
}

async fn show(req: Req) -> ApiResult {
    req.maybe_user()?;
    let id = req.path_param("id").to_string();
    let body = req.app.db.read(|c| {
        let u = find::user_by_id(c, &id)?;
        let me = req.user.as_ref();
        if me.is_some_and(|m| m.admin) {
            let mut v = admin_view(c, &req, &u)?;
            let m = v.as_object_mut().unwrap();
            // `UserHighestRole` isn't maintained for bots.
            let highest: i64 = if u.user_type != 0 {
                0
            } else {
                c.query_row("SELECT coalesce(max(access_level), 0) FROM members WHERE user_id = ?1", [u.id], |r| r.get(0))?
            };
            m.insert("highest_role".into(), Value::from(highest));
            m.insert("current_sign_in_ip".into(), Value::Null);
            m.insert("last_sign_in_ip".into(), Value::Null);
            m.insert("sign_in_count".into(), Value::from(u.sign_in_count));
            return Ok(v);
        }
        if !u.active() && me.is_none() {
            return Err(Error::not_found("User").into());
        }
        Ok::<_, crate::ctx::ApiError>(Value::Object(user_entity::user(&u, &ectx(&req))))
    })?;
    Ok(Resp::ok(body))
}

fn profile_params(req: &Req) -> Vec<(String, Option<String>)> {
    users::PROFILE_FIELDS
        .iter()
        .filter(|f| **f != "job_title" && req.params.has(f))
        .map(|f| (f.to_string(), req.params.str(f)))
        .collect()
}

async fn create(req: Req) -> ApiResult {
    let admin = req.admin()?.clone();
    let p = &req.params;
    let mut errs = vec![];
    if !p.has("password") && !p.has("reset_password") && !p.has("force_random_password") {
        errs.push("password, reset_password, force_random_password are missing, at least one parameter must be provided".to_string());
    }
    if let Err(Error::Param(m)) = p.require(&["email", "name", "username"]) {
        errs.push(m);
    }
    if !errs.is_empty() {
        return Err(Error::Param(errs.join(", ")).into());
    }
    let new = users::NewUser {
        username: p.str("username").unwrap_or_default(),
        name: p.str("name").unwrap_or_default(),
        email: p.str("email").unwrap_or_default(),
        password: p.str("password"),
        admin: p.bool_or("admin", false)?,
        external: p.bool_or("external", false)?,
        private_profile: p.bool_or("private_profile", false)?,
        can_create_group: p.bool("can_create_group")?,
        projects_limit: p.int("projects_limit")?,
        public_email: p.str("public_email"),
        skip_confirmation: p.bool_or("skip_confirmation", false)?,
        profile: profile_params(&req),
        ..Default::default()
    };
    let body = req.app.db.tx(|c| {
        if users::email_taken(c, &new.email, None)? {
            return Err(Error::message(409, "Email has already been taken"));
        }
        if users::username_taken(c, &new.username, None)? {
            return Err(Error::message(409, "Username has already been taken"));
        }
        let u = users::create(c, Some(&admin), new)?;
        admin_view(c, &req, &u)
    })?;
    Ok(Resp::created(body))
}

async fn update(req: Req) -> ApiResult {
    req.admin()?;
    let id = req.path_param("id").to_string();
    let p = &req.params;
    let upd = users::UserUpdate {
        username: p.str("username"),
        name: p.str("name"),
        email: p.str("email"),
        password: p.str("password"),
        admin: p.bool("admin")?,
        external: p.bool("external")?,
        private_profile: p.bool("private_profile")?,
        can_create_group: p.bool("can_create_group")?,
        projects_limit: p.int("projects_limit")?,
        theme_id: p.int("theme_id")?,
        color_scheme_id: p.int("color_scheme_id")?,
        profile: profile_params(&req),
    };
    let body = req.app.db.tx(|c| {
        let u = find::user_by_id(c, &id)?;
        if let Some(e) = &upd.email
            && users::email_taken(c, e, Some(u.id))? {
                return Err(Error::message(409, "Email has already been taken"));
            }
        if let Some(n) = &upd.username
            && users::username_taken(c, n, Some(u.id))? {
                return Err(Error::message(409, "Username has already been taken"));
            }
        let u = users::update(c, &u, upd)?;
        admin_view(c, &req, &u)
    })?;
    Ok(Resp::ok(body))
}

async fn destroy(req: Req) -> ApiResult {
    req.admin()?;
    let id = req.path_param("id").to_string();
    req.app.db.tx(|c| {
        let u = find::user_by_id(c, &id)?;
        users::delete(c, &u)
    })?;
    Ok(Resp::no_content())
}

async fn set_state(req: Req, state: &'static str) -> ApiResult {
    req.admin()?;
    let id = req.path_param("id").to_string();
    req.app.db.tx(|c| {
        let u = find::user_by_id(c, &id)?;
        users::set_state(c, &u, state)?;
        Ok::<_, Error>(())
    })?;
    Ok(Resp::created(Value::Bool(true)))
}

async fn block(req: Req) -> ApiResult {
    set_state(req, "blocked").await
}

async fn unblock(req: Req) -> ApiResult {
    set_state(req, "active").await
}

async fn deactivate(req: Req) -> ApiResult {
    set_state(req, "deactivated").await
}

async fn activate(req: Req) -> ApiResult {
    set_state(req, "active").await
}

fn token_params(
    req: &Req,
) -> Result<(String, Option<String>, Vec<String>, Option<String>), crate::ctx::ApiError> {
    let p = &req.params;
    p.require(&["name", "scopes"])?;
    let scopes = p.list("scopes").unwrap_or_default();
    let expires_at = p.date("expires_at")?;
    if let Some(e) = &expires_at
        && e.as_str() < time::today().as_str() {
            return Err(Error::Validation(vec![(
                "expires_at".into(),
                "must not be in the past".into(),
            )])
            .into());
        }
    Ok((
        p.str("name").unwrap(),
        p.str("description"),
        scopes,
        expires_at,
    ))
}

async fn create_token_for(req: Req, impersonation: bool) -> ApiResult {
    req.admin()?;
    let id = req.path_param("id").to_string();
    let (name, description, scopes, expires_at) = token_params(&req)?;
    let body = req.app.db.tx(|c| {
        let u = find::user_by_id(c, &id)?;
        let expires = expires_at.clone().or_else(|| Some(default_expiry()));
        let (t, value) = tokens::create(
            c,
            &u,
            tokens::NewToken {
                name: &name,
                description: description.as_deref(),
                scopes: &scopes,
                expires_at: expires.as_deref(),
                impersonation,
                value: None,
            },
        )?;
        Ok::<_, Error>(token_entity::with_token(&t, &value))
    })?;
    Ok(Resp::created(body))
}

/// GitLab's default token lifetime when `expires_at` is omitted: 365 days.
fn default_expiry() -> String {
    time::date(time::now() + 365 * 86_400_000)
}

async fn create_pat(req: Req) -> ApiResult {
    create_token_for(req, false).await
}

async fn create_impersonation(req: Req) -> ApiResult {
    create_token_for(req, true).await
}

async fn create_own_pat(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let (name, description, scopes, expires_at) = token_params(&req)?;
    if scopes
        .iter()
        .any(|s| s != "k8s_proxy" && s != "self_rotate")
    {
        return Err(Error::Param("scopes does not have a valid value".into()).into());
    }
    let body = req.app.db.tx(|c| {
        let expires = expires_at.clone().or_else(|| Some(default_expiry()));
        let (t, value) = tokens::create(
            c,
            &me,
            tokens::NewToken {
                name: &name,
                description: description.as_deref(),
                scopes: &scopes,
                expires_at: expires.as_deref(),
                impersonation: false,
                value: None,
            },
        )?;
        Ok::<_, Error>(token_entity::with_token(&t, &value))
    })?;
    Ok(Resp::created(body))
}

async fn list_impersonation(req: Req) -> ApiResult {
    req.admin()?;
    let id = req.path_param("id").to_string();
    let state = req.params.one_of("state", &["all", "active", "inactive"])?;
    let list = req.app.db.read(|c| {
        let u = find::user_by_id(c, &id)?;
        tokens::list_for_user(c, u.id, Some(true))
    })?;
    let list: Vec<Token> = list
        .into_iter()
        .filter(|t| match state.as_deref() {
            Some("active") => t.active(),
            Some("inactive") => !t.active(),
            _ => true,
        })
        .collect();
    paginate(&req, list, &[("state", "all")], |t| {
        Ok(Value::Object(token_entity::token(t)))
    })
}

async fn show_impersonation(req: Req) -> ApiResult {
    req.admin()?;
    let id = req.path_param("id").to_string();
    let tid: i64 = req
        .path_param("token_id")
        .parse()
        .map_err(|_| Error::Param("impersonation_token_id is invalid".into()))?;
    let t = req.app.db.read(|c| {
        let u = find::user_by_id(c, &id)?;
        Token::find(c, tid)?
            .filter(|t| t.user_id == u.id && t.impersonation)
            .ok_or(Error::not_found("Impersonation Token"))
    })?;
    Ok(Resp::ok(Value::Object(token_entity::token(&t))))
}

async fn revoke_impersonation(req: Req) -> ApiResult {
    req.admin()?;
    let id = req.path_param("id").to_string();
    let tid: i64 = req
        .path_param("token_id")
        .parse()
        .map_err(|_| Error::Param("impersonation_token_id is invalid".into()))?;
    req.app.db.tx(|c| {
        let u = find::user_by_id(c, &id)?;
        let t = Token::find(c, tid)?
            .filter(|t| t.user_id == u.id && t.impersonation)
            .ok_or(Error::not_found("Impersonation Token"))?;
        tokens::revoke(c, &t)
    })?;
    Ok(Resp::no_content())
}

async fn list_pats(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let p = &req.params;
    let user_id = p.int("user_id")?;
    if user_id.is_some_and(|u| u != me.id) && !me.admin {
        return Err(Error::Unauthorized(None).into());
    }
    let mut list: Vec<Token> = req.app.db.read(|c| {
        if me.admin && user_id.is_none() {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM personal_access_tokens WHERE impersonation = 0 ORDER BY id",
                Token::COLUMNS
            ))?;
            Ok(st
                .query_map([], Token::from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        } else {
            tokens::list_for_user(c, user_id.unwrap_or(me.id), Some(false))
        }
    })?;
    if let Some(s) = p.one_of("state", &["active", "inactive"])? {
        list.retain(|t| (s == "active") == t.active());
    }
    if let Some(r) = p.bool("revoked")? {
        list.retain(|t| t.revoked == r);
    }
    if let Some(s) = p.str("search") {
        let s = s.to_lowercase();
        list.retain(|t| t.name.to_lowercase().contains(&s));
    }
    paginate(&req, list, &[], |t| Ok(token_entity::with_last_used_ips(t)))
}

async fn pat_self(req: Req) -> ApiResult {
    req.user()?;
    let t = req.token.clone().ok_or(Error::Unauthorized(None))?;
    let t = req
        .app
        .db
        .read(|c| Ok::<_, Error>(Token::find(c, t.id)?.unwrap_or(t)))?;
    Ok(Resp::ok(token_entity::with_last_used_ips(&t)))
}

async fn revoke_self(req: Req) -> ApiResult {
    req.user()?;
    let t = req.token.clone().ok_or(Error::Unauthorized(None))?;
    if t.impersonation {
        return Err(Error::BadRequest(Some("Impersonation token cannot be revoked".into())).into());
    }
    req.app.db.tx(|c| tokens::revoke(c, &t))?;
    Ok(Resp::no_content())
}

async fn show_pat(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id: i64 = req
        .path_param("id")
        .parse()
        .map_err(|_| Error::Param("id is invalid".into()))?;
    let t = req.app.db.read(|c| {
        Token::find(c, id)?
            .filter(|t| !t.impersonation && (me.admin || t.user_id == me.id))
            .ok_or(Error::not_found("Personal Access Token"))
    })?;
    Ok(Resp::ok(token_entity::with_last_used_ips(&t)))
}

async fn revoke_pat(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id: i64 = req
        .path_param("id")
        .parse()
        .map_err(|_| Error::Param("id is invalid".into()))?;
    req.app.db.tx(|c| {
        let t = Token::find(c, id)?
            .filter(|t| !t.impersonation && (me.admin || t.user_id == me.id))
            .ok_or(Error::not_found("Personal Access Token"))?;
        tokens::revoke(c, &t)
    })?;
    Ok(Resp::no_content())
}
