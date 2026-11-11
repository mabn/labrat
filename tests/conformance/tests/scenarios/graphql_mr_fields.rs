//! `MergeRequest` GraphQL fields beyond the basics: approval state, participants by role,
//! to-dos and branch protection.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_mr_fields() {
    let mut s = Scenario::new("graphql_mr_fields").await;
    let admin = s.admin();
    let gpath = s.name("gmx");
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
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("admin commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.step("bob commit", POST, &commits, Some(&bob_t), json!({"branch": "feature", "commit_message": "Add b", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.step("other branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "old", "ref": "main"}), "branch").await;
    s.step("commit on main", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Main change", "actions": [{"action": "create", "file_path": "m.txt", "content": "m\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a and b", "assignee_ids": [bob], "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr from protected", POST, &mrs, Some(&admin), json!({"source_branch": "main", "target_branch": "old", "title": "Back-merge"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("bob comments", POST, &format!("{mrs}/1/notes"), Some(&bob_t), json!({"body": "A comment"}), "note").await;
    s.settle().await;
    s.graphql(
        "bob requests changes",
        Some(&bob_t),
        "mutation($path: ID!) { mergeRequestRequestChanges(input: {projectPath: $path, iid: \"1\"}) { errors } }",
        json!({"path": full}),
    )
    .await;
    s.settle().await;

    let mr = |iid: &str, sel: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequest(iid: \"{iid}\") {{ {sel} }} }} }}");
    let cases: Vec<(&str, &str, &str, bool)> = vec![
        ("commenters", "1", "commenters { count nodes { username } }", false),
        ("committers", "1", "committers { count nodes { username } }", false),
        ("branch fields", "1", "sourceBranchProtected targetBranchPath sourceBranchExists targetBranchExists", false),
        ("protected source", "2", "sourceBranchProtected targetBranchPath", false),
        ("flags", "1", "allowsMultipleAssignees allowsMultipleReviewers supportsLockOnMerge name", false),
        ("pipeline creation requests", "1", "pipelineCreationRequests { status error }", false),
        ("reference", "1", "short: reference full: reference(full: true)", false),
        ("notes only comments", "1", "notes(filter: ONLY_COMMENTS) { nodes { body system } }", false),
        ("notes only activity", "1", "notes(filter: ONLY_ACTIVITY) { nodes { body system } }", false),
        ("todos as admin", "1", "currentUserTodos { nodes { action state body } }", false),
        ("todos as bob", "1", "currentUserTodos { nodes { action state body author { username } } }", true),
        ("pending todos as bob", "1", "currentUserTodos(state: pending) { count nodes { action } }", true),
        ("done todos as bob", "1", "currentUserTodos(state: done) { count }", true),
    ];
    for (label, iid, sel, as_bob) in cases {
        let t = if as_bob { bob_t.clone() } else { admin.clone() };
        s.graphql(label, Some(&t), &mr(iid, sel), json!({"path": full})).await;
    }
    s.finish();
}
