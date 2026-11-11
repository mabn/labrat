//! Merge request description templates (`.gitlab/merge_request_templates/*.md`) through the
//! project templates API.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn mr_templates() {
    let mut s = Scenario::new("mr_templates").await;
    let admin = s.admin();
    let gpath = s.name("tp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let t = format!("/projects/{pid}/templates/merge_requests");
    s.step("no templates", GET, &t, Some(&admin), json!(null), "template").await;
    s.step(
        "add templates",
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        json!({"branch": "main", "commit_message": "Templates", "actions": [
            {"action": "create", "file_path": ".gitlab/merge_request_templates/Default.md", "content": "## What\n\n## Why\n"},
            {"action": "create", "file_path": ".gitlab/merge_request_templates/Bug fix.md", "content": "Fixes #\n"},
            {"action": "create", "file_path": ".gitlab/merge_request_templates/notes.txt", "content": "not a template\n"},
            {"action": "create", "file_path": ".gitlab/issue_templates/Bug.md", "content": "Issue\n"}
        ]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.step("list", GET, &t, Some(&admin), json!(null), "template").await;
    s.step("show", GET, &format!("{t}/Default"), Some(&admin), json!(null), "template").await;
    s.step("show with space", GET, &format!("{t}/Bug%20fix"), Some(&admin), json!(null), "template").await;
    s.step("show missing", GET, &format!("{t}/Nope"), Some(&admin), json!(null), "template").await;
    s.step("issue templates", GET, &format!("/projects/{pid}/templates/issues"), Some(&admin), json!(null), "template").await;
    s.step("bad type", GET, &format!("/projects/{pid}/templates/nope"), Some(&admin), json!(null), "template").await;
    s.finish();
}
