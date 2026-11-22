//! `lib/api/{notes,discussions,draft_notes,award_emoji,suggestions,merge_request_approvals}.rb`
//! for merge requests.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::note as ent;
use crate::entities::user::Ctx;
use crate::pagination::paginate;
use crate::routes::merge_requests::load;
use axum::Router;
use axum::routing::{get, post, put};
use labrat_core::model::{Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::comments::{self, CreateParams, Outcome};
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::notes::Note;
use labrat_core::services::positions::Position;
use labrat_core::services::{emoji, reviews, suggestions};
use labrat_core::{App, Error};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    let mr = "/projects/{id}/merge_requests/{merge_request_iid}";
    let nb = "/projects/{id}/merge_requests/{noteable_id}";
    Router::new()
        .route(&format!("{nb}/notes"), get(list_notes).post(create_note))
        .route(
            &format!("{nb}/notes/{{note_id}}"),
            get(show_note).put(update_note).delete(delete_note),
        )
        .route(
            &format!("{nb}/discussions"),
            get(list_discussions).post(create_discussion),
        )
        .route(
            &format!("{nb}/discussions/{{discussion_id}}"),
            get(show_discussion).put(resolve_discussion),
        )
        .route(
            &format!("{nb}/discussions/{{discussion_id}}/notes"),
            get(discussion_notes).post(reply),
        )
        .route(
            &format!("{nb}/discussions/{{discussion_id}}/notes/{{note_id}}"),
            get(show_note)
                .put(update_discussion_note)
                .delete(delete_note),
        )
        .route(
            &format!("{mr}/draft_notes"),
            get(list_drafts).post(create_draft),
        )
        .route(
            &format!("{mr}/draft_notes/bulk_publish"),
            post(bulk_publish),
        )
        .route(
            &format!("{mr}/draft_notes/{{draft_note_id}}"),
            get(show_draft).put(update_draft).delete(delete_draft),
        )
        .route(
            &format!("{mr}/draft_notes/{{draft_note_id}}/publish"),
            put(publish_draft),
        )
        .route(
            &format!("{mr}/award_emoji"),
            get(list_awards).post(add_award),
        )
        .route(
            &format!("{mr}/award_emoji/{{award_id}}"),
            get(show_award).delete(remove_award),
        )
        .route(
            &format!("{mr}/notes/{{note_id}}/award_emoji"),
            get(list_awards).post(add_award),
        )
        .route(
            &format!("{mr}/notes/{{note_id}}/award_emoji/{{award_id}}"),
            get(show_award).delete(remove_award),
        )
        .route("/suggestions/{id}/apply", put(apply_suggestion))
        .route("/suggestions/batch_apply", put(batch_apply))
        .route(&format!("{mr}/approvals"), get(approvals).post(change_approvals_required))
        .route(&format!("{mr}/approve"), post(approve))
        .route(&format!("{mr}/unapprove"), post(unapprove))
        .route(&format!("{mr}/reset_approvals"), put(reset_approvals))
}

pub fn ectx(req: &Req) -> Ctx<'_> {
    Ctx {
        base_url: &req.app.config.external_url, ee: req.app.config.is_ee(),
        current_user: req.user.as_ref(),
    }
}

/// MR for note routes: 404 `MergeRequest Not Found` when missing or unreadable.
fn load_noteable(
    c: &Connection,
    req: &Req,
) -> Result<(Project, ProjectPerms, MergeRequest), ApiError> {
    req.user()?;
    let (p, perms) = crate::find::project(c, req, req.path_param("id"))?;
    let iid: i64 = req
        .path_param("noteable_id")
        .parse()
        .map_err(|_| Error::Param("noteable_id is invalid".into()))?;
    let mr = MergeRequest::find_by_iid(c, p.id, iid)?;
    match mr {
        Some(mr) if perms.read_merge_request(&p) => Ok((p, perms, mr)),
        _ => Err(Error::Api(404, json!("404 Not found")).into()),
    }
}

fn note_param(req: &Req) -> Result<i64, ApiError> {
    req.path_param("note_id")
        .parse()
        .map_err(|_| Error::Param("note_id is invalid".into()).into())
}

