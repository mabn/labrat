//! Commit comments (`GET/POST /projects/:id/repository/commits/:sha/comments`, `Entities::CommitNote`).
//! A comment on a changed line becomes a `LegacyDiffNote` keyed by its line code, as in GitLab.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::user::{self as user_ent, Ctx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::{get, put};
use labrat_core::model::User;
use labrat_core::{App, Error, repo, time};
use labrat_git::Kind;
use rusqlite::params;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    let d = "/projects/{id}/repository/commits/{sha}/discussions";
    Router::new()
        .route("/projects/{id}/repository/commits/{sha}/comments", get(list).post(create))
        .route(d, get(list_discussions).post(create_discussion))
        .route(&format!("{d}/{{discussion_id}}"), get(show_discussion))
        .route(&format!("{d}/{{discussion_id}}/notes"), get(discussion_notes).post(reply))
        .route(&format!("{d}/{{discussion_id}}/notes/{{note_id}}"), put(update_note).delete(delete_note))
}

type Row = (i64, String, i64, Option<String>, i64);

fn entity(c: &rusqlite::Connection, req: &Req, row: &Row) -> Result<Value, ApiError> {
    let (_, note, author_id, legacy, created) = row;
    let mut info: Value = legacy.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    if info.get("position_type").is_some() {
        // A `DiffNote` from the discussions API: `diff_file.file_path`, `line`, `line_type`.
        let new_line = info["new_line"].clone();
        let (line, kind) = if new_line.is_null() { (info["old_line"].clone(), "old") } else { (new_line, "new") };
        let path = if info["new_path"].is_null() { info["old_path"].clone() } else { info["new_path"].clone() };
        info = json!({"path": path, "line": line, "line_type": kind});
    }
    let ctx = Ctx { base_url: &req.app.config.external_url, ee: req.app.config.is_ee(), current_user: req.user.as_ref() };
    let author = User::find(c, *author_id)?.map(|u| user_ent::basic_value(&u, &ctx)).unwrap_or(Value::Null);
    let mut m = Map::new();
    m.insert("note".into(), json!(note));
    m.insert("path".into(), info["path"].clone());
    m.insert("line".into(), info["line"].clone());
    m.insert("line_type".into(), info["line_type"].clone());
    m.insert("author".into(), author);
    m.insert("created_at".into(), json!(time::iso(*created)));
    Ok(Value::Object(m))
}

async fn list(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.read_code(&p) {
            return Err(Error::forbidden().into());
        }
        let r = repo::repo(c, &p);
        let oid = r.resolve(&sha)?.filter(|o| r.kind(o).ok().flatten() == Some(Kind::Commit)).ok_or(Error::not_found("Commit"))?;
        let mut st = c.prepare(
            "SELECT id, note, author_id, original_position, created_at FROM notes
             WHERE noteable_type = 'Commit' AND commit_id = ?1 AND project_id = ?2 ORDER BY created_at, id",
        )?;
        let rows: Vec<Row> = st
            .query_map(params![oid.hex(), p.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?;
        paginate(&req, rows, &[], |row| entity(c, &req, row))
    })
}

