//! EE merge trains over REST: adding MRs (directly and when checks pass), car pipelines on the
//! train refs, the train listings, a failing car dropped with the cars behind it restarted, a
//! car that passes early waiting for the one ahead, merging, removal through "cancel auto
//! merge", and the completed cars.

use super::approval_settings::user_with_token;
use super::merge_train_settings::{branch_mr, ci_project, enable_trains};
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn merge_train_flow() {
    let mut s = Scenario::new("merge_train_flow").await;
    let admin = s.admin();
    let (pid, _full) = ci_project(&mut s, "mtf").await;
    enable_trains(&mut s, pid).await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    let (rep, rep_t) = user_with_token(&mut s, "rep").await;
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    s.step("rep member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": rep, "access_level": 20}), "user").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let trains = format!("/projects/{pid}/merge_trains");
    for (i, name) in [(1, "one"), (2, "two"), (3, "three"), (4, "four"), (5, "five")] {
        branch_mr(&mut s, pid, i, name, &admin).await;
    }
    s.ci_pick(5).await;
    s.step("mr 1 pipeline running", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;

    // Adding while the pipeline runs: refused unless when checks pass.
    s.step("merge while running", POST, &format!("{trains}/merge_requests/5"), Some(&admin), json!({}), "car").await;
    s.step("add when checks pass", POST, &format!("{trains}/merge_requests/5"), Some(&admin), json!({"auto_merge": true}), "car").await;
    s.step("add when checks pass again", POST, &format!("{trains}/merge_requests/5"), Some(&admin), json!({"auto_merge": true}), "car").await;
    s.step("mr 5 auto merge", GET, &format!("{mrs}/5"), Some(&admin), json!(null), "mr").await;
    s.step("mr 5 notes", GET, &format!("{mrs}/5/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    for i in 1..=4 {
        s.ci_finish(&format!("refs/merge-requests/{i}/merge"), "success").await;
    }
    s.settle().await;
    for i in 1..=4 {
        s.poll_mr(pid, i).await;
    }
    s.step("mr 1 pipeline passed", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;

    // Permissions and errors.
    s.step("add as reporter", POST, &format!("{trains}/merge_requests/1"), Some(&rep_t), json!({}), "car").await;
    s.step("add wrong sha", POST, &format!("{trains}/merge_requests/1"), Some(&admin), json!({"sha": "0000000000000000000000000000000000000000"}), "car").await;
    s.step("add missing mr", POST, &format!("{trains}/merge_requests/99"), Some(&admin), json!({}), "car").await;
    s.step("list as reporter", GET, &trains, Some(&rep_t), json!(null), "car").await;

    // Four cars.
    s.step("add 1", POST, &format!("{trains}/merge_requests/1"), Some(&admin), json!({}), "car").await;
    s.settle().await;
    s.step("add 1 again", POST, &format!("{trains}/merge_requests/1"), Some(&admin), json!({}), "car").await;
    // Developers can't merge into the protected default branch.
    s.step("add 2 as dev", POST, &format!("{trains}/merge_requests/2"), Some(&dev_t), json!({}), "car").await;
    s.step("add 2", POST, &format!("{trains}/merge_requests/2"), Some(&admin), json!({}), "car").await;
    s.settle().await;
    s.step("add 3", POST, &format!("{trains}/merge_requests/3"), Some(&admin), json!({"squash": true}), "car").await;
    s.settle().await;
    s.step("add 4", POST, &format!("{trains}/merge_requests/4"), Some(&admin), json!({"when_pipeline_succeeds": false}), "car").await;
    s.settle().await;
    s.ci_pick(4).await;
    s.step("list", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("list asc", GET, &trains, Some(&admin), json!({"sort": "asc"}), "car").await;
    s.step("list active", GET, &trains, Some(&admin), json!({"scope": "active"}), "car").await;
    s.step("list complete", GET, &trains, Some(&admin), json!({"scope": "complete"}), "car").await;
    s.step("list main", GET, &format!("{trains}/main"), Some(&admin), json!({"sort": "asc"}), "car").await;
    s.step("list other branch", GET, &format!("{trains}/other"), Some(&admin), json!(null), "car").await;
    s.step("car 2", GET, &format!("{trains}/merge_requests/2"), Some(&admin), json!(null), "car").await;
    s.step("car 5 not on train", GET, &format!("{trains}/merge_requests/5"), Some(&admin), json!(null), "car").await;
    s.step("mr 2 on train", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("pipelines", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("mr 3 pipelines", GET, &format!("{mrs}/3/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("merge mr 2 directly as dev", Method::PUT, &format!("{mrs}/2/merge"), Some(&dev_t), json!(null), "mr").await;

    // Car 2 fails: it leaves, cars 3 and 4 get new pipelines.
    s.ci_finish("refs/merge-requests/2/train", "failed").await;
    s.settle().await;
    s.step("list after failure", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 2 dropped", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mr 2 dropped notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("pipelines after failure", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.ci_pick(2).await;

    // Car 3 passes before car 1: it waits.
    s.ci_finish("refs/merge-requests/3/train", "success").await;
    s.settle().await;
    s.step("list car 3 waiting", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 3 waiting", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.ci_finish("refs/merge-requests/1/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/3"), |b| b["state"] == "merged").await;
    s.step("list merged", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("list merged complete", GET, &trains, Some(&admin), json!({"scope": "complete"}), "car").await;
    s.step("car 1 merged", GET, &format!("{trains}/merge_requests/1"), Some(&admin), json!(null), "car").await;
    s.step("mr 1 merged", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 3 merged", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("mr 3 notes", GET, &format!("{mrs}/3/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("main commits", GET, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"ref_name": "main"}), "commit").await;

    // Car 4 leaves through "cancel auto merge", then comes back.
    let cancel = format!("{mrs}/4/cancel_merge_when_pipeline_succeeds");
    s.step("remove 4 as reporter", POST, &cancel, Some(&rep_t), json!(null), "mr").await;
    s.step("remove 4 as dev", POST, &cancel, Some(&dev_t), json!(null), "mr").await;
    s.step("remove 4", POST, &cancel, Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.step("remove 4 again", POST, &cancel, Some(&admin), json!(null), "mr").await;
    s.step("list after removal", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 4 removed", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.step("mr 4 notes", GET, &format!("{mrs}/4/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.step("pipelines after removal", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;

    // MR 5's pipeline passes: it joins the train by itself. MR 4 rejoins behind it.
    s.ci_finish("refs/merge-requests/5/merge", "success").await;
    s.settle().await;
    s.step("list mr 5 joined", GET, &trains, Some(&admin), json!({"scope": "active"}), "car").await;
    s.step("add 4 back", POST, &format!("{trains}/merge_requests/4"), Some(&admin), json!({}), "car").await;
    s.settle().await;
    s.ci_pick(2).await;
    s.step("mr 5 notes after joining", GET, &format!("{mrs}/5/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.ci_finish("refs/merge-requests/5/train", "success").await;
    s.ci_finish("refs/merge-requests/4/train", "success").await;
    s.settle().await;
    s.poll(&format!("{mrs}/4"), |b| b["state"] == "merged").await;
    s.step("list all done", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("mr 4 merged", GET, &format!("{mrs}/4"), Some(&admin), json!(null), "mr").await;
    s.step("mr 5 merged", GET, &format!("{mrs}/5"), Some(&admin), json!(null), "mr").await;
    s.step("pipelines at the end", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.ci_drain().await;
    s.finish();
}
