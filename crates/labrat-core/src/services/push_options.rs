//! `git push -o merge_request.*` (`Gitlab::PushOptions` and `MergeRequests::PushOptionsHandlerService`):
//! create or update merge requests for pushed branches, synchronously within the push.

use crate::model::{Project, User};
use crate::perms::ProjectPerms;
use crate::services::merge_requests::{self as mrs, MergeRequest, MrParams};
use crate::{Config, Result, repo};
use labrat_git::protocol::RefUpdate;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;

const LIMIT: usize = 10;
const KEYS: &[&str] = &[
    "assign", "create", "description", "draft", "label", "merge_when_pipeline_succeeds", "auto_merge", "milestone",
    "remove_source_branch", "squash", "target", "target_project", "title", "unassign", "unlabel",
];
const MULTI: &[&str] = &["label", "unlabel", "assign", "unassign"];

#[derive(Debug, Default)]
pub struct MrOptions {
    pub single: BTreeMap<String, String>,
    pub multi: BTreeMap<String, Vec<String>>,
}

impl MrOptions {
    fn get(&self, k: &str) -> Option<&str> {
        self.single.get(k).map(String::as_str)
    }
    fn flag(&self, k: &str) -> Option<bool> {
        self.get(k).map(|v| v != "false")
    }
    pub fn is_empty(&self) -> bool {
        self.single.is_empty() && self.multi.is_empty()
    }
}

/// `Gitlab::PushOptions#parse_options` for the `merge_request` (alias `mr`) namespace.
pub fn parse(raw: &[String]) -> MrOptions {
    let mut out = MrOptions::default();
    for opt in raw {
        let Some((ns, rest)) = opt.split_once('.') else { continue };
        let (key, value) = match rest.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (rest.trim(), ""),
        };
        let ns = if ns.trim() == "mr" { "merge_request" } else { ns.trim() };
        if ns != "merge_request" || !KEYS.contains(&key) {
            continue;
        }
        let value = if value.is_empty() { "true".to_string() } else { value.to_string() };
        if MULTI.contains(&key) {
            let list = out.multi.entry(key.to_string()).or_default();
            if !list.contains(&value) {
                list.push(value);
            }
        } else {
            out.single.insert(key.to_string(), value);
        }
    }
    out
}

/// The alert banner Gitaly prints for post-receive alert messages: words wrapped and each line
/// centered in 72 columns, between `=` rules.
pub fn alert(message: &str) -> String {
    let mut lines = vec![];
    let mut line = String::new();
    for word in message.split_whitespace() {
        if !line.is_empty() && line.len() + word.len() >= 66 {
            lines.push(line.trim_end().to_string());
            line.clear();
        }
        line.push_str(word);
        line.push(' ');
    }
    if !line.trim().is_empty() {
        lines.push(line.trim_end().to_string());
    }
    let rule = "=".repeat(72);
    let mut out = format!("\n{rule}\n\n");
    for l in lines {
        let pad = 72usize.saturating_sub(l.len()) / 2;
        out.push_str(&format!("{}{l}\n", " ".repeat(pad)));
    }
    out.push_str(&format!("\n{rule}\n\n"));
    out
}

/// Handles the options for a push; returns the alert text when something went wrong.
pub fn handle(c: &Connection, config: &Config, p: &Project, user: &User, updates: &[RefUpdate], raw: &[String]) -> Result<Option<String>> {
    let opts = parse(raw);
    if opts.is_empty() {
        return Ok(None);
    }
    let mut errors: Vec<String> = vec![];
    let r = repo::repo(c, p);
    let default = p.default_branch.clone().unwrap_or_default();
    let branches: Vec<(String, &RefUpdate)> = updates
        .iter()
        .filter(|u| !u.new.is_zero())
        .filter_map(|u| u.name.strip_prefix("refs/heads/").map(|b| (b.to_string(), u)))
        .filter(|(b, _)| *b != default || opts.get("target").is_some())
        .collect();
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !perms.read_code(p) {
        errors.push("User access was denied".into());
    } else {
        if p.feature_level("merge_requests") == crate::model::feature::DISABLED {
            errors.push(format!("Merge requests are not enabled for project {}", p.full_path));
        }
        if branches.len() > LIMIT {
            errors.push(format!("Too many branches pushed ({} were pushed, limit is {LIMIT})", branches.len()));
        }
        if let Some(t) = opts.get("target")
            && r.branch(t)?.is_none() {
                errors.push(format!("Target branch {}:{t} does not exist", p.full_path));
            }
    }
    if errors.is_empty() {
        for (branch, u) in &branches {
            if let Err(e) = for_branch(c, config, p, user, &opts, branch, u, &mut errors) {
                tracing::warn!("push options for {branch}: {e}");
                errors.push("An unknown error occurred".into());
            }
        }
    }
    if errors.is_empty() {
        return Ok(None);
    }
    let quoted: Vec<String> = raw.iter().map(|o| format!("'{o}'")).collect();
    Ok(Some(alert(&format!("WARNINGS:\nError encountered with push options {}: {}", quoted.join(" "), errors.join("\n\n")))))
}

