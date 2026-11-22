//! Webhooks: hook records, matching hooks for an event, payload builders, and queueing
//! deliveries as `web_hook` jobs (delivered by the HTTP layer).

use crate::db::OptionalExt;
use crate::model::{Namespace, Project, User};
use crate::services::emoji::Award;
use crate::services::labels::Label;
use crate::services::merge_requests::{MergeRequest, Snapshot, mergeability};
use crate::services::notes::Note;
use crate::services::positions::Position;
use crate::{Config, Result, jobs, repo, time};
use labrat_git::Oid;
use rusqlite::{Connection, Row, params};
use serde_json::{Map, Value, json};

/// Event flags a project hook can have, in the API's order.
pub const PROJECT_EVENTS: &[&str] = &[
    "push_events", "tag_push_events", "merge_requests_events", "repository_update_events", "issues_events",
    "confidential_issues_events", "note_events", "confidential_note_events", "pipeline_events", "wiki_page_events",
    "deployment_events", "feature_flag_events", "job_events", "releases_events", "milestone_events", "emoji_events",
    "resource_access_token_events", "resource_deploy_token_events", "vulnerability_events",
];

#[derive(Debug, Clone)]
pub struct Hook {
    pub id: i64,
    pub project_id: Option<i64>,
    pub kind: String,
    pub url: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub token: Option<String>,
    pub events: Map<String, Value>,
    pub push_events_branch_filter: Option<String>,
    pub branch_filter_strategy: String,
    pub enable_ssl_verification: bool,
    pub custom_headers: Vec<(String, String)>,
    pub url_variables: Vec<(String, String)>,
    pub custom_webhook_template: Option<String>,
    pub created_at: i64,
}

const COLS: &str = "id, project_id, type, url, name, description, token, events, push_events_branch_filter, branch_filter_strategy, enable_ssl_verification, custom_headers, custom_webhook_template, created_at";

fn hook_row(r: &Row) -> rusqlite::Result<Hook> {
    let events: String = r.get(7)?;
    let headers: String = r.get(11)?;
    let parsed: Value = serde_json::from_str(&headers).unwrap_or(json!({}));
    let pairs = |k: &str| -> Vec<(String, String)> {
        parsed[k]
            .as_array()
            .map(|a| a.iter().filter_map(|x| Some((x["key"].as_str()?.to_string(), x["value"].as_str()?.to_string()))).collect())
            .unwrap_or_default()
    };
    Ok(Hook {
        id: r.get(0)?,
        project_id: r.get(1)?,
        kind: r.get(2)?,
        url: r.get(3)?,
        name: r.get(4)?,
        description: r.get(5)?,
        token: r.get(6)?,
        events: serde_json::from_str(&events).unwrap_or_default(),
        push_events_branch_filter: r.get(8)?,
        branch_filter_strategy: r.get(9)?,
        enable_ssl_verification: r.get(10)?,
        custom_headers: pairs("headers"),
        url_variables: pairs("variables"),
        custom_webhook_template: r.get(12)?,
        created_at: r.get(13)?,
    })
}

impl Hook {
    pub fn find(c: &Connection, id: i64) -> Result<Option<Hook>> {
        c.query_row(&format!("SELECT {COLS} FROM web_hooks WHERE id = ?1"), [id], hook_row).optional()
    }

    pub fn list(c: &Connection, kind: &str, project_id: Option<i64>) -> Result<Vec<Hook>> {
        let mut st = c.prepare(&format!(
            "SELECT {COLS} FROM web_hooks WHERE type = ?1 AND (?2 IS NULL OR project_id = ?2) ORDER BY id"
        ))?;
        let v = st.query_map(params![kind, project_id], hook_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Event flag with GitLab's column defaults.
    pub fn event(&self, name: &str) -> Value {
        match self.events.get(name) {
            Some(v) => v.clone(),
            None => match (name, self.kind.as_str()) {
                ("push_events", "SystemHook") => json!(false),
                ("push_events", _) => json!(true),
                ("repository_update_events", "SystemHook") => json!(true),
                ("confidential_note_events", _) => Value::Null,
                _ => json!(false),
            },
        }
    }

    pub fn enabled(&self, name: &str) -> bool {
        self.event(name).as_bool().unwrap_or(false)
    }

    /// `url` with `{name}` variables substituted.
    pub fn interpolated_url(&self) -> String {
        let mut u = self.url.clone();
        for (k, v) in &self.url_variables {
            u = u.replace(&format!("{{{k}}}"), v);
        }
        u
    }
}

/// `ActiveHookFilter` for push hooks.
fn branch_matches(h: &Hook, ref_name: &str) -> bool {
    let branch = ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name);
    let filter = h.push_events_branch_filter.clone().unwrap_or_default();
    if filter.is_empty() {
        return true;
    }
    match h.branch_filter_strategy.as_str() {
        "all_branches" => true,
        "regex" => regex_lite_match(&filter, branch),
        _ => repo::ref_matches(&filter, branch),
    }
}

/// `Gitlab::UntrustedRegexp` (RE2): an unanchored match; invalid patterns match nothing.
fn regex_lite_match(pattern: &str, s: &str) -> bool {
    regex::Regex::new(pattern).is_ok_and(|r| r.is_match(s))
}

/// Queues deliveries of `data` to the project's hooks with `event` enabled (and to system hooks
/// for push, tag push and merge request events).
fn fire(c: &Connection, p: &Project, event: &str, scope: &str, data: &Value, ref_name: Option<&str>) -> Result<()> {
    for h in Hook::list(c, "ProjectHook", Some(p.id))? {
        if !h.enabled(event) {
            continue;
        }
        if scope == "push_hooks" && !ref_name.is_some_and(|r| branch_matches(&h, r)) {
            continue;
        }
        enqueue(c, &h, scope, data)?;
    }
    if matches!(scope, "push_hooks" | "tag_push_hooks" | "merge_request_hooks") {
        for h in Hook::list(c, "SystemHook", None)? {
            if h.enabled(event) {
                enqueue(c, &h, "system_hooks", data)?;
            }
        }
    }
    Ok(())
}

pub fn enqueue(c: &Connection, h: &Hook, scope: &str, data: &Value) -> Result<()> {
    let key = uuid();
    jobs::enqueue(c, "web_hook", json!({"hook_id": h.id, "hook_name": scope, "data": data, "idempotency_key": key}))
}

pub fn uuid() -> String {
    let b: [u8; 16] = rand::random();
    let h = hex::encode(b);
    format!("{}-{}-4{}-{:x}{}-{}", &h[0..8], &h[8..12], &h[13..16], (b[8] & 0x3) | 0x8, &h[17..20], &h[20..32])
}

pub(crate) fn ar_time(ms: i64) -> Value {
    json!(time::ruby_utc(ms))
}

fn opt_ar_time(ms: Option<i64>) -> Value {
    ms.map(ar_time).unwrap_or(Value::Null)
}

/// `User#hook_attrs`.
pub fn user_attrs(u: &User) -> Value {
    let hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(u.email.trim().to_lowercase().as_bytes()))
    };
    json!({
        "id": u.id,
        "name": u.name,
        "username": u.username,
        "avatar_url": format!("https://www.gravatar.com/avatar/{hash}?s=80&d=identicon"),
        "email": u.public_email.clone().filter(|e| !e.is_empty()).unwrap_or("[REDACTED]".into()),
    })
}

