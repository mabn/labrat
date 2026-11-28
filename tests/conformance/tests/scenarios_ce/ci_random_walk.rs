//! Fixed-seed random walks over CI pipelines: the runner starts, logs, uploads artifacts for and
//! finishes jobs; an admin and a developer cancel, retry, play and erase jobs and pipelines
//! through REST and GraphQL; commit statuses join the pipeline; new pipelines replace the old
//! one on the protected `main` or on `feature`. Webhook deliveries (pipeline and job hooks) are
//! compared after every step, and the pipeline is read back now and then.
//!
//! Every choice depends only on the seed and on what the API answers, which is the same on
//! both targets: "start" takes the oldest pending job, as GitLab's runner queue does. Before a
//! new pipeline, and at the end, the current one's jobs are cancelled and its running jobs
//! finished, so no job is left running on the reference.
//!
//! One GitLab race is out of reach: cancelling a whole pipeline of pending jobs processes it in
//! the background after each job, so it may pass through `running` (in its hooks and the
//! cancel answer) or not. labrat never does, so the walk doesn't cancel a pipeline whose jobs
//! are waiting with none running.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const DELETE: Method = Method::DELETE;

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
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

#[tokio::test]
async fn ci_random_walk() {
    walk("ci_random_walk", 0x2545f4914f6cdd1d, 45).await;
}

/// Another seed, longer.
#[tokio::test]
async fn ci_random_walk_2() {
    walk("ci_random_walk_2", 0x9e3779b97f4a7c15, 60).await;
}

/// Further seeds, added while walks kept finding differences.
#[tokio::test]
async fn ci_random_walk_3() {
    walk("ci_random_walk_3", 0xd6e8feb86659fd93, 60).await;
}

#[tokio::test]
async fn ci_random_walk_4() {
    walk("ci_random_walk_4", 0xa0761d6478bd642f, 60).await;
}

#[tokio::test]
async fn ci_random_walk_5() {
    walk("ci_random_walk_5", 0xe7037ed1a0b428db, 60).await;
}

#[tokio::test]
async fn ci_random_walk_6() {
    walk("ci_random_walk_6", 0x8ebc6af09c88c6e3, 60).await;
}

#[tokio::test]
async fn ci_random_walk_7() {
    walk("ci_random_walk_7", 0x589965cc75374cc3, 60).await;
}

#[tokio::test]
async fn ci_random_walk_8() {
    walk("ci_random_walk_8", 0x1d8e4e27c47d124f, 60).await;
}

#[tokio::test]
async fn ci_random_walk_9() {
    walk("ci_random_walk_9", 0xc2b2ae3d27d4eb4f, 60).await;
}

#[tokio::test]
async fn ci_random_walk_10() {
    walk("ci_random_walk_10", 0x27d4eb2f165667c5, 60).await;
}

#[tokio::test]
async fn ci_random_walk_11() {
    walk("ci_random_walk_11", 0x94d049bb133111eb, 60).await;
}

#[tokio::test]
async fn ci_random_walk_12() {
    walk("ci_random_walk_12", 0xbf58476d1ce4e5b9, 60).await;
}

#[tokio::test]
async fn ci_random_walk_13() {
    walk("ci_random_walk_13", 0x7fb5d329728ea185, 60).await;
}

#[tokio::test]
async fn ci_random_walk_14() {
    walk("ci_random_walk_14", 0x81dadef4bc2dd44d, 60).await;
}

#[tokio::test]
async fn ci_random_walk_15() {
    walk("ci_random_walk_15", 0xd2b74407b1ce6e93, 60).await;
}

const STATUS: &str = "id name label tooltip hasDetails detailsPath action { id title path confirmationMessage }";

