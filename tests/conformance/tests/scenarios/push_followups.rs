//! Git push follow-ups from the MR services audit: merge requests created by push options from
//! several commits, draft options on existing MRs, an MR for a branch without new commits, and
//! a merge request merged by pushing a merge commit to the target branch.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn push_followups() {
    let mut s = Scenario::new("push_followups").await;
    let admin = s.admin();
    let gpath = s.name("pu");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("pu"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| std::fs::write(repo.join(rel), content).unwrap();
    let mrs = format!("/projects/{pid}/merge_requests");

    // Two commits, the first with a multi-line message.
    s.git(&repo, &["checkout", "-q", "-b", "feature-two-commits"]).await;
    w("a.txt", "a\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "First change\n\nWhy the first change."]).await;
    w("b.txt", "b\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Second change"]).await;
    s.git(&repo, &["push", "-q", "-o", "merge_request.create", "origin", "feature-two-commits"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr from two commits", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;

    // One commit with a multi-line message.
    s.git(&repo, &["checkout", "-q", "main"]).await;
    s.git(&repo, &["checkout", "-q", "-b", "single"]).await;
    w("c.txt", "c\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Single change\n\nWith a body."]).await;
    s.git(&repo, &["push", "-q", "-o", "merge_request.create", "-o", "merge_request.draft", "origin", "single"]).await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("draft mr", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    // draft=false on an existing draft MR.
    w("c.txt", "c2\n");
    s.git(&repo, &["commit", "-q", "-am", "More"]).await;
    s.git(&repo, &["push", "-q", "-o", "merge_request.draft=false", "origin", "single"]).await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("after draft=false", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;

    // A branch without new commits.
    s.git(&repo, &["checkout", "-q", "main"]).await;
    s.git(&repo, &["push", "-q", "-o", "merge_request.create", "origin", "main:empty-branch"]).await;
    s.settle().await;
    s.step("empty branch mrs", GET, &mrs, Some(&admin), json!({"source_branch": "empty-branch", "state": "all"}), "mr").await;

    // Merge by pushing a merge commit to main.
    s.git(&repo, &["merge", "-q", "--no-ff", "-m", "Merge feature by hand", "feature-two-commits"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("merged by push", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("merged notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
