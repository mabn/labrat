//! Comments on merge requests: notes, threads, replies, diff notes, edits, deletes and
//! resolving (`Notes::CreateService`, `UpdateService`, `DestroyService`, resolve services).

use crate::model::{Project, User, access};
use crate::perms::ProjectPerms;
use crate::services::merge_requests::MergeRequest;
use crate::services::notes::{self, NewNote, Note};
use crate::services::positions::Position;
use crate::services::{quick_actions, suggestions, todos};
use crate::{Config, Error, Result, repo, time};
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};

/// Ruby `Hash#inspect` of model errors, as GitLab interpolates them into messages.
pub fn ruby_errors(errs: &[(&str, Vec<&str>)]) -> String {
    let parts: Vec<String> = errs
        .iter()
        .map(|(k, msgs)| {
            let m: Vec<String> = msgs.iter().map(|x| format!("{x:?}")).collect();
            format!(":{k}=>[{}]", m.join(", "))
        })
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn note_error(errs: &[(&str, Vec<&str>)]) -> Error {
    Error::BadRequest(Some(format!("Note {}", ruby_errors(errs))))
}

fn human_size(bytes: usize) -> String {
    // `number_to_human_size`: one decimal, trailing ".0" dropped.
    let mb = bytes as f64 / 1_048_576.0;
    let s = format!("{mb:.1}");
    format!("{} MiB", s.trim_end_matches(".0"))
}

const MAX_NOTE_BYTES: usize = 1_048_576;

pub fn is_member(c: &Connection, user: &User, p: &Project) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    Ok(perms.level >= access::GUEST)
}

/// `create_note` on the MR.
pub fn can_create_note(
    c: &Connection,
    user: &User,
    p: &Project,
    mr: &MergeRequest,
) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !perms.read_merge_request(p) || p.archived {
        return Ok(false);
    }
    if mr.discussion_locked == Some(true) && !(perms.level >= access::GUEST || perms.admin) {
        return Ok(false);
    }
    Ok(true)
}

/// `admin_note`: author, maintainer+ or admin; never for system notes.
pub fn can_admin_note(
    c: &Connection,
    user: &User,
    p: &Project,
    mr: &MergeRequest,
    note: &Note,
) -> Result<bool> {
    if note.system {
        return Ok(false);
    }
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !perms.read_merge_request(p) {
        return Ok(false);
    }
    if mr.discussion_locked == Some(true) && perms.level < access::GUEST && !perms.admin {
        return Ok(false);
    }
    Ok(note.author_id == user.id || perms.admin_project())
}

/// `resolve_note`: note author, MR author, developer+ or admin.
pub fn can_resolve(
    c: &Connection,
    user: &User,
    p: &Project,
    mr: &MergeRequest,
    note: &Note,
) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !perms.read_merge_request(p) {
        return Ok(false);
    }
    if mr.discussion_locked == Some(true) && perms.level < access::GUEST && !perms.admin {
        return Ok(false);
    }
    Ok(note.author_id == user.id || mr.author_id == user.id || perms.developer())
}

#[derive(Debug, Clone, Default)]
pub struct CreateParams {
    pub body: String,
    pub internal: bool,
    /// `None` = plain note; `Some(None)` = new thread; diff notes carry a position.
    pub thread: bool,
    pub position: Option<Position>,
    pub reply_to: Option<String>,
    pub created_at: Option<i64>,
    pub review_id: Option<i64>,
    pub merge_request_diff_head_sha: Option<String>,
    /// Set while publishing drafts: suppresses `/submit_review`-style re-publishing.
    pub publishing: bool,
    /// The saved note, for commands applied after saving (`/spend` records it).
    pub note_id: Option<i64>,
    /// Whether the author had a pending to-do before the note was saved: commands are
    /// interpreted (and `/todo`/`/done` offered) before saving, which resolves those to-dos.
    pub todo_pending: Option<bool>,
    /// Whether the author was subscribed before the note was saved (writing a note makes them a
    /// participant, but `/subscribe` and `/unsubscribe` are offered on the earlier state).
    pub subscribed_before: Option<bool>,
    /// `updated_at` before the saved note touched the MR: the commands' update reports the
    /// touch as a change.
    pub updated_before_touch: Option<i64>,
    /// Commands from the description of an MR being created: GitLab interprets them against the
    /// unsaved record.
    pub new_record: bool,
    /// Description commands of an update: the attribute changes are handed back (see
    /// `quick_actions::take_update`) to be merged into the update's own parameters.
    pub return_update: bool,
    /// `commit_id` of an MR diff note (`POST .../discussions` with a position, draft publish).
    pub commit_id: Option<String>,
}

