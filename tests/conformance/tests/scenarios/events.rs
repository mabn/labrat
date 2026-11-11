use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn contribution_events() {
    let mut s = Scenario::new("contribution_events").await;
    let admin = s.admin();
    let gpath = s.name("ev");
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
    // Project authorizations are refreshed asynchronously.
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("bob new branch", POST, &commits, Some(&bob_t), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a feature with a rather long commit title that goes past seventy characters", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.step("bob second commit", POST, &commits, Some(&bob_t), json!({"branch": "feature", "commit_message": "Second", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("bob mr", POST, &mrs, Some(&bob_t), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("admin comment", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "Looks good"}), "note").await;
    s.settle().await;
    s.step("bob thread", POST, &format!("{mrs}/1/discussions"), Some(&bob_t), json!({"body": "Thread"}), "discussion").await;
    s.settle().await;
    s.step("admin approves", POST, &format!("{mrs}/1/approve"), Some(&admin), json!(null), "approval").await;
    s.settle().await;
    s.step("bob closes", PUT, &format!("{mrs}/1"), Some(&bob_t), json!({"state_event": "close"}), "mr").await;
    s.step("bob reopens", PUT, &format!("{mrs}/1"), Some(&bob_t), json!({"state_event": "reopen"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("admin merges", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.step("project events", GET, &format!("/projects/{pid}/events"), Some(&admin), json!(null), "event").await;
    s.step("project events asc", GET, &format!("/projects/{pid}/events"), Some(&admin), json!({"sort": "asc", "per_page": 3}), "event").await;
    s.step("project pushes", GET, &format!("/projects/{pid}/events"), Some(&admin), json!({"action": "pushed"}), "event").await;
    s.step("project mr events", GET, &format!("/projects/{pid}/events"), Some(&admin), json!({"target_type": "merge_request"}), "event").await;
    s.step("project note events", GET, &format!("/projects/{pid}/events"), Some(&admin), json!({"target_type": "note"}), "event").await;
    s.step("bad action", GET, &format!("/projects/{pid}/events"), Some(&admin), json!({"action": "danced"}), "event").await;
    s.step("bob events", GET, &format!("/users/{bob}/events"), Some(&admin), json!(null), "event").await;
    s.step("bob events by username", GET, &format!("/users/{username}/events"), Some(&admin), json!({"action": "commented"}), "event").await;
    s.step("my events as bob", GET, "/events", Some(&bob_t), json!(null), "event").await;
    s.step("events unauthenticated", GET, "/events", None, json!(null), "event").await;
    s.finish();
}
