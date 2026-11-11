//! When to-dos are created and resolved around an MR, and how they are listed and filtered.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn todo_lifecycle() {
    let mut s = Scenario::new("todo_lifecycle").await;
    let admin = s.admin();
    let gpath = s.name("tl");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut users: Vec<(i64, String, String)> = vec![];
    for base in ["bob", "carol", "dave"] {
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
    let (bob, bob_name, bob_t) = users[0].clone();
    let (carol, carol_name, carol_t) = users[1].clone();
    let (_dave, dave_name, dave_t) = users[2].clone();
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step(
        "mr with mentions",
        POST,
        &mrs,
        Some(&admin),
        json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "description": format!("@{bob_name} please look, also @{carol_name}"), "assignee_ids": [bob], "reviewer_ids": [carol]}),
        "mr",
    )
    .await;
    s.poll_mr(pid, 1).await;
    let todos = |t: &str| (t.to_string(), json!({"state": "pending"}));
    for (label, t) in [("bob todos", &bob_t), ("carol todos", &carol_t)] {
        let (tok, q) = todos(t);
        s.step(label, GET, "/todos", Some(&tok), q, "todo").await;
    }

    // A reply in a new thread resolves only to-dos on that thread; a plain comment resolves all.
    s.step("bob starts thread", POST, &format!("{mrs}/1/discussions"), Some(&bob_t), json!({"body": "A question"}), "discussion").await;
    s.settle().await;
    s.step("bob todos after thread", GET, "/todos", Some(&bob_t), json!({"state": "pending"}), "todo").await;
    s.step("bob comments", POST, &format!("{mrs}/1/notes"), Some(&bob_t), json!({"body": "Looks fine"}), "note").await;
    s.settle().await;
    s.step("bob todos after comment", GET, "/todos", Some(&bob_t), json!({"state": "pending"}), "todo").await;
    s.step("carol reacts", POST, &format!("{mrs}/1/award_emoji"), Some(&carol_t), json!({"name": "eyes"}), "award").await;
    s.settle().await;
    s.step("carol todos after emoji", GET, "/todos", Some(&carol_t), json!({"state": "pending"}), "todo").await;

    // Re-requested reviews and submitted reviews don't duplicate pending to-dos.
    let carol_gid = format!("gid://gitlab/User/{carol}");
    let rereview = "mutation($path: ID!, $user: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $path, iid: \"1\", userId: $user}) { errors } }";
    s.graphql("rereview", Some(&admin), rereview, json!({"path": full, "user": carol_gid})).await;
    s.settle().await;
    s.graphql("rereview again", Some(&admin), rereview, json!({"path": full, "user": carol_gid})).await;
    s.settle().await;
    s.step("carol todos after rereview", GET, "/todos", Some(&carol_t), json!({"state": "pending"}), "todo").await;
    for i in 0..2 {
        s.step("carol draft", POST, &format!("{mrs}/1/draft_notes"), Some(&carol_t), json!({"note": format!("Review note {i}")}), "draft").await;
        s.step("carol submits", POST, &format!("{mrs}/1/draft_notes/bulk_publish"), Some(&carol_t), json!(null), "draft").await;
        s.settle().await;
    }
    s.step("admin todos after reviews", GET, "/todos", Some(&admin), json!({"state": "pending", "action": "review_submitted"}), "todo").await;
    s.step("bob todos after reviews", GET, "/todos", Some(&bob_t), json!({"state": "pending"}), "todo").await;

    // Mentions added later: description edits skip earlier mentions; edited notes too.
    s.step("description adds dave", PUT, &format!("{mrs}/1"), Some(&admin), json!({"description": format!("@{bob_name} please look, also @{carol_name} and @{dave_name}")}), "mr").await;
    s.settle().await;
    let note = s.step("admin note", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "No mentions yet"}), "note").await;
    let note_id = note.id();
    s.settle().await;
    s.step("admin edits note", PUT, &format!("{mrs}/1/notes/{note_id}"), Some(&admin), json!({"body": format!("Now asking @{dave_name} and `@{carol_name}`")}), "note").await;
    s.settle().await;
    s.step("dave todos", GET, "/todos", Some(&dave_t), json!({"state": "pending"}), "todo").await;
    s.step("carol todos after edit", GET, "/todos", Some(&carol_t), json!({"state": "pending"}), "todo").await;

    // Filters.
    s.step("bad action", GET, "/todos", Some(&dave_t), json!({"action": "bogus"}), "todo").await;
    s.step("work item type", GET, "/todos", Some(&dave_t), json!({"type": "WorkItem"}), "todo").await;
    let q = "query { currentUser { todos(state: pending) { nodes { action body targetUrl target { __typename ... on MergeRequest { iid } } note { body } group { name } snoozedUntil } } } }";
    s.graphql("dave graphql todos", Some(&dave_t), q, json!({})).await;
    let other = s.step("other project", POST, "/projects", Some(&admin), json!({"name": "other", "namespace_id": g.id()}), "project").await.id();
    s.step("dave in other", POST, &format!("/projects/{other}/members"), Some(&admin), json!({"user_id": users[2].0, "access_level": 30}), "user").await;
    s.settle().await;
    s.step("mark done other project", POST, "/todos/mark_as_done", Some(&dave_t), json!({"project_id": other}), "todo").await;
    s.step("dave todos still pending", GET, "/todos", Some(&dave_t), json!({"state": "pending"}), "todo").await;
    s.graphql(
        "mark all done other project",
        Some(&dave_t),
        "mutation($p: [ID!]) { todosMarkAllDone(input: {projectId: $p}) { todos { action } errors } }",
        json!({"p": [format!("gid://gitlab/Project/{other}")]}),
    )
    .await;
    s.step("dave todos after graphql", GET, "/todos", Some(&dave_t), json!({"state": "pending"}), "todo").await;
    s.step("mark done unknown project", POST, "/todos/mark_as_done", Some(&dave_t), json!({"project_id": 999999}), "todo").await;
    s.step("dave todos after unknown", GET, "/todos", Some(&dave_t), json!({"state": "pending"}), "todo").await;

    // Closing by someone else leaves the others' to-dos pending.
    s.graphql("rereview before close", Some(&admin), rereview, json!({"path": full, "user": carol_gid})).await;
    s.settle().await;
    s.step("admin closes", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.settle().await;
    s.step("dave todos after close", GET, "/todos", Some(&dave_t), json!({"state": "pending"}), "todo").await;
    s.step("carol todos after close", GET, "/todos", Some(&carol_t), json!({"state": "pending"}), "todo").await;
    let _ = Value::Null;
    s.finish();
}
