//! Reviewer states when a reviewer who requested changes, or approved, is removed and added
//! again.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn requested_changes_rows() {
    let mut s = Scenario::new("requested_changes_rows").await;
    let admin = s.admin();
    let gpath = s.name("rc");
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
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mr = format!("/projects/{pid}/merge_requests/1");
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;
    let reviewers = format!("{mr}/reviewers");
    s.step("bob requests changes", POST, &format!("{mr}/notes"), Some(&bob_t), json!({"body": "/request_changes"}), "note").await;
    s.step("reviewers 1", GET, &reviewers, Some(&admin), json!(null), "reviewer").await;
    s.step("remove reviewer", PUT, &mr, Some(&admin), json!({"reviewer_ids": []}), "mr").await;
    s.step("re-add reviewer", PUT, &mr, Some(&admin), json!({"reviewer_ids": [bob]}), "mr").await;
    s.step("reviewers 2", GET, &reviewers, Some(&admin), json!(null), "reviewer").await;
    s.step("bob approves", POST, &format!("{mr}/approve"), Some(&bob_t), json!(null), "approval").await;
    s.settle().await;
    s.step("remove reviewer again", PUT, &mr, Some(&admin), json!({"reviewer_ids": []}), "mr").await;
    s.step("re-add reviewer again", PUT, &mr, Some(&admin), json!({"reviewer_ids": [bob]}), "mr").await;
    s.step("reviewers 3", GET, &reviewers, Some(&admin), json!(null), "reviewer").await;
    s.settle().await;
    s.step("notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}
