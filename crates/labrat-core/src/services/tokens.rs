use crate::model::{Token, User};
use crate::{Error, Result, time};
use rand::RngExt;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

pub const SCOPES: &[&str] = &[
    "api",
    "read_api",
    "read_user",
    "create_runner",
    "manage_runner",
    "k8s_proxy",
    "read_repository",
    "write_repository",
    "read_registry",
    "write_registry",
    "read_virtual_registry",
    "write_virtual_registry",
    "ai_features",
    "self_rotate",
    "read_service_ping",
    "sudo",
    "admin_mode",
];

pub fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn generate() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::rng();
    let body: String = (0..20)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect();
    format!("glpat-{body}")
}

pub struct NewToken<'a> {
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub scopes: &'a [String],
    pub expires_at: Option<&'a str>,
    pub impersonation: bool,
    /// Use this exact token value (for seeding test instances).
    pub value: Option<&'a str>,
}

/// Creates a token and returns it with its secret value.
pub fn create(c: &Connection, user: &User, new: NewToken) -> Result<(Token, String)> {
    for s in new.scopes {
        if !SCOPES.contains(&s.as_str()) {
            return Err(Error::Param("scopes does not have a valid value".into()));
        }
    }
    let value = new.value.map(str::to_string).unwrap_or_else(generate);
    let now = time::now();
    c.execute(
        "INSERT INTO personal_access_tokens (user_id, name, description, token_digest, scopes, impersonation, expires_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            user.id,
            new.name,
            new.description,
            digest(&value),
            serde_json::to_string(new.scopes).unwrap(),
            new.impersonation,
            new.expires_at,
            now
        ],
    )?;
    let token = Token::find(c, c.last_insert_rowid())?.expect("just inserted");
    Ok((token, value))
}

/// Finds a token by its secret value, whatever its state.
pub fn find_by_value(c: &Connection, value: &str) -> Result<Option<Token>> {
    let id: Option<i64> = c
        .query_row(
            "SELECT id FROM personal_access_tokens WHERE token_digest = ?1",
            [digest(value)],
            |r| r.get(0),
        )
        .ok();
    match id {
        Some(id) => Token::find(c, id),
        None => Ok(None),
    }
}

pub fn touch(c: &Connection, token: &Token) -> Result<()> {
    c.execute(
        "UPDATE personal_access_tokens SET last_used_at = ?1 WHERE id = ?2",
        params![time::now(), token.id],
    )?;
    Ok(())
}

pub fn revoke(c: &Connection, token: &Token) -> Result<()> {
    c.execute(
        "UPDATE personal_access_tokens SET revoked = 1, updated_at = ?1 WHERE id = ?2",
        params![time::now(), token.id],
    )?;
    Ok(())
}

pub fn list_for_user(
    c: &Connection,
    user_id: i64,
    impersonation: Option<bool>,
) -> Result<Vec<Token>> {
    let mut sql = format!(
        "SELECT {} FROM personal_access_tokens WHERE user_id = ?1",
        Token::COLUMNS
    );
    if let Some(imp) = impersonation {
        sql.push_str(if imp {
            " AND impersonation = 1"
        } else {
            " AND impersonation = 0"
        });
    }
    sql.push_str(" ORDER BY id");
    let mut st = c.prepare(&sql)?;
    let v = st
        .query_map([user_id], Token::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}
