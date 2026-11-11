//! What a comment on a commit triggers: note webhooks, contribution events and mention to-dos.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn commit_note_effects() {
    let mut s = Scenario::new("commit_note_effects").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        // Sidekiq caches application settings for a minute.
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("cn");
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
    let commits = format!("/projects/{pid}/repository/commits");
    let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\n"}]}), "commit").await;
    let sha = c.get("id").as_str().unwrap().to_string();
    let parent = c.get("parent_ids")[0].as_str().unwrap().to_string();
    s.settle().await;
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": format!("{}/hook", rx.url), "push_events": false, "note_events": true}),
        "hook",
    )
    .await;

    s.step("plain comment", POST, &format!("{commits}/{sha}/comments"), Some(&admin), json!({"note": format!("Thanks @{username}")}), "note").await;
    s.deliveries("plain comment hook", &mut rx, 1).await;
    s.step("line comment", POST, &format!("{commits}/{sha}/comments"), Some(&admin), json!({"note": "On a line", "path": "a.txt", "line": 2, "line_type": "new"}), "note").await;
    s.deliveries("line comment hook", &mut rx, 1).await;
    let pos = json!({"base_sha": parent, "start_sha": parent, "head_sha": sha, "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "new_line": 1});
    let d = s.step("diff discussion", POST, &format!("{commits}/{sha}/discussions"), Some(&admin), json!({"body": "Diff thread", "position": pos}), "discussion").await;
    s.deliveries("diff discussion hook", &mut rx, 1).await;
    let did = d.get("id").as_str().unwrap_or_default().to_string();
    s.step("bob replies", POST, &format!("{commits}/{sha}/discussions/{did}/notes"), Some(&bob_t), json!({"body": "Reply"}), "note").await;
    s.deliveries("reply hook", &mut rx, 1).await;
    s.settle().await;
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.step("project events", GET, &format!("/projects/{pid}/events"), Some(&admin), json!({"target_type": "note"}), "event").await;
    s.step("bob events", GET, &format!("/users/{bob}/events"), Some(&admin), json!(null), "event").await;
    s.finish();
}
