//! EE merge trains, the less travelled paths: `mergeRequestAccept` with branch removal, squash
//! and commit messages; MRs behind their target in fast-forward and rebase-merge projects, which
//! the train rebases (two cars, one squashed, on the fast-forward train); and a train project
//! whose MR has no pipeline at all.

use super::merge_train_settings::{branch_mr, ci_project, enable_trains};
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const MR: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  state mergeable detailedMergeStatus shouldBeRebased divergedFromTargetBranch autoMergeEnabled
  autoMergeStrategy availableAutoMergeStrategies mergeTrainIndex headPipeline { status ref }
  mergeabilityChecks { identifier status } } } }";
const ACCEPT: &str = "mutation($path: ID!, $iid: String!, $sha: String!, $strategy: MergeStrategyEnum, $remove: Boolean,
  $msg: String, $squash: Boolean, $squashMsg: String) {
  mergeRequestAccept(input: {projectPath: $path, iid: $iid, sha: $sha, strategy: $strategy, shouldRemoveSourceBranch: $remove,
    commitMessage: $msg, squash: $squash, squashCommitMessage: $squashMsg}) {
    errors mergeRequest { state autoMergeEnabled autoMergeStrategy mergeTrainIndex } } }";

#[tokio::test]
async fn merge_train_gaps() {
    let mut s = Scenario::new("merge_train_gaps").await;
    let admin = s.admin();
    let (pid, full) = ci_project(&mut s, "mtg").await;
    enable_trains(&mut s, pid).await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let trains = format!("/projects/{pid}/merge_trains");
    for (i, name) in [(1, "one"), (2, "two"), (3, "three"), (4, "four")] {
        branch_mr(&mut s, pid, i, name, &admin).await;
    }
    s.ci_pick(4).await;
    for i in 1..=4 {
        s.ci_finish(&format!("refs/merge-requests/{i}/merge"), "success").await;
    }
    s.settle().await;
    let mut shas = vec![String::new()];
    for i in 1..=4 {
        s.poll_mr(pid, i).await;
        let r = s.step(&format!("mr {i} ready"), GET, &format!("{mrs}/{i}"), Some(&admin), json!(null), "mr").await;
        shas.push(r.body["sha"].as_str().unwrap_or("").to_string());
    }
    let mr = |i: i64| json!({"path": full, "iid": i.to_string()});

    // mergeRequestAccept onto the train with branch removal, squash and commit messages.
    s.graphql(
        "accept 1 with options",
        Some(&admin),
        ACCEPT,
        json!({"path": full, "iid": "1", "sha": shas[1], "strategy": "MERGE_TRAIN", "remove": true,
               "msg": "Train merge of one", "squash": true, "squashMsg": "Squashed one"}),
    )
    .await;
    s.settle().await;
    s.ci_pick(1).await;
    s.step("mr 1 on train", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.ci_finish("refs/merge-requests/1/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/1"), |b| b["state"] == "merged").await;
    let one = s.step("mr 1 merged", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("branch one", GET, &format!("/projects/{pid}/repository/branches/one"), Some(&admin), json!(null), "branch").await;
    for key in ["merge_commit_sha", "squash_commit_sha"] {
        if let Some(sha) = one.body[key].as_str() {
            s.step(&format!("mr 1 {key}"), GET, &format!("/projects/{pid}/repository/commits/{sha}"), Some(&admin), json!(null), "commit").await;
        }
    }

    // Fast-forward merges: MRs 2 and 4 are now behind main; both go on the train, 4 squashed.
    s.step("fast-forward", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_method": "ff"}), "project").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("mr 2 behind", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.graphql("mr 2 behind graphql", Some(&admin), MR, mr(2)).await;
    s.step("add 2 behind", POST, &format!("{trains}/merge_requests/2"), Some(&admin), json!({}), "car").await;
    s.settle().await;
    // MR 4 hasn't been read since main moved: its mergeability is unchecked, which the train
    // refuses.
    s.step("add 4 unchecked", POST, &format!("{trains}/merge_requests/4"), Some(&admin), json!({}), "car").await;
    s.poll_mr(pid, 4).await;
    s.step("mr 4 behind", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.step("add 4 behind", POST, &format!("{trains}/merge_requests/4"), Some(&admin), json!({"squash": true}), "car").await;
    s.settle().await;
    s.ci_pick(2).await;
    s.step("car 2", GET, &format!("{trains}/merge_requests/2"), Some(&admin), json!(null), "car").await;
    s.step("car 4", GET, &format!("{trains}/merge_requests/4"), Some(&admin), json!(null), "car").await;
    s.ci_finish("refs/merge-requests/2/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/2"), |b| b["state"] == "merged").await;
    s.step("mr 2 merged", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 4 behind merged car", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.ci_finish("refs/merge-requests/4/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/4"), |b| b["state"] == "merged").await;
    s.step("mr 4 merged", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.step("ff commits", GET, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"ref_name": "main", "per_page": 6}), "commit").await;

    // Rebase merges: MR 3 is behind too.
    s.step("rebase merge", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_method": "rebase_merge"}), "project").await;
    s.settle().await;
    s.poll_mr(pid, 3).await;
    s.step("mr 3 behind", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.graphql("mr 3 behind graphql", Some(&admin), MR, mr(3)).await;
    s.graphql("accept 3 behind", Some(&admin), ACCEPT, json!({"path": full, "iid": "3", "sha": shas[3], "strategy": "MERGE_TRAIN"})).await;
    s.settle().await;
    s.ci_pick(1).await;
    s.ci_finish("refs/merge-requests/3/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/3"), |b| b["state"] == "merged").await;
    s.step("mr 3 merged", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.step("main commits", GET, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"ref_name": "main", "per_page": 10}), "commit").await;
    s.ci_drain().await;

    // A train project without CI: its MR has no pipeline.
    let gpath = s.name("mtg-noci");
    let g = s.step("group no ci", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p2 = s.step("project no ci", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid2 = p2.id();
    let full2 = format!("{gpath}/app");
    s.settle().await;
    enable_trains(&mut s, pid2).await;
    branch_mr(&mut s, pid2, 1, "solo", &admin).await;
    let mrs2 = format!("/projects/{pid2}/merge_requests");
    let solo = s.step("solo", GET, &format!("{mrs2}/1"), Some(&admin), json!(null), "mr").await;
    let solo_sha = solo.body["sha"].as_str().unwrap_or("").to_string();
    s.graphql("solo graphql", Some(&admin), MR, json!({"path": full2, "iid": "1"})).await;
    s.graphql("solo accept train", Some(&admin), ACCEPT, json!({"path": full2, "iid": "1", "sha": solo_sha, "strategy": "MERGE_TRAIN"})).await;
    s.graphql("solo accept when checks pass", Some(&admin), ACCEPT, json!({"path": full2, "iid": "1", "sha": solo_sha, "strategy": "ADD_TO_MERGE_TRAIN_WHEN_CHECKS_PASS"})).await;
    s.step("solo add", POST, &format!("/projects/{pid2}/merge_trains/merge_requests/1"), Some(&admin), json!({}), "car").await;
    s.step("solo add when checks pass", POST, &format!("/projects/{pid2}/merge_trains/merge_requests/1"), Some(&admin), json!({"auto_merge": true}), "car").await;
    s.settle().await;
    s.step("solo after add", GET, &format!("{mrs2}/1"), Some(&admin), json!(null), "mr").await;
    s.step("solo notes", GET, &format!("{mrs2}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.graphql("solo accept", Some(&admin), ACCEPT, json!({"path": full2, "iid": "1", "sha": solo_sha})).await;
    s.settle().await;
    s.poll(&format!("{mrs2}/1"), |b| b["state"] == "merged").await;
    s.step("solo merged", GET, &format!("{mrs2}/1"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
