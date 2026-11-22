//! EE code owners as merge requests see them: the `code_owner` approval rules an MR gets from
//! `CODEOWNERS` (sections, approval counts, optional sections, users and groups), how they show
//! in the approval endpoints and GraphQL, that they count only when the target branch requires
//! code owner approval, that they can't be edited, approvals and hooks, rules following pushes
//! and changes to `CODEOWNERS`, and selective code owner approval removal.
//!
//! labrat doesn't read `CODEOWNERS`: when replaying, `Scenario::code_owners` hands the same
//! sections to labrat's test endpoint.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;
const PATCH: Method = Method::PATCH;

const MR_QUERY: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  approved approvalsRequired approvalsLeft detailedMergeStatus mergeable
  approvedBy { nodes { username } }
  mergeabilityChecks { identifier status }
  approvalState {
    approvalRulesOverwritten
    invalidApproversRules { id name }
    suggestedApprovers { nodes { username } }
    rules { id name type approvalsRequired approved overridden section containsHiddenGroups invalid allowMergeWhenInvalid
      eligibleApprovers { username } users { nodes { username } } groups { nodes { fullPath } }
      approvedBy { nodes { username } } commentedBy { nodes { username } } scanResultPolicies { name }
      sourceRule { id } }
  } } } }";

const BRANCH_QUERY: &str = "query($path: ID!) { project(fullPath: $path) { branchRules { nodes { name
  branchProtection { codeOwnerApprovalRequired } } } } }";

fn owners(alice: &str, bob: &str, team: &str, star: &str) -> Value {
    json!([
        {"name": null, "entries": [
            {"pattern": "*", "owners": [format!("@{star}")]},
            {"pattern": "/docs/", "owners": [format!("@{bob}")]},
        ]},
        {"name": "Backend", "approvals": 2, "entries": [
            {"pattern": "/app/", "owners": [format!("@{alice}"), format!("@{bob}"), format!("@{team}")]},
        ]},
        {"name": "Optional", "optional": true, "entries": [
            {"pattern": "/opt/", "owners": [format!("@{bob}")]},
        ]},
    ])
}

