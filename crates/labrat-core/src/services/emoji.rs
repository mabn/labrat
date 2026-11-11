//! Award emoji (reactions) on merge requests and notes.

use crate::db::OptionalExt;
use crate::model::User;
use crate::{Error, Result, time};
use rusqlite::{Connection, params};
use serde_json::json;
use std::collections::HashMap;
use std::sync::LazyLock;

const DATA: &str = include_str!("../data/emoji.txt");
const INFO: &str = include_str!("../data/emoji-info.tsv");

/// What `AwardEmojiPresenter` shows for an emoji: the character, its hex codepoints, the
/// description and the Unicode version.
#[derive(Debug, Clone)]
pub struct Info {
    pub moji: &'static str,
    pub hex: &'static str,
    pub description: &'static str,
    pub unicode_version: &'static str,
}

static INFOS: LazyLock<HashMap<&'static str, Info>> = LazyLock::new(|| {
    INFO.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let name = f.next()?;
            Some((name, Info { moji: f.next()?, hex: f.next()?, description: f.next()?, unicode_version: f.next()? }))
        })
        .collect()
});

pub fn info(name: &str) -> Option<&'static Info> {
    INFOS.get(canonical(name)?)
}

/// Alpha code (name or alias) -> canonical name.
static INDEX: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    for line in DATA.lines().filter(|l| !l.starts_with('#')) {
        let mut parts = line.split(' ');
        let Some(name) = parts.next() else { continue };
        m.insert(name, name);
        for a in parts {
            m.entry(a).or_insert(name);
        }
    }
    m
});

/// `TanukiEmoji.find_by_alpha_code(name)&.name`; colons are optional.
pub fn canonical(name: &str) -> Option<&'static str> {
    INDEX.get(name.trim_matches(':')).copied()
}

#[derive(Debug, Clone)]
pub struct Award {
    pub id: i64,
    pub name: String,
    pub user_id: i64,
    pub awardable_type: String,
    pub awardable_id: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Award> {
    Ok(Award {
        id: r.get(0)?,
        name: r.get(1)?,
        user_id: r.get(2)?,
        awardable_type: r.get(3)?,
        awardable_id: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

const COLS: &str = "id, name, user_id, awardable_type, awardable_id, created_at, updated_at";

pub fn list(c: &Connection, kind: &str, id: i64) -> Result<Vec<Award>> {
    let mut st = c.prepare(&format!(
        "SELECT {COLS} FROM award_emoji WHERE awardable_type = ?1 AND awardable_id = ?2 ORDER BY id"
    ))?;
    let v = st
        .query_map(params![kind, id], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn find(c: &Connection, kind: &str, awardable_id: i64, id: i64) -> Result<Option<Award>> {
    c.query_row(
        &format!("SELECT {COLS} FROM award_emoji WHERE awardable_type = ?1 AND awardable_id = ?2 AND id = ?3"),
        params![kind, awardable_id, id],
        from_row,
    )
    .optional()
}

/// `AwardEmojis::AddService`. Errors carry GitLab's `errors.to_sentence` text.
pub fn add(
    c: &Connection,
    user: &User,
    kind: &str,
    id: i64,
    name: &str,
    system_note: bool,
) -> Result<std::result::Result<Award, String>> {
    let Some(canon) = canonical(name) else {
        if name.is_empty() {
            return Ok(Err("Name can't be blank and Name is not a valid emoji name".into()));
        }
        return Ok(Err("Name is not a valid emoji name".into()));
    };
    if system_note {
        return Ok(Err("Awardable cannot add emoji reactions".into()));
    }
    let exists: Option<i64> = c
        .query_row(
            "SELECT id FROM award_emoji WHERE user_id = ?1 AND awardable_type = ?2 AND awardable_id = ?3 AND name = ?4",
            params![user.id, kind, id, canon],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_some() {
        return Ok(Err("Name has already been taken".into()));
    }
    let now = time::now();
    c.execute(
        "INSERT INTO award_emoji (name, user_id, awardable_type, awardable_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![canon, user.id, kind, id, now],
    )?;
    let award_id = c.last_insert_rowid();
    if kind == "Note" {
        c.execute(
            "UPDATE notes SET updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
    }
    // `TodoService#new_award_emoji`: reacting resolves the user's to-dos on the MR.
    if kind == "MergeRequest" {
        c.execute(
            "UPDATE todos SET state = 'done', snoozed_until = NULL, updated_at = ?1 WHERE user_id = ?2 AND target_type = 'MergeRequest' AND target_id = ?3 AND state = 'pending'",
            params![now, user.id, id],
        )?;
    }
    let award = Award {
        id: award_id,
        name: canon.to_string(),
        user_id: user.id,
        awardable_type: kind.into(),
        awardable_id: id,
        created_at: now,
        updated_at: now,
    };
    Ok(Ok(award))
}

pub fn remove(c: &Connection, award: &Award) -> Result<()> {
    c.execute("DELETE FROM award_emoji WHERE id = ?1", [award.id])?;
    if award.awardable_type == "Note" {
        c.execute(
            "UPDATE notes SET updated_at = ?1 WHERE id = ?2",
            params![time::now(), award.awardable_id],
        )?;
    }
    Ok(())
}

/// `/react` toggles: adds the award or removes the user's existing one.
/// Adds or removes the award; returns it and whether it was added.
pub fn toggle(c: &Connection, user: &User, kind: &str, id: i64, name: &str) -> Result<Option<(Award, bool)>> {
    let Some(canon) = canonical(name) else {
        return Err(Error::Api(400, json!("Name is not a valid emoji name")));
    };
    let existing: Option<i64> = c
        .query_row(
            "SELECT id FROM award_emoji WHERE user_id = ?1 AND awardable_type = ?2 AND awardable_id = ?3 AND name = ?4",
            params![user.id, kind, id, canon],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(aid) => {
            let award = list(c, kind, id)?.into_iter().find(|a| a.id == aid);
            c.execute("DELETE FROM award_emoji WHERE id = ?1", [aid])?;
            Ok(award.map(|a| (a, false)))
        }
        None => Ok(add(c, user, kind, id, canon, false)?.ok().map(|a| (a, true))),
    }
}
