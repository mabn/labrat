//! Project access tokens: each token belongs to a bot user that is a member of the project
//! (`ResourceAccessTokens::CreateService`).

use crate::model::{Project, User};
use crate::services::{members, tokens, users};
use crate::{Config, Result};
use rand::RngExt;
use rusqlite::{Connection, params};

pub const PROJECT_BOT: i64 = 6;

/// The host part of the external URL, for the bot's `noreply` email.
fn host(config: &Config) -> String {
    let rest = config.external_url.split_once("://").map(|(_, r)| r).unwrap_or(&config.external_url);
    rest.split(['/', ':']).next().unwrap_or("localhost").to_string()
}

pub struct NewProjectToken<'a> {
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub scopes: &'a [String],
    pub access_level: i64,
    pub expires_at: Option<&'a str>,
}

/// What a token belongs to: `("project", project id)` or `("group", namespace id)`.
#[derive(Clone, Copy)]
pub struct Resource<'a> {
    pub kind: &'a str,
    pub id: i64,
}

impl Resource<'_> {
    fn member_source(&self) -> &'static str {
        if self.kind == "group" { "Namespace" } else { "Project" }
    }
}

/// Creates a project token: see `create_for`.
pub fn create(c: &Connection, config: &Config, p: &Project, actor: &User, new: NewProjectToken) -> Result<(i64, String)> {
    create_for(c, config, Resource { kind: "project", id: p.id }, actor, new)
}

/// Creates the bot user, makes it a member and issues its token. Returns the token id and value.
pub fn create_for(c: &Connection, config: &Config, res: Resource, actor: &User, new: NewProjectToken) -> Result<(i64, String)> {
    let mut rng = rand::rng();
    let hash: String = (0..32).map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap()).collect();
    let username = format!("{}_{}_bot_{hash}", res.kind, res.id);
    let bot = users::create(
        c,
        Some(actor),
        users::NewUser {
            username: username.clone(),
            name: new.name.to_string(),
            email: format!("{username}@noreply.{}", host(config)),
            password: None,
            admin: false,
            user_type: PROJECT_BOT,
            external: false,
            private_profile: false,
            can_create_group: None,
            projects_limit: None,
            public_email: None,
            skip_confirmation: true,
            profile: vec![],
        },
    )?;
    // The bot's membership has no creator.
    members::add(c, None, res.member_source(), res.id, bot.id, new.access_level, None)?;
    issue(c, &bot, res, new)
}

fn issue(c: &Connection, bot: &User, res: Resource, new: NewProjectToken) -> Result<(i64, String)> {
    let (token, value) = tokens::create(
        c,
        bot,
        tokens::NewToken { name: new.name, description: new.description, scopes: new.scopes, expires_at: new.expires_at, impersonation: false, value: None },
    )?;
    c.execute(
        "UPDATE personal_access_tokens SET resource_type = ?1, resource_id = ?2, access_level = ?3 WHERE id = ?4",
        params![res.kind, res.id, new.access_level, token.id],
    )?;
    Ok((token.id, value))
}

/// Ids of a resource's tokens, oldest first.
pub fn ids_for(c: &Connection, res: Resource) -> Result<Vec<i64>> {
    let mut st = c.prepare("SELECT id FROM personal_access_tokens WHERE resource_type = ?1 AND resource_id = ?2 ORDER BY id")?;
    let v = st.query_map(params![res.kind, res.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

pub fn ids(c: &Connection, project_id: i64) -> Result<Vec<i64>> {
    ids_for(c, Resource { kind: "project", id: project_id })
}

/// `PersonalAccessTokens::RotateService`: revokes the token and issues a new one like it.
pub fn rotate(c: &Connection, token_id: i64, expires_at: Option<&str>) -> Result<(i64, String)> {
    let token = crate::model::Token::find(c, token_id)?.ok_or(crate::Error::not_found("Token"))?;
    let (kind, rid, level): (String, i64, i64) =
        c.query_row("SELECT resource_type, resource_id, access_level FROM personal_access_tokens WHERE id = ?1", [token_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let bot = User::get(c, token.user_id)?;
    tokens::revoke(c, &token)?;
    issue(
        c,
        &bot,
        Resource { kind: &kind, id: rid },
        NewProjectToken { name: &token.name, description: token.description.as_deref(), scopes: &token.scopes, access_level: level, expires_at },
    )
}