/// `Project#hook_attrs`.
pub fn project_attrs(c: &Connection, config: &Config, p: &Project) -> Result<Value> {
    let ns = Namespace::find(c, p.namespace_id)?.expect("namespace exists");
    let web = config.url(&format!("/{}", p.full_path));
    let ssh = config.ssh_url(&p.full_path);
    let http = config.url(&format!("/{}.git", p.full_path));
    let ci: Option<String> = p.setting("ci_config_path").and_then(Value::as_str).map(str::to_string);
    Ok(json!({
        "id": p.id,
        "name": p.name,
        "description": p.description,
        "web_url": web,
        "avatar_url": null,
        "git_ssh_url": ssh,
        "git_http_url": http,
        "namespace": ns.name,
        "visibility_level": p.visibility_level,
        "path_with_namespace": p.full_path,
        "default_branch": p.default_branch,
        "ci_config_path": ci,
        "homepage": web,
        "url": ssh,
        "ssh_url": ssh,
        "http_url": http,
    }))
}

fn repository_attrs(c: &Connection, config: &Config, p: &Project, push: bool) -> Result<Value> {
    let pa = project_attrs(c, config, p)?;
    let mut m = Map::new();
    for k in ["name", "url", "description", "homepage"] {
        m.insert(k.into(), pa[k].clone());
    }
    if push {
        for k in ["git_http_url", "git_ssh_url", "visibility_level"] {
            m.insert(k.into(), pa[k].clone());
        }
    }
    Ok(Value::Object(m))
}

pub fn label_attrs(l: &Label) -> Value {
    json!({
        "id": l.id,
        "title": l.title,
        "color": l.color,
        "project_id": l.project_id,
        "created_at": ar_time(l.created_at),
        "updated_at": ar_time(l.updated_at),
        "template": false,
        "description": l.description,
        "type": if l.group_id.is_some() { "GroupLabel" } else { "ProjectLabel" },
        "group_id": l.group_id,
    })
}

/// `Commit#hook_attrs`.
pub fn commit_attrs(config: &Config, p: &Project, r: &labrat_git::Repo, oid: &Oid, with_files: bool) -> Result<Value> {
    let c = r.commit(oid)?;
    let full_title = c.message.split(['\r', '\n']).next().unwrap_or("").to_string();
    let title = if full_title.chars().count() >= 100 {
        format!("{}...", full_title.chars().take(80).collect::<String>())
    } else {
        full_title
    };
    let mut m = Map::new();
    m.insert("id".into(), json!(oid.hex()));
    m.insert("message".into(), json!(c.message));
    m.insert("title".into(), json!(title));
    m.insert("timestamp".into(), json!(time::iso_git(c.committer.time, c.committer.offset).replace(".000", "")));
    m.insert("url".into(), json!(config.url(&format!("/{}/-/commit/{}", p.full_path, oid.hex()))));
    m.insert("author".into(), json!({"name": c.author.name, "email": c.author.email}));
    if with_files {
        let parent_tree = match c.parents.first() {
            Some(pp) => Some(r.commit(pp)?.tree),
            None => None,
        };
        let (mut added, mut modified, mut removed) = (vec![], vec![], vec![]);
        for ch in labrat_git::diff::diff_trees(r, parent_tree.as_ref(), Some(&c.tree))? {
            if ch.deleted_file {
                removed.push(ch.old_path);
            } else if ch.new_file || ch.renamed_file {
                added.push(ch.new_path);
            } else {
                modified.push(ch.new_path);
            }
        }
        m.insert("added".into(), json!(added));
        m.insert("modified".into(), json!(modified));
        m.insert("removed".into(), json!(removed));
    }
    Ok(Value::Object(m))
}

fn state_id(state: &str) -> i64 {
    match state {
        "closed" => 2,
        "merged" => 3,
        "locked" => 4,
        _ => 1,
    }
}

fn human(secs: i64) -> Value {
    crate::chronic::human_value(secs)
}

