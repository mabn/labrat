//! The cached to-do count of `/user_counts`: snoozing doesn't refresh it, other to-do changes
//! do, and the fresh count leaves snoozed to-dos out.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn todo_count_cache() {
    let mut s = Scenario::new("todo_count_cache").await;
    let admin = s.admin();
    let gpath = s.name("tc");
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
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, b) in ["x", "y"].iter().enumerate() {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("mention 1", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": format!("@{username} look")}), "note").await;
    s.step("mention 2", POST, &format!("{mrs}/2/notes"), Some(&admin), json!({"body": format!("@{username} look too")}), "note").await;
    s.settle().await;
    s.step("counts", GET, "/user_counts", Some(&bob_t), json!(null), "counts").await;
    let todos = s.send(GET, "/todos", Some(&bob_t), json!(null)).await;
    let first = todos.body[0]["id"].clone();
    let until = format!("{}T00:00:00Z", s.date_in(3));
    s.graphql(
        "snooze",
        Some(&bob_t),
        "mutation($id: TodoID!, $u: Time!) { todoSnooze(input: {id: $id, snoozeUntil: $u}) { errors todo { state } } }",
        json!({"id": format!("gid://gitlab/Todo/{first}"), "u": until}),
    )
    .await;
    s.step("counts after snooze", GET, "/user_counts", Some(&bob_t), json!(null), "counts").await;
    s.graphql("pending count", Some(&bob_t), "query { currentUser { todos(state: [pending]) { count } } }", json!({})).await;
    s.step("todos list", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    // Another to-do arrives: the count is recomputed.
    s.step("mention 3", POST, &format!("{mrs}/2/notes"), Some(&admin), json!({"body": format!("@{username} again")}), "note").await;
    s.settle().await;
    s.step("counts after new todo", GET, "/user_counts", Some(&bob_t), json!(null), "counts").await;
    s.graphql(
        "unsnooze",
        Some(&bob_t),
        "mutation($id: TodoID!) { todoUnSnooze(input: {id: $id}) { errors } }",
        json!({"id": format!("gid://gitlab/Todo/{first}")}),
    )
    .await;
    s.step("counts after unsnooze", GET, "/user_counts", Some(&bob_t), json!(null), "counts").await;
    s.step("mark all done", POST, "/todos/mark_as_done", Some(&bob_t), json!(null), "todo").await;
    s.step("counts after done", GET, "/user_counts", Some(&bob_t), json!(null), "counts").await;
    s.finish();
}
