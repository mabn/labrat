//! Extreme durations for time tracking (REST endpoints and quick actions) and an oversized
//! GraphQL cursor.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn time_tracking_extremes() {
    let mut s = Scenario::new("time_tracking_extremes").await;
    let admin = s.admin();
    let gpath = s.name("tx");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "f", "actions": [{"action": "create", "file_path": "f.txt", "content": "x\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let durations = ["9223372036854775807s", "-9223372036854775807s", "2147483648s", "-2147483648s", "999999999999w", "1mo1mo1mo", "333333333333333333333333333333m", "1y"];
    for d in durations {
        s.step(&format!("spend {d}"), POST, &format!("{mr}/add_spent_time"), Some(&admin), json!({"duration": d}), "time").await;
        s.step(&format!("estimate {d}"), POST, &format!("{mr}/time_estimate"), Some(&admin), json!({"duration": d}), "time").await;
        s.step(&format!("note spend {d}"), POST, &format!("{mr}/notes"), Some(&admin), json!({"body": format!("/spend {d}")}), "note").await;
        s.step(&format!("note estimate {d}"), POST, &format!("{mr}/notes"), Some(&admin), json!({"body": format!("/estimate {d}")}), "note").await;
    }
    s.step("time stats", GET, &format!("{mr}/time_stats"), Some(&admin), json!(null), "time").await;
    s.step("notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.graphql("huge cursor", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequests(after: \"eyJpZCI6IjEwMDAwMDAwMDAwMDAwMDAwMDAwMCJ9\") { nodes { iid } } } }", json!({"p": full})).await;
    s.finish();
}
