//! Repository entities: Commit, CommitDetail, Branch, Tag, Diff, TreeObject, Compare,
//! ProtectedBranch.

use labrat_core::model::{Project, User};
use labrat_core::repo::{self, ProtectedBranch};
use labrat_core::{Config, Result, time};
use labrat_git::diff::{self, FileChange};
use labrat_git::{Commit, Oid, Repo};
use rusqlite::Connection;
use serde_json::{Map, Value, json};

pub fn commit(config: &Config, p: &Project, oid: &Oid, c: &Commit) -> Map<String, Value> {
    let mut m = Map::new();
    let id = oid.hex();
    m.insert("id".into(), json!(id));
    m.insert("short_id".into(), json!(&id[..8]));
    m.insert(
        "created_at".into(),
        json!(time::iso_git(c.committer.time, c.committer.offset)),
    );
    m.insert(
        "parent_ids".into(),
        json!(c.parents.iter().map(|p| p.hex()).collect::<Vec<_>>()),
    );
    let title = c
        .message
        .split(['\r', '\n'])
        .next()
        .unwrap_or("")
        .to_string();
    m.insert(
        "title".into(),
        json!(if title.trim().is_empty() {
            "No commit message".into()
        } else {
            title
        }),
    );
    m.insert("message".into(), json!(c.message));
    m.insert("author_name".into(), json!(c.author.name));
    m.insert("author_email".into(), json!(c.author.email));
    m.insert(
        "authored_date".into(),
        json!(time::iso_git(c.author.time, c.author.offset)),
    );
    m.insert("committer_name".into(), json!(c.committer.name));
    m.insert("committer_email".into(), json!(c.committer.email));
    m.insert(
        "committed_date".into(),
        json!(time::iso_git(c.committer.time, c.committer.offset)),
    );
    m.insert("trailers".into(), json!({}));
    m.insert("extended_trailers".into(), json!({}));
    m.insert(
        "web_url".into(),
        json!(config.url(&format!("/{}/-/commit/{id}", p.full_path))),
    );
    m
}

/// `{additions, deletions, total}` for a commit against its first parent.
pub fn stats(r: &Repo, c: &Commit) -> Result<Value> {
    let parent_tree = match c.parents.first() {
        Some(p) => Some(r.commit(p)?.tree),
        None => None,
    };
    let changes = diff::diff_trees(r, parent_tree.as_ref(), Some(&c.tree))?;
    let (mut add, mut del) = (0, 0);
    for ch in &changes {
        let (a, d) = diff::line_stats(r, ch)?;
        add += a;
        del += d;
    }
    Ok(json!({"additions": add, "deletions": del, "total": add + del}))
}

/// `CommitDetail`.
pub fn commit_detail(
    config: &Config,
    r: &Repo,
    p: &Project,
    oid: &Oid,
    c: &Commit,
    with_stats: bool,
) -> Result<Value> {
    let mut m = commit(config, p, oid, c);
    if with_stats {
        m.insert("stats".into(), stats(r, c)?);
    }
    m.insert("status".into(), Value::Null);
    m.insert("project_id".into(), json!(p.id));
    m.insert("last_pipeline".into(), Value::Null);
    Ok(Value::Object(m))
}

/// How a view treats large patches (`diff_max_patch_bytes` = 200 KiB, collapse at a tenth).
#[derive(Clone, Copy, PartialEq)]
pub enum DiffMode {
    /// Commit diffs and MR versions: only `too_large` (hard limit) blanks the patch.
    Expanded,
    /// Compare: Gitaly collapses anything over the safe limit, before `too_large` applies.
    Collapsed,
    /// MR `/diffs` and `/changes`: `too_large` over the hard limit, `collapsed` over the safe one.
    MergeRequest,
}

const PATCH_HARD_LIMIT: usize = 204_800;
const PATCH_SAFE_LIMIT: usize = PATCH_HARD_LIMIT / 10;

/// How an MR diff list is read: `/diffs` and `/changes` collapse, versions and
/// `access_raw_diffs` don't.
#[derive(Clone, Copy, PartialEq)]
pub enum MrDiffView {
    Collapsing,
    Expanded,
}

