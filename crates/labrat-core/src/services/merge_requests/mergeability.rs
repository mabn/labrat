//! `merge_status`, `detailed_merge_status` and the mergeability check.

use super::MergeRequest;
use crate::model::Project;
use crate::{Config, Result, jobs, repo, time};
use labrat_git::merge::merge_trees;
use labrat_git::{Commit, Oid, walk};
use rusqlite::{Connection, params};
use serde_json::json;

/// `public_merge_status`: `preparing` and `cannot_be_merged_rechecking` show as `checking`.
pub fn public_status(s: &str) -> &str {
    match s {
        "preparing" | "cannot_be_merged_rechecking" => "checking",
        other => other,
    }
}

/// `mark_as_unchecked`.
pub fn mark_as_unchecked(c: &Connection, mr_id: i64) -> Result<()> {
    c.execute(
        "UPDATE merge_requests SET merge_status = CASE merge_status
            WHEN 'cannot_be_merged' THEN 'cannot_be_merged_recheck'
            WHEN 'cannot_be_merged_rechecking' THEN 'cannot_be_merged_recheck'
            WHEN 'cannot_be_merged_recheck' THEN 'cannot_be_merged_recheck'
            ELSE 'unchecked' END
         WHERE id = ?1",
        [mr_id],
    )?;
    Ok(())
}

/// `check_mergeability(async: true)`: moves to `checking` and queues the check.
pub fn check_async(c: &Connection, mr: &MergeRequest) -> Result<bool> {
    let next = match mr.merge_status.as_str() {
        "unchecked" => "checking",
        "cannot_be_merged_recheck" => "cannot_be_merged_rechecking",
        _ => return Ok(false),
    };
    c.execute(
        "UPDATE merge_requests SET merge_status = ?1 WHERE id = ?2",
        params![next, mr.id],
    )?;
    // Checks of closed and merged MRs are a separate kind so the harness can give them the
    // longer Sidekiq latency seen in recordings.
    let kind = if mr.is_open() { "mr_check_mergeability" } else { "mr_check_mergeability_closed" };
    jobs::enqueue(c, kind, json!({"merge_request_id": mr.id}))?;
    Ok(true)
}

/// No commits, or a missing source/target branch (`broken?` minus the conflict part).
pub fn broken(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<bool> {
    let r = repo::repo(c, p);
    if r.branch(&mr.source_branch)?.is_none() || r.branch(&mr.target_branch)?.is_none() {
        return Ok(true);
    }
    match mr.latest_diff(c)? {
        Some(d) => Ok(d.state == "empty" || d.commits_count == 0),
        None => Ok(true),
    }
}

/// `MergeabilityCheckService`: computes `can_be_merged` / `cannot_be_merged` and writes the
/// `refs/merge-requests/:iid/merge` test merge.
pub fn check(c: &Connection, config: &Config, mr_id: i64) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else {
        return Ok(());
    };
    // GitLab checks closed MRs too (e.g. one closed while its creation was still preparing).
    if mr.state == "merged" {
        // `MergeToRefService` refuses a merged MR: one whose status was reset (a retarget) ends
        // up unmergeable. Otherwise its status stays as it was at the merge.
        if matches!(mr.merge_status.as_str(), "unchecked" | "checking" | "cannot_be_merged_recheck" | "cannot_be_merged_rechecking") {
            c.execute("UPDATE merge_requests SET merge_status = 'cannot_be_merged' WHERE id = ?1", [mr_id])?;
        }
        return Ok(());
    }
    let p = Project::find(c, mr.target_project_id)?.expect("project exists");
    let mut conflict = false;
    let status = if broken(c, &p, &mr)? {
        "cannot_be_merged"
    } else {
        conflict = true;
        match test_merge(c, config, &p, &mr)? {
            Some(sha) => {
                c.execute(
                    "UPDATE merge_requests SET merge_ref_sha = ?1 WHERE id = ?2",
                    params![sha.hex(), mr.id],
                )?;
                "can_be_merged"
            }
            None => "cannot_be_merged",
        }
    };
    let conflict = conflict && status == "cannot_be_merged";
    c.execute(
        "UPDATE merge_requests SET merge_status = ?1 WHERE id = ?2",
        params![status, mr.id],
    )?;
    // `after_transition [:unchecked, :checking] => :cannot_be_merged`: when `notify_conflict?`
    // (open, with commits, both branches there, and a real conflict) the merge participants
    // get an unmergeable to-do.
    if conflict && matches!(mr.merge_status.as_str(), "unchecked" | "checking") && mr.is_open() {
        let mut participants = vec![mr.author_id];
        if mr.merge_when_pipeline_succeeds
            && let Some(u) = mr.merge_user_id
            && u != mr.author_id
        {
            participants.push(u);
        }
        for uid in participants {
            if let Some(u) = crate::model::User::find(c, uid)?
                && crate::perms::ProjectPerms::load(c, Some(&u), &p)?.read_merge_request(&p)
            {
                crate::services::todos::create(c, u.id, &u, &p, &mr, crate::services::todos::action::UNMERGEABLE, None)?;
            }
        }
    }
    if status == "can_be_merged" && mr.merge_when_pipeline_succeeds {
        jobs::enqueue(c, "mr_auto_merge", json!({"merge_request_id": mr.id}))?;
    }
    Ok(())
}