fn find_note(c: &Connection, mr: &MergeRequest, id: i64) -> Result<Note, ApiError> {
    Note::find(c, id)?
        .filter(|n| n.noteable_type == "MergeRequest" && n.noteable_id == mr.id)
        .ok_or_else(|| Error::Api(404, json!("404 Not found")).into())
}

fn visible(n: &Note, perms: &ProjectPerms) -> bool {
    !n.internal || perms.reporter()
}

fn outcome(
    c: &Connection,
    req: &Req,
    p: &Project,
    mr: &MergeRequest,
    o: Outcome,
    as_discussion: bool,
    created: bool,
) -> ApiResult {
    match o {
        Outcome::Created(n, changes) => {
            let body = if as_discussion {
                let notes = Note::in_discussion(c, &n.discussion_id)?;
                let mut v = ent::discussion(c, &ectx(req), p, mr, &notes)?;
                if let Some(first) = v["notes"].get_mut(0) {
                    first["commands_changes"] = Value::Object(changes);
                }
                v
            } else {
                ent::note_with_changes(c, &ectx(req), p, mr, &n, changes)?
            };
            Ok(if created {
                Resp::created(body)
            } else {
                Resp::ok(body)
            })
        }
        Outcome::Commands(changes, summary) => Ok(Resp::status(
            202,
            json!({"commands_changes": Value::Object(changes), "summary": summary}),
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Notes

async fn list_notes(req: Req) -> ApiResult {
    let order_by = req
        .params
        .one_of("order_by", &["created_at", "updated_at"])?
        .unwrap_or("created_at".into());
    let desc = req.params.one_of("sort", &["asc", "desc"])?.as_deref() != Some("asc");
    let filter = req.params.one_of(
        "activity_filter",
        &["all_notes", "only_comments", "only_activity"],
    )?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms, mr) = load_noteable(c, &req)?;
        let mut list = Note::for_noteable(c, "MergeRequest", mr.id)?;
        match filter.as_deref() {
            Some("only_comments") => list.retain(|n| !n.system),
            Some("only_activity") => list.retain(|n| n.system),
            _ => {}
        }
        list.sort_by(|a, b| {
            let o = if order_by == "updated_at" {
                a.updated_at.cmp(&b.updated_at).then(a.id.cmp(&b.id))
            } else {
                a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id))
            };
            if desc { o.reverse() } else { o }
        });
        let defaults = [
            ("activity_filter", "all_notes"),
            ("order_by", "created_at"),
            ("sort", "desc"),
        ];
        let mut resp = paginate(&req, list, &defaults, |n| {
            if visible(n, &perms) {
                Ok(ent::note(c, &ectx(&req), &p, &mr, n)?)
            } else {
                Ok(Value::Null)
            }
        })?;
        if let Some(Value::Array(a)) = &mut resp.body {
            a.retain(|v| !v.is_null());
        }
        Ok(resp)
    })
}

async fn show_note(req: Req) -> ApiResult {
    let id = note_param(&req)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, perms, mr) = load_noteable(c, &req)?;
        let n = find_note(c, &mr, id)?;
        if let Some(did) = req
            .path_params
            .iter()
            .find(|(k, _)| k == "discussion_id")
            .map(|(_, v)| v.clone())
        {
            let _ = did;
        }
        if !visible(&n, &perms) {
            return Err(Error::not_found("Note").into());
        }
        Ok::<_, ApiError>(ent::note(c, &ectx(&req), &p, &mr, &n)?)
    })?;
    Ok(Resp::ok(body))
}

fn create_params(req: &Req) -> Result<CreateParams, ApiError> {
    req.params.require(&["body"])?;
    Ok(CreateParams {
        body: req.params.str("body").unwrap_or_default(),
        internal: req
            .params
            .bool("internal")?
            .or(req.params.bool("confidential")?)
            .unwrap_or(false),
        // An unparseable `created_at` is dropped (the note gets the current time).
        created_at: req.params.str("created_at").and_then(|t| labrat_core::time::parse_time(&t)),
        merge_request_diff_head_sha: req.params.str("merge_request_diff_head_sha"),
        ..Default::default()
    })
}

