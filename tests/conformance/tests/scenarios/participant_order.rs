//! Participant order when approval notes (created asynchronously) race with comments.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn participant_order() {
    let mut s = Scenario::new("participant_order").await;
    let admin = s.admin();
    let gpath = s.name("po");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut toks = vec![];
    for who in ["bob", "carol"] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} developer"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": 30}), "user").await;
        toks.push(tok);
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "F", "actions": [{"action": "create", "file_path": "f.txt", "content": "f\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("bob approves", POST, &format!("{mrs}/1/approve"), Some(&toks[0]), json!(null), "approval").await;
    s.step("carol approves", POST, &format!("{mrs}/1/approve"), Some(&toks[1]), json!(null), "approval").await;
    s.step("carol comments", POST, &format!("{mrs}/1/notes"), Some(&toks[1]), json!({"body": "Nice"}), "note").await;
    s.step("bob internal note", POST, &format!("{mrs}/1/notes"), Some(&toks[0]), json!({"body": "Internal", "internal": true}), "note").await;
    s.settle().await;
    s.step("notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc", "order_by": "created_at"}), "note").await;
    s.step("participants", GET, &format!("{mrs}/1/participants"), Some(&admin), json!(null), "user").await;
    s.graphql("graphql participants", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { participants { nodes { username } } } } }", json!({"p": full})).await;
    s.finish();
}
