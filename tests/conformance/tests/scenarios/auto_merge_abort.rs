//! In a fast-forward project, moving the target branch under an auto-merge MR aborts the
//! auto-merge and leaves the author an `unmergeable` to-do.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn auto_merge_abort() {
    let mut s = Scenario::new("auto_merge_abort").await;
    let admin = s.admin();
    let gpath = s.name("aa");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "merge_method": "ff"}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    let sha = c.get("id").as_str().unwrap().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("running", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "running", "name": "ci"}), "status").await;
    s.settle().await;
    s.step("auto merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.step("main moves", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Other", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr after", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("todos", GET, "/todos", Some(&admin), json!({"project_id": pid}), "todo").await;
    s.finish();
}
