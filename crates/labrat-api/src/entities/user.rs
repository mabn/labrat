//! `API::Entities::UserBasic` and its descendants.

use labrat_core::model::User;
use labrat_core::time;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub struct Ctx<'a> {
    pub base_url: &'a str,
    /// Running as EE: users carry EE's extra fields.
    pub ee: bool,
    pub current_user: Option<&'a User>,
}

/// GitLab's default avatar: a Gravatar URL from the SHA-256 of the lowercased email.
pub fn avatar_url(email: &str) -> String {
    let hash = hex::encode(Sha256::digest(email.trim().to_lowercase().as_bytes()));
    format!("https://www.gravatar.com/avatar/{hash}?s=80&d=identicon")
}

/// `UserSafe` + `UserBasic`.
pub fn basic(u: &User, ctx: &Ctx) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(u.id));
    m.insert("username".into(), json!(u.username));
    m.insert("public_email".into(), json!(u.public_email));
    m.insert("name".into(), json!(u.name));
    m.insert("state".into(), json!(u.state));
    m.insert("locked".into(), json!(false));
    m.insert("avatar_url".into(), json!(u.avatar_url()));
    m.insert(
        "web_url".into(),
        json!(format!("{}/{}", ctx.base_url, u.username)),
    );
    m
}

pub fn basic_value(u: &User, ctx: &Ctx) -> Value {
    Value::Object(basic(u, ctx))
}

/// Can `current` see the full profile of `u`? (`:read_user_profile`)
fn can_read_profile(u: &User, ctx: &Ctx) -> bool {
    match ctx.current_user {
        Some(c) if c.id == u.id || c.admin => true,
        _ => !u.private_profile,
    }
}

/// `API::Entities::User`.
pub fn user(u: &User, ctx: &Ctx) -> Map<String, Value> {
    let mut m = basic(u, ctx);
    let profile = can_read_profile(u, ctx);
    if profile {
        m.insert("created_at".into(), json!(time::iso(u.created_at)));
    }
    m.insert("bio".into(), json!(u.bio));
    m.insert("location".into(), json!(u.location));
    m.insert("linkedin".into(), json!(u.linkedin));
    m.insert("twitter".into(), json!(u.twitter));
    m.insert("discord".into(), json!(u.discord));
    m.insert("website_url".into(), json!(u.website_url));
    m.insert("github".into(), json!(u.github));
    m.insert("job_title".into(), json!(u.job_title));
    m.insert("pronouns".into(), json!(u.pronouns));
    m.insert("organization".into(), json!(u.organization));
    m.insert("bot".into(), json!(u.bot()));
    m.insert("work_information".into(), work_information(u));
    // `following_users_allowed?` is false when the viewer is the user themselves.
    let following_allowed = ctx.current_user.is_none_or(|c| c.id != u.id);
    if profile && following_allowed {
        m.insert("followers".into(), json!(0));
        m.insert("following".into(), json!(0));
        if ctx.current_user.is_some() {
            m.insert("is_followed".into(), json!(false));
        }
    }
    m.insert("local_time".into(), Value::Null);
    m
}

fn work_information(u: &User) -> Value {
    match (u.job_title.is_empty(), u.organization.is_empty()) {
        (true, true) => Value::Null,
        (false, true) => json!(u.job_title),
        (true, false) => json!(u.organization),
        (false, false) => json!(format!("{} at {}", u.job_title, u.organization)),
    }
}

/// `API::Entities::UserPublic`.
pub fn public(u: &User, ctx: &Ctx) -> Map<String, Value> {
    let mut m = user(u, ctx);
    m.insert(
        "last_sign_in_at".into(),
        json!(u.last_sign_in_at.map(time::iso)),
    );
    m.insert("confirmed_at".into(), json!(u.confirmed_at.map(time::iso)));
    m.insert("last_activity_on".into(), json!(u.last_activity_on));
    m.insert("email".into(), json!(u.email));
    m.insert("theme_id".into(), json!(u.theme_id));
    m.insert("color_scheme_id".into(), json!(u.color_scheme_id));
    m.insert("projects_limit".into(), json!(u.projects_limit));
    m.insert(
        "current_sign_in_at".into(),
        json!(u.current_sign_in_at.map(time::iso)),
    );
    m.insert("identities".into(), json!([]));
    m.insert("can_create_group".into(), json!(u.can_create_group));
    m.insert("can_create_project".into(), json!(u.projects_limit > 0));
    m.insert("two_factor_enabled".into(), json!(false));
    m.insert("external".into(), json!(u.external));
    m.insert("private_profile".into(), json!(u.private_profile));
    m.insert(
        "commit_email".into(),
        json!(u.commit_email.clone().unwrap_or(u.email.clone())),
    );
    m.insert("preferred_language".into(), json!(u.preferred_language));
    if ctx.ee {
        // The fields EE adds for the user themselves.
        m.insert("shared_runners_minutes_limit".into(), Value::Null);
        m.insert("extra_shared_runners_minutes_limit".into(), Value::Null);
        m.insert("scim_identities".into(), json!([]));
    }
    m
}

/// `API::Entities::UserWithAdmin`. `created_by` is passed in by the caller.
pub fn with_admin(u: &User, ctx: &Ctx, created_by: Option<&User>) -> Map<String, Value> {
    let mut m = public(u, ctx);
    m.insert("is_admin".into(), json!(u.admin));
    m.insert("note".into(), json!(u.note));
    m.insert("namespace_id".into(), json!(u.namespace_id));
    m.insert(
        "created_by".into(),
        created_by
            .map(|c| basic_value(c, ctx))
            .unwrap_or(Value::Null),
    );
    if ctx.ee {
        // The fields EE adds for admins.
        m.insert("using_license_seat".into(), json!(false));
        m.insert("is_auditor".into(), json!(false));
        m.insert("provisioned_by_group_id".into(), Value::Null);
        m.insert("enterprise_group_id".into(), Value::Null);
        m.insert("enterprise_group_associated_at".into(), Value::Null);
    }
    m
}
