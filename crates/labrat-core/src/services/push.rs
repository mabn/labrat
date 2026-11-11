//! What happens after a push is accepted (`PostReceive`): default branch setup, merge
//! request updates and the messages printed to the pushing client.

use crate::model::{Project, User};
use crate::{Result, repo};
use labrat_git::protocol::RefUpdate;
use rusqlite::Connection;

/// Callbacks run for each pushed branch: (connection, project, user, update). Registered by
/// higher layers (merge requests, webhooks) so this module doesn't depend on them.
pub type BranchHook = fn(&Connection, &crate::Config, &Project, &User, &RefUpdate) -> Result<()>;

static BRANCH_HOOKS: std::sync::RwLock<Vec<BranchHook>> = std::sync::RwLock::new(Vec::new());

pub fn register_branch_hook(h: BranchHook) {
    BRANCH_HOOKS.write().unwrap().push(h);
}

/// Callback building the MR link message for a pushed branch: returns `Some((iid, exists))`.
pub type MrLinkLookup = fn(&Connection, &Project, &str) -> Result<Option<i64>>;

static MR_LOOKUP: std::sync::RwLock<Option<MrLinkLookup>> = std::sync::RwLock::new(None);

pub fn register_mr_lookup(f: MrLinkLookup) {
    *MR_LOOKUP.write().unwrap() = Some(f);
}

/// Runs post-receive processing and returns the messages for the client.
pub fn post_receive(
    c: &Connection,
    config: &crate::Config,
    project: &Project,
    user: &User,
    updates: &[RefUpdate],
) -> Result<String> {
    for u in updates {
        if let Some(branch) = u.name.strip_prefix("refs/heads/")
            && u.old.is_zero() && !u.new.is_zero() {
                let p = Project::find(c, project.id)?.expect("project exists");
                repo::after_branch_created(c, &p, branch)?;
            }
    }
    let project = Project::find(c, project.id)?.expect("project exists");
    let hooks: Vec<BranchHook> = BRANCH_HOOKS.read().unwrap().clone();
    // Group refs by type and action, as `Git::ProcessRefChangesService` does.
    let group = |u: &RefUpdate| -> (bool, u8) {
        let tag = u.name.starts_with("refs/tags/");
        let action = if u.old.is_zero() { 0 } else if u.new.is_zero() { 1 } else { 2 };
        (tag, action)
    };
    for u in updates {
        let size = updates.iter().filter(|o| group(o) == group(u)).count();
        crate::services::hooks::SKIP_PUSH_HOOKS.with(|s| s.set(size > crate::services::hooks::PUSH_EVENT_HOOKS_LIMIT));
        for h in &hooks {
            let r = h(c, config, &project, user, u);
            if r.is_err() {
                crate::services::hooks::SKIP_PUSH_HOOKS.with(|s| s.set(false));
            }
            r?;
        }
    }
    crate::services::hooks::SKIP_PUSH_HOOKS.with(|s| s.set(false));
    crate::services::hooks::repository_update(c, config, &project, user, updates)?;
    mr_link_messages(c, config, &project, updates)
}

/// `post_receive` for a git push with options: the push is processed (merge request refresh
/// included) before `push_options` creates or updates merge requests, so an MR created by the
/// push gets no "added commits" or "restored source branch" notes; the MR links come last.
pub fn post_receive_with_options(
    c: &Connection,
    config: &crate::Config,
    project: &Project,
    user: &User,
    updates: &[RefUpdate],
    push_options: impl FnOnce(&Connection) -> Result<Option<String>>,
) -> Result<String> {
    // Pushes to branches that all have an open MR update those MRs from the options right away
    // (in `PostReceive`), while their refresh comes later from `UpdateMergeRequestsWorker`.
    let lookup = *MR_LOOKUP.read().unwrap();
    let mut existing = !updates.is_empty();
    for u in updates {
        let has = match (u.name.strip_prefix("refs/heads/"), lookup) {
            (Some(b), Some(f)) if !u.new.is_zero() && !u.old.is_zero() => f(c, project, b)?.is_some(),
            _ => false,
        };
        existing &= has;
    }
    let alert = if existing {
        let alert = push_options(c)?;
        crate::services::merge_requests::refresh::DEFER_REFRESH.with(|d| d.set(true));
        let r = post_receive(c, config, project, user, updates);
        crate::services::merge_requests::refresh::DEFER_REFRESH.with(|d| d.set(false));
        r?;
        alert
    } else {
        post_receive(c, config, project, user, updates)?;
        push_options(c)?
    };
    let project = Project::find(c, project.id)?.expect("project exists");
    Ok(format!("{}{}", alert.unwrap_or_default(), mr_link_messages(c, config, &project, updates)?))
}

fn mr_link_messages(
    c: &Connection,
    config: &crate::Config,
    project: &Project,
    updates: &[RefUpdate],
) -> Result<String> {
    if !project.printing_merge_request_link_enabled
        || project.feature_level("merge_requests") == crate::model::feature::DISABLED
    {
        return Ok(String::new());
    }
    let base = &config.external_url;
    let lookup = *MR_LOOKUP.read().unwrap();
    let mut out = String::new();
    for u in updates {
        let Some(branch) = u.name.strip_prefix("refs/heads/") else {
            continue;
        };
        if u.new.is_zero() || project.default_branch.as_deref() == Some(branch) {
            continue;
        }
        let existing = match lookup {
            Some(f) => f(c, project, branch)?,
            None => None,
        };
        match existing {
            Some(iid) => out.push_str(&format!(
                "\nView merge request for {branch}:\n  {base}/{}/-/merge_requests/{iid}\n",
                project.full_path
            )),
            None => {
                let enc: String = url_encode(branch);
                out.push_str(&format!(
                    "\nTo create a merge request for {branch}, visit:\n  {base}/{}/-/merge_requests/new?merge_request%5Bsource_branch%5D={enc}\n",
                    project.full_path
                ));
            }
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
