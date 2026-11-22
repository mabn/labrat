//! EE approval policies (merge request approval policies) as merge requests see them. labrat
//! doesn't run security scans or link security policy projects: a test endpoint sets a
//! project's policies, in GitLab's policy schema (the `approval_policy` list of a policy file,
//! as JSON), and labrat evaluates them the way the EE 18.11.14 reference does without CI
//! reports (`fixtures-ee/approval_policies`):
//! - `any_merge_request` rules are violated (`commits: any`), or violated by the MR's commits
//!   (`commits: unsigned`; labrat never signs commits);
//! - `scan_finding` and `license_finding` rules can't be evaluated (`ARTIFACTS_MISSING`): they
//!   fail closed (the rule needs its approvals) or, with `fallback_behavior: {fail: open}`,
//!   open (the rule needs none, the policy is reported with a `WARNING` status).
//!
//! Each policy rule and approval action gives the project a (hidden) `report_approver` rule,
//! which the MRs into the branches it covers copy. The MR rules ask for no approvals until the
//! policies are evaluated, after the open hook; the GitLab Security Bot then comments on the MR.
//! Warn mode violations can be dismissed, and policies with `bypass_settings` bypassed: their
//! rules then ask for no approvals.

use crate::model::{Namespace, Project, User};
use crate::services::approvals::{self as ap, Rule};
use crate::services::merge_requests::MergeRequest;
use crate::services::notes;
use crate::{Config, Result, time};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};

/// GitLab's `user_type` for the security bot.
pub const SECURITY_BOT: i64 = 8;
pub const BOT_USERNAME: &str = "GitLab-Security-Bot";

#[derive(Debug, Clone)]
pub struct Policy {
    pub id: i64,
    pub project_id: i64,
    /// The policy's index in the policy file.
    pub position: i64,
    pub name: String,
    pub content: Value,
}

impl Policy {
    pub fn warn(&self) -> bool {
        self.content["enforcement_type"].as_str() == Some("warn")
    }

    pub fn fail_open(&self) -> bool {
        self.content["fallback_behavior"]["fail"].as_str() == Some("open")
    }

    pub fn rules(&self) -> Vec<Value> {
        self.content["rules"].as_array().cloned().unwrap_or_default()
    }

    pub fn approval_actions(&self) -> Vec<Value> {
        self.content["actions"].as_array().map(|a| a.iter().filter(|x| x["type"] == "require_approval").cloned().collect()).unwrap_or_default()
    }

    /// `approval_settings` set to `true`, in the order EE reports them.
    pub fn true_settings(&self) -> Map<String, Value> {
        const ORDER: &[&str] = &[
            "block_branch_modification",
            "block_group_branch_modification",
            "prevent_approval_by_author",
            "require_password_to_approve",
            "remove_approvals_with_new_commit",
            "prevent_approval_by_commit_author",
            "prevent_pushing_and_force_pushing",
        ];
        let mut out = Map::new();
        if let Some(m) = self.content["approval_settings"].as_object() {
            let mut keys: Vec<&String> = m.keys().filter(|k| m[*k].as_bool() == Some(true)).collect();
            keys.sort_by_key(|k| ORDER.iter().position(|o| o == k).unwrap_or(ORDER.len()));
            for k in keys {
                out.insert(k.clone(), Value::Bool(true));
            }
        }
        out
    }

    pub fn has_bypass_settings(&self) -> bool {
        self.content["bypass_settings"].as_object().is_some_and(|m| !m.is_empty())
    }
}

/// A rule's report type as approval rules name it.
pub fn report_type(rule: &Value) -> &'static str {
    match rule["type"].as_str() {
        Some("scan_finding") => "scan_finding",
        Some("license_finding") => "license_scanning",
        _ => "any_merge_request",
    }
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Policy> {
    let content: String = r.get(4)?;
    Ok(Policy { id: r.get(0)?, project_id: r.get(1)?, position: r.get(2)?, name: r.get(3)?, content: serde_json::from_str(&content).unwrap_or(Value::Null) })
}

