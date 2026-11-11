//! System hooks: `repository_update` on pushes, and merge request events.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn system_hooks_repo() {
    let mut s = Scenario::new("system_hooks_repo").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("sh");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let h = s
        .step("system hook", POST, "/hooks", Some(&admin), json!({"url": format!("{}/system", rx.url), "push_events": false, "merge_requests_events": false}), "hook")
        .await;
    let hid = h.id();
    s.step("system hook settings", GET, &format!("/hooks/{hid}"), Some(&admin), json!(null), "hook").await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "main", "commit_message": "Change", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.deliveries("repository update", &mut rx, 1).await;
    s.step("branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "topic", "ref": "main"}), "branch").await;
    s.deliveries("repository update for new branch", &mut rx, 1).await;
    s.step("delete system hook", DELETE, &format!("/hooks/{hid}"), Some(&admin), json!(null), "hook").await;
    s.finish();
}
