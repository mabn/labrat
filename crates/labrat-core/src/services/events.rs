//! Contribution events (`EventCreateService`) behind the events API.

use crate::{Result, time};
use rusqlite::{Connection, params};
use serde_json::Value;

/// Records one event. `action` is the stored action (`created`, `closed`, `reopened`, `merged`,
/// `commented`, `joined`, `pushed`, `approved`); the API derives `action_name` from it.
pub fn record(
    c: &Connection,
    project_id: Option<i64>,
    author_id: i64,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<i64>,
    push_data: Option<Value>,
) -> Result<()> {
    c.execute(
        "INSERT INTO events (project_id, author_id, action, target_type, target_id, push_data, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project_id, author_id, action, target_type, target_id, push_data.map(|v| v.to_string()), time::now()],
    )?;
    Ok(())
}

/// The note's class name as GitLab stores it in `events.target_type`.
pub fn note_type(kind: Option<&str>) -> &'static str {
    match kind {
        Some("DiffNote") => "DiffNote",
        Some("DiscussionNote") => "DiscussionNote",
        Some("LegacyDiffNote") => "LegacyDiffNote",
        _ => "Note",
    }
}
