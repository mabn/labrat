//! EE merge trains: pipeline webhooks for merge request and merge train pipelines, the merge
//! hook of a car, and `PUT /merge` with `auto_merge` on a project with merge trains (refused
//! while the pipeline runs, a direct merge once it passed).

use super::merge_train_settings::{branch_mr, ci_project, enable_trains};
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn merge_train_hooks() {
    let mut s = Scenario::new("merge_train_hooks").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    // The receiver is local when replaying (on the reference instance it is reached through
    // HOOK_PUBLIC_URL).
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (pid, _full) = ci_project(&mut s, "mth").await;
    enable_trains(&mut s, pid).await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let trains = format!("/projects/{pid}/merge_trains");

    // MRs 1 and 2 get passing pipelines before the hook exists.
    branch_mr(&mut s, pid, 1, "one", &admin).await;
    branch_mr(&mut s, pid, 2, "two", &admin).await;
    s.ci_pick(2).await;
    s.ci_finish("refs/merge-requests/1/merge", "success").await;
    s.ci_finish("refs/merge-requests/2/merge", "success").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.poll_mr(pid, 2).await;
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": rx.url, "merge_requests_events": true, "pipeline_events": true, "push_events": false}),
        "hook",
    )
    .await;

    // MR 3: its merged results pipeline is pending, then running.
    branch_mr(&mut s, pid, 3, "three", &admin).await;
    s.deliveries("mr 3 open and pipeline pending", &mut rx, 2).await;
    s.step("auto merge while pending", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.step("mwps while pending", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!({"merge_when_pipeline_succeeds": true}), "mr").await;
    s.step("mr 3 not set", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.ci_pick(1).await;
    s.deliveries("mr 3 pipeline running", &mut rx, 1).await;

    // MR 1: its pipeline passed, so `auto_merge` merges it at once.
    s.step("auto merge after pass", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.deliveries("mr 1 merge", &mut rx, 1).await;

    // MR 2 goes through the train: its train pipeline's hooks, then the merge. MR 1's merge left
    // it unchecked, so it is added once its check is done.
    s.poll_mr(pid, 2).await;
    s.step("add 2", POST, &format!("{trains}/merge_requests/2"), Some(&admin), json!({}), "car").await;
    s.settle().await;
    s.deliveries("train pipeline pending", &mut rx, 1).await;
    s.ci_pick(1).await;
    s.deliveries("train pipeline running", &mut rx, 1).await;
    s.ci_finish("refs/merge-requests/2/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/2"), |b| b["state"] == "merged").await;
    s.deliveries("train pipeline success and merge", &mut rx, 2).await;
    s.step("mr 2 merged", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;

    // MR 3's pipeline fails.
    s.ci_finish("refs/merge-requests/3/merge", "failed").await;
    s.settle().await;
    s.deliveries("mr 3 pipeline failed", &mut rx, 1).await;
    s.step("pipeline jobs", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.ci_drain().await;
    s.finish();
}
