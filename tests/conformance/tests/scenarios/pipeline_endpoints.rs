//! The rest of the pipeline API on commit-status (external) pipelines.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn pipeline_endpoints() {
    let mut s = Scenario::new("pipeline_endpoints").await;
    let admin = s.admin();
    let gpath = s.name("pe");
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
    let c = s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    let sha = c.get("id").as_str().unwrap().to_string();
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let st = s.step("running status", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "running", "name": "ci", "ref": "feature"}), "status").await;
    let plid = st.body["pipeline_id"].as_i64().unwrap();
    s.step("second status", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "pending", "name": "lint", "ref": "feature"}), "status").await;
    s.settle().await;
    let pl = format!("/projects/{pid}/pipelines/{plid}");
    s.step("jobs", GET, &format!("{pl}/jobs"), Some(&admin), json!(null), "job").await;
    s.step("jobs scope", GET, &format!("{pl}/jobs"), Some(&admin), json!({"scope[]": "running"}), "job").await;
    s.step("bridges", GET, &format!("{pl}/bridges"), Some(&admin), json!(null), "job").await;
    s.step("variables", GET, &format!("{pl}/variables"), Some(&admin), json!(null), "variable").await;
    s.step("test report", GET, &format!("{pl}/test_report"), Some(&admin), json!(null), "report").await;
    s.step("test report summary", GET, &format!("{pl}/test_report_summary"), Some(&admin), json!(null), "report").await;
    s.step("latest", GET, &format!("/projects/{pid}/pipelines/latest"), Some(&admin), json!({"ref": "feature"}), "pipeline").await;
    s.step("latest default", GET, &format!("/projects/{pid}/pipelines/latest"), Some(&admin), json!(null), "pipeline").await;
    s.step("mr pipelines", GET, &format!("/projects/{pid}/merge_requests/1/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("metadata", PUT, &format!("{pl}/metadata"), Some(&admin), json!({"name": "Named"}), "pipeline").await;
    s.step("show named", GET, &pl, Some(&admin), json!(null), "pipeline").await;
    s.step("cancel by bob", POST, &format!("{pl}/cancel"), Some(&bob_t), json!(null), "pipeline").await;
    s.settle().await;
    s.step("statuses after cancel", GET, &format!("/projects/{pid}/repository/commits/{sha}/statuses"), Some(&admin), json!(null), "status").await;
    s.step("retry", POST, &format!("{pl}/retry"), Some(&admin), json!(null), "pipeline").await;
    s.settle().await;
    s.step("delete by bob", DELETE, &pl, Some(&bob_t), json!(null), "pipeline").await;
    s.step("delete", DELETE, &pl, Some(&admin), json!(null), "pipeline").await;
    s.settle().await;
    s.step("show deleted", GET, &pl, Some(&admin), json!(null), "pipeline").await;
    s.step("mr after delete", GET, &format!("/projects/{pid}/merge_requests/1"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
