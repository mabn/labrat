//! Commit statuses, external pipelines and their effect on merge requests.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn commit_statuses() {
    let mut s = Scenario::new("commit_statuses").await;
    let admin = s.admin();
    let gpath = s.name("ci");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("rep");
    let rep = s
        .step("reporter", POST, "/users", Some(&admin), json!({"username": username, "name": "Rep", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let rep_t = s.step("reporter token", POST, &format!("/users/{rep}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("reporter member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": rep, "access_level": 20}), "user").await;
    let commits = format!("/projects/{pid}/repository/commits");
    let c1 = s.step("feature commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    let sha = c1.get("id").as_str().unwrap().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let statuses = format!("/projects/{pid}/statuses/{sha}");

    // Errors first.
    s.step("missing state", POST, &statuses, Some(&admin), json!({}), "status").await;
    s.step("bad state", POST, &statuses, Some(&admin), json!({"state": "done"}), "status").await;
    s.step("unknown commit", POST, &format!("/projects/{pid}/statuses/0000000000000000000000000000000000000000"), Some(&admin), json!({"state": "pending"}), "status").await;
    s.step("reporter cannot post", POST, &statuses, Some(&rep_t), json!({"state": "pending"}), "status").await;
    s.step("unknown pipeline", POST, &statuses, Some(&admin), json!({"state": "pending", "pipeline_id": 999999}), "status").await;

    // pending -> running -> success on the MR head.
    s.step("pending", POST, &statuses, Some(&admin), json!({"state": "pending", "description": "queued", "target_url": "https://ci.example.com/1"}), "status").await;
    s.settle().await;
    s.step("mr with pending pipeline", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("pending again", POST, &statuses, Some(&admin), json!({"state": "pending"}), "status").await;
    s.step("running", POST, &statuses, Some(&admin), json!({"state": "running", "coverage": 50.5}), "status").await;
    s.step("running again", POST, &statuses, Some(&admin), json!({"state": "running"}), "status").await;
    s.step("skip running", POST, &statuses, Some(&admin), json!({"state": "skipped"}), "status").await;
    s.settle().await;
    s.step("mr with running pipeline", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("lint failed", POST, &statuses, Some(&admin), json!({"state": "failed", "name": "lint", "ref": "feature"}), "status").await;
    s.step("success", POST, &statuses, Some(&admin), json!({"state": "success"}), "status").await;
    s.settle().await;
    s.step("statuses", GET, &format!("{commits}/{sha}/statuses"), Some(&admin), json!(null), "status").await;
    s.step("pipelines", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    let pls = s.send(GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null)).await;
    let plid = pls.body[0]["id"].as_i64().unwrap_or(0);
    s.step("pipeline", GET, &format!("/projects/{pid}/pipelines/{plid}"), Some(&admin), json!(null), "pipeline").await;
    s.step("mr pipelines", GET, &format!("{mrs}/1/pipelines"), Some(&admin), json!(null), "pipeline").await;

    // Pipelines must succeed: a failed lint job blocks merging.
    s.step("require pipelines", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_pipeline_succeeds": true}), "project").await;
    s.step("mr ci must pass", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("merge blocked", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.step("lint fixed", POST, &statuses, Some(&admin), json!({"state": "success", "name": "lint", "ref": "feature"}), "status").await;
    s.settle().await;
    s.step("statuses all", GET, &format!("{commits}/{sha}/statuses"), Some(&admin), json!({"all": true}), "status").await;
    s.step("mr mergeable", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;

    // Auto-merge waits for a running pipeline, then merges.
    s.step("next commit", POST, &commits, Some(&admin), json!({"branch": "feature", "commit_message": "Add b", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    let head = s.send(GET, &format!("{mrs}/1"), Some(&admin), json!(null)).await;
    let sha2 = head.body["sha"].as_str().unwrap_or_default().to_string();
    s.step("mr after push", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("running on new head", POST, &format!("/projects/{pid}/statuses/{sha2}"), Some(&admin), json!({"state": "running", "ref": "feature"}), "status").await;
    s.settle().await;
    s.step("auto merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.step("new head success", POST, &format!("/projects/{pid}/statuses/{sha2}"), Some(&admin), json!({"state": "success", "ref": "feature"}), "status").await;
    s.settle().await;
    s.step("merged by auto merge", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}
