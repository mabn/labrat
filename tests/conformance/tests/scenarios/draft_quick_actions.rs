//! Draft notes that hold quick actions: what the draft shows, what publishing one draft and a
//! whole review does with the commands, and commands mixed with text and replies.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn draft_quick_actions() {
    let mut s = Scenario::new("draft_quick_actions").await;
    let admin = s.admin();
    let gpath = s.name("dq");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    for l in ["bug", "ui"] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": "#ff0000"}), "label").await;
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "F", "actions": [{"action": "create", "file_path": "f.txt", "content": "one\ntwo\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let drafts = format!("{mr}/draft_notes");
    let thread = s.step("thread", POST, &format!("{mr}/discussions"), Some(&admin), json!({"body": "Thread"}), "discussion").await;
    let tid = thread.body["id"].as_str().unwrap_or_default().to_string();
    // A draft that is only a command, published on its own.
    let d1 = s.step("draft label only", POST, &drafts, Some(&admin), json!({"note": "/label ~bug"}), "draft").await.id();
    s.step("mr before publish", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("publish label only", PUT, &format!("{drafts}/{d1}/publish"), Some(&admin), json!(null), "draft").await;
    s.settle().await;
    s.step("mr after single publish", GET, &mr, Some(&admin), json!(null), "mr").await;
    // A review of several drafts with commands, text and a reply.
    s.step("draft text and commands", POST, &drafts, Some(&admin), json!({"note": "Looks good\n/label ~ui\n/assign me"}), "draft").await;
    s.step("draft reply with command", POST, &drafts, Some(&admin), json!({"note": "Done\n/title Renamed", "in_reply_to_discussion_id": tid}), "draft").await;
    s.step("draft unlabel", POST, &drafts, Some(&admin), json!({"note": "/unlabel ~bug"}), "draft").await;
    s.step("draft estimate", POST, &drafts, Some(&admin), json!({"note": "/estimate 2h\n/spend 30m"}), "draft").await;
    s.step("list drafts", GET, &drafts, Some(&admin), json!(null), "draft").await;
    s.step("bulk publish", POST, &format!("{drafts}/bulk_publish"), Some(&admin), json!(null), "draft").await;
    s.settle().await;
    s.step("mr after review", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("notes after review", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    // Another user's review: approve and request reviewer via commands.
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob draft approve", POST, &drafts, Some(&bob_t), json!({"note": "/approve"}), "draft").await;
    s.step("bob draft text", POST, &drafts, Some(&bob_t), json!({"note": "Nice\n/award :tada:"}), "draft").await;
    s.step("bob approvals before", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("bob publishes", POST, &format!("{drafts}/bulk_publish"), Some(&bob_t), json!(null), "draft").await;
    s.settle().await;
    s.step("approvals after", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("awards", GET, &format!("{mr}/award_emoji"), Some(&admin), json!(null), "award").await;
    s.step("final notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.step("final mr", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.finish();
}
