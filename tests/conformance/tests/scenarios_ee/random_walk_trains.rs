//! Fixed-seed random walks over merge trains and auto-merge, with up to three open MRs at a time:
//! merge method and target changes, REST and GraphQL train adds (straight on or when checks
//! pass, squashed or not), car pipelines passing and failing, removing cars, pushes to source
//! branches, and retrying failed MR pipelines. Pending pipelines are read back after every
//! operation and picked up by the fake runner, so the walk adapts to what GitLab did.

use super::merge_train_settings::{ci_project, enable_trains};
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const MR: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  state mergeable detailedMergeStatus shouldBeRebased autoMergeEnabled autoMergeStrategy
  availableAutoMergeStrategies mergeTrainIndex squash headPipeline { status ref } } } }";
const ACCEPT: &str = "mutation($path: ID!, $iid: String!, $sha: String!, $strategy: MergeStrategyEnum,
  $squash: Boolean, $squashMsg: String) {
  mergeRequestAccept(input: {projectPath: $path, iid: $iid, sha: $sha, strategy: $strategy,
    squash: $squash, squashCommitMessage: $squashMsg}) {
    errors mergeRequest { state autoMergeEnabled autoMergeStrategy mergeTrainIndex } } }";
const DELETE_CAR: &str = "mutation($id: MergeTrainsCarID!) { mergeTrainsDeleteCar(input: {carId: $id}) { errors } }";

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[tokio::test]
async fn random_walk_trains() {
    walk("random_walk_trains", 0x5a17c0de2f4b9e31, 30).await;
}

#[tokio::test]
async fn random_walk_trains_2() {
    walk("random_walk_trains_2", 0x9e3779b97f4a7c15, 30).await;
}

/// Picks up the project's pending jobs; returns their refs.
async fn pick_pending(s: &mut Scenario, pid: i64, label: &str, running: &mut Vec<String>) {
    s.settle().await;
    let admin = s.admin();
    let r = s.step(&format!("{label} pending"), GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!({"status": "pending", "sort": "asc"}), "pipeline").await;
    let refs: Vec<String> = r.body.as_array().map(|a| a.iter().filter_map(|p| p["ref"].as_str().map(String::from)).collect()).unwrap_or_default();
    if !refs.is_empty() {
        s.ci_pick(refs.len()).await;
        for r in refs {
            if !running.contains(&r) {
                running.push(r);
            }
        }
    }
}