async fn create_note(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let prm = create_params(&req)?;
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _, mr) = load_noteable(c, &req)?;
        let o = comments::create(c, &req.app.config, &p, &mr, &me, prm)?;
        let mr = mr.reload(c)?;
        outcome(c, &req, &p, &mr, o, false, true)
    });
    crate::jobs::kick(&req.app);
    result
}

async fn update_note(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id = note_param(&req)?;
    let body = req.params.str("body");
    if body.as_deref() == Some("") {
        return Err(Error::Param("body is empty".into()).into());
    }
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _, mr) = load_noteable(c, &req)?;
        let n = find_note(c, &mr, id)?;
        if !comments::can_admin_note(c, &me, &p, &mr, &n)? {
            return Err(Error::forbidden().into());
        }
        let Some(body) = body else {
            return Ok(Resp::ok(ent::note(c, &ectx(&req), &p, &mr, &n)?));
        };
        let o = comments::update(c, &req.app.config, &p, &mr, &me, &n, &body)?;
        outcome(c, &req, &p, &mr, o, false, false)
    });
    crate::jobs::kick(&req.app);
    result
}

async fn delete_note(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id = note_param(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _, mr) = load_noteable(c, &req)?;
        let n = find_note(c, &mr, id)?;
        if !comments::can_admin_note(c, &me, &p, &mr, &n)? {
            return Err(Error::forbidden().into());
        }
        req.check_unmodified_since(n.updated_at)?;
        comments::delete(c, &n)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

// ---------------------------------------------------------------------------------------------
// Discussions

/// Notes grouped by discussion, ordered by each discussion's first note.
fn grouped(c: &Connection, mr: &MergeRequest) -> Result<Vec<Vec<Note>>, ApiError> {
    let mut groups: Vec<Vec<Note>> = vec![];
    for n in Note::for_noteable(c, "MergeRequest", mr.id)? {
        match groups
            .iter_mut()
            .find(|g| g[0].discussion_id == n.discussion_id)
        {
            Some(g) => g.push(n),
            None => groups.push(vec![n]),
        }
    }
    Ok(groups)
}

async fn list_discussions(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms, mr) = load_noteable(c, &req)?;
        let groups = grouped(c, &mr)?;
        let mut resp = paginate(&req, groups, &[], |g| {
            let notes: Vec<Note> = g.iter().filter(|n| visible(n, &perms)).cloned().collect();
            if notes.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(ent::discussion(c, &ectx(&req), &p, &mr, &notes)?)
            }
        })?;
        if let Some(Value::Array(a)) = &mut resp.body {
            a.retain(|v| !v.is_null());
        }
        Ok(resp)
    })
}

fn discussion_notes_for(
    c: &Connection,
    req: &Req,
    mr: &MergeRequest,
    perms: &ProjectPerms,
) -> Result<Vec<Note>, ApiError> {
    let did = req.path_param("discussion_id");
    Ok(Note::in_discussion(c, did)?
        .into_iter()
        .filter(|n| {
            n.noteable_type == "MergeRequest" && n.noteable_id == mr.id && visible(n, perms)
        })
        .collect())
}

async fn show_discussion(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, perms, mr) = load_noteable(c, &req)?;
        let notes = discussion_notes_for(c, &req, &mr, &perms)?;
        if notes.is_empty() {
            return Err(Error::not_found("Discussion").into());
        }
        Ok::<_, ApiError>(ent::discussion(c, &ectx(&req), &p, &mr, &notes)?)
    })?;
    Ok(Resp::ok(body))
}

async fn discussion_notes(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms, mr) = load_noteable(c, &req)?;
        let notes = discussion_notes_for(c, &req, &mr, &perms)?;
        if notes.is_empty() {
            return Err(Error::not_found("Notes").into());
        }
        let body: Vec<Value> = notes
            .iter()
            .map(|n| ent::note(c, &ectx(&req), &p, &mr, n))
            .collect::<Result<_, _>>()?;
        Ok(Resp::ok(Value::Array(body)))
    })
}

