//! Diff shapes only git can push (symlinks, submodules, symlink-to-file type changes) and the
//! life of MR diff versions: target-only pushes, source pushes, empty commits, no commits.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn mr_diff_git() {
    let mut s = Scenario::new("mr_diff_git").await;
    let admin = s.admin();
    let gpath = s.name("dg");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;

    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("dg"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    };
    let link = |rel: &str, target: &str| {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        std::os::unix::fs::symlink(target, path).unwrap();
    };

    // main: a file and a symlink that the branch turns into a regular file.
    w("target.txt", "t\n");
    link("type/t", "../target.txt");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Base"]).await;
    s.git(&repo, &["push", "-q", "origin", "HEAD:main"]).await;
    let main_sha = s.git(&repo, &["rev-parse", "HEAD"]).await.trim().to_string();
    s.settle().await;

    // shapes: new symlink, symlink -> file, a gitlink.
    s.git(&repo, &["checkout", "-q", "-b", "shapes"]).await;
    link("sym/link", "../target.txt");
    std::fs::remove_file(repo.join("type/t")).unwrap();
    w("type/t", "now a file\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["update-index", "--add", "--cacheinfo", &format!("160000,{main_sha},sub")]).await;
    s.git(&repo, &["commit", "-q", "-m", "Shapes"]).await;
    s.git(&repo, &["push", "-q", "origin", "shapes"]).await;
    let shapes_sha = s.git(&repo, &["rev-parse", "HEAD"]).await.trim().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("shapes mr", POST, &mrs, Some(&admin), json!({"source_branch": "shapes", "target_branch": "main", "title": "Shapes"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr1 = format!("{mrs}/1");
    s.step("shapes diffs", GET, &format!("{mr1}/diffs"), Some(&admin), json!(null), "diff").await;
    s.step("shapes changes", GET, &format!("{mr1}/changes"), Some(&admin), json!(null), "mr").await;
    s.step("shapes raw diffs", GET, &format!("{mr1}/raw_diffs"), Some(&admin), json!(null), "diff").await;
    s.step("shapes commit diff", GET, &format!("/projects/{pid}/repository/commits/{shapes_sha}/diff"), Some(&admin), json!(null), "diff").await;
    s.step("shapes compare", GET, &format!("/projects/{pid}/repository/compare"), Some(&admin), json!({"from": "main", "to": "shapes"}), "compare").await;

    // Version lifecycle.
    s.git(&repo, &["checkout", "-q", "main"]).await;
    s.git(&repo, &["checkout", "-q", "-b", "life"]).await;
    w("life.txt", "one\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Life 1"]).await;
    s.git(&repo, &["push", "-q", "origin", "life"]).await;
    s.settle().await;
    s.step("life mr", POST, &mrs, Some(&admin), json!({"source_branch": "life", "target_branch": "main", "title": "Life"}), "mr").await;
    s.poll_mr(pid, 2).await;
    let mr2 = format!("{mrs}/2");
    s.step("versions 1", GET, &format!("{mr2}/versions"), Some(&admin), json!(null), "version").await;
    s.step("target push", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "main", "commit_message": "Unrelated", "actions": [{"action": "create", "file_path": "other.txt", "content": "o\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("versions after target push", GET, &format!("{mr2}/versions"), Some(&admin), json!(null), "version").await;
    s.step("mr after target push", GET, &mr2, Some(&admin), json!(null), "mr").await;
    w("life.txt", "one\ntwo\n");
    s.git(&repo, &["commit", "-q", "-am", "Life 2"]).await;
    s.git(&repo, &["push", "-q", "origin", "life"]).await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("versions after source push", GET, &format!("{mr2}/versions"), Some(&admin), json!(null), "version").await;
    s.git(&repo, &["commit", "-q", "--allow-empty", "-m", "Empty"]).await;
    s.git(&repo, &["push", "-q", "origin", "life"]).await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    let v = s.step("versions after empty commit", GET, &format!("{mr2}/versions"), Some(&admin), json!(null), "version").await;
    if let Some(id) = v.body.as_array().and_then(|a| a.first()).and_then(|x| x["id"].as_i64()) {
        s.step("empty commit version", GET, &format!("{mr2}/versions/{id}"), Some(&admin), json!(null), "version").await;
    }
    s.step("mr after empty commit", GET, &mr2, Some(&admin), json!(null), "mr").await;

    // An MR whose source has nothing the target lacks.
    s.step("same branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "same", "ref": "main"}), "branch").await;
    s.step("same mr", POST, &mrs, Some(&admin), json!({"source_branch": "same", "target_branch": "main", "title": "Same"}), "mr").await;
    s.poll_mr(pid, 3).await;
    let mr3 = format!("{mrs}/3");
    s.step("same show", GET, &mr3, Some(&admin), json!(null), "mr").await;
    s.step("same versions", GET, &format!("{mr3}/versions"), Some(&admin), json!(null), "version").await;
    s.step("same diffs", GET, &format!("{mr3}/diffs"), Some(&admin), json!(null), "diff").await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
