//! Updates of closed and merged MRs through REST: retargeting, `remove_source_branch`, state
//! events that don't apply, and what the MR and its notes show afterwards.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn closed_mr_updates() {
    let mut s = Scenario::new("closed_mr_updates").await;
    let admin = s.admin();
    let gpath = s.name("cu");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("stable", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "stable", "ref": "main"}), "branch").await;
    for b in ["a", "b"] {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
    }
    s.settle().await;
    for (i, b) in ["a", "b"].iter().enumerate() {
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("close 1", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("merge 2", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.step("retarget closed", PUT, &format!("{mrs}/1"), Some(&admin), json!({"target_branch": "stable"}), "mr").await;
    s.step("remove source closed", PUT, &format!("{mrs}/1"), Some(&admin), json!({"remove_source_branch": true}), "mr").await;
    s.step("close closed", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("retarget merged", PUT, &format!("{mrs}/2"), Some(&admin), json!({"target_branch": "stable"}), "mr").await;
    s.step("remove source merged", PUT, &format!("{mrs}/2"), Some(&admin), json!({"remove_source_branch": false}), "mr").await;
    s.step("reopen merged", PUT, &format!("{mrs}/2"), Some(&admin), json!({"state_event": "reopen"}), "mr").await;
    s.step("title merged", PUT, &format!("{mrs}/2"), Some(&admin), json!({"title": "Renamed after merge"}), "mr").await;
    s.settle().await;
    s.step("mr 1", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("mr 2", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("reopen retargeted", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "reopen"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1 reopened", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
