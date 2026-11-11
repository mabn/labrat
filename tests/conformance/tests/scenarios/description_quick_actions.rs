//! Quick actions in merge request descriptions: on create and on update, through REST and
//! GraphQL, and which commands are applied, stripped or kept.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn description_quick_actions() {
    let mut s = Scenario::new("description_quick_actions").await;
    let admin = s.admin();
    let gpath = s.name("dq");
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
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000"}), "label").await;
    s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await;
    for b in ["a", "b", "c"] {
        s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
    }
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let desc = format!("Fixes things\n/label ~bug\n/milestone %v1\n/assign @{username}\n/assign_reviewer @{username}\n/estimate 1h\n/draft\n/approve\n/merge\n/close\n/todo\n/unknown_cmd");
    s.step("create with commands", POST, &mrs, Some(&admin), json!({"source_branch": "a", "target_branch": "main", "title": "A", "description": desc}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("mr 1 approvals", GET, &format!("{mrs}/1/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("create plain", POST, &mrs, Some(&admin), json!({"source_branch": "b", "target_branch": "main", "title": "B", "description": "Plain"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("update with commands", PUT, &format!("{mrs}/2"), Some(&admin), json!({"description": format!("Now\n/label ~bug\n/title Retitled\n/assign @{username}\n/spend 30m\n/lock")}), "mr").await;
    s.settle().await;
    s.step("mr 2", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.graphql(
        "graphql create",
        Some(&admin),
        "mutation($p: ID!, $d: String) { mergeRequestCreate(input: {projectPath: $p, sourceBranch: \"c\", targetBranch: \"main\", title: \"C\", description: $d}) { errors mergeRequest { iid title description labels { nodes { title } } milestone { title } } } }",
        json!({"p": full, "d": "Via GraphQL\n/label ~bug\n/milestone %v1"}),
    )
    .await;
    s.poll_mr(pid, 3).await;
    s.graphql(
        "graphql update",
        Some(&admin),
        "mutation($p: ID!, $d: String) { mergeRequestUpdate(input: {projectPath: $p, iid: \"3\", description: $d}) { errors mergeRequest { description labels { nodes { title } } milestone { title } } } }",
        json!({"p": full, "d": "Updated\n/unlabel ~bug\n/remove_milestone"}),
    )
    .await;
    s.settle().await;
    s.step("mr 3", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