async fn create(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let sha = req.path_param("sha").to_string();
    req.params.require(&["note"])?;
    let note = req.params.str("note").unwrap_or_default();
    let path = req.params.str("path");
    // Grape's `requires :line_type, default: 'new'` inside `given :path` still demands it.
    let line = if path.is_some() {
        req.params.require(&["line", "line_type"])?;
        Some(req.params.int("line")?.ok_or_else(|| Error::Param("line is invalid".into()))?)
    } else {
        None
    };
    let line_type = if path.is_some() {
        req.params.one_of("line_type", &["new", "old"])?.unwrap_or("new".into())
    } else {
        "new".into()
    };
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.read_code(&p) || perms.level == 0 && !perms.admin {
            return Err(Error::forbidden().into());
        }
        let r = repo::repo(c, &p);
        let oid = r.resolve(&sha)?.filter(|o| r.kind(o).ok().flatten() == Some(Kind::Commit)).ok_or(Error::not_found("Commit"))?;
        if note.trim().is_empty() {
            return Err(Error::message(400, "400 Bad request - Note {:note=>[\"can't be blank\"]}").into());
        }
        // Find the diff line: an added (`new`) or removed (`old`) line with that number.
        let mut found: Option<(String, Value)> = None;
        if let (Some(path), Some(line)) = (&path, line) {
            let commit = r.commit(&oid)?;
            let parent_tree = match commit.parents.first() {
                Some(pp) => Some(r.commit(pp)?.tree),
                None => None,
            };
            for ch in labrat_git::diff::diff_trees(&r, parent_tree.as_ref(), Some(&commit.tree))? {
                if ch.new_path != *path {
                    continue;
                }
                let old = if ch.new_file { vec![] } else { r.blob(&ch.old_oid)? };
                let new = if ch.deleted_file { vec![] } else { r.blob(&ch.new_oid)? };
                for l in labrat_git::diff::diff_lines(&old, &new) {
                    let (kind, number) = match l.kind {
                        labrat_git::diff::LineKind::Added => ("new", l.new_pos),
                        labrat_git::diff::LineKind::Removed => ("old", l.old_pos),
                        _ => continue,
                    };
                    if kind == line_type && number as i64 == line {
                        let code = labrat_git::diff::line_code(&ch.new_path, l.old_pos, l.new_pos);
                        found = Some((code, json!({"path": ch.new_path, "line": line, "line_type": kind})));
                        break;
                    }
                }
                break;
            }
        }
        let now = time::now();
        let discussion = hex::encode(rand::random::<[u8; 20]>());
        let (kind, code, info) = match &found {
            Some((code, info)) => (Some("LegacyDiffNote"), Some(code.clone()), Some(info.to_string())),
            None => (None, None, None),
        };
        c.execute(
            "INSERT INTO notes (note, noteable_type, noteable_id, project_id, author_id, type, discussion_id, line_code, commit_id,
                original_position, created_at, updated_at)
             VALUES (?1, 'Commit', 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![note, p.id, user.id, kind, discussion, code, oid.hex(), info, now],
        )?;
        let id = c.last_insert_rowid();
        after_commit_note(c, &req, &p, &user, id)?;
        let row: Row = (id, note.clone(), user.id, info, now);
        entity(c, &req, &row)
    })?;
    Ok(Resp::created(body))
}

// ---------------------------------------------------------------------------------------------
// Discussions on commits (`lib/api/discussions.rb` with `noteable_type == Commit`).

use crate::entities::note as note_ent;
use labrat_core::model::Project;
use labrat_core::services::notes::{self, NewNote, Note};
use labrat_core::services::positions::Position;
use labrat_git::Oid;

fn commit_oid(c: &rusqlite::Connection, req: &Req, p: &Project) -> Result<Oid, ApiError> {
    let r = repo::repo(c, p);
    let sha = req.path_param("sha");
    Ok(r.resolve(sha)?.filter(|o| r.kind(o).ok().flatten() == Some(Kind::Commit)).ok_or(Error::not_found("Commit"))?)
}

fn readable(c: &rusqlite::Connection, req: &Req) -> Result<Project, ApiError> {
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !perms.read_code(&p) {
        return Err(Error::forbidden().into());
    }
    Ok(p)
}

fn writable(c: &rusqlite::Connection, req: &Req) -> Result<Project, ApiError> {
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !perms.read_code(&p) || perms.level == 0 && !perms.admin {
        return Err(Error::forbidden().into());
    }
    Ok(p)
}

/// The commit's notes grouped by discussion, in order of each discussion's first note.
fn discussions(c: &rusqlite::Connection, p: &Project, oid: &Oid) -> Result<Vec<Vec<Note>>, ApiError> {
    let mut st = c.prepare(
        "SELECT id FROM notes WHERE noteable_type = 'Commit' AND commit_id = ?1 AND project_id = ?2 ORDER BY created_at, id",
    )?;
    let ids: Vec<i64> = st.query_map(params![oid.hex(), p.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut out: Vec<Vec<Note>> = vec![];
    for id in ids {
        let Some(n) = Note::find(c, id)? else { continue };
        match out.iter_mut().find(|d| d[0].discussion_id == n.discussion_id) {
            Some(d) => d.push(n),
            None => out.push(vec![n]),
        }
    }
    Ok(out)
}

fn discussion(c: &rusqlite::Connection, p: &Project, oid: &Oid, id: &str, what: &'static str) -> Result<Vec<Note>, ApiError> {
    discussions(c, p, oid)?
        .into_iter()
        .find(|d| d[0].discussion_id == id)
        .ok_or_else(|| Error::not_found(what).into())
}

fn ctx(req: &Req) -> Ctx<'_> {
    Ctx { base_url: &req.app.config.external_url, ee: req.app.config.is_ee(), current_user: req.user.as_ref() }
}

async fn list_discussions(mut req: Req) -> ApiResult {
    // Grape names the commit `noteable_id` in pagination links.
    for (k, _) in req.path_params.iter_mut() {
        if k == "sha" {
            *k = "noteable_id".into();
        }
    }
    let app = req.app.clone();
    app.db.read(|c| {
        let p = readable(c, &req)?;
        let r = repo::repo(c, &p);
        let sha = req.path_param("noteable_id").to_string();
        let oid = r.resolve(&sha)?.filter(|o| r.kind(o).ok().flatten() == Some(Kind::Commit)).ok_or(Error::not_found("Commit"))?;
        let list = discussions(c, &p, &oid)?;
        paginate(&req, list, &[], |d| Ok(note_ent::commit_discussion(c, &ctx(&req), d)?))
    })
}

async fn show_discussion(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let p = readable(c, &req)?;
        let oid = commit_oid(c, &req, &p)?;
        let d = discussion(c, &p, &oid, req.path_param("discussion_id"), "Discussion")?;
        Ok::<_, ApiError>(note_ent::commit_discussion(c, &ctx(&req), &d)?)
    })?;
    Ok(Resp::ok(body))
}