fn user_ids(c: &Connection, refs: &[String]) -> Result<Vec<i64>> {
    let mut out = vec![];
    for r in refs {
        if r.chars().all(|ch| ch.is_ascii_digit()) {
            out.push(r.parse().unwrap_or(0));
        } else if let Some(u) = User::find_by_username(c, r)? {
            out.push(u.id);
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn for_branch(
    c: &Connection,
    config: &Config,
    p: &Project,
    user: &User,
    opts: &MrOptions,
    branch: &str,
    u: &RefUpdate,
    errors: &mut Vec<String>,
) -> Result<()> {
    let existing = MergeRequest::list_where(
        c,
        "source_project_id = ?1 AND source_branch = ?2 AND state = 'opened' ORDER BY id DESC LIMIT 1",
        params![p.id, branch],
    )?
    .into_iter()
    .next();
    let multi = |k: &str| opts.multi.get(k).cloned();
    let milestone_id = match opts.get("milestone") {
        Some(title) => c
            .query_row("SELECT id FROM milestones WHERE project_id = ?1 AND title = ?2", params![p.id, title], |r| r.get::<_, i64>(0))
            .ok()
            .map(Some),
        None => None,
    };
    let description = opts.get("description").map(|d| Some(d.replace("\\n", "\n")));
    let add_assignees = multi("assign").map(|v| user_ids(c, &v)).transpose()?;
    let remove_assignees = multi("unassign").map(|v| user_ids(c, &v)).transpose()?;
    let auto_merge = opts.flag("auto_merge").or(opts.flag("merge_when_pipeline_succeeds"));
    match existing {
        Some(mr) => {
            if !mrs::can_update(c, p, &mr, user)? {
                errors.push("User access was denied".into());
                return Ok(());
            }
            let mut title = opts.get("title").map(str::to_string);
            // `UpdateService`: only `draft=true` changes the title.
            if opts.flag("draft") == Some(true) {
                let t = title.clone().unwrap_or(mr.title.clone());
                title = Some(mrs::draft_title(&t));
            }
            let assignees = if add_assignees.is_some() || remove_assignees.is_some() {
                let mut list = mr.assignee_ids(c)?;
                for a in add_assignees.unwrap_or_default() {
                    if !list.contains(&a) {
                        list.push(a);
                    }
                }
                let rm = remove_assignees.unwrap_or_default();
                list.retain(|a| !rm.contains(a));
                Some(list)
            } else {
                None
            };
            let prm = MrParams {
                title,
                description,
                target_branch: opts.get("target").map(str::to_string),
                remove_source_branch: opts.flag("remove_source_branch"),
                squash: opts.flag("squash"),
                add_labels: multi("label"),
                remove_labels: multi("unlabel"),
                assignee_ids: assignees,
                milestone_id,
                ..Default::default()
            };
            match mrs::update(c, config, p, &mr, user, prm) {
                Ok(_) => {}
                Err(e @ (crate::Error::Validation(_) | crate::Error::Api(..))) => errors.extend(messages(&e)),
                Err(e) => return Err(e),
            }
            if auto_merge == Some(true) {
                enable_auto_merge(c, config, p, &mr, user, &u.new.hex())?;
            }
        }
        None => {
            if opts.flag("create") != Some(true) {
                errors.push(format!("A merge_request.create push option is required to create a merge request for branch {branch}"));
                return Ok(());
            }
            let target = opts.get("target").map(str::to_string).unwrap_or(p.default_branch.clone().unwrap_or_default());
            let mut title = opts.get("title").map(str::to_string).unwrap_or_else(|| default_title(c, p, branch, u, &target));
            // `set_draft_title_if_needed`: requested, or nothing to merge yet.
            if opts.flag("draft") == Some(true) || new_commits(c, p, u, &target).is_empty() {
                title = mrs::draft_title(&title);
            }
            // `BuildService` applies `add/remove_assignee_ids` to the new MR's (empty) assignees,
            // so the default `[current_user]` only holds without assign/unassign options.
            let present = |v: &Option<Vec<i64>>| v.as_ref().is_some_and(|l| !l.is_empty());
            let mut assignees = if present(&add_assignees) || present(&remove_assignees) { vec![] } else { vec![user.id] };
            for a in add_assignees.unwrap_or_default() {
                if !assignees.contains(&a) {
                    assignees.push(a);
                }
            }
            let rm = remove_assignees.unwrap_or_default();
            assignees.retain(|a| !rm.contains(a));
            let prm = MrParams {
                title: Some(title),
                description: description.or(Some(default_description(c, p, u, &target))),
                source_branch: Some(branch.to_string()),
                target_branch: Some(target),
                remove_source_branch: opts.flag("remove_source_branch"),
                squash: opts.flag("squash"),
                labels: multi("label"),
                assignee_ids: Some(assignees),
                milestone_id,
                ..Default::default()
            };
            match mrs::create(c, config, p, user, prm) {
                Ok(mr) => {
                    if auto_merge == Some(true) {
                        enable_auto_merge(c, config, p, &mr, user, &u.new.hex())?;
                    }
                }
                Err(e @ (crate::Error::Validation(_) | crate::Error::Api(..))) => errors.extend(messages(&e)),
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

fn enable_auto_merge(c: &Connection, _config: &Config, p: &Project, mr: &MergeRequest, user: &User, sha: &str) -> Result<()> {
    let mp = crate::services::merge_requests::merge::MergeParams {
        commit_message: None,
        squash_commit_message: None,
        should_remove_source_branch: None,
        sha: Some(sha.to_string()),
        ..Default::default()
    };
    crate::services::merge_requests::merge::enable_auto_merge(c, p, &mr.reload(c)?, user, &mp)
}

/// `MergeRequests::BuildService` defaults: one commit gives its title (and body), several give
/// the humanized branch name.
/// The commits a new MR would contain (newest first).
fn new_commits(c: &Connection, p: &Project, u: &RefUpdate, target: &str) -> Vec<labrat_git::Oid> {
    let r = repo::repo(c, p);
    let base = r.branch(target).ok().flatten();
    labrat_git::walk::rev_list(&r, &[u.new], &base.into_iter().collect::<Vec<_>>()).unwrap_or_default()
}

/// `assign_title_and_description_from_commits`: the only commit, or else the oldest non-merge
/// commit with a description (`first_multiline_commit`).
fn title_commit(c: &Connection, p: &Project, u: &RefUpdate, target: &str) -> Option<labrat_git::Commit> {
    let r = repo::repo(c, p);
    let commits = new_commits(c, p, u, target);
    if commits.len() == 1 {
        return r.commit(&commits[0]).ok();
    }
    commits.iter().rev().filter_map(|o| r.commit(o).ok()).filter(|cm| cm.parents.len() <= 1).find(|cm| {
        let mut lines = cm.message.lines();
        lines.next();
        lines.any(|l| !l.trim().is_empty())
    })
}

fn default_title(c: &Connection, p: &Project, branch: &str, u: &RefUpdate, target: &str) -> String {
    match title_commit(c, p, u, target) {
        Some(cm) => cm.message.lines().next().unwrap_or("").to_string(),
        None => {
            let h = branch.replace(['-', '_'], " ");
            let mut ch = h.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => h,
            }
        }
    }
}

fn default_description(c: &Connection, p: &Project, u: &RefUpdate, target: &str) -> Option<String> {
    let cm = title_commit(c, p, u, target)?;
    let body: Vec<&str> = cm.message.lines().skip(1).collect();
    let text = body.join("\n").trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn messages(e: &crate::Error) -> Vec<String> {
    match e {
        crate::Error::Validation(list) => list
            .iter()
            .map(|(a, m)| {
                let mut h = a.replace('_', " ");
                if let Some(f) = h.get(0..1) {
                    h = f.to_uppercase() + &h[1..];
                }
                format!("{h} {m}")
            })
            .collect(),
        crate::Error::Api(_, serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        crate::Error::Api(_, serde_json::Value::String(m)) => vec![m.clone()],
        other => vec![other.to_string()],
    }
}
