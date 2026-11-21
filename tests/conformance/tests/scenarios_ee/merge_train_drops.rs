//! EE merge trains: what takes a car off the train (a push to its branch, marking it draft,
//! closing it), merging an MR past the train ("merge immediately") and skipping the train with
//! the skip-train setting off and on.

use super::merge_train_settings::{branch_mr, ci_project, enable_trains};
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn merge_train_drops() {
    let mut s = Scenario::new("merge_train_drops").await;
    let admin = s.admin();
    let (pid, _full) = ci_project(&mut s, "mtd").await;
    enable_trains(&mut s, pid).await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let trains = format!("/projects/{pid}/merge_trains");
    let names = ["one", "two", "three", "four", "five", "six"];
    for (i, name) in names.iter().enumerate() {
        branch_mr(&mut s, pid, i as i64 + 1, name, &admin).await;
    }
    s.ci_pick(6).await;
    for i in 1..=6 {
        s.ci_finish(&format!("refs/merge-requests/{i}/merge"), "success").await;
    }
    s.settle().await;
    for i in 1..=6 {
        s.poll_mr(pid, i).await;
    }
    for i in 1..=4 {
        s.step(&format!("add {i}"), POST, &format!("{trains}/merge_requests/{i}"), Some(&admin), json!({}), "car").await;
    }
    s.settle().await;
    s.ci_pick(4).await;

    // A push to car 2's branch takes it off; cars 3 and 4 restart.
    s.step(
        "push two",
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        json!({"branch": "two", "commit_message": "More two", "actions": [{"action": "update", "file_path": "two.txt", "content": "two\nmore\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("list after push", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 2 after push", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 notes after push", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_pick(3).await;

    // Marking car 3 as draft takes it off.
    s.step("draft 3", PUT, &format!("{mrs}/3"), Some(&admin), json!({"title": "Draft: Add three"}), "mr").await;
    s.settle().await;
    s.step("list after draft", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 3 notes after draft", GET, &format!("{mrs}/3/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_pick(1).await;

    // Closing car 4 takes it off.
    // The close response races with the train dropping the car (its pipeline can read as
    // canceled and running at once), so the MR is read after things settle.
    let r = s.send(PUT, &format!("{mrs}/4"), Some(&admin), json!({"state_event": "close"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    s.settle().await;
    s.step("mr 4 closed", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.step("list after close", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 4 notes after close", GET, &format!("{mrs}/4/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("pipelines after drops", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!({"per_page": 50}), "pipeline").await;

    // Merging MR 5 past the train (skip-train setting off): car 1 restarts on top of it.
    s.step("skip train not allowed", PUT, &format!("{mrs}/5/merge"), Some(&admin), json!({"skip_merge_train": true}), "mr").await;
    s.step("merge 5 immediately", PUT, &format!("{mrs}/5/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.step("list after immediate merge", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 1 notes after immediate merge", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("mr 5 notes", GET, &format!("{mrs}/5/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_pick(1).await;

    // With the setting on, MR 6 skips the train: car 1 keeps its pipeline.
    s.step("allow skip train", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_trains_skip_train_allowed": true}), "project").await;
    s.settle().await;
    s.step("skip train 6", PUT, &format!("{mrs}/6/merge"), Some(&admin), json!({"skip_merge_train": true}), "mr").await;
    s.settle().await;
    s.step("list after skip", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("list complete after skip", GET, &trains, Some(&admin), json!({"scope": "complete"}), "car").await;
    s.step("mr 6 notes", GET, &format!("{mrs}/6/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;

    // Car 1 passes and merges.
    s.ci_finish("refs/merge-requests/1/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/1"), |b| b["state"] == "merged").await;
    s.step("list at the end", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("main commits", GET, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"ref_name": "main"}), "commit").await;
    s.ci_drain().await;
    s.finish();
}
