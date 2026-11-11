//! Archived projects are read-only: MR updates, comments, approvals and merges are refused.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn archived_readonly() {
    let mut s = Scenario::new("archived_readonly").await;
    let admin = s.admin();
    let gpath = s.name("ro");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.settle().await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("archive", POST, &format!("/projects/{pid}/archive"), Some(&admin), json!(null), "project").await;
    s.settle().await;
    for (label, tok) in [("bob", bob_t.clone()), ("admin", admin.clone())] {
        s.step(&format!("{label} update"), PUT, &format!("{mrs}/1"), Some(&tok), json!({"title": "Renamed"}), "mr").await;
        s.step(&format!("{label} comment"), POST, &format!("{mrs}/1/notes"), Some(&tok), json!({"body": "Hi"}), "note").await;
        s.step(&format!("{label} approve"), POST, &format!("{mrs}/1/approve"), Some(&tok), json!(null), "approval").await;
        s.step(&format!("{label} award"), POST, &format!("{mrs}/1/award_emoji"), Some(&tok), json!({"name": "tada"}), "award").await;
        s.graphql(
            &format!("{label} permissions"),
            Some(&tok),
            "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { userPermissions { readMergeRequest updateMergeRequest adminMergeRequest createNote canMerge canApprove pushToSourceBranch } } } }",
            json!({"path": full}),
        )
        .await;
    }
    s.step("admin merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.step("new mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Another"}), "mr").await;
    s.step("read still works", GET, &format!("{mrs}/1"), Some(&bob_t), json!(null), "mr").await;
    s.finish();
}
