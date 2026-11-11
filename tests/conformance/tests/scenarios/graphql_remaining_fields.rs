//! GraphQL fields labrat didn't resolve yet: commit diffs, user fields seen through reviewers
//! (as the user and as someone else), and pipeline job/stage fields of a commit-status pipeline.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_remaining_fields() {
    let mut s = Scenario::new("graphql_remaining_fields").await;
    let admin = s.admin();
    let gpath = s.name("gr");
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
    let c = s
        .step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Change", "actions": [
            {"action": "create", "file_path": "new.txt", "content": "n\n"},
            {"action": "update", "file_path": "README.md", "content": "changed\n"},
            {"action": "create", "file_path": "big.txt", "content": "x\n".repeat(20000)}
        ]}), "commit")
        .await;
    let sha = c.body["id"].as_str().unwrap_or_default().to_string();
    s.settle().await;
    s.step("move", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "commit_message": "Move and delete", "actions": [
        {"action": "move", "file_path": "renamed.txt", "previous_path": "new.txt"},
        {"action": "delete", "file_path": "big.txt"}
    ]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;
    let head = s.send(reqwest::Method::GET, &format!("/projects/{pid}/merge_requests/1"), Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
    s.step("status", POST, &format!("/projects/{pid}/statuses/{head}"), Some(&admin), json!({"state": "success", "name": "ci"}), "status").await;
    s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await;
    s.settle().await;
    let diffs = "query($p: ID!, $r: String!) { project(fullPath: $p) { repository { tree(ref: $r) { lastCommit { sha diffs { aMode bMode collapsed deletedFile diff newFile newPath oldPath renamedFile tooLarge } } } } } }";
    s.graphql("commit diffs head", Some(&admin), diffs, json!({"p": full, "r": "f"})).await;
    s.graphql("commit diffs first", Some(&admin), diffs, json!({"p": full, "r": sha})).await;
    let user_fields = "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { reviewers { nodes { username
        customAttributes { key value } ide { codeSuggestionsEnabled }
        userPreferences { issuesSort mergeRequestDashboardListType mergeRequestDashboardShowDrafts projectsSort timezone useWorkItemsView visibilityPipelineIdType }
        callouts { nodes { featureName } } groupCallouts { nodes { featureName } } namespaceCommitEmails { nodes { id } }
        savedReplies { nodes { id } } savedReply(id: \"gid://gitlab/Users::SavedReply/1\") { id }
        starredProjects { nodes { fullPath } } contributedProjects { nodes { fullPath } } } } } } }";
    s.step("bob comments", POST, &format!("/projects/{pid}/merge_requests/1/notes"), Some(&bob_t), json!({"body": "Looks fine"}), "note").await;
    s.settle().await;
    s.graphql("user fields as admin", Some(&admin), user_fields, json!({"p": full})).await;
    s.graphql("user fields as bob", Some(&bob_t), user_fields, json!({"p": full})).await;
    s.graphql(
        "pipeline fields",
        Some(&admin),
        "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { headPipeline { status
           jobs { nodes { name status } } stages { nodes { name status } } testReportSummary { total { count } }
           warningMessages { nodes { content } } job(name: \"ci\") { name status } manualVariables { nodes { key } }
           sourceJob { name } jobArtifacts { name } } } } }",
        json!({"p": full}),
    )
    .await;
    s.finish();
}
