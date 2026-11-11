//! A project hook whose receiver always fails (docs/gitlab-ref/labrat-shortcuts-audit.md item 3):
//! deliveries per event, retries, and the hook's `alert_status` / `disabled_until` as failures
//! pile up (`WebHooks::AutoDisabling`).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn webhook_failures() {
    let mut s = Scenario::new("webhook_failures").await;
    let admin = s.admin();
    let mut rx = s.receiver().await;
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("wf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let url = format!("{}/fail", rx.url);
    let h = s.step("hook", POST, &format!("/projects/{pid}/hooks"), Some(&admin), json!({"url": url, "note_events": true}), "hook").await;
    let hook = format!("/projects/{pid}/hooks/{}", h.id());
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "f", "actions": [{"action": "create", "file_path": "f.txt", "content": "x\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    for i in 1..=6 {
        s.step(&format!("note {i}"), POST, &format!("/projects/{pid}/merge_requests/1/notes"), Some(&admin), json!({"body": format!("Note {i}")}), "note").await;
        s.settle().await;
        s.deliveries(&format!("deliveries after note {i}"), &mut rx, 1).await;
        s.step(&format!("hook after note {i}"), GET, &hook, Some(&admin), json!(null), "hook").await;
    }
    s.step("hook events", GET, &format!("{hook}/events"), Some(&admin), json!({"per_page": 20}), "hook_log").await;
    s.step("test while disabled", POST, &format!("{hook}/test/note_events"), Some(&admin), json!(null), "hook").await;
    s.deliveries("deliveries after test", &mut rx, 1).await;
    s.step("re-enable by url change", PUT, &hook, Some(&admin), json!({"url": format!("{}/ok", rx.url)}), "hook").await;
    s.step("note after re-enable", POST, &format!("/projects/{pid}/merge_requests/1/notes"), Some(&admin), json!({"body": "After"}), "note").await;
    s.settle().await;
    s.deliveries("deliveries after re-enable", &mut rx, 1).await;
    s.step("hook final", GET, &hook, Some(&admin), json!(null), "hook").await;
    s.finish();
}
