//! EE merge trains over GraphQL: `Project.mergeTrains` and its cars, the MR's train fields
//! and auto-merge strategies, `mergeRequestAccept` with the train strategies, and
//! `mergeTrainsDeleteCar`.

use super::approval_settings::user_with_token;
use super::merge_train_settings::{branch_mr, ci_project, enable_trains};
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

const TRAINS: &str = "query($path: ID!, $status: MergeTrainStatus) { project(fullPath: $path) {
  mergeTrains(status: $status) { count nodes { targetBranch
    cars(activityStatus: ACTIVE) { count nodes { id index status targetBranch duration mergedAt
      mergeRequest { iid title } user { username } pipeline { iid status ref source }
      userPermissions { deleteMergeTrainCar } } } } } } }";
const COMPLETED: &str = "query($path: ID!) { project(fullPath: $path) {
  mergeTrains(targetBranches: [\"main\"]) { nodes { targetBranch
    cars(activityStatus: COMPLETED) { count nodes { index status mergedAt mergeRequest { iid } } } } } } }";
const MR: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  state mergeable detailedMergeStatus autoMergeEnabled autoMergeStrategy availableAutoMergeStrategies
  mergeWhenPipelineSucceeds mergeTrainIndex mergeTrainsCount
  mergeTrainCar { index status mergeRequest { iid } pipeline { iid status ref } }
  headPipeline { iid status ref source } } } }";
const ACCEPT: &str = "mutation($path: ID!, $iid: String!, $sha: String!, $strategy: MergeStrategyEnum) {
  mergeRequestAccept(input: {projectPath: $path, iid: $iid, sha: $sha, strategy: $strategy}) {
    errors mergeRequest { state autoMergeEnabled autoMergeStrategy mergeTrainIndex mergeTrainCar { index status } } } }";
const DELETE_CAR: &str = "mutation($id: MergeTrainsCarID!) { mergeTrainsDeleteCar(input: {carId: $id}) { errors } }";

#[tokio::test]
async fn graphql_merge_trains() {
    let mut s = Scenario::new("graphql_merge_trains").await;
    let admin = s.admin();
    let (pid, full) = ci_project(&mut s, "gmt").await;
    enable_trains(&mut s, pid).await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, name) in [(1, "one"), (2, "two"), (3, "three"), (4, "four")] {
        branch_mr(&mut s, pid, i, name, &admin).await;
    }
    s.ci_pick(4).await;
    let shas: Vec<String> = {
        let mut v = vec![];
        for i in 1..=4 {
            let r = s.step(&format!("mr {i}"), GET, &format!("{mrs}/{i}"), Some(&admin), json!(null), "mr").await;
            v.push(r.body["sha"].as_str().unwrap_or("").to_string());
        }
        v
    };
    let mr = |i: i64| json!({"path": full, "iid": i.to_string()});
    s.graphql("mr 1 running", Some(&admin), MR, mr(1)).await;
    s.graphql("trains empty", Some(&admin), TRAINS, json!({"path": full})).await;

    // While the pipeline runs, the train strategy can only wait for the checks.
    let accept = |i: usize, strategy: &str| json!({"path": full, "iid": (i + 1).to_string(), "sha": shas[i], "strategy": strategy});
    s.graphql("accept train while running", Some(&admin), ACCEPT, accept(0, "MERGE_TRAIN")).await;
    s.graphql("accept when checks pass", Some(&admin), ACCEPT, accept(3, "ADD_TO_MERGE_TRAIN_WHEN_CHECKS_PASS")).await;
    s.graphql("mr 4 waiting", Some(&admin), MR, mr(4)).await;
    for i in 1..=3 {
        s.ci_finish(&format!("refs/merge-requests/{i}/merge"), "success").await;
    }
    s.settle().await;
    for i in 1..=3 {
        s.poll_mr(pid, i).await;
    }
    s.graphql("mr 1 ready", Some(&admin), MR, mr(1)).await;

    // Three cars through mergeRequestAccept.
    s.graphql("accept bad sha", Some(&admin), ACCEPT, json!({"path": full, "iid": "1", "sha": "0000000000000000000000000000000000000000", "strategy": "MERGE_TRAIN"})).await;
    s.graphql("accept 1", Some(&admin), ACCEPT, accept(0, "MERGE_TRAIN")).await;
    // Developers can't merge into the protected default branch.
    s.graphql("accept 2 as dev", Some(&dev_t), ACCEPT, accept(1, "MERGE_TRAIN")).await;
    s.graphql("accept 2", Some(&admin), ACCEPT, accept(1, "MERGE_TRAIN")).await;
    s.graphql("accept 3", Some(&admin), ACCEPT, accept(2, "MERGE_TRAIN")).await;
    s.graphql("accept 3 again", Some(&admin), ACCEPT, accept(2, "MERGE_TRAIN")).await;
    s.settle().await;
    s.ci_pick(3).await;
    let trains = s.graphql("trains", Some(&admin), TRAINS, json!({"path": full})).await;
    s.graphql("trains as dev", Some(&dev_t), TRAINS, json!({"path": full})).await;
    s.graphql("trains completed filter", Some(&admin), TRAINS, json!({"path": full, "status": "COMPLETED"})).await;
    for i in 1..=3 {
        s.graphql(&format!("mr {i} on train"), Some(&admin), MR, mr(i)).await;
    }
    let car_ids: Vec<String> = trains.body["data"]["project"]["mergeTrains"]["nodes"][0]["cars"]["nodes"]
        .as_array()
        .map(|a| a.iter().map(|c| c["id"].as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default();

    // Deleting car 2 (the middle one).
    let car2 = car_ids.get(1).cloned().unwrap_or_default();
    s.graphql("delete car missing", Some(&admin), DELETE_CAR, json!({"id": "gid://gitlab/MergeTrains::Car/999999999"})).await;
    s.graphql("delete car 2 as dev", Some(&dev_t), DELETE_CAR, json!({"id": car2})).await;
    s.graphql("delete car 2", Some(&admin), DELETE_CAR, json!({"id": car2})).await;
    s.settle().await;
    s.graphql("delete car 2 again", Some(&admin), DELETE_CAR, json!({"id": car2})).await;
    s.graphql("trains after delete", Some(&admin), TRAINS, json!({"path": full})).await;
    s.graphql("mr 2 off train", Some(&admin), MR, mr(2)).await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_pick(1).await;

    // Cars 1 and 3 merge.
    s.ci_finish("refs/merge-requests/1/train", "success").await;
    s.ci_finish("refs/merge-requests/3/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/3"), |b| b["state"] == "merged").await;
    s.graphql("trains completed", Some(&admin), COMPLETED, json!({"path": full})).await;
    s.graphql("mr 1 merged", Some(&admin), MR, mr(1)).await;
    s.graphql("mr 4 still waiting", Some(&admin), MR, mr(4)).await;
    s.ci_drain().await;
    s.finish();
}
