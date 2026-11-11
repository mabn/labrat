//! Pushes to an MR's target branch (docs/gitlab-ref/push-side-effects-audit.md item 7): a
//! plain push that doesn't touch the MR's commits, then a force push that rewrites the target's
//! history. Diff versions, discussion positions, outdated notes and suggestions afterwards.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn target_force_push() {
    let mut s = Scenario::new("target_force_push").await;
    let admin = s.admin();
    let gpath = s.name("tf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    s.step("unprotect main", DELETE, &format!("/projects/{pid}/protected_branches/main"), Some(&admin), json!(null), "branch").await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("tf"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| std::fs::write(repo.join(rel), content).unwrap();
    let mrs = format!("/projects/{pid}/merge_requests");
    // main: shared.txt; feature edits line 2 of it.
    w("shared.txt", "a\nb\nc\nd\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Shared"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    s.git(&repo, &["checkout", "-q", "-b", "feature"]).await;
    w("shared.txt", "a\nB\nc\nd\n");
    w("f.txt", "f1\nf2\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Feature"]).await;
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Feature"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let m = s.send(GET, &mr, Some(&admin), json!(null)).await;
    let refs = m.body["diff_refs"].clone();
    let pos = |path: &str, line: i64| json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": path, "new_path": path, "new_line": line});
    s.step("thread on shared", POST, &format!("{mr}/discussions"), Some(&admin), json!({"body": "On B", "position": pos("shared.txt", 2)}), "discussion").await;
    s.step("suggestion on f", POST, &format!("{mr}/discussions"), Some(&admin), json!({"body": "```suggestion:-0+0\nF1\n```", "position": pos("f.txt", 1)}), "discussion").await;
    // A plain target push that doesn't touch the MR's commits.
    s.git(&repo, &["checkout", "-q", "main"]).await;
    w("other.txt", "o\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Other"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("versions after plain target push", GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
    s.step("mr after plain target push", GET, &mr, Some(&admin), json!(null), "mr").await;
    // A force push that rewrites main: shared.txt line 1 changes, `Other` is gone.
    s.git(&repo, &["reset", "-q", "--hard", "HEAD~2"]).await;
    w("shared.txt", "A\nb\nc\nd\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Shared rewritten"]).await;
    s.git(&repo, &["push", "-q", "--force", "origin", "main"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("versions after force push", GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
    s.step("mr after force push", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("discussions after force push", GET, &format!("{mr}/discussions"), Some(&admin), json!(null), "discussion").await;
    s.step("notes after force push", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("changes after force push", GET, &format!("{mr}/changes"), Some(&admin), json!(null), "mr").await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
