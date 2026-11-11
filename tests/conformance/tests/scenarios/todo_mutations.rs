//! Marking MRs as to-do (REST and GraphQL), `currentUser.todos`, and the to-do mutations.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn todo_mutations() {
    let mut s = Scenario::new("todo_mutations").await;
    let admin = s.admin();
    let gpath = s.name("td");
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
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "assignee_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;

    s.step("bob marks todo", POST, &format!("{mrs}/1/todo"), Some(&bob_t), json!(null), "todo").await;
    s.step("bob marks again", POST, &format!("{mrs}/1/todo"), Some(&bob_t), json!(null), "todo").await;
    s.step("missing mr todo", POST, &format!("{mrs}/99/todo"), Some(&bob_t), json!(null), "todo").await;

    let gid = {
        let r = s.graphql("mr gid", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { id } } }", json!({"path": full})).await;
        r.body["data"]["project"]["mergeRequest"]["id"].as_str().unwrap_or_default().to_string()
    };
    let todo_out = "todo { id action state body targetType author { username } project { name } } errors";
    s.graphql("admin todoCreate", Some(&admin), &format!("mutation($id: TodoableID!) {{ todoCreate(input: {{targetId: $id}}) {{ {todo_out} }} }}"), json!({"id": gid})).await;
    s.graphql("admin todoCreate again", Some(&admin), &format!("mutation($id: TodoableID!) {{ todoCreate(input: {{targetId: $id}}) {{ {todo_out} }} }}"), json!({"id": gid})).await;
    let list = "query { currentUser { todos(state: pending) { count nodes { id action state targetType body } } } }";
    let bob_todos = s.graphql("bob pending todos", Some(&bob_t), list, json!({})).await;
    let first = bob_todos.body["data"]["currentUser"]["todos"]["nodes"][0]["id"].as_str().unwrap_or_default().to_string();
    s.graphql("bob todos by type", Some(&bob_t), "query { currentUser { todos(type: [MERGEREQUEST], action: [assigned]) { count nodes { action } } } }", json!({})).await;
    s.graphql("bob marks done", Some(&bob_t), &format!("mutation($id: TodoID!) {{ todoMarkDone(input: {{id: $id}}) {{ {todo_out} }} }}"), json!({"id": first})).await;
    s.graphql("admin marks bob's done", Some(&admin), &format!("mutation($id: TodoID!) {{ todoMarkDone(input: {{id: $id}}) {{ {todo_out} }} }}"), json!({"id": first})).await;
    s.graphql("bob done todos", Some(&bob_t), "query { currentUser { todos(state: done) { count nodes { action state } } } }", json!({})).await;
    s.graphql("bob restores", Some(&bob_t), &format!("mutation($id: TodoID!) {{ todoRestore(input: {{id: $id}}) {{ {todo_out} }} }}"), json!({"id": first})).await;
    s.graphql("bob marks all done", Some(&bob_t), "mutation { todosMarkAllDone(input: {}) { todos { action state } errors } }", json!({})).await;
    s.graphql("bob marks all done again", Some(&bob_t), "mutation { todosMarkAllDone(input: {}) { todos { action state } errors } }", json!({})).await;
    s.graphql("bob restores many", Some(&bob_t), "mutation($ids: [TodoID!]!) { todoRestoreMany(input: {ids: $ids}) { todos { action state } errors } }", json!({"ids": [first]})).await;
    s.graphql("bob marks all done for mr", Some(&bob_t), "mutation($id: TodoableID!) { todosMarkAllDone(input: {targetId: $id}) { todos { action state } errors } }", json!({"id": gid})).await;
    s.graphql("bob pending after", Some(&bob_t), list, json!({})).await;
    s.step("rest bob todos", Method::GET, "/todos", Some(&bob_t), json!({"state": "done"}), "todo").await;
    s.finish();
}
