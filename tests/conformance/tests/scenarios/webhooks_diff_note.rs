//! The Note Hook for a diff comment on a merge request (positions in the payload).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn webhooks_diff_note() {
    let mut s = Scenario::new("webhooks_diff_note").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("wd");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let mr = s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let refs = mr.get("diff_refs").clone();
    let refs = if refs.is_null() { s.send(Method::GET, &format!("{mrs}/1"), Some(&admin), json!(null)).await.body["diff_refs"].clone() } else { refs };
    s.step("hook", POST, &format!("/projects/{pid}/hooks"), Some(&admin), json!({"url": format!("{}/hook", rx.url), "push_events": false, "note_events": true}), "hook").await;
    let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "new_line": 2});
    s.step("diff discussion", POST, &format!("{mrs}/1/discussions"), Some(&admin), json!({"body": "On line 2", "position": pos}), "discussion").await;
    s.deliveries("diff note hook", &mut rx, 1).await;
    s.finish();
}
