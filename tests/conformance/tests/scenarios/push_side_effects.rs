//! Side effects of git pushes to MR branches (docs/gitlab-ref/push-side-effects-audit.md):
//! Draft from commit titles, the pusher's to-dos, long and empty commit titles, many commits
//! in one push, the unmergeable to-do on conflicts, approvals and reviewer states kept,
//! and deleting and re-pushing a source branch.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn push_side_effects() {
    let mut s = Scenario::new("push_side_effects").await;
    let admin = s.admin();
    let gpath = s.name("ps");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("ps"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| std::fs::write(repo.join(rel), content).unwrap();
    let mrs = format!("/projects/{pid}/merge_requests");

    // MR 1 on `feature`, with bob as reviewer.
    s.git(&repo, &["checkout", "-q", "-b", "feature"]).await;
    w("f.txt", "one\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Add f"]).await;
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Feature", "description": format!("Ping @{username}"), "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");

    // Bob approves and requests changes; admin has to-dos of its own on the MR.
    s.step("bob approves", POST, &format!("{mr}/approve"), Some(&bob_t), json!(null), "approval").await;
    // The approval note is written in the background; let it land before the next action.
    s.settle().await;
    s.graphql("bob requests changes", Some(&bob_t), "mutation($p: ID!) { mergeRequestRequestChanges(input: {projectPath: $p, iid: \"1\"}) { errors } }", json!({"p": full})).await;
    s.step("admin marks todo", POST, &format!("{mr}/todo"), Some(&admin), json!(null), "todo").await;
    let me = s.send(GET, "/user", Some(&admin), json!(null)).await.body["username"].as_str().unwrap_or_default().to_string();
    s.step("bob mentions admin", POST, &format!("{mr}/notes"), Some(&bob_t), json!({"body": format!("@{me} have a look")}), "note").await;
    s.settle().await;
    s.step("admin todos before", GET, "/todos", Some(&admin), json!({"project_id": pid}), "todo").await;
    s.step("bob todos before", GET, "/todos", Some(&bob_t), json!({"project_id": pid}), "todo").await;

    // One push: a long title, an empty message, a fixup and a Draft commit.
    w("f.txt", "two\n");
    s.git(&repo, &["commit", "-q", "-am", &format!("Long {}", "x".repeat(115))]).await;
    w("f.txt", "three\n");
    s.git(&repo, &["commit", "-q", "-a", "--allow-empty-message", "-m", ""]).await;
    w("f.txt", "four\n");
    s.git(&repo, &["commit", "-q", "-am", "fixup! Add f"]).await;
    w("f.txt", "five\n");
    s.git(&repo, &["commit", "-q", "-am", "Draft: wip"]).await;
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr after push", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("notes after push", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("admin todos after push", GET, "/todos", Some(&admin), json!({"project_id": pid, "state": "done"}), "todo").await;
    s.step("bob todos after push", GET, "/todos", Some(&bob_t), json!({"project_id": pid}), "todo").await;
    s.step("approvals after push", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("reviewers after push", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "user").await;

    // Twelve commits in one push.
    s.step("ready", PUT, &mr, Some(&admin), json!({"title": "Feature"}), "mr").await;
    for i in 0..12 {
        w("f.txt", &format!("n{i}\n"));
        s.git(&repo, &["commit", "-q", "-am", &format!("Step {i}")]).await;
    }
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("notes after twelve", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("mr after twelve", GET, &mr, Some(&admin), json!(null), "mr").await;

    // A conflict with main: the author's unmergeable to-do.
    s.git(&repo, &["checkout", "-q", "main"]).await;
    w("f.txt", "main side\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Main f"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    s.settle().await;
    s.step("recheck", GET, &mr, Some(&admin), json!({"with_merge_status_recheck": true}), "mr").await;
    s.settle().await;
    s.step("mr conflicted", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("admin pending todos", GET, "/todos", Some(&admin), json!({"project_id": pid}), "todo").await;

    // Deleting the source branch over git closes the MR; pushing it again keeps it closed.
    s.git(&repo, &["push", "-q", "origin", ":feature"]).await;
    s.settle().await;
    s.step("mr after delete", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    s.step("mr after re-push", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("versions after re-push", GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
    s.step("reopen", PUT, &mr, Some(&admin), json!({"state_event": "reopen"}), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("versions after reopen", GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
    s.step("final notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
