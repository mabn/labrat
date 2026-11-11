//! To-do snoozing, bulk resolution and deletion (GraphQL), and how snoozed to-dos are listed.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn todo_snooze() {
    let mut s = Scenario::new("todo_snooze").await;
    let admin = s.admin();
    let gpath = s.name("ts");
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
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, b) in ["b1", "b2", "b3"].iter().enumerate() {
        s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}"), "assignee_ids": [bob]}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    let list = "query($snoozed: Boolean) { currentUser { todos(isSnoozed: $snoozed) { count nodes { id state snoozedUntil body } } } }";
    let r = s.graphql("pending", Some(&bob_t), list, json!({})).await;
    let ids: Vec<String> = r.body["data"]["currentUser"]["todos"]["nodes"].as_array().cloned().unwrap_or_default().iter().map(|n| n["id"].as_str().unwrap_or_default().to_string()).collect();
    let out = "todo { state snoozedUntil } errors";
    s.graphql("snooze", Some(&bob_t), &format!("mutation($id: TodoID!) {{ todoSnooze(input: {{id: $id, snoozeUntil: \"2099-01-01T00:00:00Z\"}}) {{ {out} }} }}"), json!({"id": ids[0]})).await;
    s.graphql("snooze in the past", Some(&bob_t), &format!("mutation($id: TodoID!) {{ todoSnooze(input: {{id: $id, snoozeUntil: \"2000-01-01T00:00:00Z\"}}) {{ {out} }} }}"), json!({"id": ids[1]})).await;
    s.graphql("pending after snooze", Some(&bob_t), list, json!({})).await;
    s.graphql("snoozed only", Some(&bob_t), list, json!({"snoozed": true})).await;
    s.step("rest pending", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.graphql("unsnooze", Some(&bob_t), &format!("mutation($id: TodoID!) {{ todoUnSnooze(input: {{id: $id}}) {{ {out} }} }}"), json!({"id": ids[0]})).await;
    s.graphql(
        "snooze many",
        Some(&bob_t),
        "mutation($ids: [TodoID!]!) { todoSnoozeMany(input: {ids: $ids, snoozeUntil: \"2099-01-01T00:00:00Z\"}) { todos { state snoozedUntil } errors } }",
        json!({"ids": [ids[0], ids[1]]}),
    )
    .await;
    s.graphql("unsnooze many", Some(&bob_t), "mutation($ids: [TodoID!]!) { todoUnsnoozeMany(input: {ids: $ids}) { todos { state snoozedUntil } errors } }", json!({"ids": [ids[0], ids[1]]})).await;
    s.graphql("resolve many", Some(&bob_t), "mutation($ids: [TodoID!]!) { todoResolveMany(input: {ids: $ids}) { todos { state } errors } }", json!({"ids": [ids[0], ids[1]]})).await;
    s.graphql("delete many", Some(&bob_t), "mutation($ids: [TodoID!]!) { todoDeleteMany(input: {ids: $ids}) { errors } }", json!({"ids": [ids[0], ids[2]]})).await;
    s.graphql("done after delete", Some(&bob_t), "query { currentUser { todos(state: [done, pending]) { count nodes { state } } } }", json!({})).await;
    s.graphql("delete all done", Some(&bob_t), "mutation { todoDeleteAllDone(input: {}) { errors } }", json!({})).await;
    // `todoDeleteAllDone` runs in a worker; when it finishes isn't observable reliably.
    s.finish();
}
