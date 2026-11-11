//! `Note.userPermissions` and `Discussion.resolved`/`resolvable` per role and note kind.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_note_permissions() {
    let mut s = Scenario::new("graphql_note_permissions").await;
    let admin = s.admin();
    let gpath = s.name("np");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut tokens = vec![];
    for (base, level) in [("bob", 30), ("rita", 20)] {
        let username = s.name(base);
        let id = s
            .step(base, POST, "/users", Some(&admin), json!({"username": username, "name": base, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let t = s.step("token", POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step("member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": level}), "user").await;
        tokens.push(t);
    }
    s.settle().await;
    let (bob_t, rita_t) = (tokens[0].clone(), tokens[1].clone());
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("rita comment", POST, &format!("{mrs}/1/notes"), Some(&rita_t), json!({"body": "Plain from rita"}), "note").await;
    s.step("bob thread", POST, &format!("{mrs}/1/discussions"), Some(&bob_t), json!({"body": "Thread from bob"}), "discussion").await;
    s.step("label for system note", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "/title Add a file"}), "note").await;
    s.settle().await;
    let q = "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { notes { nodes { body system author { username } \
             userPermissions { readNote createNote adminNote awardEmoji resolveNote repositionNote } } } } } }";
    for (label, tok) in [("as admin", admin.clone()), ("as bob", bob_t.clone()), ("as rita", rita_t.clone())] {
        s.graphql(label, Some(&tok), q, json!({"path": full})).await;
    }
    s.finish();
}
