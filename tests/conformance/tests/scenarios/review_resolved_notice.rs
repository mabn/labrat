//! Publishing a review on an MR whose threads are all resolved adds no second notice.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn review_resolved_notice() {
    let mut s = Scenario::new("review_resolved_notice").await;
    let admin = s.admin();
    let gpath = s.name("rn");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    let _ = c;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let d = s.step("thread", POST, &format!("{mrs}/1/discussions"), Some(&admin), json!({"body": "Question"}), "discussion").await;
    let did = d.get("id").as_str().unwrap_or_default().to_string();
    s.step("resolve", PUT, &format!("{mrs}/1/discussions/{did}"), Some(&admin), json!({"resolved": true}), "discussion").await;
    s.settle().await;
    s.step("draft", POST, &format!("{mrs}/1/draft_notes"), Some(&admin), json!({"note": "Looks good overall"}), "draft").await;
    s.step("publish", POST, &format!("{mrs}/1/draft_notes/bulk_publish"), Some(&admin), json!(null), "draft").await;
    s.settle().await;
    s.step("notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;

    s.finish();
}
