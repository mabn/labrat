use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

/// Creates a group and a project with a README; returns (group id, project id).
async fn setup(s: &mut Scenario, name: &str, merge_method: &str) -> (i64, i64) {
    let admin = s.admin();
    let gpath = s.name(name);
    let g = s
        .step(
            "create group",
            POST,
            "/groups",
            Some(&admin),
            json!({"name": gpath, "path": gpath}),
            "namespace",
        )
        .await;
    let p = s
        .step(
            "create project",
            POST,
            "/projects",
            Some(&admin),
            json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "merge_method": merge_method}),
            "project",
        )
        .await;
    s.settle().await;
    (g.id(), p.id())
}

async fn commit(
    s: &mut Scenario,
    pid: i64,
    label: &str,
    branch: &str,
    start: Option<&str>,
    actions: Value,
) {
    let admin = s.admin();
    let mut body = json!({"branch": branch, "commit_message": label, "actions": actions});
    if let Some(sb) = start {
        body["start_branch"] = json!(sb);
    }
    s.step(
        label,
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        body,
        "commit",
    )
    .await;
}

#[tokio::test]
async fn merge_request_lifecycle() {
    let mut s = Scenario::new("merge_request_lifecycle").await;
    let admin = s.admin();
    let (gid, pid) = setup(&mut s, "mrlife", "merge").await;
    commit(
        &mut s,
        pid,
        "Add feature file",
        "feature",
        Some("main"),
        json!([{"action": "create", "file_path": "feature.txt", "content": "one\ntwo\nthree\n"}]),
    )
    .await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let me = s
        .step(
            "current user",
            GET,
            "/user",
            Some(&admin),
            Value::Null,
            "user",
        )
        .await;
    let my_id = me.id();
    s.step(
        "create mr",
        POST,
        &mrs,
        Some(&admin),
        json!({"title": "Add feature", "description": "Adds a feature.\n\n- [ ] test it\n- [x] write it",
               "source_branch": "feature", "target_branch": "main", "labels": "bug,ui", "assignee_id": my_id}),
        "mr",
    )
    .await;
    s.step(
        "duplicate mr",
        POST,
        &mrs,
        Some(&admin),
        json!({"title": "Dup", "source_branch": "feature", "target_branch": "main"}),
        "mr",
    )
    .await;
    s.step(
        "same branch",
        POST,
        &mrs,
        Some(&admin),
        json!({"title": "Same", "source_branch": "main", "target_branch": "main"}),
        "mr",
    )
    .await;
    s.step(
        "missing branch",
        POST,
        &mrs,
        Some(&admin),
        json!({"title": "Missing", "source_branch": "nope", "target_branch": "main"}),
        "mr",
    )
    .await;
    s.step(
        "missing title",
        POST,
        &mrs,
        Some(&admin),
        json!({"source_branch": "feature", "target_branch": "main"}),
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "get mr",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "get mr again",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step("list mrs", GET, &mrs, Some(&admin), Value::Null, "mr")
        .await;
    s.step(
        "list simple",
        GET,
        &mrs,
        Some(&admin),
        json!({"view": "simple", "state": "opened"}),
        "mr",
    )
    .await;
    s.step(
        "group mrs",
        GET,
        &format!("/groups/{gid}/merge_requests"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "commits",
        GET,
        &format!("{mrs}/1/commits"),
        Some(&admin),
        Value::Null,
        "commit",
    )
    .await;
    s.step(
        "changes",
        GET,
        &format!("{mrs}/1/changes"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "diffs",
        GET,
        &format!("{mrs}/1/diffs"),
        Some(&admin),
        Value::Null,
        "diff",
    )
    .await;
    s.step(
        "versions",
        GET,
        &format!("{mrs}/1/versions"),
        Some(&admin),
        Value::Null,
        "version",
    )
    .await;
    s.step(
        "participants",
        GET,
        &format!("{mrs}/1/participants"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "update title and draft",
        PUT,
        &format!("{mrs}/1"),
        Some(&admin),
        json!({"title": "Draft: Add feature X", "reviewer_ids": [my_id], "remove_labels": "ui", "add_labels": "backend"}),
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "merge draft",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "mark ready",
        PUT,
        &format!("{mrs}/1"),
        Some(&admin),
        json!({"title": "Add feature X", "description": "Changed."}),
        "mr",
    )
    .await;
    commit(&mut s, pid, "Second change", "feature", None,
        json!([{"action": "update", "file_path": "feature.txt", "content": "one\ntwo\nthree\nfour\n"}])).await;
    s.settle().await;
    s.step(
        "get after push",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "versions after push",
        GET,
        &format!("{mrs}/1/versions"),
        Some(&admin),
        Value::Null,
        "version",
    )
    .await;
    s.step(
        "reviewers",
        GET,
        &format!("{mrs}/1/reviewers"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "label events",
        GET,
        &format!("{mrs}/1/resource_label_events"),
        Some(&admin),
        Value::Null,
        "label_event",
    )
    .await;
    s.step(
        "wrong sha",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        json!({"sha": "0000000000000000000000000000000000000000"}),
        "mr",
    )
    .await;
    s.step(
        "merge",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        json!({"should_remove_source_branch": true}),
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "get merged",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "merge again",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "state events",
        GET,
        &format!("{mrs}/1/resource_state_events"),
        Some(&admin),
        Value::Null,
        "state_event",
    )
    .await;
    s.step(
        "branches after merge",
        GET,
        &format!("/projects/{pid}/repository/branches"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.step(
        "main commits",
        GET,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        Value::Null,
        "commit",
    )
    .await;
    s.finish();
}

#[tokio::test]
async fn merge_request_close_reopen_conflict() {
    let mut s = Scenario::new("merge_request_close_reopen_conflict").await;
    let admin = s.admin();
    let (_, pid) = setup(&mut s, "mrconf", "merge").await;
    commit(
        &mut s,
        pid,
        "Branch change",
        "topic",
        Some("main"),
        json!([{"action": "update", "file_path": "README.md", "content": "topic version\n"}]),
    )
    .await;
    commit(
        &mut s,
        pid,
        "Main change",
        "main",
        None,
        json!([{"action": "update", "file_path": "README.md", "content": "main version\n"}]),
    )
    .await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step(
        "create",
        POST,
        &mrs,
        Some(&admin),
        json!({"title": "Topic", "source_branch": "topic", "target_branch": "main"}),
        "mr",
    )
    .await;
    s.poll_mr(pid, 1).await;
    s.step(
        "get conflict",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "merge conflict",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "close",
        PUT,
        &format!("{mrs}/1"),
        Some(&admin),
        json!({"state_event": "close"}),
        "mr",
    )
    .await;
    s.step(
        "closed list",
        GET,
        &mrs,
        Some(&admin),
        json!({"state": "closed"}),
        "mr",
    )
    .await;
    s.step(
        "reopen",
        PUT,
        &format!("{mrs}/1"),
        Some(&admin),
        json!({"state_event": "reopen"}),
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "state events",
        GET,
        &format!("{mrs}/1/resource_state_events"),
        Some(&admin),
        Value::Null,
        "state_event",
    )
    .await;
    s.step(
        "lock discussion",
        PUT,
        &format!("{mrs}/1"),
        Some(&admin),
        json!({"discussion_locked": true}),
        "mr",
    )
    .await;
    s.step(
        "subscribe again",
        POST,
        &format!("{mrs}/1/subscribe"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "unsubscribe",
        POST,
        &format!("{mrs}/1/unsubscribe"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "time estimate",
        POST,
        &format!("{mrs}/1/time_estimate"),
        Some(&admin),
        json!({"duration": "3h30m"}),
        "stats",
    )
    .await;
    s.step(
        "time stats",
        GET,
        &format!("{mrs}/1/time_stats"),
        Some(&admin),
        Value::Null,
        "stats",
    )
    .await;
    s.finish();
}

#[tokio::test]
async fn merge_request_fast_forward() {
    let mut s = Scenario::new("merge_request_fast_forward").await;
    let admin = s.admin();
    let (_, pid) = setup(&mut s, "mrff", "ff").await;
    commit(
        &mut s,
        pid,
        "Feature commit",
        "ff-feature",
        Some("main"),
        json!([{"action": "create", "file_path": "a.txt", "content": "a\n"}]),
    )
    .await;
    commit(
        &mut s,
        pid,
        "Main moves",
        "main",
        None,
        json!([{"action": "create", "file_path": "b.txt", "content": "b\n"}]),
    )
    .await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step(
        "create",
        POST,
        &mrs,
        Some(&admin),
        json!({"title": "FF feature", "source_branch": "ff-feature", "target_branch": "main"}),
        "mr",
    )
    .await;
    s.poll_mr(pid, 1).await;
    s.step(
        "get needs rebase",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "get needs rebase 2",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "merge needs rebase",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "rebase",
        PUT,
        &format!("{mrs}/1/rebase"),
        Some(&admin),
        Value::Null,
        "rebase",
    )
    .await;
    s.settle().await;
    s.step(
        "after rebase",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        json!({"include_rebase_in_progress": true}),
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "after rebase 2",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "merge ff",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "main log",
        GET,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        Value::Null,
        "commit",
    )
    .await;
    s.finish();
}

#[tokio::test]
async fn merge_request_stacked_retarget() {
    let mut s = Scenario::new("merge_request_stacked_retarget").await;
    let admin = s.admin();
    let (_, pid) = setup(&mut s, "mrstack", "merge").await;
    commit(
        &mut s,
        pid,
        "Step one",
        "stack/1",
        Some("main"),
        json!([{"action": "create", "file_path": "one.txt", "content": "1\n"}]),
    )
    .await;
    commit(
        &mut s,
        pid,
        "Step two",
        "stack/2",
        Some("stack/1"),
        json!([{"action": "create", "file_path": "two.txt", "content": "2\n"}]),
    )
    .await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr 1", POST, &mrs, Some(&admin), json!({"title": "One", "source_branch": "stack/1", "target_branch": "main", "remove_source_branch": true}), "mr").await;
    s.step("mr 2", POST, &mrs, Some(&admin), json!({"title": "Two", "source_branch": "stack/2", "target_branch": "stack/1", "remove_source_branch": true}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.poll_mr(pid, 2).await;
    s.step(
        "get 1",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "merge 1",
        PUT,
        &format!("{mrs}/1/merge"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.poll(&format!("{mrs}/2"), |b| b["target_branch"] == "main")
        .await;
    s.poll_mr(pid, 2).await;
    s.step(
        "get 2 retargeted",
        GET,
        &format!("{mrs}/2"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.settle().await;
    s.step(
        "get 2 again",
        GET,
        &format!("{mrs}/2"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.step(
        "branches",
        GET,
        &format!("/projects/{pid}/repository/branches"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.finish();
}
