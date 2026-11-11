//! `mergeRequestAccept(strategy:)`, `mergeRequestUpdate(timeEstimate:)` and
//! `MergeRequest.pipelines` arguments.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_accept_strategy() {
    let mut s = Scenario::new("graphql_accept_strategy").await;
    let admin = s.admin();
    let gpath = s.name("gas");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let mut shas = vec![];
    for (i, (b, title)) in [("b1", "With pipeline"), ("b2", "Plain"), ("b3", "Draft: Not ready"), ("b4", "Other strategy")].iter().enumerate() {
        let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        shas.push(c.get("id").as_str().unwrap().to_string());
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": title}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("pending status", POST, &format!("/projects/{pid}/statuses/{}", shas[0]), Some(&admin), json!({"state": "running", "name": "ci"}), "status").await;
    s.settle().await;

    let out = "mergeRequest { iid state autoMergeEnabled autoMergeStrategy mergeWhenPipelineSucceeds squash } errors";
    let accept = |iid: &str, extra: &str| format!("mutation($path: ID!, $sha: String!) {{ mergeRequestAccept(input: {{projectPath: $path, iid: \"{iid}\", sha: $sha{extra}}}) {{ {out} }} }}");
    s.graphql(
        "nullability mismatch",
        Some(&admin),
        &format!("mutation($path: ID!, $sha: String) {{ mergeRequestAccept(input: {{projectPath: $path, iid: \"1\", sha: $sha}}) {{ {out} }} }}"),
        json!({"path": full, "sha": shas[0]}),
    )
    .await;
    s.graphql("mwcp with running pipeline", Some(&admin), &accept("1", ", strategy: MERGE_WHEN_CHECKS_PASS, squash: true"), json!({"path": full, "sha": shas[0]})).await;
    s.graphql("mwcp again", Some(&admin), &accept("1", ", strategy: MERGE_WHEN_CHECKS_PASS"), json!({"path": full, "sha": shas[0]})).await;
    s.graphql("mwcp mergeable now", Some(&admin), &accept("2", ", strategy: MERGE_WHEN_CHECKS_PASS"), json!({"path": full, "sha": shas[1]})).await;
    s.graphql("mwcp wrong sha", Some(&admin), &accept("3", ", strategy: MERGE_WHEN_CHECKS_PASS"), json!({"path": full, "sha": shas[0]})).await;
    s.graphql("mwcp draft", Some(&admin), &accept("3", ", strategy: MERGE_WHEN_CHECKS_PASS"), json!({"path": full, "sha": shas[2]})).await;
    s.graphql("mwps", Some(&admin), &accept("4", ", strategy: MERGE_WHEN_PIPELINE_SUCCEEDS"), json!({"path": full, "sha": shas[3]})).await;
    s.settle().await;
    s.step("mr 1", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 3", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;

    let upd = |extra: &str| format!("mutation($path: ID!) {{ mergeRequestUpdate(input: {{projectPath: $path, iid: \"2\"{extra}}}) {{ mergeRequest {{ timeEstimate totalTimeSpent humanTimeEstimate }} errors }} }}");
    s.graphql("time estimate", Some(&admin), &upd(", timeEstimate: \"1h 30m\""), json!({"path": full})).await;
    s.graphql("time estimate zero", Some(&admin), &upd(", timeEstimate: \"0\""), json!({"path": full})).await;
    s.graphql("time estimate invalid", Some(&admin), &upd(", timeEstimate: \"soon\""), json!({"path": full})).await;
    s.graphql("time estimate negative", Some(&admin), &upd(", timeEstimate: \"-1h\""), json!({"path": full})).await;
    s.settle().await;

    let pl = |args: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequest(iid: \"1\") {{ pipelines{args} {{ count nodes {{ status ref sha source }} }} }} }} }}");
    s.graphql("pipelines", Some(&admin), &pl(""), json!({"path": full})).await;
    s.graphql("pipelines running", Some(&admin), &pl("(status: RUNNING)"), json!({"path": full})).await;
    s.graphql("pipelines success", Some(&admin), &pl("(status: SUCCESS)"), json!({"path": full})).await;
    s.graphql("pipelines by sha", Some(&admin), &pl("(sha: \"0000000000000000000000000000000000000000\")"), json!({"path": full})).await;
    s.finish();
}
