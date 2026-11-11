//! Webhooks around skipped pipelines: the push, MR and pipeline events when a `[skip ci]` commit
//! is pushed, an MR is opened on it, and a commit status is posted for the skipped commit.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn skip_ci_hooks() {
    let mut s = Scenario::new("skip_ci_hooks").await;
    let admin = s.admin();
    let mut rx = s.receiver().await;
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
    let url = format!("{}/hook", rx.url);
    s.step("hook", POST, &format!("/projects/{pid}/hooks"), Some(&admin), json!({"url": url, "push_events": true, "merge_requests_events": true, "pipeline_events": true, "job_events": true}), "hook").await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let c = s.step("skip commit", POST, &commits, Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Work [skip ci]", "actions": [{"action": "create", "file_path": "f.txt", "content": "x\n"}]}), "commit").await;
    let sha = c.body["id"].as_str().unwrap_or_default().to_string();
    s.settle().await;
    s.deliveries("skip push hooks", &mut rx, 2).await;
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.settle().await;
    s.deliveries("mr hooks", &mut rx, 2).await;
    s.step("status on skipped commit", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "running", "name": "ci", "ref": "f"}), "status").await;
    s.settle().await;
    s.deliveries("status hooks", &mut rx, 2).await;
    s.step("pipelines", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("mr after status", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
