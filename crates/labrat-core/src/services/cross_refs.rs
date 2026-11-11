//! Cross-reference system notes (`Mentionable#create_cross_references!`): commits pushed with
//! messages that reference merge requests add "mentioned in commit <sha>" to those MRs.

use crate::model::{Project, User};
use crate::services::merge_requests::MergeRequest;
use crate::services::notes;
use crate::{Config, Result, repo};
use labrat_git::Oid;
use labrat_git::protocol::RefUpdate;
use rusqlite::{Connection, params};

/// MR iids of `p` referenced in `text`: `!1`, `group/project!1`, or an MR URL.
pub fn referenced_iids(config: &Config, p: &Project, text: &str) -> Vec<i64> {
    let mut out: Vec<i64> = vec![];
    let mut push = |n: i64| {
        if !out.contains(&n) {
            out.push(n);
        }
    };
    let digits = |s: &str| -> Option<i64> {
        let d: String = s.chars().take_while(char::is_ascii_digit).collect();
        if d.is_empty() { None } else { d.parse().ok() }
    };
    let url_prefix = format!("{}/{}/-/merge_requests/", config.external_url.trim_end_matches('/'), p.full_path);
    let mut rest = text;
    while let Some(i) = rest.find(&url_prefix) {
        if let Some(n) = digits(&rest[i + url_prefix.len()..]) {
            push(n);
        }
        rest = &rest[i + url_prefix.len()..];
    }
    let bytes = text.as_bytes();
    for (i, ch) in text.char_indices() {
        if ch != '!' {
            continue;
        }
        let Some(n) = digits(&text[i + 1..]) else { continue };
        // What precedes the `!`: nothing/space/punctuation (local), or this project's path.
        let before = &text[..i];
        let prev = before.chars().last();
        let local = match prev {
            None => true,
            Some(c) => !(c.is_alphanumeric() || c == '_' || c == '/' || c == '-' || c == '.' || c == '!' || c == '&'),
        };
        let full = before.ends_with(&p.full_path) && {
            let start = before.len() - p.full_path.len();
            start == 0 || !matches!(bytes[start - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'/' | b'-' | b'.')
        };
        if local || full {
            push(n);
        }
    }
    out
}

/// Adds "mentioned in commit <sha>" notes for commits newly pushed to a branch.
pub fn on_push(c: &Connection, config: &Config, p: &Project, user: &User, u: &RefUpdate) -> Result<()> {
    if u.new.is_zero() || !u.name.starts_with("refs/heads/") {
        return Ok(());
    }
    let _ = config;
    // `ProcessCommitWorker` runs after the push: the notes appear asynchronously.
    let commits: Vec<String> = crate::services::hooks::push_commits(c, p, u)?.iter().map(|o| o.hex()).collect();
    if !commits.is_empty() {
        crate::jobs::enqueue(c, "commit_cross_references", serde_json::json!({"project_id": p.id, "user_id": user.id, "commits": commits}))?;
    }
    Ok(())
}

/// The `commit_cross_references` job.
pub fn process_commits(c: &Connection, config: &Config, payload: &serde_json::Value) -> Result<()> {
    let Some(p) = Project::find(c, payload["project_id"].as_i64().unwrap_or(0))? else { return Ok(()) };
    let Some(user) = User::find(c, payload["user_id"].as_i64().unwrap_or(0))? else { return Ok(()) };
    let r = repo::repo(c, &p);
    for hex in payload["commits"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
        let Ok(oid) = Oid::from_hex(hex) else { continue };
        let commit = r.commit(&oid)?;
        for iid in referenced_iids(config, &p, &commit.message) {
            commit_mentions_mr(c, &p, &user, &oid, iid)?;
        }
    }
    Ok(())
}

fn commit_mentions_mr(c: &Connection, p: &Project, author: &User, oid: &Oid, iid: i64) -> Result<()> {
    let Some(mr) = MergeRequest::find_by_iid(c, p.id, iid)? else {
        return Ok(());
    };
    let sha = oid.hex();
    let exists: i64 = c.query_row(
        "SELECT count(*) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND system = 1 AND note LIKE ?2",
        params![mr.id, format!("mentioned in commit %{sha}")],
        |r| r.get(0),
    )?;
    if exists > 0 {
        return Ok(());
    }
    notes::cross_reference_note(c, mr.id, p.id, author, &format!("mentioned in commit {sha}"))?;
    Ok(())
}

/// `Mentionable#create_cross_references!` (or `create_new_cross_references!` with the text
/// before an edit): merge requests of `p` referenced in `text` by merge request `from` get
/// "mentioned in merge request !N", once per mentioner, without being touched.
/// Commits of `p` referenced in `text` (`CommitReferenceFilter`): a 7-40 hex SHA standing as a
/// word, optionally as `group/project@sha` for this project, or a commit URL of this project.
pub fn referenced_commits(c: &Connection, config: &Config, p: &Project, text: &str) -> Result<Vec<Oid>> {
    let r = repo::repo(c, p);
    let mut out: Vec<Oid> = vec![];
    let mut candidates: Vec<String> = vec![];
    let url_prefix = format!("{}/{}/-/commit/", config.external_url.trim_end_matches('/'), p.full_path);
    let mut rest = text;
    while let Some(i) = rest.find(&url_prefix) {
        let sha: String = rest[i + url_prefix.len()..].chars().take_while(char::is_ascii_hexdigit).collect();
        candidates.push(sha);
        rest = &rest[i + url_prefix.len()..];
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut k = 0;
    while k < chars.len() {
        let (i, ch) = chars[k];
        if !ch.is_ascii_hexdigit() || (k > 0 && word(chars[k - 1].1)) {
            k += 1;
            continue;
        }
        let mut e = k;
        while e < chars.len() && chars[e].1.is_ascii_hexdigit() {
            e += 1;
        }
        let len = e - k;
        let after_ok = e == chars.len() || !word(chars[e].1);
        if (7..=40).contains(&len) && after_ok {
            let prev = if k > 0 { Some(chars[k - 1].1) } else { None };
            let local = match prev {
                None => true,
                // Inside a URL or path (`/`), or a reference of another project (`x@`).
                Some('/') | Some('.') | Some('-') => false,
                Some('@') => text[..i - 1].ends_with(&p.full_path),
                Some(_) => true,
            };
            if local {
                candidates.push(text[i..chars.get(e).map(|x| x.0).unwrap_or(text.len())].to_string());
            }
        }
        k = e.max(k + 1);
    }
    for sha in candidates {
        if sha.len() < 7 {
            continue;
        }
        if let Some(o) = r.resolve(&sha)?
            && r.commit(&o).is_ok()
            && !out.contains(&o)
        {
            out.push(o);
        }
    }
    Ok(out)
}

pub fn mr_mentions(c: &Connection, config: &Config, p: &Project, author: &User, from: &MergeRequest, text: &str, before: Option<&str>) -> Result<()> {
    // Commits mentioned get "mentioned in merge request !N" on their own discussions.
    let old_commits = match before {
        Some(b) => referenced_commits(c, config, p, b)?,
        None => vec![],
    };
    for o in referenced_commits(c, config, p, text)? {
        if old_commits.contains(&o) {
            continue;
        }
        let body = format!("mentioned in merge request !{}", from.iid);
        let exists: i64 = c.query_row(
            "SELECT count(*) FROM notes WHERE noteable_type = 'Commit' AND commit_id = ?1 AND project_id = ?2 AND system = 1 AND note = ?3",
            params![o.hex(), p.id, body],
            |r| r.get(0),
        )?;
        if exists > 0 {
            continue;
        }
        notes::insert(
            c,
            author,
            notes::NewNote {
                note: body,
                noteable_type: "Commit".into(),
                noteable_id: 0,
                project_id: Some(p.id),
                system: true,
                system_action: Some("cross_reference".into()),
                commit_id: Some(o.hex()),
                skip_touch: true,
                ..Default::default()
            },
        )?;
    }
    let old = before.map(|b| referenced_iids(config, p, b)).unwrap_or_default();
    for iid in referenced_iids(config, p, text) {
        if iid == from.iid || old.contains(&iid) {
            continue;
        }
        let Some(target) = MergeRequest::find_by_iid(c, p.id, iid)? else { continue };
        let body = format!("mentioned in merge request !{}", from.iid);
        // `cross_reference_exists?`
        let exists: i64 = c.query_row(
            "SELECT count(*) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND system = 1 AND note = ?2",
            params![target.id, body],
            |r| r.get(0),
        )?;
        if exists > 0 {
            continue;
        }
        notes::cross_reference_note(c, target.id, p.id, author, &body)?;
    }
    Ok(())
}