/// `Gitlab::HookData::MergeRequestBuilder#build` (object_attributes without action).
pub fn mr_attrs(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest) -> Result<Map<String, Value>> {
    let r = repo::repo(c, p);
    let assignees = mr.assignee_ids(c)?;
    let labels = mr.labels(c)?;
    let mut m = Map::new();
    m.insert("assignee_id".into(), json!(assignees.first()));
    m.insert("author_id".into(), json!(mr.author_id));
    m.insert("created_at".into(), ar_time(mr.created_at));
    m.insert("description".into(), json!(mr.description));
    m.insert("draft".into(), json!(mr.is_draft()));
    m.insert("head_pipeline_id".into(), json!(crate::services::ci::head_pipeline(c, mr)?.map(|pl| pl.id)));
    m.insert("id".into(), json!(mr.id));
    m.insert("iid".into(), json!(mr.iid));
    m.insert("last_edited_at".into(), opt_ar_time(mr.last_edited_at));
    m.insert("last_edited_by_id".into(), json!(mr.last_edited_by_id));
    m.insert("merge_commit_sha".into(), json!(mr.merge_commit_sha));
    m.insert("merge_error".into(), json!(mr.merge_error));
    m.insert("merge_params".into(), Value::Object(mr.merge_params.clone()));
    m.insert("merge_status".into(), json!(mr.merge_status));
    m.insert("merge_user_id".into(), json!(mr.merge_user_id));
    m.insert("merge_when_pipeline_succeeds".into(), json!(mr.merge_when_pipeline_succeeds));
    m.insert("milestone_id".into(), json!(mr.milestone_id));
    m.insert("source_branch".into(), json!(mr.source_branch));
    m.insert("source_project_id".into(), json!(mr.source_project_id));
    m.insert("state_id".into(), json!(state_id(&mr.state)));
    m.insert("target_branch".into(), json!(mr.target_branch));
    m.insert("target_project_id".into(), json!(mr.target_project_id));
    m.insert("time_estimate".into(), json!(mr.time_estimate));
    m.insert("title".into(), json!(mr.title));
    m.insert("updated_at".into(), ar_time(mr.updated_at));
    m.insert("updated_by_id".into(), json!(mr.updated_by_id));
    m.insert("prepared_at".into(), opt_ar_time(mr.prepared_at));
    m.insert("assignee_ids".into(), json!(assignees));
    m.insert("blocking_discussions_resolved".into(), json!(mergeability::blocking_discussions_resolved(c, p, mr)?));
    m.insert("detailed_merge_status".into(), json!(mergeability::detailed_status(c, config, p, mr)?));
    let first = if mr.state == "merged" {
        mr.first_contribution
    } else {
        let n: i64 = c.query_row(
            "SELECT count(*) FROM merge_requests WHERE target_project_id = ?1 AND author_id = ?2 AND state = 'merged'",
            params![p.id, mr.author_id],
            |r| r.get(0),
        )?;
        n == 0
    };
    m.insert("first_contribution".into(), json!(first));
    m.insert("human_time_change".into(), Value::Null);
    m.insert("human_time_estimate".into(), human(mr.time_estimate));
    m.insert("human_total_time_spent".into(), human(mr.total_time_spent));
    m.insert("labels".into(), Value::Array(labels.iter().map(label_attrs).collect()));
    let last_commit = match mr.diff_head_sha(c, p)?.and_then(|h| Oid::from_hex(&h).ok()) {
        Some(o) if r.exists(&o)? => commit_attrs(config, p, &r, &o, false)?,
        _ => Value::Null,
    };
    m.insert("last_commit".into(), last_commit);
    m.insert("reviewer_ids".into(), json!(mr.reviewer_ids(c)?));
    let pa = project_attrs(c, config, p)?;
    m.insert("source".into(), pa.clone());
    m.insert("state".into(), json!(mr.state));
    m.insert("system".into(), json!(false));
    m.insert("target".into(), pa);
    m.insert("time_change".into(), json!(0));
    m.insert("total_time_spent".into(), json!(mr.total_time_spent));
    m.insert("url".into(), json!(mr.web_url(config, p)));
    m.insert("work_in_progress".into(), json!(mr.is_draft()));
    if config.is_ee() {
        m.insert("approval_rules".into(), Value::Array(crate::services::approvals::hook_rules(c, mr)?));
    }
    Ok(m)
}

fn reviewer_attrs(c: &Connection, mr: &MergeRequest, rerequested: Option<i64>) -> Result<Vec<Value>> {
    let mut out = vec![];
    for (_, uid, state, _) in mr.reviewers(c)? {
        let u = User::get(c, uid)?;
        let mut v = user_attrs(&u);
        let name = match state {
            1 => "reviewed",
            2 => "requested_changes",
            3 => "approved",
            4 => "unapproved",
            5 => "review_started",
            _ => "unreviewed",
        };
        v["state"] = json!(name);
        v["re_requested"] = json!(rerequested == Some(uid));
        out.push(v);
    }
    Ok(out)
}

/// `changes` for an update: attribute and association differences in GitLab's safe-list order.
fn changes(c: &Connection, before: &Snapshot, after: &MergeRequest) -> Result<Map<String, Value>> {
    let b = &before.mr;
    let mut m = Map::new();
    let state_name = |s: i64| match s {
        1 => "reviewed",
        2 => "requested_changes",
        3 => "approved",
        4 => "unapproved",
        5 => "review_started",
        _ => "unreviewed",
    };
    let reviewer_list = |list: &[(i64, i64)], rerequested: Option<i64>| -> Value {
        Value::Array(
            list.iter()
                .filter_map(|(id, st)| User::find(c, *id).ok().flatten().map(|u| (u, *st)))
                .map(|(u, st)| {
                    let mut v = user_attrs(&u);
                    v["state"] = json!(state_name(st));
                    v["re_requested"] = json!(rerequested == Some(u.id));
                    v
                })
                .collect(),
        )
    };
    let now_states: Vec<(i64, i64)> = after.reviewers(c)?.into_iter().map(|(_, u, s, _)| (u, s)).collect();
    let time_pairs = |m: &mut Map<String, Value>| {
        if b.total_time_spent != after.total_time_spent {
            m.insert("total_time_spent".into(), json!({"previous": b.total_time_spent, "current": after.total_time_spent}));
        }
        if let Some((prev, cur)) = before.time_change {
            m.insert("time_change".into(), json!({"previous": prev, "current": cur}));
        }
    };
    // `AddSpentTimeService` passes only `total_time_spent` as old associations, so the
    // reviewers (and assignees) show up as changed from nothing.
    if before.spend {
        // The saved note's touch of the same record shows up as an `updated_at` change.
        if let Some(t) = before.note_touch {
            m.insert("updated_at".into(), json!({"previous": ar_time(b.updated_at), "current": ar_time(t)}));
        }
        let assignees = after.assignee_ids(c)?;
        if !assignees.is_empty() {
            let v: Vec<Value> = assignees.iter().filter_map(|id| User::find(c, *id).ok().flatten()).map(|u| user_attrs(&u)).collect();
            m.insert("assignees".into(), json!({"previous": [], "current": v}));
        }
        if !now_states.is_empty() {
            m.insert("reviewers".into(), json!({"previous": [], "current": reviewer_list(&now_states, None)}));
        }
        time_pairs(&mut m);
        return Ok(m);
    }
    // Times compare at full precision but render at second precision.
    let mut pair = |k: &str, old: Value, new: Value| {
        if old != new {
            let render = |v: Value| match (k, v.as_i64()) {
                ("last_edited_at" | "updated_at" | "prepared_at", Some(ms)) => ar_time(ms),
                _ => v,
            };
            m.insert(k.to_string(), json!({"previous": render(old), "current": render(new)}));
        }
    };
    pair("description", json!(b.description), json!(after.description));
    pair("draft", json!(b.is_draft()), json!(after.is_draft()));
    pair("last_edited_at", json!(b.last_edited_at), json!(after.last_edited_at));
    pair("last_edited_by_id", json!(b.last_edited_by_id), json!(after.last_edited_by_id));
    pair("merge_commit_sha", json!(b.merge_commit_sha), json!(after.merge_commit_sha));
    pair("merge_error", json!(b.merge_error), json!(after.merge_error));
    pair("merge_params", Value::Object(b.merge_params.clone()), Value::Object(after.merge_params.clone()));
    pair("merge_status", json!(b.merge_status), json!(after.merge_status));
    pair("merge_user_id", json!(b.merge_user_id), json!(after.merge_user_id));
    pair("merge_when_pipeline_succeeds", json!(b.merge_when_pipeline_succeeds), json!(after.merge_when_pipeline_succeeds));
    pair("milestone_id", json!(b.milestone_id), json!(after.milestone_id));
    pair("state_id", json!(state_id(&b.state)), json!(state_id(&after.state)));
    pair("target_branch", json!(b.target_branch), json!(after.target_branch));
    pair("time_estimate", json!(b.time_estimate), json!(after.time_estimate));
    pair("title", json!(b.title), json!(after.title));
    pair("updated_at", json!(b.updated_at), json!(after.updated_at));
    pair("updated_by_id", json!(b.updated_by_id), json!(after.updated_by_id));
    pair("prepared_at", json!(b.prepared_at), json!(after.prepared_at));
    let users = |ids: &[i64]| -> Result<Value> {
        Ok(Value::Array(ids.iter().filter_map(|id| User::find(c, *id).ok().flatten()).map(|u| user_attrs(&u)).collect()))
    };
    let new_assignees = after.assignee_ids(c)?;
    if new_assignees != before.assignees {
        pair("assignees", users(&before.assignees)?, users(&new_assignees)?);
    }
    let new_labels: Vec<i64> = after.labels(c)?.into_iter().map(|l| l.id).collect();
    let (mut a, mut bl) = (new_labels.clone(), before.labels.clone());
    a.sort();
    bl.sort();
    if a != bl {
        let ls = |ids: &[i64]| -> Value {
            let mut v: Vec<Label> = ids.iter().filter_map(|id| Label::find(c, *id).ok().flatten()).collect();
            v.sort_by(|x, y| x.title.cmp(&y.title));
            Value::Array(v.iter().map(label_attrs).collect())
        };
        pair("labels", ls(&before.labels), ls(&new_labels));
    }
    // Reviewers with their real states; a re-requested review marks that reviewer.
    if now_states != before.reviewer_states || before.rerequested.is_some() {
        pair("reviewers", reviewer_list(&before.reviewer_states, None), reviewer_list(&now_states, before.rerequested));
    }
    time_pairs(&mut m);
    Ok(m)
}

