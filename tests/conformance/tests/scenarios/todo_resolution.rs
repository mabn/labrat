//! To-dos resolved by actions other than commenting: updating labels and resolving threads.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn todo_resolution() {
    let mut s = Scenario::new("todo_resolution").await;
    let admin = s.admin();
    let gpath = s.name("tr");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000"}), "label").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, b) in ["b1", "b2"].iter().enumerate() {
        s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}"), "reviewer_ids": [bob]}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    // MR 1: bob changes the labels; his review request should be resolved.
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.step("bob adds a label", PUT, &format!("{mrs}/1"), Some(&bob_t), json!({"add_labels": "bug"}), "mr").await;
    s.settle().await;
    s.step("bob todos after label", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    // MR 2: a thread mentioning bob; bob resolves it.
    let d = s.step("thread", POST, &format!("{mrs}/2/discussions"), Some(&admin), json!({"body": format!("@{username} please check")}), "discussion").await;
    let did = d.get("id").as_str().unwrap_or_default().to_string();
    s.settle().await;
    s.step("bob todos with mention", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.step("bob resolves", PUT, &format!("{mrs}/2/discussions/{did}"), Some(&bob_t), json!({"resolved": true}), "discussion").await;
    s.settle().await;
    s.step("bob todos after resolve", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.finish();
}
