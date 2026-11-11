//! More push side effects (docs/gitlab-ref/push-side-effects-audit.md): a push that aborts
//! auto-merge, the pusher's snoozed to-do, an MR merged by a push of an already merged branch
//! into its target, and long commit titles in the commits API.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn push_side_effects_2() {
    let mut s = Scenario::new("push_side_effects_2").await;
    let admin = s.admin();
    let gpath = s.name("pt");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("pt"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| std::fs::write(repo.join(rel), content).unwrap();
    let mrs = format!("/projects/{pid}/merge_requests");

    // `dev` branch, and `feature` with a long-titled commit.
    s.git(&repo, &["branch", "dev"]).await;
    s.git(&repo, &["push", "-q", "origin", "dev"]).await;
    s.git(&repo, &["checkout", "-q", "-b", "feature"]).await;
    w("f.txt", "one\n");
    s.git(&repo, &["add", "-A"]).await;
    let long = format!("Make {} the change", "very ".repeat(25));
    s.git(&repo, &["commit", "-q", "-m", &long]).await;
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    let head = s.git(&repo, &["rev-parse", "HEAD"]).await.trim().to_string();
    s.step("long commit", GET, &format!("/projects/{pid}/repository/commits/{head}"), Some(&admin), json!(null), "commit").await;
    s.step("commit list", GET, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"ref_name": "feature", "per_page": 1}), "commit").await;
    // MR 1 feature -> main, MR 2 feature -> dev.
    s.step("mr main", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "To main"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr dev", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "dev", "title": "To dev"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("mr commits", GET, &format!("{mrs}/1/commits"), Some(&admin), json!(null), "commit").await;

    // Auto-merge armed on MR 1 by a running pipeline status; then a push aborts it. The
    // admin's own snoozed to-do on MR 1 is resolved by the push.
    s.step("running status", POST, &format!("/projects/{pid}/statuses/{head}"), Some(&admin), json!({"state": "running", "name": "ci"}), "status").await;
    s.settle().await;
    s.step("arm auto-merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.step("admin todo", POST, &format!("{mrs}/1/todo"), Some(&admin), json!(null), "todo").await;
    let todos = s.send(GET, "/todos", Some(&admin), json!({"project_id": pid})).await;
    let todo_id = todos.body[0]["id"].clone();
    let until = s.date_in(3);
    s.graphql("snooze", Some(&admin), "mutation($id: TodoID!, $u: Time!) { todoSnooze(input: {id: $id, snoozeUntil: $u}) { errors todo { state snoozedUntil } } }", json!({"id": format!("gid://gitlab/Todo/{todo_id}"), "u": format!("{until}T10:00:00Z")})).await;
    w("f.txt", "two\n");
    s.git(&repo, &["commit", "-q", "-am", "More"]).await;
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1 after push", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.graphql("snoozed todo after push", Some(&admin), "query($p: [ID!]) { currentUser { todos(state: [done], type: [MERGEREQUEST], projectId: $p) { nodes { action state snoozedUntil } } } }", json!({"p": [pid.to_string()]})).await;

    // MR 1 is merged through the API; pushing main into dev then merges MR 2.
    s.step("merge 1", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"should_remove_source_branch": false}), "mr").await;
    s.settle().await;
    s.git(&repo, &["fetch", "-q", "origin"]).await;
    s.git(&repo, &["push", "-q", "origin", "origin/main:dev"]).await;
    s.settle().await;
    s.step("mr 2 after push", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 state events", GET, &format!("{mrs}/2/resource_state_events"), Some(&admin), json!(null), "event").await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
