//! Repeated and conflicting quick actions in one note: which one wins, how updates add up and
//! which messages are reported.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn quick_action_combos() {
    let mut s = Scenario::new("quick_action_combos").await;
    let admin = s.admin();
    let gpath = s.name("qc");
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
    for (l, col) in [("bug", "#ff0000"), ("ui", "#00ff00"), ("docs", "#0000ff")] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": col}), "label").await;
    }
    for m in ["v1", "v2"] {
        s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": m}), "milestone").await;
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "F", "actions": [{"action": "create", "file_path": "f.txt", "content": "f\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F", "labels": "docs"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let notes = format!("{mrs}/1/notes");
    let bodies = [
        ("title twice", "/title First\n/title Second"),
        ("assign twice", format!("/assign @claude\n/assign @{username}").leak() as &str),
        ("label then unlabel", "/label ~bug\n/unlabel ~bug"),
        ("unlabel twice", "/unlabel ~docs\n/unlabel ~bug"),
        ("relabel then label", "/relabel ~ui\n/label ~bug"),
        ("milestone twice", "/milestone %v1\n/milestone %v2"),
        ("draft then ready", "/draft\n/ready"),
        ("ready then draft", "/ready\n/draft"),
        ("close then reopen", "/close\n/reopen"),
        ("reviewer twice", format!("/assign_reviewer @claude\n/assign_reviewer @{username}").leak() as &str),
        ("estimate twice", "/estimate 1h\n/estimate 2h"),
        ("spend twice", "/spend 1h\n/spend 30m"),
        ("todo and done", "/todo\n/done"),
        ("subscribe twice", "/unsubscribe\n/subscribe\n/unsubscribe"),
        ("lock unlock", "/lock\n/unlock"),
        ("award twice", "/award :thumbsup:\n/award :thumbsup:"),
        ("unassign then assign", format!("/unassign\n/assign @{username}").leak() as &str),
    ];
    for (label, body) in bodies {
        s.step(label, POST, &notes, Some(&admin), json!({"body": body}), "note").await;
        s.settle().await;
        s.step(&format!("{label} mr"), GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    }
    s.step("notes after", GET, &notes, Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.step("todos", GET, "/todos", Some(&admin), json!({"project_id": pid}), "todo").await;
    s.finish();
}