fn pipeline_query() -> String {
    format!(
        "query($p: ID!, $iid: ID!) {{ project(fullPath: $p) {{ pipeline(iid: $iid) {{
          status active complete cancelable retryable warnings hasManualActions totalJobs failedJobsCount coverage
          detailedStatus {{ {STATUS} }}
          userPermissions {{ updatePipeline cancelPipeline }}
          stages {{ nodes {{ name status detailedStatus {{ {STATUS} }}
            groups {{ nodes {{ name size detailedStatus {{ {STATUS} }} }} }} }} }}
          jobs {{ nodes {{ id name status retried allowFailure playable retryable cancelable canPlayJob manualJob
            detailedStatus {{ {STATUS} }} }} }}
        }} }} }}"
    )
}

/// A random pipeline definition: a few stages with ordinary, allowed-to-fail, manual and
/// `on_failure`/`always` jobs.
fn random_jobs(rng: &mut Rng, n: usize) -> Value {
    let mut jobs = vec![json!({"name": "build", "stage": "build"})];
    if rng.chance(60) {
        jobs.push(json!({"name": format!("compile{n}"), "stage": "build", "allow_failure": rng.chance(30)}));
    }
    jobs.push(json!({"name": "unit", "stage": "test"}));
    if rng.chance(70) {
        jobs.push(json!({"name": "lint", "stage": "test", "allow_failure": rng.chance(50)}));
    }
    if rng.chance(60) {
        let mut d = json!({"name": "deploy", "stage": "deploy", "when": "manual", "allow_failure": rng.chance(50)});
        if rng.chance(40) {
            d["manual_confirmation"] = json!("Deploy now?");
        }
        jobs.push(d);
    }
    match rng.pick(3) {
        0 => jobs.push(json!({"name": "notify", "stage": "post", "when": "on_failure"})),
        1 => jobs.push(json!({"name": "cleanup", "stage": "post", "when": "always"})),
        _ => {}
    }
    Value::Array(jobs)
}

