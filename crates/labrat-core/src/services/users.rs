use crate::model::{User, access, visibility};
use crate::services::paths;
use crate::{Error, Result, time};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

#[derive(Debug, Default, Clone)]
pub struct NewUser {
    pub username: String,
    pub name: String,
    pub email: String,
    pub password: Option<String>,
    pub admin: bool,
    pub user_type: i64,
    pub external: bool,
    pub private_profile: bool,
    pub can_create_group: Option<bool>,
    pub projects_limit: Option<i64>,
    pub public_email: Option<String>,
    pub skip_confirmation: bool,
    /// Profile fields such as `bio` or `location`, applied after creation.
    pub profile: Vec<(String, Option<String>)>,
}

/// Profile columns that can be set directly from API parameters.
pub const PROFILE_FIELDS: &[&str] = &[
    "bio",
    "location",
    "linkedin",
    "twitter",
    "discord",
    "website_url",
    "github",
    "job_title",
    "organization",
    "pronouns",
    "note",
    "public_email",
    "commit_email",
];

pub fn hash_password(password: &str) -> String {
    let salt: u64 = rand::random();
    let digest = Sha256::digest(format!("{salt:x}:{password}").as_bytes());
    format!("{salt:x}:{}", hex::encode(digest))
}

pub fn check_password(user: &User, password: &str) -> bool {
    let Some(stored) = &user.encrypted_password else {
        return false;
    };
    let Some((salt, hash)) = stored.split_once(':') else {
        return false;
    };
    hex::encode(Sha256::digest(format!("{salt}:{password}").as_bytes())) == hash
}

fn validate_username(
    c: &Connection,
    username: &str,
    except_id: Option<i64>,
) -> Result<Vec<(String, String)>> {
    let mut errs = vec![];
    if username.is_empty() {
        errs.push(("username".into(), "can't be blank".into()));
        return Ok(errs);
    }
    if !paths::valid_namespace_path(username) {
        errs.push(("username".into(), paths::NAMESPACE_PATH_MESSAGE.into()));
    }
    if paths::reserved_top_level(username) {
        errs.push(("username".into(), format!("{username} is a reserved name")));
    }
    let taken: Option<i64> = c
        .query_row(
            "SELECT id FROM users WHERE lower(username) = lower(?1)",
            [username],
            |r| r.get(0),
        )
        .ok();
    if taken.is_some() && taken != except_id {
        errs.push(("username".into(), "has already been taken".into()));
    }
    Ok(errs)
}

pub fn username_taken(c: &Connection, username: &str, except_id: Option<i64>) -> Result<bool> {
    let taken: Option<i64> = c
        .query_row(
            "SELECT id FROM users WHERE lower(username) = lower(?1)",
            [username],
            |r| r.get(0),
        )
        .ok();
    let route: Option<(String, i64)> = c
        .query_row(
            "SELECT source_type, source_id FROM routes WHERE lower(path) = lower(?1)",
            [username],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let route_owned_by_other = match route {
        Some((t, id)) if t == "Namespace" => {
            let owner: Option<i64> = c
                .query_row(
                    "SELECT owner_id FROM namespaces WHERE id = ?1 AND type = 'User'",
                    [id],
                    |r| r.get(0),
                )
                .ok();
            owner.is_none() || owner != except_id
        }
        Some(_) => true,
        None => false,
    };
    Ok((taken.is_some() && taken != except_id) || route_owned_by_other)
}

pub fn email_taken(c: &Connection, email: &str, except_id: Option<i64>) -> Result<bool> {
    let taken: Option<i64> = c
        .query_row(
            "SELECT id FROM users WHERE lower(email) = lower(?1)",
            [email],
            |r| r.get(0),
        )
        .ok();
    Ok(taken.is_some() && taken != except_id)
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty() && !domain.is_empty() && !email.contains(char::is_whitespace)
}

/// `Users::CreateService`: creates the user and their personal namespace.
pub fn create(c: &Connection, actor: Option<&User>, new: NewUser) -> Result<User> {
    let mut errs = validate_username(c, &new.username, None)?;
    if new.name.trim().is_empty() {
        errs.push(("name".into(), "can't be blank".into()));
    }
    if !valid_email(&new.email) {
        errs.push(("email".into(), "is invalid".into()));
    } else if email_taken(c, &new.email, None)? {
        errs.push(("email".into(), "has already been taken".into()));
    }
    if let Some(pw) = &new.password
        && pw.chars().count() < 8 {
            errs.push((
                "password".into(),
                "is too short (minimum is 8 characters)".into(),
            ));
        }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }
    let now = time::now();
    let password = new.password.as_deref().map(hash_password);
    let confirmed_at = if new.skip_confirmation || actor.is_none() {
        Some(now)
    } else {
        None
    };
    c.execute(
        "INSERT INTO users (username, name, email, encrypted_password, admin, user_type, external, private_profile,
            can_create_group, projects_limit, public_email, commit_email, created_by_id, created_at, updated_at, confirmed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?3, ?12, ?13, ?13, ?14)",
        params![
            new.username,
            new.name,
            new.email,
            password,
            new.admin,
            new.user_type,
            new.external,
            new.private_profile,
            new.can_create_group.unwrap_or(true),
            new.projects_limit.unwrap_or(100000),
            new.public_email,
            actor.map(|a| a.id),
            now,
            confirmed_at,
        ],
    )?;
    let id = c.last_insert_rowid();
    for (field, value) in &new.profile {
        if PROFILE_FIELDS.contains(&field.as_str()) {
            set_profile_field(c, id, field, value.as_deref())?;
        }
    }
    let ns_id = create_personal_namespace(c, id, &new.username, &new.name, now)?;
    c.execute(
        "UPDATE users SET namespace_id = ?1 WHERE id = ?2",
        params![ns_id, id],
    )?;
    User::get(c, id)
}

