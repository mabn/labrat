//! EE approval policies as merge requests see them: the policy approval rules next to the
//! project's own, the policy violations and their details, the bot comment, warn mode
//! (dismissing violations), bypassing a policy, policies overriding approval settings, the
//! `security_policy_violations` merge status and check, and what a push does to all of it.
//!
//! labrat doesn't evaluate policies against CI results: when replaying,
//! `Scenario::approval_policies` hands the policies to labrat's test endpoint, and labrat
//! reports what EE reports without security reports (scan and license rules can't be
//! evaluated, every commit is unsigned).

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

const MR_QUERY: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  approved approvalsRequired approvalsLeft detailedMergeStatus mergeable
  mergeabilityChecks { identifier status }
  approvalState {
    approvalRulesOverwritten
    suggestedApprovers { nodes { username } }
    rules { id name type approvalsRequired approved overridden section invalid allowMergeWhenInvalid
      eligibleApprovers { username } users { nodes { username } } approvedBy { nodes { username } }
      scanResultPolicies { name approvalsRequired reportType } sourceRule { id name approvalsRequired } }
  }
  policyViolations { violationsCount
    policies { name reportType status dismissed enforcementType securityPolicyId }
    anyMergeRequest { name commits } newScanFinding { name } previousScanFinding { name }
    licenseScanning { license dependencies url }
    errors { error reportType message data }
    comparisonPipelines { reportType source target } }
  policiesOverridingApprovalSettings { name editPath settings }
  policyBypassStatuses { id name allowBypass bypassed }
} } }";

const DISMISS: &str = "mutation($path: ID!, $iid: String!, $ids: [ID!]!, $types: [DismissalType!]!, $comment: String!) {
  dismissPolicyViolations(input: {projectPath: $path, iid: $iid, securityPolicyIds: $ids, dismissalTypes: $types, comment: $comment}) {
    errors mergeRequest { detailedMergeStatus approvalsLeft } } }";

const BYPASS: &str = "mutation($path: ID!, $iid: String!, $ids: [ID!]!, $reason: String!) {
  mergeRequestBypassSecurityPolicy(input: {projectPath: $path, iid: $iid, securityPolicyIds: $ids, reason: $reason}) {
    errors mergeRequest { detailedMergeStatus approvalsLeft } } }";

fn policies(bob: &str, alice: &str, carol: &str, carol_id: i64) -> Value {
    json!([
        {
            "name": "Any MR needs bob",
            "description": "Every merge request needs bob",
            "enabled": true,
            "rules": [{"type": "any_merge_request", "branch_type": "protected", "commits": "any"}],
            "actions": [{"type": "require_approval", "approvals_required": 1, "user_approvers": [bob]}],
            "approval_settings": {"prevent_approval_by_author": true, "require_password_to_approve": false},
            "bypass_settings": {"users": [{"id": carol_id}]},
        },
        {
            "name": "No critical vulns",
            "description": "",
            "enabled": true,
            "rules": [{"type": "scan_finding", "branches": [], "scanners": ["sast"], "vulnerabilities_allowed": 0,
                       "severity_levels": ["critical"], "vulnerability_states": ["new_needs_triage"]}],
            "actions": [{"type": "require_approval", "approvals_required": 2, "user_approvers": [alice, bob]}],
        },
        {
            "name": "Warn unsigned",
            "description": "Only a warning",
            "enabled": true,
            "enforcement_type": "warn",
            "rules": [{"type": "any_merge_request", "branch_type": "protected", "commits": "unsigned"}],
            "actions": [{"type": "require_approval", "approvals_required": 1, "user_approvers": [carol]}],
        },
        {
            "name": "Licenses open",
            "description": "",
            "enabled": true,
            "fallback_behavior": {"fail": "open"},
            "rules": [{"type": "license_finding", "branch_type": "protected", "match_on_inclusion_license": true,
                       "license_types": ["MIT License"], "license_states": ["newly_detected"]}],
            "actions": [{"type": "require_approval", "approvals_required": 1, "user_approvers": [bob]}],
        },
    ])
}

/// The security policy ids `policyViolations` reports, by policy name.
fn policy_id(res: &labrat_conformance::Res, name: &str) -> String {
    res.body["data"]["project"]["mergeRequest"]["policyViolations"]["policies"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["name"] == name))
        .and_then(|p| p["securityPolicyId"].as_str())
        .unwrap_or_else(|| panic!("no policy {name}: {}", res.body))
        .to_string()
}