/// Builds the test merge commit on `refs/merge-requests/:iid/merge`; `None` on conflicts.
pub fn test_merge(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
) -> Result<Option<Oid>> {
    let r = repo::repo(c, p);
    let (Some(source), Some(target)) = (r.branch(&mr.source_branch)?, r.branch(&mr.target_branch)?)
    else {
        return Ok(None);
    };
    let base = walk::merge_base(&r, &source, &target)?;
    let base_tree = match base {
        Some(b) => Some(r.commit(&b)?.tree),
        None => None,
    };
    let result = merge_trees(
        &r,
        base_tree.as_ref(),
        &r.commit(&target)?.tree,
        &r.commit(&source)?.tree,
    )?;
    if !result.clean() {
        return Ok(None);
    }
    let author = crate::model::User::get(c, mr.author_id)?;
    let sig = repo::signature(&author);
    let message = super::merge::default_merge_commit_message(c, config, p, mr, Some(&author))?;
    let commit = Commit {
        tree: result.tree,
        parents: vec![target, source],
        author: sig.clone(),
        committer: sig,
        extra: vec![],
        message,
    };
    let oid = r.write_commit(&commit)?;
    r.set_ref(&format!("refs/merge-requests/{}/merge", mr.iid), &oid)?;
    let _ = time::now();
    Ok(Some(oid))
}

/// Does the project require the source to contain the target head (ff or semi-linear)?
pub fn ff_required(p: &Project) -> bool {
    p.merge_method == "ff" || p.merge_method == "rebase_merge"
}

