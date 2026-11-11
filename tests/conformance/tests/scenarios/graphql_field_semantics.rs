//! GraphQL field semantics found by reading GitLab's types: permissions for anonymous users,
//! merge users, pipelines, participants, ordering, counts and namespaces.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn graphql_field_semantics() {
    let mut s = Scenario::new("graphql_field_semantics").await;
    let admin = s.admin();
    let gpath = s.name("gf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s
        .step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public", "squash_commit_template": "Squashed: %{title}"}), "project")
        .await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut users = vec![];
    for base in ["bob", "carol"] {
        let username = s.name(base);
        let id = s
            .step(base, POST, "/users", Some(&admin), json!({"username": username, "name": base, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let t = s.step("token", POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step("developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": 30}), "user").await;
        users.push((id, username, t));
    }
    s.settle().await;
    let (_, bob_name, bob_t) = users[0].clone();
    let (_, _carol_name, carol_t) = users[1].clone();
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let mut shas = vec![];
    for (i, b) in ["b1", "b2"].iter().enumerate() {
        let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        shas.push(c.get("id").as_str().unwrap().to_string());
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}"), "description": format!("cc @{bob_name}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    // MR 1: approvals by both developers, a comment and an internal note, then a direct merge.
    s.step("bob approves", POST, &format!("{mrs}/1/approve"), Some(&bob_t), json!(null), "approval").await;
    s.step("carol approves", POST, &format!("{mrs}/1/approve"), Some(&carol_t), json!(null), "approval").await;
    s.step("carol comments", POST, &format!("{mrs}/1/notes"), Some(&carol_t), json!({"body": "Nice"}), "note").await;
    s.step("bob internal note", POST, &format!("{mrs}/1/notes"), Some(&bob_t), json!({"body": "Internal", "internal": true}), "note").await;
    s.settle().await;
    s.step("merge 1", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    // MR 2: a pipeline on its first commit, then a newer commit; main moves on.
    s.step("status", POST, &format!("/projects/{pid}/statuses/{}", shas[1]), Some(&admin), json!({"state": "success", "name": "ci", "ref": "b2"}), "status").await;
    s.step("second commit", POST, &commits, Some(&admin), json!({"branch": "b2", "commit_message": "More", "actions": [{"action": "create", "file_path": "more.txt", "content": "m\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;

    let mrq = "iid mergeStatus mergeStatusEnum divergedFromTargetBranch hasCi defaultSquashCommitMessage mergeUser { username } approvedBy { nodes { username } } \
               participants { nodes { username } } commenters { nodes { username } } userPermissions { readMergeRequest removeSourceBranch canMerge updateMergeRequest createNote } \
               pipelines { nodes { type latest } }";
    let q = |iid: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequest(iid: \"{iid}\") {{ {mrq} }} }} }}");
    s.graphql("merged mr", Some(&admin), &q("1"), json!({"path": full})).await;
    s.graphql("open mr", Some(&admin), &q("2"), json!({"path": full})).await;
    s.graphql(
        "anonymous permissions",
        None,
        "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"2\") { userPermissions { readMergeRequest createNote canMerge } } } }",
        json!({"path": full}),
    )
    .await;
    s.graphql("count limit", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequests { count(limit: 1) } } }", json!({"path": full})).await;
    s.graphql("user namespace", Some(&admin), "query { currentUser { namespace { webUrl fullPath } } }", json!({})).await;
    s.graphql(
        "commit email of others",
        Some(&bob_t),
        "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { author { username commitEmail } } } }",
        json!({"path": full}),
    )
    .await;
    s.graphql("group as anonymous", None, "query($path: ID!) { group(fullPath: $path) { name } }", json!({"path": gpath})).await;
    s.graphql("group on user namespace", Some(&admin), "query { group(fullPath: \"claude\") { name } }", json!({})).await;
    s.finish();
}
