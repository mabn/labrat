//! Notes (comments) and system notes.

use crate::db::OptionalExt;
use crate::model::User;
use crate::{Result, time};
use rusqlite::{Connection, Row, params};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct Note {
    pub id: i64,
    pub note: String,
    pub noteable_type: String,
    pub noteable_id: i64,
    pub project_id: Option<i64>,
    pub author_id: i64,
    pub system: bool,
    pub system_action: Option<String>,
    pub kind: Option<String>,
    pub discussion_id: String,
    pub resolvable: bool,
    pub resolved_at: Option<i64>,
    pub resolved_by_id: Option<i64>,
    pub resolved_by_push: bool,
    pub position: Option<String>,
    pub original_position: Option<String>,
    pub change_position: Option<String>,
    pub line_code: Option<String>,
    pub commit_id: Option<String>,
    pub internal: bool,
    pub review_id: Option<i64>,
    pub updated_by_id: Option<i64>,
    pub last_edited_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Note {
    pub const COLUMNS: &'static str = "id, note, noteable_type, noteable_id, project_id, author_id, system, system_action, type, discussion_id, resolvable, resolved_at, resolved_by_id, resolved_by_push, position, original_position, change_position, line_code, commit_id, internal, review_id, updated_by_id, last_edited_at, created_at, updated_at";

    pub fn from_row(r: &Row) -> rusqlite::Result<Note> {
        Ok(Note {
            id: r.get(0)?,
            note: r.get(1)?,
            noteable_type: r.get(2)?,
            noteable_id: r.get(3)?,
            project_id: r.get(4)?,
            author_id: r.get(5)?,
            system: r.get(6)?,
            system_action: r.get(7)?,
            kind: r.get(8)?,
            discussion_id: r.get(9)?,
            resolvable: r.get(10)?,
            resolved_at: r.get(11)?,
            resolved_by_id: r.get(12)?,
            resolved_by_push: r.get(13)?,
            position: r.get(14)?,
            original_position: r.get(15)?,
            change_position: r.get(16)?,
            line_code: r.get(17)?,
            commit_id: r.get(18)?,
            internal: r.get(19)?,
            review_id: r.get(20)?,
            updated_by_id: r.get(21)?,
            last_edited_at: r.get(22)?,
            created_at: r.get(23)?,
            updated_at: r.get(24)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<Note>> {
        c.query_row(
            &format!("SELECT {} FROM notes WHERE id = ?1", Self::COLUMNS),
            [id],
            Self::from_row,
        )
        .optional()
    }

    pub fn for_noteable(
        c: &Connection,
        noteable_type: &str,
        noteable_id: i64,
    ) -> Result<Vec<Note>> {
        let mut st = c.prepare(&format!(
            "SELECT {} FROM notes WHERE noteable_type = ?1 AND noteable_id = ?2 ORDER BY created_at, id",
            Self::COLUMNS
        ))?;
        let v = st
            .query_map(params![noteable_type, noteable_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn in_discussion(c: &Connection, discussion_id: &str) -> Result<Vec<Note>> {
        let mut st = c.prepare(&format!(
            "SELECT {} FROM notes WHERE discussion_id = ?1 ORDER BY created_at, id",
            Self::COLUMNS
        ))?;
        let v = st
            .query_map([discussion_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Resolvable notes are user notes in threads (`DiscussionNote`/`DiffNote`).
    pub fn is_resolved(&self) -> bool {
        self.resolved_at.is_some()
    }
}

/// A new random discussion id (GitLab: SHA1 over the noteable and a random value).
pub fn new_discussion_id() -> String {
    let r: u128 = rand::random();
    let h = Sha256::digest(format!("discussion-{r}").as_bytes());
    hex::encode(&h[..20])
}

#[derive(Debug, Default, Clone)]
pub struct NewNote {
    pub note: String,
    pub noteable_type: String,
    pub noteable_id: i64,
    pub project_id: Option<i64>,
    pub system: bool,
    pub system_action: Option<String>,
    pub kind: Option<String>,
    pub discussion_id: Option<String>,
    pub resolvable: bool,
    pub position: Option<String>,
    pub original_position: Option<String>,
    pub line_code: Option<String>,
    pub commit_id: Option<String>,
    pub internal: bool,
    pub review_id: Option<i64>,
    pub created_at: Option<i64>,
    /// `skip_touch_noteable` (cross-reference notes).
    pub skip_touch: bool,
}

pub fn insert(c: &Connection, author: &User, n: NewNote) -> Result<Note> {
    let now = n.created_at.unwrap_or_else(time::now);
    let discussion_id = n.discussion_id.unwrap_or_else(new_discussion_id);
    c.execute(
        "INSERT INTO notes (note, noteable_type, noteable_id, project_id, author_id, system, system_action, type, discussion_id,
            resolvable, position, original_position, line_code, commit_id, internal, review_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
        params![
            n.note,
            n.noteable_type,
            n.noteable_id,
            n.project_id,
            author.id,
            n.system,
            n.system_action,
            n.kind,
            discussion_id,
            n.resolvable,
            n.position,
            n.original_position,
            n.line_code,
            n.commit_id,
            n.internal,
            n.review_id,
            now
        ],
    )?;
    let note = Note::find(c, c.last_insert_rowid())?.expect("just inserted");
    if !n.skip_touch {
        touch_noteable(c, &note.noteable_type, note.noteable_id, now)?;
    }
    Ok(note)
}

/// `Note#touch_noteable` (after every save). Merge requests use `ThrottledTouch`: they are only
/// touched when `updated_at` is more than a minute old.
pub fn touch_noteable(c: &Connection, noteable_type: &str, noteable_id: i64, now: i64) -> Result<()> {
    if noteable_type == "MergeRequest" {
        c.execute(
            "UPDATE merge_requests SET updated_at = ?1 WHERE id = ?2 AND ?1 - updated_at > 60000",
            params![now, noteable_id],
        )?;
    }
    Ok(())
}

/// Creates a system note on a merge request.
pub fn system_note(
    c: &Connection,
    mr_id: i64,
    project_id: i64,
    author: &User,
    body: &str,
    action: &str,
) -> Result<Note> {
    insert(
        c,
        author,
        NewNote {
            note: body.to_string(),
            noteable_type: "MergeRequest".into(),
            noteable_id: mr_id,
            project_id: Some(project_id),
            system: true,
            system_action: Some(action.into()),
            ..Default::default()
        },
    )
}

/// Rails' `Array#to_sentence`: "a", "a and b", "a, b, and c".
pub fn to_sentence(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        n => format!("{}, and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

/// `ERB::Util.html_escape`.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// `changed title from ... to ...` with GitLab's inline diff markers.
pub fn title_change_body(old: &str, new: &str) -> String {
    let (old_ranges, new_ranges) = char_diff_ranges(old, new);
    format!(
        "<p>changed title from <code class=\"idiff\">{}</code> to <code class=\"idiff\">{}</code></p>",
        mark(old, &old_ranges, "deletion"),
        mark(new, &new_ranges, "addition")
    )
}

/// Character ranges that differ between `old` and `new`: `Gitlab::Diff::CharDiff`, i.e.
/// diff-match-patch's `diff_main` followed by `diff_cleanupSemantic` (`dissimilar` is a port).
fn char_diff_ranges(old: &str, new: &str) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    let mut del: Vec<(usize, usize)> = vec![];
    let mut add: Vec<(usize, usize)> = vec![];
    let (mut o, mut n) = (0, 0);
    for chunk in dissimilar::diff(old, new) {
        match chunk {
            dissimilar::Chunk::Equal(t) => {
                let len = t.chars().count();
                o += len;
                n += len;
            }
            dissimilar::Chunk::Delete(t) => {
                let len = t.chars().count();
                del.push((o, o + len));
                o += len;
            }
            dissimilar::Chunk::Insert(t) => {
                let len = t.chars().count();
                add.push((n, n + len));
                n += len;
            }
        }
    }
    (merge_ranges(del), merge_ranges(add))
}

fn merge_ranges(mut v: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    v.sort();
    let mut out: Vec<(usize, usize)> = vec![];
    for r in v {
        match out.last_mut() {
            Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

fn mark(text: &str, ranges: &[(usize, usize)], mode: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut pos = 0;
    for (i, (s, e)) in ranges.iter().enumerate() {
        out.push_str(&html_escape(&chars[pos..*s].iter().collect::<String>()));
        let mut classes = vec!["idiff"];
        if i == 0 {
            classes.push("left");
        }
        if i == ranges.len() - 1 {
            classes.push("right");
        }
        classes.push(mode);
        out.push_str(&format!(
            "<span class=\"{}\">{}</span>",
            classes.join(" "),
            html_escape(&chars[*s..*e].iter().collect::<String>())
        ));
        pos = *e;
    }
    out.push_str(&html_escape(&chars[pos..].iter().collect::<String>()));
    out
}

/// A cross-reference system note ("mentioned in ..."): `skip_touch_noteable: true`.
pub fn cross_reference_note(c: &Connection, mr_id: i64, project_id: i64, author: &User, body: &str) -> Result<Note> {
    insert(
        c,
        author,
        NewNote {
            note: body.to_string(),
            noteable_type: "MergeRequest".into(),
            noteable_id: mr_id,
            project_id: Some(project_id),
            system: true,
            system_action: Some("cross_reference".into()),
            skip_touch: true,
            ..Default::default()
        },
    )
}
