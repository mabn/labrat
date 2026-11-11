//! `Note`, `Discussion`, `DraftNote`, `Suggestion`, `AwardEmoji`, `MergeRequestApprovals`.

use crate::entities::user::{self, Ctx};
use labrat_core::model::{Project, User};
use labrat_core::services::emoji::Award;
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::notes::Note;
use labrat_core::services::positions::Position;
use labrat_core::services::reviews::{self, Draft};
use labrat_core::services::suggestions;
use labrat_core::{Result, time};
use rusqlite::Connection;
use serde_json::{Map, Value, json};

pub fn note(c: &Connection, ctx: &Ctx, p: &Project, mr: &MergeRequest, n: &Note) -> Result<Value> {
    note_with_changes(c, ctx, p, mr, n, Map::new())
}

/// A note with the `commands_changes` of the request that created or updated it.
pub fn note_with_changes(
    c: &Connection,
    ctx: &Ctx,
    p: &Project,
    mr: &MergeRequest,
    n: &Note,
    changes: Map<String, Value>,
) -> Result<Value> {
    let author = User::get(c, n.author_id)?;
    let mut m = Map::new();
    m.insert("id".into(), json!(n.id));
    m.insert("type".into(), json!(n.kind));
    m.insert("body".into(), json!(n.note));
    m.insert("author".into(), user::basic_value(&author, ctx));
    m.insert("created_at".into(), json!(time::iso(n.created_at)));
    m.insert("updated_at".into(), json!(time::iso(n.updated_at)));
    m.insert("system".into(), json!(n.system));
    m.insert("noteable_id".into(), json!(n.noteable_id));
    m.insert("noteable_type".into(), json!(n.noteable_type));
    m.insert("project_id".into(), json!(n.project_id));
    let diff = n.kind.as_deref() == Some("DiffNote");
    if diff {
        m.insert("commit_id".into(), json!(n.commit_id));
        let pos = n
            .position
            .as_deref()
            .and_then(Position::parse)
            .map(|p| p.to_json())
            .unwrap_or(Value::Null);
        m.insert("position".into(), pos);
    }
    let resolvable = n.resolvable && !n.system;
    m.insert("resolvable".into(), json!(resolvable));
    if resolvable {
        m.insert("resolved".into(), json!(n.resolved_at.is_some()));
        let by = match n.resolved_by_id {
            Some(id) => User::find(c, id)?
                .map(|u| user::basic_value(&u, ctx))
                .unwrap_or(Value::Null),
            None => Value::Null,
        };
        m.insert("resolved_by".into(), by);
        m.insert("resolved_at".into(), json!(n.resolved_at.map(time::iso)));
    }
    if diff {
        let list = suggestions::for_note(c, n.id)?;
        let mut out = vec![];
        for s in &list {
            out.push(suggestion(c, p, mr, n, s)?);
        }
        m.insert("suggestions".into(), Value::Array(out));
    }
    m.insert("confidential".into(), json!(n.internal));
    m.insert("internal".into(), json!(n.internal));
    m.insert("imported".into(), json!(false));
    m.insert("imported_from".into(), json!("none"));
    m.insert("noteable_iid".into(), json!(mr.iid));
    m.insert("commands_changes".into(), Value::Object(changes));
    Ok(Value::Object(m))
}

/// `Entities::Note` for a note on a commit (no noteable id, never resolvable).
pub fn commit_note(c: &Connection, ctx: &Ctx, n: &Note) -> Result<Value> {
    let author = User::get(c, n.author_id)?;
    let mut m = Map::new();
    m.insert("id".into(), json!(n.id));
    m.insert("type".into(), json!(n.kind));
    m.insert("body".into(), json!(n.note));
    m.insert("author".into(), user::basic_value(&author, ctx));
    m.insert("created_at".into(), json!(time::iso(n.created_at)));
    m.insert("updated_at".into(), json!(time::iso(n.updated_at)));
    m.insert("system".into(), json!(n.system));
    m.insert("noteable_id".into(), Value::Null);
    m.insert("noteable_type".into(), json!("Commit"));
    m.insert("project_id".into(), json!(n.project_id));
    if n.kind.as_deref() == Some("DiffNote") {
        let pos = n.position.as_deref().and_then(Position::parse).map(|p| p.to_json()).unwrap_or(Value::Null);
        m.insert("position".into(), pos);
    }
    m.insert("resolvable".into(), json!(false));
    m.insert("confidential".into(), json!(n.internal));
    m.insert("internal".into(), json!(n.internal));
    m.insert("imported".into(), json!(false));
    m.insert("imported_from".into(), json!("none"));
    m.insert("noteable_iid".into(), Value::Null);
    m.insert("commands_changes".into(), json!({}));
    Ok(Value::Object(m))
}

