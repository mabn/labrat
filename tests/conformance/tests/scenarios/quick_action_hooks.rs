//! Webhook deliveries caused by each quick action on its own: which commands fire a merge
//! request `update` hook (and with which `changes`), which fire note or emoji hooks, and which
//! fire nothing.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn quick_action_hooks() {
    let mut s = Scenario::new("quick_action_hooks").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("qh");
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
    s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await;
    for b in ["f", "g"] {
        s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
    }
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr 2", POST, &mrs, Some(&admin), json!({"source_branch": "g", "target_branch": "main", "title": "G", "labels": "bug"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": format!("{}/hook", rx.url), "merge_requests_events": true, "note_events": true, "emoji_events": true}),
        "hook",
    )
    .await;
    let notes = format!("{mrs}/1/notes");
    let commands = [
        "/estimate 2h",
        "/estimate 3h",
        "/remove_estimate",
        "/spend 1h",
        "/title Renamed",
        "/label ~bug",
        "/milestone %v1",
        format!("/assign @{username}").leak(),
        format!("/assign_reviewer @{username}").leak(),
        "/todo",
        "/subscribe",
        "/award :tada:",
        "/copy_metadata !2",
        "/lock",
        "/unlock",
        "/draft",
        "/ready",
        "/target_branch main",
        "/remove_milestone",
        "/unlabel",
        "Plain comment with /estimate 1h inside",
        "Text first\n/estimate 4h",
    ];
    for (i, body) in commands.iter().enumerate() {
        let label = format!("{i:02} {}", body.split_whitespace().next().unwrap_or(""));
        s.step(&label, POST, &notes, Some(&admin), json!({"body": body}), "note").await;
        s.settle().await;
        s.deliveries(&format!("{label} hooks"), &mut rx, 0).await;
    }
    s.finish();
}