async fn discussion_notes(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let p = readable(c, &req)?;
        let oid = commit_oid(c, &req, &p)?;
        let d = discussion(c, &p, &oid, req.path_param("discussion_id"), "Notes")?;
        let list = d.iter().map(|n| note_ent::commit_note(c, &ctx(&req), n)).collect::<labrat_core::Result<Vec<_>>>()?;
        Ok::<_, ApiError>(Value::Array(list))
    })?;
    Ok(Resp::ok(body))
}

/// The diff line a position points at in the commit's diff, as a line code.
fn position_line_code(r: &labrat_git::Repo, oid: &Oid, pos: &Value) -> Result<Option<String>, ApiError> {
    let commit = r.commit(oid)?;
    let parent_tree = match commit.parents.first() {
        Some(pp) => Some(r.commit(pp)?.tree),
        None => None,
    };
    let path = pos["new_path"].as_str().or(pos["old_path"].as_str()).unwrap_or_default();
    let (old_line, new_line) = (pos["old_line"].as_u64(), pos["new_line"].as_u64());
    for ch in labrat_git::diff::diff_trees(r, parent_tree.as_ref(), Some(&commit.tree))? {
        if ch.new_path != path && ch.old_path != path {
            continue;
        }
        let old = if ch.new_file { vec![] } else { r.blob(&ch.old_oid)? };
        let new = if ch.deleted_file { vec![] } else { r.blob(&ch.new_oid)? };
        for l in labrat_git::diff::diff_lines(&old, &new) {
            let hit = match (l.kind, old_line, new_line) {
                (labrat_git::diff::LineKind::Added, None, Some(n)) => l.new_pos as u64 == n,
                (labrat_git::diff::LineKind::Removed, Some(o), None) => l.old_pos as u64 == o,
                (labrat_git::diff::LineKind::Context, Some(o), Some(n)) => l.old_pos as u64 == o && l.new_pos as u64 == n,
                _ => false,
            };
            if hit {
                return Ok(Some(labrat_git::diff::line_code(&ch.new_path, l.old_pos, l.new_pos)));
            }
        }
    }
    Ok(None)
}

async fn create_discussion(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    req.params.require(&["body"])?;
    let body = req.params.str("body").unwrap_or_default();
    let position = req.params.raw("position").cloned().filter(Value::is_object);
    let app = req.app.clone();
    let out = app.db.tx(|c| {
        let p = writable(c, &req)?;
        let oid = commit_oid(c, &req, &p)?;
        if body.trim().is_empty() {
            return Err(Error::message(400, "400 Bad request - Note {:note=>[\"can't be blank\"]}").into());
        }
        let (kind, stored, code) = match &position {
            Some(pos) => {
                let r = repo::repo(c, &p);
                let Some(code) = position_line_code(&r, &oid, pos)? else {
                    return Err(Error::message(400, "400 Bad request - Note {:line_code=>[\"can't be blank\", \"must be a valid line code\"]}").into());
                };
                let stored = Position::parse(&pos.to_string()).map(|p| p.to_json().to_string());
                ("DiffNote", stored, Some(code))
            }
            None => ("DiscussionNote", None, None),
        };
        let n = notes::insert(
            c,
            &user,
            NewNote {
                note: body.clone(),
                noteable_type: "Commit".into(),
                noteable_id: 0,
                project_id: Some(p.id),
                kind: Some(kind.into()),
                position: stored.clone(),
                original_position: stored,
                line_code: code,
                commit_id: Some(oid.hex()),
                ..Default::default()
            },
        )?;
        after_commit_note(c, &req, &p, &user, n.id)?;
        Ok::<_, ApiError>(note_ent::commit_discussion(c, &ctx(&req), &[n])?)
    })?;
    Ok(Resp::created(out))
}

