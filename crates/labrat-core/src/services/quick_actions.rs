//! Quick actions (`/close`, `/approve`, ...) in merge request notes and descriptions.

use crate::model::{Project, User};
use crate::time;
use crate::perms::ProjectPerms;
use crate::services::comments::CreateParams;
use crate::services::merge_requests::{self as mrs, MergeRequest, MrParams, merge, mergeability};
use crate::services::{labels, reviews, subscriptions, todos};
use crate::{Config, Result, repo};
use rusqlite::Connection;
use serde_json::{Map, Value, json};

pub struct Extracted {
    pub text: String,
    pub commands: Vec<(String, String)>,
}

/// Known MR command names and aliases (others are left in the text).
const COMMANDS: &[&str] = &[
    "close",
    "reopen",
    "title",
    "label",
    "labels",
    "unlabel",
    "remove_label",
    "relabel",
    "todo",
    "done",
    "subscribe",
    "unsubscribe",
    "react",
    "award",
    "shrug",
    "tableflip",
    "assign",
    "unassign",
    "milestone",
    "remove_milestone",
    "copy_metadata",
    "estimate",
    "estimate_time",
    "remove_estimate",
    "remove_time_estimate",
    "spend",
    "spent",
    "spend_time",
    "remove_time_spent",
    "lock",
    "unlock",
    "merge",
    "rebase",
    "draft",
    "ready",
    "target_branch",
    "submit_review",
    "request_changes",
    "approve",
    "unapprove",
    "request_review",
    "assign_reviewer",
    "reviewer",
    "unassign_reviewer",
    "remove_reviewer",
    "run_pipeline",
];

/// Commands GitLab defines for other targets (issues, incidents, work items, service desk):
/// the extractor still recognises them, so they leave the note and report "Could not apply".
const OTHER_COMMANDS: &[&str] = &[
    "add_child", "add_contacts", "add_email", "board_move", "clone", "confidential", "convert_to_ticket", "create_merge_request", "due",
    "duplicate", "move", "promote_to", "promote_to_incident", "relate", "remove_child", "remove_contacts", "remove_due_date",
    "remove_email", "remove_parent", "remove_zoom", "set_parent", "epic", "severity", "tag", "timeline", "type", "unlink", "zoom",
];

/// `Gitlab::QuickActions::Extractor`: command lines outside code blocks are removed.
pub fn extract(text: &str) -> Extracted {
    let text = text.replace('\r', "");
    let mut out: Vec<String> = vec![];
    let mut commands = vec![];
    let mut in_fence = false;
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        // Code fences and `>>>` multi-line blockquotes hold no commands.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") || line.starts_with(">>>") {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        if !in_fence && line.starts_with('/') {
            let rest = &line[1..];
            // `commands_regex`: the name ends the line or is followed by spaces (not tabs).
            let (name, args) = match rest.find(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
                None => (rest.to_lowercase(), String::new()),
                Some(i) if rest[i..].starts_with(' ') => (rest[..i].to_lowercase(), rest[i..].trim().to_string()),
                Some(_) => (String::new(), String::new()),
            };
            if COMMANDS.contains(&name.as_str()) || OTHER_COMMANDS.contains(&name.as_str()) {
                match name.as_str() {
                    // Substitutions replace the whole line (the text after them is dropped).
                    "shrug" => out.push("¯\\＿(ツ)＿/¯".to_string()),
                    "tableflip" => out.push("(╯°□°)╯︵ ┻━┻".to_string()),
                    _ => commands.push((name, args)),
                }
                continue;
            }
        }
        out.push(line.to_string());
    }
    Extracted {
        text: out.join("\n").trim_end().to_string(),
        commands,
    }
}

/// `extract_users`: space- or comma-separated `@user`, `user` or `me` (`and` is skipped). Any name
/// that matches no user fails the command with GitLab's message.
fn resolve_users(c: &Connection, me: &User, args: &str) -> Result<std::result::Result<Vec<User>, String>> {
    let mut found: Vec<User> = vec![];
    let mut missing: Vec<String> = vec![];
    for tok in args.split(|ch: char| ch.is_whitespace() || ch == ',').filter(|t| !t.is_empty() && *t != "and") {
        let u = if tok == "me" { Some(me.clone()) } else { User::find_by_username(c, tok.trim_start_matches('@'))? };
        match u {
            Some(u) => {
                if !found.iter().any(|x| x.id == u.id) {
                    found.push(u);
                }
            }
            None => missing.push(format!("'{tok}'")),
        }
    }
    if !missing.is_empty() {
        return Ok(Err(format!("Failed to find users for {}", missing.join(", "))));
    }
    Ok(Ok(found))
}