async fn walk(name: &str, seed: u64, steps: usize) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let (pid, full) = ci_project(&mut s, "rwt").await;
    enable_trains(&mut s, pid).await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let trains = format!("/projects/{pid}/merge_trains");

    let mut rng = Rng(seed);
    let mut open: Vec<i64> = vec![];
    let mut next_iid = 0i64;
    let mut running: Vec<String> = vec![];
    let mut n = 0;
    for step in 0..steps {
        // Merged MRs leave the walk.
        let mut still = vec![];
        for iid in open.clone() {
            let r = s.send(GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null)).await;
            if r.body["state"] == "opened" {
                still.push(iid);
            }
        }
        open = still;
        let op = if open.is_empty() { 0 } else { rng.pick(10) };
        let op = if op == 0 && open.len() >= 3 { 1 + rng.pick(9) } else { op };
        let iid = if open.is_empty() { 0 } else { open[rng.pick(open.len())] };
        let label = format!("{step:02} op{op}");
        n += 1;
        let mut target = iid;
        match op {
            0 => {
                next_iid += 1;
                target = next_iid;
                let branch = format!("f{next_iid}");
                let ncommits = 1 + rng.pick(2);
                for k in 0..ncommits {
                    let body = if k == 0 {
                        json!({"branch": branch, "start_branch": "main", "commit_message": format!("Start {branch}"), "actions": [{"action": "create", "file_path": format!("{branch}.txt"), "content": "x\n"}]})
                    } else {
                        json!({"branch": branch, "commit_message": format!("More {branch}"), "actions": [{"action": "create", "file_path": format!("{branch}-2.txt"), "content": "y\n"}]})
                    };
                    s.step(&format!("{label} commit {k}"), POST, &commits, Some(&admin), body, "commit").await;
                    s.settle().await;
                }
                s.step(&format!("{label} new mr"), POST, &mrs, Some(&admin), json!({"source_branch": branch, "target_branch": "main", "title": format!("MR {next_iid}")}), "mr").await;
                s.poll_mr(pid, next_iid).await;
                open.push(next_iid);
            }
            1 => {
                let method = ["merge", "rebase_merge", "ff"][rng.pick(3)];
                s.step(&format!("{label} method {method}"), PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_method": method}), "project").await;
            }
            2 => {
                s.step(&format!("{label} main moves"), POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": format!("Main {n}"), "actions": [{"action": "create", "file_path": format!("main-{n}.txt"), "content": "m\n"}]}), "commit").await;
            }
            3 | 4 => {
                s.poll_mr(pid, iid).await;
                let mut body = json!({});
                if rng.pick(2) == 0 {
                    body["squash"] = json!(true);
                }
                if op == 4 {
                    body["auto_merge"] = json!(true);
                }
                s.step(&format!("{label} add {iid}"), POST, &format!("{trains}/merge_requests/{iid}"), Some(&admin), body, "car").await;
            }
            5 => {
                s.poll_mr(pid, iid).await;
                let m = s.send(GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null)).await;
                let sha = m.body["sha"].as_str().unwrap_or("").to_string();
                let strategy = ["MERGE_TRAIN", "ADD_TO_MERGE_TRAIN_WHEN_CHECKS_PASS"][rng.pick(2)];
                let mut vars = json!({"path": full, "iid": iid.to_string(), "sha": sha, "strategy": strategy});
                if rng.pick(2) == 0 {
                    vars["squash"] = json!(true);
                    vars["squashMsg"] = json!(format!("Squashed {iid} ({n})"));
                }
                s.graphql(&format!("{label} accept {iid}"), Some(&admin), ACCEPT, vars).await;
            }
            6 | 7 => {
                // A running pipeline finishes: mostly passing.
                if running.is_empty() {
                    s.step(&format!("{label} nothing running"), GET, &format!("{trains}/main"), Some(&admin), json!(null), "car").await;
                } else {
                    let r = running.remove(rng.pick(running.len()));
                    let status = if rng.pick(4) == 0 { "failed" } else { "success" };
                    s.ci_finish(&r, status).await;
                    s.settle().await;
                    s.step(&format!("{label} finished {r} {status}"), GET, &format!("{trains}/main"), Some(&admin), json!(null), "car").await;
                }
            }
            8 => {
                // The MR's car leaves the train, or its failed pipeline is retried.
                let car = s.send(GET, &format!("{trains}/merge_requests/{iid}"), Some(&admin), json!(null)).await;
                if car.status == 200 {
                    let id = car.body["id"].as_i64().unwrap_or(0);
                    s.graphql(&format!("{label} delete car {iid}"), Some(&admin), DELETE_CAR, json!({"id": format!("gid://gitlab/MergeTrains::Car/{id}")})).await;
                } else {
                    let m = s.send(GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null)).await;
                    let pl = m.body["head_pipeline"]["id"].as_i64().unwrap_or(0);
                    if m.body["head_pipeline"]["status"] == "failed" {
                        let r = s.step(&format!("{label} retry {iid}"), POST, &format!("/projects/{pid}/pipelines/{pl}/retry"), Some(&admin), json!(null), "pipeline").await;
                        // The retried job is pending in a running pipeline, which the pending
                        // pipelines don't list: the runner takes it now.
                        if r.status == 201 {
                            s.settle().await;
                            s.ci_pick(1).await;
                            let r#ref = m.body["head_pipeline"]["ref"].as_str().unwrap_or("").to_string();
                            if !running.contains(&r#ref) {
                                running.push(r#ref);
                            }
                        }
                    } else {
                        s.step(&format!("{label} cancel auto-merge {iid}"), POST, &format!("{mrs}/{iid}/cancel_merge_when_pipeline_succeeds"), Some(&admin), json!(null), "mr").await;
                    }
                }
            }
            _ => {
                s.step(&format!("{label} push {iid}"), POST, &commits, Some(&admin), json!({"branch": format!("f{iid}"), "commit_message": format!("Push {n}"), "actions": [{"action": "create", "file_path": format!("f{iid}-{n}.txt"), "content": "p\n"}]}), "commit").await;
            }
        }
        pick_pending(&mut s, pid, &label, &mut running).await;
        if target > 0 {
            s.poll_mr(pid, target).await;
            s.step(&format!("{label} mr {target}"), GET, &format!("{mrs}/{target}"), Some(&admin), json!(null), "mr").await;
            s.graphql(&format!("{label} mr {target} graphql"), Some(&admin), MR, json!({"path": full, "iid": target.to_string()})).await;
        }
        s.step(&format!("{label} train"), GET, &format!("{trains}/main"), Some(&admin), json!({"scope": "active"}), "car").await;
    }
    s.step("main commits", GET, &commits, Some(&admin), json!({"ref_name": "main", "per_page": 40}), "commit").await;
    s.ci_drain().await;
    s.finish();
}