async fn reply(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    req.params.require(&["body"])?;
    let body = req.params.str("body").unwrap_or_default();
    let app = req.app.clone();
    let out = app.db.tx(|c| {
        let p = writable(c, &req)?;
        let oid = commit_oid(c, &req, &p)?;
        let d = discussion(c, &p, &oid, req.path_param("discussion_id"), "Discussion")?;
        let first = &d[0];
        if first.system {
            return Err(Error::message(400, "400 Bad request - Replies to system notes are not allowed.").into());
        }
        if body.trim().is_empty() {
            return Err(Error::message(400, "400 Bad request - Note {:note=>[\"can't be blank\"]}").into());
        }
        // Commits aren't `replyable_types`: a plain comment can't become a thread.
        if first.kind.is_none() {
            return Err(Error::message(400, "400 Bad request - Discussion can not be replied to.").into());
        }
        // Replies keep the discussion's note type (line comments stay `LegacyDiffNote`).
        let kind = match first.kind.as_deref() {
            Some("DiffNote") => "DiffNote",
            Some("LegacyDiffNote") => "LegacyDiffNote",
            _ => "DiscussionNote",
        };
        let n = notes::insert(
            c,
            &user,
            NewNote {
                note: body.clone(),
                noteable_type: "Commit".into(),
                noteable_id: 0,
                project_id: Some(p.id),
                kind: Some(kind.into()),
                discussion_id: Some(first.discussion_id.clone()),
                position: first.position.clone(),
                original_position: first.original_position.clone(),
                line_code: first.line_code.clone(),
                commit_id: Some(oid.hex()),
                ..Default::default()
            },
        )?;
        after_commit_note(c, &req, &p, &user, n.id)?;
        Ok::<_, ApiError>(note_ent::commit_note(c, &ctx(&req), &n)?)
    })?;
    Ok(Resp::created(out))
}

fn discussion_note(c: &rusqlite::Connection, req: &Req, p: &Project) -> Result<Note, ApiError> {
    let oid = commit_oid(c, req, p)?;
    let d = discussion(c, p, &oid, req.path_param("discussion_id"), "Discussion")?;
    let id: i64 = req.path_param("note_id").parse().unwrap_or(-1);
    d.into_iter().find(|n| n.id == id).ok_or_else(|| Error::not_found("Note").into())
}

async fn update_note(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let body = req.params.str("body");
    let app = req.app.clone();
    let out = app.db.tx(|c| {
        let p = writable(c, &req)?;
        let n = discussion_note(c, &req, &p)?;
        if n.author_id != user.id && !user.admin {
            return Err(Error::forbidden().into());
        }
        if let Some(b) = body.filter(|b| *b != n.note) {
            let now = time::now();
            c.execute(
                "UPDATE notes SET note = ?1, updated_at = ?2, last_edited_at = ?2, updated_by_id = ?3 WHERE id = ?4",
                params![b, now, user.id, n.id],
            )?;
        }
        let n = Note::find(c, n.id)?.expect("exists");
        Ok::<_, ApiError>(note_ent::commit_note(c, &ctx(&req), &n)?)
    })?;
    Ok(Resp::ok(out))
}

async fn delete_note(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let app = req.app.clone();
    app.db.tx(|c| {
        let p = writable(c, &req)?;
        let n = discussion_note(c, &req, &p)?;
        if n.author_id != user.id && !user.admin {
            return Err(Error::forbidden().into());
        }
        c.execute("DELETE FROM notes WHERE id = ?1", [n.id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

/// What a new commit comment triggers: note hooks, mention to-dos and (asynchronously) the
/// "commented on" event.
fn after_commit_note(c: &rusqlite::Connection, req: &Req, p: &Project, user: &User, id: i64) -> Result<(), ApiError> {
    let Some(n) = Note::find(c, id)? else { return Ok(()) };
    labrat_core::services::hooks::commit_note_event(c, &req.app.config, p, &n, "create")?;
    if let Some(sha) = n.commit_id.clone() {
        labrat_core::services::mentions::commit_note_todos(c, p, &sha, user, &n)?;
    }
    labrat_core::jobs::enqueue(c, "note_event", json!({"note_id": n.id, "user_id": user.id}))?;
    Ok(())
}