#[allow(clippy::too_many_arguments)]
pub fn mr_payload(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: Option<&User>,
    action: &str,
    before: Option<&Snapshot>,
    oldrev: Option<&str>,
) -> Result<Value> {
    let mut attrs = mr_attrs(c, config, p, mr)?;
    if let Some((_, cur)) = before.and_then(|b| b.time_change) {
        attrs.insert("time_change".into(), json!(cur));
        attrs.insert("human_time_change".into(), human(cur));
    }
    attrs.insert("action".into(), json!(action));
    attrs.insert("actioned_at".into(), ar_time(time::now()));
    if let Some(o) = oldrev {
        attrs.insert("oldrev".into(), json!(o));
    }
    let labels = mr.labels(c)?;
    let mut m = Map::new();
    m.insert("object_kind".into(), json!("merge_request"));
    m.insert("event_type".into(), json!("merge_request"));
    m.insert("user".into(), user.map(user_attrs).unwrap_or(Value::Null));
    m.insert("project".into(), project_attrs(c, config, p)?);
    m.insert("object_attributes".into(), Value::Object(attrs));
    m.insert("labels".into(), Value::Array(labels.iter().map(label_attrs).collect()));
    m.insert("changes".into(), Value::Object(match before {
        Some(b) => changes(c, b, mr)?,
        None => Map::new(),
    }));
    m.insert("repository".into(), repository_attrs(c, config, p, false)?);
    let assignees = mr.assignee_ids(c)?;
    if !assignees.is_empty() {
        let v: Vec<Value> = assignees.iter().filter_map(|id| User::find(c, *id).ok().flatten()).map(|u| user_attrs(&u)).collect();
        m.insert("assignees".into(), Value::Array(v));
    }
    let reviewers = reviewer_attrs(c, mr, before.and_then(|b| b.rerequested))?;
    if !reviewers.is_empty() {
        m.insert("reviewers".into(), Value::Array(reviewers));
    }
    Ok(Value::Object(m))
}

fn has_hooks(c: &Connection, p: &Project, event: &str) -> Result<bool> {
    let project = Hook::list(c, "ProjectHook", Some(p.id))?.iter().any(|h| h.enabled(event));
    let system = matches!(event, "push_events" | "tag_push_events" | "merge_requests_events")
        && Hook::list(c, "SystemHook", None)?.iter().any(|h| h.enabled(event));
    Ok(project || system)
}

/// MR events: `open`, `close`, `reopen`, `merge`, `update`, `approved`, `unapproved`, ...
pub fn merge_request_event(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: Option<&User>,
    action: &str,
    before: Option<&Snapshot>,
) -> Result<()> {
    if !has_hooks(c, p, "merge_requests_events")? {
        return Ok(());
    }
    let data = mr_payload(c, config, p, &mr.reload(c)?, user, action, before, None)?;
    fire(c, p, "merge_requests_events", "merge_request_hooks", &data, None)
}

/// An MR event raised by the system rather than the user (EE: `approvals_reset_on_push`):
/// `system` is true, `system_action` follows `actioned_at`, and `merge_status` is the one the
/// MR had when the event was raised.
pub fn merge_request_system_event(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: Option<&User>,
    action: &str,
    system_action: &str,
    merge_status: &str,
) -> Result<()> {
    if !has_hooks(c, p, "merge_requests_events")? {
        return Ok(());
    }
    let mut data = mr_payload(c, config, p, &mr.reload(c)?, user, action, None, None)?;
    let mut then = mr.reload(c)?;
    then.merge_status = merge_status.to_string();
    let detailed = crate::services::merge_requests::mergeability::detailed_status(c, config, p, &then)?;
    let attrs = &mut data["object_attributes"];
    attrs["merge_status"] = json!(merge_status);
    attrs["detailed_merge_status"] = json!(detailed);
    attrs["system"] = json!(true);
    attrs["system_action"] = json!(system_action);
    fire(c, p, "merge_requests_events", "merge_request_hooks", &data, None)
}

/// `Refresh::WebHooksService`: `update` with `oldrev` after a push to the source branch.
pub fn merge_request_push_update(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, user: &User, _before: &Snapshot, oldrev: &Oid) -> Result<()> {
    if !has_hooks(c, p, "merge_requests_events")? {
        return Ok(());
    }
    let old = if oldrev.is_zero() { None } else { Some(oldrev.hex()) };
    let data = mr_payload(c, config, p, mr, Some(user), "update", None, old.as_deref())?;
    fire(c, p, "merge_requests_events", "merge_request_hooks", &data, None)
}

