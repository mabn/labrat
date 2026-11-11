use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn milestones_and_events() {
    let mut s = Scenario::new("milestones_and_events").await;
    let admin = s.admin();
    let gpath = s.name("ms");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s
        .step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project")
        .await;
    let pid = p.id();
    s.settle().await;
    let ms = format!("/projects/{pid}/milestones");
    let m1 = s.step("create milestone", POST, &ms, Some(&admin), json!({"title": "v1", "description": "First", "due_date": "2030-01-31", "start_date": "2030-01-01"}), "milestone").await;
    s.step("past milestone", POST, &ms, Some(&admin), json!({"title": "old", "due_date": "2020-01-01"}), "milestone").await;
    s.step("duplicate title", POST, &ms, Some(&admin), json!({"title": "v1"}), "milestone").await;
    s.step("missing title", POST, &ms, Some(&admin), json!({"description": "x"}), "milestone").await;
    s.step("bad dates", POST, &ms, Some(&admin), json!({"title": "v2", "due_date": "2030-01-01", "start_date": "2030-02-01"}), "milestone").await;
    s.step("list", GET, &ms, Some(&admin), json!(null), "milestone").await;
    s.step("list active by title", GET, &ms, Some(&admin), json!({"state": "active", "title": "v1"}), "milestone").await;
    s.step("search", GET, &ms, Some(&admin), json!({"search": "fir"}), "milestone").await;
    let mid = m1.id();
    s.step("get", GET, &format!("{ms}/{mid}"), Some(&admin), json!(null), "milestone").await;
    s.step("missing", GET, &format!("{ms}/999999"), Some(&admin), json!(null), "milestone").await;

    s.step(
        "commit",
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "milestone_id": mid, "labels": "bug"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("labels", PUT, &format!("{mrs}/1"), Some(&admin), json!({"add_labels": "ui", "remove_labels": "bug"}), "mr").await;
    s.step("milestone removed", PUT, &format!("{mrs}/1"), Some(&admin), json!({"milestone_id": 0}), "mr").await;
    s.step("milestone again", PUT, &format!("{mrs}/1"), Some(&admin), json!({"milestone_id": mid}), "mr").await;
    s.step("close", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("reopen", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "reopen"}), "mr").await;
    s.settle().await;
    let mr = format!("{mrs}/1");
    let le = s.step("label events", GET, &format!("{mr}/resource_label_events"), Some(&admin), json!(null), "label_event").await;
    let first = le.body[0]["id"].as_i64().unwrap_or(0);
    s.step("label event", GET, &format!("{mr}/resource_label_events/{first}"), Some(&admin), json!(null), "label_event").await;
    s.step("missing label event", GET, &format!("{mr}/resource_label_events/999999"), Some(&admin), json!(null), "label_event").await;
    s.step("state events", GET, &format!("{mr}/resource_state_events"), Some(&admin), json!(null), "state_event").await;
    s.step("milestone events", GET, &format!("{mr}/resource_milestone_events"), Some(&admin), json!(null), "milestone_event").await;
    s.step("milestone mrs", GET, &format!("{ms}/{mid}/merge_requests"), Some(&admin), json!(null), "mr").await;
    s.step("close milestone", PUT, &format!("{ms}/{mid}"), Some(&admin), json!({"state_event": "close", "description": "Done"}), "milestone").await;
    s.step("closed list", GET, &ms, Some(&admin), json!({"state": "closed"}), "milestone").await;
    s.step("delete milestone", DELETE, &format!("{ms}/{mid}"), Some(&admin), json!(null), "milestone").await;
    s.step("mr after milestone delete", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("milestone events after delete", GET, &format!("{mr}/resource_milestone_events"), Some(&admin), json!(null), "milestone_event").await;
    s.finish();
}
