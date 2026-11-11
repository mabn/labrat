//! Group access tokens: their bot users act on merge requests in the group's projects.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn group_access_tokens() {
    let mut s = Scenario::new("group_access_tokens").await;
    let admin = s.admin();
    let gpath = s.name("gt");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let expires = s.date_in(30);
    let tokens = format!("/groups/{gid}/access_tokens");
    s.step("missing scopes", POST, &tokens, Some(&admin), json!({"name": "bot"}), "token").await;
    let t = s.step("create", POST, &tokens, Some(&admin), json!({"name": "group-bot", "scopes": ["api"], "access_level": 30, "expires_at": expires}), "token").await;
    let tid = t.id();
    let bot_token = t.body["token"].as_str().unwrap_or_default().to_string();
    let bot_id = t.body["user_id"].as_i64().unwrap_or(0);
    s.step("list", GET, &tokens, Some(&admin), json!(null), "token").await;
    s.step("show", GET, &format!("{tokens}/{tid}"), Some(&admin), json!(null), "token").await;
    s.step("group members", GET, &format!("/groups/{gid}/members"), Some(&admin), json!(null), "user").await;
    s.step("bot user", GET, &format!("/users/{bot_id}"), Some(&admin), json!(null), "user").await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("bot opens mr", POST, &mrs, Some(&bot_token), json!({"source_branch": "feature", "target_branch": "main", "title": "From the group bot"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("bot comments", POST, &format!("{mrs}/1/notes"), Some(&bot_token), json!({"body": "Group bot here"}), "note").await;
    s.step("revoke", DELETE, &format!("{tokens}/{tid}"), Some(&admin), json!(null), "token").await;
    s.step("revoked", GET, "/user", Some(&bot_token), json!(null), "user").await;
    s.finish();
}
