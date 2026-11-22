//! Merge requests: model, create, update, close/reopen and the effects of pushes.

pub mod diffs;
pub mod merge;
pub mod mergeability;
pub mod refresh;

use crate::db::OptionalExt;
use crate::model::{Project, User, access};
use crate::perms::ProjectPerms;
use crate::services::{labels, notes};
use crate::{Config, Error, Result, jobs, repo, time};
use rusqlite::{Connection, Row, params};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
pub struct MergeRequest {
    pub id: i64,
    pub iid: i64,
    pub target_project_id: i64,
    pub source_project_id: Option<i64>,
    pub source_branch: String,
    pub target_branch: String,
    pub title: String,
    pub description: Option<String>,
    pub author_id: i64,
    pub state: String,
    pub merge_status: String,
    pub discussion_locked: Option<bool>,
    pub squash: bool,
    pub merge_params: Map<String, Value>,
    pub merge_when_pipeline_succeeds: bool,
    pub merge_user_id: Option<i64>,
    pub merge_commit_sha: Option<String>,
    pub squash_commit_sha: Option<String>,
    pub merged_commit_sha: Option<String>,
    pub merge_error: Option<String>,
    pub rebase_commit_sha: Option<String>,
    pub rebase_in_progress: bool,
    pub latest_diff_id: Option<i64>,
    pub milestone_id: Option<i64>,
    pub time_estimate: i64,
    pub total_time_spent: i64,
    pub prepared_at: Option<i64>,
    pub last_edited_at: Option<i64>,
    pub last_edited_by_id: Option<i64>,
    pub updated_by_id: Option<i64>,
    pub merged_by_id: Option<i64>,
    pub merged_at: Option<i64>,
    pub latest_closed_by_id: Option<i64>,
    pub latest_closed_at: Option<i64>,
    pub merge_ref_sha: Option<String>,
    pub first_contribution: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLUMNS: &str = "id, iid, target_project_id, source_project_id, source_branch, target_branch, title, description, author_id, state, merge_status, discussion_locked, squash, merge_params, merge_when_pipeline_succeeds, merge_user_id, merge_commit_sha, squash_commit_sha, merged_commit_sha, merge_error, rebase_commit_sha, rebase_in_progress, latest_diff_id, milestone_id, time_estimate, total_time_spent, prepared_at, last_edited_at, last_edited_by_id, updated_by_id, merged_by_id, merged_at, latest_closed_by_id, latest_closed_at, merge_ref_sha, first_contribution, created_at, updated_at";

impl MergeRequest {
    fn from_row(r: &Row) -> rusqlite::Result<MergeRequest> {
        let mp: String = r.get(13)?;
        Ok(MergeRequest {
            id: r.get(0)?,
            iid: r.get(1)?,
            target_project_id: r.get(2)?,
            source_project_id: r.get(3)?,
            source_branch: r.get(4)?,
            target_branch: r.get(5)?,
            title: r.get(6)?,
            description: r.get(7)?,
            author_id: r.get(8)?,
            state: r.get(9)?,
            merge_status: r.get(10)?,
            discussion_locked: r.get(11)?,
            squash: r.get(12)?,
            merge_params: serde_json::from_str(&mp).unwrap_or_default(),
            merge_when_pipeline_succeeds: r.get(14)?,
            merge_user_id: r.get(15)?,
            merge_commit_sha: r.get(16)?,
            squash_commit_sha: r.get(17)?,
            merged_commit_sha: r.get(18)?,
            merge_error: r.get(19)?,
            rebase_commit_sha: r.get(20)?,
            rebase_in_progress: r.get(21)?,
            latest_diff_id: r.get(22)?,
            milestone_id: r.get(23)?,
            time_estimate: r.get(24)?,
            total_time_spent: r.get(25)?,
            prepared_at: r.get(26)?,
            last_edited_at: r.get(27)?,
            last_edited_by_id: r.get(28)?,
            updated_by_id: r.get(29)?,
            merged_by_id: r.get(30)?,
            merged_at: r.get(31)?,
            latest_closed_by_id: r.get(32)?,
            latest_closed_at: r.get(33)?,
            merge_ref_sha: r.get(34)?,
            first_contribution: r.get(35)?,
            created_at: r.get(36)?,
            updated_at: r.get(37)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<MergeRequest>> {
        c.query_row(
            &format!("SELECT {COLUMNS} FROM merge_requests WHERE id = ?1"),
            [id],
            Self::from_row,
        )
        .optional()
    }

    pub fn get(c: &Connection, id: i64) -> Result<MergeRequest> {
        Self::find(c, id)?.ok_or(Error::Api(404, json!("404 Not found")))
    }

    pub fn find_by_iid(c: &Connection, project_id: i64, iid: i64) -> Result<Option<MergeRequest>> {
        c.query_row(
            &format!(
                "SELECT {COLUMNS} FROM merge_requests WHERE target_project_id = ?1 AND iid = ?2"
            ),
            params![project_id, iid],
            Self::from_row,
        )
        .optional()
    }

    pub fn list_where(
        c: &Connection,
        cond: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<MergeRequest>> {
        let mut st = c.prepare(&format!(
            "SELECT {COLUMNS} FROM merge_requests WHERE {cond}"
        ))?;
        let v = st
            .query_map(args, Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn is_open(&self) -> bool {
        self.state == "opened"
    }

    pub fn is_draft(&self) -> bool {
        is_draft_title(&self.title)
    }

    pub fn assignee_ids(&self, c: &Connection) -> Result<Vec<i64>> {
        let mut st = c.prepare("SELECT user_id FROM merge_request_assignees WHERE merge_request_id = ?1 ORDER BY rowid")?;
        let v = st
            .query_map([self.id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    }

    /// Reviewers as (reviewer row id, user id, state, created_at), in id order.
    pub fn reviewers(&self, c: &Connection) -> Result<Vec<(i64, i64, i64, i64)>> {
        let mut st = c.prepare(
            "SELECT id, user_id, state, created_at FROM merge_request_reviewers WHERE merge_request_id = ?1 ORDER BY id",
        )?;
        let v = st
            .query_map([self.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn reviewer_ids(&self, c: &Connection) -> Result<Vec<i64>> {
        Ok(self.reviewers(c)?.into_iter().map(|r| r.1).collect())
    }

    pub fn labels(&self, c: &Connection) -> Result<Vec<labels::Label>> {
        labels::for_target(c, "MergeRequest", self.id)
    }

    pub fn latest_diff(&self, c: &Connection) -> Result<Option<diffs::Diff>> {
        match self.latest_diff_id {
            Some(id) => diffs::Diff::find(c, id),
            None => Ok(None),
        }
    }

    pub fn force_remove_source_branch(&self) -> Option<bool> {
        self.merge_params
            .get("force_remove_source_branch")
            .and_then(to_bool)
    }

    pub fn should_remove_source_branch(&self) -> Option<bool> {
        self.merge_params
            .get("should_remove_source_branch")
            .and_then(to_bool)
    }

    /// `diff_head_sha`: the latest diff's head, falling back to the source branch head.
    pub fn diff_head_sha(&self, c: &Connection, p: &Project) -> Result<Option<String>> {
        if let Some(d) = self.latest_diff(c)?
            && let Some(h) = d.head_commit_sha {
                return Ok(Some(h));
            }
        self.source_branch_sha(c, p)
    }

    pub fn source_branch_sha(&self, c: &Connection, p: &Project) -> Result<Option<String>> {
        Ok(repo::repo(c, p)
            .branch(&self.source_branch)?
            .map(|o| o.hex()))
    }

    pub fn target_branch_sha(&self, c: &Connection, p: &Project) -> Result<Option<String>> {
        Ok(repo::repo(c, p)
            .branch(&self.target_branch)?
            .map(|o| o.hex()))
    }

    pub fn web_url(&self, config: &Config, p: &Project) -> String {
        config.url(&format!("/{}/-/merge_requests/{}", p.full_path, self.iid))
    }

    pub fn reload(&self, c: &Connection) -> Result<MergeRequest> {
        Self::get(c, self.id)
    }
}

fn to_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.as_str() {
            "true" | "1" | "t" | "yes" => Some(true),
            "false" | "0" | "f" | "no" | "" => Some(false),
            _ => None,
        },
        Value::Number(n) => Some(n.as_i64() != Some(0)),
        _ => None,
    }
}

/// `MergeRequest.draft?`: `[draft]`, `(draft)` or `draft:` at the very start, any case.
pub fn is_draft_title(title: &str) -> bool {
    let l = title.to_lowercase();
    l.starts_with("[draft]") || l.starts_with("(draft)") || l.starts_with("draft:")
}

/// Strips draft prefixes (repeated) and following whitespace.
pub fn draftless_title(title: &str) -> String {
    let mut t = title.to_string();
    loop {
        let l = t.to_lowercase();
        let n = if l.starts_with("[draft]") || l.starts_with("(draft)") {
            7
        } else if l.starts_with("draft:") {
            6
        } else {
            break;
        };
        t = t[n..].trim_start().to_string();
    }
    t
}

pub fn draft_title(title: &str) -> String {
    if is_draft_title(title) {
        title.to_string()
    } else {
        format!("Draft: {title}")
    }
}

fn next_mr_iid(c: &Connection, project_id: i64) -> Result<i64> {
    next_iid(c, "merge_requests", project_id)
}

/// `InternalId.generate_next` for a usage (`merge_requests`, `milestones`, ...).
pub fn next_iid(c: &Connection, usage: &str, project_id: i64) -> Result<i64> {
    let cur: Option<i64> = c
        .query_row(
            "SELECT last_value FROM internal_ids WHERE project_id = ?1 AND usage = ?2",
            params![project_id, usage],
            |r| r.get(0),
        )
        .optional()?;
    let next = cur.unwrap_or(0) + 1;
    c.execute(
        "INSERT INTO internal_ids (project_id, usage, last_value) VALUES (?1, ?2, ?3)
         ON CONFLICT (project_id, usage) DO UPDATE SET last_value = excluded.last_value",
        params![project_id, usage, next],
    )?;
    Ok(next)
}

/// Parameters shared by create and update. `None` means "not given".
#[derive(Debug, Default, Clone)]
pub struct MrParams {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub source_branch: Option<String>,
    pub target_branch: Option<String>,
    pub assignee_ids: Option<Vec<i64>>,
    pub reviewer_ids: Option<Vec<i64>>,
    pub labels: Option<Vec<String>>,
    pub add_labels: Option<Vec<String>>,
    pub remove_labels: Option<Vec<String>>,
    pub milestone_id: Option<Option<i64>>,
    pub remove_source_branch: Option<bool>,
    pub squash: Option<bool>,
    pub discussion_locked: Option<bool>,
    pub state_event: Option<String>,
    pub allow_collaboration: Option<bool>,
    /// `Some(None)` removes the merge schedule.
    pub merge_after: Option<Option<i64>>,
    /// `spend_time` (seconds, the note that asked for it).
    pub spend: Option<(i64, Option<i64>)>,
    /// `time_estimate` in seconds (quick actions `/estimate`, `/remove_estimate`).
    pub time_estimate: Option<i64>,
}

impl MrParams {
    /// `params.merge!(command_params)`: values from quick actions override the request's.
    pub fn merge_commands(&mut self, cmd: MrParams) {
        macro_rules! take {
            ($($f:ident),*) => { $( if cmd.$f.is_some() { self.$f = cmd.$f; } )* };
        }
        take!(title, target_branch, assignee_ids, reviewer_ids, labels, add_labels, remove_labels, milestone_id, discussion_locked, state_event, spend, time_estimate);
    }
}

/// Errors in GitLab's `handle_merge_request_errors!` shapes.
fn branch_errors(
    c: &Connection,
    p: &Project,
    source: &str,
    target: &str,
    check_existence: bool,
) -> Result<Vec<(String, String)>> {
    let r = repo::repo(c, p);
    let mut errs = vec![];
    if source.is_empty() {
        errs.push(("source_branch".into(), "can't be blank".into()));
    } else if !repo::valid_branch_name(source) {
        errs.push(("source_branch".into(), "is invalid".into()));
    } else if check_existence && r.branch(source)?.is_none() {
        errs.push(("source_branch".into(), "does not exist".into()));
    }
    if target.is_empty() {
        errs.push(("target_branch".into(), "can't be blank".into()));
    } else if !repo::valid_branch_name(target) {
        errs.push(("target_branch".into(), "is invalid".into()));
    } else if check_existence && r.branch(target)?.is_none() {
        errs.push(("target_branch".into(), "does not exist".into()));
    }
    Ok(errs)
}

fn duplicate_open(
    c: &Connection,
    p: &Project,
    source: &str,
    target: &str,
    except: Option<i64>,
) -> Result<Option<i64>> {
    let iid: Option<i64> = c
        .query_row(
            "SELECT iid FROM merge_requests WHERE target_project_id = ?1 AND source_branch = ?2 AND target_branch = ?3
             AND state = 'opened' AND id != ?4 ORDER BY id LIMIT 1",
            params![p.id, source, target, except.unwrap_or(-1)],
            |r| r.get(0),
        )
        .optional()?;
    Ok(iid)
}

/// Users from `ids` who can read merge requests in `p`: CE keeps only the first; EE (multiple
/// assignees and reviewers) keeps every one, in the order given.
/// `Some(vec![])` clears; `None` means "no change".
fn filter_users(c: &Connection, config: &Config, p: &Project, ids: &[i64]) -> Result<Option<Vec<i64>>> {
    if ids.is_empty() || ids == [0] {
        return Ok(Some(vec![]));
    }
    if config.is_ee() {
        let mut keep = vec![];
        for id in ids {
            if let Some(u) = User::find(c, *id)?
                && ProjectPerms::load(c, Some(&u), p)?.read_merge_request(p)
                && !keep.contains(&u.id)
            {
                keep.push(u.id);
            }
        }
        if keep.is_empty() {
            return Ok(None);
        }
        return Ok(Some(keep));
    }
    let first = ids[0];
    if first == 0 {
        return Ok(Some(vec![]));
    }
    let Some(u) = User::find(c, first)? else {
        return Ok(None);
    };
    let perms = ProjectPerms::load(c, Some(&u), p)?;
    if perms.read_merge_request(p) {
        Ok(Some(vec![u.id]))
    } else {
        Ok(None)
    }
}

fn can_admin_mr(c: &Connection, user: &User, p: &Project) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    Ok(perms.developer())
}

fn set_assignees(c: &Connection, mr_id: i64, ids: &[i64]) -> Result<()> {
    let old: Vec<i64> = {
        let mut st = c.prepare("SELECT user_id FROM merge_request_assignees WHERE merge_request_id = ?1")?;
        st.query_map([mr_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    // `HandleAssigneesChangeService`: old and new assignees.
    let mut affected = old.clone();
    affected.extend_from_slice(ids);
    if old.iter().collect::<std::collections::BTreeSet<_>>() != ids.iter().collect() {
        crate::services::user_counts::invalidate(c, &affected)?;
    }
    // Assignees who stay keep their row; new ones are added in user id order (`User.where(id:)`),
    // and the list follows the rows.
    for id in &old {
        if !ids.contains(id) {
            c.execute("DELETE FROM merge_request_assignees WHERE merge_request_id = ?1 AND user_id = ?2", params![mr_id, id])?;
        }
    }
    let mut new: Vec<i64> = ids.to_vec();
    new.sort();
    for id in &new {
        if !old.contains(id) {
            c.execute(
                "INSERT INTO merge_request_assignees (merge_request_id, user_id, created_at) VALUES (?1, ?2, ?3)",
                params![mr_id, id, time::now()],
            )?;
        }
    }
    Ok(())
}

fn set_reviewers(c: &Connection, mr_id: i64, ids: &[i64], _ee: bool) -> Result<()> {
    let old: Vec<i64> = {
        let mut st = c.prepare("SELECT user_id FROM merge_request_reviewers WHERE merge_request_id = ?1")?;
        st.query_map([mr_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    // `handle_reviewers_change`: reviewers added or removed, and the assignees.
    let affected: Vec<i64> = old.iter().filter(|i| !ids.contains(i)).chain(ids.iter().filter(|i| !old.contains(i))).copied().collect();
    if !affected.is_empty() {
        let mut users = affected;
        let mut st = c.prepare("SELECT user_id FROM merge_request_assignees WHERE merge_request_id = ?1")?;
        users.extend(st.query_map([mr_id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
        crate::services::user_counts::invalidate(c, &users)?;
    }
    let list = if ids.is_empty() {
        "-1".to_string()
    } else {
        ids.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    c.execute(&format!("DELETE FROM merge_request_reviewers WHERE merge_request_id = ?1 AND user_id NOT IN ({list})"), [mr_id])?;
    // The reviewers kept stay in their order and new ones follow (EE answers in no fixed
    // order; the replays compare its reviewer lists as sets).
    for id in ids {
        c.execute(
            // `set_reviewers_approved`: a reviewer who already approved comes back `approved`.
            "INSERT OR IGNORE INTO merge_request_reviewers (merge_request_id, user_id, state, created_at)
             VALUES (?1, ?2, CASE
                 WHEN EXISTS (SELECT 1 FROM approvals WHERE merge_request_id = ?1 AND user_id = ?2) THEN 3
                 -- EE: an outstanding change request comes back with the reviewer (only EE
                 -- fills this table).
                 WHEN EXISTS (SELECT 1 FROM merge_request_requested_changes WHERE merge_request_id = ?1 AND user_id = ?2) THEN 2
                 ELSE 0 END, ?3)",
            params![mr_id, id, time::now()],
        )?;
    }
    Ok(())
}

/// Computes the new label set: `(labels || current) | add - remove`.
fn resolve_labels(
    c: &Connection,
    p: &Project,
    mr_id: Option<i64>,
    prm: &MrParams,
    can_create: bool,
) -> Result<Option<Vec<i64>>> {
    if prm.labels.is_none() && prm.add_labels.is_none() && prm.remove_labels.is_none() {
        return Ok(None);
    }
    let current: Vec<i64> = match mr_id {
        Some(id) => labels::for_target(c, "MergeRequest", id)?
            .into_iter()
            .map(|l| l.id)
            .collect(),
        None => vec![],
    };
    let mut set: Vec<i64> = match &prm.labels {
        Some(titles) => labels::find_or_create_by_titles(c, p, titles, can_create)?
            .into_iter()
            .map(|l| l.id)
            .collect(),
        None => current.clone(),
    };
    if let Some(add) = &prm.add_labels {
        for l in labels::find_or_create_by_titles(c, p, add, can_create)? {
            if !set.contains(&l.id) {
                set.push(l.id);
            }
        }
    }
    if let Some(remove) = &prm.remove_labels {
        let ids: Vec<i64> = labels::find_by_titles(c, p, remove)?
            .into_iter()
            .map(|l| l.id)
            .collect();
        set.retain(|i| !ids.contains(i));
    }
    Ok(Some(set))
}

fn label_events(c: &Connection, mr_id: i64, user: &User, old: &[i64], new: &[i64]) -> Result<()> {
    let now = time::now();
    let mut added: Vec<i64> = new.iter().filter(|i| !old.contains(i)).copied().collect();
    let mut removed: Vec<i64> = old.iter().filter(|i| !new.contains(i)).copied().collect();
    added.sort();
    removed.sort();
    for id in added {
        c.execute(
            "INSERT INTO resource_label_events (merge_request_id, user_id, label_id, action, created_at) VALUES (?1, ?2, ?3, 'add', ?4)",
            params![mr_id, user.id, id, now],
        )?;
    }
    for id in removed {
        c.execute(
            "INSERT INTO resource_label_events (merge_request_id, user_id, label_id, action, created_at) VALUES (?1, ?2, ?3, 'remove', ?4)",
            params![mr_id, user.id, id, now],
        )?;
    }
    Ok(())
}

fn milestone_event(
    c: &Connection,
    mr: &MergeRequest,
    user: &User,
    old: Option<i64>,
    new: Option<i64>,
) -> Result<()> {
    let (action, milestone) = match new {
        Some(m) => ("add", Some(m)),
        None => ("remove", old),
    };
    c.execute(
        "INSERT INTO resource_milestone_events (merge_request_id, user_id, milestone_id, action, state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![mr.id, user.id, milestone, action, mr.state, time::now()],
    )?;
    Ok(())
}

fn valid_milestone(c: &Connection, p: &Project, id: Option<i64>) -> Result<Option<Option<i64>>> {
    match id {
        None | Some(0) => Ok(Some(None)),
        Some(m) => {
            let groups = crate::model::Namespace::self_and_ancestor_ids(c, p.namespace_id)?;
            let list = groups
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let ok: Option<i64> = c
                .query_row(
                    &format!("SELECT id FROM milestones WHERE id = ?1 AND (project_id = ?2 OR group_id IN ({list}))"),
                    params![m, p.id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(ok.map(Some))
        }
    }
}

/// `MergeRequests::CreateService` for a merge request within one project.
pub fn create(
    c: &Connection,
    _config: &Config,
    p: &Project,
    user: &User,
    prm: MrParams,
) -> Result<MergeRequest> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !perms.create_merge_request_in(p) {
        return Err(Error::forbidden());
    }
    let title = prm.title.clone().unwrap_or_default().trim().to_string();
    let source = prm.source_branch.clone().unwrap_or_default();
    let target = prm.target_branch.clone().unwrap_or_default();
    if p.feature_level("merge_requests") == crate::model::feature::DISABLED {
        return Err(Error::Api(
            422,
            json!(["Target project has disabled merge requests"]),
        ));
    }
    if !source.is_empty() && source == target {
        return Err(Error::Api(
            422,
            json!(["You can't use same project/branch for source and target"]),
        ));
    }
    let mut errs: Vec<(String, String)> = vec![];
    if title.trim().is_empty() {
        errs.push(("title".into(), "can't be blank".into()));
    } else if title.chars().count() > 255 {
        errs.push((
            "title".into(),
            "is too long (maximum is 255 characters)".into(),
        ));
    }
    // GitLab 18.11 doesn't validate that the branches exist (the flag is off by default).
    errs.extend(branch_errors(c, p, &source, &target, false)?);
    if let Some(Some(d)) = &prm.description
        && d.len() > 1_048_576 {
            errs.push((
                "description".into(),
                "is too long (maximum is 1048576 characters)".into(),
            ));
        }
    // `handle_merge_request_errors!`: `validate_branches` errors (409) win over the others.
    if let Some(iid) = duplicate_open(c, p, &source, &target, None)? {
            return Err(Error::Api(
                409,
                json!([format!(
                    "Another open merge request already exists for this source branch: !{iid}"
                )]),
            ));
        }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }
    let admin = can_admin_mr(c, user, p)?;
    let squash = prm
        .squash
        .unwrap_or(matches!(p.squash_option.as_str(), "always" | "default_on"));
    let mut merge_params = Map::new();
    merge_params.insert(
        "force_remove_source_branch".into(),
        Value::Bool(
            prm.remove_source_branch
                .unwrap_or(p.remove_source_branch_after_merge),
        ),
    );
    let milestone = if admin {
        valid_milestone(c, p, prm.milestone_id.flatten())?.flatten()
    } else {
        None
    };
    let now = time::now();
    let iid = next_mr_iid(c, p.id)?;
    c.execute(
        &format!(
            "INSERT INTO merge_requests (iid, target_project_id, source_project_id, source_branch, target_branch, title, description,
               author_id, state, merge_status, squash, merge_params, milestone_id, created_at, updated_at, discussion_locked)
             VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, 'opened', 'preparing', ?8, ?9, ?10, ?11, ?11, {})",
            if admin && prm.discussion_locked.is_some() { if prm.discussion_locked == Some(true) { "1" } else { "0" } } else { "NULL" }
        ),
        params![
            iid,
            p.id,
            source,
            target,
            title,
            prm.description.clone().flatten(),
            user.id,
            squash,
            Value::Object(merge_params).to_string(),
            milestone,
            now
        ],
    )?;
    let id = c.last_insert_rowid();
    if let Some(Some(t)) = prm.merge_after {
        c.execute("INSERT INTO merge_request_merge_schedules (merge_request_id, merge_after) VALUES (?1, ?2)", params![id, t])?;
    }
    if admin {
        if let Some(ids) = &prm.assignee_ids
            && let Some(list) = filter_users(c, _config, p, ids)? {
                set_assignees(c, id, &list)?;
            }
        if let Some(ids) = &prm.reviewer_ids
            && let Some(list) = filter_users(c, _config, p, ids)? {
                set_reviewers(c, id, &list, _config.is_ee())?;
            }
    }
    let can_create_labels = ProjectPerms::load(c, Some(user), p)?.reporter();
    if admin
        && let Some(set) = resolve_labels(c, p, None, &prm, can_create_labels)? {
            labels::set_for_target(c, "MergeRequest", id, &set)?;
            label_events(c, id, user, &[], &set)?;
        }
    let mr = MergeRequest::get(c, id)?;
    if _config.is_ee() {
        // EE copies the project's approval rules onto the new MR, then syncs its approvals.
        crate::services::approvals::copy_project_rules(c, p, &mr)?;
        crate::services::approvals::start_sync(c, _config, &mr)?;
    }
    if milestone.is_some() {
        milestone_event(c, &mr, user, None, milestone)?;
    }
    // `handle_changes` on create: assignee note (async worker), reviewer note (inline).
    let assignees = mr.assignee_ids(c)?;
    if !assignees.is_empty() {
        jobs::enqueue(
            c,
            "assignee_note",
            json!({"merge_request_id": id, "user_id": user.id, "old": [], "new": assignees}),
        )?;
    }
    let reviewers = mr.reviewer_ids(c)?;
    if !reviewers.is_empty() {
        reviewer_note(c, p, &mr, user, &[], &reviewers)?;
        crate::services::todos::review_requested(c, p, &mr, user, &[], &reviewers)?;
    }
    jobs::enqueue(
        c,
        "mr_after_create",
        json!({"merge_request_id": id, "user_id": user.id}),
    )?;
    crate::services::subscriptions::ensure_participant(c, user.id, "MergeRequest", id)?;
    crate::services::user_counts::invalidate_all(c, &mr)?;
    Ok(mr)
}

/// `MergeRequests::AfterCreateService`: diff, refs, mergeability, webhooks.
pub fn after_create(c: &Connection, config: &Config, mr_id: i64, user_id: i64) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else {
        return Ok(());
    };
    let p = Project::find(c, mr.target_project_id)?.ok_or(Error::not_found("Project"))?;
    let before = snapshot(c, &mr)?;
    diffs::ensure_version(c, &p, &mr)?;
    // `create_pipeline_for`: the MR pipeline exists (as head pipeline) when the open hook runs;
    // its hook data is built now, while the MR is still preparing, and delivered after.
    let mr_pipeline = match User::find(c, user_id)? {
        Some(u) => match crate::services::ci::create_skipped_mr_pipeline(c, &p, &MergeRequest::get(c, mr_id)?, &u)? {
            Some(pl) => crate::services::hooks::pipeline_event_data(c, config, &p, &pl)?,
            None => None,
        },
        None => None,
    };
    // Fake CI: the MR's pipeline. On the EE reference the open hook already names it as the
    // head pipeline.
    if crate::services::fake_ci::config(c, p.id)?.is_some() {
        // A merged results pipeline runs on the merge ref, so the MR is checked first.
        if config.is_ee() && crate::services::merge_trains::merge_pipelines_enabled(&p) && mr.merge_status == "preparing" {
            c.execute("UPDATE merge_requests SET merge_status = 'unchecked' WHERE id = ?1", [mr_id])?;
            mergeability::check(c, config, mr_id)?;
            // That takes long enough for the approvals sync begun at creation to finish first.
            c.execute("DELETE FROM jobs WHERE kind = 'approvals_sync' AND json_extract(payload, '$.merge_request_id') = ?1", [mr_id])?;
            crate::services::approvals::finish_sync(c, mr_id)?;
        }
        crate::services::fake_ci::create_mr_pipeline(c, config, mr_id, user_id)?;
    }
    let mr = MergeRequest::get(c, mr_id)?;
    if mr.merge_status == "preparing" {
        c.execute(
            "UPDATE merge_requests SET merge_status = 'unchecked' WHERE id = ?1",
            [mr_id],
        )?;
        mergeability::check_async(c, &MergeRequest::get(c, mr_id)?)?;
    }
    c.execute(
        "UPDATE merge_requests SET prepared_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![time::now(), mr_id],
    )?;
    let mr = MergeRequest::get(c, mr_id)?;
    crate::services::ci::update_head_pipeline(c, &p, &mr)?;
    let user = User::find(c, user_id)?;

    if let Some(u) = &user {
        crate::services::events::record(c, Some(p.id), u.id, "created", Some("MergeRequest"), Some(mr.id), None)?;
        crate::services::todos::new_merge_request(c, &p, &mr, u)?;
        // `AfterCreateService`: `create_cross_references!`.
        crate::services::cross_refs::mr_mentions(c, config, &p, u, &mr, &crate::services::todos::mentionable_text(&mr), None)?;
    }
    // EE: an MR whose creation failed after it was saved (`approvals_before_merge` clashing
    // with a copied any-approver rule) gets no open hook.
    let failed: bool = c.query_row("SELECT creation_failed FROM merge_requests WHERE id = ?1", [mr.id], |r| r.get::<_, i64>(0))? != 0;
    if !failed {
        crate::services::hooks::merge_request_event(c, config, &p, &mr, user.as_ref(), "open", Some(&before))?;
    }
    // The pipeline's own hook is delivered after the open hook.
    if let Some(data) = mr_pipeline {
        crate::services::hooks::fire_pipeline_event(c, &p, &data)?;
    }
    Ok(())
}

/// The state of an MR before an update, for system notes and webhook `changes`.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub mr: MergeRequest,
    pub assignees: Vec<i64>,
    pub reviewers: Vec<i64>,
    pub labels: Vec<i64>,
    /// `(user_id, state)` of each reviewer, for `changes.reviewers`.
    pub reviewer_states: Vec<(i64, i64)>,
    /// The reviewer whose review was re-requested (`re_requested: true`).
    pub rerequested: Option<i64>,
    /// `updated_at` after the note carrying the commands touched the MR (`mr` has the value
    /// from before).
    pub note_touch: Option<i64>,
    /// `time_change` (previous, current) for time tracking hooks.
    pub time_change: Option<(i64, i64)>,
    /// `AddSpentTimeService`: only `total_time_spent` was captured before the change.
    pub spend: bool,
}

pub fn snapshot(c: &Connection, mr: &MergeRequest) -> Result<Snapshot> {
    Ok(Snapshot {
        mr: mr.clone(),
        assignees: mr.assignee_ids(c)?,
        reviewers: mr.reviewer_ids(c)?,
        labels: mr.labels(c)?.into_iter().map(|l| l.id).collect(),
        reviewer_states: mr.reviewers(c)?.into_iter().map(|(_, u, s, _)| (u, s)).collect(),
        rerequested: None,
        note_touch: None,
        time_change: None,
        spend: false,
    })
}

/// `MergeRequests::UpdateService` (plus close/reopen via `state_event`).
/// The `state_event` part of an update: close or reopen. A reopen that would conflict with
/// another open MR, or one blocked by a locked discussion, silently does nothing.
pub fn apply_state_event(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    event: Option<&str>,
) -> Result<()> {
    match event {
        Some("close") => {
            close(c, config, p, mr, user)?;
        }
        Some("reopen") => {
            let perms = ProjectPerms::load(c, Some(user), p)?;
            let can_reopen = (perms.developer()
                || mr.author_id == user.id
                || mr.assignee_ids(c)?.contains(&user.id))
                && mr.discussion_locked != Some(true);
            if can_reopen
                && mr.state == "closed"
                && duplicate_open(c, p, &mr.source_branch, &mr.target_branch, Some(mr.id))?.is_none()
            {
                reopen(c, config, p, mr, user)?;
            } else if config.is_ee() && can_reopen && mr.state == "opened" {
                // EE drops the approvals on a reopen request even when the MR is already open
                // (recorded on EE 18.11: `approval_persistence`, "bob reopen").
                c.execute("DELETE FROM approvals WHERE merge_request_id = ?1", [mr.id])?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn update(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    prm: MrParams,
) -> Result<MergeRequest> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    let admin = perms.developer();
    let mut before = snapshot(c, mr)?;
    let stored = MergeRequest::get(c, mr.id)?.updated_at;
    if stored != mr.updated_at {
        before.note_touch = Some(stored);
    }
    // Time-tracking notes come after the title, description and lock notes
    // (`CommonSystemNotesService#handle_time_tracking_note`: estimate, then time spent).
    let mut spend_note: Option<String> = None;
    let mut estimate_note: Option<String> = None;
    // `spend_time`: a timelog and a system note; the hook shows `time_change`.
    if let Some((secs, _)) = prm.spend
        && let Some(errs) = spend_errors(mr, secs)
    {
        return Err(Error::Validation(errs));
    }
    // `time_estimate` must not be negative; positive ones are capped at `MAX_INT_VALUE`.
    if prm.time_estimate.is_some_and(|t| t < 0) {
        return Err(Error::Validation(vec![("time_estimate".into(), "must have a valid format and be greater than or equal to zero.".into())]));
    }
    let mut prm = prm;
    prm.time_estimate = prm.time_estimate.map(|t| t.min(i32::MAX as i64));
    if let Some((secs, note_id)) = prm.spend {
        before.spend = true;
        before.time_change = Some((0, secs));
        let now = time::now();
        c.execute("UPDATE merge_requests SET total_time_spent = max(0, total_time_spent + ?1) WHERE id = ?2", params![secs, mr.id])?;
        c.execute(
            "INSERT INTO timelogs (merge_request_id, user_id, time_spent, summary, spent_at, created_at, note_id) VALUES (?1, ?2, ?3, NULL, ?4, ?4, ?5)",
            params![mr.id, user.id, secs, now, note_id],
        )?;
        let body = if secs >= 0 {
            format!("added {} of time spent at {}", human_duration(secs), time::ruby_utc(now))
        } else {
            format!("subtracted {} of time spent at {}", human_duration(secs), time::ruby_utc(now))
        };
        spend_note = Some(body);
    }

    // State changes run first (and are committed on their own by GitLab, see `apply_state_event`).
    apply_state_event(c, config, p, mr, user, prm.state_event.as_deref())?;
    let mr = mr.reload(c)?;

    let mut errs: Vec<(String, String)> = vec![];
    if let Some(t) = &prm.title {
        prm.title = Some(t.trim().to_string());
    }
    if let Some(t) = &prm.title {
        if t.trim().is_empty() {
            errs.push(("title".into(), "can't be blank".into()));
        } else if t.chars().count() > 255 {
            errs.push((
                "title".into(),
                "is too long (maximum is 255 characters)".into(),
            ));
        }
    }
    // `closed_or_merged_without_fork?` only drops the target branch when the source project is
    // gone (forks), so a closed or merged MR can still be retargeted.
    let new_target = prm.target_branch.clone();
    if let Some(t) = &new_target {
        if !repo::valid_branch_name(t) {
            errs.push(("target_branch".into(), "is invalid".into()));
        } else if t == &mr.source_branch {
            return Err(Error::Api(
                422,
                json!(["You can't use same project/branch for source and target"]),
            ));
        } else if t != &mr.target_branch && mr.is_open()
            && let Some(iid) = duplicate_open(c, p, &mr.source_branch, t, Some(mr.id))? {
                return Err(Error::Api(
                    409,
                    json!([format!(
                        "Another open merge request already exists for this source branch: !{iid}"
                    )]),
                ));
            }
    }
    if !errs.is_empty() {
        return Err(Error::Validation(errs));
    }

    let now = time::now();
    let mut changed = false;
    if let Some(t) = &prm.title
        && *t != mr.title {
            c.execute(
                "UPDATE merge_requests SET title = ?1 WHERE id = ?2",
                params![t, mr.id],
            )?;
            changed = true;
        }
    let mut description_changed = false;
    if let Some(d) = &prm.description
        && *d != mr.description {
            c.execute(
                "UPDATE merge_requests SET description = ?1, last_edited_at = ?2, last_edited_by_id = ?3 WHERE id = ?4",
                params![d, now, user.id, mr.id],
            )?;
            changed = true;
            description_changed = true;
        }
    let mut target_changed = None;
    if let Some(t) = &new_target
        && *t != mr.target_branch {
            c.execute(
                "UPDATE merge_requests SET target_branch = ?1 WHERE id = ?2",
                params![t, mr.id],
            )?;
            target_changed = Some((mr.target_branch.clone(), t.clone()));
            changed = true;
        }
    if let Some(at) = prm.merge_after {
        set_merge_after(c, &mr, at)?;
    }
    if let Some(s) = prm.squash
        && s != mr.squash {
            c.execute(
                "UPDATE merge_requests SET squash = ?1 WHERE id = ?2",
                params![s, mr.id],
            )?;
            changed = true;
        }
    // Like the target branch, only dropped for forks whose source project is gone.
    if let Some(r) = prm.remove_source_branch {
            let mut mp = mr.merge_params.clone();
            let v = Value::Bool(r);
            if mp.get("force_remove_source_branch") != Some(&v) {
                mp.insert("force_remove_source_branch".into(), v);
                c.execute(
                    "UPDATE merge_requests SET merge_params = ?1 WHERE id = ?2",
                    params![Value::Object(mp).to_string(), mr.id],
                )?;
                changed = true;
            }
        }
    let mut lock_changed = false;
    if let (Some(l), true) = (prm.discussion_locked, admin)
        && mr.discussion_locked != Some(l) {
            c.execute(
                "UPDATE merge_requests SET discussion_locked = ?1 WHERE id = ?2",
                params![l, mr.id],
            )?;
            changed = true;
            lock_changed = true;
        }
    let mut milestone_changed = None;
    if let (Some(m), true) = (prm.milestone_id, admin)
        && let Some(valid) = valid_milestone(c, p, m)?
            && valid != mr.milestone_id {
                c.execute(
                    "UPDATE merge_requests SET milestone_id = ?1 WHERE id = ?2",
                    params![valid, mr.id],
                )?;
                milestone_changed = Some((mr.milestone_id, valid));
                changed = true;
            }
    // `time_estimate`: a column change with its time-tracking system note.
    if let Some(t) = prm.time_estimate
        && t != mr.time_estimate
    {
        c.execute("UPDATE merge_requests SET time_estimate = ?1 WHERE id = ?2", params![t, mr.id])?;
        let body = if mr.time_estimate == 0 {
            format!("added time estimate of {}", estimate_text(t))
        } else if t == 0 {
            format!("removed time estimate of {}", estimate_text(mr.time_estimate))
        } else {
            format!("changed time estimate to {} from {}", estimate_text(t), estimate_text(mr.time_estimate))
        };
        estimate_note = Some(body);
        changed = true;
    }
    let mut new_labels = None;
    if admin
        && let Some(set) = resolve_labels(c, p, Some(mr.id), &prm, perms.reporter())? {
            let mut a = set.clone();
            a.sort();
            let mut b = before.labels.clone();
            b.sort();
            if a != b {
                // `Callbacks::Labels` touches the MR (throttled to once a minute).
                notes::touch_noteable(c, "MergeRequest", mr.id, now)?;
                labels::set_for_target(c, "MergeRequest", mr.id, &set)?;
                new_labels = Some(set);
            }
        }
    let mut new_assignees = None;
    if admin
        && let Some(ids) = &prm.assignee_ids
            && let Some(list) = filter_users(c, config, p, ids)?
                && list != before.assignees {
                    // `MergeRequestAssignee` touches the MR (throttled).
                    notes::touch_noteable(c, "MergeRequest", mr.id, now)?;
                    set_assignees(c, mr.id, &list)?;
                    new_assignees = Some(list);
                }
    let mut new_reviewers = None;
    if admin
        && let Some(ids) = &prm.reviewer_ids
            && let Some(list) = filter_users(c, config, p, ids)?
                && list != before.reviewers {
                    set_reviewers(c, mr.id, &list, config.is_ee())?;
                    new_reviewers = Some(list);
                }
    // `update_timestamp?` is true unless only `relative_position` changed, so every save
    // through `UpdateService` records the user; Rails then writes (and touches `updated_at`)
    // only when something changed, the updater included.
    // Labels, assignees and reviewers are associations: they don't change a column, so a change
    // by the user who last updated the MR leaves `updated_at` alone.
    if changed || mr.updated_by_id != Some(user.id) {
        c.execute(
            "UPDATE merge_requests SET updated_at = ?1, updated_by_id = ?2 WHERE id = ?3",
            params![now, user.id, mr.id],
        )?;
    }
    let after = mr.reload(c)?;

    // System notes, in GitLab's order: common notes, then assignees/reviewers, then branch.
    if let Some(t) = &prm.title
        && *t != before.mr.title {
            let (old_d, new_d) = (is_draft_title(&before.mr.title), is_draft_title(t));
            if old_d != new_d {
                let body = if new_d {
                    "marked this merge request as **draft**"
                } else {
                    "marked this merge request as **ready**"
                };
                notes::system_note(c, mr.id, p.id, user, body, "title")?;
                // EE: a draft leaves its merge train.
                if new_d && config.is_ee() {
                    crate::services::merge_trains::abort_for_mr(c, config, &after, user, &format!(
                        "the merge request is marked as draft. [Learn more]({}).",
                        config.url("/help/ci/pipelines/merge_trains.md#merge-request-dropped-from-the-merge-train")
                    ), false)?;
                }
            }
            if draftless_title(&before.mr.title) != draftless_title(t) {
                notes::system_note(
                    c,
                    mr.id,
                    p.id,
                    user,
                    &notes::title_change_body(&before.mr.title, t),
                    "title",
                )?;
            }
        }
    if description_changed {
        notes::system_note(
            c,
            mr.id,
            p.id,
            user,
            "changed the description",
            "description",
        )?;
    }
    if lock_changed {
        let body = if after.discussion_locked == Some(true) {
            "locked the discussion in this merge request"
        } else {
            "unlocked the discussion in this merge request"
        };
        notes::system_note(
            c,
            mr.id,
            p.id,
            user,
            body,
            if after.discussion_locked == Some(true) {
                "locked"
            } else {
                "unlocked"
            },
        )?;
    }
    for body in [estimate_note, spend_note].into_iter().flatten() {
        notes::system_note(c, mr.id, p.id, user, &body, "time_tracking")?;
    }
    if let Some((old, new)) = milestone_changed {
        milestone_event(c, &after, user, old, new)?;
    }
    if let Some(set) = &new_labels {
        label_events(c, mr.id, user, &before.labels, set)?;
    }
    if let Some(list) = &new_assignees {
        // `HandleAssigneesChangeWorker` runs asynchronously.
        jobs::enqueue(
            c,
            "assignee_note",
            json!({"merge_request_id": mr.id, "user_id": user.id, "old": before.assignees, "new": list}),
        )?;
    }
    if let Some(list) = &new_reviewers {
        reviewer_note(c, p, &after, user, &before.reviewers, list)?;
        crate::services::todos::review_requested(c, p, &after, user, &before.reviewers, list)?;
    }
    // `update_issuable`: mentions added to the title or description, skipping earlier ones.
    if after.title != mr.title || after.description != mr.description {
        let old = crate::services::mentions::mentioned_user_ids(c, p, &crate::services::todos::mentionable_text(&mr), user)?;
        crate::services::mentions::text_todos(c, p, &after, user, &crate::services::todos::mentionable_text(&after), None, &old)?;
        // `create_new_cross_references!`: only references the old text didn't have.
        let before_text = crate::services::todos::mentionable_text(&mr);
        crate::services::cross_refs::mr_mentions(c, config, p, user, &after, &crate::services::todos::mentionable_text(&after), Some(&before_text))?;
    }
    // `MergeRequests::ResolveTodosService` (asynchronous; usually after the mention to-dos above,
    // but GitLab races here, see PLAN.md):
    // changing the title, description, milestone, target branch, labels, assignees or
    // reviewers resolves the updater's to-dos on the MR.
    {
        let labels_now: Vec<i64> = after.labels(c)?.into_iter().map(|l| l.id).collect();
        let (mut a, mut b) = (labels_now, before.labels.clone());
        a.sort();
        b.sort();
        if a != b
            || after.assignee_ids(c)? != before.assignees
            || after.reviewer_ids(c)? != before.reviewers
            || after.title != mr.title
            || after.description != mr.description
            || after.milestone_id != mr.milestone_id
            || after.target_branch != mr.target_branch
        {
            crate::services::todos::resolve_for_target(c, &after, Some(user.id))?;
        }
    }
    if let Some((old, new)) = &target_changed {
        notes::system_note(
            c,
            mr.id,
            p.id,
            user,
            &format!("changed target branch from `{old}` to `{new}`"),
            "branch",
        )?;
        // `reload_diff_if_branch_changed` needs both branch heads.
        let r = repo::repo(c, p);
        if r.branch(&after.target_branch)?.is_some() && r.branch(&after.source_branch)?.is_some() {
            diffs::create_version(c, p, &after)?;
        }
        mergeability::mark_as_unchecked(c, mr.id)?;
        merge::abort_auto_merge(c, config, p, &after, user, "target branch was changed")?;
    }
    let after = mr.reload(c)?;
    // `ProcessAutoMergeFromEventWorker` on `DraftStateChangeEvent`: marking an MR ready lets an
    // armed auto-merge go ahead.
    if before.mr.is_draft() && !after.is_draft() && after.merge_when_pipeline_succeeds && after.is_open() {
        jobs::enqueue(c, "mr_auto_merge", json!({"merge_request_id": after.id}))?;
    }
    // `IssuableBaseService#update` fires `update` for every successful update call, even
    // without changes, and also after a `state_event` close/reopen.
    let _ = (changed, &new_assignees, &new_reviewers);
    {
        crate::services::hooks::merge_request_event(
            c,
            config,
            p,
            &after,
            Some(user),
            "update",
            Some(&before),
        )?;
    }
    Ok(after)
}

/// `HandleAssigneesChangeService`: the assignee system note and assignment to-dos.
pub fn assignee_change_job(
    c: &Connection,
    mr_id: i64,
    user_id: i64,
    old: &[i64],
    new: &[i64],
) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else {
        return Ok(());
    };
    let p = Project::find(c, mr.target_project_id)?.ok_or(Error::not_found("Project"))?;
    let user = User::get(c, user_id)?;
    assignee_note(c, &p, &mr, &user, old, new)?;
    crate::services::todos::assigned(c, &p, &mr, &user, old, new)
}

pub fn assignee_note(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    old: &[i64],
    new: &[i64],
) -> Result<()> {
    let added: Vec<String> = refs(
        c,
        &new.iter()
            .filter(|i| !old.contains(i))
            .copied()
            .collect::<Vec<_>>(),
    )?;
    let removed: Vec<String> = refs(
        c,
        &old.iter()
            .filter(|i| !new.contains(i))
            .copied()
            .collect::<Vec<_>>(),
    )?;
    let mut parts = vec![];
    if !added.is_empty() {
        parts.push(format!("assigned to {}", notes::to_sentence(&added)));
    }
    if !removed.is_empty() {
        parts.push(format!("unassigned {}", notes::to_sentence(&removed)));
    }
    if !parts.is_empty() {
        notes::system_note(c, mr.id, p.id, user, &parts.join(" and "), "assignee")?;
    }
    Ok(())
}

pub fn reviewer_note(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    old: &[i64],
    new: &[i64],
) -> Result<()> {
    let added: Vec<String> = refs(
        c,
        &new.iter()
            .filter(|i| !old.contains(i))
            .copied()
            .collect::<Vec<_>>(),
    )?;
    let removed: Vec<String> = refs(
        c,
        &old.iter()
            .filter(|i| !new.contains(i))
            .copied()
            .collect::<Vec<_>>(),
    )?;
    let mut parts = vec![];
    if !added.is_empty() {
        parts.push(format!(
            "requested review from {}",
            notes::to_sentence(&added)
        ));
    }
    if !removed.is_empty() {
        parts.push(format!(
            "removed review request for {}",
            notes::to_sentence(&removed)
        ));
    }
    if !parts.is_empty() {
        notes::system_note(c, mr.id, p.id, user, &parts.join(" and "), "reviewer")?;
    }
    Ok(())
}

fn refs(c: &Connection, ids: &[i64]) -> Result<Vec<String>> {
    let mut out = vec![];
    for id in ids {
        if let Some(u) = User::find(c, *id)? {
            out.push(format!("@{}", u.username));
        }
    }
    Ok(out)
}

pub fn state_event(
    c: &Connection,
    mr_id: i64,
    user: Option<&User>,
    state: &str,
    source_commit: Option<&str>,
) -> Result<()> {
    c.execute(
        "INSERT INTO resource_state_events (merge_request_id, user_id, state, source_commit, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![mr_id, user.map(|u| u.id), state, source_commit, time::now()],
    )?;
    Ok(())
}

/// `MergeRequests::CloseService`.
pub fn close(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<bool> {
    if mr.state != "opened" {
        return Ok(false);
    }
    let before = snapshot(c, mr)?;
    let now = time::now();
    c.execute(
        // `CloseService` leaves `updated_by` to the update that follows.
        "UPDATE merge_requests SET state = 'closed', merge_error = NULL, latest_closed_by_id = ?1, latest_closed_at = ?2, updated_at = ?2
         WHERE id = ?3",
        params![user.id, now, mr.id],
    )?;
    state_event(c, mr.id, Some(user), "closed", None)?;
    crate::services::events::record(c, Some(p.id), user.id, "closed", Some("MergeRequest"), Some(mr.id), None)?;
    let mr = mr.reload(c)?;
    merge::abort_auto_merge(c, config, p, &mr, user, "merge request was closed")?;
    crate::services::todos::resolve_for_target(c, &mr, Some(user.id))?;
    crate::services::user_counts::invalidate_all(c, &mr)?;
    crate::services::hooks::merge_request_event(c, config, p, &mr, Some(user), "close", Some(&before))?;
    Ok(true)
}

/// `MergeRequests::ReopenService`.
pub fn reopen(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<bool> {
    if mr.state != "closed" {
        return Ok(false);
    }
    let before = snapshot(c, mr)?;
    let now = time::now();
    c.execute(
        "UPDATE merge_requests SET state = 'opened', merge_error = NULL, latest_closed_by_id = NULL, latest_closed_at = NULL,
            merge_ref_sha = NULL, updated_at = ?1 WHERE id = ?2",
        params![now, mr.id],
    )?;
    state_event(c, mr.id, Some(user), "reopened", None)?;
    crate::services::events::record(c, Some(p.id), user.id, "reopened", Some("MergeRequest"), Some(mr.id), None)?;
    let mr = mr.reload(c)?;
    // The hook fires before the diff reload and the mergeability reset.
    crate::services::hooks::merge_request_event(c, config, p, &mr, Some(user), "reopen", Some(&before))?;
    let r = repo::repo(c, p);
    if r.branch(&mr.source_branch)?.is_some() && r.branch(&mr.target_branch)?.is_some() {
        diffs::create_version(c, p, &mr)?;
    }
    mergeability::mark_as_unchecked(c, mr.id)?;
    // EE drops the approvals of a reopened MR (no system note; recorded on EE 18.11).
    if config.is_ee() {
        c.execute("DELETE FROM approvals WHERE merge_request_id = ?1", [mr.id])?;
    }
    crate::services::ci::update_head_pipeline(c, p, &mr)?;
    crate::services::user_counts::invalidate_all(c, &mr.reload(c)?)?;
    Ok(true)
}

/// `Issuable::DestroyService`.
pub fn delete(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<()> {
    let r = repo::repo(c, p);
    for suffix in ["head", "merge", "train", "rebase_on_merge"] {
        r.delete_ref(&format!("refs/merge-requests/{}/{suffix}", mr.iid))?;
    }
    c.execute(
        "DELETE FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1",
        [mr.id],
    )?;
    c.execute(
        "DELETE FROM label_links WHERE target_type = 'MergeRequest' AND target_id = ?1",
        [mr.id],
    )?;
    c.execute(
        "DELETE FROM award_emoji WHERE awardable_type = 'MergeRequest' AND awardable_id = ?1",
        [mr.id],
    )?;
    c.execute(
        "DELETE FROM todos WHERE target_type = 'MergeRequest' AND target_id = ?1",
        [mr.id],
    )?;
    c.execute("DELETE FROM merge_requests WHERE id = ?1", [mr.id])?;
    Ok(())
}

/// Users who may merge into the target branch (`can_be_merged_by?`).
pub fn can_be_merged_by(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<bool> {
    repo::can_merge_to_branch(c, user, p, &mr.target_branch)
}

/// The reporter level is enough to see MRs; used by callers deciding on recheck.
pub fn can_update(c: &Connection, p: &Project, mr: &MergeRequest, user: &User) -> Result<bool> {
    // Archived projects are read-only.
    if p.archived {
        return Ok(false);
    }
    let perms = ProjectPerms::load(c, Some(user), p)?;
    Ok(perms.level >= access::DEVELOPER
        || perms.admin
        || mr.author_id == user.id
        || mr.assignee_ids(c)?.contains(&user.id))
}

/// `merge_schedule&.merge_after`.
pub fn merge_after(c: &Connection, mr: &MergeRequest) -> Result<Option<i64>> {
    c
        .query_row(
            "SELECT merge_after FROM merge_request_merge_schedules WHERE merge_request_id = ?1",
            [mr.id],
            |r| r.get(0),
        )
        .optional()
}

/// `UpdateMergeScheduleService`.
pub fn set_merge_after(c: &Connection, mr: &MergeRequest, at: Option<i64>) -> Result<()> {
    match at {
        Some(t) => c.execute(
            "INSERT INTO merge_request_merge_schedules (merge_request_id, merge_after) VALUES (?1, ?2)
             ON CONFLICT (merge_request_id) DO UPDATE SET merge_after = excluded.merge_after",
            params![mr.id, t],
        )?,
        None => c.execute("DELETE FROM merge_request_merge_schedules WHERE merge_request_id = ?1", [mr.id])?,
    };
    Ok(())
}

/// Open MRs with auto-merge set whose schedule is due (`ProcessScheduledMergeWorker`).
pub fn due_scheduled_merges(c: &Connection, now: i64) -> Result<Vec<i64>> {
    let mut st = c.prepare(
        "SELECT s.merge_request_id FROM merge_request_merge_schedules s JOIN merge_requests m ON m.id = s.merge_request_id
         WHERE s.merge_after <= ?1 AND m.merge_when_pipeline_succeeds = 1 AND m.state = 'opened'
         ORDER BY s.merge_after, s.merge_request_id",
    )?;
    let ids = st.query_map([now], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(ids)
}

/// GitLab's `Gitlab::TimeTrackingFormatter` output for system notes (`1h 30m`).
pub fn human_duration(secs: i64) -> String {
    crate::chronic::human(secs)
}

/// Durations in time-tracking notes: weeks of 5 days of 8 hours, e.g. `1w 2d 3h`.
pub fn estimate_text(secs: i64) -> String {
    crate::chronic::human(secs)
}

/// `Timelog::MAX_TOTAL_TIME_SPENT` (4 years).
pub const MAX_TOTAL_TIME_SPENT: i64 = 126_230_400;

/// `TimeTrackable#check_negative_time_spent` and the timelog's total-time validation for
/// spending `secs` more on the MR.
pub fn spend_errors(mr: &MergeRequest, secs: i64) -> Option<Vec<(String, String)>> {
    let mut errs = vec![];
    if secs < 0 && secs.unsigned_abs() > mr.total_time_spent.max(0) as u64 {
        errs.push(("base".to_string(), "Time to subtract exceeds the total time spent".to_string()));
    }
    let total = mr.total_time_spent.saturating_add(secs);
    if total < 0 {
        errs.push(("timelogs.base".to_string(), "Total time spent cannot be negative.".to_string()));
    } else if total > MAX_TOTAL_TIME_SPENT {
        errs.push(("timelogs.base".to_string(), "Total time spent cannot exceed 4 years.".to_string()));
    }
    if errs.is_empty() { None } else { Some(errs) }
}
