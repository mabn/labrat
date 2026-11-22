//! EE approval rules and approval settings (GitLab Premium and Ultimate).
//!
//! Everything here applies only when labrat runs as EE (`Config::is_ee`). On CE the routes don't
//! exist and the MR approvals body is CE's.
//!
//! GitLab EE's source isn't a reference for labrat, so this follows the behaviour recorded on an
//! EE 18.11.14 instance (`tests/conformance/fixtures-ee/approval_*.json`):
//! - Project rules are the defaults. An MR copies the rules that apply to its target branch
//!   when it is created. While the project allows overrides, the MR's own rules are used;
//!   otherwise (or when the MR has none) the project's rules are.
//! - Any developer may approve. An approval counts toward every rule that lists the approver,
//!   and toward an `any_approver` rule.
//! - The overall count is the larger of the `any_approver` rule's and the sum of the other
//!   rules'.

use crate::Result;
use crate::model::{Namespace, Project, User, access};
use crate::perms::ProjectPerms;
use crate::services::merge_requests::MergeRequest;
use crate::time;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------------------------
// Settings

/// A project's own approval settings, as stored (the legacy `/projects/:id/approvals` names).
#[derive(Debug, Clone)]
pub struct ProjectSettings {
    pub approvals_before_merge: i64,
    pub reset_approvals_on_push: bool,
    pub selective_code_owner_removals: bool,
    pub disable_overriding_approvers_per_merge_request: bool,
    pub merge_requests_author_approval: bool,
    pub merge_requests_disable_committers_approval: bool,
    /// Also `require_reauthentication_to_approve`: one setting with two names.
    pub require_password_to_approve: bool,
}

