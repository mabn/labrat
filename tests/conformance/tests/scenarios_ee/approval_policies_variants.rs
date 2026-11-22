//! EE approval policies one at a time: a warn mode policy alone (it blocks until dismissed), a
//! scan policy that fails open (nothing to approve), a license policy that fails closed, and
//! removing the policies. Each shows its own wording of the security bot's comment.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;

const MR_QUERY: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  approved approvalsRequired approvalsLeft detailedMergeStatus
  mergeabilityChecks { identifier status }
  approvalState { rules { name type approvalsRequired approved } }
  policyViolations { violationsCount policies { name reportType status dismissed enforcementType securityPolicyId }
    anyMergeRequest { name commits } errors { error reportType message } }
} } }";

const DISMISS: &str = "mutation($path: ID!, $iid: String!, $ids: [ID!]!) {
  dismissPolicyViolations(input: {projectPath: $path, iid: $iid, securityPolicyIds: $ids, dismissalTypes: [OTHER], comment: \"Known\"}) {
    errors mergeRequest { detailedMergeStatus } } }";

fn warn_any(approver: &str) -> Value {
    json!({"name": "Warn any", "description": "", "enabled": true, "enforcement_type": "warn",
        "rules": [{"type": "any_merge_request", "branch_type": "protected", "commits": "any"}],
        "actions": [{"type": "require_approval", "approvals_required": 1, "user_approvers": [approver]}]})
}

fn scan_open(approver: &str) -> Value {
    json!({"name": "Scan open", "description": "", "enabled": true, "fallback_behavior": {"fail": "open"},
        "rules": [{"type": "scan_finding", "branches": [], "scanners": ["dependency_scanning"], "vulnerabilities_allowed": 0,
                   "severity_levels": ["high"], "vulnerability_states": ["new_needs_triage"]}],
        "actions": [{"type": "require_approval", "approvals_required": 1, "user_approvers": [approver]}]})
}

fn license_closed(approver: &str) -> Value {
    json!({"name": "License closed", "description": "", "enabled": true,
        "rules": [{"type": "license_finding", "branch_type": "protected", "match_on_inclusion_license": false,
                   "license_types": ["MIT License"], "license_states": ["newly_detected"]}],
        "actions": [{"type": "require_approval", "approvals_required": 1, "user_approvers": [approver]}]})
}

async fn mr(s: &mut Scenario, pid: i64, i: i64, token: &str) {
    s.step(
        &format!("commit {i}"),
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(token),
        json!({"branch": format!("b{i}"), "start_branch": "main", "commit_message": format!("Add {i}"), "actions": [{"action": "create", "file_path": format!("{i}.txt"), "content": "x\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.step(&format!("mr {i}"), POST, &format!("/projects/{pid}/merge_requests"), Some(token), json!({"source_branch": format!("b{i}"), "target_branch": "main", "title": format!("MR {i}")}), "mr").await;
    s.poll_mr(pid, i).await;
    s.poll(&format!("/projects/{pid}/merge_requests/{i}/notes"), |b| b.as_array().is_some_and(|a| a.iter().any(|n| n["system"] == false))).await;
}

#[tokio::test]
async fn approval_policies_variants() {
    let mut s = Scenario::new("approval_policies_variants").await;
    let admin = s.admin();
    let gpath = s.name("polv");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let (author, author_t) = user_with_token(&mut s, "author").await;
    let (rev, _rev_t) = user_with_token(&mut s, "rev").await;
    for (label, uid) in [("author member", author), ("rev member", rev)] {
        s.step(label, POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": uid, "access_level": 30}), "user").await;
    }
    let rev_n = s.name("rev");
    let mrs = format!("/projects/{pid}/merge_requests");

    // A warn mode policy alone: it blocks until its violation is dismissed.
    s.approval_policies(&full, json!([warn_any(&rev_n)])).await;
    mr(&mut s, pid, 1, &author_t).await;
    s.step("mr 1", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!(null), "note").await;
    let g1 = s.graphql("graphql 1", Some(&admin), MR_QUERY, json!({"path": full, "iid": "1"})).await;
    let id = g1.body["data"]["project"]["mergeRequest"]["policyViolations"]["policies"][0]["securityPolicyId"].as_str().unwrap_or_default().to_string();
    s.graphql("dismiss", Some(&author_t), DISMISS, json!({"path": full, "iid": "1", "ids": [id]})).await;
    s.settle().await;
    s.step("mr 1 dismissed", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.graphql("graphql 1 dismissed", Some(&admin), MR_QUERY, json!({"path": full, "iid": "1"})).await;

    // A scan policy that fails open: nothing to approve.
    s.approval_policies(&full, json!([scan_open(&rev_n)])).await;
    mr(&mut s, pid, 2, &author_t).await;
    s.step("mr 2", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!(null), "note").await;
    s.graphql("graphql 2", Some(&admin), MR_QUERY, json!({"path": full, "iid": "2"})).await;

    // A license policy that fails closed.
    s.approval_policies(&full, json!([license_closed(&rev_n)])).await;
    mr(&mut s, pid, 3, &author_t).await;
    s.step("mr 3", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.step("mr 3 notes", GET, &format!("{mrs}/3/notes"), Some(&admin), json!(null), "note").await;
    s.graphql("graphql 3", Some(&admin), MR_QUERY, json!({"path": full, "iid": "3"})).await;

    // Without policies, the MRs keep the bot's comment but lose the policy rules.
    s.approval_policies(&full, json!([])).await;
    s.step("mr 3 rules after removal", GET, &format!("{mrs}/3/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.step("mr 3 notes after removal", GET, &format!("{mrs}/3/notes"), Some(&admin), json!(null), "note").await;
    s.graphql("graphql 3 after removal", Some(&admin), MR_QUERY, json!({"path": full, "iid": "3"})).await;
    s.step("mr 3 after removal", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
