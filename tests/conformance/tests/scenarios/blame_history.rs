//! Blame across a merge commit, a squash merge and a file rename.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn blame_history() {
    let mut s = Scenario::new("blame_history").await;
    let admin = s.admin();
    let gpath = s.name("bh");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let body: String = (1..=8).map(|i| format!("line {i}\n")).collect();
    s.step("base", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Base", "actions": [{"action": "create", "file_path": "f.txt", "content": body}]}), "commit").await;
    s.settle().await;
    // A feature branch edits line 2; main edits line 7 meanwhile; merged with a merge commit.
    let feature: String = body.replace("line 2\n", "line 2 feature\n");
    s.step("feature edit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Feature edit", "actions": [{"action": "update", "file_path": "f.txt", "content": feature}]}), "commit").await;
    let main_edit: String = body.replace("line 7\n", "line 7 main\n");
    s.step("main edit", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Main edit", "actions": [{"action": "update", "file_path": "f.txt", "content": main_edit}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Feature"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    let blame = |path: &str| format!("/projects/{pid}/repository/files/{path}/blame");
    s.step("blame after merge", GET, &blame("f.txt"), Some(&admin), json!({"ref": "main"}), "blame").await;
    // A squashed MR: its lines belong to the squash commit.
    let squashed: String = format!("{}line 9\nline 10\n", main_edit.replace("line 2\n", "line 2 feature\n"));
    s.step("squash edit 1", POST, &commits, Some(&admin), json!({"branch": "sq", "start_branch": "main", "commit_message": "Add 9", "actions": [{"action": "update", "file_path": "f.txt", "content": format!("{}line 9\n", main_edit.replace("line 2\n", "line 2 feature\n"))}]}), "commit").await;
    s.step("squash edit 2", POST, &commits, Some(&admin), json!({"branch": "sq", "commit_message": "Add 10", "actions": [{"action": "update", "file_path": "f.txt", "content": squashed}]}), "commit").await;
    s.settle().await;
    s.step("mr squash", POST, &mrs, Some(&admin), json!({"source_branch": "sq", "target_branch": "main", "title": "Squash", "squash": true}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("merge squash", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!({"squash": true}), "mr").await;
    s.settle().await;
    s.step("blame after squash", GET, &blame("f.txt"), Some(&admin), json!({"ref": "main"}), "blame").await;
    s.step("blame feature branch", GET, &blame("f.txt"), Some(&admin), json!({"ref": "feature"}), "blame").await;
    // A rename, then an edit of the renamed file.
    s.step("rename", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Rename", "actions": [{"action": "move", "file_path": "g.txt", "previous_path": "f.txt"}]}), "commit").await;
    s.step("edit renamed", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Edit renamed", "actions": [{"action": "update", "file_path": "g.txt", "content": squashed.replace("line 5\n", "line 5 renamed\n")}]}), "commit").await;
    s.settle().await;
    s.step("blame renamed", GET, &blame("g.txt"), Some(&admin), json!({"ref": "main"}), "blame").await;
    s.step("blame by sha range", GET, &blame("g.txt"), Some(&admin), json!({"ref": "main", "range[start]": 4, "range[end]": 6}), "blame").await;
    s.finish();
}