#[tokio::test]
async fn approval_policies() {
    let mut s = Scenario::new("approval_policies").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let gpath = s.name("pol");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let (author, author_t) = user_with_token(&mut s, "author").await;
    let (alice, alice_t) = user_with_token(&mut s, "alice").await;
    let (bob, bob_t) = user_with_token(&mut s, "bob").await;
    let (carol, carol_t) = user_with_token(&mut s, "carol").await;
    for (label, uid) in [("author member", author), ("alice member", alice), ("bob member", bob), ("carol member", carol)] {
        s.step(label, POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": uid, "access_level": 30}), "user").await;
    }
    // The project lets authors approve; a policy takes that back.
    s.step("author approval", POST, &format!("/projects/{pid}/approvals"), Some(&admin), json!({"merge_requests_author_approval": true}), "approval").await;
    s.step("project rule", POST, &format!("/projects/{pid}/approval_rules"), Some(&admin), json!({"name": "Team", "approvals_required": 1, "user_ids": [alice]}), "rule").await;
    let (alice_n, bob_n, carol_n) = (s.name("alice"), s.name("bob"), s.name("carol"));
    s.approval_policies(&full, policies(&bob_n, &alice_n, &carol_n, carol)).await;
    let hook = s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": rx.url, "merge_requests_events": true, "note_events": true, "push_events": false}),
        "hook",
    )
    .await;

    // The author opens an MR into the protected default branch.
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("commit", POST, &commits, Some(&author_t), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&author_t), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.poll(&format!("{mrs}/1/notes"), |b| b.as_array().is_some_and(|a| a.iter().any(|n| n["system"] == false))).await;
    s.deliveries("open hooks", &mut rx, 2).await;
    let mr = format!("{mrs}/1");
    let vars = json!({"path": full, "iid": "1"});
    s.step("mr show", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("approvals", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("approvals as author", GET, &format!("{mr}/approvals"), Some(&author_t), json!(null), "approval").await;
    s.step("approval state", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("approval settings", GET, &format!("{mr}/approval_settings"), Some(&admin), json!(null), "approval").await;
    let rules = s.step("mr rules", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;
    let policy_rule = rules.body.as_array().unwrap().iter().find(|r| r["rule_type"] == "report_approver").unwrap()["id"].as_i64().unwrap();
    s.step("notes", GET, &format!("{mr}/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    let g1 = s.graphql("graphql", Some(&admin), MR_QUERY, vars.clone()).await;
    s.graphql("graphql as carol", Some(&carol_t), MR_QUERY, vars.clone()).await;
    let (any_id, warn_id, scan_id) = (policy_id(&g1, "Any MR needs bob"), policy_id(&g1, "Warn unsigned"), policy_id(&g1, "No critical vulns"));
    s.step("merge", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;

    // Policy rules can't be changed through the API.
    s.step("update policy rule", PUT, &format!("{mr}/approval_rules/{policy_rule}"), Some(&admin), json!({"approvals_required": 3}), "rule").await;
    s.step("delete policy rule", DELETE, &format!("{mr}/approval_rules/{policy_rule}"), Some(&admin), json!(null), "rule").await;

    // The policy keeps the author from approving.
    s.step("author approves", POST, &format!("{mr}/approve"), Some(&author_t), json!(null), "approval").await;
    s.step("bob approves", POST, &format!("{mr}/approve"), Some(&bob_t), json!(null), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.graphql("graphql after bob", Some(&admin), MR_QUERY, vars.clone()).await;

    // Warn mode: violations can be dismissed, enforced ones can't.
    let dv = |ids: Vec<String>| json!({"path": full, "iid": "1", "ids": ids, "types": ["POLICY_FALSE_POSITIVE"], "comment": "Not relevant"});
    s.graphql("dismiss enforced", Some(&admin), DISMISS, dv(vec![scan_id.clone()])).await;
    s.graphql("dismiss as outsider", Some(&alice_t), DISMISS, dv(vec![warn_id.clone()])).await;
    s.graphql("dismiss warn", Some(&admin), DISMISS, dv(vec![warn_id.clone()])).await;
    s.settle().await;
    s.graphql("graphql after dismissal", Some(&admin), MR_QUERY, vars.clone()).await;

    // Bypassing: only users the policy names may.
    let bv = |ids: Vec<String>| json!({"path": full, "iid": "1", "ids": ids, "reason": "Hotfix"});
    s.graphql("bypass as alice", Some(&alice_t), BYPASS, bv(vec![any_id.clone()])).await;
    s.graphql("bypass without settings", Some(&carol_t), BYPASS, bv(vec![scan_id.clone()])).await;
    s.graphql("bypass as carol", Some(&carol_t), BYPASS, bv(vec![any_id.clone()])).await;
    s.settle().await;
    s.graphql("graphql after bypass as carol", Some(&carol_t), MR_QUERY, vars.clone()).await;

    // The scan policy fails closed: two approvals from its approvers.
    s.step("alice approves", POST, &format!("{mr}/approve"), Some(&alice_t), json!(null), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("approvals approved", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("approval state approved", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("mr approved", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.graphql("graphql approved", Some(&admin), MR_QUERY, vars.clone()).await;
    s.step("notes approved", GET, &format!("{mr}/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.deliveries("approval hooks", &mut rx, 3).await;

    // A push: approvals reset, the violations are evaluated again. (The bot comment goes
    // through intermediate states meanwhile, which differ between runs on EE: no note hooks.)
    s.step("hook without notes", PUT, &format!("/projects/{pid}/hooks/{}", hook.id()), Some(&admin), json!({"url": rx.url, "note_events": false}), "hook").await;
    s.step("push", POST, &commits, Some(&author_t), json!({"branch": "feature", "commit_message": "Edit a", "actions": [{"action": "update", "file_path": "a.txt", "content": "a2\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    // EE resets the approvals a moment after the push.
    s.poll(&format!("{mr}/approvals"), |b| b["approved_by"].as_array().is_some_and(|a| a.is_empty())).await;
    s.step("approval state after push", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.graphql("graphql after push", Some(&admin), MR_QUERY, vars.clone()).await;
    s.step("notes after push", GET, &format!("{mr}/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.deliveries("push hooks", &mut rx, 2).await;

    // An MR into an unprotected branch gets no policy rules.
    s.step("branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "dev", "ref": "main"}), "branch").await;
    s.step("commit 2", POST, &commits, Some(&author_t), json!({"branch": "f2", "start_branch": "main", "commit_message": "Add b", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr 2", POST, &mrs, Some(&author_t), json!({"source_branch": "f2", "target_branch": "dev", "title": "Add b"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("mr 2 rules", GET, &format!("{mrs}/2/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.graphql("graphql mr 2", Some(&admin), MR_QUERY, json!({"path": full, "iid": "2"})).await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.settle().await;
    s.deliveries("mr 2 hooks", &mut rx, 1).await;
    s.finish();
}
