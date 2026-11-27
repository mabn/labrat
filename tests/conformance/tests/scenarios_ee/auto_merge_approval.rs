//! EE: an auto-merge armed while approvals are missing completes when the last approval
//! arrives, and one armed while a blocking MR is open completes when the blocker merges.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn auto_merge_approval() {
    let mut s = Scenario::new("auto_merge_approval").await;
    let admin = s.admin();
    let gpath = s.name("ama");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let (approver, approver_t) = user_with_token(&mut s, "approver").await;
    s.step("approver member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": approver, "access_level": 30}), "user").await;
    s.step("rule", POST, &format!("/projects/{pid}/approval_rules"), Some(&admin), json!({"name": "Review", "approvals_required": 1, "user_ids": [approver]}), "rule").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, name) in [(1, "one"), (2, "two"), (3, "three")] {
        s.step(
            &format!("commit {name}"),
            POST,
            &format!("/projects/{pid}/repository/commits"),
            Some(&admin),
            json!({"branch": name, "start_branch": "main", "commit_message": format!("Add {name}"), "actions": [{"action": "create", "file_path": format!("{name}.txt"), "content": format!("{name}\n")}]}),
            "commit",
        )
        .await;
        s.settle().await;
        s.step(&format!("mr {name}"), POST, &mrs, Some(&admin), json!({"source_branch": name, "target_branch": "main", "title": format!("Add {name}")}), "mr").await;
        s.poll_mr(pid, i).await;
    }

    // Armed without the approval; the approval merges it.
    s.step("mr 1 unapproved", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("arm 1", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.step("approve 1", POST, &format!("{mrs}/1/approve"), Some(&approver_t), json!(null), "approval").await;
    s.settle().await;
    s.poll(&format!("{mrs}/1"), |b| b["state"] == "merged").await;
    s.step("mr 1 after approval", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;

    // Approved but blocked by MR 3: armed, then the blocker merges.
    s.step("approve 2", POST, &format!("{mrs}/2/approve"), Some(&approver_t), json!(null), "approval").await;
    s.step("approve 3", POST, &format!("{mrs}/3/approve"), Some(&approver_t), json!(null), "approval").await;
    s.step("block 2 by 3", POST, &format!("{mrs}/2/blocks"), Some(&admin), json!({"blocking_merge_request_iid": 3}), "block").await;
    s.settle().await;
    s.step("arm 2", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.step("merge 3", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.poll(&format!("{mrs}/2"), |b| b["state"] == "merged").await;
    s.step("mr 2 after blocker", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
