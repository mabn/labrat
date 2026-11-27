//! What completes an armed auto-merge: the MR's own (detached) pipeline passing, over REST and
//! GraphQL (with the branch removal and commit message kept); a failing pipeline that leaves it
//! armed; a draft marked ready; and the last open thread resolved.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const ACCEPT: &str = "mutation($path: ID!, $iid: String!, $sha: String!, $msg: String) {
  mergeRequestAccept(input: {projectPath: $path, iid: $iid, sha: $sha, strategy: MERGE_WHEN_CHECKS_PASS,
    shouldRemoveSourceBranch: true, commitMessage: $msg}) {
    errors mergeRequest { state autoMergeEnabled autoMergeStrategy } } }";

#[tokio::test]
async fn auto_merge_triggers() {
    let mut s = Scenario::new("auto_merge_triggers").await;
    let admin = s.admin();
    let gpath = s.name("amt");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    s.ci_enable("ci config", pid, &admin).await;
    s.settle().await;
    s.step("discussions setting", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_all_discussions_are_resolved": true}), "project").await;
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, name) in [(1, "one"), (2, "two"), (3, "three"), (4, "four"), (5, "five")] {
        s.step(
            &format!("commit {name}"),
            POST,
            &format!("/projects/{pid}/repository/commits"),
            Some(&admin),
            json!({"branch": name, "start_branch": "main", "commit_message": format!("Add {name}"), "actions": [{"action": "create", "file_path": format!("{name}.txt"), "content": format!("{name}\n")}]}),
            "commit",
        )
        .await;
        s.settle().await;
        let title = if i == 4 { format!("Draft: Add {name}") } else { format!("Add {name}") };
        s.step(&format!("mr {name}"), POST, &mrs, Some(&admin), json!({"source_branch": name, "target_branch": "main", "title": title}), "mr").await;
        s.poll_mr(pid, i).await;
    }
    s.ci_pick(5).await;
    let mut shas = vec![String::new()];
    for i in 1..=5 {
        let r = s.step(&format!("mr {i} running"), GET, &format!("{mrs}/{i}"), Some(&admin), json!(null), "mr").await;
        shas.push(r.body["sha"].as_str().unwrap_or("").to_string());
    }

    // Armed while the pipelines run: over REST, and over GraphQL with branch removal and a
    // commit message.
    s.step("arm 1", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.graphql("arm 2", Some(&admin), ACCEPT, json!({"path": full, "iid": "2", "sha": shas[2], "msg": "Custom merge of two"})).await;
    s.step("arm 3", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;

    // The MR pipelines finish one at a time (merges at the same moment race on GitLab): 1 and
    // 2 merge, 3 failed and stays armed.
    s.ci_finish("refs/merge-requests/1/head", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/1"), |b| b["state"] == "merged").await;
    s.ci_finish("refs/merge-requests/2/head", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/2"), |b| b["state"] == "merged").await;
    s.ci_finish("refs/merge-requests/3/head", "failed").await;
    s.settle().await;
    s.step("mr 1 merged", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    let two = s.step("mr 2 merged", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("branch two", GET, &format!("/projects/{pid}/repository/branches/two"), Some(&admin), json!(null), "branch").await;
    if let Some(sha) = two.body["merge_commit_sha"].as_str() {
        s.step("mr 2 merge commit", GET, &format!("/projects/{pid}/repository/commits/{sha}"), Some(&admin), json!(null), "commit").await;
    }
    s.step("mr 3 failed", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.step("mr 3 notes", GET, &format!("{mrs}/3/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;

    // A draft with a green pipeline, armed, then marked ready.
    s.ci_finish("refs/merge-requests/4/head", "success").await;
    s.settle().await;
    s.poll_mr(pid, 4).await;
    s.step("arm 4", PUT, &format!("{mrs}/4/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.step("mr 4 ready", PUT, &format!("{mrs}/4"), Some(&admin), json!({"title": "Add four"}), "mr").await;
    s.settle().await;
    s.poll(&format!("{mrs}/4"), |b| b["state"] == "merged").await;
    s.step("mr 4 after ready", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;

    // An open thread with a green pipeline, armed, then resolved.
    let d = s.step("thread", POST, &format!("{mrs}/5/discussions"), Some(&admin), json!({"body": "Please check"}), "discussion").await;
    let did = d.body["id"].as_str().unwrap_or("").to_string();
    s.ci_finish("refs/merge-requests/5/head", "success").await;
    s.settle().await;
    s.poll_mr(pid, 5).await;
    s.step("arm 5", PUT, &format!("{mrs}/5/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.step("resolve", PUT, &format!("{mrs}/5/discussions/{did}"), Some(&admin), json!({"resolved": true}), "discussion").await;
    s.settle().await;
    s.poll(&format!("{mrs}/5"), |b| b["state"] == "merged").await;
    s.step("mr 5 after resolve", GET, &format!("{mrs}/5"), Some(&admin), json!(null), "mr").await;
    s.ci_drain().await;
    s.finish();
}
