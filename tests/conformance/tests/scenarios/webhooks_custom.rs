//! Webhook custom headers, URL variables and custom payload templates on merge request events.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn webhooks_custom() {
    let mut s = Scenario::new("webhooks_custom").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("wc");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let hooks = format!("/projects/{pid}/hooks");
    let template = r#"{"kind":"{{object_kind}}","title":"{{object_attributes.title}}","who":"{{user.username}}","missing":"{{object_attributes.nope}}","labels":"{{labels}}"}"#;
    let h = s
        .step(
            "hook",
            POST,
            &hooks,
            Some(&admin),
            json!({"url": format!("{}/hook/{{secret}}", rx.url), "merge_requests_events": true, "note_events": true, "push_events": false,
                   "url_variables": [{"key": "secret", "value": "s3cr3t"}],
                   "custom_headers": [{"key": "X-Custom", "value": "hello"}],
                   "custom_webhook_template": template}),
            "hook",
        )
        .await;
    let hid = h.id();
    s.step("show", GET, &format!("{hooks}/{hid}"), Some(&admin), json!(null), "hook").await;
    s.step("bad template", PUT, &format!("{hooks}/{hid}"), Some(&admin), json!({"custom_webhook_template": "{\"a\": \"{{"}), "hook").await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Say \"hi\""}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.deliveries("mr hook", &mut rx, 1).await;
    s.step("note", POST, &format!("{}/merge_requests/1/notes", format!("/projects/{pid}")), Some(&admin), json!({"body": "Comment"}), "note").await;
    s.settle().await;
    s.deliveries("note hook", &mut rx, 1).await;
    // Change the header and the variable, drop the template.
    s.step("set header", PUT, &format!("{hooks}/{hid}/custom_headers/X-Custom"), Some(&admin), json!({"value": "changed"}), "hook").await;
    s.step("set variable", PUT, &format!("{hooks}/{hid}/url_variables/secret"), Some(&admin), json!({"value": "other"}), "hook").await;
    s.step("no template", PUT, &format!("{hooks}/{hid}"), Some(&admin), json!({"custom_webhook_template": ""}), "hook").await;
    s.step("update mr", PUT, &format!("/projects/{pid}/merge_requests/1"), Some(&admin), json!({"title": "Renamed"}), "mr").await;
    s.settle().await;
    s.deliveries("update hook", &mut rx, 1).await;
    s.step("delete header", DELETE, &format!("{hooks}/{hid}/custom_headers/X-Custom"), Some(&admin), json!(null), "hook").await;
    s.step("test hook", POST, &format!("{hooks}/{hid}/test/merge_requests_events"), Some(&admin), json!(null), "hook").await;
    s.settle().await;
    s.deliveries("test hook delivery", &mut rx, 1).await;
    s.step("events", GET, &format!("{hooks}/{hid}/events"), Some(&admin), json!(null), "hook_event").await;
    s.finish();
}
