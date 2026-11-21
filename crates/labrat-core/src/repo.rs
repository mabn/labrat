//! Repository-level rules on top of the git store: protected branches, push and merge
//! permissions, commits created through the API, and what happens when branches appear.

use crate::model::{Namespace, Project, User, access};
use crate::perms::ProjectPerms;
use crate::{Error, Result, time};
use labrat_git::merge::{FileMap, write_tree};
use labrat_git::object::{MODE_EXEC, MODE_FILE};
use labrat_git::{Commit, Kind, Oid, Repo, Signature};
use rusqlite::{Connection, params};

pub fn repo<'a>(c: &'a Connection, p: &Project) -> Repo<'a> {
    Repo::new(c, p.id)
}

/// The commit signature GitLab uses for a user acting through the API or UI.
pub fn signature(user: &User) -> Signature {
    Signature {
        name: user.name.clone(),
        email: user.commit_email.clone().unwrap_or(user.email.clone()),
        time: time::now() / 1000,
        offset: 0,
    }
}

/// Default branch name used when the repository has none yet.
pub const DEFAULT_BRANCH: &str = "main";

pub fn default_branch(p: &Project) -> String {
    p.default_branch.clone().unwrap_or(DEFAULT_BRANCH.into())
}

#[derive(Debug, Clone)]
pub struct AccessLevel {
    pub id: i64,
    pub access_level: i64,
}

