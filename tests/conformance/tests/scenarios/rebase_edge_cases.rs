//! Rebases and suggestions on unusual histories (docs/gitlab-ref/labrat-shortcuts-audit.md items 10
//! and 17): a source branch that merged the target in, commits whose dates run backwards, and a
//! suggestion applied to a Latin-1 file.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn rebase_edge_cases() {
    let mut s = Scenario::new("rebase_edge_cases").await;
    let admin = s.admin();
    let gpath = s.name("re");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("re"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &[u8]| std::fs::write(repo.join(rel), content).unwrap();
    let mrs = format!("/projects/{pid}/merge_requests");
    // A Latin-1 file on main.
    w("latin1.txt", b"caf\xe9\nna\xefve\nend\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Latin-1"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    // `merged`: a commit, main merged in, another commit.
    s.git(&repo, &["checkout", "-q", "-b", "merged"]).await;
    w("m1.txt", b"1\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "M1"]).await;
    s.git(&repo, &["checkout", "-q", "main"]).await;
    w("main1.txt", b"x\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Main 1"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    s.git(&repo, &["checkout", "-q", "merged"]).await;
    s.git(&repo, &["merge", "-q", "--no-edit", "--no-ff", "main"]).await;
    w("m2.txt", b"2\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "M2"]).await;
    s.git(&repo, &["push", "-q", "origin", "merged"]).await;
    // `skewed`: three commits, the later ones dated earlier.
    s.git(&repo, &["checkout", "-q", "main"]).await;
    s.git(&repo, &["checkout", "-q", "-b", "skewed"]).await;
    for (i, date) in [(1, "2026-01-03T00:00:00Z"), (2, "2026-01-02T00:00:00Z"), (3, "2026-01-01T00:00:00Z")] {
        w(&format!("s{i}.txt"), b"s\n");
        s.git(&repo, &["add", "-A"]).await;
        s.git(&repo, &["-c", &format!("user.name=Skew"), "commit", "-q", "-m", &format!("S{i}"), "--date", date]).await;
    }
    s.git(&repo, &["push", "-q", "origin", "skewed"]).await;
    // main moves on.
    s.git(&repo, &["checkout", "-q", "main"]).await;
    w("main2.txt", b"y\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Main 2"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    s.settle().await;
    for (iid, b) in [(1, "merged"), (2, "skewed")] {
        s.step(&format!("mr {b}"), POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": b}), "mr").await;
        s.poll_mr(pid, iid).await;
        s.step(&format!("rebase {b}"), PUT, &format!("{mrs}/{iid}/rebase"), Some(&admin), json!(null), "mr").await;
        s.settle().await;
        s.poll_mr(pid, iid).await;
        s.step(&format!("mr {b} after rebase"), GET, &format!("{mrs}/{iid}"), Some(&admin), json!({"include_rebase_in_progress": true}), "mr").await;
        s.step(&format!("{b} log"), GET, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"ref_name": b, "per_page": 8}), "commit").await;
    }
    // A suggestion on the Latin-1 file (through an MR that touches it).
    s.git(&repo, &["fetch", "-q", "origin"]).await;
    s.git(&repo, &["checkout", "-q", "-b", "latin", "origin/main"]).await;
    w("latin1.txt", b"caf\xe9\nna\xefve\nend\nmore\n");
    s.git(&repo, &["commit", "-q", "-am", "Latin more"]).await;
    s.git(&repo, &["push", "-q", "origin", "latin"]).await;
    s.settle().await;
    s.step("mr latin", POST, &mrs, Some(&admin), json!({"source_branch": "latin", "target_branch": "main", "title": "Latin"}), "mr").await;
    s.poll_mr(pid, 3).await;
    let m = s.send(GET, &format!("{mrs}/3"), Some(&admin), json!(null)).await;
    let refs = m.body["diff_refs"].clone();
    let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": "latin1.txt", "new_path": "latin1.txt", "new_line": 4});
    let d = s.step("suggestion latin", POST, &format!("{mrs}/3/discussions"), Some(&admin), json!({"body": "```suggestion:-0+0\nMORE\n```", "position": pos}), "discussion").await;
    let did = d.body["id"].as_str().unwrap_or_default().to_string();
    let full_d = s.send(GET, &format!("{mrs}/3/discussions/{did}"), Some(&admin), json!(null)).await;
    let sid = full_d.body["notes"][0]["suggestions"][0]["id"].clone();
    s.step("apply latin", PUT, &format!("/suggestions/{sid}/apply"), Some(&admin), json!(null), "suggestion").await;
    s.settle().await;
    s.step("latin file after", GET, &format!("/projects/{pid}/repository/files/latin1.txt"), Some(&admin), json!({"ref": "latin"}), "file").await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