pub enum Outcome {
    /// The note and the `commands_changes` of this request (not persisted, as in GitLab).
    Created(Note, Map<String, Value>),
    /// Only quick actions: `commands_changes` and the summary.
    Commands(Map<String, Value>, Vec<String>),
}

/// The first note of a discussion, if any.
pub fn first_note(c: &Connection, discussion_id: &str) -> Result<Option<Note>> {
    Ok(Note::in_discussion(c, discussion_id)?.into_iter().next())
}

/// Is the discussion resolvable and fully resolved?
pub fn discussion_resolved(notes: &[Note]) -> bool {
    let resolvable: Vec<&Note> = notes.iter().filter(|n| n.resolvable).collect();
    !resolvable.is_empty() && resolvable.iter().all(|n| n.resolved_at.is_some())
}

/// `Notes::CreateService` for a merge request.
pub fn create(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    prm: CreateParams,
) -> Result<Outcome> {
    // A quick action whose update fails validation fails the note: its full messages, joined,
    // under `:validation`.
    create_note(c, config, p, mr, user, prm).map_err(|e| match e {
        Error::Validation(list) => {
            let joined = list.iter().map(|(attr, msg)| full_message(attr, msg)).collect::<Vec<_>>().join(", ");
            note_error(&[("validation", vec![joined.as_str()])])
        }
        other => other,
    })
}

/// `ActiveModel::Errors#full_message`: `base` has no attribute; `timelogs.base` reads
/// `Timelogs`.
fn full_message(attr: &str, msg: &str) -> String {
    if attr == "base" {
        return msg.to_string();
    }
    let name = attr.strip_suffix(".base").unwrap_or(attr).replace('_', " ");
    let mut ch = name.chars();
    let name = match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    };
    format!("{name} {msg}")
}

