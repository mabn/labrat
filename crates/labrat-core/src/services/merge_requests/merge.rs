//! Merging (merge commit, semi-linear, fast-forward, squash), rebase, auto-merge and the
//! source-branch cleanup that follows a merge.

use super::{MergeRequest, mergeability};
use crate::model::{Project, User};
use crate::services::notes;
use crate::{Config, Error, Result, jobs, repo, time};
use labrat_git::merge::{merge_trees, rebase};
use labrat_git::protocol::RefUpdate;
use labrat_git::{Commit, Oid, walk};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

fn full_reference(p: &Project, mr: &MergeRequest) -> String {
    format!("{}!{}", p.full_path, mr.iid)
}

/// `default_merge_commit_message` (no template): branch line, title, closing line, reference.
/// `user`: the merging user (`metrics.merged_by || merge_user || current_user`).
pub fn default_merge_commit_message(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: Option<&User>,
) -> Result<String> {
    if let Some(t) = p
        .merge_commit_template
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        return expand_template(c, config, p, mr, t, user, false);
    }
    Ok([
        format!(
            "Merge branch '{}' into '{}'",
            mr.source_branch, mr.target_branch
        ),
        mr.title.clone(),
        format!("See merge request {}", full_reference(p, mr)),
    ]
    .join("\n\n"))
}

pub fn default_squash_commit_message(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: Option<&User>,
) -> Result<String> {
    if let Some(t) = p
        .squash_commit_template
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        return expand_template(c, config, p, mr, t, user, true);
    }
    Ok(mr.title.clone())
}