fn pbool(p: &Project, key: &str, default: bool) -> bool {
    p.setting(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn project_settings(p: &Project) -> ProjectSettings {
    ProjectSettings {
        approvals_before_merge: p.setting("approvals_before_merge").and_then(Value::as_i64).unwrap_or(0),
        reset_approvals_on_push: pbool(p, "reset_approvals_on_push", true),
        selective_code_owner_removals: pbool(p, "selective_code_owner_removals", false),
        disable_overriding_approvers_per_merge_request: pbool(p, "disable_overriding_approvers_per_merge_request", false),
        merge_requests_author_approval: pbool(p, "merge_requests_author_approval", false),
        merge_requests_disable_committers_approval: pbool(p, "merge_requests_disable_committers_approval", false),
        require_password_to_approve: pbool(p, "require_password_to_approve", false),
    }
}

/// Stores project settings (keys of the legacy endpoint).
pub fn set_project_settings(c: &Connection, p: &Project, values: &[(&str, Value)]) -> Result<()> {
    let mut s = p.settings.clone();
    for (k, v) in values {
        s.insert(k.to_string(), v.clone());
    }
    c.execute("UPDATE projects SET settings = ?1 WHERE id = ?2", params![Value::Object(s).to_string(), p.id])?;
    Ok(())
}

/// A top-level group's approval settings (`/groups/:id/merge_request_approval_setting`).
#[derive(Debug, Clone)]
pub struct GroupSettings {
    pub allow_author_approval: bool,
    pub allow_committer_approval: bool,
    pub allow_overrides_to_approver_list_per_merge_request: bool,
    pub retain_approvals_on_push: bool,
    pub require_reauthentication_to_approve: bool,
}

fn gbool(g: &Namespace, key: &str, default: bool) -> bool {
    g.settings.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn group_settings(g: &Namespace) -> GroupSettings {
    GroupSettings {
        allow_author_approval: gbool(g, "mr_allow_author_approval", true),
        allow_committer_approval: gbool(g, "mr_allow_committer_approval", true),
        allow_overrides_to_approver_list_per_merge_request: gbool(g, "mr_allow_overrides_to_approver_list_per_merge_request", true),
        retain_approvals_on_push: gbool(g, "mr_retain_approvals_on_push", true),
        require_reauthentication_to_approve: gbool(g, "mr_require_reauthentication_to_approve", false),
    }
}

pub fn set_group_settings(c: &Connection, g: &Namespace, values: &[(&str, bool)]) -> Result<()> {
    let mut s = g.settings.clone();
    for (k, v) in values {
        s.insert(format!("mr_{k}"), json!(v));
    }
    c.execute("UPDATE namespaces SET settings = ?1 WHERE id = ?2", params![Value::Object(s).to_string(), g.id])?;
    Ok(())
}

/// The top-level group of a project's namespace, if the project is in a group.
pub fn top_group(c: &Connection, p: &Project) -> Result<Option<Namespace>> {
    let mut ns = match Namespace::find(c, p.namespace_id)? {
        Some(n) => n,
        None => return Ok(None),
    };
    if !ns.is_group() {
        return Ok(None);
    }
    while let Some(parent) = ns.parent_id {
        match Namespace::find(c, parent)? {
            Some(n) => ns = n,
            None => break,
        }
    }
    Ok(Some(ns))
}

/// One setting as `merge_request_approval_setting` shows it.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub value: bool,
    /// Locked by the top-level group.
    pub locked: bool,
}

impl Setting {
    pub fn json(&self) -> Value {
        json!({"value": self.value, "locked": self.locked, "inherited_from": if self.locked { json!("group") } else { Value::Null }})
    }
}

/// The settings in force for a project, in `merge_request_approval_setting` terms.
#[derive(Debug, Clone)]
pub struct Effective {
    pub allow_author_approval: Setting,
    pub allow_committer_approval: Setting,
    pub allow_overrides_to_approver_list_per_merge_request: Setting,
    pub retain_approvals_on_push: Setting,
    pub selective_code_owner_removals: Setting,
    pub require_password_to_approve: Setting,
}

impl Effective {
    /// The `merge_request_approval_setting` body.
    pub fn json(&self) -> Value {
        json!({
            "allow_author_approval": self.allow_author_approval.json(),
            "allow_committer_approval": self.allow_committer_approval.json(),
            "allow_overrides_to_approver_list_per_merge_request": self.allow_overrides_to_approver_list_per_merge_request.json(),
            "retain_approvals_on_push": self.retain_approvals_on_push.json(),
            "selective_code_owner_removals": self.selective_code_owner_removals.json(),
            "require_password_to_approve": self.require_password_to_approve.json(),
            "require_reauthentication_to_approve": self.require_password_to_approve.json(),
        })
    }
}

/// A group setting locks the project's when it is the restrictive value; the project then
/// gets the group's value. Stored project values are kept and come back when the lock goes.
pub fn effective(c: &Connection, p: &Project) -> Result<Effective> {
    let s = project_settings(p);
    let g = top_group(c, p)?.map(|g| group_settings(&g));
    let lock_false = |own: bool, group: Option<bool>| match group {
        Some(false) => Setting { value: false, locked: true },
        _ => Setting { value: own, locked: false },
    };
    let retain_locked = g.as_ref().is_some_and(|g| !g.retain_approvals_on_push);
    Ok(Effective {
        allow_author_approval: lock_false(s.merge_requests_author_approval, g.as_ref().map(|g| g.allow_author_approval)),
        allow_committer_approval: lock_false(!s.merge_requests_disable_committers_approval, g.as_ref().map(|g| g.allow_committer_approval)),
        allow_overrides_to_approver_list_per_merge_request: lock_false(
            !s.disable_overriding_approvers_per_merge_request,
            g.as_ref().map(|g| g.allow_overrides_to_approver_list_per_merge_request),
        ),
        retain_approvals_on_push: lock_false(!s.reset_approvals_on_push, g.as_ref().map(|g| g.retain_approvals_on_push)),
        selective_code_owner_removals: if retain_locked {
            Setting { value: false, locked: true }
        } else {
            Setting { value: s.selective_code_owner_removals, locked: false }
        },
        require_password_to_approve: match g.as_ref().map(|g| g.require_reauthentication_to_approve) {
            Some(true) => Setting { value: true, locked: true },
            _ => Setting { value: s.require_password_to_approve, locked: false },
        },
    })
}

/// The group's own setting body: a group locks nothing for itself, except that "selective
/// code owner removals" needs approvals to be retained.
pub fn group_json(g: &Namespace) -> Value {
    let s = group_settings(g);
    let free = |v: bool| Setting { value: v, locked: false }.json();
    json!({
        "allow_author_approval": free(s.allow_author_approval),
        "allow_committer_approval": free(s.allow_committer_approval),
        "allow_overrides_to_approver_list_per_merge_request": free(s.allow_overrides_to_approver_list_per_merge_request),
        "retain_approvals_on_push": free(s.retain_approvals_on_push),
        "selective_code_owner_removals": Setting { value: false, locked: !s.retain_approvals_on_push }.json(),
        "require_password_to_approve": free(s.require_reauthentication_to_approve),
        "require_reauthentication_to_approve": free(s.require_reauthentication_to_approve),
    })
}

/// The legacy `/projects/:id/approvals` body (effective values).
pub fn legacy_json(c: &Connection, p: &Project) -> Result<Value> {
    let s = project_settings(p);
    let e = effective(c, p)?;
    Ok(json!({
        "approvers": [],
        "approver_groups": [],
        "approvals_before_merge": s.approvals_before_merge,
        "reset_approvals_on_push": !e.retain_approvals_on_push.value,
        "selective_code_owner_removals": e.selective_code_owner_removals.value,
        "disable_overriding_approvers_per_merge_request": !e.allow_overrides_to_approver_list_per_merge_request.value,
        "merge_requests_author_approval": e.allow_author_approval.value,
        "merge_requests_disable_committers_approval": !e.allow_committer_approval.value,
        "require_password_to_approve": e.require_password_to_approve.value,
        "require_reauthentication_to_approve": e.require_password_to_approve.value,
    }))
}

/// The project's `approvals_before_merge` (deprecated; `0` by default).
pub fn project_approvals_before_merge(_c: &Connection, p: &Project) -> Result<i64> {
    Ok(project_settings(p).approvals_before_merge)
}

/// The MR's own `approvals_before_merge` (deprecated; `null` unless given on create).
pub fn mr_approvals_before_merge(c: &Connection, mr: &MergeRequest) -> Result<Option<i64>> {
    Ok(c.query_row("SELECT approvals_before_merge FROM merge_requests WHERE id = ?1", [mr.id], |r| r.get(0))?)
}

// ---------------------------------------------------------------------------------------------
// Rules

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    MergeRequest,
}

impl Scope {
    fn table(self) -> &'static str {
        match self {
            Scope::Project => "approval_project_rules",
            Scope::MergeRequest => "approval_merge_request_rules",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub scope: Scope,
    pub id: i64,
    /// The project (project rules) or MR (MR rules) the rule belongs to.
    pub owner_id: i64,
    pub name: String,
    pub approvals_required: i64,
    pub rule_type: String,
    pub report_type: Option<String>,
    pub applies_to_all_protected_branches: bool,
    /// MR rules: the project rule this was copied from.
    pub source_id: Option<i64>,
    pub modified_from_project_rule: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub user_ids: Vec<i64>,
    pub group_ids: Vec<i64>,
    pub protected_branch_ids: Vec<i64>,
}

impl Rule {
    pub fn is_any_approver(&self) -> bool {
        self.rule_type == "any_approver"
    }
}

fn ids(c: &Connection, sql: &str, id: i64) -> Result<Vec<i64>> {
    let mut st = c.prepare(sql)?;
    let v = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(v)
}

fn load(c: &Connection, scope: Scope, where_sql: &str, arg: i64) -> Result<Vec<Rule>> {
    let (owner, extra) = match scope {
        Scope::Project => ("project_id", "applies_to_all_protected_branches, NULL, 0"),
        Scope::MergeRequest => ("merge_request_id", "0, approval_project_rule_id, modified_from_project_rule"),
    };
    let sql = format!(
        "SELECT id, {owner}, name, approvals_required, rule_type, report_type, {extra}, created_at, updated_at FROM {} WHERE {where_sql} ORDER BY id",
        scope.table()
    );
    let mut st = c.prepare(&sql)?;
    let mut rules = st
        .query_map([arg], |r| {
            Ok(Rule {
                scope,
                id: r.get(0)?,
                owner_id: r.get(1)?,
                name: r.get(2)?,
                approvals_required: r.get(3)?,
                rule_type: r.get(4)?,
                report_type: r.get(5)?,
                applies_to_all_protected_branches: r.get::<_, i64>(6)? != 0,
                source_id: r.get(7)?,
                modified_from_project_rule: r.get::<_, i64>(8)? != 0,
                created_at: r.get(9)?,
                updated_at: r.get(10)?,
                user_ids: vec![],
                group_ids: vec![],
                protected_branch_ids: vec![],
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let t = scope.table();
    for rule in &mut rules {
        rule.user_ids = ids(c, &format!("SELECT user_id FROM {t}_users WHERE rule_id = ?1 ORDER BY user_id"), rule.id)?;
        rule.group_ids = ids(c, &format!("SELECT group_id FROM {t}_groups WHERE rule_id = ?1 ORDER BY group_id"), rule.id)?;
        if scope == Scope::Project {
            rule.protected_branch_ids = ids(
                c,
                "SELECT protected_branch_id FROM approval_project_rules_protected_branches WHERE rule_id = ?1 ORDER BY protected_branch_id",
                rule.id,
            )?;
        }
    }
    Ok(rules)
}

/// Project rules in id order.
pub fn project_rules(c: &Connection, p: &Project) -> Result<Vec<Rule>> {
    load(c, Scope::Project, "project_id = ?1", p.id)
}

/// Project rules as the API lists them: the `any_approver` rule first, then by id.
pub fn project_rules_listed(c: &Connection, p: &Project) -> Result<Vec<Rule>> {
    let mut v = project_rules(c, p)?;
    v.sort_by_key(|r| (!r.is_any_approver(), r.id));
    Ok(v)
}

pub fn project_rule(c: &Connection, p: &Project, id: i64) -> Result<Option<Rule>> {
    let found = c
        .query_row("SELECT 1 FROM approval_project_rules WHERE id = ?1 AND project_id = ?2", params![id, p.id], |_| Ok(()))
        .optional()?;
    if found.is_none() {
        return Ok(None);
    }
    Ok(load(c, Scope::Project, "id = ?1", id)?.pop())
}

/// MR rules in id order.
pub fn mr_rules(c: &Connection, mr: &MergeRequest) -> Result<Vec<Rule>> {
    load(c, Scope::MergeRequest, "merge_request_id = ?1", mr.id)
}

pub fn mr_rule(c: &Connection, mr: &MergeRequest, id: i64) -> Result<Option<Rule>> {
    let found = c
        .query_row("SELECT 1 FROM approval_merge_request_rules WHERE id = ?1 AND merge_request_id = ?2", params![id, mr.id], |_| Ok(()))
        .optional()?;
    if found.is_none() {
        return Ok(None);
    }
    Ok(load(c, Scope::MergeRequest, "id = ?1", id)?.pop())
}

/// What a create or update sets on a rule. `None` leaves a field as it is.
#[derive(Debug, Default, Clone)]
pub struct RuleParams {
    pub name: Option<String>,
    pub approvals_required: Option<i64>,
    pub rule_type: Option<String>,
    pub report_type: Option<String>,
    pub applies_to_all_protected_branches: Option<bool>,
    pub user_ids: Option<Vec<i64>>,
    pub group_ids: Option<Vec<i64>>,
    pub protected_branch_ids: Option<Vec<i64>>,
    pub source_id: Option<i64>,
    /// Approvers were asked for (even if none of them could be added): the rule is regular.
    pub approvers_given: bool,
}

/// ActiveRecord validations of a rule. Errors are (field, message) pairs.
fn validate(c: &Connection, scope: Scope, owner_id: i64, self_id: Option<i64>, name: &str, required: i64, rule_type: &str, report_type: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut errs = vec![];
    if name.trim().is_empty() {
        errs.push(("name".into(), "can't be blank".into()));
    } else if name.chars().count() > 1024 {
        errs.push(("name".into(), "is too long (maximum is 1024 characters)".into()));
    }
    if required < 0 {
        errs.push(("approvals_required".into(), "must be greater than or equal to 0".into()));
    } else if required > 100 {
        errs.push(("approvals_required".into(), "must be less than or equal to 100".into()));
    }
    if rule_type == "report_approver" && report_type.is_none() {
        errs.push(("report_type".into(), "can't be blank".into()));
    }
    let owner = match scope {
        Scope::Project => "project_id",
        Scope::MergeRequest => "merge_request_id",
    };
    if errs.is_empty() {
        if rule_type == "any_approver" {
            let n: i64 = c.query_row(
                &format!("SELECT count(*) FROM {} WHERE {owner} = ?1 AND rule_type = 'any_approver' AND id != ?2", scope.table()),
                params![owner_id, self_id.unwrap_or(0)],
                |r| r.get(0),
            )?;
            if n > 0 {
                let what = match scope {
                    Scope::Project => "project",
                    Scope::MergeRequest => "merge request",
                };
                errs.push(("rule_type".into(), format!("any-approver for the {what} already exists")));
            }
        } else {
            let n: i64 = c.query_row(
                &format!("SELECT count(*) FROM {} WHERE {owner} = ?1 AND name = ?2 AND rule_type != 'any_approver' AND id != ?3", scope.table()),
                params![owner_id, name, self_id.unwrap_or(0)],
                |r| r.get(0),
            )?;
            if n > 0 {
                errs.push(("name".into(), "has already been taken".into()));
            }
        }
    }
    Ok(errs)
}

fn set_links(c: &Connection, scope: Scope, id: i64, user_ids: Option<&[i64]>, group_ids: Option<&[i64]>, pb_ids: Option<&[i64]>) -> Result<()> {
    let t = scope.table();
    if let Some(u) = user_ids {
        c.execute(&format!("DELETE FROM {t}_users WHERE rule_id = ?1"), [id])?;
        for uid in u {
            c.execute(&format!("INSERT OR IGNORE INTO {t}_users (rule_id, user_id) VALUES (?1, ?2)"), params![id, uid])?;
        }
    }
    if let Some(g) = group_ids {
        c.execute(&format!("DELETE FROM {t}_groups WHERE rule_id = ?1"), [id])?;
        for gid in g {
            c.execute(&format!("INSERT OR IGNORE INTO {t}_groups (rule_id, group_id) VALUES (?1, ?2)"), params![id, gid])?;
        }
    }
    if let (Some(b), Scope::Project) = (pb_ids, scope) {
        c.execute("DELETE FROM approval_project_rules_protected_branches WHERE rule_id = ?1", [id])?;
        for bid in b {
            c.execute(
                "INSERT OR IGNORE INTO approval_project_rules_protected_branches (rule_id, protected_branch_id) VALUES (?1, ?2)",
                params![id, bid],
            )?;
        }
    }
    Ok(())
}

/// Creates a rule. A rule given no approvers (users or groups) is an `any_approver` rule.
pub fn create_rule(c: &Connection, scope: Scope, owner_id: i64, rp: &RuleParams) -> Result<std::result::Result<Rule, Vec<(String, String)>>> {
    let name = rp.name.clone().unwrap_or_default();
    let required = rp.approvals_required.unwrap_or(0);
    let mut rule_type = rp.rule_type.clone().unwrap_or_else(|| "regular".into());
    let has_approvers = rp.approvers_given
        || rp.user_ids.as_ref().is_some_and(|v| !v.is_empty())
        || rp.group_ids.as_ref().is_some_and(|v| !v.is_empty());
    if rule_type == "regular" && !has_approvers {
        rule_type = "any_approver".into();
    }
    let errs = validate(c, scope, owner_id, None, &name, required, &rule_type, rp.report_type.as_deref())?;
    if !errs.is_empty() {
        return Ok(Err(errs));
    }
    let now = time::now();
    // Project and MR rule ids come from one counter, so an id never names both kinds (on a
    // real instance the two tables' ids are far apart, and recordings rely on that).
    let id: i64 = c.query_row(
        "SELECT 1 + max(coalesce((SELECT max(id) FROM approval_project_rules), 0), coalesce((SELECT max(id) FROM approval_merge_request_rules), 0),
                        coalesce((SELECT max(seq) FROM sqlite_sequence WHERE name IN ('approval_project_rules', 'approval_merge_request_rules')), 0))",
        [],
        |r| r.get(0),
    )?;
    match scope {
        Scope::Project => c.execute(
            "INSERT INTO approval_project_rules (id, project_id, name, approvals_required, rule_type, report_type, applies_to_all_protected_branches, created_at, updated_at) VALUES (?8, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![owner_id, name, required, rule_type, rp.report_type, rp.applies_to_all_protected_branches.unwrap_or(false), now, id],
        )?,
        Scope::MergeRequest => c.execute(
            "INSERT INTO approval_merge_request_rules (id, merge_request_id, name, approvals_required, rule_type, report_type, approval_project_rule_id, created_at, updated_at) VALUES (?8, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![owner_id, name, required, rule_type, rp.report_type, rp.source_id, now, id],
        )?,
    };
    // `applies_to_all_protected_branches` ignores the branch list.
    let pbs = if rp.applies_to_all_protected_branches == Some(true) { Some(vec![]) } else { rp.protected_branch_ids.clone() };
    set_links(c, scope, id, rp.user_ids.as_deref(), rp.group_ids.as_deref(), pbs.as_deref())?;
    Ok(Ok(load(c, scope, "id = ?1", id)?.pop().expect("rule was inserted")))
}

/// Updates a rule; the approver lists given replace the old ones.
pub fn update_rule(c: &Connection, rule: &Rule, rp: &RuleParams) -> Result<std::result::Result<Rule, Vec<(String, String)>>> {
    let name = rp.name.clone().unwrap_or_else(|| rule.name.clone());
    let required = rp.approvals_required.unwrap_or(rule.approvals_required);
    let errs = validate(c, rule.scope, rule.owner_id, Some(rule.id), &name, required, &rule.rule_type, rule.report_type.as_deref())?;
    if !errs.is_empty() {
        return Ok(Err(errs));
    }
    let t = rule.scope.table();
    c.execute(
        &format!("UPDATE {t} SET name = ?1, approvals_required = ?2, updated_at = ?3 WHERE id = ?4"),
        params![name, required, time::now(), rule.id],
    )?;
    if rule.scope == Scope::Project
        && let Some(all) = rp.applies_to_all_protected_branches {
            c.execute("UPDATE approval_project_rules SET applies_to_all_protected_branches = ?1 WHERE id = ?2", params![all, rule.id])?;
        }
    let pbs = if rp.applies_to_all_protected_branches == Some(true) { Some(vec![]) } else { rp.protected_branch_ids.clone() };
    set_links(c, rule.scope, rule.id, rp.user_ids.as_deref(), rp.group_ids.as_deref(), pbs.as_deref())?;
    Ok(Ok(load(c, rule.scope, "id = ?1", rule.id)?.pop().expect("rule exists")))
}

pub fn delete_rule(c: &Connection, rule: &Rule) -> Result<()> {
    c.execute(&format!("DELETE FROM {} WHERE id = ?1", rule.scope.table()), [rule.id])?;
    Ok(())
}

/// Protected branch ids a project rule covers: every protected branch for
/// `applies_to_all_protected_branches`, else its own list.
pub fn rule_protected_branch_ids(c: &Connection, p: &Project, rule: &Rule) -> Result<Vec<i64>> {
    if rule.applies_to_all_protected_branches {
        ids(c, "SELECT id FROM protected_branches WHERE project_id = ?1 ORDER BY id", p.id)
    } else {
        Ok(rule.protected_branch_ids.clone())
    }
}

/// Whether a project rule applies to MRs targeting `branch`.
fn applies_to_branch(c: &Connection, p: &Project, rule: &Rule, branch: &str) -> Result<bool> {
    if !rule.applies_to_all_protected_branches && rule.protected_branch_ids.is_empty() {
        return Ok(true);
    }
    for id in rule_protected_branch_ids(c, p, rule)? {
        let name: Option<String> = c.query_row("SELECT name FROM protected_branches WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        if let Some(n) = name
            && crate::repo::ref_matches(&n, branch) {
                return Ok(true);
            }
    }
    Ok(false)
}

/// Copies the project rules that apply to the MR's target branch (`MergeRequests::CreateService`).
pub fn copy_project_rules(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<()> {
    // GitLab copies them in no fixed order (two recordings of one scenario differ); labrat
    // goes by id, and the conformance harness compares rule lists sorted by type and name.
    for rule in project_rules(c, p)? {
        if !applies_to_branch(c, p, &rule, &mr.target_branch)? {
            continue;
        }
        let rp = RuleParams {
            name: Some(rule.name.clone()),
            approvals_required: Some(rule.approvals_required),
            rule_type: Some(rule.rule_type.clone()),
            report_type: rule.report_type.clone(),
            user_ids: Some(rule.user_ids.clone()),
            group_ids: Some(rule.group_ids.clone()),
            source_id: Some(rule.id),
            ..Default::default()
        };
        let _ = create_rule(c, Scope::MergeRequest, mr.id, &rp)?;
    }
    Ok(())
}

/// Users who can act on a project (any membership, direct or inherited, or admins).
pub fn project_member(c: &Connection, p: &Project, uid: i64) -> Result<bool> {
    let Some(u) = User::find(c, uid)? else { return Ok(false) };
    Ok(ProjectPerms::load(c, Some(&u), p)?.level > 0)
}

/// Direct members of a group (approver groups count only direct members).
pub fn group_member_ids(c: &Connection, gid: i64) -> Result<Vec<i64>> {
    ids(c, "SELECT user_id FROM members WHERE source_type = 'Namespace' AND source_id = ?1 ORDER BY user_id", gid)
}

/// A rule's users together with its groups' members, by id.
pub fn rule_approver_ids(c: &Connection, rule: &Rule) -> Result<Vec<i64>> {
    let mut v = rule.user_ids.clone();
    for g in &rule.group_ids {
        v.extend(group_member_ids(c, *g)?);
    }
    v.sort();
    v.dedup();
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// Approval state

/// A rule as an MR's approval state sees it (`ApprovalWrappedRule`).
#[derive(Debug, Clone)]
pub struct Wrapped {
    pub rule: Rule,
    /// The project rule an MR rule came from, when the MR's own rules are in force.
    pub source: Option<Rule>,
    /// Who may approve for the rule (empty for `any_approver`).
    pub approvers: Vec<i64>,
    /// Approvals that count toward the rule, by user id.
    pub approved_by: Vec<i64>,
}

impl Wrapped {
    /// `any_approver` rules show under GitLab's fixed name.
    pub fn name(&self) -> String {
        if self.rule.is_any_approver() { "All Members".into() } else { self.rule.name.clone() }
    }

    pub fn approved(&self) -> bool {
        self.approved_by.len() as i64 >= self.rule.approvals_required
    }

    pub fn left(&self) -> i64 {
        (self.rule.approvals_required - self.approved_by.len() as i64).max(0)
    }

    /// Whether the MR rule differs from the project rule it was copied from.
    pub fn overridden(&self) -> bool {
        match &self.source {
            Some(s) => {
                s.approvals_required != self.rule.approvals_required
                    || s.name != self.rule.name
                    || s.user_ids != self.rule.user_ids
                    || s.group_ids != self.rule.group_ids
            }
            None => false,
        }
    }
}

pub struct State {
    /// The MR uses its own rules (overrides are allowed and it has some).
    pub overwritten: bool,
    /// Rules in display order: `any_approver` first, then by id.
    pub rules: Vec<Wrapped>,
    /// Approvals as (user id, approved at), in approval order.
    pub approvals: Vec<(i64, i64)>,
    pub author_id: i64,
    pub author_can_approve: bool,
}

impl State {
    pub fn approvals_required(&self) -> i64 {
        let any = self.rules.iter().filter(|w| w.rule.is_any_approver()).map(|w| w.rule.approvals_required).max().unwrap_or(0);
        let regular: i64 = self.rules.iter().filter(|w| !w.rule.is_any_approver()).map(|w| w.rule.approvals_required).sum();
        any.max(regular)
    }

    pub fn approvals_left(&self) -> i64 {
        let any = self.rules.iter().filter(|w| w.rule.is_any_approver()).map(|w| w.left()).max().unwrap_or(0);
        let regular: i64 = self.rules.iter().filter(|w| !w.rule.is_any_approver()).map(|w| w.left()).sum();
        any.max(regular)
    }

    pub fn approved(&self) -> bool {
        self.rules.iter().all(|w| w.approved())
    }

    pub fn has_approved(&self, uid: i64) -> bool {
        self.approvals.iter().any(|(u, _)| *u == uid)
    }

    /// Approvers of unapproved rules who haven't approved yet, by id.
    pub fn suggested_approvers(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self
            .rules
            .iter()
            .filter(|w| !w.rule.is_any_approver())
            .flat_map(|w| w.approvers.iter().copied())
            .filter(|u| !self.has_approved(*u))
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Whether the MR's own rules are in force.
pub fn uses_mr_rules(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<bool> {
    if !effective(c, p)?.allow_overrides_to_approver_list_per_merge_request.value {
        return Ok(false);
    }
    Ok(!mr_rules(c, mr)?.is_empty())
}

pub fn state(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<State> {
    let eff = effective(c, p)?;
    let author_can_approve = eff.allow_author_approval.value;
    let overwritten = uses_mr_rules(c, p, mr)?;
    let rules: Vec<(Rule, Option<Rule>)> = if overwritten {
        let mut v = vec![];
        for r in mr_rules(c, mr)? {
            let source = match r.source_id {
                Some(sid) => project_rule(c, p, sid)?,
                None => None,
            };
            v.push((r, source));
        }
        v
    } else {
        let mut v = vec![];
        for r in project_rules(c, p)? {
            if applies_to_branch(c, p, &r, &mr.target_branch)? {
                v.push((r, None));
            }
        }
        v
    };
    let approvals = crate::services::reviews::approvals(c, mr)?;
    let mut wrapped = vec![];
    for (rule, source) in rules {
        let mut approvers = if rule.is_any_approver() { vec![] } else { rule_approver_ids(c, &rule)? };
        if !author_can_approve {
            approvers.retain(|u| *u != mr.author_id);
        }
        let mut approved_by: Vec<i64> = approvals
            .iter()
            .map(|(u, _)| *u)
            .filter(|u| rule.is_any_approver() || approvers.contains(u))
            .collect();
        approved_by.sort();
        wrapped.push(Wrapped { rule, source, approvers, approved_by });
    }
    wrapped.sort_by_key(|w| (!w.rule.is_any_approver(), w.rule.id));
    Ok(State { overwritten, rules: wrapped, approvals, author_id: mr.author_id, author_can_approve })
}

/// EE's approval eligibility. `Err(403)`-style refusals (no permission) are reported as
/// `Eligibility::Forbidden`, others as `Eligibility::No`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Yes,
    /// The user may approve merge requests in general but not this one now (API: 401).
    No,
    /// The user may not approve merge requests in this project (API: 403).
    Forbidden,
}

pub fn eligibility(c: &Connection, p: &Project, mr: &MergeRequest, user: &User) -> Result<Eligibility> {
    let perms = ProjectPerms::load(c, Some(user), p)?;
    if !(perms.level >= access::DEVELOPER || perms.admin) {
        return Ok(Eligibility::Forbidden);
    }
    if mr.state == "merged" || crate::services::reviews::has_approved(c, mr, user)? {
        return Ok(Eligibility::No);
    }
    let eff = effective(c, p)?;
    if mr.author_id == user.id && !eff.allow_author_approval.value {
        return Ok(Eligibility::No);
    }
    Ok(Eligibility::Yes)
}

/// EE treats an MR as temporarily unapproved while its approvals are synced after it is created
/// or pushed to: `detailed_merge_status` is `approvals_syncing` and merging is refused until a
/// background job finishes.
pub fn start_sync(c: &Connection, config: &crate::Config, mr: &MergeRequest) -> Result<()> {
    if !config.is_ee() {
        return Ok(());
    }
    c.execute("UPDATE merge_requests SET approvals_syncing = 1 WHERE id = ?1", [mr.id])?;
    crate::jobs::enqueue(c, "approvals_sync", json!({"merge_request_id": mr.id}))
}

pub fn finish_sync(c: &Connection, mr_id: i64) -> Result<()> {
    c.execute("UPDATE merge_requests SET approvals_syncing = 0 WHERE id = ?1", [mr_id])?;
    Ok(())
}

/// A pipeline of the MR started running: a sync waiting for it begins, and ends a moment
/// later, as without CI.
pub fn pipeline_running(c: &Connection, mr_id: i64) -> Result<()> {
    if c.execute("UPDATE merge_requests SET approvals_syncing = 1 WHERE id = ?1 AND approvals_syncing = 2", [mr_id])? > 0 {
        crate::jobs::enqueue(c, "approvals_sync", json!({"merge_request_id": mr_id}))?;
    }
    Ok(())
}

pub fn syncing(c: &Connection, mr: &MergeRequest) -> Result<bool> {
    Ok(c.query_row("SELECT approvals_syncing FROM merge_requests WHERE id = ?1", [mr.id], |r| r.get::<_, i64>(0))? == 1)
}

/// Ruby's `Array#to_sentence`.
fn to_sentence(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        n => format!("{}, and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

/// EE drops an MR's approvals when commits are pushed to it, unless the project retains them:
/// a system note naming the approvers, and an `unapproved` hook.
pub fn reset_on_push(c: &Connection, config: &crate::Config, p: &Project, mr: &MergeRequest, user: &User, prev_merge_status: &str) -> Result<()> {
    if !config.is_ee() || effective(c, p)?.retain_approvals_on_push.value {
        return Ok(());
    }
    let mut approvers: Vec<i64> = crate::services::reviews::approvals(c, mr)?.into_iter().map(|(u, _)| u).collect();
    if approvers.is_empty() {
        return Ok(());
    }
    approvers.sort();
    let was_approved = state(c, p, mr)?.approved();
    let mut names = vec![];
    c.execute("DELETE FROM approvals WHERE merge_request_id = ?1", [mr.id])?;
    for uid in &approvers {
        if let Some(u) = User::find(c, *uid)? {
            names.push(format!("@{}", u.username));
            // The reset approvers who are reviewers go back to `unapproved`. Their cached
            // review-request counts stay as they were (the EE reference still counted 0 for
            // such a reviewer).
            c.execute(
                "UPDATE merge_request_reviewers SET state = ?1 WHERE merge_request_id = ?2 AND user_id = ?3",
                params![crate::services::reviews::state::UNAPPROVED, mr.id, u.id],
            )?;
        }
    }
    crate::services::notes::system_note(
        c,
        mr.id,
        p.id,
        user,
        &format!("reset approvals from {} by pushing to the branch", to_sentence(&names)),
        "approvals_reset",
    )?;
    let mr = mr.reload(c)?;
    // Like an unapprove: `unapproved` only when the reset takes the MR from approved to not.
    let action = if was_approved && !state(c, p, &mr)?.approved() { "unapproved" } else { "unapproval" };
    crate::services::hooks::merge_request_system_event(c, config, p, &mr, Some(user), action, "approvals_reset_on_push", prev_merge_status)
}

/// Sets the MR's required approvals through its `any_approver` rule, creating an
/// "All Members" rule when it has none.
pub fn set_required(c: &Connection, mr: &MergeRequest, n: i64) -> Result<()> {
    match mr_rules(c, mr)?.into_iter().find(|r| r.is_any_approver()) {
        Some(r) => {
            c.execute(
                // A rule copied from the project's is now modified from it.
                "UPDATE approval_merge_request_rules SET approvals_required = ?1, updated_at = ?2,
                 modified_from_project_rule = (approval_project_rule_id IS NOT NULL) WHERE id = ?3",
                params![n, time::now(), r.id],
            )?;
        }
        None => {
            let rp = RuleParams { name: Some("All Members".into()), approvals_required: Some(n), rule_type: Some("any_approver".into()), ..Default::default() };
            let _ = create_rule(c, Scope::MergeRequest, mr.id, &rp)?;
        }
    }
    Ok(())
}

/// `approval_rules` in MR webhook payloads: the MR's own rule rows.
pub fn hook_rules(c: &Connection, mr: &MergeRequest) -> Result<Vec<Value>> {
    let p = Project::find(c, mr.target_project_id)?;
    let mut out = vec![];
    for r in mr_rules(c, mr)? {
        out.push(json!({
            "id": r.id,
            "approvals_required": r.approvals_required,
            "name": r.name,
            "rule_type": r.rule_type,
            "report_type": r.report_type,
            "merge_request_id": mr.id,
            "section": null,
            "modified_from_project_rule": r.modified_from_project_rule,
            "orchestration_policy_idx": null,
            "vulnerabilities_allowed": 0,
            "scanners": [],
            "severity_levels": [],
            "vulnerability_states": ["new_needs_triage", "new_dismissed"],
            "security_orchestration_policy_configuration_id": null,
            "scan_result_policy_id": null,
            "applicable_post_merge": null,
            "project_id": p.as_ref().map(|p| p.id),
            "approval_policy_rule_id": null,
            "updated_at": time::iso(r.updated_at),
            "created_at": time::iso(r.created_at),
        }));
    }
    Ok(out)
}