/// Linguist-style generated files GitLab recognises by name (`Gitaly` `CheckBlobsGenerated`).
const GENERATED_NAMES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "composer.lock",
    "Cargo.lock",
    "Gopkg.lock",
    "glide.lock",
    "poetry.lock",
    "Pipfile.lock",
];

/// `generated_file` for an MR diff: `gitlab-generated`/`linguist-generated` in the target's
/// `.gitattributes` (a negated attribute wins over the name rules), then well-known generated
/// file names, and Go files marked `Code generated ... DO NOT EDIT.` (checked on the old blob
/// for a changed file).
fn is_generated(r: &Repo, attrs: &labrat_git::attributes::Attributes, ch: &FileChange) -> bool {
    use labrat_git::attributes::Setting;
    let path = if ch.deleted_file { &ch.old_path } else { &ch.new_path };
    for attr in ["gitlab-generated", "linguist-generated"] {
        match attrs.get(&ch.old_path, attr) {
            Some(Setting::Set) => return true,
            Some(Setting::Value(v)) if v != "false" => return true,
            Some(_) => return false,
            None => {}
        }
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    if GENERATED_NAMES.contains(&base) {
        return true;
    }
    if path.ends_with(".go") {
        let oid = if ch.new_file { ch.new_oid } else { ch.old_oid };
        let head = r.blob(&oid).unwrap_or_default();
        let text = String::from_utf8_lossy(&head[..head.len().min(1024)]).into_owned();
        return text.lines().take(5).any(|l| l.contains("Code generated") && l.contains("DO NOT EDIT"));
    }
    false
}

/// The `.gitattributes` at the MR's target tip.
pub fn target_attributes(r: &Repo, target: Option<labrat_git::Oid>) -> labrat_git::attributes::Attributes {
    let text = target
        .and_then(|t| r.commit(&t).ok())
        .and_then(|c| r.entry_at(&c.tree, ".gitattributes").ok().flatten())
        .and_then(|e| r.blob(&e.oid).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    labrat_git::attributes::Attributes::parse(&text)
}

/// Renders files `skip..skip+take` of an MR diff the way `Gitlab::Git::DiffCollection` does:
/// a patch of 20 KiB or more collapses (also generated files), files after the cumulative
/// safe limits (100 files, 5000 lines, 512,000 bytes; counted over post-collapse sizes and
/// including the skipped files) collapse too, and a list of exactly one file is expanded.
pub fn mr_diff_list(
    r: &Repo,
    target: Option<labrat_git::Oid>,
    changes: &[FileChange],
    skip: usize,
    take: usize,
    view: MrDiffView,
) -> Result<Vec<Value>> {
    let attrs = target_attributes(r, target);
    let end = (skip + take).min(changes.len());
    let loaded = &changes[..end];
    let expanded = view == MrDiffView::Expanded || loaded.len() == 1;
    let (mut lines, mut bytes, mut yielded) = (0usize, 0usize, 0usize);
    let mut out = vec![];
    for (idx, ch) in loaded.iter().enumerate() {
        let generated = is_generated(r, &attrs, ch);
        let text = json_safe(diff::change_patch(r, ch)?, ch);
        let size = text.len();
        let too_large = size >= PATCH_HARD_LIMIT;
        let mut collapsed = !too_large && !expanded && (size >= PATCH_SAFE_LIMIT || generated);
        let mut shown = if too_large || collapsed { String::new() } else { text };
        let visible = idx >= skip;
        if visible && !expanded {
            let over = yielded >= 100 || lines > 5000 || bytes >= 512_000;
            if over && !shown.is_empty() {
                collapsed = true;
                shown = String::new();
            }
        }
        lines += shown.lines().count();
        bytes += shown.len();
        if visible {
            yielded += 1;
            out.push(json!({
                "diff": shown,
                "collapsed": collapsed,
                "too_large": too_large,
                "new_path": ch.new_path,
                "old_path": ch.old_path,
                "a_mode": ch.a_mode,
                "b_mode": ch.b_mode,
                "new_file": ch.new_file,
                "renamed_file": ch.renamed_file,
                "deleted_file": ch.deleted_file,
                "generated_file": generated,
            }));
        }
    }
    Ok(out)
}

/// `unidiff=true`: textual patches get `---`/`+++` file headers.
pub fn unidiff(v: &mut Value, ch: &FileChange) {
    let text = v["diff"].as_str().unwrap_or_default();
    if text.is_empty() || text.starts_with("Binary files") {
        return;
    }
    let a = if ch.new_file { "/dev/null".to_string() } else { format!("a/{}", ch.old_path) };
    let b = if ch.deleted_file { "/dev/null".to_string() } else { format!("b/{}", ch.new_path) };
    v["diff"] = json!(format!("--- {a}\n+++ {b}\n{text}"));
}

pub fn diff(r: &Repo, ch: &FileChange) -> Result<Value> {
    diff_with(r, ch, DiffMode::Expanded)
}

/// `Gitlab::Git::Diff#json_safe_diff`: a patch that itself contains NUL bytes (git saw text,
/// e.g. a NUL past the first 8000 bytes) is replaced by Rails' own binary notice, without the
/// `a/`/`b/` prefixes git uses.
pub fn json_safe(text: String, ch: &FileChange) -> String {
    if text.as_bytes().contains(&0) {
        format!("Binary files {} and {} differ\n", ch.old_path, ch.new_path)
    } else {
        text
    }
}

/// Compare (`collapse_diffs`): Gitaly collapses a patch of 20 KiB or more, and once the files
/// shown so far pass the safe limits (100 files, 5000 lines, 512,000 bytes) it collapses the
/// file that crossed them and every file after it.
pub fn compare_diff_list(r: &Repo, changes: &[FileChange]) -> Result<Vec<Value>> {
    let (mut files, mut lines, mut bytes, mut over) = (0usize, 0usize, 0usize, false);
    let mut out = vec![];
    for ch in changes {
        // Gitaly collapses before Rails' too-large check, so a huge patch is `collapsed`.
        let text = json_safe(diff::change_patch(r, ch)?, ch);
        let mut v = diff_with(r, ch, DiffMode::Collapsed)?;
        let mut collapse = over && !text.is_empty();
        if !collapse && text.len() >= PATCH_SAFE_LIMIT {
            collapse = true;
        } else if !collapse && !text.is_empty() {
            files += 1;
            lines += text.lines().count();
            bytes += text.len();
            if files > 100 || lines > 5000 || bytes > 512_000 {
                over = true;
                collapse = true;
            }
        }
        if collapse {
            v["diff"] = json!("");
            v["collapsed"] = json!(true);
        }
        out.push(v);
    }
    Ok(out)
}

pub fn diff_with(r: &Repo, ch: &FileChange, mode: DiffMode) -> Result<Value> {
    let text = json_safe(diff::change_patch(r, ch)?, ch);
    let size = text.len();
    let (too_large, collapsed) = match mode {
        DiffMode::Expanded => (size >= PATCH_HARD_LIMIT, false),
        DiffMode::Collapsed => (false, size >= PATCH_SAFE_LIMIT),
        DiffMode::MergeRequest => (size >= PATCH_HARD_LIMIT, (PATCH_SAFE_LIMIT..PATCH_HARD_LIMIT).contains(&size)),
    };
    Ok(json!({
        "diff": if too_large || collapsed { String::new() } else { text },
        "collapsed": collapsed,
        "too_large": too_large,
        "new_path": ch.new_path,
        "old_path": ch.old_path,
        "a_mode": ch.a_mode,
        "b_mode": ch.b_mode,
        "new_file": ch.new_file,
        "renamed_file": ch.renamed_file,
        "deleted_file": ch.deleted_file,
        "generated_file": null,
    }))
}

pub fn branch(
    c: &Connection,
    config: &Config,
    p: &Project,
    user: Option<&User>,
    name: &str,
    oid: &Oid,
    merged: Option<bool>,
) -> Result<Value> {
    let r = repo::repo(c, p);
    let commit_obj = r.commit(oid)?;
    let rules = repo::matching_rules(c, p, name)?;
    let protected = repo::is_protected(c, p, name)?;
    let can_push = match user {
        Some(u) => repo::can_push_to_branch(c, u, p, name)?,
        None => false,
    };
    Ok(json!({
        "name": name,
        "commit": Value::Object(commit(config, p, oid, &commit_obj)),
        "merged": merged,
        "protected": protected,
        "developers_can_push": repo::developers_can(&rules, false),
        "developers_can_merge": repo::developers_can(&rules, true),
        "can_push": can_push,
        "default": p.default_branch.as_deref() == Some(name),
        "web_url": config.url(&format!("/{}/-/tree/{name}", p.full_path)),
    }))
}

/// `Entities::Tag`. `created` marks the response of tag creation, where GitLab has no date and
/// no release field.
pub fn tag(
    config: &Config,
    r: &Repo,
    p: &Project,
    name: &str,
    target: &Oid,
    created: bool,
) -> Result<Value> {
    let peeled = r.peel(target)?;
    let c = r.commit(&peeled)?;
    let (message, date) = match r.read(target)? {
        (labrat_git::Kind::Tag, data) => {
            let t = labrat_git::Tag::parse(&data)?;
            let date = t.tagger.as_ref().map(|s| time::iso_git(s.time, s.offset));
            (t.message.trim_end_matches('\n').to_string(), date)
        }
        _ => (String::new(), None),
    };
    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("message".into(), json!(message));
    m.insert("target".into(), json!(target.hex()));
    m.insert(
        "commit".into(),
        Value::Object(commit(config, p, &peeled, &c)),
    );
    // On create the tag is presented without a current user: `release` shows only when anyone
    // may read releases (a public project).
    if !created || p.visibility_level == 20 {
        m.insert("release".into(), Value::Null);
    }
    m.insert("protected".into(), json!(labrat_core::services::protected_tags::is_protected(r.c, p.id, name)?));
    m.insert(
        "created_at".into(),
        if created { Value::Null } else { json!(date) },
    );
    Ok(Value::Object(m))
}

pub fn protected_branch(pb: &ProtectedBranch, ee: bool) -> Value {
    let level = |id: i64, access_level: i64, with_deploy_key: bool| {
        let mut m = Map::new();
        m.insert("id".into(), json!(id));
        m.insert("access_level".into(), json!(access_level));
        m.insert(
            "access_level_description".into(),
            json!(match access_level {
                0 => "No one",
                30 => "Developers + Maintainers",
                60 => "Instance admins",
                _ => "Maintainers",
            }),
        );
        if with_deploy_key {
            m.insert("deploy_key_id".into(), Value::Null);
        }
        if ee {
            // EE allows specific users and groups; labrat has only role levels.
            m.insert("user_id".into(), Value::Null);
            m.insert("group_id".into(), Value::Null);
        }
        Value::Object(m)
    };
    let mut m = Map::new();
    m.insert("id".into(), json!(pb.id));
    m.insert("name".into(), json!(pb.name));
    m.insert("push_access_levels".into(), json!(pb.push.iter().map(|l| level(l.id, l.access_level, true)).collect::<Vec<_>>()));
    m.insert("merge_access_levels".into(), json!(pb.merge.iter().map(|l| level(l.id, l.access_level, false)).collect::<Vec<_>>()));
    m.insert("allow_force_push".into(), json!(pb.allow_force_push));
    if ee {
        // EE's "allowed to unprotect" level; it borrows the branch's id.
        let unprotect: Vec<Value> = pb.unprotect_access_level.map(|l| level(pb.id, l, false)).into_iter().collect();
        m.insert("unprotect_access_levels".into(), json!(unprotect));
        m.insert("code_owner_approval_required".into(), json!(pb.code_owner_approval_required));
        m.insert("inherited".into(), json!(false));
    }
    Value::Object(m)
}