#[derive(Debug, Clone)]
pub struct ProtectedBranch {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub allow_force_push: bool,
    pub code_owner_approval_required: bool,
    pub push: Vec<AccessLevel>,
    pub merge: Vec<AccessLevel>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn protected_branches(c: &Connection, project_id: i64) -> Result<Vec<ProtectedBranch>> {
    let mut st = c.prepare(
        "SELECT id, project_id, name, allow_force_push, created_at, updated_at, code_owner_approval_required FROM protected_branches
         WHERE project_id = ?1 ORDER BY id",
    )?;
    let rows = st
        .query_map([project_id], |r| {
            Ok(ProtectedBranch {
                id: r.get(0)?,
                project_id: r.get(1)?,
                name: r.get(2)?,
                allow_force_push: r.get(3)?,
                code_owner_approval_required: r.get(6)?,
                push: vec![],
                merge: vec![],
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = vec![];
    for mut pb in rows {
        let mut st = c.prepare(
            "SELECT id, kind, access_level FROM protected_branch_access_levels WHERE protected_branch_id = ?1 ORDER BY id",
        )?;
        let levels = st
            .query_map([pb.id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, kind, level) in levels {
            let l = AccessLevel {
                id,
                access_level: level,
            };
            match kind.as_str() {
                "push" => pb.push.push(l),
                _ => pb.merge.push(l),
            }
        }
        out.push(pb);
    }
    Ok(out)
}

/// `RefMatcher`: `*` matches any characters (including `/`); everything else is literal.
pub fn ref_matches(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == name;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !name.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            return name.len() >= pos + part.len() && name[pos..].ends_with(part);
        } else {
            match name[pos..].find(part) {
                Some(j) => pos += j + part.len(),
                None => return false,
            }
        }
    }
    true
}

pub fn matching_rules(c: &Connection, p: &Project, branch: &str) -> Result<Vec<ProtectedBranch>> {
    Ok(protected_branches(c, p.id)?
        .into_iter()
        .filter(|r| ref_matches(&r.name, branch))
        .collect())
}

/// Default branch protection for new default branches: (push level, merge level, force push,
/// developer can initial push). GitLab's instance default is "fully protected".
pub fn default_protection(c: &Connection, p: &Project) -> Result<(i64, i64, bool, bool)> {
    let ns = Namespace::find(c, p.namespace_id)?;
    if let Some(ns) = ns.filter(|n| n.is_group())
        && let Some(d) = ns.settings.get("default_branch_protection_defaults") {
            let level = |k: &str| -> i64 {
                d[k].as_array()
                    .map(|a| {
                        let levels: Vec<i64> = a
                            .iter()
                            .filter_map(|x| x["access_level"].as_i64())
                            .collect();
                        if levels.contains(&0) {
                            0
                        } else if levels.contains(&30) {
                            30
                        } else if levels.contains(&40) {
                            40
                        } else {
                            60
                        }
                    })
                    .unwrap_or(40)
            };
            return Ok((
                level("allowed_to_push"),
                level("allowed_to_merge"),
                d["allow_force_push"].as_bool().unwrap_or(false),
                d["developer_can_initial_push"].as_bool().unwrap_or(false),
            ));
        }
    Ok((access::MAINTAINER, access::MAINTAINER, false, false))
}

/// `ProtectedBranch.protected?`: a matching rule, or any branch while the repo is empty and
/// developers can't push to the default branch.
pub fn is_protected(c: &Connection, p: &Project, branch: &str) -> Result<bool> {
    if !matching_rules(c, p, branch)?.is_empty() {
        return Ok(true);
    }
    if repo(c, p).is_empty()? {
        let (push, _, _, _) = default_protection(c, p)?;
        return Ok(push != access::DEVELOPER);
    }
    Ok(false)
}

fn level_accepts(level: i64, perms: &ProjectPerms, user: &User) -> bool {
    match level {
        0 => false,
        60 => user.admin,
        l => user.admin || perms.level >= l,
    }
}

pub fn can_push_to_branch(c: &Connection, user: &User, p: &Project, branch: &str) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !perms.developer() || p.archived {
        return Ok(false);
    }
    let rules = matching_rules(c, p, branch)?;
    if rules.is_empty() {
        if repo(c, p).is_empty()? {
            let (push, _, _, initial) = default_protection(c, p)?;
            return Ok(perms.admin_project()
                || push == access::DEVELOPER
                || initial
                || perms.level >= push);
        }
        return Ok(true);
    }
    Ok(rules
        .iter()
        .flat_map(|r| r.push.iter())
        .any(|l| level_accepts(l.access_level, &perms, user)))
}

pub fn can_merge_to_branch(c: &Connection, user: &User, p: &Project, branch: &str) -> Result<bool> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !perms.developer() || p.archived {
        return Ok(false);
    }
    let rules = matching_rules(c, p, branch)?;
    if rules.is_empty() {
        return Ok(true);
    }
    Ok(rules
        .iter()
        .flat_map(|r| r.merge.iter())
        .any(|l| level_accepts(l.access_level, &perms, user)))
}

pub fn developers_can(rules: &[ProtectedBranch], merge: bool) -> bool {
    rules
        .iter()
        .flat_map(|r| if merge { r.merge.iter() } else { r.push.iter() })
        .any(|l| l.access_level == access::DEVELOPER)
}

pub fn create_protected_branch(
    c: &Connection,
    p: &Project,
    name: &str,
    push: i64,
    merge: i64,
    allow_force_push: bool,
) -> Result<i64> {
    let now = time::now();
    c.execute(
        "INSERT INTO protected_branches (project_id, name, allow_force_push, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![p.id, name, allow_force_push, now],
    )?;
    let id = c.last_insert_rowid();
    for (kind, level) in [("push", push), ("merge", merge)] {
        c.execute(
            "INSERT INTO protected_branch_access_levels (protected_branch_id, kind, access_level) VALUES (?1, ?2, ?3)",
            params![id, kind, level],
        )?;
    }
    Ok(id)
}

/// Runs when a branch is created by a push or the API (`BranchHooksService`): the first
/// branch becomes the default branch, which then gets default protection.
pub fn after_branch_created(c: &Connection, p: &Project, branch: &str) -> Result<()> {
    if p.default_branch.is_none() || p.default_branch.as_deref() == Some(branch) {
        if p.default_branch.is_none() {
            c.execute(
                "UPDATE projects SET default_branch = ?1 WHERE id = ?2",
                params![branch, p.id],
            )?;
        }
        let (push, merge, force, _) = default_protection(c, p)?;
        let fully_open = force && push == access::DEVELOPER && merge == access::DEVELOPER;
        if !fully_open && matching_rules(c, p, branch)?.is_empty() {
            create_protected_branch(c, p, branch, push, merge, force)?;
        }
    }
    Ok(())
}

/// One file action of the commits API.
#[derive(Debug, Clone)]
pub enum FileAction {
    Create {
        path: String,
        content: Vec<u8>,
        exec: Option<bool>,
    },
    Update {
        path: String,
        content: Vec<u8>,
        exec: Option<bool>,
    },
    Delete {
        path: String,
    },
    Move {
        from: String,
        to: String,
        content: Option<Vec<u8>>,
    },
    Chmod {
        path: String,
        exec: bool,
    },
}

fn bad(msg: impl Into<String>) -> Error {
    Error::message(400, msg)
}

fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() {
        return Err(bad("You must provide a file path"));
    }
    if path.split('/').any(|seg| seg == "..") {
        return Err(bad("Path cannot include directory traversal"));
    }
    if path.starts_with('/') || path.contains("//") || path.ends_with('/') {
        return Err(bad(format!("invalid path: '{path}'")));
    }
    Ok(())
}

fn is_dir(files: &FileMap, path: &str) -> bool {
    let prefix = format!("{path}/");
    files.keys().any(|k| k.starts_with(&prefix))
}

/// Applies file actions to a flat file map.
pub fn apply_actions(repo: &Repo, files: &mut FileMap, actions: &[FileAction]) -> Result<()> {
    for a in actions {
        match a {
            FileAction::Create {
                path,
                content,
                exec,
            } => {
                validate_path(path)?;
                if is_dir(files, path) {
                    return Err(bad("A directory with this name already exists"));
                }
                if files.contains_key(path) {
                    return Err(bad("A file with this name already exists"));
                }
                // A parent path that is a file can't become a directory.
                let mut parent = path.as_str();
                while let Some((p, _)) = parent.rsplit_once('/') {
                    if files.contains_key(p) {
                        return Err(bad("A file with this name already exists"));
                    }
                    parent = p;
                }
                let oid = repo.write(Kind::Blob, content)?;
                files.insert(
                    path.clone(),
                    (
                        if exec.unwrap_or(false) {
                            MODE_EXEC
                        } else {
                            MODE_FILE
                        },
                        oid,
                    ),
                );
            }
            FileAction::Update {
                path,
                content,
                exec,
            } => {
                validate_path(path)?;
                let Some((mode, _)) = files.get(path).copied() else {
                    return Err(bad("A file with this name doesn't exist"));
                };
                let oid = repo.write(Kind::Blob, content)?;
                let mode = match exec {
                    Some(true) => MODE_EXEC,
                    Some(false) => MODE_FILE,
                    None => mode,
                };
                files.insert(path.clone(), (mode, oid));
            }
            FileAction::Delete { path } => {
                validate_path(path)?;
                if files.remove(path).is_none() {
                    return Err(bad("A file with this name doesn't exist"));
                }
            }
            FileAction::Move { from, to, content } => {
                validate_path(to)?;
                let Some((mode, oid)) = files.remove(from) else {
                    return Err(bad("A file with this name doesn't exist"));
                };
                if files.contains_key(to) {
                    return Err(bad("A file with this name already exists"));
                }
                let oid = match content {
                    Some(c) => repo.write(Kind::Blob, c)?,
                    None => oid,
                };
                files.insert(to.clone(), (mode, oid));
            }
            FileAction::Chmod { path, exec } => {
                validate_path(path)?;
                let Some((_, oid)) = files.get(path).copied() else {
                    return Err(bad("A file with this name doesn't exist"));
                };
                files.insert(
                    path.clone(),
                    (if *exec { MODE_EXEC } else { MODE_FILE }, oid),
                );
            }
        }
    }
    Ok(())
}

/// A valid git branch name (`git check-ref-format --branch`).
pub fn valid_branch_name(name: &str) -> bool {
    if name.is_empty()
        || name == "HEAD"
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
    {
        return false;
    }
    if name.ends_with(".lock")
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
    {
        return false;
    }
    if name
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
    {
        return false;
    }
    !name
        .split('/')
        .any(|seg| seg.starts_with('.') || seg.ends_with(".lock"))
}

pub struct NewCommit<'a> {
    pub branch: &'a str,
    pub start_branch: Option<&'a str>,
    pub start_sha: Option<Oid>,
    pub message: &'a str,
    pub actions: &'a [FileAction],
    pub author_name: Option<&'a str>,
    pub author_email: Option<&'a str>,
    pub force: bool,
}

