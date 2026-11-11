//! Extreme pagination values on MR endpoints: huge, zero, negative and non-numeric `page` and
//! `per_page`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn pagination_extremes() {
    let mut s = Scenario::new("pagination_extremes").await;
    let admin = s.admin();
    let gpath = s.name("px");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "f", "actions": [{"action": "create", "file_path": "f.txt", "content": "x\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("note", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "hi"}), "note").await;
    let endpoints = [mrs.clone(), format!("{mrs}/1/notes"), format!("{mrs}/1/diffs"), format!("{mrs}/1/discussions"), format!("/projects/{pid}/repository/commits")];
    let values = ["9223372036854775807", "9223372036854775808", "2147483648", "0", "-1", "abc", "1.5"];
    for e in &endpoints {
        let short = e.rsplit('/').next().unwrap_or("");
        for v in values {
            s.step(&format!("{short} page={v}"), GET, e, Some(&admin), json!({"page": v}), short).await;
        }
        for v in ["0", "-1", "101", "9223372036854775807", "abc"] {
            s.step(&format!("{short} per_page={v}"), GET, e, Some(&admin), json!({"per_page": v}), short).await;
        }
    }
    s.step("big page and per_page", GET, &mrs, Some(&admin), json!({"page": "4611686018427387904", "per_page": "100"}), "merge_requests").await;
    s.finish();
}