/// `Note` hook attributes (object_attributes without `action`).
pub fn note_attrs(config: &Config, p: &Project, mr: &MergeRequest, n: &Note) -> Map<String, Value> {
    let pos = |s: &Option<String>| -> Value { s.as_deref().and_then(Position::parse).map(|p| p.to_json()).unwrap_or(Value::Null) };
    let mut m = Map::new();
    m.insert("author_id".into(), json!(n.author_id));
    // A `DiffNote` without a change position serializes an empty `Position`.
    let change = match (&n.change_position, n.kind.as_deref()) {
        (None, Some("DiffNote")) => json!({"base_sha": null, "start_sha": null, "head_sha": null, "old_path": null, "new_path": null,
                                          "position_type": "text", "old_line": null, "new_line": null, "line_range": null}),
        _ => pos(&n.change_position),
    };
    m.insert("change_position".into(), change);
    m.insert("commit_id".into(), json!(n.commit_id));
    m.insert("created_at".into(), ar_time(n.created_at));
    m.insert("discussion_id".into(), json!(n.discussion_id));
    m.insert("id".into(), json!(n.id));
    m.insert("internal".into(), json!(n.internal));
    m.insert("line_code".into(), json!(n.line_code));
    m.insert("note".into(), json!(n.note));
    m.insert("noteable_id".into(), json!(n.noteable_id));
    m.insert("noteable_type".into(), json!(n.noteable_type));
    m.insert("original_position".into(), pos(&n.original_position));
    m.insert("position".into(), pos(&n.position));
    m.insert("project_id".into(), json!(n.project_id));
    m.insert("resolved_at".into(), opt_ar_time(n.resolved_at));
    m.insert("resolved_by_id".into(), json!(n.resolved_by_id));
    // Only set once the note has been resolved.
    m.insert("resolved_by_push".into(), if n.resolved_at.is_some() { json!(n.resolved_by_push) } else { Value::Null });
    m.insert("st_diff".into(), Value::Null);
    m.insert("system".into(), json!(n.system));
    m.insert("type".into(), json!(n.kind));
    m.insert("updated_at".into(), ar_time(n.updated_at));
    m.insert("updated_by_id".into(), json!(n.updated_by_id));
    m.insert("description".into(), json!(n.note));
    m.insert("url".into(), json!(format!("{}#note_{}", mr.web_url(config, p), n.id)));
    m
}

/// Note hooks (`note` / `confidential_note`); never for system notes.
pub fn note_event(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, user: &User, note: &Note, action: &str) -> Result<()> {
    let prepared = prepare_note_event(c, config, p, mr, user, note, action)?;
    fire_prepared(c, p, prepared)
}

/// A note hook built now and fired later.
pub struct PreparedHook {
    event: &'static str,
    scope: &'static str,
    data: Value,
    mr_id: i64,
}

/// Builds a note hook's payload without delivering it: a note with quick actions is hooked with
/// the MR as it was before its commands ran, but delivered after their hooks.
pub fn prepare_note_event(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, _user: &User, note: &Note, action: &str) -> Result<Option<PreparedHook>> {
    if note.system {
        return Ok(None);
    }
    let (event, scope) = if note.internal {
        ("confidential_note_events", "confidential_note_hooks")
    } else {
        ("note_events", "note_hooks")
    };
    if !has_hooks(c, p, event)? {
        return Ok(None);
    }
    let author = User::get(c, note.author_id)?;
    let data = note_payload(c, config, p, mr, &author, note, action)?;
    Ok(Some(PreparedHook { event, scope, data, mr_id: mr.id }))
}

/// Columns stay as they were when the note was saved, but time spent is summed from the
/// timelogs when the hook is built, so a `/spend` in the note shows.
pub fn fire_prepared(c: &Connection, p: &Project, prepared: Option<PreparedHook>) -> Result<()> {
    let Some(mut h) = prepared else { return Ok(()) };
    if let Some(m) = h.data.get_mut("merge_request").and_then(Value::as_object_mut)
        && let Some(mr) = MergeRequest::find(c, h.mr_id)?
    {
        m.insert("total_time_spent".into(), json!(mr.total_time_spent));
        m.insert("human_total_time_spent".into(), human(mr.total_time_spent));
    }
    fire(c, p, h.event, h.scope, &h.data, None)
}

/// `Gitlab::DataBuilder::Note.build`.
pub fn note_payload(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, author: &User, note: &Note, action: &str) -> Result<Value> {
    let event_type = if note.internal { "confidential_note" } else { "note" };
    let mut attrs = note_attrs(config, p, mr, note);
    attrs.insert("action".into(), json!(action));
    let mut m = Map::new();
    m.insert("object_kind".into(), json!("note"));
    m.insert("event_type".into(), json!(event_type));
    m.insert("user".into(), user_attrs(author));
    m.insert("project_id".into(), json!(p.id));
    m.insert("project".into(), project_attrs(c, config, p)?);
    m.insert("object_attributes".into(), Value::Object(attrs));
    m.insert("repository".into(), repository_attrs(c, config, p, false)?);
    m.insert("merge_request".into(), Value::Object(mr_attrs(c, config, p, &mr.reload(c)?)?));
    Ok(Value::Object(m))
}

/// Emoji hooks (`award` / `revoke`).
#[allow(clippy::too_many_arguments)]
pub fn emoji_event(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    note: Option<&Note>,
    user: &User,
    award: &Award,
    action: &str,
) -> Result<()> {
    if !has_hooks(c, p, "emoji_events")? {
        return Ok(());
    }
    let data = emoji_payload(c, config, p, mr, note, user, award, action)?;
    fire(c, p, "emoji_events", "emoji_hooks", &data, None)
}

/// `Gitlab::DataBuilder::Emoji.build`.
#[allow(clippy::too_many_arguments)]
pub fn emoji_payload(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    note: Option<&Note>,
    user: &User,
    award: &Award,
    action: &str,
) -> Result<Value> {
    let url = match note {
        Some(n) => format!("{}#note_{}", mr.web_url(config, p), n.id),
        None => mr.web_url(config, p),
    };
    let mut m = Map::new();
    m.insert("object_kind".into(), json!("emoji"));
    m.insert("event_type".into(), json!(action));
    m.insert("user".into(), user_attrs(user));
    m.insert("project_id".into(), json!(p.id));
    m.insert("project".into(), project_attrs(c, config, p)?);
    m.insert(
        "object_attributes".into(),
        json!({
            "user_id": award.user_id,
            "created_at": ar_time(award.created_at),
            "id": award.id,
            "name": award.name,
            "awardable_type": award.awardable_type,
            "awardable_id": award.awardable_id,
            "updated_at": ar_time(award.updated_at),
            "awarded_on_url": url,
        }),
    );
    if let Some(n) = note {
        m.insert("note".into(), Value::Object(note_attrs(config, p, mr, n)));
    }
    m.insert("merge_request".into(), Value::Object(mr_attrs(c, config, p, &mr.reload(c)?)?));
    Ok(Value::Object(m))
}

