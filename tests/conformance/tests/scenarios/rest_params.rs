//! REST parameter edge cases: quick actions in descriptions, discussion parameters, merge ref,
//! auto-merge notes, position validation and conditional deletes.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn rest_params() {
    let mut s = Scenario::new("rest_params").await;
    let admin = s.admin();
    let gpath = s.name("rp");
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
    s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000"}), "label").await;
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    for b in ["b1", "b2"] {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "one\ntwo\n"}]}), "commit").await;
    }
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step(
        "create with quick actions",
        POST,
        &mrs,
        Some(&admin),
        json!({"source_branch": "b1", "target_branch": "main", "title": "Quick", "description": format!("Some text\n/label ~bug\n/assign @{username}\nmore text")}),
        "mr",
    )
    .await;
    s.poll_mr(pid, 1).await;
    s.step("mr after create", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("update with quick actions", PUT, &format!("{mrs}/1"), Some(&admin), json!({"description": "Updated\n/draft\n/unlabel ~bug"}), "mr").await;
    s.settle().await;
    s.step("mr after update", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("notes after quick actions", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;

    // Discussions: `internal` and bad positions.
    s.step("internal discussion", POST, &format!("{mrs}/1/discussions"), Some(&admin), json!({"body": "Secret?", "internal": true}), "discussion").await;
    let mr = s.send(GET, &format!("{mrs}/1"), Some(&admin), json!(null)).await;
    let refs = mr.body["diff_refs"].clone();
    s.step(
        "position line not a number",
        POST,
        &format!("{mrs}/1/discussions"),
        Some(&admin),
        json!({"body": "x", "position": {"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "new_path": "b1.txt", "new_line": "abc"}}),
        "discussion",
    )
    .await;
    s.step("note bad created_at", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "dated", "created_at": "not a date"}), "note").await;

    // Merge ref is stable while nothing moves; auto-merge twice writes one note.
    s.step("mr 2", POST, &mrs, Some(&admin), json!({"source_branch": "b2", "target_branch": "main", "title": "Second"}), "mr").await;
    s.poll_mr(pid, 2).await;
    let first = s.step("merge ref", GET, &format!("{mrs}/2/merge_ref"), Some(&admin), json!(null), "commit").await;
    let second = s.step("merge ref again", GET, &format!("{mrs}/2/merge_ref"), Some(&admin), json!(null), "commit").await;
    let _ = (first, second);
    let sha = s.send(GET, &format!("{mrs}/2"), Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
    s.step("running status", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "running", "name": "ci"}), "status").await;
    s.settle().await;
    s.step("auto merge", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.step("auto merge again", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.step("notes of mr 2", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}