/// `Commits::CreateService`: builds a commit from file actions and moves the branch.
/// Returns the new commit id.
pub fn create_commit(c: &Connection, p: &Project, user: &User, n: NewCommit) -> Result<Oid> {
    let r = repo(c, p);
    if n.message.trim().is_empty() {
        return Err(bad("You must provide a commit message"));
    }
    let existing = r.branch(n.branch)?;
    let empty = r.is_empty()?;
    let start: Option<Oid> = match (n.start_sha, n.start_branch) {
        (Some(_), Some(_)) => return Err(bad("You can't pass both start_branch and start_sha")),
        (Some(sha), None) => {
            if !r.exists(&sha)? {
                return Err(bad(format!("Cannot find start_sha '{sha}'")));
            }
            Some(sha)
        }
        (None, Some(sb)) => match r.branch(sb)? {
            Some(o) => Some(o),
            None if empty => None,
            None => {
                return Err(bad(
                    "You can only create or edit files when you are on a branch",
                ));
            }
        },
        // Without either, the start is the branch itself.
        (None, None) if existing.is_none() && !empty => {
            return Err(bad("You can only create or edit files when you are on a branch"));
        }
        (None, None) => existing,
    };
    if let (Some(e), Some(s)) = (existing, start)
        && e != s && !n.force {
            return Err(bad(format!(
                "A branch called '{}' already exists. Switch to that branch in order to make changes",
                n.branch
            )));
        }
    if existing.is_none() && !valid_branch_name(n.branch) {
        return Err(bad(format!(
            "Something went wrong when we tried to create '{}' for you: Branch name is invalid",
            n.branch
        )));
    }
    let mut files = match start {
        Some(s) => labrat_git::diff::flatten(&r, &r.commit(&s)?.tree)?,
        None => FileMap::new(),
    };
    let known_blobs: std::collections::HashSet<Oid> = files.values().map(|(_, o)| *o).collect();
    apply_actions(&r, &mut files, n.actions)?;
    // The pre-receive `LfsCheck`: new LFS pointers need their objects uploaded, and labrat has
    // no LFS storage.
    if lfs_enabled(p) {
        for (_, oid) in files.values() {
            if !known_blobs.contains(oid) && is_lfs_pointer(&r.blob(oid).unwrap_or_default()) {
                return Err(bad(LFS_MISSING));
            }
        }
    }
    let tree = write_tree(&r, &files)?;
    let committer = signature(user);
    let author = Signature {
        name: n
            .author_name
            .map(str::to_string)
            .unwrap_or(committer.name.clone()),
        email: n
            .author_email
            .map(str::to_string)
            .unwrap_or(committer.email.clone()),
        ..committer.clone()
    };
    let commit = Commit {
        tree,
        parents: start.into_iter().collect(),
        author,
        committer,
        extra: vec![],
        message: if n.message.ends_with('\n') {
            n.message.to_string()
        } else {
            n.message.to_string()
        },
    };
    let oid = r.write_commit(&commit)?;
    r.set_ref(&format!("refs/heads/{}", n.branch), &oid)?;
    let p = Project::find(c, p.id)?.expect("project exists");
    if existing.is_none() {
        after_branch_created(c, &p, n.branch)?;
    }
    Ok(oid)
}