/// Pipeline hooks (`Gitlab::DataBuilder::Pipeline`) on each pipeline status transition.
pub fn pipeline_event(c: &Connection, config: &Config, p: &Project, pl: &crate::services::ci::Pipeline) -> Result<()> {
    match pipeline_event_data(c, config, p, pl)? {
        Some(data) => fire_pipeline_event(c, p, &data),
        None => Ok(()),
    }
}

/// Delivers a pipeline hook payload built earlier (`pipeline_event_data`).
pub fn fire_pipeline_event(c: &Connection, p: &Project, data: &Value) -> Result<()> {
    fire(c, p, "pipeline_events", "pipeline_hooks", data, None)
}

/// The pipeline hook payload as of now (`None` without pipeline hooks): GitLab builds it when the
/// pipeline changes and delivers it later.
pub fn pipeline_event_data(c: &Connection, config: &Config, p: &Project, pl: &crate::services::ci::Pipeline) -> Result<Option<Value>> {
    if !has_hooks(c, p, "pipeline_events")? {
        return Ok(None);
    }
    let r = repo::repo(c, p);
    let user = pl.user_id.map(|id| User::find(c, id)).transpose()?.flatten();
    let commit = match Oid::from_hex(&pl.sha) {
        Ok(o) if r.exists(&o)? => commit_attrs(config, p, &r, &o, false)?,
        _ => Value::Null,
    };
    let label = match pl.status.as_str() {
        "success" => "passed",
        other => other,
    };
    // MR pipelines (detached, merged results, merge train) report the source branch as `ref`
    // and carry their MR.
    let mr = match (pl.merge_request_id, pl.ref_.as_deref().and_then(|r| r.strip_prefix("refs/merge-requests/")).and_then(|r| r.strip_suffix("/head"))) {
        (Some(id), _) if pl.source == "merge_request_event" => crate::services::merge_requests::MergeRequest::find(c, id)?,
        (None, Some(iid)) if pl.source == "merge_request_event" => {
            let iid: i64 = iid.parse().unwrap_or(0);
            crate::services::merge_requests::MergeRequest::find_by_iid(c, p.id, iid)?
        }
        _ => None,
    };
    let hook_ref = mr.as_ref().map(|m| m.source_branch.clone()).or(pl.ref_.clone());
    let merge_request = match &mr {
        Some(m) => json!({
            "id": m.id, "iid": m.iid, "title": m.title, "source_branch": m.source_branch, "source_project_id": m.source_project_id,
            "target_branch": m.target_branch, "target_project_id": m.target_project_id, "state": m.state,
            "merge_status": crate::services::merge_requests::mergeability::public_status(&m.merge_status),
            "detailed_merge_status": mergeability::detailed_status(c, config, p, m)?,
            "url": config.url(&format!("/{}/-/merge_requests/{}", p.full_path, m.iid)),
        }),
        None => Value::Null,
    };
    // Stages of the pipeline's jobs: commit statuses run in `external`, fake CI jobs in theirs.
    let jobs: i64 = c.query_row("SELECT count(*) FROM commit_statuses WHERE pipeline_id = ?1 AND build = 0", [pl.id], |r| r.get(0))?;
    let builds = build_attrs(c, pl)?;
    let stages = if !builds.is_empty() {
        let mut st: Vec<String> = vec![];
        for b in &builds {
            if let Some(x) = b["stage"].as_str()
                && !st.iter().any(|s| s == x)
            {
                st.push(x.to_string());
            }
        }
        json!(st)
    } else if jobs > 0 {
        json!(["external"])
    } else {
        json!([])
    };
    let ref_status = if builds.is_empty() { Value::Null } else { json!(ci_ref_status(c, pl, hook_ref.as_deref())?) };
    // `hook_attrs(backward: false)`: without the legacy URL keys.
    let mut pipeline_project = project_attrs(c, config, p)?;
    if let Some(o) = pipeline_project.as_object_mut() {
        for k in ["homepage", "url", "ssh_url", "http_url"] {
            o.shift_remove(k);
        }
    }
    let data = json!({
        "object_kind": "pipeline",
        "object_attributes": {
            "id": pl.id,
            "iid": pl.iid,
            "name": pl.name,
            "ref": hook_ref,
            // `ci_ref&.status_name`: commit-status pipelines have no CI ref.
            "ref_status_name": ref_status,
            "tag": pl.tag,
            "sha": pl.sha,
            "before_sha": pl.before_sha.clone().unwrap_or_else(|| "0000000000000000000000000000000000000000".into()),
            "source": pl.source,
            "status": pl.status,
            "detailed_status": label,
            "stages": stages,
            "created_at": ar_time(pl.created_at),
            "finished_at": opt_ar_time(pl.finished_at),
            "duration": pl.duration,
            "queued_duration": pl.queued_duration,
            "protected_ref": pl.ref_.as_deref().map(|b| repo::is_protected(c, p, b)).transpose()?.unwrap_or(false),
            "variables": [],
            "url": config.url(&format!("/{}/-/pipelines/{}", p.full_path, pl.id)),
        },
        "merge_request": merge_request,
        "user": user.as_ref().map(user_attrs),
        "project": pipeline_project,
        "commit": commit,
        "builds": builds,
    });
    Ok(Some(data))
}

/// The runner labrat's fake CI jobs run on, as pipeline hooks show it.
fn fake_runner_attrs() -> Value {
    json!({"id": 1, "description": "labrat-fake-runner", "runner_type": "instance_type", "active": true, "is_shared": true, "tags": []})
}

/// The status of the pipeline's CI ref (its branch; the source branch for MR pipelines): set
/// as each fake CI pipeline of the ref finishes, `unknown` before any did. On the EE
/// reference a merge train pipeline reported the source branch's earlier `success` while it
/// was still pending.
fn ci_ref_status(c: &Connection, pl: &crate::services::ci::Pipeline, branch: Option<&str>) -> Result<&'static str> {
    let mut status = "unknown";
    let mut all = crate::services::ci::Pipeline::list(c, pl.project_id)?;
    all.reverse();
    for other in all.iter().filter(|o| o.id <= pl.id && matches!(o.status.as_str(), "success" | "failed")) {
        let has_builds: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM commit_statuses WHERE pipeline_id = ?1 AND build = 1)", [other.id], |r| r.get(0))?;
        if !has_builds {
            continue;
        }
        let other_branch = match other.merge_request_id {
            Some(id) => crate::services::merge_requests::MergeRequest::find(c, id)?.map(|m| m.source_branch),
            None => other.ref_.clone(),
        };
        if other_branch.as_deref() != branch {
            continue;
        }
        status = match (other.status.as_str(), status) {
            ("success", "failed" | "broken" | "still_failing") => "fixed",
            ("success", _) => "success",
            ("failed", "success" | "fixed") => "broken",
            ("failed", "failed" | "broken" | "still_failing") => "still_failing",
            _ => "failed",
        };
    }
    Ok(status)
}

