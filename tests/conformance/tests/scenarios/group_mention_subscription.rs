//! Whether a group mention makes the group's members participants (and so implicitly
//! subscribed), compared with a user mention.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn group_mention_subscription() {
    let mut s = Scenario::new("group_mention_subscription").await;
    let admin = s.admin();
    let gpath = s.name("gm");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut toks = vec![];
    for who in ["bob", "cat"] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} member"), POST, &format!("/groups/{gid}/members"), Some(&admin), json!({"user_id": id, "access_level": 30}), "user").await;
        toks.push((username, tok));
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "F", "actions": [{"action": "create", "file_path": "f.txt", "content": "x\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("group mention", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": format!("Note about @{gpath}")}), "note").await;
    s.step("user mention", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": format!("And @{}", toks[0].0)}), "note").await;
    s.settle().await;
    s.step("participants", GET, &format!("{mrs}/1/participants"), Some(&admin), json!(null), "user").await;
    s.graphql("graphql participants", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { participants { nodes { username } } } } }", json!({"p": full})).await;
    for (name, tok) in toks.clone() {
        let short = name.split('-').next().unwrap_or("").to_string();
        s.step(&format!("{short} sees"), GET, &format!("{mrs}/1"), Some(&tok), json!(null), "mr").await;
        s.graphql(&format!("{short} graphql"), Some(&tok), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { subscribed } } }", json!({"p": full})).await;
        s.step(&format!("{short} unsubscribe cmd"), POST, &format!("{mrs}/1/notes"), Some(&tok), json!({"body": "/unsubscribe"}), "note").await;
    }
    s.finish();
}