/// The current pipeline's jobs (latest attempts first, as the API lists them): `(id, name,
/// status)`, without recording the request.
async fn jobs(s: &mut Scenario, pid: i64, pl: i64, admin: &str) -> Vec<(i64, String, String)> {
    let r = s.send(GET, &format!("/projects/{pid}/pipelines/{pl}/jobs"), Some(admin), json!({"per_page": 100, "include_retried": true})).await;
    r.body
        .as_array()
        .map(|a| {
            a.iter()
                .map(|j| (j["id"].as_i64().unwrap_or(0), j["name"].as_str().unwrap_or("").to_string(), j["status"].as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Cancels the current pipeline's unfinished jobs one at a time and finishes its running ones,
/// so none is left behind. (Cancelling the whole pipeline at once races with GitLab's
/// background processing: a pipeline of pending jobs may show `running` for a moment, or not.)
async fn wind_down(s: &mut Scenario, pid: i64, pl: i64, admin: &str, label: &str) {
    let mut seen: Vec<String> = vec![];
    for (id, name, status) in jobs(s, pid, pl, admin).await {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name.clone());
        if matches!(status.as_str(), "created" | "pending" | "running" | "manual" | "scheduled") {
            s.step(&format!("{label} cancel {name}"), POST, &format!("/projects/{pid}/jobs/{id}/cancel"), Some(admin), json!(null), "job").await;
            s.settle().await;
        }
    }
    for (_, name, status) in jobs(s, pid, pl, admin).await {
        if matches!(status.as_str(), "running" | "canceling") {
            s.ci_job_finish(&name, "failed", None).await;
        }
    }
    s.settle().await;
}

async fn walk(name: &str, seed: u64, steps: usize) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let gpath = s.name("cirw");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = p.body["path_with_namespace"].as_str().unwrap_or("").to_string();
    s.settle().await;
    let username = s.name("dev");
    let dev = s
        .step("developer", POST, "/users", Some(&admin), json!({"username": username, "name": "Dev", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let dev_t = s.step("developer token", POST, &format!("/users/{dev}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("developer member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "member").await;
    if !s.target.recording {
        // The reference instance allows webhooks to local addresses.
        s.send(Method::PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    }
    let mut rx = s.receiver().await;
    let url = rx.url.clone();
    s.step("hook", POST, &format!("/projects/{pid}/hooks"), Some(&admin), json!({"url": url, "pipeline_events": true, "job_events": true, "push_events": false}), "hook").await;

    let mut rng = Rng(seed);
    let users = [("admin", admin.clone()), ("dev", dev_t.clone())];
    let mut pipelines = 1;
    let mut feature = false;
    let jobs0 = random_jobs(&mut rng, 0);
    let mut branch = "main";
    s.ci_pipeline("pipeline 1", pid, &admin, branch, None, jobs0).await;
    s.deliveries("pipeline 1 hooks", &mut rx, 0).await;
    let mut pl = s.ci_pipeline_id();
    let mut iid = 1;
    let mut sha = s.send(GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or("").to_string();
    let mut uploaded: Vec<i64> = vec![];
    let mut statuses = 0;
    let pq = pipeline_query();

    for step in 0..steps {
        let (who, tok) = users[rng.pick(users.len())].clone();
        let all = jobs(&mut s, pid, pl, &admin).await;
        // The latest attempt of each name.
        let mut latest: Vec<(i64, String, String)> = vec![];
        for j in &all {
            if !latest.iter().any(|l| l.1 == j.1) {
                latest.push(j.clone());
            }
        }
        let with = |st: &[&str]| -> Vec<(i64, String, String)> { latest.iter().filter(|j| st.contains(&j.2.as_str())).cloned().collect() };
        let running = with(&["running"]);
        let op = rng.pick(20);
        let label = format!("{step:02} {who} op{op}");
        match op {
            // The runner takes the oldest pending job.
            0..=3 => {
                let mut pending = with(&["pending"]);
                pending.sort_by_key(|j| j.0);
                match pending.first() {
                    Some((_, n, _)) => {
                        let n = n.clone();
                        s.ci_job_start(&n).await;
                    }
                    None => continue,
                }
            }
            4 if !running.is_empty() => {
                let (_, n, _) = running[rng.pick(running.len())].clone();
                s.ci_job_log(&n, &format!("step {step}: \u{1b}[32mok\u{1b}[0m\n")).await;
            }
            5..=7 if !running.is_empty() => {
                let (_, n, _) = running[rng.pick(running.len())].clone();
                let status = if rng.chance(65) { "success" } else { "failed" };
                // GitLab reads coverage from the log; the logs here have none.
                s.ci_job_finish(&n, status, None).await;
            }
            8 if !running.is_empty() => {
                let (id, n, _) = running[rng.pick(running.len())].clone();
                if uploaded.contains(&id) {
                    continue;
                }
                uploaded.push(id);
                let expire = ["1 day", "1 week"][rng.pick(2)];
                s.ci_job_artifacts(&n, &[("out/report.txt", &format!("step {step}\n")), ("out/log.txt", "log\n")], Some(expire)).await;
            }
            // Job actions through REST.
            9 | 10 => {
                let Some((id, _, _)) = latest.get(rng.pick(latest.len().max(1))).cloned() else { continue };
                let action = ["retry", "cancel", "play", "erase"][rng.pick(4)];
                s.step(&label, POST, &format!("/projects/{pid}/jobs/{id}/{action}"), Some(&tok), json!(null), "job").await;
            }
            // Job actions through GraphQL.
            11 => {
                let Some((id, _, _)) = latest.get(rng.pick(latest.len().max(1))).cloned() else { continue };
                let (m, ty) = [("jobRetry", "CiProcessableID!"), ("jobPlay", "CiProcessableID!"), ("jobCancel", "CiBuildID!")][rng.pick(3)];
                let q = format!("mutation($id: {ty}) {{ {m}(input: {{id: $id}}) {{ job {{ name status }} errors }} }}");
                s.graphql(&label, Some(&tok), &q, json!({"id": format!("gid://gitlab/Ci::Build/{id}")})).await;
            }
            // Pipeline actions.
            12 => {
                // Cancelling a pipeline whose jobs are waiting but none running hits GitLab's race
                // (see the module docs), so the walk skips it.
                let racy = running.is_empty() && !with(&["pending", "created"]).is_empty();
                if rng.chance(50) {
                    let action = ["retry", "cancel"][rng.pick(2)];
                    if action == "cancel" && racy {
                        continue;
                    }
                    s.step(&label, POST, &format!("/projects/{pid}/pipelines/{pl}/{action}"), Some(&tok), json!(null), "pipeline").await;
                } else {
                    let m = ["pipelineRetry", "pipelineCancel"][rng.pick(2)];
                    if m == "pipelineCancel" && racy {
                        continue;
                    }
                    let q = format!("mutation($id: CiPipelineID!) {{ {m}(input: {{id: $id}}) {{ errors }} }}");
                    s.graphql(&label, Some(&tok), &q, json!({"id": format!("gid://gitlab/Ci::Pipeline/{pl}")})).await;
                }
            }
            // Artifacts.
            13 => {
                let Some((id, _, _)) = latest.get(rng.pick(latest.len().max(1))).cloned() else { continue };
                if rng.chance(50) {
                    s.step(&label, POST, &format!("/projects/{pid}/jobs/{id}/artifacts/keep"), Some(&tok), json!(null), "job").await;
                } else {
                    s.step(&label, DELETE, &format!("/projects/{pid}/jobs/{id}/artifacts"), Some(&tok), json!(null), "job").await;
                }
            }
            // A commit status joins the pipeline.
            14 => {
                statuses += 1;
                let state = ["pending", "running", "success", "failed", "canceled"][rng.pick(5)];
                let name = format!("ext-{}", rng.pick(2));
                s.step(&label, POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": state, "name": name, "pipeline_id": pl, "description": format!("Status {statuses}")}), "status").await;
            }
            // Reads.
            15 => {
                s.graphql(&label, Some(&tok), &pq, json!({"p": full, "iid": iid.to_string()})).await;
            }
            16 => {
                s.step(&label, GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&tok), json!(null), "pipeline").await;
                let scope = ["pending", "running", "success", "failed", "canceled", "manual"][rng.pick(6)];
                s.step(&format!("{label} jobs"), GET, &format!("/projects/{pid}/pipelines/{pl}/jobs"), Some(&tok), json!({"scope": scope, "include_retried": rng.chance(50)}), "job").await;
            }
            17 => {
                let Some((id, _, _)) = latest.get(rng.pick(latest.len().max(1))).cloned() else { continue };
                s.step(&label, GET, &format!("/projects/{pid}/jobs/{id}"), Some(&tok), json!(null), "job").await;
            }
            // A new pipeline replaces the current one.
            18 => {
                wind_down(&mut s, pid, pl, &admin, &label).await;
                pipelines += 1;
                let start = if rng.chance(50) {
                    let start = (!feature).then_some("main");
                    feature = true;
                    branch = "feature";
                    start
                } else {
                    branch = "main";
                    None
                };
                let spec = random_jobs(&mut rng, pipelines);
                s.ci_pipeline(&format!("{label} pipeline {pipelines} on {branch}"), pid, &admin, branch, start, spec).await;
                pl = s.ci_pipeline_id();
                iid = pipelines;
                sha = s.send(GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or("").to_string();
                uploaded.clear();
            }
            _ => {
                s.graphql(
                    &label,
                    Some(&tok),
                    "query($p: ID!) { project(fullPath: $p) { pipelineCounts { all finished running pending } jobs(first: 20) { nodes { name status detailedStatus { label } } } } }",
                    json!({"p": full}),
                )
                .await;
            }
        }
        s.settle().await;
        s.deliveries(&format!("{label} hooks"), &mut rx, 0).await;
    }
    s.graphql("final pipeline", Some(&admin), &pq, json!({"p": full, "iid": iid.to_string()})).await;
    wind_down(&mut s, pid, pl, &admin, "final").await;
    s.deliveries("final hooks", &mut rx, 0).await;
    s.step("final pipelines", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("final jobs", GET, &format!("/projects/{pid}/jobs"), Some(&admin), json!({"per_page": 100}), "job").await;
    s.finish();
}