#[tokio::test]
async fn code_owners() {
    let mut s = Scenario::new("code_owners").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let gpath = s.name("co");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let (author, author_t) = user_with_token(&mut s, "author").await;
    let (alice, alice_t) = user_with_token(&mut s, "alice").await;
    let (bob, bob_t) = user_with_token(&mut s, "bob").await;
    let (carol, carol_t) = user_with_token(&mut s, "carol").await;
    for (label, uid) in [("author member", author), ("alice member", alice), ("bob member", bob)] {
        s.step(label, POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": uid, "access_level": 30}), "user").await;
    }
    // Carol owns code through the project's group.
    let tpath = gpath.clone();
    s.step("carol in group", POST, &format!("/groups/{}/members", g.id()), Some(&admin), json!({"user_id": carol, "access_level": 30}), "user").await;
    s.settle().await;
    let hook = s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": rx.url, "merge_requests_events": true, "push_events": false}),
        "hook",
    )
    .await;
    let (alice_n, bob_n) = (s.name("alice"), s.name("bob"));
    s.code_owners("codeowners", pid, &admin, "main", owners(&alice_n, &bob_n, &tpath, &alice_n), true).await;
    s.settle().await;

    // The author's MR touches every section.
    let commits = format!("/projects/{pid}/repository/commits");
    s.step(
        "commit",
        POST,
        &commits,
        Some(&author_t),
        json!({"branch": "feature", "start_branch": "main", "commit_message": "Add files", "actions": [
            {"action": "create", "file_path": "docs/guide.md", "content": "guide\n"},
            {"action": "create", "file_path": "app/main.rb", "content": "main\n"},
            {"action": "create", "file_path": "app/util.rb", "content": "util\n"},
            {"action": "create", "file_path": "opt/extra.txt", "content": "extra\n"},
            {"action": "create", "file_path": "notes.txt", "content": "notes\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&author_t), json!({"source_branch": "feature", "target_branch": "main", "title": "Add files"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.deliveries("open hook", &mut rx, 1).await;
    let mr = format!("{mrs}/1");
    let vars = json!({"path": full, "iid": "1"});

    // Without "code owner approval required", the rules don't count.
    s.step("mr show", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("approvals", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("approval state", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("approval settings", GET, &format!("{mr}/approval_settings"), Some(&admin), json!(null), "approval").await;
    s.step("mr rules", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.graphql("graphql not required", Some(&admin), MR_QUERY, vars.clone()).await;

    // With it, they do.
    s.step("require code owners", PATCH, &format!("/projects/{pid}/protected_branches/main"), Some(&admin), json!({"code_owner_approval_required": true}), "protected_branch").await;
    s.step("protected branch", GET, &format!("/projects/{pid}/protected_branches/main"), Some(&admin), json!(null), "protected_branch").await;
    s.graphql("branch rules", Some(&admin), BRANCH_QUERY, json!({"path": full})).await;
    s.settle().await;
    s.step("mr required", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("approvals required", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("approvals as alice", GET, &format!("{mr}/approvals"), Some(&alice_t), json!(null), "approval").await;
    s.step("approval state required", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("approval settings required", GET, &format!("{mr}/approval_settings"), Some(&admin), json!(null), "approval").await;
    s.step("mr rules required", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.graphql("graphql required", Some(&admin), MR_QUERY, vars.clone()).await;
    s.step("merge unapproved", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;

    // Code owner rules can't be changed through the API.
    let rules = s.step("mr rules again", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;
    let first = rules.body[0]["id"].as_i64().unwrap();
    s.step("update code owner rule", PUT, &format!("{mr}/approval_rules/{first}"), Some(&admin), json!({"approvals_required": 3}), "rule").await;
    s.step("delete code owner rule", DELETE, &format!("{mr}/approval_rules/{first}"), Some(&admin), json!(null), "rule").await;
    s.step("show code owner rule", GET, &format!("{mr}/approval_rules/{first}"), Some(&admin), json!(null), "rule").await;
    // An MR rule of its own doesn't replace them.
    s.step("mr rule", POST, &format!("{mr}/approval_rules"), Some(&admin), json!({"name": "QA", "approvals_required": 1, "user_ids": [carol]}), "rule").await;
    s.step("approval state with mr rule", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;

    // Approvals, one owner at a time.
    s.step("alice approves", POST, &format!("{mr}/approve"), Some(&alice_t), json!(null), "approval").await;
    s.settle().await;
    s.step("state after alice", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("carol approves", POST, &format!("{mr}/approve"), Some(&carol_t), json!(null), "approval").await;
    s.settle().await;
    s.step("approvals after carol", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("bob approves", POST, &format!("{mr}/approve"), Some(&bob_t), json!(null), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.deliveries("approval hooks", &mut rx, 4).await;
    s.step("approvals approved", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("mr approved", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.graphql("graphql approved", Some(&admin), MR_QUERY, vars.clone()).await;

    // Selective removal: a push takes back only the approvals of the changed files' owners.
    s.step("selective removal", POST, &format!("/projects/{pid}/approvals"), Some(&admin), json!({"reset_approvals_on_push": false, "selective_code_owner_removals": true}), "approval").await;
    s.step(
        "push docs",
        POST,
        &commits,
        Some(&author_t),
        json!({"branch": "feature", "commit_message": "Edit guide", "actions": [{"action": "update", "file_path": "docs/guide.md", "content": "guide 2\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("state after docs push", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("approvals after docs push", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.deliveries("docs push hooks", &mut rx, 2).await;
    // A backend file changes: its owners' approvals go, group members' included.
    s.step(
        "push app",
        POST,
        &commits,
        Some(&author_t),
        json!({"branch": "feature", "commit_message": "Edit main", "actions": [{"action": "update", "file_path": "app/main.rb", "content": "main 2\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("state after app push", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.deliveries("app push hooks", &mut rx, 2).await;
    // A push removing the last backend file drops the Backend rule. (Without the hook: whether
    // the push's update hook lists the rules before or after that differs between runs on EE.)
    s.step("delete hook", DELETE, &format!("/projects/{pid}/hooks/{}", hook.id()), Some(&admin), json!(null), "hook").await;
    s.step(
        "push app removal",
        POST,
        &commits,
        Some(&author_t),
        json!({"branch": "feature", "commit_message": "Drop app", "actions": [
            {"action": "delete", "file_path": "app/main.rb"}, {"action": "delete", "file_path": "app/util.rb"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("rules after app removal", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.step("state after app removal", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;

    // Changing CODEOWNERS on the target branch updates the open MR's rules.
    s.code_owners("codeowners bob", pid, &admin, "main", owners(&alice_n, &bob_n, &tpath, &bob_n), false).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("rules after codeowners change", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.step("state after codeowners change", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.graphql("graphql after codeowners change", Some(&admin), MR_QUERY, vars.clone()).await;

    // A second MR only touching a file owned by the catch-all rule.
    s.step(
        "commit 2",
        POST,
        &commits,
        Some(&author_t),
        json!({"branch": "small", "start_branch": "main", "commit_message": "Readme", "actions": [{"action": "update", "file_path": "README.md", "content": "readme\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.step("mr 2", POST, &mrs, Some(&author_t), json!({"source_branch": "small", "target_branch": "main", "title": "Readme"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("mr 2 rules", GET, &format!("{mrs}/2/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.step("mr 2 state", GET, &format!("{mrs}/2/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("bob approves 2", POST, &format!("{mrs}/2/approve"), Some(&bob_t), json!(null), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("merge 2", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.finish();
}
