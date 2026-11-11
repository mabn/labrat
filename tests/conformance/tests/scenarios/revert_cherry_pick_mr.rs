//! Reverting and cherry-picking the merge commit of a merged MR through the commits API: the
//! new commits, conflicts and repeats, and what the MR shows afterwards.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn revert_cherry_pick_mr() {
    let mut s = Scenario::new("revert_cherry_pick_mr").await;
    let admin = s.admin();
    let gpath = s.name("rc");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("stable", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "stable", "ref": "main"}), "branch").await;
    s.step("commit", POST, &commits, Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Add f", "actions": [{"action": "create", "file_path": "f.txt", "content": "f\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Add f"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let merged = s.step("merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    let merge_sha = merged.body["merge_commit_sha"].as_str().unwrap_or_default().to_string();
    s.settle().await;
    s.step("revert merge commit", POST, &format!("{commits}/{merge_sha}/revert"), Some(&admin), json!({"branch": "main"}), "commit").await;
    s.step("revert again", POST, &format!("{commits}/{merge_sha}/revert"), Some(&admin), json!({"branch": "main"}), "commit").await;
    s.step("revert dry run", POST, &format!("{commits}/{merge_sha}/revert"), Some(&admin), json!({"branch": "main", "dry_run": true}), "commit").await;
    s.step("cherry-pick to stable", POST, &format!("{commits}/{merge_sha}/cherry_pick"), Some(&admin), json!({"branch": "stable"}), "commit").await;
    s.step("cherry-pick again", POST, &format!("{commits}/{merge_sha}/cherry_pick"), Some(&admin), json!({"branch": "stable"}), "commit").await;
    s.step("cherry-pick dry run", POST, &format!("{commits}/{merge_sha}/cherry_pick"), Some(&admin), json!({"branch": "stable", "dry_run": true}), "commit").await;
    s.step("cherry-pick custom message", POST, &format!("{commits}/{merge_sha}/cherry_pick"), Some(&admin), json!({"branch": "main", "message": "Custom"}), "commit").await;
    s.settle().await;
    s.step("mr after", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("commit mrs", GET, &format!("{commits}/{merge_sha}/merge_requests"), Some(&admin), json!(null), "mr").await;
    s.step("main log", GET, &commits, Some(&admin), json!({"ref_name": "main", "per_page": 5}), "commit").await;
    s.step("stable log", GET, &commits, Some(&admin), json!({"ref_name": "stable", "per_page": 5}), "commit").await;
    s.finish();
}
