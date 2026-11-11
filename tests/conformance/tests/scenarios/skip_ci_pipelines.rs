//! Skipped pipelines in a project without CI configuration: `[skip ci]` / `[ci skip]` commit
//! messages (git push and commits API), the `ci.skip` push option, MRs whose head skips CI,
//! rebase with `skip_ci`, merge commits that skip CI, and what MRs and merge checks show.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn skip_ci_pipelines() {
    let mut s = Scenario::new("skip_ci_pipelines").await;
    let admin = s.admin();
    let gpath = s.name("sk");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("sk"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| std::fs::write(repo.join(rel), content).unwrap();
    let pipelines = format!("/projects/{pid}/pipelines");
    let mrs = format!("/projects/{pid}/merge_requests");
    let commits = format!("/projects/{pid}/repository/commits");

    // A plain push creates nothing; a `[skip ci]` push creates a skipped branch pipeline.
    s.git(&repo, &["checkout", "-q", "-b", "plain"]).await;
    w("p.txt", "p\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Plain"]).await;
    s.git(&repo, &["push", "-q", "origin", "plain"]).await;
    s.settle().await;
    s.step("after plain push", GET, &pipelines, Some(&admin), json!(null), "pipeline").await;
    s.git(&repo, &["checkout", "-q", "-b", "skip"]).await;
    w("s.txt", "s\n");
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Change [skip ci]"]).await;
    s.git(&repo, &["push", "-q", "origin", "skip"]).await;
    s.settle().await;
    s.step("after skip push", GET, &pipelines, Some(&admin), json!(null), "pipeline").await;
    // An MR whose head skips CI, then a `[ci skip]` push to it, then a `ci.skip` push option.
    s.step("mr skip", POST, &mrs, Some(&admin), json!({"source_branch": "skip", "target_branch": "main", "title": "Skip"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 pipelines", GET, &format!("{mrs}/1/pipelines"), Some(&admin), json!(null), "pipeline").await;
    w("s.txt", "s2\n");
    s.git(&repo, &["commit", "-q", "-am", "Second [CI SKIP] change"]).await;
    s.git(&repo, &["push", "-q", "origin", "skip"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1 after ci skip push", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    w("s.txt", "s3\n");
    s.git(&repo, &["commit", "-q", "-am", "Third change"]).await;
    s.git(&repo, &["push", "-q", "-o", "ci.skip", "origin", "skip"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1 after push option", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 pipelines after", GET, &format!("{mrs}/1/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("all pipelines", GET, &pipelines, Some(&admin), json!(null), "pipeline").await;
    // A plain push to the MR after skipped ones.
    w("s.txt", "s4\n");
    s.git(&repo, &["commit", "-q", "-am", "Fourth change"]).await;
    s.git(&repo, &["push", "-q", "origin", "skip"]).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1 after plain push", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    // Commits API with the marker.
    s.step("api commit skip", POST, &commits, Some(&admin), json!({"branch": "api", "start_branch": "main", "commit_message": "API [skip ci]", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    s.step("pipelines after api", GET, &pipelines, Some(&admin), json!(null), "pipeline").await;
    // Merge checks with a skipped head pipeline.
    s.step("mr api", POST, &mrs, Some(&admin), json!({"source_branch": "api", "target_branch": "main", "title": "API"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("require pipelines", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_pipeline_succeeds": true}), "project").await;
    s.step("mr 2 required", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("allow skipped", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"allow_merge_on_skipped_pipeline": true}), "project").await;
    s.step("mr 2 skipped allowed", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("relax", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_pipeline_succeeds": false, "allow_merge_on_skipped_pipeline": false}), "project").await;
    // Rebase with skip_ci.
    s.step("main moves", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Main moves", "actions": [{"action": "create", "file_path": "m.txt", "content": "m\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr plain", POST, &mrs, Some(&admin), json!({"source_branch": "plain", "target_branch": "main", "title": "Plain"}), "mr").await;
    s.poll_mr(pid, 3).await;
    s.step("rebase skip_ci", PUT, &format!("{mrs}/3/rebase"), Some(&admin), json!({"skip_ci": true}), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 3).await;
    s.step("mr 3 after rebase", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.step("mr 3 pipelines", GET, &format!("{mrs}/3/pipelines"), Some(&admin), json!(null), "pipeline").await;
    // A merge whose merge commit skips CI.
    s.step("merge with skip", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!({"merge_commit_message": "Merge it [skip ci]"}), "mr").await;
    s.settle().await;
    s.step("final pipelines", GET, &pipelines, Some(&admin), json!({"per_page": 50}), "pipeline").await;
    let first = s.send(GET, &pipelines, Some(&admin), json!({"per_page": 1})).await;
    let id = first.body[0]["id"].clone();
    s.step("pipeline detail", GET, &format!("{pipelines}/{id}"), Some(&admin), json!(null), "pipeline").await;
    s.graphql("gql mr pipelines", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { headPipeline { iid status source ref sha active } pipelines { count nodes { iid status source ref } } } } }", json!({"p": full})).await;
    s.graphql("gql project pipelines", Some(&admin), "query($p: ID!) { project(fullPath: $p) { pipelines(source: \"merge_request_event\") { count nodes { iid ref } } skipped: pipelines(status: SKIPPED, ref: \"skip\") { count } } }", json!({"p": full})).await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
