//! Suggestions in diff notes (```` ```suggestion:-N+M ```` blocks) and applying them.

use crate::db::OptionalExt;
use crate::model::{Project, User};
use crate::services::merge_requests::MergeRequest;
use crate::services::notes::Note;
use crate::services::positions::Position;
use crate::{Config, Result, repo};
use rusqlite::{Connection, Row, params};

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub id: i64,
    pub note_id: i64,
    pub relative_order: i64,
    pub from_content: String,
    pub to_content: String,
    pub lines_above: i64,
    pub lines_below: i64,
    pub applied: bool,
    pub commit_id: Option<String>,
    pub outdated: bool,
}

const COLS: &str = "id, note_id, relative_order, from_content, to_content, lines_above, lines_below, applied, commit_id, outdated";

fn row(r: &Row) -> rusqlite::Result<Suggestion> {
    Ok(Suggestion {
        id: r.get(0)?,
        note_id: r.get(1)?,
        relative_order: r.get(2)?,
        from_content: r.get(3)?,
        to_content: r.get(4)?,
        lines_above: r.get(5)?,
        lines_below: r.get(6)?,
        applied: r.get(7)?,
        commit_id: r.get(8)?,
        outdated: r.get(9)?,
    })
}

pub fn for_note(c: &Connection, note_id: i64) -> Result<Vec<Suggestion>> {
    let mut st = c.prepare(&format!(
        "SELECT {COLS} FROM suggestions WHERE note_id = ?1 ORDER BY relative_order"
    ))?;
    let v = st
        .query_map([note_id], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn find(c: &Connection, id: i64) -> Result<Option<Suggestion>> {
    c.query_row(
        &format!("SELECT {COLS} FROM suggestions WHERE id = ?1"),
        [id],
        row,
    )
    .optional()
}

/// Blocks of `suggestion` code fences: (above, below, content).
fn parse_blocks(body: &str) -> Vec<(i64, i64, String)> {
    let mut out = vec![];
    let mut lines = body.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let t = line.trim_end_matches(['\n', '\r']);
        let trimmed = t.trim_start();
        let fence = if trimmed.starts_with("```") {
            "```"
        } else if trimmed.starts_with("~~~") {
            "~~~"
        } else {
            continue;
        };
        let info = trimmed
            .trim_start_matches(fence[..1].chars().next().unwrap())
            .trim();
        let (lang, params) = info.split_once(':').unwrap_or((info, ""));
        if lang != "suggestion" {
            // Skip over other fenced blocks.
            for l in lines.by_ref() {
                if l.trim().starts_with(fence) {
                    break;
                }
            }
            continue;
        }
        let (mut above, mut below) = (0, 0);
        let p = params.trim();
        let re_ok = {
            let mut s = p;
            let mut ok = true;
            if let Some(r) = s.strip_prefix('-') {
                let n: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
                if n.is_empty() {
                    ok = false;
                } else {
                    above = n.parse().unwrap_or(0);
                    s = &r[n.len()..];
                }
            }
            if ok
                && let Some(r) = s.strip_prefix('+') {
                    let n: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if n.is_empty() {
                        ok = false;
                    } else {
                        below = n.parse().unwrap_or(0);
                        s = &r[n.len()..];
                    }
                }
            ok && s.is_empty()
        };
        if !re_ok {
            above = 0;
            below = 0;
        }
        let mut content = String::new();
        for l in lines.by_ref() {
            if l.trim().starts_with(fence) {
                break;
            }
            content.push_str(l);
        }
        out.push((above, below, content));
    }
    out
}

/// `Suggestions::CreateService` for a diff note on a non-removed text line.
pub fn create_for_note(c: &Connection, p: &Project, mr: &MergeRequest, note: &Note) -> Result<()> {
    if note.kind.as_deref() != Some("DiffNote") || note.system {
        return Ok(());
    }
    let Some(pos) = note.position.as_deref().and_then(Position::parse) else {
        return Ok(());
    };
    if !pos.on_text() || pos.new_line.is_none() {
        return Ok(());
    }
    let r = repo::repo(c, p);
    let Some(blob) = pos.new_blob(&r)? else {
        return Ok(());
    };
    let text = utf8_dropping_invalid(&blob);
    let lines: Vec<&str> = text
        .as_str()
        .split_inclusive('\n')
        .collect();
    let target = pos.new_line.unwrap();
    for (i, (above, below, content)) in parse_blocks(&note.note).into_iter().enumerate() {
        let from = (target - above.min(100)).max(1);
        let to = target + below.min(100);
        let from_content: String = lines
            .iter()
            .skip((from - 1) as usize)
            .take((to - from + 1).max(0) as usize)
            .copied()
            .collect();
        let ending = ["\r\n", "\n", "\r"]
            .iter()
            .find(|e| from_content.ends_with(*e))
            .copied()
            .unwrap_or("");
        let to_content = format!(
            "{}{}",
            content.trim_end_matches(['\n', '\r']),
            if content.is_empty() { "" } else { ending }
        );
        // `@text.blank?`: a blank suggestion replaces the lines with nothing.
        let to_content = if content.trim().is_empty() {
            String::new()
        } else {
            to_content
        };
        c.execute(
            "INSERT INTO suggestions (note_id, relative_order, from_content, to_content, lines_above, lines_below) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![note.id, i as i64, from_content, to_content, above, below],
        )?;
    }
    let _ = mr;
    Ok(())
}

pub fn recreate_for_note(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    note: &Note,
) -> Result<()> {
    c.execute("DELETE FROM suggestions WHERE note_id = ?1", [note.id])?;
    create_for_note(c, p, mr, note)
}

/// `from_line`, `to_line` for the API entity.
pub fn line_range(s: &Suggestion, note: &Note) -> (i64, i64) {
    let target = note
        .position
        .as_deref()
        .and_then(Position::parse)
        .and_then(|p| p.new_line)
        .unwrap_or(1);
    (
        (target - s.lines_above.min(100)).max(1),
        target + s.lines_below.min(100),
    )
}

/// `Suggestion#inapplicable_reason` (cached variant).
pub fn inapplicable_reason(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    note: &Note,
    s: &Suggestion,
) -> Result<Option<String>> {
    if s.applied {
        return Ok(Some("Can't apply this suggestion.".into()));
    }
    if mr.state == "merged" {
        return Ok(Some(
            "This merge request was merged. To apply this suggestion, edit this file directly."
                .into(),
        ));
    }
    if mr.state == "closed" {
        return Ok(Some(
            "This merge request is closed. To apply this suggestion, edit this file directly."
                .into(),
        ));
    }
    if repo::repo(c, p).branch(&mr.source_branch)?.is_none() {
        return Ok(Some("Can't apply as the source branch was deleted.".into()));
    }
    let active = match (
        note.position.as_deref().and_then(Position::parse),
        mr.latest_diff(c)?,
    ) {
        (Some(pos), Some(d)) => pos.head_sha == d.head_commit_sha,
        _ => false,
    };
    if s.outdated || !active {
        return Ok(Some(if s.lines_above > 0 || s.lines_below > 0 {
            "Can't apply as these lines were changed in a more recent version.".into()
        } else {
            "Can't apply as this line was changed in a more recent version.".into()
        }));
    }
    if s.from_content == s.to_content {
        return Ok(Some("This suggestion already matches its content.".into()));
    }
    let path = note
        .position
        .as_deref()
        .and_then(Position::parse)
        .and_then(|p| p.new_path)
        .unwrap_or_default();
    if path.ends_with("ipynb") {
        return Ok(Some("This file was modified for readability, and can't accept suggestions. Edit it directly.".into()));
    }
    Ok(None)
}

/// `Suggestions::ApplyService` for one or more suggestions. `Err` carries the API message.
pub fn apply(
    c: &Connection,
    config: &Config,
    user: &User,
    ids: &[i64],
    message: Option<&str>,
) -> Result<std::result::Result<(), String>> {
    let mut items = vec![];
    for id in ids {
        let s = find(c, *id)?.expect("checked by caller");
        let note = Note::find(c, s.note_id)?.expect("note exists");
        let mr = MergeRequest::get(c, note.noteable_id)?;
        let p = Project::find(c, mr.target_project_id)?.expect("project exists");
        items.push((s, note, mr, p));
    }
    let branch = items[0].2.source_branch.clone();
    let mut by_file: std::collections::BTreeMap<String, Vec<(i64, i64, String)>> =
        Default::default();
    for (s, note, mr, p) in &items {
        // `on_same_branch?`: the branch (and project), not the merge request.
        if mr.source_branch != branch || mr.source_project_id != items[0].2.source_project_id {
            return Ok(Err("Suggestions must all be on the same branch.".into()));
        }
        if let Some(reason) = inapplicable_reason(c, p, mr, note, s)? {
            return Ok(Err(reason));
        }
        let pos = Position::parse(note.position.as_deref().unwrap_or("")).unwrap_or_default();
        if pos.head_sha != mr.source_branch_sha(c, p)? {
            return Ok(Err("A file has been changed.".into()));
        }
        let (from, to) = line_range(s, note);
        by_file
            .entry(pos.new_path.clone().unwrap_or_default())
            .or_default()
            .push((from, to, s.to_content.clone()));
    }
    let (_, _, mr, p) = &items[0];
    let r = repo::repo(c, p);
    let head = r.branch(&mr.source_branch)?.expect("branch exists");
    let tree = r.commit(&head)?.tree;
    let mut actions = vec![];
    for (path, mut ranges) in by_file.clone() {
        ranges.sort_by_key(|x| x.0);
        for w in ranges.windows(2) {
            if w[1].0 <= w[0].1 {
                return Ok(Err(
                    "Suggestions are not applicable as their lines cannot overlap.".into(),
                ));
            }
        }
        let Some(entry) = r.entry_at(&tree, &path)? else {
            return Ok(Err("A file was not found.".into()));
        };
        let data = r.blob(&entry.oid)?;
        let text = utf8_dropping_invalid(&data);
        let lines: Vec<&str> = text
            .as_str()
            .split_inclusive('\n')
            .collect();
        let mut out = String::new();
        let mut i = 1i64;
        for (from, to, content) in &ranges {
            while i < *from {
                out.push_str(lines.get((i - 1) as usize).copied().unwrap_or(""));
                i += 1;
            }
            out.push_str(content);
            i = to + 1;
        }
        while (i as usize) <= lines.len() {
            out.push_str(lines[(i - 1) as usize]);
            i += 1;
        }
        actions.push(repo::FileAction::Update {
            path,
            content: out.into_bytes(),
            exec: None,
        });
    }
    let files: Vec<String> = by_file.keys().cloned().collect();
    let mut co: Vec<String> = vec![];
    for (_, note, _, _) in &items {
        if note.author_id != user.id {
            let a = User::get(c, note.author_id)?;
            let line = format!(
                "Co-authored-by: {} <{}>",
                a.name,
                a.commit_email.clone().unwrap_or(a.email.clone())
            );
            if !co.contains(&line) {
                co.push(line);
            }
        }
    }
    let template = message
        .filter(|m| !m.trim().is_empty())
        .map(str::to_string)
        .or(p.suggestion_commit_message.clone().filter(|m| !m.trim().is_empty()))
        .unwrap_or("Apply %{suggestions_count} suggestion(s) to %{files_count} file(s)\n\n%{co_authored_by}".into());
    let msg = template
        .replace("%{branch_name}", &mr.source_branch)
        .replace("%{files_count}", &files.len().to_string())
        .replace("%{file_paths}", &files.join(", "))
        .replace("%{project_name}", &p.name)
        .replace("%{project_path}", &p.path)
        .replace("%{user_full_name}", &user.name)
        .replace("%{username}", &user.username)
        .replace("%{suggestions_count}", &ids.len().to_string())
        .replace("%{co_authored_by}", &co.join("\n"));
    let oid = repo::create_commit(
        c,
        p,
        user,
        repo::NewCommit {
            branch: &mr.source_branch,
            start_branch: None,
            start_sha: None,
            message: &msg,
            actions: &actions,
            author_name: None,
            author_email: None,
            force: false,
        },
    )?;
    for id in ids {
        c.execute(
            "UPDATE suggestions SET applied = 1, commit_id = ?1 WHERE id = ?2",
            params![oid.hex(), id],
        )?;
    }
    crate::services::push::post_receive(
        c,
        config,
        p,
        user,
        &[labrat_git::protocol::RefUpdate {
            old: head,
            new: oid,
            name: format!("refs/heads/{}", mr.source_branch),
        }],
    )?;
    Ok(Ok(()))
}

/// Blob text as GitLab reads it for suggestions: bytes that aren't valid UTF-8 are dropped.
fn utf8_dropping_invalid(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len());
    for chunk in data.utf8_chunks() {
        out.push_str(chunk.valid());
    }
    out
}
