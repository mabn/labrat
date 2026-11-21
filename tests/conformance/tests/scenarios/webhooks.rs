use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

async fn allow_local(s: &mut Scenario, allow: bool) {
    let admin = s.admin();
    let r = s
        .send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": allow}))
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording && allow {
        // Sidekiq caches application settings for a minute.
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
}

#[tokio::test]
async fn webhooks_flow() {
    let mut s = Scenario::new("webhooks_flow").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let gpath = s.name("hk");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s
        .step(
            "project",
            POST,
            "/projects",
            Some(&admin),
            json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}),
            "project",
        )
        .await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step(
            "create bob",
            POST,
            "/users",
            Some(&admin),
            json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"),
                   "password": "Password-123-labrat", "skip_confirmation": true}),
            "user",
        )
        .await
        .id();
    let bob_t = s
        .step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token")
        .await
        .get("token")
        .as_str()
        .unwrap()
        .to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user")
        .await;

    // Hook management
    let hooks = format!("/projects/{pid}/hooks");
    allow_local(&mut s, false).await;
    // A private address rather than the receiver's: when recording through a tunnel the
    // receiver's URL is public.
    s.step("local url blocked", POST, &hooks, Some(&admin), json!({"url": "http://10.0.0.1/hook"}), "hook").await;
    allow_local(&mut s, true).await;
    s.step("developer cannot list hooks", GET, &hooks, Some(&bob_t), json!(null), "hook").await;
    s.step("missing url", POST, &hooks, Some(&admin), json!({"push_events": true}), "hook").await;
    s.step("bad url", POST, &hooks, Some(&admin), json!({"url": "ftp://example.com/x"}), "hook").await;
    let h = s
        .step(
            "create hook",
            POST,
            &hooks,
            Some(&admin),
            json!({"url": format!("{}/hook", rx.url), "name": "receiver", "token": "s3cret",
                   "push_events": true, "tag_push_events": true, "merge_requests_events": true,
                   "note_events": true, "confidential_note_events": true, "emoji_events": true,
                   "push_events_branch_filter": "feat*"}),
            "hook",
        )
        .await;
    let hid = h.id();
    let hook = format!("{hooks}/{hid}");
    s.step("get hook", GET, &hook, Some(&admin), json!(null), "hook").await;
    s.step("list hooks", GET, &hooks, Some(&admin), json!(null), "hook").await;
    s.step("update hook name", PUT, &hook, Some(&admin), json!({"name": "receiver 2", "description": "the receiver"}), "hook").await;
    s.step("missing hook", GET, &format!("{hooks}/999999"), Some(&admin), json!(null), "hook").await;

    // Pushes
    let commits = format!("/projects/{pid}/repository/commits");
    s.step(
        "commit on new feature branch",
        POST,
        &commits,
        Some(&admin),
        json!({"branch": "feature", "start_branch": "main", "commit_message": "Add app\n\nWith a body.",
               "actions": [{"action": "create", "file_path": "app.txt", "content": "one\ntwo\n"}]}),
        "commit",
    )
    .await;
    s.deliveries("push to feature", &mut rx, 1).await;
    s.step(
        "commit on main",
        POST,
        &commits,
        Some(&admin),
        json!({"branch": "main", "commit_message": "Docs", "actions": [{"action": "create", "file_path": "docs.md", "content": "docs\n"}]}),
        "commit",
    )
    .await;
    s.deliveries("push to main is filtered", &mut rx, 0).await;
    s.step(
        "annotated tag",
        POST,
        &format!("/projects/{pid}/repository/tags"),
        Some(&admin),
        json!({"tag_name": "v1.0", "ref": "main", "message": "First release"}),
        "tag",
    )
    .await;
    s.deliveries("tag push", &mut rx, 1).await;

    // Merge request events
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step(
        "create mr",
        POST,
        &mrs,
        Some(&admin),
        json!({"title": "Add app", "description": "Adds the app", "source_branch": "feature", "target_branch": "main",
               "reviewer_ids": [bob], "assignee_ids": [bob]}),
        "mr",
    )
    .await;
    s.poll_mr(pid, 1).await;
    s.deliveries("mr open", &mut rx, 1).await;
    let mr = format!("{mrs}/1");
    s.step("mark draft", PUT, &mr, Some(&admin), json!({"title": "Draft: Add app", "description": "Adds the app.\n\nMore."}), "mr").await;
    s.deliveries("mr update title", &mut rx, 1).await;
    s.step("same values", PUT, &mr, Some(&admin), json!({"title": "Draft: Add app"}), "mr").await;
    s.deliveries("mr update without changes", &mut rx, 1).await;

    // Notes and emoji
    let note = s.step("bob comments", POST, &format!("{mr}/notes"), Some(&bob_t), json!({"body": "Looks good"}), "note").await;
    s.deliveries("note hook", &mut rx, 1).await;
    s.step("internal note", POST, &format!("{mr}/notes"), Some(&admin), json!({"body": "Secret", "internal": true}), "note").await;
    s.deliveries("confidential note hook", &mut rx, 1).await;
    s.step("bob edits note", PUT, &format!("{mr}/notes/{}", note.id()), Some(&bob_t), json!({"body": "Looks great"}), "note").await;
    s.deliveries("note update hook", &mut rx, 1).await;
    let award = s.step("bob awards mr", POST, &format!("{mr}/award_emoji"), Some(&bob_t), json!({"name": "thumbsup"}), "award").await;
    s.deliveries("emoji award", &mut rx, 1).await;
    s.step("admin awards note", POST, &format!("{mr}/notes/{}/award_emoji", note.id()), Some(&admin), json!({"name": "tada"}), "award").await;
    s.deliveries("emoji award on note", &mut rx, 1).await;
    s.step("bob revokes", DELETE, &format!("{mr}/award_emoji/{}", award.id()), Some(&bob_t), json!(null), "award").await;
    s.deliveries("emoji revoke", &mut rx, 1).await;

    // Push to the source branch: push hook + MR update with oldrev
    let pushed = s.step(
        "push to feature",
        POST,
        &commits,
        Some(&admin),
        json!({"branch": "feature", "commit_message": "More", "actions": [{"action": "update", "file_path": "app.txt", "content": "one\ntwo\nthree\n"}]}),
        "commit",
    )
    .await;
    // Collect the deliveries before reading the MR: a read triggers the mergeability check,
    // which would race with the hook's payload.
    s.deliveries("push with open mr", &mut rx, 2).await;
    let sha = pushed.get("id").as_str().unwrap().to_string();
    s.poll(&mr, move |v| v["sha"] == sha.as_str()).await;
    s.poll_mr(pid, 1).await;
    s.step("mark ready", PUT, &mr, Some(&admin), json!({"title": "Add app"}), "mr").await;
    s.deliveries("mr ready", &mut rx, 1).await;

    // Approvals
    s.step("bob approves", POST, &format!("{mr}/approve"), Some(&bob_t), json!(null), "approval").await;
    s.deliveries("approved", &mut rx, 1).await;
    s.step("bob unapproves", POST, &format!("{mr}/unapprove"), Some(&bob_t), json!(null), "approval").await;
    s.deliveries("unapproved", &mut rx, 1).await;

    // Close, reopen, merge
    s.step("close", PUT, &mr, Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.deliveries("close", &mut rx, 1).await;
    s.step("reopen", PUT, &mr, Some(&admin), json!({"state_event": "reopen"}), "mr").await;
    s.deliveries("reopen", &mut rx, 1).await;
    s.poll_mr(pid, 1).await;
    s.step("merge", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.deliveries("merge", &mut rx, 1).await;

    // Test deliveries
    for trigger in ["push_events", "tag_push_events", "merge_requests_events", "note_events", "emoji_events"] {
        s.step(&format!("test {trigger}"), POST, &format!("{hook}/test/{trigger}"), Some(&admin), json!(null), "hook").await;
        s.deliveries(&format!("test delivery {trigger}"), &mut rx, 1).await;
    }
    for trigger in ["issues_events", "confidential_note_events", "pipeline_events", "bogus_events"] {
        s.step(&format!("test {trigger}"), POST, &format!("{hook}/test/{trigger}"), Some(&admin), json!(null), "hook").await;
    }
    s.step("events", GET, &format!("{hook}/events"), Some(&admin), json!({"per_page": 2}), "hook_log").await;
    s.step("events bad per_page", GET, &format!("{hook}/events"), Some(&admin), json!({"per_page": 50}), "hook_log").await;

    // Changing the URL drops the token; failing receivers.
    s.step("change url", PUT, &hook, Some(&admin), json!({"url": format!("{}/fail", rx.url)}), "hook").await;
    s.step("test failing", POST, &format!("{hook}/test/push_events"), Some(&admin), json!(null), "hook").await;
    s.deliveries("failing delivery without token", &mut rx, 1).await;
    s.step("events after failure", GET, &format!("{hook}/events"), Some(&admin), json!({"per_page": 1}), "hook_log").await;
    s.step("delete hook", DELETE, &hook, Some(&admin), json!(null), "hook").await;
    s.step("deleted hook", GET, &hook, Some(&admin), json!(null), "hook").await;
    allow_local(&mut s, false).await;
    s.finish();
}
