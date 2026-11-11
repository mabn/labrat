//! Path validation and the `routes` table.

use crate::db::OptionalExt;
use crate::{Error, Result};
use rusqlite::{Connection, params};

/// Top-level paths GitLab reserves (subset of `Gitlab::PathRegex::TOP_LEVEL_ROUTES`).
const RESERVED_TOP_LEVEL: &[&str] = &[
    "-",
    ".well-known",
    "404.html",
    "422.html",
    "500.html",
    "502.html",
    "503.html",
    "admin",
    "api",
    "apple-touch-icon.png",
    "assets",
    "dashboard",
    "deploy.html",
    "explore",
    "favicon.ico",
    "favicon.png",
    "files",
    "groups",
    "health_check",
    "help",
    "import",
    "jwt",
    "login",
    "oauth",
    "profile",
    "projects",
    "public",
    "robots.txt",
    "s",
    "search",
    "sitemap",
    "sitemap.xml",
    "sitemap.xml.gz",
    "slash-command-logo.png",
    "snippets",
    "unsubscribes",
    "uploads",
    "users",
    "v2",
];

/// Paths that can't be used inside a namespace (subset of `PROJECT_WILDCARD_ROUTES`).
const RESERVED_PROJECT: &[&str] = &[
    "-",
    "badges",
    "blame",
    "blob",
    "builds",
    "commits",
    "create",
    "create_dir",
    "edit",
    "environments/folders",
    "files",
    "find_file",
    "gitlab-lfs/objects",
    "info/lfs/objects",
    "new",
    "preview",
    "raw",
    "refs",
    "tree",
    "update",
    "wikis",
];

fn path_chars_ok(path: &str) -> bool {
    let mut chars = path.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphanumeric() || matches!(first, '_' | '.'))
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        && path.len() <= 255
}

fn no_bad_suffix(path: &str) -> bool {
    !(path.ends_with(".git") || path.ends_with(".atom"))
}

/// `Gitlab::PathRegex.namespace_format_regex`.
pub fn valid_namespace_path(path: &str) -> bool {
    path_chars_ok(path) && !path.ends_with('.') && no_bad_suffix(path)
}

/// `Gitlab::PathRegex.project_path_format_regex`.
pub fn valid_project_path(path: &str) -> bool {
    path_chars_ok(path) && no_bad_suffix(path)
}

pub fn reserved_top_level(path: &str) -> bool {
    RESERVED_TOP_LEVEL.contains(&path.to_ascii_lowercase().as_str())
}

pub fn reserved_in_namespace(path: &str) -> bool {
    RESERVED_PROJECT.contains(&path.to_ascii_lowercase().as_str())
}

pub const NAMESPACE_PATH_MESSAGE: &str = "can contain only letters, digits, '_', '-' and '.'. Cannot start with '-' or end in '.', '.git' or '.atom'.";
pub const PROJECT_PATH_MESSAGE: &str = "can contain only letters, digits, '_', '-' and '.'. Cannot start with '-', end in '.git' or end in '.atom'";

pub fn route_taken(c: &Connection, full_path: &str) -> Result<bool> {
    let n: Option<i64> = c
        .query_row(
            "SELECT id FROM routes WHERE lower(path) = lower(?1)",
            [full_path],
            |r| r.get(0),
        )
        .optional()?;
    Ok(n.is_some())
}

pub fn insert_route(
    c: &Connection,
    source_type: &str,
    source_id: i64,
    path: &str,
    name: &str,
) -> Result<()> {
    c.execute(
        "INSERT INTO routes (source_type, source_id, path, name) VALUES (?1, ?2, ?3, ?4)",
        params![source_type, source_id, path, name],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Error::validation("path", "has already been taken")
        }
        e => e.into(),
    })?;
    Ok(())
}

/// Rewrites routes under `old_prefix` after a rename or transfer.
pub fn rename_routes(
    c: &Connection,
    old_prefix: &str,
    new_prefix: &str,
    old_name: &str,
    new_name: &str,
) -> Result<()> {
    let mut st = c.prepare("SELECT id, path, name FROM routes WHERE lower(path) = lower(?1) OR lower(path) LIKE lower(?1) || '/%'")?;
    let rows = st
        .query_map([old_prefix], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, path, name) in rows {
        let new_path = format!("{new_prefix}{}", &path[old_prefix.len()..]);
        let new_name = if name.starts_with(old_name) {
            format!("{new_name}{}", &name[old_name.len()..])
        } else {
            name
        };
        c.execute(
            "UPDATE routes SET path = ?1, name = ?2 WHERE id = ?3",
            params![new_path, new_name, id],
        )?;
    }
    Ok(())
}

/// Turns a name into a path the way GitLab's frontend and `Namespace.clean_path` do.
pub fn path_from_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() {
            out.push('-');
        }
    }
    out
}
