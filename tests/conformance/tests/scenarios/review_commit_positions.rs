//! Review comments on a single commit of a merge request and on an older diff version: the
//! position given, the stored `commit_id`, and how they read back after another push.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn review_commit_positions() {
    let mut s = Scenario::new("review_commit_positions").await;
    let admin = s.admin();
    let gpath = s.name("cp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let c1 = s.step("commit 1", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\nthree\n"}]}), "commit").await;
    let c1 = c1.get("id").as_str().unwrap().to_string();
    let c1_parent = s.send(GET, &format!("{commits}/{c1}"), Some(&admin), json!(null)).await.body["parent_ids"][0].as_str().unwrap_or_default().to_string();
    let c2 = s.step("commit 2", POST, &commits, Some(&admin), json!({"branch": "feature", "commit_message": "Change a", "actions": [{"action": "update", "file_path": "a.txt", "content": "one\nTWO\nthree\nfour\n"}]}), "commit").await;
    let c2 = c2.get("id").as_str().unwrap().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let d = format!("{mr}/discussions");
    let v1 = s.step("versions", GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
    let v1 = v1.body[0].clone();
    // A comment on the first commit only (its parent to itself).
    s.step(
        "on commit 1",
        POST,
        &d,
        Some(&admin),
        json!({"body": "On commit 1", "position": {"base_sha": c1_parent, "start_sha": c1_parent, "head_sha": c1, "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "new_line": 2}}),
        "discussion",
    )
    .await;
    // On the second commit's own change.
    s.step(
        "on commit 2",
        POST,
        &d,
        Some(&admin),
        json!({"body": "On commit 2", "position": {"base_sha": c1, "start_sha": c1, "head_sha": c2, "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "old_line": 2, "new_line": null}}),
        "discussion",
    )
    .await;
    // `commit_id` on a plain comment ties it to a commit.
    s.step("commit_id comment", POST, &d, Some(&admin), json!({"body": "About commit 1", "commit_id": c1}), "discussion").await;
    // On the latest version, then push again so it becomes an older version.
    s.step(
        "on version 1",
        POST,
        &d,
        Some(&admin),
        json!({"body": "On version 1", "position": {"base_sha": v1["base_commit_sha"], "start_sha": v1["start_commit_sha"], "head_sha": v1["head_commit_sha"], "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "new_line": 4}}),
        "discussion",
    )
    .await;
    s.step("commit 3", POST, &commits, Some(&admin), json!({"branch": "feature", "commit_message": "Change a again", "actions": [{"action": "update", "file_path": "a.txt", "content": "zero\none\nTWO\nthree\nfour\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("discussions", GET, &d, Some(&admin), json!(null), "discussion").await;
    s.step("notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("commit 1 comments", GET, &format!("{commits}/{c1}/discussions"), Some(&admin), json!(null), "discussion").await;
    s.graphql(
        "graphql positions",
        Some(&admin),
        "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { discussions { nodes { resolvable notes { nodes { body position { headSha baseSha startSha newLine oldLine filePath diffRefs { headSha } } } } } } } } }",
        json!({"p": full}),
    )
    .await;
    s.finish();
}
