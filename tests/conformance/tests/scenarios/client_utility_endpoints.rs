//! Endpoints MR clients call around the merge request API: markdown previews, the project's
//! users (reviewer/assignee pickers), user counts and status, namespaces, `-/search`, and
//! token rotation.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn client_utility_endpoints() {
    let mut s = Scenario::new("client_utility_endpoints").await;
    let admin = s.admin();
    let gpath = s.name("cu");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob Builder", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await;
    let bob_token = bob_t.get("token").as_str().unwrap().to_string();
    let bob_tid = bob_t.id();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;

    s.step("markdown plain", POST, "/markdown", Some(&admin), json!({"text": "Hello **world**"}), "markdown").await;
    s.step("markdown gfm", POST, "/markdown", Some(&admin), json!({"text": format!("See !1 and @{username}"), "gfm": true, "project": full}), "markdown").await;
    s.step("markdown missing", POST, "/markdown", Some(&admin), json!({}), "markdown").await;
    s.step("project users", GET, &format!("/projects/{pid}/users"), Some(&admin), json!(null), "user").await;
    s.step("project users search", GET, &format!("/projects/{pid}/users"), Some(&admin), json!({"search": "build"}), "user").await;
    s.step("user counts bob", GET, "/user_counts", Some(&bob_token), json!(null), "counts").await;
    s.step("set status", PUT, "/user/status", Some(&bob_token), json!({"emoji": "coffee", "message": "Reviewing"}), "status").await;
    s.step("own status", GET, "/user/status", Some(&bob_token), json!(null), "status").await;
    s.step("bob status", GET, &format!("/users/{bob}/status"), Some(&admin), json!(null), "status").await;
    s.step("namespace", GET, &format!("/namespaces/{gid}"), Some(&admin), json!(null), "namespace").await;
    s.step("namespaces search", GET, "/namespaces", Some(&admin), json!({"search": gpath}), "namespace").await;
    s.step("namespace exists", GET, &format!("/namespaces/{gpath}/exists"), Some(&admin), json!(null), "namespace").await;
    s.step("project dash search", GET, &format!("/projects/{pid}/-/search"), Some(&admin), json!({"scope": "merge_requests", "search": "Add"}), "mr").await;
    s.step("group dash search", GET, &format!("/groups/{gid}/-/search"), Some(&admin), json!({"scope": "merge_requests", "search": "Add"}), "mr").await;
    let r = s.step("rotate pat", POST, &format!("/personal_access_tokens/{bob_tid}/rotate"), Some(&admin), json!(null), "token").await;
    let new_token = r.body["token"].as_str().unwrap_or_default().to_string();
    s.step("old pat", GET, "/user", Some(&bob_token), json!(null), "user").await;
    let r2 = s.step("self rotate", POST, "/personal_access_tokens/self/rotate", Some(&new_token), json!(null), "token").await;
    let _ = r2;
    s.finish();
}
