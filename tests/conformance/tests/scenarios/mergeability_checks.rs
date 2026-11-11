//! GraphQL `MergeRequest.mergeabilityChecks`: every check with its status, in GitLab's order.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn mergeability_checks() {
    let mut s = Scenario::new("mergeability_checks").await;
    let admin = s.admin();
    let gpath = s.name("mc");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    s.step("settings", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_all_discussions_are_resolved": true, "only_allow_merge_if_pipeline_succeeds": true}), "project").await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let mut shas = vec![];
    for (i, (b, title, extra)) in [
        ("b1", "Clean", json!({})),
        ("b2", "Draft: Busy", json!({"merge_after": "2099-01-01T00:00:00Z"})),
    ]
    .iter()
    .enumerate()
    {
        let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        shas.push(c.get("id").as_str().unwrap().to_string());
        s.settle().await;
        let mut body = json!({"source_branch": b, "target_branch": "main", "title": title});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        s.step("mr", POST, &mrs, Some(&admin), body, "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("thread", POST, &format!("{mrs}/2/discussions"), Some(&admin), json!({"body": "Open question"}), "discussion").await;
    s.step("running", POST, &format!("/projects/{pid}/statuses/{}", shas[1]), Some(&admin), json!({"state": "running", "name": "ci"}), "status").await;
    s.step("success", POST, &format!("/projects/{pid}/statuses/{}", shas[0]), Some(&admin), json!({"state": "success", "name": "ci"}), "status").await;
    s.settle().await;
    let q = |iid: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequest(iid: \"{iid}\") {{ detailedMergeStatus mergeabilityChecks {{ identifier status }} }} }} }}");
    s.graphql("clean", Some(&admin), &q("1"), json!({"path": full})).await;
    s.graphql("blocked", Some(&admin), &q("2"), json!({"path": full})).await;
    s.step("close clean", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.settle().await;
    s.graphql("closed", Some(&admin), &q("1"), json!({"path": full})).await;
    s.finish();
}