/// `builds` of a pipeline hook: the fake CI jobs, newest first.
fn build_attrs(c: &Connection, pl: &crate::services::ci::Pipeline) -> Result<Vec<Value>> {
    let mut st = c.prepare(
        "SELECT id, name, stage, status, user_id, created_at, queued_at, started_at, finished_at FROM commit_statuses
         WHERE pipeline_id = ?1 AND build = 1 AND retried = 0 ORDER BY id DESC",
    )?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(i64, String, Option<String>, String, Option<i64>, i64, Option<i64>, Option<i64>, Option<i64>)> = st
        .query_map([pl.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for (id, name, stage, status, user_id, created, queued, started, finished) in rows {
        let user = user_id.map(|u| User::find(c, u)).transpose()?.flatten();
        let secs = |ms: i64| ms as f64 / 1000.0;
        out.push(json!({
            "id": id,
            "stage": stage.unwrap_or_else(|| "test".into()),
            "name": name,
            "status": status,
            "created_at": ar_time(created),
            "started_at": opt_ar_time(started),
            "finished_at": opt_ar_time(finished),
            // A running job reports the time it has run so far.
            "duration": started.map(|s| secs(finished.unwrap_or_else(time::now) - s)),
            "queued_duration": started.map(|s| secs(s - queued.unwrap_or(created))),
            "failure_reason": if status == "failed" { json!("script_failure") } else { Value::Null },
            "when": "on_success",
            "manual": false,
            "allow_failure": false,
            "user": user.as_ref().map(user_attrs),
            "runner": if started.is_some() { fake_runner_attrs() } else { Value::Null },
            "artifacts_file": {"filename": null, "size": null},
            "environment": null,
        }));
    }
    Ok(out)
}

/// Push and tag push hooks for one ref update.
pub fn push_event(c: &Connection, config: &Config, p: &Project, user: &User, old: &Oid, new: &Oid, ref_name: &str, commits: &[Oid], total: usize) -> Result<()> {
    let tag = ref_name.starts_with("refs/tags/");
    let event = if tag { "tag_push_events" } else { "push_events" };
    if !has_hooks(c, p, event)? {
        return Ok(());
    }
    let r = repo::repo(c, p);
    let (checkout, message) = if new.is_zero() {
        (Value::Null, Value::Null)
    } else if tag {
        let peeled = r.peel(new)?;
        let msg = match r.read(new)? {
            (labrat_git::Kind::Tag, d) => json!(labrat_git::Tag::parse(&d)?.message.trim_end()),
            _ => Value::Null,
        };
        (json!(peeled.hex()), msg)
    } else {
        (json!(new.hex()), Value::Null)
    };
    let shown: Vec<Value> = commits
        .iter()
        .skip(commits.len().saturating_sub(20))
        .map(|o| commit_attrs(config, p, &r, o, true))
        .collect::<Result<_>>()?;
    let protected = if tag { false } else { repo::is_protected(c, p, ref_name.trim_start_matches("refs/heads/"))? };
    let kind = if tag { "tag_push" } else { "push" };
    let hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(user.email.trim().to_lowercase().as_bytes()))
    };
    let mut m = Map::new();
    m.insert("object_kind".into(), json!(kind));
    m.insert("event_name".into(), json!(kind));
    m.insert("before".into(), json!(old.hex()));
    m.insert("after".into(), json!(new.hex()));
    m.insert("ref".into(), json!(ref_name));
    m.insert("ref_protected".into(), json!(protected));
    m.insert("checkout_sha".into(), checkout);
    m.insert("message".into(), message);
    m.insert("user_id".into(), json!(user.id));
    m.insert("user_name".into(), json!(user.name));
    m.insert("user_username".into(), json!(user.username));
    m.insert("user_email".into(), json!(user.public_email.clone().filter(|e| !e.is_empty())));
    m.insert("user_avatar".into(), json!(format!("https://www.gravatar.com/avatar/{hash}?s=80&d=identicon")));
    m.insert("project_id".into(), json!(p.id));
    m.insert("project".into(), project_attrs(c, config, p)?);
    m.insert("commits".into(), Value::Array(shown));
    m.insert("total_commits_count".into(), json!(total));
    m.insert("push_options".into(), json!({}));
    m.insert("repository".into(), repository_attrs(c, config, p, true)?);
    let scope = if tag { "tag_push_hooks" } else { "push_hooks" };
    fire(c, p, event, scope, &Value::Object(m), Some(ref_name))
}

/// The commits a push hook reports for a ref update, oldest first (before the 20 limit).
pub fn push_commits(c: &Connection, p: &Project, u: &labrat_git::protocol::RefUpdate) -> Result<Vec<Oid>> {
    let r = repo::repo(c, p);
    let mut commits: Vec<Oid> = vec![];
    if u.new.is_zero() {
        return Ok(commits);
    }
    if !u.name.starts_with("refs/tags/") {
        let branch = u.name.trim_start_matches("refs/heads/");
        let is_default = p.default_branch.is_none() || p.default_branch.as_deref() == Some(branch);
        if u.old.is_zero() {
            if is_default {
                commits = labrat_git::walk::rev_list(&r, &[u.new], &[])?;
                commits.truncate(101);
            } else {
                let default = p.default_branch.as_deref().and_then(|b| r.branch(b).ok().flatten());
                commits = labrat_git::walk::rev_list(&r, &[u.new], &default.into_iter().collect::<Vec<_>>())?;
            }
        } else {
            commits = labrat_git::walk::rev_list(&r, &[u.new], &[u.old])?;
        }
        commits.reverse();
    } else {
        let peeled = r.peel(&u.new)?;
        if r.kind(&peeled)? == Some(labrat_git::Kind::Commit) {
            commits = vec![peeled];
        }
    }
    Ok(commits)
}

