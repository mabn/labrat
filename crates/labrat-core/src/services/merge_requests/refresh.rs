//! `MergeRequests::RefreshService`: what a branch push does to merge requests.

use super::{MergeRequest, diffs, mergeability};
use crate::model::{Project, User};
use crate::services::notes;
use crate::{Config, Result, repo};
use labrat_git::protocol::RefUpdate;
use labrat_git::{Oid, walk};
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};

/// Registered as a push branch hook.
/// Pushes refresh their merge requests right away (GitLab does it in `UpdateMergeRequestsWorker`;
/// the merge of an MR defers it, see `merge::merge`).
pub fn on_push(c: &Connection, config: &Config, p: &Project, user: &User, u: &RefUpdate) -> Result<()> {
    if DEFER_REFRESH.with(|d| d.get()) {
        return crate::jobs::enqueue(
            c,
            "update_merge_requests",
            serde_json::json!({"project_id": p.id, "user_id": user.id, "old": u.old.hex(), "new": u.new.hex(), "ref": u.name,
                "skip_ci": crate::services::ci::PUSH_SKIPS_CI.with(|f| f.get())}),
        );
    }
    refresh(c, config, p, user, u)
}

thread_local! {
    /// Set while an MR merge writes its target branch: the refresh of the other MRs then runs as a
    /// background job, as in GitLab where it follows the merge through Gitaly's hooks and Sidekiq.
    pub static DEFER_REFRESH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The `update_merge_requests` job.
pub fn refresh_job(c: &Connection, config: &Config, payload: &serde_json::Value) -> Result<()> {
    let Some(p) = Project::find(c, payload["project_id"].as_i64().unwrap_or(0))? else { return Ok(()) };
    let Some(user) = User::find(c, payload["user_id"].as_i64().unwrap_or(0))? else { return Ok(()) };
    let oid = |k: &str| Oid::from_hex(payload[k].as_str().unwrap_or("")).unwrap_or(Oid::ZERO);
    let u = RefUpdate { old: oid("old"), new: oid("new"), name: payload["ref"].as_str().unwrap_or("").to_string() };
    // The push's `ci.skip` option still applies to the MR pipelines of the refresh.
    crate::services::ci::PUSH_SKIPS_CI.with(|f| f.set(payload["skip_ci"].as_bool().unwrap_or(false)));
    let r = refresh(c, config, &p, &user, &u);
    crate::services::ci::PUSH_SKIPS_CI.with(|f| f.set(false));
    r
}

/// `MergeRequests::RefreshService`.
pub fn refresh(
    c: &Connection,
    config: &Config,
    p: &Project,
    user: &User,
    u: &RefUpdate,
) -> Result<()> {
    let Some(branch) = u.name.strip_prefix("refs/heads/") else {
        return Ok(());
    };
    let r = repo::repo(c, p);
    let created = u.old.is_zero();
    let removed = u.new.is_zero();
    let force = !created && !removed && !walk::is_ancestor(&r, &u.old, &u.new)?;

    // New commits in push order (oldest first).
    let pushed: Vec<Oid> = if removed {
        vec![]
    } else if created {
        // Commits not on the default branch.
        let default = p
            .default_branch
            .as_deref()
            .and_then(|b| r.branch(b).ok().flatten());
        let mut v = walk::rev_list(
            &r,
            &[u.new],
            &default
                .filter(|d| *d != u.new)
                .into_iter()
                .collect::<Vec<_>>(),
        )?;
        v.reverse();
        v
    } else {
        let mut v = walk::rev_list(&r, &[u.new], &[u.old])?;
        v.reverse();
        v
    };
    let pushed_set: HashSet<String> = pushed.iter().map(|o| o.hex()).collect();

    // Close open MRs whose source branch disappeared.
    if removed {
        for mr in MergeRequest::list_where(
            c,
            "target_project_id = ?1 AND source_branch = ?2 AND state = 'opened'",
            params![p.id, branch],
        )? {
            super::close(c, config, p, &mr, user)?;
        }
    }

    // Merged by push: open MRs targeting this branch whose head is now in it.
    let merge_commits = if removed { HashMap::new() } else { merge_commit_analysis(&r, &pushed)? };
    if !removed {
        for mr in MergeRequest::list_where(
            c,
            "target_project_id = ?1 AND target_branch = ?2 AND state = 'opened'",
            params![p.id, branch],
        )? {
            let Some(d) = mr.latest_diff(c)? else {
                continue;
            };
            if d.state == "empty" {
                continue;
            }
            let Some(head) = d.head_commit_sha.clone() else {
                continue;
            };
            if pushed_set.contains(&head) {
                let merge_commit = merge_commits.get(&head).cloned().unwrap_or(head.clone());
                mark_merged_by_push(c, config, p, &mr, user, &merge_commit)?;
            }
        }
    }

    // Reload diffs and mark unchecked.
    let open = MergeRequest::list_where(
        c,
        "target_project_id = ?1 AND state = 'opened' AND (source_branch = ?2 OR target_branch = ?2)",
        params![p.id, branch],
    )?;
    for mr in open {
        let before = super::snapshot(c, &mr)?;
        let is_source = mr.source_branch == branch;
        let mut reloaded = false;
        if (is_source || force) && !removed {
            let old_diff = mr.latest_diff(c)?;
            diffs::create_version(c, p, &mr)?;
            c.execute(
                "UPDATE merge_requests SET merge_error = NULL WHERE id = ?1",
                [mr.id],
            )?;
            reloaded = true;
            let mr2 = mr.reload(c)?;
            crate::services::discussions::update_diff_positions(
                c,
                config,
                p,
                &mr2,
                old_diff.as_ref(),
                user,
            )?;
        } else if !is_source
            && let Some(d) = mr.latest_diff(c)? {
                let commits: HashSet<String> = d.commits(&r)?.iter().map(|o| o.hex()).collect();
                if commits.iter().any(|x| pushed_set.contains(x)) {
                    diffs::create_version(c, p, &mr)?;
                    reloaded = true;
                }
            }
        mergeability::mark_as_unchecked(c, mr.id)?;
        let mr = mr.reload(c)?;
        if is_source && !removed {
            if let Some(pl) = crate::services::ci::create_skipped_mr_pipeline(c, p, &mr, user)? {
                crate::services::hooks::pipeline_event(c, config, p, &pl)?;
            }
        }
        // `mark_pending_todos_done`: the pusher's pending to-dos on MRs from this branch.
        if is_source && !removed {
            crate::services::todos::resolve_for_target(c, &mr, Some(user.id))?;
        }
        // `abort_auto_merge_with_todo`: in fast-forward projects, an auto-merge that now needs a
        // rebase is aborted, and the author (the only merge participant left) gets a to-do.
        if !is_source
            && !removed
            && mr.merge_when_pipeline_succeeds
            && matches!(p.merge_method.as_str(), "ff" | "rebase_merge")
            && mergeability::should_be_rebased(c, p, &mr)?
        {
            super::merge::abort_auto_merge(c, p, &mr, user, "target branch was updated")?;
            let mr = mr.reload(c)?;
            if let Some(author) = User::find(c, mr.author_id)?
                && crate::perms::ProjectPerms::load(c, Some(&author), p)?.read_merge_request(p)
            {
                crate::services::todos::create(c, author.id, &author, p, &mr, crate::services::todos::action::UNMERGEABLE, None)?;
            }
        }
        let mr = mr.reload(c)?;
        if is_source {
            if mr.merge_when_pipeline_succeeds
                && mr.merge_params.get("sha").and_then(|v| v.as_str()) != Some(&u.new.hex())
            {
                super::merge::abort_auto_merge(
                    c,
                    p,
                    &mr,
                    user,
                    &format!(
                        "the source branch was updated. [Learn more]({}).",
                        config.url("/help/ci/pipelines/merge_trains.md#merge-request-dropped-from-the-merge-train")
                    ),
                )?;
            }
            if created {
                notes::system_note(
                    c,
                    mr.id,
                    p.id,
                    user,
                    &format!("restored source branch `{branch}`"),
                    "branch",
                )?;
            } else if removed {
                notes::system_note(
                    c,
                    mr.id,
                    p.id,
                    user,
                    &format!("deleted source branch `{branch}`"),
                    "branch",
                )?;
            }
            if !pushed.is_empty() && reloaded {
                add_commits_note(
                    c,
                    p,
                    &mr,
                    user,
                    &pushed,
                    if created { None } else { Some(u.old) },
                )?;
                auto_draft_from_commits(c, p, &mr, user, &pushed)?;
                crate::services::approvals::reset_on_push(c, config, p, &mr, user, &before.mr.merge_status)?;
                crate::services::approvals::start_sync(c, config, &mr)?;
            }
            let mr = mr.reload(c)?;
            if reloaded {
                crate::services::hooks::merge_request_push_update(
                    c, config, p, &mr, user, &before, &u.old,
                )?;
            }
        }
    }
    Ok(())
}

fn mark_merged_by_push(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    head: &str,
) -> Result<()> {
    let before = super::snapshot(c, mr)?;
    let now = crate::time::now();
    c.execute(
        "UPDATE merge_requests SET state = 'merged', merged_by_id = ?1, merged_at = ?2, merge_commit_sha = ?3,
            merged_commit_sha = ?3, updated_at = ?2, merge_error = NULL WHERE id = ?4",
        params![user.id, now, head, mr.id],
    )?;
    crate::services::user_counts::invalidate_all(c, mr)?;
    super::merge::set_first_contribution(c, p, mr)?;
    // The event's source: a merged MR (one merged without an event source, earliest merge
    // first) whose merge commit, squash commit or commits include the SHA; else the commit.
    match source_merge_request(c, p, mr.id, head)? {
        Some(src) => {
            super::state_event(c, mr.id, Some(user), "merged", None)?;
            c.execute(
                "UPDATE resource_state_events SET source_merge_request_id = ?1 WHERE id = ?2",
                params![src, c.last_insert_rowid()],
            )?;
        }
        None => super::state_event(c, mr.id, Some(user), "merged", Some(head))?,
    }
    crate::services::events::record(c, Some(p.id), user.id, "merged", Some("MergeRequest"), Some(mr.id), None)?;
    let mr = mr.reload(c)?;
    crate::services::todos::resolve_for_target(c, &mr, Some(user.id))?;
    crate::services::hooks::merge_request_event(c, config, p, &mr, Some(user), "merge", Some(&before))?;
    Ok(())
}

/// `SystemNoteService.add_commits`.
fn add_commits_note(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    pushed: &[Oid],
    oldrev: Option<Oid>,
) -> Result<()> {
    // `notify_about_push`: the pushed commits split into those in the MR ("new") and the rest
    // (e.g. target-branch commits brought in by a rebase or merge, "existing").
    let r = repo::repo(c, p);
    if pushed.is_empty() {
        return Ok(());
    }
    let current: HashSet<String> = match mr.latest_diff(c)? {
        Some(d) => d.commits(&r)?.iter().map(|o| o.hex()).collect(),
        None => HashSet::new(),
    };
    let (new, existing): (Vec<Oid>, Vec<Oid>) = pushed.iter().copied().partition(|o| current.contains(&o.hex()));
    let total = new.len() + existing.len();
    let mut items = String::new();
    let short = |o: &Oid| o.hex()[..8].to_string();
    if !existing.is_empty() {
        let ids = if existing.len() == 1 {
            short(&existing[0])
        } else if let Some(old) = oldrev {
            format!("{}...{}", short(&old), short(existing.last().unwrap()))
        } else {
            format!(
                "{}..{}",
                short(&existing[0]),
                short(existing.last().unwrap())
            )
        };
        let n = existing.len();
        items.push_str(&format!(
            "<li>{ids} - {n} commit{} from branch <code>{}</code></li>",
            if n == 1 { "" } else { "s" },
            notes::html_escape(&mr.target_branch)
        ));
    }
    let limit = 10;
    let mut shown: &[Oid] = &new;
    if new.len() > limit {
        let k = new.len() - limit;
        let collapsed = &new[..k];
        let start_rev = if existing.is_empty() {
            oldrev
        } else {
            existing.last().copied()
        };
        let ids = if k == 1 {
            short(&collapsed[0])
        } else if let Some(s) = start_rev {
            format!("{}...{}", short(&s), short(collapsed.last().unwrap()))
        } else {
            format!(
                "{}..{}",
                short(&collapsed[0]),
                short(collapsed.last().unwrap())
            )
        };
        items.push_str(&format!(
            "<li>{ids} - {k} earlier commit{}</li>",
            if k == 1 { "" } else { "s" }
        ));
        shown = &new[k..];
    }
    for o in shown {
        let title = gitlab_title(&r.commit(o)?.title());
        items.push_str(&format!(
            "<li>{} - {}</li>",
            short(o),
            notes::html_escape(&title)
        ));
    }
    let diff_id = mr.latest_diff_id.unwrap_or(0);
    let old = oldrev.map(|o| o.hex()).unwrap_or_else(|| Oid::ZERO.hex());
    let body = format!(
        "added {total} commit{}\n\n<ul>{items}</ul>\n\n[Compare with previous version](/{}/-/merge_requests/{}/diffs?diff_id={diff_id}&start_sha={old})",
        if total == 1 { "" } else { "s" },
        p.full_path,
        mr.iid
    );
    let note = notes::system_note(c, mr.id, p.id, user, &body, "commit")?;
    let _ = note;
    Ok(())
}

/// Commits titled `draft:`/`fixup!`/`squash!` mark the MR as draft.
fn auto_draft_from_commits(
    c: &Connection,
    p: &Project,
    mr: &MergeRequest,
    user: &User,
    pushed: &[Oid],
) -> Result<()> {
    if mr.is_draft() {
        return Ok(());
    }
    let r = repo::repo(c, p);
    for o in pushed {
        let title = r.commit(o)?.title();
        let t = title.trim_start();
        // `Commit::DRAFT_REGEX`: a Draft prefix, or `fixup!`/`squash!` and whitespace anywhere.
        let fixup = ["fixup!", "squash!"].iter().any(|k| t.match_indices(k).any(|(i, _)| t[i + k.len()..].starts_with(char::is_whitespace)));
        if super::is_draft_title(t) || fixup {
            let new_title = super::draft_title(&mr.title);
            c.execute(
                "UPDATE merge_requests SET title = ?1 WHERE id = ?2",
                params![new_title, mr.id],
            )?;
            notes::system_note(
                c,
                mr.id,
                p.id,
                user,
                &format!(
                    "marked this merge request as **draft** from {}",
                    o.hex()
                ),
                "title",
            )?;
            break;
        }
    }
    Ok(())
}

/// `Gitlab::BranchPushMergeCommitAnalyzer`: for each pushed commit, the commit that merged it
/// into the branch. The new head and its first-parent ancestors are their own merge commits;
/// any other commit takes the merge commit of its first child seen (newest first).
fn merge_commit_analysis(r: &labrat_git::Repo, pushed_oldest_first: &[labrat_git::Oid]) -> Result<HashMap<String, String>> {
    let newest_first: Vec<&labrat_git::Oid> = pushed_oldest_first.iter().rev().collect();
    let mut out: HashMap<String, String> = HashMap::new();
    let Some(head) = newest_first.first() else { return Ok(out) };
    let in_push: HashSet<String> = newest_first.iter().map(|o| o.hex()).collect();
    let mut cur = **head;
    loop {
        out.insert(cur.hex(), cur.hex());
        let cm = r.commit(&cur)?;
        match cm.parents.first() {
            Some(p) if in_push.contains(&p.hex()) => cur = *p,
            _ => break,
        }
    }
    for oid in &newest_first {
        let cm = r.commit(oid)?;
        let Some(mine) = out.get(&oid.hex()).cloned() else { continue };
        for parent in &cm.parents {
            if in_push.contains(&parent.hex()) {
                out.entry(parent.hex()).or_insert(mine.clone());
            }
        }
    }
    Ok(out)
}

/// `Commit#title`: "No commit message" for an empty message, and titles of 100 or more
/// characters cut with Rails' `truncate(81, separator: ' ', omission: '...')`.
pub fn gitlab_title(full: &str) -> String {
    if full.trim().is_empty() {
        return "No commit message".into();
    }
    let chars: Vec<char> = full.chars().collect();
    if chars.len() < 100 {
        return full.to_string();
    }
    let stop = 81 - 3;
    // `rindex(separator, stop)` looks at index `stop` too; without a space the cut is at `stop`.
    let cut = chars[..=stop].iter().rposition(|c| *c == ' ').unwrap_or(stop);
    format!("{}...", chars[..cut].iter().collect::<String>())
}

/// `MergeRequestsFinder(merged_without_event_source: true, state: merged, sort: merged_at,
/// commit_sha: sha).first`.
fn source_merge_request(c: &Connection, p: &Project, exclude: i64, sha: &str) -> Result<Option<i64>> {
    let candidates: Vec<i64> = c
        .prepare(
            "SELECT m.id FROM merge_requests m WHERE m.target_project_id = ?1 AND m.state = 'merged' AND m.id != ?2
               AND EXISTS (SELECT 1 FROM resource_state_events e WHERE e.merge_request_id = m.id AND e.state = 'merged'
                           AND e.source_commit IS NULL AND e.source_merge_request_id IS NULL)
             ORDER BY m.merged_at, m.id",
        )?
        .query_map(params![p.id, exclude], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let r = repo::repo(c, p);
    for id in candidates {
        let m = MergeRequest::get(c, id)?;
        if m.merge_commit_sha.as_deref() == Some(sha) || m.squash_commit_sha.as_deref() == Some(sha) {
            return Ok(Some(id));
        }
        if let Some(d) = m.latest_diff(c)?
            && d.commits(&r)?.iter().any(|o| o.hex() == sha)
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}