fn position_param(req: &Req) -> Result<Option<Position>, ApiError> {
    let Some(v) = req.params.raw("position") else {
        return Ok(None);
    };
    if !v.is_object() {
        return Err(Error::Param("position is invalid".into()).into());
    }
    let mut missing = vec![];
    for k in ["base_sha", "start_sha", "head_sha", "position_type"] {
        if v.get(k).is_none_or(|x| x.is_null()) {
            missing.push(format!("position[{k}] is missing"));
        }
    }
    if !missing.is_empty() {
        return Err(Error::Param(missing.join(", ")).into());
    }
    // Integer fields must coerce.
    for k in ["new_line", "old_line", "width", "height", "x", "y"] {
        let ok = match v.get(k) {
            None | Some(Value::Null) => true,
            Some(Value::Number(n)) => n.is_i64(),
            Some(Value::String(s)) => s.trim().parse::<i64>().is_ok(),
            _ => false,
        };
        if !ok {
            return Err(Error::Param(format!("position[{k}] is invalid")).into());
        }
    }
    let pt = v["position_type"].as_str().unwrap_or("");
    if !["text", "image", "file"].contains(&pt) {
        return Err(
            Error::Param("position[position_type] does not have a valid value".into()).into(),
        );
    }
    Ok(Some(Position::from_json(v)))
}

async fn create_discussion(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let mut prm = create_params(&req)?;
    // `POST .../discussions` declares neither `internal` nor `merge_request_diff_head_sha`.
    prm.internal = false;
    prm.merge_request_diff_head_sha = None;
    prm.thread = true;
    prm.position = position_param(&req)?;
    // `opts[:commit_id] = params[:commit_id]` for MR diff notes.
    if prm.position.is_some() {
        prm.commit_id = req.params.str("commit_id").filter(|s| !s.is_empty());
    }
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _, mr) = load_noteable(c, &req)?;
        let o = comments::create(c, &req.app.config, &p, &mr, &me, prm)?;
        outcome(c, &req, &p, &mr, o, true, true)
    });
    crate::jobs::kick(&req.app);
    result
}

async fn reply(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let mut prm = create_params(&req)?;
    let did = req.path_param("discussion_id").to_string();
    prm.reply_to = Some(did.clone());
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, perms, mr) = load_noteable(c, &req)?;
        let notes = discussion_notes_for(c, &req, &mr, &perms)?;
        let Some(first) = notes.first() else {
            return Err(Error::not_found("Discussion").into());
        };
        if first.system {
            return Err(
                Error::BadRequest(Some("Replies to system notes are not allowed.".into())).into(),
            );
        }
        let o = comments::create(c, &req.app.config, &p, &mr, &me, prm)?;
        outcome(c, &req, &p, &mr, o, false, true)
    });
    crate::jobs::kick(&req.app);
    result
}

async fn update_discussion_note(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id = note_param(&req)?;
    let has_body = req.params.has("body");
    let has_resolved = req.params.has("resolved");
    if has_body && has_resolved {
        return Err(Error::Param("body, resolved are mutually exclusive".into()).into());
    }
    if !has_body && !has_resolved {
        return Err(Error::Param(
            "body, resolved are missing, exactly one parameter must be provided".into(),
        )
        .into());
    }
    let resolved = req.params.bool("resolved")?;
    let body = req.params.str("body");
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _, mr) = load_noteable(c, &req)?;
        let n = find_note(c, &mr, id)?;
        if let Some(res) = resolved {
            if !comments::can_resolve(c, &me, &p, &mr, &n)? {
                return Err(Error::forbidden().into());
            }
            if !n.resolvable || n.system {
                return Err(Error::BadRequest(Some("Note is not resolvable".into())).into());
            }
            comments::resolve_note(c, &req.app.config, &p, &mr, &me, &n, res)?;
            let n = Note::find(c, n.id)?.unwrap();
            return Ok(Resp::ok(ent::note(c, &ectx(&req), &p, &mr, &n)?));
        }
        if !comments::can_admin_note(c, &me, &p, &mr, &n)? {
            return Err(Error::forbidden().into());
        }
        let o = comments::update(
            c,
            &req.app.config,
            &p,
            &mr,
            &me,
            &n,
            &body.unwrap_or_default(),
        )?;
        outcome(c, &req, &p, &mr, o, false, false)
    });
    crate::jobs::kick(&req.app);
    result
}

