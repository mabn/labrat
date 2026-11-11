//! Project and group labels, and labels on merge requests.

use crate::db::OptionalExt;
use crate::model::{Namespace, Project};
use crate::{Error, Result, time};
use rusqlite::{Connection, Row, params};

#[derive(Debug, Clone)]
pub struct Label {
    pub id: i64,
    pub title: String,
    pub color: String,
    pub description: Option<String>,
    pub project_id: Option<i64>,
    pub group_id: Option<i64>,
    pub priority: Option<i64>,
    pub archived: bool,
    pub lock_on_merge: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Label {
    pub const COLUMNS: &'static str = "id, title, color, description, project_id, group_id, priority, archived, lock_on_merge, created_at, updated_at";

    pub fn from_row(r: &Row) -> rusqlite::Result<Label> {
        Ok(Label {
            id: r.get(0)?,
            title: r.get(1)?,
            color: r.get(2)?,
            description: r.get(3)?,
            project_id: r.get(4)?,
            group_id: r.get(5)?,
            priority: r.get(6)?,
            archived: r.get(7)?,
            lock_on_merge: r.get(8)?,
            created_at: r.get(9)?,
            updated_at: r.get(10)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<Label>> {
        c.query_row(
            &format!("SELECT {} FROM labels WHERE id = ?1", Self::COLUMNS),
            [id],
            Self::from_row,
        )
        .optional()
    }

    /// Text color GitLab derives from the background color.
    pub fn text_color(&self) -> String {
        text_color_for(&self.color)
    }
}

/// `Gitlab::Color#contrast`: WCAG contrast against white and GitLab's dark text color.
pub fn text_color_for(color: &str) -> String {
    let hex = color.trim().trim_start_matches('#');
    let rgb: Vec<f64> = match hex.len() {
        3 if hex.chars().all(|c| c.is_ascii_hexdigit()) => hex
            .chars()
            .map(|c| u8::from_str_radix(&format!("{c}{c}"), 16).unwrap() as f64)
            .collect(),
        6 if hex.chars().all(|c| c.is_ascii_hexdigit()) => (0..3)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap() as f64)
            .collect(),
        _ => return "#FFFFFF".into(),
    };
    let srgb = |v: f64| {
        let n = v / 255.0;
        if n <= 0.03928 { n / 12.92 } else { ((n + 0.055) / 1.055).powf(2.4) }
    };
    let lum = |c: &[f64]| 0.2126 * srgb(c[0]) + 0.7152 * srgb(c[1]) + 0.0722 * srgb(c[2]);
    let l = lum(&rgb);
    let light = lum(&[255.0, 255.0, 255.0]);
    let dark = lum(&[31.0, 30.0, 36.0]);
    let contrast_light = (light + 0.05) / (l + 0.05);
    let contrast_dark = (l + 0.05) / (dark + 0.05);
    let is_light = !(contrast_light >= 2.4 || contrast_light > contrast_dark);
    if is_light { "#1F1E24".into() } else { "#FFFFFF".into() }
}

/// GitLab's default color for labels created implicitly.
pub const DEFAULT_COLOR: &str = "#6699cc";

/// Labels visible to a project: its own plus its ancestor groups', ordered by title.
pub fn available(c: &Connection, p: &Project) -> Result<Vec<Label>> {
    let groups = Namespace::self_and_ancestor_ids(c, p.namespace_id)?;
    let list = groups
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut st = c.prepare(&format!(
        "SELECT {} FROM labels WHERE project_id = ?1 OR group_id IN ({}) ORDER BY title, id",
        Label::COLUMNS,
        if list.is_empty() {
            "-1".to_string()
        } else {
            list
        }
    ))?;
    let v = st
        .query_map([p.id], Label::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn create(
    c: &Connection,
    project_id: Option<i64>,
    group_id: Option<i64>,
    title: &str,
    color: &str,
    description: Option<&str>,
    priority: Option<i64>,
) -> Result<Label> {
    let now = time::now();
    c.execute(
        "INSERT INTO labels (title, color, description, project_id, group_id, priority, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![title, color, description, project_id, group_id, priority, now],
    )?;
    Label::find(c, c.last_insert_rowid())?.ok_or(Error::not_found("Label"))
}

/// `Labels::FindOrCreateService` for titles: existing labels by exact title; unknown ones are
/// created as project labels when `can_create`.
pub fn find_or_create_by_titles(
    c: &Connection,
    p: &Project,
    titles: &[String],
    can_create: bool,
) -> Result<Vec<Label>> {
    let existing = available(c, p)?;
    let mut out: Vec<Label> = vec![];
    for t in titles {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(l) = existing.iter().find(|l| l.title == t) {
            if !out.iter().any(|x| x.id == l.id) {
                out.push(l.clone());
            }
        } else if can_create {
            let l = create(c, Some(p.id), None, t, DEFAULT_COLOR, None, None)?;
            out.push(l);
        }
    }
    Ok(out)
}

pub fn find_by_titles(c: &Connection, p: &Project, titles: &[String]) -> Result<Vec<Label>> {
    let existing = available(c, p)?;
    Ok(existing
        .into_iter()
        .filter(|l| titles.iter().any(|t| t.trim() == l.title))
        .collect())
}

pub fn for_target(c: &Connection, target_type: &str, target_id: i64) -> Result<Vec<Label>> {
    let mut st = c.prepare(&format!(
        "SELECT {} FROM labels WHERE id IN (SELECT label_id FROM label_links WHERE target_type = ?1 AND target_id = ?2) ORDER BY title, id",
        Label::COLUMNS
    ))?;
    let v = st
        .query_map(params![target_type, target_id], Label::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn set_for_target(
    c: &Connection,
    target_type: &str,
    target_id: i64,
    label_ids: &[i64],
) -> Result<()> {
    c.execute(
        "DELETE FROM label_links WHERE target_type = ?1 AND target_id = ?2",
        params![target_type, target_id],
    )?;
    let now = time::now();
    for id in label_ids {
        c.execute(
            "INSERT OR IGNORE INTO label_links (label_id, target_type, target_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, target_type, target_id, now],
        )?;
    }
    Ok(())
}
