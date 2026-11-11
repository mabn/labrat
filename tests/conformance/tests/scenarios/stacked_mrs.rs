//! Stacked merge requests: the child targets the parent's branch and has auto-merge enabled
//! while a pipeline runs; merging the parent (removing its branch) retargets the child.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn stacked_mrs() {
    let mut s = Scenario::new("stacked_mrs").await;
    let admin = s.admin();
    let gpath = s.name("sk");
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
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("parent commit", POST, &commits, Some(&admin), json!({"branch": "parent", "start_branch": "main", "commit_message": "Parent", "actions": [{"action": "create", "file_path": "p.txt", "content": "p\n"}]}), "commit").await;
    s.step("child commit", POST, &commits, Some(&bob_t), json!({"branch": "child", "start_branch": "parent", "commit_message": "Child", "actions": [{"action": "create", "file_path": "c.txt", "content": "c\n"}]}), "commit").await;
    s.settle().await;
    s.step("parent mr", POST, &mrs, Some(&admin), json!({"source_branch": "parent", "target_branch": "main", "title": "Parent", "remove_source_branch": true}), "mr").await;
    s.poll_mr(pid, 1).await;
    let child = s.step("child mr", POST, &mrs, Some(&bob_t), json!({"source_branch": "child", "target_branch": "parent", "title": "Child"}), "mr").await;
    s.poll_mr(pid, 2).await;
    let sha = child.body["sha"].as_str().unwrap_or_default().to_string();
    s.step("child running", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "running", "name": "ci"}), "status").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("child auto-merge", PUT, &format!("{mrs}/2/merge"), Some(&bob_t), json!({"merge_when_pipeline_succeeds": true}), "mr").await;
    s.settle().await;
    s.step("merge parent", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"should_remove_source_branch": true}), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("child after", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("child notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.graphql(
        "child graphql",
        Some(&admin),
        "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"2\") { targetBranch autoMergeEnabled retargeted detailedMergeStatus mergeUser { username } } } }",
        json!({"p": full}),
    )
    .await;
    s.finish();
}
