//! Discussions on commits (`/repository/commits/:sha/discussions`) and how commit comments
//! appear on the merge requests that contain the commit.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn commit_discussions() {
    let mut s = Scenario::new("commit_discussions").await;
    let admin = s.admin();
    let gpath = s.name("cd");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\n"}]}), "commit").await;
    let sha = c.get("id").as_str().unwrap().to_string();
    let parent = c.get("parent_ids")[0].as_str().unwrap().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;

    let base = format!("{commits}/{sha}/discussions");
    let d1 = s.step("create discussion", POST, &base, Some(&admin), json!({"body": "On the commit"}), "discussion").await;
    let d1_id = d1.get("id").as_str().unwrap_or_default().to_string();
    let pos = json!({"base_sha": parent, "start_sha": parent, "head_sha": sha, "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "new_line": 2});
    let d2 = s.step("create diff discussion", POST, &base, Some(&admin), json!({"body": "On line 2", "position": pos}), "discussion").await;
    let d2_id = d2.get("id").as_str().unwrap_or_default().to_string();
    s.step("legacy comment", POST, &format!("{commits}/{sha}/comments"), Some(&admin), json!({"note": "Old style"}), "note").await;
    s.step("bad position", POST, &base, Some(&admin), json!({"body": "x", "position": {"base_sha": parent, "start_sha": parent, "head_sha": sha, "position_type": "text", "new_path": "missing.txt", "new_line": 1}}), "discussion").await;
    s.step("missing body", POST, &base, Some(&admin), json!({}), "discussion").await;
    s.step("unknown commit", POST, &format!("{commits}/0000000000000000000000000000000000000000/discussions"), Some(&admin), json!({"body": "x"}), "discussion").await;
    s.settle().await;
    s.step("list", GET, &base, Some(&admin), json!(null), "discussion").await;
    s.step("show", GET, &format!("{base}/{d1_id}"), Some(&admin), json!(null), "discussion").await;
    let reply = s.step("reply", POST, &format!("{base}/{d1_id}/notes"), Some(&admin), json!({"body": "A reply"}), "note").await;
    let reply_id = reply.id();
    s.step("reply to diff discussion", POST, &format!("{base}/{d2_id}/notes"), Some(&admin), json!({"body": "Also"}), "note").await;
    s.step("discussion notes", GET, &format!("{base}/{d1_id}/notes"), Some(&admin), json!(null), "note").await;
    s.step("update note", PUT, &format!("{base}/{d1_id}/notes/{reply_id}"), Some(&admin), json!({"body": "Edited"}), "note").await;
    s.step("resolve on commit", PUT, &format!("{base}/{d2_id}"), Some(&admin), json!({"resolved": true}), "discussion").await;
    s.step("delete note", DELETE, &format!("{base}/{d1_id}/notes/{reply_id}"), Some(&admin), json!(null), "note").await;
    s.step("missing discussion", GET, &format!("{base}/deadbeef"), Some(&admin), json!(null), "discussion").await;
    s.settle().await;
    s.step("list after", GET, &base, Some(&admin), json!(null), "discussion").await;
    s.step("commit comments", GET, &format!("{commits}/{sha}/comments"), Some(&admin), json!(null), "note").await;
    s.step("mr discussions", GET, &format!("{mrs}/1/discussions"), Some(&admin), json!(null), "discussion").await;
    s.step("mr notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!(null), "note").await;
    s.step("mr", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