/// The project's enabled policies, in file order.
pub fn project_policies(c: &Connection, project_id: i64) -> Result<Vec<Policy>> {
    let mut st = c.prepare("SELECT id, project_id, position, name, content FROM security_policies WHERE project_id = ?1 ORDER BY position")?;
    let v = st.query_map([project_id], from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn find(c: &Connection, id: i64) -> Result<Option<Policy>> {
    Ok(c.query_row("SELECT id, project_id, position, name, content FROM security_policies WHERE id = ?1", [id], from_row).optional()?)
}

fn resolve_users(c: &Connection, action: &Value) -> Result<Vec<i64>> {
    let mut out = vec![];
    for u in action["user_approvers"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if let Some(name) = u.as_str()
            && let Some(user) = User::find_by_username(c, name)?
        {
            out.push(user.id);
        }
    }
    for id in action["user_approvers_ids"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if let Some(id) = id.as_i64()
            && User::find(c, id)?.is_some()
        {
            out.push(id);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn resolve_groups(c: &Connection, action: &Value) -> Result<Vec<i64>> {
    let mut out = vec![];
    for g in action["group_approvers"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if let Some(path) = g.as_str()
            && let Some(n) = Namespace::find_by_full_path(c, path)?.filter(|n| n.kind == "Group")
        {
            out.push(n.id);
        }
    }
    for id in action["group_approvers_ids"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if let Some(id) = id.as_i64()
            && Namespace::find(c, id)?.is_some_and(|n| n.kind == "Group")
        {
            out.push(id);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Checks the policy list (the parts labrat uses).
pub fn validate(list: &Value) -> std::result::Result<(), String> {
    let Some(list) = list.as_array() else { return Err("approval_policy must be an array".into()) };
    for p in list {
        let Some(name) = p["name"].as_str() else { return Err("each policy needs a name".into()) };
        if !p["rules"].is_array() {
            return Err(format!("policy {name:?} needs rules"));
        }
        for r in p["rules"].as_array().unwrap() {
            if !matches!(r["type"].as_str(), Some("any_merge_request" | "scan_finding" | "license_finding")) {
                return Err(format!("policy {name:?}: unknown rule type {}", r["type"]));
            }
        }
        if let Some(e) = p["enforcement_type"].as_str()
            && !matches!(e, "enforce" | "warn")
        {
            return Err(format!("policy {name:?}: unknown enforcement_type {e:?}"));
        }
    }
    Ok(())
}

/// Replaces the project's policies, and brings the open MRs' policy rules in line.
pub fn set_policies(c: &Connection, config: &Config, p: &Project, list: &Value) -> Result<()> {
    // The old policies' rules go, on the project and on the MRs.
    c.execute(
        "DELETE FROM approval_merge_request_rules WHERE security_policy_id IN (SELECT id FROM security_policies WHERE project_id = ?1)",
        [p.id],
    )?;
    c.execute("DELETE FROM approval_project_rules WHERE security_policy_id IN (SELECT id FROM security_policies WHERE project_id = ?1)", [p.id])?;
    c.execute("DELETE FROM security_policies WHERE project_id = ?1", [p.id])?;
    for (i, pol) in list.as_array().map(Vec::as_slice).unwrap_or_default().iter().enumerate() {
        if pol["enabled"].as_bool() == Some(false) {
            continue;
        }
        c.execute(
            "INSERT INTO security_policies (project_id, position, name, content) VALUES (?1, ?2, ?3, ?4)",
            params![p.id, i as i64, pol["name"].as_str().unwrap_or_default(), pol.to_string()],
        )?;
        let policy = find(c, c.last_insert_rowid())?.expect("just inserted");
        // One project rule per policy rule and approval action.
        for rule in policy.rules() {
            for action in policy.approval_actions() {
                let users = resolve_users(c, &action)?;
                let groups = resolve_groups(c, &action)?;
                let rp = ap::RuleParams {
                    name: Some(policy.name.clone()),
                    approvals_required: Some(action["approvals_required"].as_i64().unwrap_or(0)),
                    rule_type: Some("report_approver".into()),
                    report_type: Some(report_type(&rule).into()),
                    user_ids: Some(users),
                    group_ids: Some(groups),
                    ..Default::default()
                };
                let id = ap::insert_policy_project_rule(c, p.id, &rp, policy.id, &rule, &action)?;
                let _ = id;
            }
        }
    }
    let ids: Vec<i64> = {
        let mut st = c.prepare("SELECT id FROM merge_requests WHERE target_project_id = ?1 AND state = 'opened' ORDER BY id")?;
        st.query_map([p.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    for id in ids {
        let mr = MergeRequest::get(c, id)?;
        sync_rules(c, p, &mr)?;
        evaluate(c, config, p, &MergeRequest::get(c, id)?)?;
    }
    Ok(())
}

/// Whether a policy rule covers `branch`: the branches it names, else the protected ones
/// (`branch_type: protected`, the default), the default branch or all branches.
pub fn covers(c: &Connection, p: &Project, rule: &Value, branch: &str) -> Result<bool> {
    if let Some(list) = rule["branches"].as_array()
        && !list.is_empty()
    {
        return Ok(list.iter().filter_map(Value::as_str).any(|b| crate::repo::ref_matches(b, branch)));
    }
    match rule["branch_type"].as_str() {
        Some("default") => Ok(crate::repo::default_branch(p) == branch),
        Some("all") => Ok(true),
        _ => crate::repo::is_protected(c, p, branch),
    }
}

/// The project's policy rules (hidden `report_approver` project rules) with their policies.
fn policy_project_rules(c: &Connection, p: &Project) -> Result<Vec<(Rule, Policy, Value)>> {
    let mut out = vec![];
    for r in ap::policy_project_rules(c, p)? {
        let (pid, rule_json): (i64, String) =
            c.query_row("SELECT security_policy_id, policy_rule FROM approval_project_rules WHERE id = ?1", [r.id], |x| Ok((x.get(0)?, x.get(1)?)))?;
        if let Some(pol) = find(c, pid)? {
            out.push((r, pol, serde_json::from_str(&rule_json).unwrap_or(Value::Null)));
        }
    }
    Ok(out)
}

/// Gives the MR the policy rules that cover its target branch (asking for no approvals until
/// evaluated), and drops the others. Before the open hook, as on EE.
pub fn sync_rules(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<()> {
    let existing: Vec<Rule> = ap::mr_rules(c, mr)?.into_iter().filter(|r| r.rule_type == "report_approver").collect();
    let mut kept = vec![];
    if mr.is_open() {
        for (prule, _pol, rule_json) in policy_project_rules(c, p)? {
            if !covers(c, p, &rule_json, &mr.target_branch)? {
                continue;
            }
            match existing.iter().find(|r| r.source_id == Some(prule.id)) {
                Some(r) => kept.push(r.id),
                None => {
                    let id = ap::copy_policy_rule(c, mr.id, &prule)?;
                    kept.push(id);
                }
            }
        }
    }
    for r in existing.iter().filter(|r| !kept.contains(&r.id)) {
        c.execute("DELETE FROM approval_merge_request_rules WHERE id = ?1", [r.id])?;
    }
    Ok(())
}

/// A policy's outcome for an MR.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub policy: Policy,
    pub rule: Value,
    /// `FAILED` (violated), `WARNING` (failed open), or (set through the test endpoint)
    /// `RUNNING` or `SKIPPED`.
    pub status: String,
    /// `any_merge_request` violations: `true` (any commits) or the unsigned commits' short ids.
    pub commits: Option<Value>,
    /// What kept rules from being evaluated: `PolicyError`s (`error`, `reportType`, `message`,
    /// `data`).
    pub errors: Vec<Value>,
    /// Set through the test endpoint, as GraphQL's `PolicyViolationDetails` lists them:
    /// `newScanFinding`, `previousScanFinding`, `licenseScanning`, `comparisonPipelines`.
    pub details: Map<String, Value>,
}

impl Outcome {
    pub fn report_type(&self) -> &'static str {
        report_type(&self.rule)
    }

    /// Whether the policy's rules need their approvals (violated, or still being evaluated).
    pub fn blocking(&self) -> bool {
        matches!(self.status.as_str(), "FAILED" | "RUNNING")
    }

    pub fn failed(&self) -> bool {
        self.status == "FAILED"
    }

    /// One of the `details` lists.
    pub fn list(&self, key: &str) -> Vec<Value> {
        self.details.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
    }
}

fn artifacts_missing(pol: &Policy, rule: &Value) -> Value {
    let (rt, what) = if rule["type"] == "scan_finding" { ("SCAN_FINDING", "Security") } else { ("LICENSE_SCANNING", "SBOM") };
    json!({"error": "ARTIFACTS_MISSING", "reportType": rt, "message": format!("Pipeline configuration error: {what} reports required by policy `{}` could not be found.", pol.name), "data": {}})
}

/// The MR's commits, newest first, as short ids.
fn short_commits(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Vec<String>> {
    let Some(d) = mr.latest_diff(c)? else { return Ok(vec![]) };
    let r = crate::repo::repo(c, p);
    Ok(d.commits(&r)?.iter().map(|o| o.hex()[..8].to_string()).collect())
}

/// How each policy covering the MR's target branch fares, in file order.
pub fn outcomes(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Vec<Outcome>> {
    let mut out: Vec<Outcome> = vec![];
    let commits = short_commits(c, p, mr)?;
    for pol in project_policies(c, p.id)? {
        let set = result_override(c, mr.id, pol.id)?;
        if set.as_ref().is_some_and(|r| r["violated"] == false) {
            continue;
        }
        for rule in pol.rules() {
            if !covers(c, p, &rule, &mr.target_branch)? || out.iter().any(|o| o.policy.id == pol.id) {
                continue;
            }
            let scan = matches!(rule["type"].as_str(), Some("scan_finding") | Some("license_finding"));
            let unsigned = rule["commits"].as_str() == Some("unsigned");
            let mut o = match &set {
                // Set through the test endpoint: no reports needed.
                Some(r) => Outcome {
                    policy: pol.clone(),
                    rule: rule.clone(),
                    status: r["status"].as_str().unwrap_or("FAILED").to_string(),
                    commits: if scan { None } else { Some(if unsigned { json!(commits) } else { json!(true) }) },
                    errors: r["errors"].as_array().cloned().unwrap_or_default(),
                    details: Map::new(),
                },
                None if scan => Outcome {
                    policy: pol.clone(),
                    rule: rule.clone(),
                    status: if pol.fail_open() { "WARNING" } else { "FAILED" }.into(),
                    commits: None,
                    errors: vec![artifacts_missing(&pol, &rule)],
                    details: Map::new(),
                },
                None => {
                    if unsigned && commits.is_empty() {
                        continue;
                    }
                    Outcome {
                        policy: pol.clone(),
                        rule: rule.clone(),
                        status: "FAILED".into(),
                        commits: Some(if unsigned { json!(commits) } else { json!(true) }),
                        errors: vec![],
                        details: Map::new(),
                    }
                }
            };
            if let Some(r) = &set {
                for k in ["newScanFinding", "previousScanFinding", "licenseScanning", "comparisonPipelines"] {
                    if let Some(v) = r.get(k) {
                        o.details.insert(k.into(), v.clone());
                    }
                }
            }
            out.push(o);
        }
    }
    Ok(out)
}

/// Evaluates the MR's policies: its policy rules ask for their approvals where violated, and
/// the security bot's comment says why.
pub fn evaluate(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest) -> Result<()> {
    if !config.is_ee() || !mr.is_open() {
        return Ok(());
    }
    let outs = outcomes(c, p, mr)?;
    for r in ap::mr_rules(c, mr)?.into_iter().filter(|r| r.rule_type == "report_approver") {
        let Some(source) = r.source_id.map(|id| ap::any_project_rule(c, id)).transpose()?.flatten() else { continue };
        let pid: Option<i64> = c.query_row("SELECT security_policy_id FROM approval_project_rules WHERE id = ?1", [source.id], |x| x.get(0))?;
        let violated = outs.iter().any(|o| Some(o.policy.id) == pid && o.blocking());
        let want = if violated { source.approvals_required } else { 0 };
        if want != r.approvals_required {
            c.execute("UPDATE approval_merge_request_rules SET approvals_required = ?1, updated_at = ?2 WHERE id = ?3", params![want, time::now(), r.id])?;
        }
    }
    if outs.is_empty() {
        return Ok(());
    }
    let author = User::get(c, mr.author_id)?;
    let body = comment_body(config, p, &author, &outs);
    let bot = bot_user(c, config)?;
    let existing: Option<i64> = c
        .query_row(
            "SELECT id FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND author_id = ?2 AND system = 0 AND note LIKE '<!-- policy_violation_comment -->%' ORDER BY id LIMIT 1",
            params![mr.id, bot.id],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => {
            let note = notes::Note::find(c, id)?.expect("note exists");
            if note.note != body {
                c.execute("UPDATE notes SET note = ?1, updated_at = ?2 WHERE id = ?3", params![body, time::now(), id])?;
                let note = notes::Note::find(c, id)?.expect("note exists");
                crate::services::hooks::note_event(c, config, p, mr, &bot, &note, "update")?;
            }
        }
        None => {
            let note = notes::insert(
                c,
                &bot,
                notes::NewNote { note: body, noteable_type: "MergeRequest".into(), noteable_id: mr.id, project_id: Some(p.id), ..Default::default() },
            )?;
            crate::services::hooks::note_event(c, config, p, mr, &bot, &note, "create")?;
        }
    }
    Ok(())
}

/// The GitLab Security Bot, created on first use.
pub fn bot_user(c: &Connection, config: &Config) -> Result<User> {
    if let Some(u) = User::find_by_username(c, BOT_USERNAME)? {
        return Ok(u);
    }
    let host = config.external_url.split("://").nth(1).unwrap_or("localhost").split([':', '/']).next().unwrap_or("localhost").to_string();
    let u = crate::services::users::create(
        c,
        None,
        crate::services::users::NewUser {
            username: BOT_USERNAME.into(),
            name: "GitLab Security Bot".into(),
            email: format!("security-bot@noreply.{host}"),
            password: None,
            admin: false,
            user_type: SECURITY_BOT,
            external: false,
            private_profile: false,
            can_create_group: None,
            projects_limit: None,
            public_email: None,
            skip_confirmation: true,
            profile: vec![],
        },
    )?;
    c.execute(
        "UPDATE users SET avatar_url = ?1 WHERE id = ?2",
        params![config.url(&format!("/uploads/-/system/user/avatar/{}/security-bot.png", u.id)), u.id],
    )?;
    User::get(c, u.id)
}

/// The security bot's comment, as EE 18.11 words it.
pub fn comment_body(config: &Config, p: &Project, author: &User, outs: &[Outcome]) -> String {
    let failed: Vec<&Outcome> = outs.iter().filter(|o| o.status == "FAILED").collect();
    let skipped: Vec<&Outcome> = outs.iter().filter(|o| o.status == "WARNING").collect();
    let mut reports: Vec<&str> = outs.iter().map(Outcome::report_type).collect();
    reports.sort();
    reports.dedup();
    let mut optional: Vec<&str> = skipped.iter().map(|o| o.report_type()).collect();
    optional.sort();
    optional.dedup();
    let mut s = String::new();
    s.push_str("<!-- policy_violation_comment -->\n");
    s.push_str(&format!("<!-- violated_reports: {} -->\n", reports.join(",")));
    if optional.is_empty() {
        s.push('\n');
    } else {
        s.push_str(&format!("<!-- optional_approvals: {} -->\n", optional.join(",")));
    }
    s.push('\n');
    s.push_str(&format!("{}, this merge request has policy violations and errors.\n", author.name));
    // Policies are named in alphabetical order.
    let names = |v: &[&Outcome]| {
        let mut n: Vec<String> = v.iter().map(|o| o.policy.name.clone()).collect();
        n.sort();
        n.join(", ")
    };
    let any_mr: Vec<&Outcome> = failed.iter().copied().filter(|o| o.report_type() == "any_merge_request").collect();
    let closed_errors = failed.iter().any(|o| !o.errors.is_empty());
    let mut items = vec![];
    if !failed.is_empty() {
        items.push(format!("- Resolve all violations in the following merge request approval policies: {}.", names(&failed)));
    }
    if !any_mr.is_empty() {
        items.push(format!("- Acquire approvals from eligible approvers defined in the following merge request approval policies: {}.", names(&any_mr)));
    }
    if closed_errors {
        items.push("- Resolve the errors and re-run the pipeline.".into());
    }
    if items.is_empty() {
        s.push('\n');
        if !optional.is_empty() {
            s.push_str("\nConsider including optional reviewers based on the policy rules in the MR widget.\n");
        }
    } else {
        s.push_str("**To unblock this merge request, fix these items:**\n\n");
        for i in &items {
            s.push_str(i);
            s.push('\n');
        }
        s.push_str("If you think these items shouldn't be violations, ask eligible approvers of each policy to approve this merge request.\n");
    }
    s.push('\n');
    let warn: Vec<&Outcome> = failed.iter().copied().filter(|o| o.policy.warn()).collect();
    if !warn.is_empty() {
        s.push_str("**Note:** The following policies are in warn mode and can be bypassed to make approvals optional: \n");
        for o in &warn {
            s.push_str(&format!("- {}\n", o.policy.name));
        }
    }
    s.push('\n');
    if !skipped.is_empty() {
        s.push_str(&format!(
            "The following policies enforced on your project were skipped because they are configured to fail open: {}.\n\n",
            names(&skipped)
        ));
        if skipped.iter().any(|o| o.report_type() == "scan_finding") {
            s.push_str("- Confirm that scanners are properly configured and producing results. Vulnerability detection depends on successful execution of security scan jobs in the target and source branches.\n");
        }
        if skipped.iter().any(|o| o.report_type() == "license_scanning") {
            s.push_str("- Confirm that dependency scanning is properly configured and is producing results. License detection depends on SBOM reports produced by the dependency scanning job in the target and source branches.\n");
        }
    }
    s.push('\n');
    let section = |s: &mut String, heading: &str, intro: &str, list: &[&Outcome]| {
        if list.is_empty() {
            return;
        }
        s.push_str(heading);
        s.push('\n');
        s.push_str(intro);
        s.push_str("\n\n");
        let mut unsigned: Vec<String> = vec![];
        let mut list = list.to_vec();
        list.sort_by(|a, b| a.policy.name.cmp(&b.policy.name));
        for o in list {
            let what = if o.rule["commits"].as_str() == Some("unsigned") { "any unsigned commits" } else { "any commits" };
            s.push_str(&format!("* {}: {what}\n", o.policy.name));
            if let Some(Value::Array(a)) = &o.commits {
                for x in a.iter().filter_map(Value::as_str) {
                    if !unsigned.iter().any(|u| u == x) {
                        unsigned.push(x.to_string());
                    }
                }
            }
        }
        s.push('\n');
        if !unsigned.is_empty() {
            s.push_str("Unsigned commits:\n\n");
            for x in &unsigned {
                s.push_str(&format!("1. [`{x}`]({}/-/commit/{x})\n", config.url(&format!("/{}", p.full_path))));
            }
        }
        s.push('\n');
    };
    let enforced: Vec<&Outcome> = outs.iter().filter(|o| !o.policy.warn()).collect();
    if !enforced.is_empty() {
        s.push_str("### :warning: **Violations blocking this merge request**\n\n");
        let list: Vec<&Outcome> = failed.iter().copied().filter(|o| !o.policy.warn() && o.report_type() == "any_merge_request").collect();
        section(&mut s, "#### Enforced `any_merge_request` violations", "The following policies require approval:", &list);
        s.push_str("\n\n---\n");
    }
    s.push('\n');
    if !warn.is_empty() {
        s.push_str("### :construction: **Violations from warn mode policies that users can bypass**\n\n");
        let list: Vec<&Outcome> = warn.iter().copied().filter(|o| o.report_type() == "any_merge_request").collect();
        section(&mut s, "#### `any_merge_request` violations that users can bypass", "The following policies require either approval or bypass:", &list);
        s.push_str("\n\n---\n");
    }
    s.push('\n');
    let errors: Vec<String> = outs.iter().flat_map(|o| o.errors.iter().filter_map(|e| e["message"].as_str().map(str::to_string))).collect();
    if !errors.is_empty() {
        s.push_str("\n:exclamation: **Errors**\n\n");
        for e in errors {
            s.push_str(&format!("- {e}\n"));
        }
    }
    s
}

/// The policy's result on the MR as labrat's test endpoint set it, if it did.
fn result_override(c: &Connection, mr_id: i64, policy_id: i64) -> Result<Option<Value>> {
    let raw: Option<String> = c
        .query_row(
            "SELECT result FROM security_policy_mr_states WHERE merge_request_id = ?1 AND security_policy_id = ?2",
            params![mr_id, policy_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

const SEVERITIES: &[&str] = &["INFO", "UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL"];
const VULN_REPORT_TYPES: &[&str] =
    &["SAST", "DEPENDENCY_SCANNING", "CONTAINER_SCANNING", "DAST", "SECRET_DETECTION", "COVERAGE_FUZZING", "API_FUZZING", "CLUSTER_IMAGE_SCANNING", "CONTAINER_SCANNING_FOR_REGISTRY", "SARIF", "GENERIC"];
const ERROR_TYPES: &[&str] = &["SCAN_REMOVED", "ARTIFACTS_MISSING", "SCAN_NOT_SUCCEEDED", "UNKNOWN"];
const REPORT_TYPES: &[&str] = &["SCAN_FINDING", "LICENSE_SCANNING", "ANY_MERGE_REQUEST"];
const STATUSES: &[&str] = &["FAILED", "RUNNING", "WARNING", "SKIPPED"];

/// Checks one policy's result against GraphQL's types, filling in what GraphQL can't leave out:
/// `{"violated": bool, "status": "FAILED", "newScanFinding": [{"name", "path", "reportType",
/// "severity", "location"}], "previousScanFinding": [...], "licenseScanning": [{"license",
/// "dependencies", "url"}], "errors": [{"error", "reportType", "message", "data"}],
/// "comparisonPipelines": [{"reportType", "source", "target"}]}`.
pub fn validate_result(policy: &Policy, v: &Value) -> std::result::Result<Value, String> {
    let Some(m) = v.as_object() else { return Err("a result must be an object".into()) };
    let known = ["violated", "status", "newScanFinding", "previousScanFinding", "licenseScanning", "errors", "comparisonPipelines"];
    if let Some(k) = m.keys().find(|k| !known.contains(&k.as_str())) {
        return Err(format!("unknown key {k:?}"));
    }
    let mut out = Map::new();
    let violated = match m.get("violated") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("violated must be true or false".into()),
    };
    out.insert("violated".into(), json!(violated));
    let status = m.get("status").and_then(Value::as_str).unwrap_or("FAILED");
    if !STATUSES.contains(&status) {
        return Err(format!("status must be one of {STATUSES:?}"));
    }
    out.insert("status".into(), json!(status));
    let enum_of = |x: &Value, k: &str, allowed: &[&str], required: bool| -> std::result::Result<Value, String> {
        match x.get(k) {
            None | Some(Value::Null) if !required => Ok(Value::Null),
            Some(Value::String(s)) if allowed.contains(&s.as_str()) => Ok(json!(s)),
            _ => Err(format!("{k} must be one of {allowed:?}")),
        }
    };
    let str_of = |x: &Value, k: &str, required: bool| -> std::result::Result<Value, String> {
        match x.get(k) {
            None | Some(Value::Null) if !required => Ok(Value::Null),
            Some(Value::String(s)) => Ok(json!(s)),
            _ => Err(format!("{k} must be a string")),
        }
    };
    let list = |k: &str| -> std::result::Result<Vec<Value>, String> {
        match m.get(k) {
            None | Some(Value::Null) => Ok(vec![]),
            Some(Value::Array(a)) => Ok(a.clone()),
            Some(_) => Err(format!("{k} must be a list")),
        }
    };
    for k in ["newScanFinding", "previousScanFinding"] {
        let mut items = vec![];
        for f in list(k)? {
            items.push(json!({
                "location": f.get("location").cloned().unwrap_or(Value::Null),
                "name": str_of(&f, "name", false)?,
                "path": str_of(&f, "path", false)?,
                "reportType": enum_of(&f, "reportType", VULN_REPORT_TYPES, true)?,
                "severity": enum_of(&f, "severity", SEVERITIES, false)?,
            }));
        }
        out.insert(k.into(), json!(items));
    }
    let mut lic = vec![];
    for l in list("licenseScanning")? {
        let deps = match l.get("dependencies") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(a)) if a.iter().all(Value::is_string) => a.clone(),
            _ => return Err("dependencies must be a list of strings".into()),
        };
        lic.push(json!({"dependencies": deps, "license": str_of(&l, "license", true)?, "url": str_of(&l, "url", false)?}));
    }
    out.insert("licenseScanning".into(), json!(lic));
    let own_type = report_type(policy.rules().first().unwrap_or(&Value::Null)).to_uppercase();
    let mut errors = vec![];
    for e in list("errors")? {
        let rt = match e.get("reportType") {
            None | Some(Value::Null) => json!(own_type),
            _ => enum_of(&e, "reportType", REPORT_TYPES, true)?,
        };
        errors.push(json!({"data": e.get("data").cloned().unwrap_or(json!({})), "error": enum_of(&e, "error", ERROR_TYPES, true)?, "message": str_of(&e, "message", true)?, "reportType": rt}));
    }
    out.insert("errors".into(), json!(errors));
    let mut pipes = vec![];
    for cp in list("comparisonPipelines")? {
        let ids = |k: &str| -> std::result::Result<Value, String> {
            match cp.get(k) {
                None | Some(Value::Null) => Ok(Value::Null),
                Some(Value::Array(a)) if a.iter().all(Value::is_string) => Ok(Value::Array(a.clone())),
                _ => Err(format!("comparisonPipelines {k} must be a list of pipeline global ids")),
            }
        };
        let rt = match cp.get("reportType") {
            None | Some(Value::Null) => json!(own_type),
            _ => enum_of(&cp, "reportType", REPORT_TYPES, true)?,
        };
        pipes.push(json!({"reportType": rt, "source": ids("source")?, "target": ids("target")?}));
    }
    out.insert("comparisonPipelines".into(), json!(pipes));
    Ok(Value::Object(out))
}

/// labrat's test endpoint: sets policies' results on the MR, by policy name (`None`: as
/// evaluated), and evaluates the MR again.
pub fn set_results(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, results: &[(String, Option<Value>)]) -> Result<std::result::Result<(), String>> {
    let policies = project_policies(c, p.id)?;
    let mut checked = vec![];
    for (name, result) in results {
        let Some(pol) = policies.iter().find(|x| &x.name == name) else { return Ok(Err(format!("no policy named {name:?}"))) };
        let v = match result {
            Some(r) => match validate_result(pol, r) {
                Ok(v) => Some(v.to_string()),
                Err(e) => return Ok(Err(format!("{name}: {e}"))),
            },
            None => None,
        };
        checked.push((pol.id, v));
    }
    for (pid, v) in checked {
        c.execute(
            "INSERT INTO security_policy_mr_states (merge_request_id, security_policy_id, result) VALUES (?1, ?2, ?3)
             ON CONFLICT (merge_request_id, security_policy_id) DO UPDATE SET result = excluded.result",
            params![mr.id, pid, v],
        )?;
    }
    evaluate(c, config, p, mr)?;
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------------------------
// Dismissals and bypasses

/// The MR's dismissal or bypass of a policy: (dismissed, bypassed).
pub fn mr_state(c: &Connection, mr_id: i64, policy_id: i64) -> Result<(bool, bool)> {
    Ok(c
        .query_row(
            "SELECT dismissed, bypassed FROM security_policy_mr_states WHERE merge_request_id = ?1 AND security_policy_id = ?2",
            params![mr_id, policy_id],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
        )
        .optional()?
        .unwrap_or((false, false)))
}

/// The policy behind an MR rule copied from a policy's project rule.
pub fn rule_policy(c: &Connection, rule: &Rule) -> Result<Option<Policy>> {
    let Some(src) = rule.source_id else { return Ok(None) };
    let pid: Option<i64> = c.query_row("SELECT security_policy_id FROM approval_project_rules WHERE id = ?1", [src], |r| r.get(0)).optional()?.flatten();
    match pid {
        Some(id) => find(c, id),
        None => Ok(None),
    }
}

/// Whether a policy rule asks for no approvals because its violation was dismissed or the
/// policy bypassed.
pub fn waived(c: &Connection, mr_id: i64, rule: &Rule) -> Result<bool> {
    if rule.rule_type != "report_approver" {
        return Ok(false);
    }
    match rule_policy(c, rule)? {
        Some(pol) => {
            let (d, b) = mr_state(c, mr_id, pol.id)?;
            Ok(d || b)
        }
        None => Ok(false),
    }
}

/// `dismissPolicyViolations`: dismisses the violations of the given warn mode policies.
pub fn dismiss(c: &Connection, p: &Project, mr: &MergeRequest, user: &User, ids: &[i64], types: &[String], comment: &str) -> Result<Vec<String>> {
    let outs = outcomes(c, p, mr)?;
    let targets: Vec<&Outcome> = outs.iter().filter(|o| ids.contains(&o.policy.id) && o.policy.warn() && o.status == "FAILED").collect();
    if targets.is_empty() {
        return Ok(vec!["No warn mode policies are found.".into()]);
    }
    for o in targets {
        c.execute(
            "INSERT INTO security_policy_mr_states (merge_request_id, security_policy_id, dismissed, dismissal) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (merge_request_id, security_policy_id) DO UPDATE SET dismissed = 1, dismissal = excluded.dismissal",
            params![mr.id, o.policy.id, json!({"user_id": user.id, "types": types, "comment": comment, "at": time::now()}).to_string()],
        )?;
    }
    Ok(vec![])
}

/// Whether `user` may bypass `policy` (its `bypass_settings` name them, a group of theirs or
/// their role in the project).
pub fn can_bypass(c: &Connection, p: &Project, policy: &Policy, user: &User) -> Result<bool> {
    let bs = &policy.content["bypass_settings"];
    let ids = |k: &str| -> Vec<i64> { bs[k].as_array().map(|a| a.iter().filter_map(|x| x["id"].as_i64().or(x.as_i64())).collect()).unwrap_or_default() };
    if ids("users").contains(&user.id) {
        return Ok(true);
    }
    for g in ids("groups") {
        if ap::group_member_ids(c, g)?.contains(&user.id) {
            return Ok(true);
        }
    }
    if let Some(roles) = bs["roles"].as_array() {
        let level = crate::perms::ProjectPerms::load(c, Some(user), p)?.level;
        for r in roles.iter().filter_map(Value::as_str) {
            let want = match r {
                "guest" => 10,
                "planner" => 15,
                "reporter" => 20,
                "developer" => 30,
                "maintainer" => 40,
                "owner" => 50,
                _ => i64::MAX,
            };
            if level == want {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `mergeRequestBypassSecurityPolicy`: bypasses the given policies, with a reason.
pub fn bypass(c: &Connection, p: &Project, mr: &MergeRequest, user: &User, ids: &[i64], reason: &str) -> Result<Option<String>> {
    let outs = outcomes(c, p, mr)?;
    let targets: Vec<&Outcome> = outs.iter().filter(|o| ids.contains(&o.policy.id)).collect();
    if targets.is_empty() {
        return Ok(Some("Security policies not found".into()));
    }
    for o in &targets {
        if !o.policy.has_bypass_settings() || !can_bypass(c, p, &o.policy, user)? {
            return Ok(Some("You are not allowed to bypass this security policy.".into()));
        }
    }
    for o in targets {
        c.execute(
            "INSERT INTO security_policy_mr_states (merge_request_id, security_policy_id, bypassed, bypass) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (merge_request_id, security_policy_id) DO UPDATE SET bypassed = 1, bypass = excluded.bypass",
            params![mr.id, o.policy.id, json!({"user_id": user.id, "reason": reason, "at": time::now()}).to_string()],
        )?;
    }
    Ok(None)
}

// ---------------------------------------------------------------------------------------------
// Effects on approvals and merging

/// The approval settings policies with violations on the MR override (`true` ones only).
pub fn overriding_settings(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for o in outcomes(c, p, mr)?.iter().filter(|o| o.status == "FAILED") {
        for (k, v) in o.policy.true_settings() {
            out.insert(k, v);
        }
    }
    Ok(out)
}

/// Whether a policy with `approval_settings.<setting>` covers the protected branch `branch`.
/// These settings apply while the policy is in force, violations or not.
fn branch_setting(c: &Connection, p: &Project, branch: &str, setting: &str) -> Result<bool> {
    if !crate::repo::is_protected(c, p, branch)? {
        return Ok(false);
    }
    for pol in project_policies(c, p.id)? {
        if pol.content["approval_settings"][setting].as_bool() != Some(true) {
            continue;
        }
        for rule in pol.rules() {
            if covers(c, p, &rule, branch)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `prevent_pushing_and_force_pushing`: pushes (and API commits) to the branch are refused.
pub fn push_blocked(c: &Connection, p: &Project, branch: &str) -> Result<bool> {
    branch_setting(c, p, branch, "prevent_pushing_and_force_pushing")
}

pub const PUSH_BLOCKED: &str = "Push is blocked by settings overridden by a security policy";

/// `block_branch_modification`: the branch's protection can't be changed or removed, nor the
/// branch deleted or made the default.
pub fn modification_blocked(c: &Connection, p: &Project, branch: &str) -> Result<bool> {
    branch_setting(c, p, branch, "block_branch_modification")
}

/// Whether approving the MR needs the approver's password: the project's setting, or a policy
/// violated on the MR.
pub fn password_required(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<bool> {
    Ok(ap::effective(c, p)?.require_password_to_approve.value || overriding_settings(c, p, mr)?.contains_key("require_password_to_approve"))
}

/// The `SECURITY_POLICY_VIOLATIONS` check: `INACTIVE` without policies, `SUCCESS` when every
/// violated policy's rule has its approvals, else `WARNING` when the MR is approved anyway
/// (dismissed or bypassed policies) and `FAILED` when it isn't.
pub fn check_status(c: &Connection, p: &Project, mr: &MergeRequest, st: &ap::State) -> Result<&'static str> {
    let policy_rules: Vec<&ap::Wrapped> = st.rules.iter().filter(|w| w.rule.rule_type == "report_approver").collect();
    if policy_rules.is_empty() {
        return Ok("INACTIVE");
    }
    let short = policy_rules.iter().any(|w| (w.approved_by.len() as i64) < w.rule.approvals_required);
    if !short {
        return Ok("SUCCESS");
    }
    let _ = (c, p, mr);
    Ok(if st.approved() { "WARNING" } else { "FAILED" })
}

/// The `security_policy_violations` merge check fails.
pub fn blocks_merge(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<bool> {
    let st = ap::state(c, p, mr)?;
    Ok(check_status(c, p, mr, &st)? == "FAILED")
}