/// Last commit on `from` that changed `path` (like `git log -1 -- path`).
pub fn last_commit_for_path(r: &Repo, from: &Oid, path: &str) -> Result<Option<Oid>> {
    let commits = labrat_git::walk::rev_list(r, &[*from], &[])?;
    for oid in commits {
        let c = r.commit(&oid)?;
        let here = r.entry_at(&c.tree, path)?.map(|e| e.oid);
        let parent_entry = match c.parents.first() {
            Some(pp) => r.entry_at(&r.commit(pp)?.tree, path)?.map(|e| e.oid),
            None => None,
        };
        if here != parent_entry {
            return Ok(Some(oid));
        }
    }
    Ok(None)
}

pub const LFS_MISSING: &str = "LFS objects are missing. Ensure LFS is properly set up or try a manual \"git lfs push --all\".";

pub fn lfs_enabled(p: &Project) -> bool {
    p.setting("lfs_enabled").and_then(serde_json::Value::as_bool).unwrap_or(true)
}

/// A git-lfs pointer file (`version https://git-lfs.github.com/spec/v1`, `oid sha256:<hex>`,
/// `size <n>`), at most 200 bytes like Gitaly's `ListLFSPointers`.
pub fn is_lfs_pointer(data: &[u8]) -> bool {
    if data.len() > 200 {
        return false;
    }
    let Ok(text) = std::str::from_utf8(data) else { return false };
    let mut lines = text.lines();
    if lines.next() != Some("version https://git-lfs.github.com/spec/v1") {
        return false;
    }
    let (mut oid, mut size) = (false, false);
    for l in lines {
        if let Some(h) = l.strip_prefix("oid sha256:") {
            oid = h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit());
        } else if let Some(n) = l.strip_prefix("size ") {
            size = n.parse::<u64>().is_ok();
        }
    }
    oid && size
}