/// Registered as a push hook: fires push / tag push webhooks for one ref update.
pub fn on_push(c: &Connection, config: &Config, p: &Project, user: &User, u: &labrat_git::protocol::RefUpdate) -> Result<()> {
    let commits = push_commits(c, p, u)?;
    let total = commits.len();
    // `EventCreateService#push` with `PushEventPayloadService`.
    let r = repo::repo(c, p);
    let title = match commits.last() {
        Some(o) => {
            let msg = r.commit(o)?.message;
            let first = msg.split('\n').next().unwrap_or("").trim().to_string();
            Some(if first.chars().count() > 70 { format!("{}...", first.chars().take(67).collect::<String>()) } else { first })
        }
        None => None,
    };
    let tag = u.name.starts_with("refs/tags/");
    let payload = json!({
        "commit_count": total,
        "action": if u.old.is_zero() { "created" } else if u.new.is_zero() { "removed" } else { "pushed" },
        "ref_type": if tag { "tag" } else { "branch" },
        "commit_from": if u.old.is_zero() { Value::Null } else { json!(u.old.hex()) },
        "commit_to": if u.new.is_zero() { Value::Null } else { json!(u.new.hex()) },
        "ref": u.name.trim_start_matches("refs/heads/").trim_start_matches("refs/tags/"),
        "commit_title": title,
        "ref_count": null,
    });
    crate::services::events::record(c, Some(p.id), user.id, "pushed", Some("Project"), Some(p.id), Some(payload))?;
    if SKIP_PUSH_HOOKS.with(|s| s.get()) {
        return Ok(());
    }
    push_event(c, config, p, user, &u.old, &u.new, &u.name, &commits, total)
}

thread_local! {
    /// Set while processing refs of a push that exceeds `push_event_hooks_limit`.
    pub static SKIP_PUSH_HOOKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `push_event_hooks_limit` (default 3): a push that creates, removes or updates more refs of
/// one type than this sends no push hooks for them (`Git::ProcessRefChangesService`).
pub const PUSH_EVENT_HOOKS_LIMIT: usize = 3;

/// Note hooks for a comment on a commit: the note's attributes and the commit.
pub fn commit_note_event(c: &Connection, config: &Config, p: &Project, note: &Note, action: &str) -> Result<()> {
    if note.system {
        return Ok(());
    }
    let (event, scope) = if note.internal { ("confidential_note_events", "confidential_note_hooks") } else { ("note_events", "note_hooks") };
    if !has_hooks(c, p, event)? {
        return Ok(());
    }
    let Some(data) = commit_note_payload(c, config, p, note, action)? else { return Ok(()) };
    fire(c, p, event, scope, &data, None)
}

/// The Note Hook payload for a comment on a commit.
pub fn commit_note_payload(c: &Connection, config: &Config, p: &Project, note: &Note, action: &str) -> Result<Option<Value>> {
    let Some(sha) = note.commit_id.clone() else { return Ok(None) };
    let r = crate::repo::repo(c, p);
    let oid = Oid::from_hex(&sha)?;
    let author = User::get(c, note.author_id)?;
    let pos = |s: &Option<String>| -> Value { s.as_deref().and_then(Position::parse).map(|p| p.to_json()).unwrap_or(Value::Null) };
    // `LegacyDiffNote#st_diff`: the commit's diff of the commented file.
    let st_diff = if note.kind.as_deref() == Some("LegacyDiffNote") {
        let info: Value = note.original_position.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
        let path = info["path"].as_str().unwrap_or_default().to_string();
        let commit = r.commit(&oid)?;
        let parent_tree = match commit.parents.first() {
            Some(pp) => Some(r.commit(pp)?.tree),
            None => None,
        };
        let mut out = Value::Null;
        for ch in labrat_git::diff::diff_trees(&r, parent_tree.as_ref(), Some(&commit.tree))? {
            if ch.new_path == path {
                out = json!({
                    "diff": labrat_git::diff::change_patch(&r, &ch)?, "new_path": ch.new_path, "old_path": ch.old_path,
                    "a_mode": ch.a_mode, "b_mode": ch.b_mode, "new_file": ch.new_file, "renamed_file": ch.renamed_file,
                    "deleted_file": ch.deleted_file, "too_large": false, "generated": null, "encoded_file_path": false,
                });
                break;
            }
        }
        out
    } else {
        Value::Null
    };
    let original = if note.kind.as_deref() == Some("LegacyDiffNote") { Value::Null } else { pos(&note.original_position) };
    // A `DiffNote` without a change position serializes an empty `Position`.
    let change_position = match (&note.change_position, note.kind.as_deref()) {
        (None, Some("DiffNote")) => json!({"base_sha": null, "start_sha": null, "head_sha": null, "old_path": null, "new_path": null,
                                          "position_type": "text", "old_line": null, "new_line": null, "line_range": null}),
        _ => pos(&note.change_position),
    };
    let attrs = json!({
        "author_id": note.author_id, "change_position": change_position, "commit_id": sha,
        "created_at": ar_time(note.created_at), "discussion_id": note.discussion_id, "id": note.id, "internal": note.internal,
        "line_code": note.line_code, "note": note.note, "noteable_id": null, "noteable_type": "Commit",
        "original_position": original, "position": pos(&note.position), "project_id": note.project_id,
        "resolved_at": null, "resolved_by_id": null, "resolved_by_push": null, "st_diff": st_diff, "system": false,
        "type": note.kind, "updated_at": ar_time(note.updated_at), "updated_by_id": note.updated_by_id, "description": note.note,
        "url": config.url(&format!("/{}/-/commit/{sha}#note_{}", p.full_path, note.id)), "action": action,
    });
    let data = json!({
        "object_kind": "note", "event_type": if note.internal { "confidential_note" } else { "note" },
        "user": user_attrs(&author), "project_id": p.id, "project": project_attrs(c, config, p)?,
        "object_attributes": attrs, "repository": repository_attrs(c, config, p, false)?,
        "commit": commit_attrs(config, p, &r, &oid, false)?,
    });
    Ok(Some(data))
}

/// `SystemHooksService` `repository_update` (on by default for system hooks): one per push, with
/// every ref it changed.
pub fn repository_update(c: &Connection, config: &Config, p: &Project, user: &User, updates: &[labrat_git::protocol::RefUpdate]) -> Result<()> {
    let hooks: Vec<Hook> = Hook::list(c, "SystemHook", None)?.into_iter().filter(|h| h.enabled("repository_update_events")).collect();
    if hooks.is_empty() || updates.is_empty() {
        return Ok(());
    }
    use sha2::Digest;
    let hash = hex::encode(sha2::Sha256::digest(user.email.trim().to_lowercase().as_bytes()));
    let changes: Vec<Value> = updates.iter().map(|u| json!({"before": u.old.hex(), "after": u.new.hex(), "ref": u.name})).collect();
    let refs: Vec<Value> = updates.iter().map(|u| json!(u.name)).collect();
    let data = json!({
        "event_name": "repository_update",
        "user_id": user.id,
        "user_name": user.name,
        "user_email": user.email,
        "user_avatar": format!("https://www.gravatar.com/avatar/{hash}?s=80&d=identicon"),
        "project_id": p.id,
        "project": project_attrs(c, config, p)?,
        "changes": changes,
        "refs": refs,
    });
    for h in hooks {
        enqueue(c, &h, "system_hooks", &data)?;
    }
    Ok(())
}