async fn resolve_discussion(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    req.params.require(&["resolved"])?;
    let resolved = req.params.bool("resolved")?.unwrap_or(false);
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms, mr) = load_noteable(c, &req)?;
        let notes = discussion_notes_for(c, &req, &mr, &perms)?;
        let Some(first) = notes.first() else {
            return Err(Error::not_found("Discussion").into());
        };
        let resolvable = first.kind.is_some() && notes.iter().any(|n| n.resolvable && !n.system);
        if !resolvable || !comments::can_resolve(c, &me, &p, &mr, first)? {
            return Err(Error::forbidden().into());
        }
        comments::resolve_discussion(
            c,
            &req.app.config,
            &p,
            &mr,
            &me,
            &first.discussion_id,
            resolved,
        )?;
        let notes = discussion_notes_for(c, &req, &mr, &perms)?;
        Ok::<_, ApiError>(ent::discussion(c, &ectx(&req), &p, &mr, &notes)?)
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

// ---------------------------------------------------------------------------------------------
// Draft notes

fn draft_id(req: &Req) -> Result<i64, ApiError> {
    req.path_param("draft_note_id")
        .parse()
        .map_err(|_| Error::Param("draft_note_id is invalid".into()).into())
}

async fn list_drafts(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        Ok::<_, ApiError>(Value::Array(
            reviews::drafts(c, &mr, &me)?
                .iter()
                .map(ent::draft)
                .collect(),
        ))
    })?;
    Ok(Resp::ok(body))
}

async fn show_draft(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id = draft_id(&req)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        let d = reviews::find_draft(c, &mr, &me, id)?
            .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        Ok::<_, ApiError>(ent::draft(&d))
    })?;
    Ok(Resp::ok(body))
}

async fn create_draft(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    req.params.require(&["note"])?;
    let note = req.params.str("note").unwrap_or_default();
    // An empty id is no discussion at all.
    let reply_to = req.params.str("in_reply_to_discussion_id").filter(|s| !s.is_empty());
    let resolve = req.params.bool_or("resolve_discussion", false)?;
    let position = position_param(&req)?;
    let commit_id = req.params.str("commit_id").filter(|s| s != "undefined");
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        // `authorize_create_note!` calls `access_denied!`, which the draft notes API doesn't
        // define: a denied draft is a NoMethodError, so a 500.
        if !labrat_core::services::comments::can_create_note(c, &me, &p, &mr)? {
            return Err(Error::Api(500, json!("500 Internal Server Error")).into());
        }
        match reviews::create_draft(
            c,
            &p,
            &mr,
            &me,
            &note,
            reply_to.as_deref(),
            resolve,
            position,
            commit_id.as_deref(),
        )? {
            Ok(d) => Ok::<_, ApiError>(ent::draft(&d)),
            Err(errs) => Err(Error::Validation(errs).into()),
        }
    })?;
    Ok(Resp::created(body))
}

async fn update_draft(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id = draft_id(&req)?;
    let note = req.params.str("note");
    if note.as_deref() == Some("") {
        return Err(Error::Param("note is empty".into()).into());
    }
    let position = position_param(&req)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (_, _, mr) = load(c, &req)?;
        let Some(note) = note.filter(|n| !n.is_empty()) else {
            return Err(Error::BadRequest(Some("Missing params to modify".into())).into());
        };
        let d = reviews::find_draft(c, &mr, &me, id)?
            .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        reviews::update_draft(c, &d, &note, position)?;
        let d = reviews::find_draft(c, &mr, &me, id)?.unwrap();
        Ok::<_, ApiError>(ent::draft(&d))
    })?;
    Ok(Resp::ok(body))
}

