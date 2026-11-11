//! Probes from docs/gitlab-ref/labrat-shortcuts-audit.md: a merge across a rename, a diff thread
//! whose file is renamed, `(fixup!|squash!)` anywhere in a commit title, task completion in
//! GraphQL, `authorIsContributor`, `commit_id` on diff discussions, and `Commit.tags` /
//! `pipelines` in GraphQL.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn shortcuts_probes() {
    let mut s = Scenario::new("shortcuts_probes").await;
    let admin = s.admin();
    let gpath = s.name("sp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public"}), "project").await;
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
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("sp"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let w = |rel: &str, content: &str| std::fs::write(repo.join(rel), content).unwrap();
    let mrs = format!("/projects/{pid}/merge_requests");
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    w("a.txt", &body);
    w("t.txt", &body);
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Base"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    // 1. The source edits a.txt; the target renames it.
    s.git(&repo, &["checkout", "-q", "-b", "edit"]).await;
    w("a.txt", &body.replace("line 10\n", "LINE 10\n"));
    s.git(&repo, &["commit", "-q", "-am", "Edit a"]).await;
    s.git(&repo, &["push", "-q", "origin", "edit"]).await;
    s.git(&repo, &["checkout", "-q", "main"]).await;
    s.git(&repo, &["mv", "a.txt", "renamed.txt"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Rename a"]).await;
    s.git(&repo, &["push", "-q", "origin", "main"]).await;
    s.settle().await;
    s.step("mr across rename", POST, &mrs, Some(&admin), json!({"source_branch": "edit", "target_branch": "main", "title": "Edit across rename", "description": "- [x] done\n- [ ] open\n1. [ ] numbered"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.graphql("task completion", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { taskCompletionStatus { count completedCount } } } }", json!({"p": full})).await;
    s.step("merge across rename", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.step("renamed after merge", GET, &format!("/projects/{pid}/repository/files/renamed.txt/raw"), Some(&admin), json!({"ref": "main"}), "file").await;
    // 2. A thread on t.txt, then the source renames it.
    s.git(&repo, &["fetch", "-q", "origin"]).await;
    s.git(&repo, &["checkout", "-q", "-b", "move", "origin/main"]).await;
    w("t.txt", &body.replace("line 5\n", "LINE 5\n"));
    s.git(&repo, &["commit", "-q", "-am", "Edit t"]).await;
    s.git(&repo, &["push", "-q", "origin", "move"]).await;
    s.settle().await;
    s.step("mr move", POST, &mrs, Some(&bob_t), json!({"source_branch": "move", "target_branch": "main", "title": "Move"}), "mr").await;
    s.poll_mr(pid, 2).await;
    let m = s.send(GET, &format!("{mrs}/2"), Some(&admin), json!(null)).await;
    let refs = m.body["diff_refs"].clone();
    let head = refs["head_sha"].clone();
    let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": "t.txt", "new_path": "t.txt", "new_line": 5});
    s.step("thread with commit_id", POST, &format!("{mrs}/2/discussions"), Some(&admin), json!({"body": "On line 5", "position": pos, "commit_id": head}), "discussion").await;
    s.git(&repo, &["mv", "t.txt", "moved.txt"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Tidy (squash! later)"]).await;
    s.git(&repo, &["push", "-q", "origin", "move"]).await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("mr 2 after rename", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("discussions after rename", GET, &format!("{mrs}/2/discussions"), Some(&admin), json!(null), "discussion").await;
    s.step("notes after rename", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    // 3. Bob's MR merged on the default branch; then bob is no longer a member.
    s.step("merge bob mr", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.step("bob removed", DELETE, &format!("/projects/{pid}/members/{bob}"), Some(&admin), json!(null), "user").await;
    s.step("bob comments", POST, &format!("{mrs}/2/notes"), Some(&bob_t), json!({"body": "Thanks"}), "note").await;
    s.graphql("author is contributor", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"2\") { notes(filter: ONLY_COMMENTS) { nodes { body authorIsContributor } } } } }", json!({"p": full})).await;
    // 4. Commit tags and pipelines in GraphQL.
    let main_head = s.send(GET, &format!("/projects/{pid}/repository/branches/main"), Some(&admin), json!(null)).await.body["commit"]["id"].as_str().unwrap_or_default().to_string();
    s.step("tag", POST, &format!("/projects/{pid}/repository/tags"), Some(&admin), json!({"tag_name": "v1", "ref": main_head}), "tag").await;
    s.step("status", POST, &format!("/projects/{pid}/statuses/{main_head}"), Some(&admin), json!({"state": "success", "name": "ci", "ref": "main"}), "status").await;
    s.settle().await;
    s.graphql("commit tags pipelines", Some(&admin), "query($p: ID!, $s: String!) { project(fullPath: $p) { repository { commit(ref: $s) { sha pipelines { nodes { status source } } } } } }", json!({"p": full, "s": main_head})).await;
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
