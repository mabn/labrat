//! Armed auto-merges and MR pipeline events: a push to the source branch aborts it (the new
//! pipeline passing doesn't merge); the target branch moving on doesn't stop it; cancelling it
//! before the pipeline passes; and a failed pipeline retried until it passes.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn auto_merge_pipeline_events() {
    let mut s = Scenario::new("auto_merge_pipeline_events").await;
    let admin = s.admin();
    let gpath = s.name("ampe");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    s.ci_enable("ci config", pid, &admin).await;
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, name) in [(1, "one"), (2, "two"), (3, "three"), (4, "four")] {
        s.step(
            &format!("commit {name}"),
            POST,
            &commits,
            Some(&admin),
            json!({"branch": name, "start_branch": "main", "commit_message": format!("Add {name}"), "actions": [{"action": "create", "file_path": format!("{name}.txt"), "content": format!("{name}\n")}]}),
            "commit",
        )
        .await;
        s.settle().await;
        s.step(&format!("mr {name}"), POST, &mrs, Some(&admin), json!({"source_branch": name, "target_branch": "main", "title": format!("Add {name}")}), "mr").await;
        s.poll_mr(pid, i).await;
    }
    s.ci_pick(4).await;
    for i in 1..=4 {
        s.step(&format!("arm {i}"), PUT, &format!("{mrs}/{i}/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    }
    s.settle().await;

    // A push to MR 1's branch aborts its auto-merge; the new pipeline passing doesn't merge it.
    s.step("push one", POST, &commits, Some(&admin), json!({"branch": "one", "commit_message": "More one", "actions": [{"action": "update", "file_path": "one.txt", "content": "one more\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1 after push", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_pick(1).await;
    s.ci_finish("refs/merge-requests/1/head", "success").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr 1 new pipeline passed", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;

    // main moves on while MR 2's pipeline runs; it still merges when the pipeline passes.
    s.step("main moves", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Upstream", "actions": [{"action": "create", "file_path": "upstream.txt", "content": "up\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("mr 2 after main moved", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.ci_finish("refs/merge-requests/2/head", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/2"), |b| b["state"] == "merged").await;
    s.step("mr 2 merged", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;

    // MR 3: cancelled before its pipeline passes.
    s.step("cancel 3", POST, &format!("{mrs}/3/cancel_merge_when_pipeline_succeeds"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.ci_finish("refs/merge-requests/3/head", "success").await;
    s.settle().await;
    s.poll_mr(pid, 3).await;
    s.step("mr 3 not merged", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;

    // MR 4: the pipeline fails, is retried, and passes.
    s.ci_finish("refs/merge-requests/4/head", "failed").await;
    s.settle().await;
    let m4 = s.step("mr 4 failed", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    let pl = m4.body["head_pipeline"]["id"].as_i64().unwrap_or(0);
    s.step("retry 4", POST, &format!("/projects/{pid}/pipelines/{pl}/retry"), Some(&admin), json!(null), "pipeline").await;
    s.settle().await;
    s.step("pipeline 4 retried", GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&admin), json!(null), "pipeline").await;
    s.step("mr 4 retried", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.ci_pick(1).await;
    s.step("pipeline 4 rerunning", GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&admin), json!(null), "pipeline").await;
    s.ci_finish("refs/merge-requests/4/head", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/4"), |b| b["state"] == "merged").await;
    s.step("mr 4 after retry", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.step("pipeline 4 passed", GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&admin), json!(null), "pipeline").await;
    s.step("mr 4 notes", GET, &format!("{mrs}/4/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_drain().await;
    s.finish();
}
