//! More EE merge train paths: adding an MR whose pipeline is still running (straight onto the
//! train, and "when checks pass" over REST), and squashing on a rebase-merge train, over REST
//! and through `mergeRequestAccept`, with two cars behind the target.

use super::merge_train_settings::{branch_mr, ci_project, enable_trains};
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const MR: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  state mergeable detailedMergeStatus shouldBeRebased autoMergeEnabled autoMergeStrategy
  availableAutoMergeStrategies mergeTrainIndex headPipeline { status ref } } } }";
const ACCEPT: &str = "mutation($path: ID!, $iid: String!, $sha: String!, $strategy: MergeStrategyEnum,
  $squash: Boolean, $squashMsg: String) {
  mergeRequestAccept(input: {projectPath: $path, iid: $iid, sha: $sha, strategy: $strategy,
    squash: $squash, squashCommitMessage: $squashMsg}) {
    errors mergeRequest { state autoMergeEnabled autoMergeStrategy mergeTrainIndex } } }";

#[tokio::test]
async fn merge_train_gaps_2() {
    let mut s = Scenario::new("merge_train_gaps_2").await;
    let admin = s.admin();
    let (pid, full) = ci_project(&mut s, "mtg2").await;
    enable_trains(&mut s, pid).await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let trains = format!("/projects/{pid}/merge_trains");
    for (i, name) in [(1, "one"), (2, "two"), (3, "three")] {
        branch_mr(&mut s, pid, i, name, &admin).await;
    }
    s.ci_pick(3).await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    let mr = |i: i64| json!({"path": full, "iid": i.to_string()});

    // MR 1's pipeline is running: straight onto the train is refused; "when checks pass"
    // over REST arms it, and it joins once the pipeline passes.
    s.step("mr 1 running", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.graphql("mr 1 running graphql", Some(&admin), MR, mr(1)).await;
    s.step("add 1 running", POST, &format!("{trains}/merge_requests/1"), Some(&admin), json!({}), "car").await;
    s.step("add 1 when checks pass", POST, &format!("{trains}/merge_requests/1"), Some(&admin), json!({"auto_merge": true}), "car").await;
    s.settle().await;
    s.step("mr 1 armed", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.graphql("mr 1 armed graphql", Some(&admin), MR, mr(1)).await;
    s.step("car 1 waiting", GET, &format!("{trains}/merge_requests/1"), Some(&admin), json!(null), "car").await;
    s.step("train waiting", GET, &format!("{trains}/main"), Some(&admin), json!(null), "car").await;
    s.step("mr 1 notes armed", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_finish("refs/merge-requests/1/merge", "success").await;
    s.settle().await;
    s.ci_pick(1).await;
    s.step("car 1", GET, &format!("{trains}/merge_requests/1"), Some(&admin), json!(null), "car").await;
    s.ci_finish("refs/merge-requests/1/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/1"), |b| b["state"] == "merged").await;
    s.step("mr 1 merged", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;

    // Rebase merges with squash: MRs 2 and 3 are behind main now. 2 goes on over REST, 3
    // through mergeRequestAccept with a squash message.
    for ref_ in ["refs/merge-requests/2/merge", "refs/merge-requests/3/merge"] {
        s.ci_finish(ref_, "success").await;
    }
    s.settle().await;
    s.step("rebase merge", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_method": "rebase_merge"}), "project").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("mr 2 behind", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("add 2 squash", POST, &format!("{trains}/merge_requests/2"), Some(&admin), json!({"squash": true}), "car").await;
    s.settle().await;
    s.poll_mr(pid, 3).await;
    let m3 = s.step("mr 3 behind", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    let sha3 = m3.body["sha"].as_str().unwrap_or("").to_string();
    s.graphql(
        "accept 3 squash",
        Some(&admin),
        ACCEPT,
        json!({"path": full, "iid": "3", "sha": sha3, "strategy": "MERGE_TRAIN", "squash": true, "squashMsg": "Squashed three"}),
    )
    .await;
    s.settle().await;
    s.ci_pick(2).await;
    s.step("car 2", GET, &format!("{trains}/merge_requests/2"), Some(&admin), json!(null), "car").await;
    s.step("car 3", GET, &format!("{trains}/merge_requests/3"), Some(&admin), json!(null), "car").await;
    s.ci_finish("refs/merge-requests/2/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/2"), |b| b["state"] == "merged").await;
    let two = s.step("mr 2 merged", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.ci_finish("refs/merge-requests/3/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/3"), |b| b["state"] == "merged").await;
    let three = s.step("mr 3 merged", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    for (i, b) in [(2, &two), (3, &three)] {
        for key in ["merge_commit_sha", "squash_commit_sha"] {
            if let Some(sha) = b.body[key].as_str() {
                s.step(&format!("mr {i} {key}"), GET, &format!("/projects/{pid}/repository/commits/{sha}"), Some(&admin), json!(null), "commit").await;
            }
        }
    }
    s.step("main commits", GET, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"ref_name": "main", "per_page": 10}), "commit").await;
    s.ci_drain().await;
    s.finish();
}
