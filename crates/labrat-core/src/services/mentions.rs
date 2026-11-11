//! `@user` mentions in notes: `directly_addressed` and `mentioned` to-dos.

use crate::Result;
use crate::model::{Project, User};
use crate::perms::ProjectPerms;
use crate::services::merge_requests::MergeRequest;
use crate::services::notes::Note;
use crate::services::todos::{self, action};
use rusqlite::Connection;

/// The text without code: fenced blocks and inline code spans don't mention anyone.
fn without_code(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            out.push('\n');
            continue;
        }
        if fenced {
            out.push('\n');
            continue;
        }
        let mut in_code = false;
        for ch in line.chars() {
            if ch == '`' {
                in_code = !in_code;
                out.push(' ');
            } else if in_code {
                out.push(' ');
            } else {
                out.push(ch);
            }
        }
    }
    out
}

fn usernames(text: &str) -> Vec<String> {
    let text = &without_code(text);
    let mut out = vec![];
    for (i, _) in text.match_indices('@') {
        if i > 0
            && text[..i]
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric())
        {
            continue;
        }
        let name: String = text[i + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            .collect();
        let name = name.trim_end_matches('.').to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The users a mention stands for: the user with that username, or the direct members of the
/// group with that path (`GroupMember.of_groups`).
pub fn users_for_mention(c: &Connection, name: &str) -> Result<Vec<User>> {
    if let Some(u) = User::find_by_username(c, name)? {
        return Ok(vec![u]);
    }
    let Some(g) = crate::model::Namespace::find_by_full_path(c, name)? else { return Ok(vec![]) };
    if !g.is_group() {
        return Ok(vec![]);
    }
    let mut out: Vec<User> = vec![];
    for m in crate::services::members::list(c, "Namespace", g.id)? {
        if !out.iter().any(|u| u.id == m.user_id)
            && let Some(u) = User::find(c, m.user_id)?
        {
            out.push(u);
        }
    }
    Ok(out)
}

/// Users mentioned in `text` who can read the MR, with the mention that named each. GitLab
/// doesn't leave the author out: mentioning yourself, or a group you belong to, makes a to-do
/// for yourself too.
fn mentioned_users(c: &Connection, p: &Project, text: &str, author: &User) -> Result<Vec<(User, String)>> {
    let _ = author;
    let mut out: Vec<(User, String)> = vec![];
    for name in usernames(text) {
        for u in users_for_mention(c, &name)? {
            if out.iter().any(|(x, _)| x.id == u.id) {
                continue;
            }
            if ProjectPerms::load(c, Some(&u), p)?.read_merge_request(p) {
                out.push((u, name.clone()));
            }
        }
    }
    Ok(out)
}

/// Users mentioned in `text` who can read the MR (excluding `author`).
pub fn mentioned_user_ids(c: &Connection, p: &Project, text: &str, author: &User) -> Result<Vec<i64>> {
    Ok(mentioned_users(c, p, text, author)?.into_iter().map(|(u, _)| u.id).collect())
}

/// `create_mention_todos`: users addressed at the start of the text (or of a line) get
/// `directly_addressed`, others `mentioned`; users in `skip` get nothing.
pub fn text_todos(c: &Connection, p: &Project, mr: &MergeRequest, author: &User, text: &str, note_id: Option<i64>, skip: &[i64]) -> Result<()> {
    let plain = without_code(text);
    let mut direct = vec![];
    for line in plain.lines() {
        let t = line.trim_start();
        if t.starts_with('@') {
            for word in t.split_whitespace() {
                match word.strip_prefix('@') {
                    Some(n) => direct.push(n.trim_end_matches([',', ':']).to_string()),
                    None => break,
                }
            }
        }
    }
    let users = mentioned_users(c, p, text, author)?;
    // Directly addressed first, then the other mentions.
    for want_direct in [true, false] {
        for (u, via) in &users {
            if skip.contains(&u.id) {
                continue;
            }
            if direct.contains(via) != want_direct {
                continue;
            }
            let a = if want_direct { action::DIRECTLY_ADDRESSED } else { action::MENTIONED };
            todos::create(c, u.id, author, p, mr, a, note_id)?;
        }
    }
    Ok(())
}

/// To-dos for the mentions in a new or edited note.
pub fn note_todos(c: &Connection, p: &Project, mr: &MergeRequest, author: &User, note: &Note) -> Result<()> {
    note_todos_skipping(c, p, mr, author, note, &[])
}

pub fn note_todos_skipping(c: &Connection, p: &Project, mr: &MergeRequest, author: &User, note: &Note, skip: &[i64]) -> Result<()> {
    if note.system {
        return Ok(());
    }
    text_todos(c, p, mr, author, &note.note, Some(note.id), skip)
}

/// Mention to-dos for a comment on a commit (users who can read the code).
pub fn commit_note_todos(c: &Connection, p: &Project, sha: &str, author: &User, note: &Note) -> Result<()> {
    if note.system {
        return Ok(());
    }
    let plain = without_code(&note.note);
    let mut direct = vec![];
    for line in plain.lines() {
        let t = line.trim_start();
        if t.starts_with('@') {
            for word in t.split_whitespace() {
                match word.strip_prefix('@') {
                    Some(n) => direct.push(n.trim_end_matches([',', ':']).to_string()),
                    None => break,
                }
            }
        }
    }
    for name in usernames(&note.note) {
        let Some(u) = User::find_by_username(c, &name)? else { continue };
        if u.id == author.id || !ProjectPerms::load(c, Some(&u), p)?.read_code(p) {
            continue;
        }
        let a = if direct.contains(&name) { action::DIRECTLY_ADDRESSED } else { action::MENTIONED };
        todos::create_for_commit(c, u.id, author, p, sha, a, Some(note.id))?;
    }
    Ok(())
}

/// Usernames @mentioned in `text` (outside code), in order.
pub fn mentioned_usernames(text: &str) -> Vec<String> {
    usernames(text)
}