fn create_note(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    prm: CreateParams,
) -> Result<Outcome> {
    if !can_create_note(c, user, p, mr)? {
        return Err(Error::forbidden());
    }
    let perms = ProjectPerms::load(c, Some(user), p)?;
    let mut kind: Option<&str> = if prm.position.is_some() {
        Some("DiffNote")
    } else if prm.thread {
        Some("DiscussionNote")
    } else {
        None
    };
    let mut discussion_id = None;
    let mut position = prm.position.clone();
    let mut internal = prm.internal && perms.reporter();
    let mut reply_resolved = false;
    if let Some(did) = &prm.reply_to {
        let thread = Note::in_discussion(c, did)?;
        let Some(first) = thread.first() else {
            return Err(note_error(&[(
                "base",
                vec!["Discussion to reply to cannot be found"],
            )]));
        };
        if first.system {
            return Err(note_error(&[(
                "base",
                vec!["Replies to system notes are not allowed"],
            )]));
        }
        if first.kind.is_none() {
            // Replying to an individual note turns it into a thread.
            c.execute(
                "UPDATE notes SET type = 'DiscussionNote', resolvable = 1 WHERE id = ?1",
                [first.id],
            )?;
        }
        kind = Some(if first.kind.as_deref() == Some("DiffNote") {
            "DiffNote"
        } else {
            "DiscussionNote"
        });
        position = first.position.as_deref().and_then(Position::parse);
        internal = first.internal;
        discussion_id = Some(did.clone());
        reply_resolved = discussion_resolved(&thread);
    }

    // Quick actions run before saving; the remaining text is the note.
    let qa = quick_actions::extract(&prm.body);
    let has_commands = !qa.commands.is_empty();
    let body = qa.text.clone();
    if has_commands && body.trim().is_empty() {
        let (changes, messages) =
            quick_actions::execute(c, config, p, mr, user, &qa.commands, &prm)?;
        if messages.is_empty() && changes.is_empty() {
            return Err(Error::BadRequest(Some("Failed to apply commands.".into())));
        }
        let summary = if messages.is_empty() {
            vec![]
        } else {
            vec![messages.join(" ")]
        };
        return Ok(Outcome::Commands(changes, summary));
    }

    // Commands whose blocks call services directly (rather than collecting `@updates`) act
    // while the note is interpreted, before it is saved: their notes come first.
    const INLINE: [&str; 4] = ["submit_review", "request_changes", "approve", "unapprove"];
    let (inline, later): (Vec<_>, Vec<_>) = qa.commands.iter().cloned().partition(|(n, _)| INLINE.contains(&n.as_str()));
    let todo_pending = crate::services::todos::any_pending(c, user.id, mr)?;
    let subscribed_before = crate::services::subscriptions::subscribed_to_mr(c, user.id, mr)?;
    let mut changes = Map::new();
    if !inline.is_empty() {
        changes = quick_actions::execute(c, config, p, mr, user, &inline, &prm)?.0;
    }
    if body.trim().is_empty() {
        return Err(note_error(&[("note", vec!["can't be blank"])]));
    }
    if body.len() > MAX_NOTE_BYTES {
        let msg = format!(
            "is too long ({}). The maximum size is 1 MiB.",
            human_size(body.len())
        );
        return Err(note_error(&[("note", vec![&msg])]));
    }
    let mut line_code = None;
    let mut stored_position = None;
    let mut original_position = None;
    let mut change_position = None;
    if let Some(pos) = &position {
        if prm.reply_to.is_none() {
            if !pos.complete() {
                return Err(note_error(&[("position", vec!["is incomplete"])]));
            }
            let r = repo::repo(c, p);
            let Some(diff) = mr.latest_diff(c)? else {
                return Err(note_error(&[(
                    "noteable",
                    vec!["doesn't support new-style diff notes"],
                )]));
            };
            if pos.on_text() {
                line_code = pos.line_code(&r)?;
                if line_code.is_none() {
                    return Err(note_error(&[(
                        "line_code",
                        vec!["can't be blank", "must be a valid line code"],
                    )]));
                }
            }
            original_position = Some(pos.store());
            let current = (
                diff.base_commit_sha.clone(),
                diff.start_commit_sha.clone(),
                diff.head_commit_sha.clone(),
            );
            if pos.diff_refs() != current {
                match pos.trace(
                    &r,
                    current.0.as_deref().unwrap_or(""),
                    current.1.as_deref().unwrap_or(""),
                    current.2.as_deref().unwrap_or(""),
                )? {
                    Some(t) => {
                        if t.on_text() {
                            line_code = t.line_code(&r)?.or(line_code);
                        }
                        stored_position = Some(t.store());
                    }
                    None => {
                        stored_position = Some(pos.store());
                        change_position = Some(pos.store());
                    }
                }
            } else {
                stored_position = Some(pos.store());
            }
        } else {
            let first = first_note(c, prm.reply_to.as_deref().unwrap())?.unwrap();
            stored_position = first.position.clone();
            original_position = first.original_position.clone();
            line_code = first.line_code.clone();
        }
    }
    let resolvable = kind.is_some();
    let created_at = if prm.created_at.is_some() && perms.owner() {
        prm.created_at
    } else {
        None
    };
    // Saving the note touches the MR (throttled); the commands below update the same record, so
    // their hook reports that touch as an `updated_at` change.
    let updated_before_touch = MergeRequest::get(c, mr.id)?.updated_at;
    let note = notes::insert(
        c,
        user,
        NewNote {
            note: body.clone(),
            noteable_type: "MergeRequest".into(),
            noteable_id: mr.id,
            project_id: Some(p.id),
            kind: kind.map(str::to_string),
            discussion_id,
            resolvable,
            position: stored_position,
            original_position,
            line_code,
            internal,
            review_id: prm.review_id,
            commit_id: if kind == Some("DiffNote") { prm.commit_id.clone() } else { None },
            created_at,
            ..Default::default()
        },
    )?;
    if let Some(cp) = change_position {
        c.execute(
            "UPDATE notes SET change_position = ?1 WHERE id = ?2",
            params![cp, note.id],
        )?;
    }
    if reply_resolved {
        c.execute(
            "UPDATE notes SET resolved_at = ?1, resolved_by_id = ?2 WHERE id = ?3",
            params![time::now(), user.id, note.id],
        )?;
    }
    // `Note#touch_noteable` (throttled).
    notes::touch_noteable(c, "MergeRequest", mr.id, time::now())?;
    if crate::services::reviews::PUBLISHING.with(|f| f.get()) {
        // While a review is published, `HandleAssigneesChangeWorker` from an earlier draft's
        // `/assign` finishes around the time the next note is saved (before its commands).
        crate::jobs::run_kind_now(c, config, "assignee_note")?;
    }
    let note = Note::find(c, note.id)?.unwrap();
    suggestions::create_for_note(c, p, mr, &note)?;
    // `handle_note`: a note in a thread resolves only that thread's to-dos.
    if note.kind.is_none() {
        todos::resolve_for_target(c, mr, Some(user.id))?;
    } else {
        todos::resolve_for_discussion(c, mr, user.id, &note.discussion_id)?;
    }
    crate::services::mentions::note_todos(c, p, mr, user, &note)?;
    // `Notes::PostProcessService`: `create_cross_references!` (the MR is the mentioner).
    if !note.system {
        crate::services::cross_refs::mr_mentions(c, config, p, user, mr, &note.note, None)?;
    }
    // The note hook carries the MR as it was when the note was saved, before its commands ran,
    // and is delivered after the hooks those commands fire.
    let note_hook = crate::services::hooks::prepare_note_event(c, config, p, mr, user, &note, "create")?;
    if has_commands && !later.is_empty() {
        let mr_now = MergeRequest::get(c, mr.id)?;
        let with_note = CreateParams { note_id: Some(note.id), updated_before_touch: Some(updated_before_touch), todo_pending: Some(todo_pending), subscribed_before: Some(subscribed_before), ..prm.clone() };
        let (ch, _) = quick_actions::execute(c, config, p, &mr_now, user, &later, &with_note)?;
        changes.extend(ch);
    }
    crate::services::hooks::fire_prepared(c, p, note_hook)?;
    // `NewNoteWorker` -> `Notes::PostProcessService` records the event asynchronously.
    crate::jobs::enqueue(c, "note_event", serde_json::json!({"note_id": note.id, "user_id": user.id}))?;
    Ok(Outcome::Created(Note::find(c, note.id)?.unwrap(), changes))
}

