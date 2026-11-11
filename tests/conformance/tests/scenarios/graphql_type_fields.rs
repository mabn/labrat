//! More GraphQL fields on users, commits, pipelines, milestones and groups.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_type_fields() {
    let mut s = Scenario::new("graphql_type_fields").await;
    let admin = s.admin();
    let gpath = s.name("tf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    s.step("subgroup", POST, "/groups", Some(&admin), json!({"name": "sub", "path": "sub", "parent_id": gid}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
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
    let mrs = format!("/projects/{pid}/merge_requests");
    let mut shas = vec![];
    for (i, (b, extra)) in [("b1", json!({"assignee_ids": [bob]})), ("b2", json!({"reviewer_ids": [bob]})), ("b3", json!({}))].iter().enumerate() {
        let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        shas.push(c.get("id").as_str().unwrap().to_string());
        s.settle().await;
        let mut body = json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        s.step("mr", POST, &mrs, Some(&admin), body, "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("tag", POST, &format!("/projects/{pid}/repository/tags"), Some(&admin), json!({"tag_name": "v1", "ref": "b1"}), "tag").await;
    s.step("status", POST, &format!("/projects/{pid}/statuses/{}", shas[0]), Some(&admin), json!({"state": "failed", "name": "ci", "ref": "b1"}), "status").await;
    s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1", "due_date": "2099-01-01"}), "milestone").await;
    s.settle().await;
    s.graphql(
        "bob dashboard",
        Some(&bob_t),
        "query { currentUser { assigneeOrReviewerMergeRequests { nodes { iid } } groupCount projectCount groups { nodes { fullPath } } } }",
        json!({}),
    )
    .await;
    s.graphql(
        "commit and pipeline",
        Some(&admin),
        "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { commits { nodes { name tags } } headPipeline { refText commitPath cancelable retryable failureReason totalJobs \
         commit { title } mergeRequest { iid } project { name } userPermissions { updatePipeline adminPipeline destroyPipeline cancelPipeline } } } } }",
        json!({"path": full}),
    )
    .await;
    s.graphql(
        "milestone",
        Some(&admin),
        "query($path: ID!) { project(fullPath: $path) { milestones { nodes { title upcoming project { name } group { name } stats { totalIssuesCount closedIssuesCount } } } } }",
        json!({"path": full}),
    )
    .await;
    s.graphql(
        "group",
        Some(&admin),
        "query($path: ID!) { group(fullPath: $path) { avatarUrl editPath archived descendantGroups { nodes { fullPath } } descendantGroupsCount groupMembers { nodes { user { username } accessLevel { stringValue } } } projects { nodes { name } } } }",
        json!({"path": gpath}),
    )
    .await;
    s.finish();
}
