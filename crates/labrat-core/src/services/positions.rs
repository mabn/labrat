//! Diff positions (`Gitlab::Diff::Position`): parsing, validation, line codes and tracing.

use crate::Result;
use labrat_git::diff::{self, LineKind};
use labrat_git::{Oid, Repo};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Position {
    pub base_sha: Option<String>,
    pub start_sha: Option<String>,
    pub head_sha: Option<String>,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub position_type: String,
    pub old_line: Option<i64>,
    pub new_line: Option<i64>,
    pub line_range: Option<Value>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub x: Option<i64>,
    pub y: Option<i64>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn i(v: &Value, k: &str) -> Option<i64> {
    match v.get(k)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

impl Position {
    pub fn from_json(v: &Value) -> Position {
        Position {
            base_sha: s(v, "base_sha"),
            start_sha: s(v, "start_sha"),
            head_sha: s(v, "head_sha"),
            old_path: s(v, "old_path"),
            new_path: s(v, "new_path"),
            position_type: s(v, "position_type").unwrap_or("text".into()),
            old_line: i(v, "old_line"),
            new_line: i(v, "new_line"),
            line_range: v.get("line_range").filter(|x| !x.is_null()).cloned(),
            width: i(v, "width"),
            height: i(v, "height"),
            x: i(v, "x"),
            y: i(v, "y"),
        }
    }

    pub fn parse(stored: &str) -> Option<Position> {
        serde_json::from_str::<Value>(stored)
            .ok()
            .map(|v| Position::from_json(&v))
    }

    /// `to_h.except(:ignore_whitespace_change)` as the API exposes it.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("base_sha".into(), json!(self.base_sha));
        m.insert("start_sha".into(), json!(self.start_sha));
        m.insert("head_sha".into(), json!(self.head_sha));
        m.insert("old_path".into(), json!(self.old_path));
        m.insert("new_path".into(), json!(self.new_path));
        m.insert("position_type".into(), json!(self.position_type));
        match self.position_type.as_str() {
            "image" => {
                m.insert("width".into(), json!(self.width));
                m.insert("height".into(), json!(self.height));
                m.insert("x".into(), json!(self.x));
                m.insert("y".into(), json!(self.y));
            }
            "file" => {}
            _ => {
                m.insert("old_line".into(), json!(self.old_line));
                m.insert("new_line".into(), json!(self.new_line));
                m.insert(
                    "line_range".into(),
                    self.line_range.clone().unwrap_or(Value::Null),
                );
            }
        }
        Value::Object(m)
    }

    pub fn store(&self) -> String {
        self.to_json().to_string()
    }

    /// `positions_complete`.
    pub fn complete(&self) -> bool {
        if self.start_sha.is_none() || self.head_sha.is_none() {
            return false;
        }
        match self.position_type.as_str() {
            "image" => {
                self.x.is_some()
                    && self.y.is_some()
                    && self.width.is_some()
                    && self.height.is_some()
            }
            "file" => self.old_path.is_some() && self.new_path.is_some(),
            _ => self.file_path().is_some() && (self.old_line.is_some() || self.new_line.is_some()),
        }
    }

    pub fn file_path(&self) -> Option<String> {
        self.new_path
            .clone()
            .filter(|p| !p.is_empty())
            .or(self.old_path.clone())
    }

    pub fn on_text(&self) -> bool {
        self.position_type == "text"
    }

    pub fn diff_refs(&self) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.base_sha.clone(),
            self.start_sha.clone(),
            self.head_sha.clone(),
        )
    }

    /// The blobs on both sides of this position's file diff.
    fn blobs(&self, r: &Repo) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        let base = self.base_sha.clone().or(self.start_sha.clone());
        let (Some(base), Some(head)) = (base, self.head_sha.clone()) else {
            return Ok(None);
        };
        let (Ok(base), Ok(head)) = (Oid::from_hex(&base), Oid::from_hex(&head)) else {
            return Ok(None);
        };
        let (Ok(bc), Ok(hc)) = (r.commit(&base), r.commit(&head)) else {
            return Ok(None);
        };
        let old = match &self.old_path {
            Some(p) => r
                .entry_at(&bc.tree, p)?
                .filter(|e| !e.is_tree())
                .map(|e| r.blob(&e.oid))
                .transpose()?,
            None => None,
        };
        let new = match &self.new_path {
            Some(p) => r
                .entry_at(&hc.tree, p)?
                .filter(|e| !e.is_tree())
                .map(|e| r.blob(&e.oid))
                .transpose()?,
            None => None,
        };
        if old.is_none() && new.is_none() {
            return Ok(None);
        }
        Ok(Some((old.unwrap_or_default(), new.unwrap_or_default())))
    }

    /// The diff line this text position points at, if it exists in the diff.
    pub fn line(&self, r: &Repo) -> Result<Option<diff::DiffLine>> {
        if !self.on_text() {
            return Ok(None);
        }
        let Some((old, new)) = self.blobs(r)? else {
            return Ok(None);
        };
        // No text diff (pure rename, mode change, binary): `diff_lines` is empty, so no line
        // can be addressed, not even by unfolding.
        if old == new || old.contains(&0) || new.contains(&0) {
            return Ok(None);
        }
        let want_old = self.old_line.map(|v| v as usize);
        let want_new = self.new_line.map(|v| v as usize);
        Ok(diff::diff_lines(&old, &new)
            .into_iter()
            .find(|l| l.old_line() == want_old && l.new_line() == want_new))
    }

    /// `line_code` for text positions, `None` if the line isn't in the diff.
    pub fn line_code(&self, r: &Repo) -> Result<Option<String>> {
        let Some(l) = self.line(r)? else {
            return Ok(None);
        };
        let path = self.file_path().unwrap_or_default();
        Ok(Some(diff::line_code(&path, l.old_pos, l.new_pos)))
    }

    /// Lines of the new blob at this position's head (for suggestions).
    pub fn new_blob(&self, r: &Repo) -> Result<Option<Vec<u8>>> {
        let (Some(head), Some(path)) = (self.head_sha.clone(), self.new_path.clone()) else {
            return Ok(None);
        };
        let Ok(head) = Oid::from_hex(&head) else {
            return Ok(None);
        };
        let hc = r.commit(&head)?;
        Ok(r.entry_at(&hc.tree, &path)?
            .filter(|e| !e.is_tree())
            .map(|e| r.blob(&e.oid))
            .transpose()?)
    }

    /// `PositionTracer`'s position for a changed line: the line in the diff from this
    /// position's head to `new_head`, where it was removed or modified.
    pub fn change_position(&self, r: &Repo, new_head: &str) -> Result<Option<Position>> {
        if !self.on_text() {
            return Ok(None);
        }
        let Some(cur) = self.line(r)? else {
            return Ok(None);
        };
        let blob_at = |sha: &str, path: &Option<String>| -> Result<Vec<u8>> {
            let (Ok(o), Some(path)) = (Oid::from_hex(sha), path) else {
                return Ok(vec![]);
            };
            let c = r.commit(&o)?;
            Ok(r.entry_at(&c.tree, path)?
                .filter(|e| !e.is_tree())
                .map(|e| r.blob(&e.oid))
                .transpose()?
                .unwrap_or_default())
        };
        let old_head = self.head_sha.clone().unwrap_or_default();
        let a = blob_at(&old_head, &self.new_path)?;
        let b = blob_at(new_head, &self.new_path)?;
        let line = diff::diff_lines(&a, &b)
            .into_iter()
            .find(|l| l.kind == LineKind::Removed && l.old_pos == cur.new_pos);
        let Some(l) = line else { return Ok(None) };
        Ok(Some(Position {
            base_sha: Some(old_head.clone()),
            start_sha: Some(old_head),
            head_sha: Some(new_head.to_string()),
            old_line: Some(l.old_pos as i64),
            new_line: None,
            line_range: None,
            ..self.clone()
        }))
    }

    /// Traces this position onto new diff refs (`PositionTracer`). `Ok(None)` means the line
    /// changed: the position is outdated.
    pub fn trace(&self, r: &Repo, base: &str, start: &str, head: &str) -> Result<Option<Position>> {
        let mut next = self.clone();
        next.base_sha = Some(base.to_string());
        next.start_sha = Some(start.to_string());
        next.head_sha = Some(head.to_string());
        if !self.on_text() {
            // `FileStrategy` (and `ImageStrategy`): outdated when the file changed between the
            // old and the new head (`bd_diffs`); current when the new diff still has it
            // (`cd_diffs`); otherwise outdated.
            let blob = |sha: Option<&str>, path: &Option<String>| -> Result<Option<Oid>> {
                let (Some(sha), Some(path)) = (sha, path) else { return Ok(None) };
                let Ok(o) = Oid::from_hex(sha) else { return Ok(None) };
                Ok(r.entry_at(&r.commit(&o)?.tree, path)?.filter(|e| !e.is_tree()).map(|e| e.oid))
            };
            let at_new_head = blob(Some(head), &self.new_path)?;
            // A deleted file is absent at both heads and still in the diff: current.
            if blob(self.head_sha.as_deref(), &self.new_path)? != at_new_head {
                return Ok(None);
            }
            let in_new_diff = blob(Some(base), &self.new_path)? != at_new_head;
            return Ok(if in_new_diff { Some(next) } else { None });
        }
        let Some(cur) = self.line(r)? else {
            return Ok(None);
        };
        let blob_at = |sha: &Option<String>, path: &Option<String>| -> Result<Option<Vec<u8>>> {
            let (Some(sha), Some(path)) = (sha, path) else {
                return Ok(None);
            };
            let Ok(o) = Oid::from_hex(sha) else {
                return Ok(None);
            };
            let c = r.commit(&o)?;
            Ok(r.entry_at(&c.tree, path)?
                .filter(|e| !e.is_tree())
                .map(|e| r.blob(&e.oid))
                .transpose()?)
        };
        let new_base = Some(base.to_string());
        let new_head = Some(head.to_string());
        match cur.kind {
            LineKind::Added | LineKind::Context => {
                let (Some(old_head), Some(new_head_blob)) = (
                    blob_at(&self.head_sha, &self.new_path)?,
                    blob_at(&new_head, &self.new_path)?,
                ) else {
                    return Ok(None);
                };
                let map = diff::line_map(&old_head, &new_head_blob);
                let Some(nl) = map.get(&cur.new_pos).copied() else {
                    return Ok(None);
                };
                let old_side =
                    blob_at(&new_base, &self.old_path.clone().or(self.new_path.clone()))?
                        .unwrap_or_default();
                let found = diff::diff_lines(&old_side, &new_head_blob)
                    .into_iter()
                    .find(|l| l.new_line() == Some(nl));
                let Some(l) = found else { return Ok(None) };
                if (cur.kind == LineKind::Added) != (l.kind == LineKind::Added) {
                    return Ok(None);
                }
                next.new_line = Some(nl as i64);
                next.old_line = l.old_line().map(|v| v as i64);
                Ok(Some(next))
            }
            LineKind::Removed => {
                let (Some(old_base), Some(new_base_blob)) = (
                    blob_at(
                        &self.base_sha.clone().or(self.start_sha.clone()),
                        &self.old_path,
                    )?,
                    blob_at(&new_base, &self.old_path)?,
                ) else {
                    return Ok(None);
                };
                let map = diff::line_map(&old_base, &new_base_blob);
                let Some(ol) = map.get(&cur.old_pos).copied() else {
                    return Ok(None);
                };
                let new_side =
                    blob_at(&new_head, &self.new_path.clone().or(self.old_path.clone()))?
                        .unwrap_or_default();
                let found = diff::diff_lines(&new_base_blob, &new_side)
                    .into_iter()
                    .find(|l| l.kind == LineKind::Removed && l.old_line() == Some(ol));
                if found.is_none() {
                    return Ok(None);
                }
                next.old_line = Some(ol as i64);
                next.new_line = None;
                Ok(Some(next))
            }
        }
    }
}