/// `Notes::UpdateService`.
pub fn update(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    note: &Note,
    body: &str,
) -> Result<Outcome> {
    let qa = quick_actions::extract(body);
    if !qa.commands.is_empty() && qa.text.trim().is_empty() {
        let (changes, messages) = quick_actions::execute(
            c,
            config,
            p,
            mr,
            user,
            &qa.commands,
            &CreateParams::default(),
        )?;
        delete(c, note)?;
        return Ok(Outcome::Commands(
            changes,
            if messages.is_empty() {
                vec![]
            } else {
                vec![messages.join(" ")]
            },
        ));
    }
    let text = qa.text;
    if text.trim().is_empty() {
        return Err(note_error(&[("note", vec!["can't be blank"])]));
    }
    if text.len() > MAX_NOTE_BYTES {
        let msg = format!(
            "is too long ({}). The maximum size is 1 MiB.",
            human_size(text.len())
        );
        return Err(note_error(&[("note", vec![&msg])]));
    }
    if text != note.note {
        let now = time::now();
        c.execute(
            "UPDATE notes SET note = ?1, updated_at = ?2, last_edited_at = ?2, updated_by_id = ?3 WHERE id = ?4",
            params![text, now, user.id, note.id],
        )?;
        notes::touch_noteable(c, "MergeRequest", mr.id, now)?;
        let updated = Note::find(c, note.id)?.unwrap();
        // `TodoService#update_note`: resolve as for a new note, and to-dos for new mentions only.
        if updated.kind.is_none() {
            todos::resolve_for_target(c, mr, Some(user.id))?;
        } else {
            todos::resolve_for_discussion(c, mr, user.id, &updated.discussion_id)?;
        }
        let before = crate::services::mentions::mentioned_user_ids(c, p, &note.note, user)?;
        crate::services::mentions::note_todos_skipping(c, p, mr, user, &updated, &before)?;
        crate::services::cross_refs::mr_mentions(c, config, p, user, mr, &updated.note, Some(&note.note))?;
        crate::services::hooks::note_event(c, config, p, mr, user, &updated, "update")?;
    }
    // `Notes::UpdateService` recreates suggestions on every edit, even an identical one.
    suggestions::recreate_for_note(c, p, mr, &Note::find(c, note.id)?.unwrap())?;
    let mut changes = Map::new();
    if !qa.commands.is_empty() {
        changes = quick_actions::execute(
            c,
            config,
            p,
            mr,
            user,
            &qa.commands,
            &CreateParams::default(),
        )?
        .0;
    }
    Ok(Outcome::Created(Note::find(c, note.id)?.unwrap(), changes))
}