fn set_profile_field(c: &Connection, id: i64, field: &str, value: Option<&str>) -> Result<()> {
    // Text columns that are NOT NULL store an empty string instead of null.
    let not_null = !matches!(field, "pronouns" | "note" | "public_email" | "commit_email");
    let value = if not_null {
        Some(value.unwrap_or(""))
    } else {
        value.filter(|v| !v.is_empty())
    };
    c.execute(
        &format!("UPDATE users SET {field} = ?1 WHERE id = ?2"),
        params![value, id],
    )?;
    Ok(())
}

fn create_personal_namespace(
    c: &Connection,
    user_id: i64,
    username: &str,
    name: &str,
    now: i64,
) -> Result<i64> {
    c.execute(
        "INSERT INTO namespaces (type, name, path, owner_id, visibility_level, created_at, updated_at)
         VALUES ('User', ?1, ?2, ?3, ?4, ?5, ?5)",
        params![name, username, user_id, visibility::PRIVATE, now],
    )?;
    let ns_id = c.last_insert_rowid();
    paths::insert_route(c, "Namespace", ns_id, username, name)?;
    Ok(ns_id)
}

#[derive(Debug, Default, Clone)]
pub struct UserUpdate {
    pub username: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    pub admin: Option<bool>,
    pub external: Option<bool>,
    pub private_profile: Option<bool>,
    pub can_create_group: Option<bool>,
    pub projects_limit: Option<i64>,
    pub theme_id: Option<i64>,
    pub color_scheme_id: Option<i64>,
    pub profile: Vec<(String, Option<String>)>,
}

