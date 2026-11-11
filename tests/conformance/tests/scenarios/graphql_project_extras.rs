//! Project fields MR clients use around reviewer pickers and project context:
//! `autocompleteUsers`, fork fields of a non-fork, root group, languages, statistics, target
//! branch rules, and the current user's recently viewed items.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_project_extras() {
    let mut s = Scenario::new("graphql_project_extras").await;
    let admin = s.admin();
    let gpath = s.name("gx");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let sub = s.step("subgroup", POST, "/groups", Some(&admin), json!({"name": "sub", "path": "sub", "parent_id": g.id()}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": sub.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/sub/app");
    s.settle().await;
    let mut toks = vec![];
    for (who, level, at_group) in [("bob", 30, false), ("carol", 20, true), ("dave", 40, false)] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": format!("{who} Smith"), "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        let path = if at_group { format!("/groups/{}/members", g.id()) } else { format!("/projects/{pid}/members") };
        s.step(&format!("{who} member"), POST, &path, Some(&admin), json!({"user_id": id, "access_level": level}), "user").await;
        toks.push(tok);
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "main", "commit_message": "Code", "actions": [
        {"action": "create", "file_path": "main.rs", "content": "fn main() {\n    println!(\"hi\");\n}\n"},
        {"action": "create", "file_path": "script.py", "content": "print('hi')\n"}
    ]}), "commit").await;
    s.settle().await;
    let q = "query($p: ID!, $s: String) { project(fullPath: $p) { autocompleteUsers(search: $s) { username } } }";
    s.graphql("autocomplete all", Some(&admin), q, json!({"p": full})).await;
    s.graphql("autocomplete search", Some(&admin), q, json!({"p": full, "s": "smi"})).await;
    s.graphql("autocomplete by bob", Some(&toks[0]), q, json!({"p": full, "s": "car"})).await;
    s.graphql(
        "project extras",
        Some(&admin),
        "query($p: ID!) { project(fullPath: $p) { forkedFrom { fullPath } rootGroup { fullPath }
           languages { name share color } } }",
        json!({"p": full}),
    )
    .await;
    s.graphql("recently viewed", Some(&admin), "query { currentUser { recentlyViewedItems { item { __typename } } } }", json!({})).await;
    s.finish();
}
