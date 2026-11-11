//! Which updates touch the MR (`updated_at`, `updated_by_id`), seen through the `changes` of
//! merge request hooks: quick successive changes by the same user and by another user.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn mr_touch_rules() {
    let mut s = Scenario::new("mr_touch_rules").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("tr");
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
    for l in ["bug", "ui"] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": "#00ff00"}), "label").await;
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mr = format!("/projects/{pid}/merge_requests/1");
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": format!("{}/hook", rx.url), "merge_requests_events": true}),
        "hook",
    )
    .await;
    // Each step runs right after the previous one, well within GitLab's one-minute throttle.
    let ops: Vec<(&str, &String, serde_json::Value)> = vec![
        ("admin adds label", &admin, json!({"add_labels": "bug"})),
        ("admin adds another label", &admin, json!({"add_labels": "ui"})),
        ("admin sets reviewer", &admin, json!({"reviewer_ids": [bob]})),
        ("admin sets assignee", &admin, json!({"assignee_ids": [bob]})),
        ("admin edits title", &admin, json!({"title": "Add a!"})),
        ("admin edits description", &admin, json!({"description": "Body"})),
        ("bob removes label", &bob_t, json!({"remove_labels": "bug"})),
        ("bob removes other label", &bob_t, json!({"remove_labels": "ui"})),
        ("bob removes reviewer", &bob_t, json!({"reviewer_ids": []})),
        ("admin locks", &admin, json!({"discussion_locked": true})),
        ("admin unlocks", &admin, json!({"discussion_locked": false})),
        ("admin schedules", &admin, json!({"merge_after": "2099-01-01T00:00:00Z"})),
        ("bob no-op", &bob_t, json!({"title": "Add a!"})),
        ("bob no-op again", &bob_t, json!({"title": "Add a!"})),
    ];
    for (label, tok, body) in ops {
        s.step(label, PUT, &mr, Some(tok), body, "mr").await;
        s.settle().await;
        s.deliveries(&format!("{label} hooks"), &mut rx, 0).await;
    }
    s.finish();
}
