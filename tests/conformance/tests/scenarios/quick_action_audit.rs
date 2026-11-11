//! Quick-action findings of docs/gitlab-ref/quick-actions-audit.md: commands of other types,
//! user and label argument parsing, empty and invalid arguments, time-tracking grammar, merge
//! and rebase messages, reactions, shrug, same-note sequencing and new-MR descriptions.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn quick_action_audit() {
    let mut s = Scenario::new("quick_action_audit").await;
    let admin = s.admin();
    let gpath = s.name("qa");
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
    for (l, col) in [("bug", "#ff0000"), ("ui", "#00ff00")] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": col}), "label").await;
    }
    s.step("group milestone", POST, &format!("/groups/{}/milestones", g.id()), Some(&admin), json!({"title": "rel-1"}), "milestone").await;
    for b in ["f", "g"] {
        s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
    }
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F", "labels": "bug,ui"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let notes = format!("{mrs}/1/notes");
    let bodies: Vec<String> = vec![
        "/due tomorrow".into(),
        "/relate #1".into(),
        "/confidential".into(),
        "/epic &1".into(),
        "/q some text".into(),
        "/unlabel bug".into(),
        "/label bug".into(),
        "/label ~zzz".into(),
        "/unlabel ~zzz".into(),
        "/relabel".into(),
        "/assign me".into(),
        format!("/assign @{username} thanks"),
        format!("/assign {username}"),
        "/assign @nobody-at-all".into(),
        "/target_branch nope".into(),
        "/title".into(),
        "/estimate 30".into(),
        "/estimate 1.5h".into(),
        "/estimate 0".into(),
        "/spend 1h 30m".into(),
        "/spend 1h 2025-01-01".into(),
        "/react thumbsup".into(),
        "/react :nonexistent:".into(),
        "/react :tada: thanks".into(),
        "/shrug hello".into(),
        "/tableflip hey".into(),
        "/approve\n/approve".into(),
        "/approve\n/unapprove".into(),
        "/assign_reviewer @nobody-at-all".into(),
        "/reviewer @nobody-at-all".into(),
        "/milestone %\"rel-1\"".into(),
        "/copy_metadata #1".into(),
        "/run_pipeline".into(),
        "/submit_review approve".into(),
        "/merge".into(),
    ];
    for (i, body) in bodies.iter().enumerate() {
        let label = format!("{i:02} {}", body.lines().next().unwrap_or(""));
        s.step(&label, POST, &notes, Some(&admin), json!({"body": body}), "note").await;
        s.settle().await;
    }
    s.step("mr after", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("notes after", GET, &notes, Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    // A new MR whose description holds commands that need a saved MR.
    s.step(
        "create with persisted-only commands",
        POST,
        &mrs,
        Some(&admin),
        json!({"source_branch": "g", "target_branch": "main", "title": "G", "description": "Body\n/title Other\n/lock\n/subscribe\n/react :tada:\n/remove_milestone"}),
        "mr",
    )
    .await;
    s.poll_mr(pid, 2).await;
    s.step("mr 2", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("mr 2 award", GET, &format!("{mrs}/2/award_emoji"), Some(&admin), json!(null), "award").await;
    let _ = PUT;
    s.finish();
}
