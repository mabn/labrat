//! EE code owners as merge requests see them. labrat doesn't read `CODEOWNERS` files: a test
//! endpoint sets each project's code owners per branch (sections of path patterns with their
//! owners), and merge requests get `code_owner` approval rules from them the way EE derives
//! them from the file on the target branch.
//!
//! What follows the EE 18.11.14 reference (`fixtures-ee/code_owners`):
//! - one rule per entry that owns a file the MR changes, named after the pattern, with the
//!   section's name (`codeowners` for the unnamed one) and count (`[N]`, else 0);
//! - within a section the last matching pattern owns a file;
//! - the rules are synced after the open hook, after a push (after the push's update hook), and
//!   when the code owners change; a synced rule keeps its id;
//! - with `selective_code_owner_removals`, a push takes back only the approvals of the owners
//!   of the files it changed.

use crate::model::{Namespace, Project, User};
use crate::services::approvals::{self as ap, SystemRule};
use crate::services::merge_requests::MergeRequest;
use crate::{Config, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

pub const DEFAULT_SECTION: &str = "codeowners";

#[derive(Debug, Clone)]
pub struct Entry {
    pub pattern: String,
    pub user_ids: Vec<i64>,
    pub group_ids: Vec<i64>,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub approvals: i64,
    pub optional: bool,
    pub entries: Vec<Entry>,
}

/// Resolves the test endpoint's sections (`{"name", "approvals", "optional", "entries":
/// [{"pattern", "owners": ["@user", "@group/path"]}]}`). Unknown owners are skipped, as GitLab
/// skips owners it can't find.
pub fn parse(c: &Connection, v: &Value) -> Result<std::result::Result<Vec<Section>, String>> {
    let Some(list) = v.as_array() else { return Ok(Err("sections must be an array".into())) };
    let mut out = vec![];
    for s in list {
        let name = s["name"].as_str().unwrap_or(DEFAULT_SECTION).to_string();
        let mut entries = vec![];
        for e in s["entries"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let Some(pattern) = e["pattern"].as_str() else { return Ok(Err("each entry needs a pattern".into())) };
            let (mut user_ids, mut group_ids) = (vec![], vec![]);
            for o in e["owners"].as_array().map(Vec::as_slice).unwrap_or_default() {
                let Some(o) = o.as_str() else { continue };
                let name = o.trim_start_matches('@');
                if let Some(u) = User::find_by_username(c, name)? {
                    user_ids.push(u.id);
                } else if let Some(g) = Namespace::find_by_full_path(c, name)?.filter(|n| n.kind == "Group") {
                    group_ids.push(g.id);
                }
            }
            entries.push(Entry { pattern: pattern.to_string(), user_ids, group_ids });
        }
        out.push(Section { name, approvals: s["approvals"].as_i64().unwrap_or(0), optional: s["optional"].as_bool().unwrap_or(false), entries });
    }
    Ok(Ok(out))
}

fn to_json(sections: &[Section]) -> Value {
    json!(sections
        .iter()
        .map(|s| json!({
            "name": s.name, "approvals": s.approvals, "optional": s.optional,
            "entries": s.entries.iter().map(|e| json!({"pattern": e.pattern, "user_ids": e.user_ids, "group_ids": e.group_ids})).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>())
}

fn from_json(v: &Value) -> Vec<Section> {
    let ids = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    v.as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|s| Section {
            name: s["name"].as_str().unwrap_or(DEFAULT_SECTION).to_string(),
            approvals: s["approvals"].as_i64().unwrap_or(0),
            optional: s["optional"].as_bool().unwrap_or(false),
            entries: s["entries"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|e| Entry { pattern: e["pattern"].as_str().unwrap_or_default().to_string(), user_ids: ids(&e["user_ids"]), group_ids: ids(&e["group_ids"]) })
                .collect(),
        })
        .collect()
}

/// The code owners of `branch`, if set.
pub fn config(c: &Connection, project_id: i64, branch: &str) -> Result<Option<Vec<Section>>> {
    let raw: Option<String> = c
        .query_row("SELECT sections FROM code_owner_configs WHERE project_id = ?1 AND branch = ?2", params![project_id, branch], |r| r.get(0))
        .optional()?;
    Ok(raw.map(|r| from_json(&serde_json::from_str(&r).unwrap_or(Value::Null))))
}

/// Sets (or, with `None`, clears) the code owners of `branch`, and syncs the rules of the
/// open MRs targeting it.
pub fn set_config(c: &Connection, p: &Project, branch: &str, sections: Option<&[Section]>) -> Result<()> {
    match sections {
        Some(s) => c.execute(
            "INSERT INTO code_owner_configs (project_id, branch, sections) VALUES (?1, ?2, ?3)
             ON CONFLICT (project_id, branch) DO UPDATE SET sections = excluded.sections",
            params![p.id, branch, to_json(s).to_string()],
        )?,
        None => c.execute("DELETE FROM code_owner_configs WHERE project_id = ?1 AND branch = ?2", params![p.id, branch])?,
    };
    let ids: Vec<i64> = {
        let mut st = c.prepare("SELECT id FROM merge_requests WHERE target_project_id = ?1 AND target_branch = ?2 AND state = 'opened' ORDER BY id")?;
        st.query_map(params![p.id, branch], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    for id in ids {
        sync(c, p, &MergeRequest::get(c, id)?)?;
    }
    Ok(())
}

/// Glob matching for one path segment run: `*` stays within a segment, `**` crosses them.
fn glob(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') if p.get(1) == Some(&b'*') => {
            let rest = p[2..].strip_prefix(b"/").unwrap_or(&p[2..]);
            (0..=s.len()).any(|i| glob(rest, &s[i..]))
        }
        Some(b'*') => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| glob(&p[1..], &s[i..])),
        Some(b'?') => !s.is_empty() && s[0] != b'/' && glob(&p[1..], &s[1..]),
        Some(c) => s.first() == Some(c) && glob(&p[1..], &s[1..]),
    }
}

/// Whether a CODEOWNERS pattern matches `path` (gitignore-style): `/` at the start anchors it
/// at the root, otherwise it may match at any depth; a pattern naming a directory (with or
/// without a trailing `/`) owns everything under it.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" {
        return true;
    }
    let anchored = pattern.starts_with('/') || pattern.trim_end_matches('/').contains('/');
    let pat = pattern.trim_start_matches('/');
    let dir_only = pat.ends_with('/');
    let pat = pat.trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').collect();
    // Each prefix of the path is a candidate (the file itself, or a directory above it).
    let starts: Vec<usize> = if anchored { vec![0] } else { (0..segments.len()).collect() };
    for start in starts {
        for end in start + 1..=segments.len() {
            let is_file = end == segments.len();
            if dir_only && is_file {
                continue;
            }
            if glob(pat.as_bytes(), segments[start..end].join("/").as_bytes()) {
                return true;
            }
        }
    }
    false
}