async fn delete_draft(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id = draft_id(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (_, _, mr) = load(c, &req)?;
        let d = reviews::find_draft(c, &mr, &me, id)?
            .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        reviews::delete_draft(c, &mr, &me, &d)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn publish_draft(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id = draft_id(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        let d = reviews::find_draft(c, &mr, &me, id)?
            .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        reviews::publish_draft(c, &req.app.config, &p, &mr, &me, &d)?;
        Ok::<_, ApiError>(())
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::no_content())
}

async fn bulk_publish(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        reviews::publish_drafts(c, &req.app.config, &p, &mr, &me)?;
        Ok::<_, ApiError>(())
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::no_content())
}

// ---------------------------------------------------------------------------------------------
// Award emoji

/// The awardable: the MR or one of its notes.
fn awardable(
    c: &Connection,
    req: &Req,
    mr: &MergeRequest,
) -> Result<(&'static str, i64, Option<Note>), ApiError> {
    if req.path_params.iter().any(|(k, _)| k == "note_id") {
        let n = find_note(c, mr, note_param(req)?)?;
        Ok(("Note", n.id, Some(n)))
    } else {
        Ok(("MergeRequest", mr.id, None))
    }
}

async fn list_awards(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        let (kind, id, _) = awardable(c, &req, &mr)?;
        let list = emoji::list(c, kind, id)?;
        paginate(&req, list, &[], |a| Ok(ent::award(c, &ectx(&req), a)?))
    })
}

async fn show_award(req: Req) -> ApiResult {
    let aid: i64 = req
        .path_param("award_id")
        .parse()
        .map_err(|_| Error::Param("award_id is invalid".into()))?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        let (kind, id, _) = awardable(c, &req, &mr)?;
        let a = emoji::find(c, kind, id, aid)?
            .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        Ok::<_, ApiError>(ent::award(c, &ectx(&req), &a)?)
    })?;
    Ok(Resp::ok(body))
}

async fn add_award(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    req.params.require(&["name"])?;
    let name = req.params.str("name").unwrap_or_default();
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms, mr) = load(c, &req)?;
        let (kind, id, note) = awardable(c, &req, &mr)?;
        let locked_out = mr.discussion_locked == Some(true) && perms.level == 0 && !perms.admin;
        // `award_emoji` needs `read_note`, which internal notes restrict to Reporter+.
        let hidden = note.as_ref().is_some_and(|n| n.internal) && perms.level < 20 && !perms.admin;
        // `award_emoji` is a write: archived projects refuse it as not found.
        if locked_out || hidden || p.archived {
            return Err(Error::not_found("Award Emoji").into());
        }
        let system = note.as_ref().is_some_and(|n| n.system);
        match emoji::add(c, &me, kind, id, &name, system)? {
            Ok(a) => {
                labrat_core::services::hooks::emoji_event(
                    c,
                    &req.app.config,
                    &p,
                    &mr,
                    note.as_ref(),
                    &me,
                    &a,
                    "award",
                )?;
                Ok::<_, ApiError>(ent::award(c, &ectx(&req), &a)?)
            }
            Err(msg) => {
                Err(Error::Api(404, json!(format!("404 Award Emoji {msg} Not Found"))).into())
            }
        }
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::created(body))
}

async fn remove_award(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let aid: i64 = req
        .path_param("award_id")
        .parse()
        .map_err(|_| Error::Param("award_id is invalid".into()))?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        let (kind, id, note) = awardable(c, &req, &mr)?;
        let a = emoji::find(c, kind, id, aid)?
            .ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        if a.user_id != me.id && !me.admin {
            return Err(Error::Unauthorized(None).into());
        }
        req.check_unmodified_since(a.updated_at)?;
        emoji::remove(c, &a)?;
        labrat_core::services::hooks::emoji_event(
            c,
            &req.app.config,
            &p,
            &mr,
            note.as_ref(),
            &me,
            &a,
            "revoke",
        )?;
        Ok::<_, ApiError>(())
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::no_content())
}

// ---------------------------------------------------------------------------------------------
// Suggestions