fn label_titles(args: &str) -> Vec<String> {
    let mut out = vec![];
    // Without `~`, each word is a label name.
    if !args.contains('~') {
        return args.split_whitespace().map(str::to_string).collect();
    }
    let mut rest = args.trim();
    while let Some(i) = rest.find('~') {
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('"') {
            let end = r.find('"').unwrap_or(r.len());
            out.push(r[..end].to_string());
            rest = &r[(end + 1).min(r.len())..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    out
}


fn parse_duration(s: &str) -> Option<i64> {
    crate::chronic::parse_time(s.trim(), false).map(|n| n.to_i64())
}


fn human(secs: i64) -> String {
    crate::chronic::human(secs)
}

/// Runs the commands; returns `commands_changes` and the execution messages.
pub fn execute(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    commands: &[(String, String)],
    ctx: &CreateParams,
) -> Result<(Map<String, Value>, Vec<String>)> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    let can_update = mrs::can_update(c, p, mr, user)?;
    let admin = perms.developer();
    let mut changes = Map::new();
    let mut messages: Vec<String> = vec![];
    let mut upd = MrParams::default();
    let mut deferred: Vec<(&str, String)> = vec![];
    let mut label_msg: Option<usize> = None;
    let mut unlabel_msg: Option<usize> = None;
    for (name, args) in commands {
        let mr_now = mr.reload(c)?;
        // Commands conditioned on `persisted?` don't exist for an MR being created.
        if ctx.new_record
            && matches!(
                name.as_str(),
                "approve" | "unapprove" | "todo" | "done" | "merge" | "rebase" | "relabel" | "unlabel" | "remove_label" | "remove_estimate"
                    | "remove_time_estimate" | "remove_time_spent" | "unassign" | "submit_review" | "request_changes" | "title" | "lock" | "unlock"
                    | "subscribe" | "unsubscribe" | "react" | "award" | "remove_milestone" | "run_pipeline"
            )
        {
            continue;
        }
        // `/draft` on a new MR is part of its title: no "marked as draft" note.
        if ctx.new_record && name == "draft" {
            if !mr_now.is_draft() {
                c.execute("UPDATE merge_requests SET title = ?1 WHERE id = ?2", rusqlite::params![mrs::draft_title(&mr_now.title), mr.id])?;
            }
            changes.insert("wip_event".into(), json!("draft"));
            continue;
        }
        match name.as_str() {
            "close" if mr_now.is_open() && can_update => {
                upd.state_event = Some("close".into());
                changes.insert("state_event".into(), json!("close"));
                messages.push("Closed this merge request.".into());
            }
            "reopen"
                if mr_now.state == "closed"
                    && can_update
                    && mr_now.discussion_locked != Some(true) =>
            {
                upd.state_event = Some("reopen".into());
                changes.insert("state_event".into(), json!("reopen"));
                messages.push("Reopened this merge request.".into());
            }
            "title" if can_update => {
                if !args.is_empty() {
                    upd.title = Some(args.clone());
                    changes.insert("title".into(), json!(args));
                    messages.push(format!("Changed the title to \"{args}\"."));
                }
            }
            // `run_label_command`: repeated commands add up (unique ids in command order), and one
            // message, at the first command's place, lists every label added, sorted.
            // Available when the project has any labels (`find_labels.any?`); a name that matches
            // none then does nothing, silently.
            "label" | "labels" if admin && !labels::available(c, p)?.is_empty() => {
                let found = labels::find_or_create_by_titles(c, p, &label_titles(args), false)?;
                if !found.is_empty() {
                    let mut ids: Vec<i64> = changes.get("add_label_ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                    let mut titles: Vec<String> = upd.add_labels.clone().unwrap_or_default();
                    for l in &found {
                        if !ids.contains(&l.id) {
                            ids.push(l.id);
                            titles.push(l.title.clone());
                        }
                    }
                    changes.insert("add_label_ids".into(), json!(ids));
                    upd.add_labels = Some(titles.clone());
                    let mut refs: Vec<String> = titles.iter().map(|t| label_ref(t)).collect();
                    refs.sort();
                    let msg = format!("Added {} label{}.", refs.join(" "), if refs.len() == 1 { "" } else { "s" });
                    match label_msg {
                        Some(i) => messages[i] = msg,
                        None => {
                            label_msg = Some(messages.len());
                            messages.push(msg);
                        }
                    }
                }
            }
            // Only available when the MR already has labels.
            "unlabel" | "remove_label" if admin && !labels::for_target(c, "MergeRequest", mr.id)?.is_empty() => {
                let titles = label_titles(args);
                if titles.is_empty() {
                    changes.insert("label_ids".into(), json!([]));
                    upd.labels = Some(vec![]);
                    messages.push("Removed all labels.".into());
                } else if labels::find_by_titles(c, p, &titles)?.is_empty() {
                    // Labels named but none found: the explanation's text, no change.
                    messages.push("Removed all labels.".into());
                } else {
                    let found = labels::find_by_titles(c, p, &titles)?;
                    // `run_label_command`: repeated commands add up, with one sorted message.
                    let mut ids: Vec<i64> = changes.get("remove_label_ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                    let mut names: Vec<String> = upd.remove_labels.clone().unwrap_or_default();
                    for l in &found {
                        if !ids.contains(&l.id) {
                            ids.push(l.id);
                            names.push(l.title.clone());
                        }
                    }
                    changes.insert("remove_label_ids".into(), json!(ids));
                    upd.remove_labels = Some(names.clone());
                    let mut refs: Vec<String> = names.iter().map(|t| label_ref(t)).collect();
                    refs.sort();
                    let msg = format!("Removed {} label{}.", refs.join(" "), if refs.len() == 1 { "" } else { "s" });
                    match unlabel_msg {
                        Some(i) => messages[i] = msg,
                        None => {
                            unlabel_msg = Some(messages.len());
                            messages.push(msg);
                        }
                    }
                }
            }
            "relabel" if admin && !labels::for_target(c, "MergeRequest", mr.id)?.is_empty() && labels::find_by_titles(c, p, &label_titles(args))?.is_empty() => {}
            "relabel" if admin && !labels::for_target(c, "MergeRequest", mr.id)?.is_empty() => {
                let found = labels::find_or_create_by_titles(c, p, &label_titles(args), false)?;
                let ids: Vec<i64> = found.iter().map(|l| l.id).collect();
                changes.insert("label_ids".into(), json!(ids));
                upd.labels = Some(found.iter().map(|l| l.title.clone()).collect());
                let refs: Vec<String> = found.iter().map(|l| label_ref(&l.title)).collect();
                messages.push(format!(
                    "Replaced all labels with {} label{}.",
                    refs.join(" "),
                    if refs.len() == 1 { "" } else { "s" }
                ));
            }
            // `/todo` is offered only without a pending to-do, `/done` only with one.
            "todo" if !ctx.todo_pending.map_or_else(|| todos::any_pending(c, user.id, &mr_now), Ok)? => {
                changes.insert("todo_event".into(), json!("add"));
                deferred.push(("todo", String::new()));
                messages.push("Added a to-do item.".into());
            }
            "done" if ctx.todo_pending.map_or_else(|| todos::any_pending(c, user.id, &mr_now), Ok)? => {
                changes.insert("todo_event".into(), json!("done"));
                deferred.push(("done", String::new()));
                messages.push("Marked to-do item as done.".into());
            }
            // Offered only when the user isn't subscribed (explicitly or as a participant).
            "subscribe" if !ctx.subscribed_before.map_or_else(|| crate::services::subscriptions::subscribed_to_mr(c, user.id, mr), Ok)? => {
                changes.insert("subscription_event".into(), json!("subscribe"));
                deferred.push(("subscribe", String::new()));
                messages.push("Subscribed to notifications.".into());
            }
            "unsubscribe" if ctx.subscribed_before.map_or_else(|| crate::services::subscriptions::subscribed_to_mr(c, user.id, mr), Ok)? => {
                changes.insert("subscription_event".into(), json!("unsubscribe"));
                deferred.push(("unsubscribe", String::new()));
                messages.push("Unsubscribed from notifications.".into());
            }
            "react" | "award" => {
                // `parse_params`: the first `:name:` token; anything else gives nothing.
                let first = args.split_whitespace().next().unwrap_or("");
                let emoji = first.strip_prefix(':').and_then(|x| x.strip_suffix(':')).unwrap_or("").to_string();
                if !emoji.is_empty() && crate::services::emoji::canonical(&emoji).is_some() {
                    changes.insert("emoji_award".into(), json!(emoji));
                    // `@updates[:emoji_award]` holds one emoji: only the last command's is toggled.
                    deferred.retain(|(w, _)| *w != "react");
                    deferred.push(("react", emoji.clone()));
                    messages.push(format!("Toggled :{emoji}: emoji reaction."));
                }
            }
            "assign" if admin => match resolve_users(c, user, args)? {
                Err(e) => messages.push(format!("Could not apply assign command. {e}.")),
                Ok(users) => {
                    // One assignee: the first named.
                    if let Some(u) = users.first() {
                        upd.assignee_ids = Some(vec![u.id]);
                        changes.insert("assignee_ids".into(), json!([u.id]));
                        messages.push(format!("Assigned @{}.", u.username));
                    }
                }
            },
            "unassign" if admin && !mr_now.assignee_ids(c)?.is_empty() => {
                let current = mr_now.assignee_ids(c)?;
                if !current.is_empty() {
                    let names: Vec<String> = current
                        .iter()
                        .filter_map(|id| User::find(c, *id).ok().flatten())
                        .map(|u| format!("@{}", u.username))
                        .collect();
                    upd.assignee_ids = Some(vec![0]);
                    changes.insert("assignee_ids".into(), json!([]));
                    messages.push(format!("Removed assignee {}.", names.join(" and ")));
                }
            }
            // Offered only when the project or its groups have an active milestone.
            "milestone" if admin && any_active_milestone(c, p)? => {
                let title = args.trim().trim_start_matches('%').trim_matches('"');
                let groups = crate::model::Namespace::self_and_ancestor_ids(c, p.namespace_id)?.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
                let id: Option<i64> = c
                    .query_row(
                        &format!("SELECT id FROM milestones WHERE (project_id = ?1 OR group_id IN ({groups})) AND title = ?2 AND state = 'active' ORDER BY id LIMIT 1"),
                        rusqlite::params![p.id, title],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(id) = id {
                    upd.milestone_id = Some(Some(id));
                    changes.insert("milestone_id".into(), json!(id));
                    messages.push(format!("Set the milestone to %\"{title}\"."));
                }
            }
            // `/copy_metadata !N`: labels and milestone of another MR of the project.
            "copy_metadata" if admin => {
                let iid = args.trim().trim_start_matches('!').parse::<i64>().ok();
                // `can_copy_metadata?` only needs the same namespace: the MR itself qualifies.
                if let Some(src) = iid.and_then(|n| MergeRequest::find_by_iid(c, p.id, n).ok().flatten()) {
                    let found = labels::for_target(c, "MergeRequest", src.id)?;
                    let ids: Vec<i64> = found.iter().map(|l| l.id).collect();
                    changes.insert("add_label_ids".into(), json!(ids));
                    if !found.is_empty() {
                        upd.add_labels = Some(found.iter().map(|l| l.title.clone()).collect());
                    }
                    if let Some(m) = src.milestone_id {
                        upd.milestone_id = Some(Some(m));
                        changes.insert("milestone_id".into(), json!(m));
                    }
                    messages.push(format!("Copied labels and milestone from !{}.", src.iid));
                }
            }
            "remove_milestone" if admin && mr_now.milestone_id.is_some() => {
                upd.milestone_id = Some(None);
                changes.insert("milestone_id".into(), Value::Null);
                let title: String = c.query_row("SELECT title FROM milestones WHERE id = ?1", [mr_now.milestone_id.unwrap_or(0)], |r| r.get(0)).unwrap_or_default();
                messages.push(format!("Removed %\"{title}\" milestone."));
            }
            "estimate" | "estimate_time" if admin => {
                if let Some(n) = crate::chronic::parse_time(args.trim(), true) {
                    let s = n.to_i64();
                    changes.insert("time_estimate".into(), n.to_json());
                    upd.time_estimate = Some(s);
                    // The message shows the parsed value; the column keeps at most 2^31 - 1.
                    let text = match n {
                        crate::chronic::Num::Int(i) => crate::chronic::output(i.abs()).unwrap_or_default(),
                        crate::chronic::Num::Float(f) => crate::chronic::output((f.abs()) as i128).unwrap_or_default(),
                    };
                    messages.push(if n.is_zero() { "Removed time estimate.".to_string() } else { format!("Set time estimate to {text}.") });
                }
            }
            "remove_estimate" | "remove_time_estimate" if admin => {
                changes.insert("time_estimate".into(), json!(0));
                upd.time_estimate = Some(0);
                messages.push("Removed time estimate.".into());
            }
            "spend" | "spent" | "spend_time" if admin => {
                // `<time> [<date>]`: the duration, then an optional `YYYY-MM-DD` for `spent_at`.
                let words: Vec<&str> = args.split_whitespace().collect();
                let date = words.last().filter(|w| w.len() == 10 && time::parse_date(w).is_some()).copied();
                let dur = words.iter().filter(|w| Some(**w) != date).copied().collect::<Vec<_>>().join(" ");
                let spent_at = date.and_then(time::parse_date).map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis()).unwrap_or_else(time::now);
                if let Some(s) = parse_duration(&dur) {
                    changes.insert(
                        "spend_time".into(),
                        json!({"duration": s, "user_id": user.id, "spent_at": time::iso(spent_at), "category": null, "note_id": ctx.note_id}),
                    );
                    upd.spend = Some((s, ctx.note_id));
                    messages.push(if s >= 0 {
                        format!("Added {} spent time.", human(s))
                    } else {
                        format!("Subtracted {} spent time.", human(s))
                    });
                }
            }
            "remove_time_spent" if admin => {
                changes.insert(
                    "spend_time".into(),
                    json!({"duration": "reset", "user_id": user.id}),
                );
                deferred.push(("reset_spent", String::new()));
                messages.push("Removed spent time.".into());
            }
            "lock" if admin && mr_now.discussion_locked != Some(true) => {
                upd.discussion_locked = Some(true);
                changes.insert("discussion_locked".into(), json!(true));
                messages.push("Locked the discussion.".into());
            }
            "unlock" if admin && mr_now.discussion_locked == Some(true) => {
                upd.discussion_locked = Some(false);
                changes.insert("discussion_locked".into(), json!(false));
                messages.push("Unlocked the discussion.".into());
            }
            "draft" if can_update => {
                changes.insert("wip_event".into(), json!("draft"));
                // Commands see the MR as it was before the note, and the last `wip_event` wins:
                // `/draft` after `/ready` keeps a draft a draft.
                upd.title = Some(mrs::draft_title(&mr_now.title));
                if mr_now.is_draft() {
                    messages.push("No change to this merge request's draft status.".into());
                } else {
                    messages.push("Marked this merge request as a draft.".into());
                }
            }
            // Unlike `/draft`, `/ready` only sets `wip_event` on a draft.
            "ready" if can_update => {
                if mr_now.is_draft() {
                    changes.insert("wip_event".into(), json!("ready"));
                    upd.title = Some(mrs::draftless_title(&mr_now.title));
                    messages.push("Marked this merge request as ready.".into());
                } else {
                    messages.push("No change to this merge request's draft status.".into());
                }
            }
            "target_branch" if can_update => {
                let b = args.trim();
                if repo::repo(c, p).branch(b)?.is_some() {
                    upd.target_branch = Some(b.to_string());
                    changes.insert("target_branch".into(), json!(b));
                }
                // The message is shown whether or not the branch exists.
                if !b.is_empty() {
                    messages.push(format!("Set target branch to {b}."));
                }
            }
            "merge" if mrs::can_be_merged_by(c, p, &mr_now, user)? => {
                let head = mr_now.diff_head_sha(c, p)?;
                match &ctx.merge_request_diff_head_sha {
                    None => messages.push(
                        "The `/merge` quick action requires the SHA of the head of the branch."
                            .into(),
                    ),
                    Some(sha) if Some(sha) != head.as_ref() => messages
                        .push("Branch has been updated since the merge was requested.".into()),
                    Some(sha) => {
                        changes.insert("merge".into(), json!(sha));
                        deferred.push(("merge", sha.clone()));
                        messages.push("Merged this merge request.".into());
                    }
                }
            }
            "rebase"
                if mr_now.is_open()
                    && repo::repo(c, p).branch(&mr_now.source_branch)?.is_some()
                    && repo::can_push_to_branch(c, user, p, &mr_now.source_branch)? =>
            {
                if mr_now.rebase_in_progress {
                    messages.push("A rebase is already in progress.".into());
                } else {
                    changes.insert("rebase".into(), json!(true));
                    deferred.push(("rebase", String::new()));
                    messages.push(format!(
                        "Scheduled a rebase of branch {}.",
                        mr_now.source_branch
                    ));
                }
            }
            // `/approve` repeated in a note runs once (its message is shown once).
            "approve" if deferred.iter().any(|(w, _)| *w == "approve") => {}
            // Messages are computed after every command ran: an `/approve` that wasn't
            // available, followed by an `/unapprove` that ran, is available again by then and
            // reports an empty message.
            "approve"
                if reviews::has_approved(c, &mr_now, user)?
                    && mr_now.state != "merged"
                    && commands.iter().skip_while(|(n, _)| n != "approve").any(|(n, _)| n == "unapprove") =>
            {
                messages.push(String::new());
            }
            "approve"
                if !reviews::has_approved(c, &mr_now, user)?
                    && mr_now.state != "merged"
                    && reviews::can_approve(c, p, &mr_now, user)? =>
            {
                if !ctx.publishing && reviews::has_drafts(c, &mr_now, user)? {
                    reviews::publish_drafts(c, config, p, &mr_now, user)?;
                    messages.push("Submitted the current review.".into());
                }
                deferred.push(("approve", String::new()));
                messages.push("Approved the current merge request.".into());
            }
            "unapprove" if reviews::has_approved(c, &mr_now, user)? && mr_now.state != "merged" => {
                deferred.push(("unapprove", String::new()));
                messages.push("Unapproved the current merge request.".into());
            }
            // `/run_pipeline`: queues a pipeline (none appears without CI configuration).
            "run_pipeline" if perms.developer() && mr_now.is_open() => {
                messages.push("New pipeline has been triggered and will appear shortly.".into());
            }
            "submit_review" => {
                if !ctx.publishing {
                    reviews::publish_drafts(c, config, p, &mr_now, user)?;
                }
                messages.push("Submitted the current review.".into());
                let state = if args.trim().is_empty() {
                    "reviewed".to_string()
                } else {
                    args.trim().to_string()
                };
                if state == "approve" && !reviews::has_approved(c, &mr_now, user)? && reviews::can_approve(c, p, &mr_now, user)? {
                    messages.push("Approved the current merge request.".into());
                }
                deferred.push(("submit_review", state));
            }
            "request_changes" if mr_now.reviewer_ids(c)?.contains(&user.id) => {
                deferred.push(("request_changes", String::new()));
                messages.push("Changes requested to the current merge request.".into());
            }
            "request_review" | "assign_reviewer" | "reviewer" if admin => {
                let resolved = resolve_users(c, user, args)?;
                if let Err(e) = &resolved {
                    messages.push(format!("Could not apply request_review command. {e}."));
                }
                if let Some(u) = resolved.ok().and_then(|v| v.into_iter().next()) {
                    // `@updates[:reviewer_ids] |= users`: the existing reviewers stay first, so in
                    // With one reviewer allowed, an extra reviewer is dropped by the update.
                    let current = mr_now.reviewer_ids(c)?;
                    // Earlier `/assign_reviewer` commands of the same note add up.
                    let mut union = upd.reviewer_ids.clone().unwrap_or_else(|| current.clone());
                    if !union.contains(&u.id) {
                        union.push(u.id);
                    }
                    // `RequestReviewService` runs inside the command block (its own hook), and
                    // `@updates[:reviewer_ids]` is always set, so an update follows either way.
                    if current.contains(&u.id) {
                        reviews::rerequest(c, config, p, &mr_now, user, &u)?;
                    }
                    upd.reviewer_ids = Some(union.clone());
                    changes.insert("reviewer_ids".into(), json!(union));
                    messages.push(format!("Requested a review from @{}.", u.username));
                }
            }
            "unassign_reviewer" | "remove_reviewer" if admin => {
                let current = mr_now.reviewer_ids(c)?;
                // Only one reviewer is allowed, so `parse_params` ignores the users given
                // (`extract_users ... if allows_multiple_reviewers?`): every reviewer is removed.
                let names: Vec<String> = vec![];
                let _ = args;
                let remove: Vec<i64> = if names.is_empty() {
                    current.clone()
                } else {
                    names
                        .iter()
                        .filter_map(|n| User::find_by_username(c, n).ok().flatten())
                        .map(|u| u.id)
                        .filter(|id| current.contains(id))
                        .collect()
                };
                if !remove.is_empty() {
                    let keep: Vec<i64> = current
                        .iter()
                        .filter(|id| !remove.contains(id))
                        .copied()
                        .collect();
                    let refs: Vec<String> = remove
                        .iter()
                        .filter_map(|id| User::find(c, *id).ok().flatten())
                        .map(|u| format!("@{}", u.username))
                        .collect();
                    upd.reviewer_ids = Some(if keep.is_empty() {
                        vec![0]
                    } else {
                        keep.clone()
                    });
                    changes.insert("reviewer_ids".into(), json!(keep));
                    messages.push(format!("Removed reviewer {}.", refs.join(" and ")));
                } else {
                    // `reviewers_to_remove?` fails the command's condition.
                    messages.push("Could not apply unassign_reviewer command.".into());
                }
            }
            // `CommandDefinition#execute_message`: a recognized command that isn't available
            // still leaves the note, with its definition name.
            _ => messages.push(format!("Could not apply {} command.", canonical_command(&name))),
        }
    }
    // Apply MR attribute changes in one update, then the side commands.
    let has_update = upd.title.is_some()
        || upd.labels.is_some()
        || upd.add_labels.is_some()
        || upd.remove_labels.is_some()
        || upd.assignee_ids.is_some()
        || upd.reviewer_ids.is_some()
        || upd.milestone_id.is_some()
        || upd.discussion_locked.is_some()
        || upd.target_branch.is_some()
        || upd.state_event.is_some()
        || upd.time_estimate.is_some()
        || upd.spend.is_some();
    let only_state = upd.state_event.is_some() && {
        let mut rest = upd.clone();
        rest.state_event = None;
        !(rest.title.is_some()
            || rest.labels.is_some()
            || rest.add_labels.is_some()
            || rest.remove_labels.is_some()
            || rest.assignee_ids.is_some()
            || rest.reviewer_ids.is_some()
            || rest.milestone_id.is_some()
            || rest.discussion_locked.is_some()
            || rest.target_branch.is_some()
            || rest.time_estimate.is_some()
            || rest.spend.is_some())
    };
    if ctx.return_update {
        PENDING_UPDATE.with(|u| *u.borrow_mut() = Some(upd));
    } else if only_state {
        // `IssuableBaseService#update` consumes `state_event` first; with no other parameter
        // left, nothing is saved: no `updated_by` and no `update` hook, only the close/reopen.
        mrs::apply_state_event(c, config, p, &mr.reload(c)?, user, upd.state_event.as_deref())?;
    } else if has_update {
        let mut now = mr.reload(c)?;
        if let Some(t) = ctx.updated_before_touch {
            now.updated_at = t;
        }
        mrs::update(c, config, p, &now, user, upd)?;
    }
    for (what, arg) in deferred {
        let mr_now = mr.reload(c)?;
        match what {
            "todo" => {
                todos::create(c, user.id, user, p, &mr_now, todos::action::MARKED, None)?;
            }
            // `change_todo`: `TodosFinder#find_by(target:)` (unordered, so the oldest pending,
            // unsnoozed to-do) is the only one resolved.
            "done" => {
                let oldest: Option<i64> = c
                    .query_row(
                        "SELECT id FROM todos WHERE user_id = ?1 AND target_type = 'MergeRequest' AND target_id = ?2 AND state = 'pending'
                           AND (snoozed_until IS NULL OR snoozed_until <= ?3) ORDER BY id LIMIT 1",
                        rusqlite::params![user.id, mr_now.id, time::now()],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(id) = oldest {
                    c.execute("UPDATE todos SET state = 'done', snoozed_until = NULL, updated_at = ?1 WHERE id = ?2", rusqlite::params![time::now(), id])?;
                }
            }
            "subscribe" => subscriptions::set(c, user.id, "MergeRequest", mr.id, true)?,
            "unsubscribe" => subscriptions::set(c, user.id, "MergeRequest", mr.id, false)?,
            "react" => {
                if let Some((a, added)) = crate::services::emoji::toggle(c, user, "MergeRequest", mr.id, &arg)? {
                    crate::services::hooks::emoji_event(c, config, p, &mr_now, None, user, &a, if added { "award" } else { "revoke" })?;
                }
            }
            "reset_spent" => {
                c.execute(
                    "UPDATE merge_requests SET total_time_spent = 0 WHERE id = ?1",
                    [mr.id],
                )?;
            }
            "merge" => {
                let mp = merge::MergeParams {
                    sha: Some(arg),
                    ..Default::default()
                };
                if mergeability::mergeable(c, config, p, &mr_now)? {
                    merge::merge(c, config, p, &mr_now, user, &mp)?;
                } else {
                    merge::enable_auto_merge(c, p, &mr_now, user, &mp)?;
                }
            }
            "rebase" => merge::request_rebase(c, &mr_now, user, false)?,
            "approve" => {
                reviews::approve(c, config, p, &mr_now, user)?;
            }
            "unapprove" => {
                reviews::unapprove(c, config, p, &mr_now, user)?;
            }
            "submit_review" => {
                if arg == "approve" {
                    reviews::approve(c, config, p, &mr_now, user)?;
                } else {
                    reviews::update_reviewer_state(c, config, p, &mr_now, user, &arg)?;
                }
            }
            "request_changes" => {
                reviews::update_reviewer_state(c, config, p, &mr_now, user, "requested_changes")?;
            }
            _ => {}
        }
    }
    // `commands_changes` keeps GitLab's key order (`Note#commands_changes` whitelist).
    const ORDER: &[&str] = &[
        "due_date",
        "label_ids",
        "remove_label_ids",
        "add_label_ids",
        "canonical_issue_id",
        "clone_with_notes",
        "confidential",
        "create_merge_request",
        "add_contacts",
        "remove_contacts",
        "assignee_ids",
        "milestone_id",
        "time_estimate",
        "spend_time",
        "discussion_locked",
        "merge",
        "rebase",
        "wip_event",
        "target_branch",
        "reviewer_ids",
        "health_status",
        "promote_to_epic",
        "weight",
        "emoji_award",
        "todo_event",
        "subscription_event",
        "state_event",
        "title",
        "tag_message",
        "tag_name",
    ];
    let mut ordered = Map::new();
    for k in ORDER {
        if let Some(v) = changes.get(*k) {
            ordered.insert(k.to_string(), v.clone());
        }
    }
    Ok((ordered, messages))
}

/// `Label#to_reference(format: :name)`: names are always quoted.
fn label_ref(title: &str) -> String {
    format!("~\"{title}\"")
}

/// The definition name of a quick action typed with one of its aliases.
fn canonical_command(name: &str) -> &str {
    match name {
        "labels" => "label",
        "remove_label" => "unlabel",
        "award" => "react",
        "estimate_time" => "estimate",
        "remove_time_estimate" => "remove_estimate",
        "spent" | "spend_time" => "spend",
        "remove_reviewer" => "unassign_reviewer",
        "reviewer" | "assign_reviewer" => "request_review",
        "epic" => "set_parent",
        other => other,
    }
}

/// Whether the project or one of its groups has an active milestone (`/milestone`'s condition).
fn any_active_milestone(c: &Connection, p: &Project) -> Result<bool> {
    let groups = crate::model::Namespace::self_and_ancestor_ids(c, p.namespace_id)?.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let n: i64 = c.query_row(
        &format!("SELECT count(*) FROM milestones WHERE state = 'active' AND (project_id = ?1 OR group_id IN ({groups}))"),
        [p.id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}


thread_local! {
    static PENDING_UPDATE: std::cell::RefCell<Option<MrParams>> = const { std::cell::RefCell::new(None) };
}

/// The attribute changes collected by the last `execute` run with `return_update`.
pub fn take_update() -> MrParams {
    PENDING_UPDATE.with(|u| u.borrow_mut().take()).unwrap_or_default()
}
