//! Project access tokens and their bot users, as review bots use them on merge requests.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn project_access_tokens() {
    let mut s = Scenario::new("project_access_tokens").await;
    let admin = s.admin();
    let gpath = s.name("pt");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let _ = full;
    let expires = s.date_in(30);
    let rotated = s.date_in(60);
    let tokens = format!("/projects/{pid}/access_tokens");
    s.step("missing fields", POST, &tokens, Some(&admin), json!({"name": "bot"}), "token").await;
    s.step("bad level", POST, &tokens, Some(&admin), json!({"name": "bot", "scopes": ["api"], "access_level": 33, "expires_at": expires}), "token").await;
    let t = s.step("create", POST, &tokens, Some(&admin), json!({"name": "review-bot", "scopes": ["api"], "access_level": 30, "expires_at": expires}), "token").await;
    let tid = t.id();
    let bot_token = t.body["token"].as_str().unwrap_or_default().to_string();
    let bot_id = t.body["user_id"].as_i64().unwrap_or(0);
    s.step("list", GET, &tokens, Some(&admin), json!(null), "token").await;
    s.step("show", GET, &format!("{tokens}/{tid}"), Some(&admin), json!(null), "token").await;
    s.step("members", GET, &format!("/projects/{pid}/members/all"), Some(&admin), json!(null), "user").await;
    s.step("bot user", GET, &format!("/users/{bot_id}"), Some(&admin), json!(null), "user").await;
    s.step("bot self", GET, "/user", Some(&bot_token), json!(null), "user").await;
    s.graphql("bot current user", Some(&bot_token), "query { currentUser { username name bot human type state } }", json!({})).await;
    // The bot at work on a merge request.
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr with bot reviewer", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bot_id]}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("bot comments", POST, &format!("{mrs}/1/notes"), Some(&bot_token), json!({"body": "Automated review: looks fine"}), "note").await;
    s.step("bot approves", POST, &format!("{mrs}/1/approve"), Some(&bot_token), json!(null), "approval").await;
    s.step("bot opens mr", POST, &mrs, Some(&bot_token), json!({"source_branch": "feature", "target_branch": "main", "title": "Duplicate"}), "mr").await;
    s.settle().await;
    s.step("mr", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("bot todos", GET, "/todos", Some(&bot_token), json!(null), "todo").await;
    // Rotation and revocation.
    let r = s.step("rotate", POST, &format!("{tokens}/{tid}/rotate"), Some(&admin), json!({"expires_at": rotated}), "token").await;
    let new_token = r.body["token"].as_str().unwrap_or_default().to_string();
    let new_id = r.id();
    s.step("old token after rotate", GET, "/user", Some(&bot_token), json!(null), "user").await;
    s.step("new token", GET, "/user", Some(&new_token), json!(null), "user").await;
    s.step("revoke", DELETE, &format!("{tokens}/{new_id}"), Some(&admin), json!(null), "token").await;
    s.step("revoked token", GET, "/user", Some(&new_token), json!(null), "user").await;
    s.step("list after revoke", GET, &tokens, Some(&admin), json!(null), "token").await;
    s.step("list active", GET, &tokens, Some(&admin), json!({"state": "active"}), "token").await;
    let _ = PUT;
    s.finish();
}