/// For each section, the index of the entry owning `path` (the last matching one).
fn owners_of(sections: &[Section], path: &str) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for (si, s) in sections.iter().enumerate() {
        if let Some(ei) = s.entries.iter().rposition(|e| matches(&e.pattern, path)) {
            out.push((si, ei));
        }
    }
    out
}

/// The paths the MR's latest diff touches, sorted.
fn changed_paths(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Vec<String>> {
    let Some(d) = mr.latest_diff(c)? else { return Ok(vec![]) };
    let r = crate::repo::repo(c, p);
    let mut out = vec![];
    for ch in d.changes(&r)? {
        out.push(ch.new_path.clone());
        if ch.old_path != ch.new_path {
            out.push(ch.old_path.clone());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Brings the MR's code owner rules in line with its changes and the target branch's code
/// owners.
pub fn sync(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<()> {
    let existing: Vec<ap::Rule> = ap::mr_rules(c, mr)?.into_iter().filter(ap::Rule::is_code_owner).collect();
    let sections = if mr.is_open() { config(c, p.id, &mr.target_branch)?.unwrap_or_default() } else { vec![] };
    let mut wanted: Vec<(usize, usize)> = vec![];
    for path in changed_paths(c, p, mr)? {
        for o in owners_of(&sections, &path) {
            if !wanted.contains(&o) {
                wanted.push(o);
            }
        }
    }
    let mut kept = vec![];
    for (si, ei) in wanted {
        let (s, e) = (&sections[si], &sections[si].entries[ei]);
        let rule = SystemRule {
            name: &e.pattern,
            approvals_required: s.approvals,
            rule_type: "code_owner",
            report_type: None,
            section: Some(&s.name),
            optional: s.optional,
            user_ids: &e.user_ids,
            group_ids: &e.group_ids,
        };
        match existing.iter().find(|r| r.name == e.pattern && r.section.as_deref() == Some(s.name.as_str())) {
            Some(r) => {
                ap::update_system_rule(c, r, &rule)?;
                kept.push(r.id);
            }
            None => {
                ap::insert_system_rule(c, mr.id, &rule)?;
            }
        }
    }
    for r in existing.iter().filter(|r| !kept.contains(&r.id)) {
        c.execute("DELETE FROM approval_merge_request_rules WHERE id = ?1", [r.id])?;
    }
    Ok(())
}

/// After a push to the MR's source branch (from `old` to `new`): syncs the rules, and with
/// selective code owner removal takes back the approvals of the owners of the files the push
/// changed, with an `unapproved` (or `unapproval`) hook like a reset.
pub fn after_push(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, user: &User, old: &str, new: &str, prev_merge_status: &str) -> Result<()> {
    if !config.is_ee() {
        return Ok(());
    }
    let was_approved = ap::state(c, p, mr)?.approved();
    sync(c, p, mr)?;
    let eff = ap::effective(c, p)?;
    if !(eff.retain_approvals_on_push.value && eff.selective_code_owner_removals.value) {
        return Ok(());
    }
    let r = crate::repo::repo(c, p);
    let (Ok(old), Ok(new)) = (labrat_git::Oid::from_hex(old), labrat_git::Oid::from_hex(new)) else { return Ok(()) };
    let (Ok(old_c), Ok(new_c)) = (r.commit(&old), r.commit(&new)) else { return Ok(()) };
    let mut paths = vec![];
    for ch in labrat_git::diff::diff_trees(&r, Some(&old_c.tree), Some(&new_c.tree))? {
        paths.push(ch.new_path.clone());
        paths.push(ch.old_path.clone());
    }
    let sections = config_for(c, p, mr)?;
    let st = ap::state(c, p, mr)?;
    let mut owners: Vec<i64> = vec![];
    for w in st.rules.iter().filter(|w| w.rule.is_code_owner()) {
        let owns = paths.iter().any(|path| {
            owners_of(&sections, path)
                .iter()
                .any(|(si, ei)| sections[*si].name.as_str() == w.rule.section.as_deref().unwrap_or("") && sections[*si].entries[*ei].pattern == w.rule.name)
        });
        if owns {
            owners.extend(w.approvers.iter().copied());
        }
    }
    let approved: Vec<i64> = st.approvals.iter().map(|(u, _)| *u).filter(|u| owners.contains(u)).collect();
    if approved.is_empty() {
        return Ok(());
    }
    for uid in &approved {
        c.execute("DELETE FROM approvals WHERE merge_request_id = ?1 AND user_id = ?2", params![mr.id, uid])?;
        c.execute(
            "UPDATE merge_request_reviewers SET state = ?1 WHERE merge_request_id = ?2 AND user_id = ?3",
            params![crate::services::reviews::state::UNAPPROVED, mr.id, uid],
        )?;
    }
    let mr = mr.reload(c)?;
    let action = if was_approved && !ap::state(c, p, &mr)?.approved() { "unapproved" } else { "unapproval" };
    crate::services::hooks::merge_request_system_event(c, config, p, &mr, Some(user), action, "code_owner_approvals_reset_on_push", prev_merge_status)
}

fn config_for(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Vec<Section>> {
    Ok(config(c, p.id, &mr.target_branch)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn patterns() {
        assert!(matches("*", "a/b.txt"));
        assert!(matches("/docs/", "docs/a.md"));
        assert!(!matches("/docs/", "x/docs/a.md"));
        assert!(matches("docs/", "x/docs/a.md"));
        assert!(!matches("docs/", "docs"));
        assert!(matches("/app", "app/main.rb"));
        assert!(matches("*.rb", "app/main.rb"));
        assert!(!matches("/*.rb", "app/main.rb"));
        assert!(matches("/app/*.rb", "app/main.rb"));
        assert!(!matches("/app/*.rb", "app/x/main.rb"));
        assert!(matches("/app/**/*.rb", "app/x/main.rb"));
        assert!(matches("README.md", "README.md"));
        assert!(!matches("README.md", "README.txt"));
    }
}
