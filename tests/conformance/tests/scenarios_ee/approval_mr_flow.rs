//! EE approvals on a merge request: rules copied from the project, who may approve, the EE
//! approval bodies (`/approvals`, `/approval_state`), the merge status and merge refusal while
//! approvals are missing, webhook actions, resets on push, and MR-level rules.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn approval_mr_flow() {
    let mut s = Scenario::new("approval_mr_flow").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let gpath = s.name("amf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let (author, author_t) = user_with_token(&mut s, "author").await;
    let (a1, a1_t) = user_with_token(&mut s, "alice").await;
    let (a2, a2_t) = user_with_token(&mut s, "bob").await;
    let (rep, rep_t) = user_with_token(&mut s, "reporter").await;
    let (_out, out_t) = user_with_token(&mut s, "outsider").await;
    for (label, uid, level) in [("author member", author, 30), ("alice member", a1, 30), ("bob member", a2, 40), ("reporter member", rep, 20)] {
        s.step(label, POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": uid, "access_level": level}), "user").await;
    }
    s.settle().await;
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": rx.url, "merge_requests_events": true, "note_events": true, "push_events": false}),
        "hook",
    )
    .await;
    let rules = format!("/projects/{pid}/approval_rules");
    s.step("rule backend", POST, &rules, Some(&admin), json!({"name": "Backend", "approvals_required": 2, "user_ids": [a1, a2]}), "rule").await;
    s.step("rule any", POST, &rules, Some(&admin), json!({"name": "All members", "approvals_required": 1, "rule_type": "any_approver"}), "rule").await;

    // The author opens an MR.
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("commit", POST, &commits, Some(&author_t), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&author_t), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.deliveries("open hook", &mut rx, 1).await;
    let mr = format!("{mrs}/1");
    s.step("mr show", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("approvals admin", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("approvals author", GET, &format!("{mr}/approvals"), Some(&author_t), json!(null), "approval").await;
    s.step("approvals alice", GET, &format!("{mr}/approvals"), Some(&a1_t), json!(null), "approval").await;
    s.step("approvals reporter", GET, &format!("{mr}/approvals"), Some(&rep_t), json!(null), "approval").await;
    s.step("approvals outsider", GET, &format!("{mr}/approvals"), Some(&out_t), json!(null), "approval").await;
    s.step("approval state", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("approval settings alias", GET, &format!("{mr}/approval_settings"), Some(&a1_t), json!(null), "approval").await;
    s.step("mr rules", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;

    // Who may approve.
    s.step("author approves", POST, &format!("{mr}/approve"), Some(&author_t), json!(null), "approval").await;
    s.step("reporter approves", POST, &format!("{mr}/approve"), Some(&rep_t), json!(null), "approval").await;
    s.step("outsider approves", POST, &format!("{mr}/approve"), Some(&out_t), json!(null), "approval").await;
    s.step("merge unapproved", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
    s.step("alice approves wrong sha", POST, &format!("{mr}/approve"), Some(&a1_t), json!({"sha": "0000000000000000000000000000000000000000"}), "approval").await;
    s.step("alice approves", POST, &format!("{mr}/approve"), Some(&a1_t), json!(null), "approval").await;
    s.settle().await;
    s.deliveries("approval hook", &mut rx, 1).await;
    s.step("alice approves again", POST, &format!("{mr}/approve"), Some(&a1_t), json!(null), "approval").await;
    s.step("approvals after alice", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("state after alice", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.poll_mr(pid, 1).await;
    s.step("mr after alice", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("admin approves", POST, &format!("{mr}/approve"), Some(&admin), json!(null), "approval").await;
    s.settle().await;
    s.deliveries("admin approval hook", &mut rx, 1).await;
    s.step("bob approves", POST, &format!("{mr}/approve"), Some(&a2_t), json!(null), "approval").await;
    s.settle().await;
    s.deliveries("approved hook", &mut rx, 1).await;
    s.poll_mr(pid, 1).await;
    s.step("mr approved", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("state approved", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("bob unapproves", POST, &format!("{mr}/unapprove"), Some(&a2_t), json!(null), "approval").await;
    s.settle().await;
    s.deliveries("unapproved hook", &mut rx, 1).await;
    s.step("bob unapproves again", POST, &format!("{mr}/unapprove"), Some(&a2_t), json!(null), "approval").await;
    s.step("bob approves back", POST, &format!("{mr}/approve"), Some(&a2_t), json!(null), "approval").await;
    s.settle().await;
    s.deliveries("approved again hook", &mut rx, 1).await;

    // A push resets the approvals (`reset_approvals_on_push` is on by default).
    s.step("push", POST, &commits, Some(&author_t), json!({"branch": "feature", "commit_message": "More", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.deliveries("push hooks", &mut rx, 1).await;
    s.step("approvals after push", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("notes after push", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;

    // MR-level rules override the project's.
    let mr_rules = format!("{mr}/approval_rules");
    s.step("mr rules before override", GET, &mr_rules, Some(&admin), json!(null), "rule").await;
    s.step("mr rule as reporter", POST, &mr_rules, Some(&rep_t), json!({"name": "Mine", "approvals_required": 1}), "rule").await;
    s.step("mr rule missing params", POST, &mr_rules, Some(&admin), json!({}), "rule").await;
    let r = s.step("mr rule create", POST, &mr_rules, Some(&author_t), json!({"name": "Docs", "approvals_required": 1, "user_ids": [a1]}), "rule").await;
    let rid = r.body["id"].as_i64().unwrap_or(0);
    let project_rule = s.step("project rules now", GET, &rules, Some(&admin), json!(null), "rule").await;
    let backend_id = project_rule.body.as_array().and_then(|a| a.iter().find(|r| r["name"] == "Backend")).and_then(|r| r["id"].as_i64()).unwrap_or(0);
    s.step("mr rule from project rule", POST, &mr_rules, Some(&admin), json!({"name": "Copy", "approvals_required": 1, "approval_project_rule_id": backend_id}), "rule").await;
    s.step("mr rules overridden", GET, &mr_rules, Some(&admin), json!(null), "rule").await;
    s.step("mr rule show", GET, &format!("{mr_rules}/{rid}"), Some(&admin), json!(null), "rule").await;
    s.step("mr rule update", PUT, &format!("{mr_rules}/{rid}"), Some(&admin), json!({"approvals_required": 0, "user_ids": [a2]}), "rule").await;
    s.step("state overridden", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("approvals overridden", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("mr rule delete", DELETE, &format!("{mr_rules}/{rid}"), Some(&admin), json!(null), "rule").await;
    s.step("mr rule delete again", DELETE, &format!("{mr_rules}/{rid}"), Some(&admin), json!(null), "rule").await;
    s.step("change required count", POST, &format!("{mr}/approvals"), Some(&admin), json!({"approvals_required": 3}), "approval").await;
    s.step("change required count bad", POST, &format!("{mr}/approvals"), Some(&admin), json!({}), "approval").await;
    s.step("reset approvals as human", PUT, &format!("{mr}/reset_approvals"), Some(&admin), json!(null), "approval").await;

    // Overrides can be switched off for the project.
    s.step("prevent overrides", POST, &format!("/projects/{pid}/approvals"), Some(&admin), json!({"disable_overriding_approvers_per_merge_request": true}), "x").await;
    s.step("mr rule create prevented", POST, &mr_rules, Some(&admin), json!({"name": "Late", "approvals_required": 1}), "rule").await;
    s.step("mr rules prevented", GET, &mr_rules, Some(&admin), json!(null), "rule").await;
    s.step("state prevented", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;

    // A second MR, created with a required count, and the author allowed to approve.
    s.step("allow author", POST, &format!("/projects/{pid}/approvals"), Some(&admin), json!({"merge_requests_author_approval": true, "disable_overriding_approvers_per_merge_request": false}), "x").await;
    s.step("commit 2", POST, &commits, Some(&author_t), json!({"branch": "second", "start_branch": "main", "commit_message": "Add c", "actions": [{"action": "create", "file_path": "c.txt", "content": "c\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr 2", POST, &mrs, Some(&author_t), json!({"source_branch": "second", "target_branch": "main", "title": "Add c"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.deliveries("open hook 2", &mut rx, 1).await;
    s.step("mr 2 rules", GET, &format!("{mrs}/2/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.step("mr 2 approvals", GET, &format!("{mrs}/2/approvals"), Some(&author_t), json!(null), "approval").await;
    s.step("author approves 2", POST, &format!("{mrs}/2/approve"), Some(&author_t), json!(null), "approval").await;
    s.settle().await;
    s.deliveries("author approval hook", &mut rx, 1).await;
    s.step("update mr 2 count", PUT, &format!("{mrs}/2"), Some(&author_t), json!({"approvals_before_merge": 1}), "mr").await;
    s.settle().await;
    s.deliveries("update hook 2", &mut rx, 1).await;
    s.step("mr 2 list", GET, &mrs, Some(&admin), json!({"state": "opened"}), "mr").await;
    // `approvals_before_merge` on create: with the project's `any_approver` rule already copied,
    // EE creates the MR and then fails.
    s.step("commit 3", POST, &commits, Some(&author_t), json!({"branch": "third", "start_branch": "main", "commit_message": "Add d", "actions": [{"action": "create", "file_path": "d.txt", "content": "d\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr 3 with count", POST, &mrs, Some(&author_t), json!({"source_branch": "third", "target_branch": "main", "title": "Add d", "approvals_before_merge": 2}), "mr").await;
    s.poll_mr(pid, 3).await;
    s.deliveries("open hook 3", &mut rx, 1).await;
    s.step("mr 3", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.step("mr 3 rules", GET, &format!("{mrs}/3/approval_rules"), Some(&admin), json!(null), "rule").await;

    // Merge once approved.
    s.step("alice approves 1", POST, &format!("{mr}/approve"), Some(&a1_t), json!(null), "approval").await;
    s.step("bob approves 1", POST, &format!("{mr}/approve"), Some(&a2_t), json!(null), "approval").await;
    s.step("admin approves 1", POST, &format!("{mr}/approve"), Some(&admin), json!(null), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.deliveries("approval hooks", &mut rx, 3).await;
    s.step("merge", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.deliveries("merge hook", &mut rx, 1).await;
    s.step("approvals merged", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("state merged", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}