fn suggestion_value(c: &Connection, s: &suggestions::Suggestion) -> Result<Value, ApiError> {
    let note = Note::find(c, s.note_id)?.unwrap();
    let mr = MergeRequest::get(c, note.noteable_id)?;
    let p = Project::find(c, mr.target_project_id)?.unwrap();
    Ok(ent::suggestion(c, &p, &mr, &note, s)?)
}

fn can_apply(c: &Connection, user: &User, s: &suggestions::Suggestion) -> Result<bool, ApiError> {
    let note = Note::find(c, s.note_id)?.unwrap();
    let mr = MergeRequest::get(c, note.noteable_id)?;
    let p = Project::find(c, mr.target_project_id)?.unwrap();
    Ok(labrat_core::repo::can_push_to_branch(
        c,
        user,
        &p,
        &mr.source_branch,
    )?)
}

async fn apply_suggestion(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let id: i64 = req
        .path_param("id")
        .parse()
        .map_err(|_| Error::Param("id is invalid".into()))?;
    let message = req.params.str("commit_message");
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let Some(s) = suggestions::find(c, id)? else {
            return Err::<Value, ApiError>(
                Error::Api(
                    404,
                    json!("Suggestion is not applicable as the suggestion was not found."),
                )
                .into(),
            );
        };
        if !can_apply(c, &me, &s)? {
            return Err(Error::forbidden().into());
        }
        let before = suggestion_value(c, &s)?;
        match suggestions::apply(c, &req.app.config, &me, &[id], message.as_deref())? {
            Ok(()) => Ok(before),
            Err(msg) => Err(Error::Api(400, json!(msg)).into()),
        }
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

async fn batch_apply(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    req.params.require(&["ids"])?;
    let ids = req.params.int_list("ids")?.unwrap_or_default();
    if ids.is_empty() {
        return Err(Error::Param("ids is empty".into()).into());
    }
    let message = req.params.str("commit_message");
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let mut list = vec![];
        for id in &ids {
            match suggestions::find(c, *id)? {
                Some(s) if !list.iter().any(|x: &suggestions::Suggestion| x.id == s.id) => list.push(s),
                _ => {
                    return Err::<Value, ApiError>(Error::Api(404, json!("Suggestions are not applicable as one or more suggestions were not found.")).into());
                }
            }
        }
        for s in &list {
            if !can_apply(c, &me, s)? {
                return Err(Error::forbidden().into());
            }
        }
        // `Suggestion.id_in(ids)` comes back in id order.
        list.sort_by_key(|s| s.id);
        let before: Vec<Value> = list.iter().map(|s| suggestion_value(c, s)).collect::<Result<_, _>>()?;
        match suggestions::apply(c, &req.app.config, &me, &ids, message.as_deref())? {
            Ok(()) => Ok(Value::Array(before)),
            Err(msg) => Err(Error::Api(400, json!(msg)).into()),
        }
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::ok(body))
}

// ---------------------------------------------------------------------------------------------
// Approvals

async fn approvals(req: Req) -> ApiResult {
    let app = req.app.clone();
    if !app.config.is_ee() {
        let body = app.db.read(|c| {
            let (p, _, mr) = load(c, &req)?;
            Ok::<_, ApiError>(approvals_body(c, &req, &p, &mr, req.user.as_ref())?)
        })?;
        return Ok(Resp::ok(body));
    }
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        // EE's `/approvals` never answered `checking` in the recordings, even right after a
        // read had started a check: a pending check is settled before it answers.
        let mr = if matches!(mr.merge_status.as_str(), "unchecked" | "checking") && mr.is_open() {
            labrat_core::services::merge_requests::mergeability::check(c, &app.config, mr.id)?;
            mr.reload(c)?
        } else {
            mr
        };
        Ok::<_, ApiError>(approvals_body(c, &req, &p, &mr, req.user.as_ref())?)
    })?;
    Ok(Resp::ok(body))
}