/// `should_be_rebased?`: fast-forward required but the target head isn't in the source.
pub fn should_be_rebased(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<bool> {
    if !ff_required(p) {
        return Ok(false);
    }
    let r = repo::repo(c, p);
    let (Some(target), Some(head)) = (
        r.branch(&mr.target_branch)?,
        mr.diff_head_sha(c, p)?.and_then(|h| Oid::from_hex(&h).ok()),
    ) else {
        return Ok(false);
    };
    Ok(!walk::is_ancestor(&r, &target, &head)?)
}

pub fn unresolved_discussions(c: &Connection, mr: &MergeRequest) -> Result<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND resolvable = 1 AND resolved_at IS NULL",
        [mr.id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `blocking_discussions_resolved` as exposed in the API.
pub fn blocking_discussions_resolved(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
) -> Result<bool> {
    if !p.only_allow_merge_if_all_discussions_are_resolved {
        return Ok(true);
    }
    Ok(!unresolved_discussions(c, mr)?)
}

/// `Mergeability::DetailedMergeStatusService`.
pub fn detailed_status(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest) -> Result<String> {
    match mr.merge_status.as_str() {
        "preparing" => return Ok("preparing".into()),
        "checking" | "cannot_be_merged_rechecking" => return Ok("checking".into()),
        "unchecked" => return Ok("unchecked".into()),
        _ => {}
    }
    if let Some(failed) = first_failed_check(c, config, p, mr, true)? {
        return Ok(failed);
    }
    // The CI check runs last on its own.
    if let Some(ci) = crate::services::ci::ci_check(c, p, mr)? {
        return Ok(ci.into());
    }
    Ok("mergeable".into())
}

/// The checks auto-merge does not skip (`skipped_auto_merge_checks` for merge-when-checks-pass):
/// the MR must be open, have commits and both branches, not need a rebase, and have no known
/// conflict (a status still to be rechecked doesn't count).
pub fn auto_merge_blocking(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Option<String>> {
    if !mr.is_open() {
        return Ok(Some("not_open".into()));
    }
    if broken(c, p, mr)? {
        return Ok(Some("commits_status".into()));
    }
    if should_be_rebased(c, p, mr)? {
        return Ok(Some("need_rebase".into()));
    }
    let recheck = matches!(mr.merge_status.as_str(), "unchecked" | "checking" | "cannot_be_merged_recheck" | "cannot_be_merged_rechecking");
    if !recheck && mr.merge_status == "cannot_be_merged" {
        return Ok(Some("conflict".into()));
    }
    Ok(None)
}

/// Returns the identifier of the first blocking check (skipping CI when `skip_ci`).
pub fn first_failed_check(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    skip_ci: bool,
) -> Result<Option<String>> {
    if !mr.is_open() {
        return Ok(Some("not_open".into()));
    }
    if mr.is_draft() {
        return Ok(Some("draft_status".into()));
    }
    if super::merge_after(c, mr)?.is_some_and(|t| t > crate::time::now()) {
        return Ok(Some("merge_time".into()));
    }
    // EE: a reviewer's "changes requested" blocks merging.
    if config.is_ee() && mr.reviewers(c)?.iter().any(|(_, _, state, _)| *state == crate::services::reviews::state::REQUESTED_CHANGES) {
        return Ok(Some("requested_changes".into()));
    }
    if p.only_allow_merge_if_all_discussions_are_resolved && unresolved_discussions(c, mr)? {
        return Ok(Some("discussions_not_resolved".into()));
    }
    if !skip_ci && crate::services::ci::ci_check(c, p, mr)?.is_some() {
        return Ok(Some("ci_must_pass".into()));
    }
    if broken(c, p, mr)? {
        return Ok(Some("commits_status".into()));
    }
    // EE checks the approval rules after the commits and before the rebase (EE 18.11's
    // `mergeabilityChecks` order).
    if config.is_ee() && !crate::services::approvals::state(c, p, mr)?.approved() {
        return Ok(Some("not_approved".into()));
    }
    if should_be_rebased(c, p, mr)? {
        return Ok(Some("need_rebase".into()));
    }
    let source = mr.source_branch_sha(c, p)?;
    let diff_head = mr.latest_diff(c)?.and_then(|d| d.head_commit_sha);
    if source != diff_head
        || !matches!(
            mr.merge_status.as_str(),
            "can_be_merged" | "cannot_be_merged"
        )
    {
        return Ok(Some("conflict".into()));
    }
    if mr.merge_status == "cannot_be_merged" {
        return Ok(Some("conflict".into()));
    }
    Ok(None)
}

/// `mergeable?`: all checks including CI, after a synchronous recheck if needed.
pub fn mergeable(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest) -> Result<bool> {
    if matches!(
        mr.merge_status.as_str(),
        "unchecked"
            | "checking"
            | "cannot_be_merged_recheck"
            | "cannot_be_merged_rechecking"
            | "preparing"
    ) {
        check(c, config, mr.id)?;
    }
    let mr = mr.reload(c)?;
    Ok(first_failed_check(c, config, p, &mr, false)?.is_none())
}
