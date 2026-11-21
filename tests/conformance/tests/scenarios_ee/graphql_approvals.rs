//! EE approvals over GraphQL: the MR's approval fields and state, mergeability checks, branch
//! rule approval rules and the approval rule mutations.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

const MR_QUERY: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  approved approvalsRequired approvalsLeft detailedMergeStatus
  approvedBy { nodes { username } }
  userPermissions { canApprove }
  mergeabilityChecks { identifier status }
  approvalState {
    approvalRulesOverwritten
    invalidApproversRules { id name }
    suggestedApprovers { nodes { username } }
    rules { id name type approvalsRequired approved overridden section containsHiddenGroups invalid allowMergeWhenInvalid
      eligibleApprovers { username } users { nodes { username } } groups { nodes { fullPath } }
      approvedBy { nodes { username } } commentedBy { nodes { username } }
      sourceRule { id name type approvalsRequired } }
  } } } }";

#[tokio::test]
async fn graphql_approvals() {
    let mut s = Scenario::new("graphql_approvals").await;
    let admin = s.admin();
    let gpath = s.name("gqa");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let (author, author_t) = user_with_token(&mut s, "author").await;
    let (a1, a1_t) = user_with_token(&mut s, "alice").await;
    let (a2, _a2_t) = user_with_token(&mut s, "bob").await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    for (label, uid, level) in [("author member", author, 30), ("alice member", a1, 30), ("bob member", a2, 40), ("dev member", dev, 30)] {
        s.step(label, POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": uid, "access_level": level}), "user").await;
    }
    s.settle().await;
    let rules = format!("/projects/{pid}/approval_rules");
    s.step("rule backend", POST, &rules, Some(&admin), json!({"name": "Backend", "approvals_required": 1, "user_ids": [a1, a2]}), "rule").await;
    s.step("rule any", POST, &rules, Some(&admin), json!({"name": "Anyone", "approvals_required": 2, "rule_type": "any_approver"}), "rule").await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&author_t), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&author_t), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let vars = json!({"path": full, "iid": "1"});
    s.graphql("mr as admin", Some(&admin), MR_QUERY, vars.clone()).await;
    s.graphql("mr as author", Some(&author_t), MR_QUERY, vars.clone()).await;
    s.step("alice approves", POST, &format!("{mrs}/1/approve"), Some(&a1_t), json!(null), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.graphql("mr after alice", Some(&admin), MR_QUERY, vars.clone()).await;
    s.graphql("mr as alice", Some(&a1_t), MR_QUERY, vars.clone()).await;

    // Changing an MR rule.
    let mr_rules = s.step("mr rules", GET, &format!("{mrs}/1/approval_rules"), Some(&admin), json!(null), "rule").await;
    let backend = mr_rules.body.as_array().and_then(|a| a.iter().find(|r| r["name"] == "Backend")).and_then(|r| r["id"].as_i64()).unwrap_or(0);
    let update = "mutation($path: ID!, $iid: String!, $rule: Int!, $n: Int!, $name: String!, $users: [String!]) {
      mergeRequestUpdateApprovalRule(input: {projectPath: $path, iid: $iid, approvalRuleId: $rule, approvalsRequired: $n, name: $name, userIds: $users}) {
        errors mergeRequest { approvalsRequired approvalsLeft approvalState { rules { name type approvalsRequired overridden users { nodes { username } } sourceRule { approvalsRequired } } } } } }";
    s.graphql("update mr rule", Some(&admin), update, json!({"path": full, "iid": "1", "rule": backend, "n": 2, "name": "Backend team", "users": [a2.to_string()]})).await;
    s.graphql("update mr rule as dev", Some(&dev_t), update, json!({"path": full, "iid": "1", "rule": backend, "n": 1, "name": "x", "users": []})).await;
    s.graphql("update missing rule", Some(&admin), update, json!({"path": full, "iid": "1", "rule": 999999999, "n": 1, "name": "x", "users": []})).await;
    s.graphql("update bad count", Some(&admin), update, json!({"path": full, "iid": "1", "rule": backend, "n": -1, "name": "Backend team", "users": [a2.to_string()]})).await;
    s.graphql("mr after update", Some(&admin), MR_QUERY, vars.clone()).await;

    // Branch rules carry the project rules.
    let branch_rules = "query($path: ID!) { project(fullPath: $path) { mergeRequestsDisableCommittersApproval branchRules { nodes { id name isDefault isProtected
      branchProtection { allowForcePush codeOwnerApprovalRequired }
      approvalRules { nodes { id name type approvalsRequired eligibleApprovers { nodes { username } } } } } } } }";
    let br = s.graphql("branch rules", Some(&admin), branch_rules, json!({"path": full})).await;
    let br_id = br.body["data"]["project"]["branchRules"]["nodes"][0]["id"].as_str().unwrap_or("").to_string();
    let create = "mutation($br: ProjectsBranchRuleID!, $name: String!, $n: Int!, $users: [ID!]) {
      branchRuleApprovalProjectRuleCreate(input: {branchRuleId: $br, name: $name, approvalsRequired: $n, userIds: $users}) {
        errors approvalRule { id name type approvalsRequired eligibleApprovers { nodes { username } } } } }";
    let created = s
        .graphql("create branch rule approval", Some(&admin), create, json!({"br": br_id, "name": "Main reviewers", "n": 1, "users": [format!("gid://gitlab/User/{dev}")]}))
        .await;
    s.graphql("create duplicate", Some(&admin), create, json!({"br": br_id, "name": "Main reviewers", "n": 1, "users": []})).await;
    s.graphql("create as dev", Some(&dev_t), create, json!({"br": br_id, "name": "Mine", "n": 1, "users": []})).await;
    s.graphql("create bad branch rule", Some(&admin), create, json!({"br": "gid://gitlab/Projects::BranchRule/999999999", "name": "x", "n": 1, "users": []})).await;
    let rule_gid = created.body["data"]["branchRuleApprovalProjectRuleCreate"]["approvalRule"]["id"].as_str().unwrap_or("").to_string();
    let upd = "mutation($id: ApprovalProjectRuleID!, $name: String!, $n: Int!, $users: [ID!]) {
      approvalProjectRuleUpdate(input: {id: $id, name: $name, approvalsRequired: $n, userIds: $users}) {
        errors approvalRule { id name type approvalsRequired eligibleApprovers { nodes { username } } } } }";
    s.graphql("update project rule", Some(&admin), upd, json!({"id": rule_gid, "name": "Main team", "n": 2, "users": [format!("gid://gitlab/User/{a1}"), format!("gid://gitlab/User/{dev}")]})).await;
    s.graphql("update project rule bad", Some(&admin), upd, json!({"id": rule_gid, "name": "", "n": 2, "users": []})).await;
    s.graphql("update project rule as dev", Some(&dev_t), upd, json!({"id": rule_gid, "name": "x", "n": 1, "users": []})).await;
    s.step("project rules rest", GET, &rules, Some(&admin), json!(null), "rule").await;
    s.graphql("branch rules after", Some(&admin), branch_rules, json!({"path": full})).await;
    let del = "mutation($id: ApprovalProjectRuleID!) { approvalProjectRuleDelete(input: {id: $id}) { errors approvalRule { id name } } }";
    s.graphql("delete project rule as dev", Some(&dev_t), del, json!({"id": rule_gid})).await;
    s.graphql("delete project rule", Some(&admin), del, json!({"id": rule_gid})).await;
    s.graphql("delete project rule again", Some(&admin), del, json!({"id": rule_gid})).await;
    s.graphql("branch rules final", Some(&admin), branch_rules, json!({"path": full})).await;
    s.finish();
}
