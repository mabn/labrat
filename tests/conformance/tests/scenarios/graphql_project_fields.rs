//! `Project` GraphQL fields that MR tools read: settings, templates, URLs, counts, permissions.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_project_fields() {
    let mut s = Scenario::new("graphql_project_fields").await;
    let admin = s.admin();
    let gpath = s.name("pf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s
        .step(
            "project",
            POST,
            "/projects",
            Some(&admin),
            json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public", "description": "An **app**",
                   "merge_commit_template": "Merge %{source_branch}", "squash_commit_template": "%{title}", "suggestion_commit_message": "Apply %{suggestions_count}",
                   "allow_merge_on_skipped_pipeline": true, "squash_option": "always", "topics": ["rust", "tools"]}),
            "project",
        )
        .await;
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
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let q = "query($path: ID!) { project(fullPath: $path) { \
             allowMergeOnSkippedPipeline allowsMultipleMergeRequestAssignees allowsMultipleMergeRequestReviewers autocloseReferencedIssues avatarUrl \
             ciConfigPathOrDefault editPath forksCount issuesEnabled jobsEnabled lfsEnabled mergeCommitTemplate mergeRequestsAccessLevel { stringValue integerValue } \
             openIssuesCount openMergeRequestsCount printingMergeRequestLinkEnabled \
             requestAccessEnabled squashCommitTemplate squashReadOnly sshUrlToRepo starCount suggestionCommitMessage topics webPath wikiEnabled snippetsEnabled \
             serviceDeskEnabled sharedRunnersEnabled containerRegistryEnabled maxAccessLevel { integerValue stringValue } \
             userPermissions { pushCode createMergeRequestIn adminProject readProject removeProject createIssue downloadCode forkProject } } }";
    s.graphql("as admin", Some(&admin), q, json!({"path": full})).await;
    s.graphql("as developer", Some(&bob_t), q, json!({"path": full})).await;
    s.graphql("anonymous", None, q, json!({"path": full})).await;
    s.finish();
}