/// `Entities::Discussion` on a commit.
pub fn commit_discussion(c: &Connection, ctx: &Ctx, notes: &[Note]) -> Result<Value> {
    let first = &notes[0];
    let mut out = vec![];
    for n in notes {
        out.push(commit_note(c, ctx, n)?);
    }
    Ok(json!({
        "id": first.discussion_id,
        // A line comment is a `LegacyDiffDiscussion`, not an individual note.
        "individual_note": first.kind.is_none(),
        "resolvable": false,
        "notes": out,
    }))
}

pub fn suggestion(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    n: &Note,
    s: &suggestions::Suggestion,
) -> Result<Value> {
    let (from, to) = suggestions::line_range(s, n);
    let appliable = suggestions::inapplicable_reason(c, p, mr, n, s)?.is_none();
    Ok(json!({
        "id": s.id,
        "from_line": from,
        "to_line": to,
        "appliable": appliable,
        "applied": s.applied,
        "from_content": s.from_content,
        "to_content": s.to_content,
    }))
}

pub fn discussion(
    c: &Connection,
    ctx: &Ctx,
    p: &Project,
    mr: &MergeRequest,
    notes: &[Note],
) -> Result<Value> {
    let first = &notes[0];
    let individual = first.kind.is_none() && !first.system
        || (first.system && first.kind.as_deref() != Some("DiffNote"));
    let individual = individual && first.kind.is_none();
    let resolvable = first.kind.is_some() && notes.iter().any(|n| n.resolvable && !n.system);
    let mut m = Map::new();
    m.insert("id".into(), json!(first.discussion_id));
    m.insert("individual_note".into(), json!(individual));
    m.insert("resolvable".into(), json!(resolvable));
    if resolvable {
        let resolved = notes
            .iter()
            .filter(|n| n.resolvable && !n.system)
            .all(|n| n.resolved_at.is_some());
        m.insert("resolved".into(), json!(resolved));
    }
    let mut out = vec![];
    for n in notes {
        out.push(note(c, ctx, p, mr, n)?);
    }
    m.insert("notes".into(), Value::Array(out));
    Ok(Value::Object(m))
}

pub fn draft(d: &Draft) -> Value {
    let position = d
        .position
        .as_deref()
        .and_then(Position::parse)
        .map(|p| p.to_json())
        .unwrap_or_else(|| {
            json!({"base_sha": null, "start_sha": null, "head_sha": null, "old_path": null, "new_path": null,
                   "position_type": "text", "old_line": null, "new_line": null, "line_range": null})
        });
    json!({
        "id": d.id,
        "author_id": d.author_id,
        "merge_request_id": d.merge_request_id,
        "resolve_discussion": d.resolve_discussion,
        "discussion_id": d.discussion_id,
        "note": d.note,
        "commit_id": d.commit_id,
        "line_code": d.line_code,
        "position": position,
    })
}

pub fn award(c: &Connection, ctx: &Ctx, a: &Award) -> Result<Value> {
    let u = User::get(c, a.user_id)?;
    Ok(json!({
        "id": a.id,
        "name": a.name,
        "user": user::basic_value(&u, ctx),
        "created_at": time::iso(a.created_at),
        "updated_at": time::iso(a.updated_at),
        "awardable_id": a.awardable_id,
        "awardable_type": a.awardable_type,
        "url": null,
    }))
}

/// `API::Entities::MergeRequestApprovals`.
pub fn merge_request_approvals(
    c: &Connection,
    ctx: &Ctx,
    p: &Project,
    mr: &MergeRequest,
    current: Option<&User>,
) -> Result<Value> {
    let mut approved_by = vec![];
    for (uid, at) in reviews::approvals(c, mr)? {
        let u = User::get(c, uid)?;
        approved_by.push(json!({"user": user::basic_value(&u, ctx), "approved_at": time::iso(at)}));
    }
    let (has, can) = match current {
        Some(u) => {
            let has = reviews::has_approved(c, mr, u)?;
            // `eligible_for_approval_by?`: not yet approved and allowed to approve.
            (has, !has && reviews::can_approve(c, p, mr, u)?)
        }
        None => (false, false),
    };
    Ok(json!({
        "user_has_approved": has,
        "user_can_approve": can,
        "approved": !approved_by.is_empty(),
        "approved_by": approved_by,
    }))
}
