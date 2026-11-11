//! `MergeRequest.availableAutoMergeStrategies` in various states, and `retargeted` after a
//! dependent MR's target branch is merged and deleted.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn graphql_auto_merge_fields() {
    let mut s = Scenario::new("graphql_auto_merge_fields").await;
    let admin = s.admin();
    let gpath = s.name("am");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("carol");
    let carol = s
        .step("carol", POST, "/users", Some(&admin), json!({"username": username, "name": "Carol", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let carol_t = s.step("carol token", POST, &format!("/users/{carol}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("carol reporter", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": carol, "access_level": 20}), "user").await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    // f1 off main; f2 off f1 (stacked).
    let c1 = s.step("commit f1", POST, &commits, Some(&admin), json!({"branch": "f1", "start_branch": "main", "commit_message": "F1", "actions": [{"action": "create", "file_path": "f1.txt", "content": "1\n"}]}), "commit").await;
    let sha1 = c1.get("id").as_str().unwrap().to_string();
    s.step("commit f2", POST, &commits, Some(&admin), json!({"branch": "f2", "start_branch": "f1", "commit_message": "F2", "actions": [{"action": "create", "file_path": "f2.txt", "content": "2\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr1", POST, &mrs, Some(&admin), json!({"source_branch": "f1", "target_branch": "main", "title": "F1", "remove_source_branch": true}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr2", POST, &mrs, Some(&admin), json!({"source_branch": "f2", "target_branch": "f1", "title": "F2"}), "mr").await;
    s.poll_mr(pid, 2).await;
    let q = "query($p: ID!, $iid: String!) { project(fullPath: $p) { mergeRequest(iid: $iid) { availableAutoMergeStrategies retargeted targetBranch mergeable } } }";
    let ask = |iid: &str| json!({"p": full, "iid": iid});
    s.graphql("mergeable", Some(&admin), q, ask("1")).await;
    s.graphql("mergeable as reporter", Some(&carol_t), q, ask("1")).await;
    s.step("running", POST, &format!("/projects/{pid}/statuses/{sha1}"), Some(&admin), json!({"state": "running", "name": "ci", "ref": "f1"}), "status").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.graphql("pipeline running", Some(&admin), q, ask("1")).await;
    s.step("draft", PUT, &format!("{mrs}/1"), Some(&admin), json!({"title": "Draft: F1"}), "mr").await;
    s.settle().await;
    s.graphql("draft with running pipeline", Some(&admin), q, ask("1")).await;
    s.step("success", POST, &format!("/projects/{pid}/statuses/{sha1}"), Some(&admin), json!({"state": "success", "name": "ci", "ref": "f1"}), "status").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.graphql("draft with green pipeline", Some(&admin), q, ask("1")).await;
    s.step("ready", PUT, &format!("{mrs}/1"), Some(&admin), json!({"title": "F1"}), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"should_remove_source_branch": true}), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.graphql("merged", Some(&admin), q, ask("1")).await;
    s.graphql("retargeted", Some(&admin), q, ask("2")).await;
    s.finish();
}
