//! Action parameters from docs/gitlab-ref/mr-endpoint-params-audit.md: merge and squash commit
//! messages, rebase `skip_ci`, async pipeline creation, replies
//! with `internal`, spent-time summaries, `include_diverged_commits_count`
//! and `render_html`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn mr_action_params() {
    let mut s = Scenario::new("mr_action_params").await;
    let admin = s.admin();
    let gpath = s.name("ap");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for b in ["a", "b", "c"] {
        s.step("commit 1", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("{b} one"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "1\n"}]}), "commit").await;
        // GitLab may not see a branch created a moment ago (its branch cache): wait.
        s.settle().await;
        s.step("commit 2", POST, &commits, Some(&admin), json!({"branch": b, "commit_message": format!("{b} two"), "actions": [{"action": "update", "file_path": format!("{b}.txt"), "content": "2\n"}]}), "commit").await;
    }
    s.settle().await;
    for (i, b) in ["a", "b", "c"].iter().enumerate() {
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    // `main` moves on, so MR 3 is behind for the diverged count and the rebase.
    s.step("main moves", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "main moves", "actions": [{"action": "create", "file_path": "m.txt", "content": "m\n"}]}), "commit").await;
    s.settle().await;
    s.step("diverged count", GET, &format!("{mrs}/3"), Some(&admin), json!({"include_diverged_commits_count": true}), "mr").await;
    s.step("render_html", GET, &format!("{mrs}/3"), Some(&admin), json!({"render_html": true}), "mr").await;
    s.step("merge with message", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"merge_commit_message": "Custom merge message\n\nWith a body"}), "mr").await;
    s.settle().await;
    s.step("squash merge with messages", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!({"squash": true, "squash_commit_message": "Squashed!", "merge_commit_message": "Merging squashed"}), "mr").await;
    s.settle().await;
    s.step("main log", GET, &commits, Some(&admin), json!({"ref_name": "main", "per_page": 6}), "commit").await;
    s.step("rebase skip_ci", PUT, &format!("{mrs}/3/rebase"), Some(&admin), json!({"skip_ci": true}), "mr").await;
    s.settle().await;
    s.step("mr 3 after rebase", GET, &format!("{mrs}/3"), Some(&admin), json!({"include_rebase_in_progress": true}), "mr").await;
    s.step("pipeline async", POST, &format!("{mrs}/3/pipelines"), Some(&admin), json!({"async": true}), "pipeline").await;
    s.step("pipeline sync", POST, &format!("{mrs}/3/pipelines"), Some(&admin), json!(null), "pipeline").await;
    let thread = s.step("thread", POST, &format!("{mrs}/3/discussions"), Some(&admin), json!({"body": "Thread"}), "discussion").await;
    let tid = thread.body["id"].as_str().unwrap_or_default().to_string();
    s.step("reply internal", POST, &format!("{mrs}/3/discussions/{tid}/notes"), Some(&admin), json!({"body": "Reply", "internal": true}), "note").await;
    s.step("spend with summary", POST, &format!("{mrs}/3/add_spent_time"), Some(&admin), json!({"duration": "1h", "summary": "Reviewing"}), "time").await;
    s.graphql("timelogs", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"3\") { timelogs { nodes { timeSpent summary } } } } }", json!({"p": format!("{gpath}/app")})).await;
    // Arrays go into the query string as repeated `[]` params.
    s.finish();
}