async fn approve(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let sha = req.params.str("sha");
    let password = req.params.str("approval_password");
    let publish = req.params.bool_or("publish_review", false)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        let ee = req.app.config.is_ee();
        if ee {
            // EE: developers may approve; the author only when the project allows it.
            if labrat_core::services::approvals::eligibility(c, &p, &mr, &me)? == labrat_core::services::approvals::Eligibility::Forbidden {
                return Err(Error::forbidden().into());
            }
        } else if !reviews::can_approve(c, &p, &mr, &me)? {
            return Err(Error::forbidden().into());
        }
        if let Some(sha) = &sha {
            let head = mr.diff_head_sha(c, &p)?;
            if Some(sha) != head.as_ref() {
                return Err(Error::message(
                    409,
                    format!(
                        "SHA does not match HEAD of source branch: {}",
                        head.unwrap_or_default()
                    ),
                )
                .into());
            }
        }
        if publish {
            reviews::publish_drafts(c, &req.app.config, &p, &mr, &me)?;
        }
        if ee && labrat_core::services::approvals::eligibility(c, &p, &mr, &me)? != labrat_core::services::approvals::Eligibility::Yes {
            return Err(Error::Unauthorized(None).into());
        }
        // EE: approving may need the approver's password (`approval_password`).
        if ee && labrat_core::services::policies::password_required(c, &p, &mr)?
            && !password.as_deref().is_some_and(|pw| labrat_core::services::users::check_password(&me, pw))
        {
            return Err(Error::Unauthorized(None).into());
        }
        if !reviews::approve(c, &req.app.config, &p, &mr, &me)? {
            return Err(Error::Unauthorized(None).into());
        }
        let mr = mr.reload(c)?;
        Ok::<_, ApiError>(approvals_body(c, &req, &p, &mr, Some(&me))?)
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::created(body))
}

async fn unapprove(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _, mr) = load(c, &req)?;
        let allowed = if req.app.config.is_ee() {
            labrat_core::services::approvals::eligibility(c, &p, &mr, &me)? != labrat_core::services::approvals::Eligibility::Forbidden
        } else {
            reviews::can_approve(c, &p, &mr, &me)?
        };
        if !allowed {
            return Err(Error::forbidden().into());
        }
        if !reviews::unapprove(c, &req.app.config, &p, &mr, &me)? {
            return Err(Error::NotFound(None).into());
        }
        let mr = mr.reload(c)?;
        Ok::<_, ApiError>(approvals_body(c, &req, &p, &mr, Some(&me))?)
    })?;
    crate::jobs::kick(&req.app);
    Ok(Resp::created(body))
}

/// The approvals body: CE's `MergeRequestApprovals`, or EE's approval state.
fn approvals_body(c: &Connection, req: &Req, p: &Project, mr: &MergeRequest, current: Option<&User>) -> labrat_core::Result<Value> {
    if req.app.config.is_ee() {
        crate::routes::approvals::approvals_json(c, &ectx(req), &req.app.config, p, mr, current)
    } else {
        ent::merge_request_approvals(c, &ectx(req), p, mr, current)
    }
}

/// EE's deprecated `POST /approvals`: sets the MR's required approval count (its
/// `any_approver` rule). CE has no such route.
async fn change_approvals_required(req: Req) -> ApiResult {
    if !req.app.config.is_ee() {
        return Err(Error::Body(404, json!({"error": "404 Not Found"})).into());
    }
    let me = req.user()?.clone();
    req.params.require(&["approvals_required"])?;
    let n = req.params.int("approvals_required")?.unwrap_or(0);
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms, mr) = load(c, &req)?;
        if !(perms.admin || perms.admin_project() || (mr.author_id == me.id && perms.developer())) {
            return Err(Error::forbidden().into());
        }
        labrat_core::services::approvals::set_required(c, &mr, n)?;
        Ok::<_, ApiError>(approvals_body(c, &req, &p, &mr, Some(&me))?)
    })?;
    Ok(Resp::created(body))
}

async fn reset_approvals(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let app = req.app.clone();
    app.db.tx(|c| {
        let (_, _, mr) = load(c, &req)?;
        if !me.bot() || mr.state == "merged" {
            return Err(Error::Unauthorized(None).into());
        }
        c.execute("DELETE FROM approvals WHERE merge_request_id = ?1", [mr.id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp {
        status: axum::http::StatusCode::ACCEPTED,
        body: None,
        headers: vec![],
    })
}