/// `Users::UpdateService`.
pub fn update(c: &Connection, user: &User, upd: UserUpdate) -> Result<User> {
    let mut errs = vec![];
    if let Some(u) = &upd.username {
        errs.extend(validate_username(c, u, Some(user.id))?);
    }
    if let Some(n) = &upd.name
        && n.trim().is_empty() {
            errs.push(("name".into(), "can't be blank".into()));
        }
    if let Some(e) = &upd.email
        && !valid_email(e) {
            errs.push(("email".into(), "is invalid".into()));
        }
    if let Some(pw) = &upd.password
        && pw.chars().count() < 8 {
            errs.push((
                "password".into(),
                "is too short (minimum is 8 characters)".into(),
            ));
        }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }
    let now = time::now();
    if let Some(username) = &upd.username
        && username != &user.username {
            c.execute(
                "UPDATE users SET username = ?1 WHERE id = ?2",
                params![username, user.id],
            )?;
            if let Some(ns) = user.namespace_id {
                c.execute(
                    "UPDATE namespaces SET path = ?1 WHERE id = ?2",
                    params![username, ns],
                )?;
                paths::rename_routes(c, &user.username, username, &user.name, &user.name)?;
            }
        }
    if let Some(name) = &upd.name {
        c.execute(
            "UPDATE users SET name = ?1 WHERE id = ?2",
            params![name, user.id],
        )?;
        if let Some(ns) = user.namespace_id {
            c.execute(
                "UPDATE namespaces SET name = ?1 WHERE id = ?2",
                params![name, ns],
            )?;
            c.execute(
                "UPDATE routes SET name = ?1 WHERE source_type = 'Namespace' AND source_id = ?2",
                params![name, ns],
            )?;
        }
    }
    if let Some(email) = &upd.email {
        c.execute(
            "UPDATE users SET email = ?1 WHERE id = ?2",
            params![email, user.id],
        )?;
    }
    if let Some(pw) = &upd.password {
        c.execute(
            "UPDATE users SET encrypted_password = ?1 WHERE id = ?2",
            params![hash_password(pw), user.id],
        )?;
    }
    let flags: [(&str, Option<i64>); 7] = [
        ("admin", upd.admin.map(i64::from)),
        ("external", upd.external.map(i64::from)),
        ("private_profile", upd.private_profile.map(i64::from)),
        ("can_create_group", upd.can_create_group.map(i64::from)),
        ("projects_limit", upd.projects_limit),
        ("theme_id", upd.theme_id),
        ("color_scheme_id", upd.color_scheme_id),
    ];
    for (col, v) in flags {
        if let Some(v) = v {
            c.execute(
                &format!("UPDATE users SET {col} = ?1 WHERE id = ?2"),
                params![v, user.id],
            )?;
        }
    }
    for (field, value) in &upd.profile {
        if PROFILE_FIELDS.contains(&field.as_str()) {
            set_profile_field(c, user.id, field, value.as_deref())?;
        }
    }
    c.execute(
        "UPDATE users SET updated_at = ?1 WHERE id = ?2",
        params![now, user.id],
    )?;
    User::get(c, user.id)
}

pub fn set_state(c: &Connection, user: &User, state: &str) -> Result<User> {
    c.execute(
        "UPDATE users SET state = ?1, updated_at = ?2 WHERE id = ?3",
        params![state, time::now(), user.id],
    )?;
    User::get(c, user.id)
}

/// Deletes a user immediately. GitLab does this asynchronously; the visible effect is the same.
pub fn delete(c: &Connection, user: &User) -> Result<()> {
    c.execute("DELETE FROM members WHERE user_id = ?1", [user.id])?;
    if let Some(ns) = user.namespace_id {
        // Personal projects are deleted with the user.
        let projects: Vec<i64> = c
            .prepare("SELECT id FROM projects WHERE namespace_id = ?1")?
            .query_map([ns], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for p in projects {
            crate::services::projects::delete_by_id(c, p)?;
        }
        c.execute(
            "DELETE FROM routes WHERE source_type = 'Namespace' AND source_id = ?1",
            [ns],
        )?;
        c.execute("DELETE FROM namespaces WHERE id = ?1", [ns])?;
    }
    c.execute("DELETE FROM users WHERE id = ?1", [user.id])?;
    Ok(())
}

/// Records sign-in activity the way GitLab does on each authenticated API request.
pub fn touch_activity(c: &Connection, user: &User) -> Result<()> {
    let today = time::today();
    if user.last_activity_on.as_deref() != Some(today.as_str()) {
        c.execute(
            "UPDATE users SET last_activity_on = ?1 WHERE id = ?2",
            params![today, user.id],
        )?;
    }
    Ok(())
}

/// Creates the initial administrator if the instance has no users.
pub fn ensure_root(c: &Connection, password: &str) -> Result<User> {
    if let Some(u) = User::find_by_username(c, "root")? {
        return Ok(u);
    }
    let user = create(
        c,
        None,
        NewUser {
            username: "root".into(),
            name: "Administrator".into(),
            email: "admin@example.com".into(),
            password: Some(password.into()),
            admin: true,
            skip_confirmation: true,
            ..Default::default()
        },
    )?;
    let _ = access::OWNER;
    Ok(user)
}
