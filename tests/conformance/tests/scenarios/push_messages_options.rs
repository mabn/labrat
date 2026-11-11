//! What git pushes print and do (docs/gitlab-ref/mr-feature-inventory.md): the "create a merge
//! request" / "view merge request" messages with `printing_merge_request_link_enabled` on and off,
//! and the MR push options (`target`, `title`, `description`, `label`, `unlabel`, `assign`,
//! `unassign`, `milestone`, `remove_source_branch`, `squash`, the `mr.` alias, bad values).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn push_messages_options() {
    let mut s = Scenario::new("push_messages_options").await;
    let admin = s.admin();
    let gpath = s.name("pm");
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
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await;
    s.step("dev branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "dev", "ref": "main"}), "branch").await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("pm"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| std::fs::write(repo.join(rel), content).unwrap();
    let mrs = format!("/projects/{pid}/merge_requests");
    s.git(&repo, &["checkout", "-q", "-b", "one"]).await;
    w("one.txt", "1\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "One"]).await;
    s.git_remote_lines("push new branch", &repo, &["push", "origin", "one"]).await;
    s.settle().await;
    s.git(&repo, &["checkout", "-q", "main"]).await;
    w("main.txt", "m\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Main"]).await;
    s.git_remote_lines("push default branch", &repo, &["push", "origin", "main"]).await;
    s.settle().await;
    // Every MR option at once.
    s.git(&repo, &["checkout", "-q", "-b", "opts"]).await;
    w("opts.txt", "o\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Opts"]).await;
    let assign = format!("merge_request.assign={username}");
    s.git_remote_lines(
        "push all options",
        &repo,
        &[
            "push", "-o", "merge_request.create", "-o", "merge_request.target=dev", "-o", "merge_request.title=Pushed title",
            "-o", "merge_request.description=Pushed description", "-o", "merge_request.label=one", "-o", "merge_request.label=two",
            "-o", &assign, "-o", "merge_request.milestone=v1", "-o", "merge_request.remove_source_branch", "-o", "merge_request.squash",
            "origin", "opts",
        ],
    )
    .await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr from options", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    // Update the same MR, one option per push (the `mr.` alias for one of them).
    let unassign = format!("merge_request.unassign={username}");
    for (i, opt) in ["merge_request.unlabel=one", unassign.as_str(), "mr.title=Retitled", "merge_request.target=main", "merge_request.label=three"].iter().enumerate() {
        w("opts.txt", &format!("o{i}\n"));
        s.git(&repo, &["commit", "-q", "-am", &format!("Opts {i}")]).await;
        s.git_remote_lines(&format!("push update {opt}"), &repo, &["push", "-o", opt, "origin", "opts"]).await;
        s.settle().await;
        s.poll_mr(pid, 1).await;
    }
    s.step("mr after update options", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    // Bad values.
    s.git(&repo, &["checkout", "-q", "-b", "bad"]).await;
    w("bad.txt", "b\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Bad"]).await;
    s.git_remote_lines("push bad target", &repo, &["push", "-o", "merge_request.create", "-o", "merge_request.target=nope", "origin", "bad"]).await;
    s.settle().await;
    w("bad.txt", "b2\n");
    s.git(&repo, &["commit", "-q", "-am", "Bad 2"]).await;
    s.git_remote_lines("push bad assignee", &repo, &["push", "-o", "merge_request.create", "-o", "merge_request.assign=no-such-user-x", "origin", "bad"]).await;
    s.settle().await;
    s.step("mrs after bad", GET, &mrs, Some(&admin), json!({"state": "all"}), "mr").await;
    // Existing MR, then the link setting off.
    s.git(&repo, &["checkout", "-q", "one"]).await;
    w("one.txt", "1c\n");
    s.git(&repo, &["commit", "-q", "-am", "One again"]).await;
    s.step("mr for one", POST, &mrs, Some(&admin), json!({"source_branch": "one", "target_branch": "main", "title": "One"}), "mr").await;
    s.settle().await;
    s.git_remote_lines("push with existing mr", &repo, &["push", "origin", "one"]).await;
    s.settle().await;
    s.step("links off", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"printing_merge_request_link_enabled": false}), "project").await;
    w("one.txt", "1d\n");
    s.git(&repo, &["commit", "-q", "-am", "One more"]).await;
    s.git_remote_lines("push links off", &repo, &["push", "origin", "one"]).await;
    s.settle().await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