/// `Notes::DestroyService`.
pub fn delete(c: &Connection, note: &Note) -> Result<()> {
    c.execute("DELETE FROM todos WHERE note_id = ?1", [note.id])?;
    c.execute(
        "DELETE FROM award_emoji WHERE awardable_type = 'Note' AND awardable_id = ?1",
        [note.id],
    )?;
    // No `after_destroy` touch: deleting a note leaves the MR's timestamps alone.
    c.execute("DELETE FROM notes WHERE id = ?1", [note.id])?;
    Ok(())
}

/// Resolves or unresolves a single note.
pub fn resolve_note(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    note: &Note,
    resolved: bool,
) -> Result<()> {
    let now = time::now();
    if resolved {
        if note.resolved_at.is_none() {
            c.execute(
                "UPDATE notes SET resolved_at = ?1, resolved_by_id = ?2, updated_at = ?1 WHERE id = ?3",
                params![now, user.id, note.id],
            )?;
            notes::touch_noteable(c, "MergeRequest", mr.id, now)?;
        }
        all_resolved_notification(c, config, p, mr, user)?;
    } else if note.resolved_at.is_some() {
        c.execute(
            "UPDATE notes SET resolved_at = NULL, resolved_by_id = NULL, resolved_by_push = 0, updated_at = ?1 WHERE id = ?2",
            params![now, note.id],
        )?;
        notes::touch_noteable(c, "MergeRequest", mr.id, now)?;
    }
    Ok(())
}

/// `Discussions::ResolveService` / `UnresolveService` for a whole thread (bulk update, no touch).
pub fn resolve_discussion(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    discussion_id: &str,
    resolved: bool,
) -> Result<()> {
    let now = time::now();
    if resolved {
        c.execute(
            "UPDATE notes SET resolved_at = ?1, resolved_by_id = ?2, updated_at = ?1 WHERE discussion_id = ?3 AND resolvable = 1 AND resolved_at IS NULL",
            params![now, user.id, discussion_id],
        )?;
        all_resolved_notification(c, config, p, mr, user)?;
    } else {
        c.execute(
            "UPDATE notes SET resolved_at = NULL, resolved_by_id = NULL, resolved_by_push = 0, updated_at = ?1 WHERE discussion_id = ?2 AND resolvable = 1",
            params![now, discussion_id],
        )?;
    }
    Ok(())
}

/// `ResolvedDiscussionNotificationService`: once every thread is resolved, a system note and an
/// MR `update` webhook.
pub fn all_resolved_notification(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<()> {
    let open: i64 = c.query_row(
        "SELECT count(*) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND resolvable = 1 AND resolved_at IS NULL",
        [mr.id],
        |r| r.get(0),
    )?;
    let any: i64 = c.query_row(
        "SELECT count(*) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND resolvable = 1",
        [mr.id],
        |r| r.get(0),
    )?;
    if open == 0 && any > 0 {
        notes::system_note(c, mr.id, p.id, user, "resolved all threads", "discussion")?;
        let mr2 = mr.reload(c)?;
        crate::services::hooks::merge_request_event(
            c,
            config,
            p,
            &mr2,
            Some(user),
            "update",
            None,
        )?;
        if mr2.merge_when_pipeline_succeeds {
            crate::jobs::enqueue(c, "mr_auto_merge", json!({"merge_request_id": mr.id}))?;
        }
    }
    Ok(())
}