/// `Gitlab::MergeRequests::MessageGenerator` placeholders.
pub fn expand_template(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    tpl: &str,
    merger: Option<&User>,
    squash: bool,
) -> Result<String> {
    // `metrics.merged_by || merge_user || current_user`.
    let merged_by_user = match mr.merged_by_id.or(mr.merge_user_id) {
        Some(id) => User::find(c, id)?,
        None => None,
    };
    let merger = merged_by_user.as_ref().or(merger);
    let commit_email = |u: &User| u.commit_email.clone().unwrap_or(u.email.clone());
    let r = repo::repo(c, p);
    let commits: Vec<Commit> = match mr.latest_diff(c)? {
        Some(d) => d
            .commits(&r)?
            .iter()
            .map(|o| r.commit(o))
            .collect::<std::result::Result<_, _>>()?,
        None => vec![],
    };
    let author = User::get(c, mr.author_id)?;
    let oldest = commits.last();
    let first_multiline = commits
        .iter()
        .rev()
        .find(|c| c.message.trim_end().contains('\n'));
    let mut values: Vec<(&str, String)> = vec![
        ("source_branch", mr.source_branch.clone()),
        ("target_branch", mr.target_branch.clone()),
        ("title", mr.title.clone()),
        ("issues", String::new()),
        ("description", mr.description.clone().unwrap_or_default()),
        ("reference", full_reference(p, mr)),
        ("local_reference", format!("!{}", mr.iid)),
        (
            "source_project_id",
            mr.source_project_id.unwrap_or(p.id).to_string(),
        ),
        (
            "first_commit",
            oldest
                .map(|c| c.message.trim_end().to_string())
                .unwrap_or_default(),
        ),
        (
            "first_multiline_commit",
            first_multiline
                .map(|c| c.message.trim_end().to_string())
                .unwrap_or(mr.title.clone()),
        ),
        (
            "first_multiline_commit_description",
            first_multiline
                .map(|c| {
                    c.message
                        .trim_end().split_once('\n').map(|x| x.1)
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
                .unwrap_or_default(),
        ),
        ("url", mr.web_url(config, p)),
        ("approved_by", {
            let mut lines = vec![];
            for (uid, _) in crate::services::reviews::approvals(c, mr)? {
                if let Some(u) = User::find(c, uid)? {
                    lines.push(format!("Approved-by: {} <{}>", u.name, commit_email(&u)));
                }
            }
            lines.join("\n")
        }),
        // `reviewed_by_users`: reviewers whose state is `reviewed`.
        ("reviewed_by", {
            let mut st = c.prepare("SELECT user_id FROM merge_request_reviewers WHERE merge_request_id = ?1 AND state = ?2 ORDER BY id")?;
            let ids: Vec<i64> = st.query_map(rusqlite::params![mr.id, crate::services::reviews::state::REVIEWED], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let mut lines = vec![];
            for uid in ids {
                if let Some(u) = User::find(c, uid)? {
                    lines.push(format!("Reviewed-by: {} <{}>", u.name, commit_email(&u)));
                }
            }
            lines.join("\n")
        }),
        (
            "merged_by",
            merger.map(|u| format!("{} <{}>", u.name, commit_email(u))).unwrap_or_else(|| " <>".to_string()),
        ),
        (
            "merge_request_author",
            format!("{} <{}>", author.name, commit_email(&author)),
        ),
        ("title_from_branch", humanize_branch(&mr.source_branch)),
        (
            "all_commits",
            commits
                .iter()
                .filter(|c| c.parents.len() < 2)
                .map(|c| format!("* {}", c.message.trim()))
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        // `recent_commits.to_h { email => name }` (newest first; a later name wins) without the
        // commit author: the MR author when squashing, the merging user otherwise.
        ("co_authored_by", {
            let excluded = if squash { Some(commit_email(&author)) } else { merger.map(|u| commit_email(u)) };
            let mut order: Vec<String> = vec![];
            let mut names: std::collections::HashMap<String, String> = Default::default();
            for cm in &commits {
                if !names.contains_key(&cm.author.email) {
                    order.push(cm.author.email.clone());
                }
                names.insert(cm.author.email.clone(), cm.author.name.clone());
            }
            order
                .iter()
                .filter(|e| Some(*e) != excluded.as_ref())
                .map(|e| format!("Co-authored-by: {} <{}>", names[e], e))
                .collect::<Vec<_>>()
                .join("\n")
        }),
    ];
    values.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    let mut out = tpl.replace('\r', "");
    // `replace_placeholders`: an empty placeholder that is a paragraph of its own (between blank
    // lines, or last) goes with its paragraph break; one that starts a line goes with that line's
    // newline. Then every placeholder is replaced.
    for (k, v) in &values {
        let ph = format!("%{{{k}}}");
        if !v.trim().is_empty() || !out.contains(&ph) {
            continue;
        }
        out = out.replace(&format!("\n\n{ph}\n\n"), "\n\n");
        if out.ends_with(&format!("\n\n{ph}")) {
            out.truncate(out.len() - ph.len() - 2);
        }
        let lines: Vec<&str> = out.split_inclusive('\n').collect();
        out = lines.into_iter().filter(|l| *l != format!("{ph}\n")).collect();
    }
    for (k, v) in &values {
        out = out.replace(&format!("%{{{k}}}"), v);
    }
    Ok(out)
}

fn humanize_branch(b: &str) -> String {
    let s = b.replace(['-', '_'], " ");
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn squash_on_merge(p: &Project, mr: &MergeRequest) -> bool {
    match p.squash_option.as_str() {
        "always" => true,
        "never" => false,
        _ => mr.squash,
    }
}

#[derive(Debug, Default, Clone)]
pub struct MergeParams {
    pub commit_message: Option<String>,
    pub squash_commit_message: Option<String>,
    pub should_remove_source_branch: Option<bool>,
    pub sha: Option<String>,
}

/// `MergeRequests::MergeService#execute`. On failure the MR stays open with `merge_error` set and
/// `Ok(false)` is returned.
pub fn merge(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    mp: &MergeParams,
) -> Result<bool> {
    let mr = mr.reload(c)?;
    if mr.state == "merged" {
        return Ok(false);
    }
    let fail = |msg: &str| -> Result<bool> {
        c.execute(
            "UPDATE merge_requests SET merge_error = ?1 WHERE id = ?2",
            params![msg, mr.id],
        )?;
        Ok(false)
    };
    if !super::can_be_merged_by(c, p, &mr, user)? {
        return fail("You are not allowed to merge this merge request");
    }
    let r = repo::repo(c, p);
    let Some(head_hex) = mr.diff_head_sha(c, p)? else {
        return fail("No source for merge");
    };
    let head = Oid::from_hex(&head_hex).map_err(Error::from)?;
    if p.merge_method == "ff" && mergeability::should_be_rebased(c, p, &mr)? {
        return fail(
            "Only fast-forward merge is allowed for your project. Please update your source branch",
        );
    }
    if !mergeability::mergeable(c, config, p, &mr)? {
        return fail("Merge request is not mergeable");
    }
    let squash = squash_on_merge(p, &mr);
    if p.squash_option == "always" && !mr.squash {
        // GitLab still requires the flag to be set when squashing is mandatory.
        return fail("This project requires squashing commits when merge requests are accepted.");
    }
    if let Some(sha) = &mp.sha
        && *sha != head_hex {
            return fail(
                "Branch has been updated since the merge was requested. Please review the changes.",
            );
        }
    let Some(target) = r.branch(&mr.target_branch)? else {
        return fail("No source for merge");
    };
    c.execute(
        "UPDATE merge_requests SET state = 'locked' WHERE id = ?1",
        [mr.id],
    )?;

    let mut source_tip = head;
    let mut squash_sha = None;
    if squash {
        let diff = mr.latest_diff(c)?.expect("diff exists");
        let commits = diff.commits(&r)?;
        let message = mp
            .squash_commit_message
            .clone()
            .map(Ok)
            .unwrap_or_else(|| default_squash_commit_message(c, config, p, &mr, Some(user)))?;
        let single_same = commits.len() == 1 && r.commit(&commits[0])?.message == message;
        if !single_same {
            let start = diff.oids().1.unwrap_or(target);
            let base = walk::merge_base(&r, &start, &head)?;
            let base_tree = match base {
                Some(b) => Some(r.commit(&b)?.tree),
                None => None,
            };
            let merged = merge_trees(
                &r,
                base_tree.as_ref(),
                &r.commit(&start)?.tree,
                &r.commit(&head)?.tree,
            )?;
            if !merged.clean() {
                c.execute(
                    "UPDATE merge_requests SET state = 'opened' WHERE id = ?1",
                    [mr.id],
                )?;
                return fail(
                    "Squashing failed: Squash the commits locally, resolve any conflicts, then push the branch.",
                );
            }
            let author = User::get(c, mr.author_id)?;
            let mut asig = repo::signature(&author);
            asig.time = time::now() / 1000;
            let commit = Commit {
                tree: merged.tree,
                parents: vec![start],
                author: asig,
                committer: repo::signature(user),
                extra: vec![],
                // Gitaly's squash goes through `git commit`, which ends the message with a newline.
                message: if message.ends_with('\n') { message.clone() } else { format!("{message}\n") },
            };
            source_tip = r.write_commit(&commit)?;
        }
        squash_sha = Some(source_tip);
    }

    let new_target;
    let mut merge_commit = None;
    if p.merge_method == "ff" {
        if !walk::is_ancestor(&r, &target, &source_tip)? {
            c.execute(
                "UPDATE merge_requests SET state = 'opened' WHERE id = ?1",
                [mr.id],
            )?;
            return fail(
                "Only fast-forward merge is allowed for your project. Please update your source branch",
            );
        }
        new_target = source_tip;
    } else {
        let base = walk::merge_base(&r, &target, &source_tip)?;
        let base_tree = match base {
            Some(b) => Some(r.commit(&b)?.tree),
            None => None,
        };
        let merged = merge_trees(
            &r,
            base_tree.as_ref(),
            &r.commit(&target)?.tree,
            &r.commit(&source_tip)?.tree,
        )?;
        if !merged.clean() {
            c.execute(
                "UPDATE merge_requests SET state = 'opened' WHERE id = ?1",
                [mr.id],
            )?;
            return fail("An error occurred while merging");
        }
        let message = mp
            .commit_message
            .clone()
            .map(Ok)
            .unwrap_or_else(|| default_merge_commit_message(c, config, p, &mr, Some(user)))?;
        let sig = repo::signature(user);
        let commit = Commit {
            tree: merged.tree,
            parents: vec![target, source_tip],
            author: sig.clone(),
            committer: sig,
            extra: vec![],
            message,
        };
        let oid = r.write_commit(&commit)?;
        merge_commit = Some(oid);
        new_target = oid;
    }
    r.set_ref(&format!("refs/heads/{}", mr.target_branch), &new_target)?;
    let mut before = super::snapshot(c, &mr.reload(c)?)?;
    // The hook's object never saw the temporary lock.
    if before.mr.state == "locked" {
        before.mr.state = "opened".into();
    }
    let now = time::now();
    c.execute(
        "UPDATE merge_requests SET state = 'merged', merge_error = NULL, merged_by_id = ?1, merged_at = ?2,
            merge_commit_sha = ?3, squash_commit_sha = ?4, merged_commit_sha = ?5, updated_at = ?2
         WHERE id = ?6",
        params![user.id, now, merge_commit.map(|o| o.hex()), squash_sha.map(|o| o.hex()), new_target.hex(), mr.id],
    )?;
    crate::services::user_counts::invalidate_all(c, &mr)?;
    set_first_contribution(c, p, &mr)?;
    let mut mparams = mr.merge_params.clone();
    let remove = mp
        .should_remove_source_branch
        .or(mr.force_remove_source_branch())
        .unwrap_or(false);
    // `merge_request_merge_param`: only kept when the branch can actually be removed.
    let removable = can_remove_source_branch(c, p, &mr, user, &head_hex)?;
    if let Some(s) = mp.should_remove_source_branch.filter(|_| removable) {
        mparams.insert("should_remove_source_branch".into(), Value::Bool(s));
        c.execute(
            "UPDATE merge_requests SET merge_params = ?1 WHERE id = ?2",
            params![Value::Object(mparams).to_string(), mr.id],
        )?;
    }
    super::state_event(c, mr.id, Some(user), "merged", None)?;
    crate::services::events::record(c, Some(p.id), user.id, "merged", Some("MergeRequest"), Some(mr.id), None)?;
    let merged = mr.reload(c)?;
    crate::services::todos::resolve_for_target(c, &merged, Some(user.id))?;
    // The target branch moved: run push processing for it (other MRs, merged-by-push, ...). The
    // refresh of the other MRs targeting it comes later, from a background job.
    super::refresh::DEFER_REFRESH.with(|d| d.set(true));
    let pushed = crate::services::push::post_receive(
        c,
        config,
        p,
        user,
        &[RefUpdate {
            old: target,
            new: new_target,
            name: format!("refs/heads/{}", mr.target_branch),
        }],
    );
    super::refresh::DEFER_REFRESH.with(|d| d.set(false));
    pushed?;
    // EE: the other open MRs into the target branch read as unchecked right after the merge
    // (the EE reference shows `checking` on them a moment later); CE waits for the refresh job.
    if config.is_ee() {
        for other in MergeRequest::list_where(
            c,
            "target_project_id = ?1 AND target_branch = ?2 AND state = 'opened' AND id != ?3",
            params![p.id, mr.target_branch, mr.id],
        )? {
            mergeability::mark_as_unchecked(c, other.id)?;
        }
    }
    crate::services::hooks::merge_request_event(c, config, p, &merged, Some(user), "merge", Some(&before))?;
    // `cancel_auto_merges_targeting_source_branch`: MRs stacked on the removed branch lose their
    // auto-merge, and their merge participants get an unmergeable to-do.
    if remove {
        for d in MergeRequest::list_where(
            c,
            "target_project_id = ?1 AND target_branch = ?2 AND state = 'opened' AND merge_when_pipeline_succeeds = 1",
            params![p.id, mr.source_branch],
        )? {
            abort_auto_merge(c, p, &d, user, &format!("target branch was merged in !{}", mr.iid))?;
            let d = d.reload(c)?;
            if let Some(author) = User::find(c, d.author_id)?
                && crate::perms::ProjectPerms::load(c, Some(&author), p)?.read_merge_request(p)
            {
                crate::services::todos::create(c, author.id, &author, p, &d, crate::services::todos::action::UNMERGEABLE, None)?;
            }
        }
    }
    if remove && can_remove_source_branch(c, p, &merged, user, &head_hex)? {
        jobs::enqueue(
            c,
            "delete_source_branch",
            json!({"merge_request_id": mr.id, "user_id": user.id, "sha": head_hex}),
        )?;
    }
    Ok(true)
}

/// `can_remove_source_branch?`: not protected, not the default branch, pushable, unchanged.
pub fn can_remove_source_branch(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    head: &str,
) -> Result<bool> {
    if p.default_branch.as_deref() == Some(mr.source_branch.as_str())
        || repo::is_protected(c, p, &mr.source_branch)?
    {
        return Ok(false);
    }
    if !repo::can_push_to_branch(c, user, p, &mr.source_branch)? {
        return Ok(false);
    }
    Ok(mr.source_branch_sha(c, p)?.as_deref() == Some(head))
}

/// `DeleteSourceBranchWorker`: retarget dependent MRs, then delete the branch.
pub fn delete_source_branch_job(
    c: &Connection,
    config: &Config,
    mr_id: i64,
    user_id: i64,
    sha: &str,
) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else {
        return Ok(());
    };
    let p = Project::find(c, mr.target_project_id)?.expect("project exists");
    let user = User::get(c, user_id)?;
    if mr.source_branch_sha(c, &p)?.as_deref() != Some(sha) {
        return Ok(());
    }
    // `RetargetChainService`: up to 4 open MRs targeting the merged MR's source branch.
    let dependents = MergeRequest::list_where(
        c,
        "target_project_id = ?1 AND target_branch = ?2 AND state = 'opened' ORDER BY id LIMIT 4",
        params![p.id, mr.source_branch],
    )?;
    for d in dependents {
        let before = super::snapshot(c, &d)?;
        c.execute(
            "UPDATE merge_requests SET target_branch = ?1, retargeted = 1, updated_at = ?2 WHERE id = ?3",
            params![mr.target_branch, time::now(), d.id],
        )?;
        notes::system_note(
            c,
            d.id,
            p.id,
            &user,
            &format!(
                "deleted the `{}` branch. This merge request now targets the `{}` branch",
                mr.source_branch, mr.target_branch
            ),
            "branch",
        )?;
        let d2 = d.reload(c)?;
        super::diffs::create_version(c, &p, &d2)?;
        // `target_branch_was_deleted`: the head pipeline is dropped so a new one must run.
        c.execute("UPDATE merge_request_pipelines SET head_pipeline_id = NULL WHERE merge_request_id = ?1", [d.id])?;
        mergeability::mark_as_unchecked(c, d.id)?;
        let d2 = d.reload(c)?;
        crate::services::hooks::merge_request_event(
            c,
            config,
            &p,
            &d2,
            Some(&user),
            "update",
            Some(&before),
        )?;
    }
    let r = repo::repo(c, &p);
    if let Some(old) = r.branch(&mr.source_branch)? {
        r.delete_ref(&format!("refs/heads/{}", mr.source_branch))?;
        crate::services::push::post_receive(
            c,
            config,
            &p,
            &user,
            &[RefUpdate {
                old,
                new: Oid::ZERO,
                name: format!("refs/heads/{}", mr.source_branch),
            }],
        )?;
    }
    Ok(())
}

/// Queues a rebase (`PUT /rebase`); the job does the work.
pub fn request_rebase(c: &Connection, mr: &MergeRequest, user: &User, skip_ci: bool) -> Result<()> {
    c.execute(
        "UPDATE merge_requests SET rebase_in_progress = 1 WHERE id = ?1",
        [mr.id],
    )?;
    jobs::enqueue(
        c,
        "mr_rebase",
        json!({"merge_request_id": mr.id, "user_id": user.id, "skip_ci": skip_ci}),
    )?;
    Ok(())
}

/// `RebaseService#execute`.
pub fn rebase_job(
    c: &Connection,
    config: &Config,
    mr_id: i64,
    user_id: i64,
    skip_ci: bool,
) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else {
        return Ok(());
    };
    let p = Project::find(c, mr.target_project_id)?.expect("project exists");
    let user = User::get(c, user_id)?;
    let r = repo::repo(c, &p);
    let done = |err: Option<&str>| -> Result<()> {
        c.execute(
            "UPDATE merge_requests SET rebase_in_progress = 0, merge_error = ?1 WHERE id = ?2",
            params![err, mr.id],
        )?;
        Ok(())
    };
    let (Some(source), Some(target)) = (r.branch(&mr.source_branch)?, r.branch(&mr.target_branch)?)
    else {
        return done(Some(
            "Rebase failed: Rebase locally, resolve all conflicts, then push the branch.",
        ));
    };
    let base = walk::merge_base(&r, &source, &target)?;
    let mut commits = walk::rev_list(
        &r,
        &[source],
        &base.into_iter().chain([target]).collect::<Vec<_>>(),
    )?;
    commits.reverse();
    // Already on top of the target: Gitaly leaves the branch as it is, but the "push" (with the
    // same old and new revision) is still processed.
    let up_to_date = base == Some(target);
    let result = if up_to_date { Ok(source) } else { rebase(&r, &target, &commits, &repo::signature(&user))? };
    match result {
        Ok(new_head) => {
            if new_head != source || up_to_date {
                r.set_ref(&format!("refs/heads/{}", mr.source_branch), &new_head)?;
                c.execute(
                    "UPDATE merge_requests SET rebase_commit_sha = ?1 WHERE id = ?2",
                    params![new_head.hex(), mr.id],
                )?;
                // `skip_ci` pushes the rebased branch with the `ci.skip` push option.
                crate::services::ci::PUSH_SKIPS_CI.with(|f| f.set(skip_ci));
                let pushed = crate::services::push::post_receive(
                    c,
                    config,
                    &p,
                    &user,
                    &[RefUpdate {
                        old: source,
                        new: new_head,
                        name: format!("refs/heads/{}", mr.source_branch),
                    }],
                );
                crate::services::ci::PUSH_SKIPS_CI.with(|f| f.set(false));
                pushed?;
            }
            done(None)
        }
        Err(_) => done(Some(
            "Rebase failed: Rebase locally, resolve all conflicts, then push the branch.",
        )),
    }
}

/// Enables auto-merge (`merge_when_checks_pass`).
pub fn enable_auto_merge(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    mp: &MergeParams,
) -> Result<()> {
    let sha = mp
        .sha
        .clone()
        .or(mr.diff_head_sha(c, p)?)
        .unwrap_or_default();
    let mut params_map = mr.merge_params.clone();
    params_map.insert(
        "auto_merge_strategy".into(),
        json!("merge_when_checks_pass"),
    );
    params_map.insert("sha".into(), json!(sha));
    if let Some(m) = &mp.commit_message {
        params_map.insert("commit_message".into(), json!(m));
    }
    if let Some(m) = &mp.squash_commit_message {
        params_map.insert("squash_commit_message".into(), json!(m));
    }
    if let Some(s) = mp.should_remove_source_branch {
        params_map.insert("should_remove_source_branch".into(), json!(s));
    }
    c.execute(
        "UPDATE merge_requests SET merge_when_pipeline_succeeds = 1, merge_user_id = ?1, merge_params = ?2 WHERE id = ?3",
        params![user.id, Value::Object(params_map).to_string(), mr.id],
    )?;
    // The note is written only when auto-merge switches on (`saved_change_to_auto_merge_enabled?`).
    if !mr.merge_when_pipeline_succeeds {
        notes::system_note(
            c,
            mr.id,
            p.id,
            user,
            &format!("enabled an automatic merge when all merge checks for {sha} pass"),
            "merge",
        )?;
    }
    Ok(())
}

fn clear_auto_merge(c: &Connection, mr: &MergeRequest) -> Result<()> {
    let mut mp = mr.merge_params.clone();
    for k in [
        "should_remove_source_branch",
        "commit_message",
        "squash_commit_message",
        "auto_merge_strategy",
    ] {
        mp.remove(k);
    }
    c.execute(
        "UPDATE merge_requests SET merge_when_pipeline_succeeds = 0, merge_user_id = NULL, merge_params = ?1 WHERE id = ?2",
        params![Value::Object(mp).to_string(), mr.id],
    )?;
    Ok(())
}

/// Cancels auto-merge (`POST /cancel_merge_when_pipeline_succeeds`).
pub fn cancel_auto_merge(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
) -> Result<bool> {
    if !mr.merge_when_pipeline_succeeds {
        return Ok(false);
    }
    clear_auto_merge(c, mr)?;
    notes::system_note(
        c,
        mr.id,
        p.id,
        user,
        "canceled the automatic merge",
        "merge",
    )?;
    Ok(true)
}

pub fn abort_auto_merge(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    reason: &str,
) -> Result<()> {
    if !mr.merge_when_pipeline_succeeds {
        return Ok(());
    }
    clear_auto_merge(c, mr)?;
    notes::system_note(
        c,
        mr.id,
        p.id,
        user,
        &format!("aborted the automatic merge because {reason}"),
        "merge",
    )?;
    Ok(())
}

/// `AutoMergeProcessWorker`: merges once all checks pass.
pub fn auto_merge_process(c: &Connection, config: &Config, mr_id: i64) -> Result<()> {
    let Some(mr) = MergeRequest::find(c, mr_id)? else {
        return Ok(());
    };
    if !mr.merge_when_pipeline_succeeds || !mr.is_open() {
        return Ok(());
    }
    let p = Project::find(c, mr.target_project_id)?.expect("project exists");
    // `mwcp_skip_ci_guard` is off in 18.11: with CI, wait for a green head pipeline.
    if crate::services::ci::has_ci(c, &p, &mr)? && !crate::services::ci::head_pipeline_success(c, &p, &mr)? {
        return Ok(());
    }
    if !mergeability::mergeable(c, config, &p, &mr)? {
        return Ok(());
    }
    let Some(uid) = mr.merge_user_id else {
        return Ok(());
    };
    let user = User::get(c, uid)?;
    let mp = MergeParams {
        commit_message: mr
            .merge_params
            .get("commit_message")
            .and_then(Value::as_str)
            .map(str::to_string),
        squash_commit_message: mr
            .merge_params
            .get("squash_commit_message")
            .and_then(Value::as_str)
            .map(str::to_string),
        should_remove_source_branch: mr
            .merge_params
            .get("should_remove_source_branch")
            .and_then(Value::as_bool),
        sha: mr
            .merge_params
            .get("sha")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    merge(c, config, &p, &mr, &user, &mp)?;
    Ok(())
}

/// `first_contribution`: no earlier merged MR of the author in the project (set on every merge,
/// through the API or by push).
pub(crate) fn set_first_contribution(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<()> {
    let first: i64 = c.query_row(
        "SELECT count(*) FROM merge_requests WHERE target_project_id = ?1 AND author_id = ?2 AND state = 'merged' AND id != ?3",
        params![p.id, mr.author_id, mr.id],
        |r| r.get(0),
    )?;
    c.execute("UPDATE merge_requests SET first_contribution = ?1 WHERE id = ?2", params![first == 0, mr.id])?;
    Ok(())
}
