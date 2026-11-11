//! Which operations make GitLab drop an existing approval (found by the random walk).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn approval_persistence() {
    let mut s = Scenario::new("approval_persistence").await;
    let admin = s.admin();
    let gpath = s.name("ap");
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
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mr = format!("/projects/{pid}/merge_requests/1");
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("bob approves", POST, &format!("{mr}/approve"), Some(&bob_t), json!(null), "approval").await;
    s.step("admin approves", POST, &format!("{mr}/approve"), Some(&admin), json!(null), "approval").await;
    let ops: Vec<(&str, Method, String, &String, serde_json::Value)> = vec![
        ("bob /ready", POST, format!("{mr}/notes"), &bob_t, json!({"body": "/ready"})),
        ("bob comment", POST, format!("{mr}/notes"), &bob_t, json!({"body": "hello"})),
        ("bob thread", POST, format!("{mr}/discussions"), &bob_t, json!({"body": "thread"})),
        ("bob self-assign", PUT, mr.clone(), &bob_t, json!({"assignee_ids": [bob]})),
        ("bob labels", PUT, mr.clone(), &bob_t, json!({"add_labels": "x"})),
        ("bob reopen", PUT, mr.clone(), &bob_t, json!({"state_event": "reopen"})),
        ("admin push", POST, commits.clone(), &admin, json!({"branch": "feature", "commit_message": "More", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]})),
    ];
    for (label, m, path, tok, body) in ops {
        s.step(label, m, &path, Some(tok), body, "x").await;
        s.settle().await;
        s.poll_mr(pid, 1).await;
        s.step(&format!("after {label}"), GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    }
    // Background work after the push.
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    s.step("after wait", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.finish();
}
