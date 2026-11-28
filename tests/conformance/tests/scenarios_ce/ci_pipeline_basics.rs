//! A pipeline of three stages, followed through REST and webhooks as its jobs run: the first
//! stage queued and the rest created, a job's log and artifacts, coverage, a failure allowed to
//! fail, and a manual job.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn ci_pipeline_basics() {
    let mut s = Scenario::new("ci_pipeline_basics").await;
    let admin = s.admin();
    let gpath = s.name("cipb");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    if !s.target.recording {
        // The reference instance allows webhooks to local addresses.
        s.send(Method::PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    }
    let mut rx = s.receiver().await;
    let url = rx.url.clone();
    s.step("hook", POST, &format!("/projects/{pid}/hooks"), Some(&admin), json!({"url": url, "pipeline_events": true, "job_events": true, "push_events": false}), "hook").await;

    let jobs = json!([
        {"name": "build", "stage": "build"},
        {"name": "unit", "stage": "test", "coverage": true},
        {"name": "lint", "stage": "test", "allow_failure": true},
        {"name": "deploy", "stage": "deploy", "when": "manual"},
    ]);
    s.ci_pipeline("ci config", pid, &admin, "main", None, jobs).await;
    let pl = s.ci_pipeline_id();
    let pls = format!("/projects/{pid}/pipelines");
    let base = format!("{pls}/{pl}");
    s.step("pipeline created", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.step("jobs created", GET, &format!("{base}/jobs"), Some(&admin), json!(null), "job").await;
    s.step("pipelines", GET, &pls, Some(&admin), json!(null), "pipeline").await;
    s.deliveries("created hooks", &mut rx, 6).await;

    s.ci_job_start("build").await;
    s.step("pipeline running", GET, &base, Some(&admin), json!(null), "pipeline").await;
    let jobs = s.step("project jobs", GET, &format!("/projects/{pid}/jobs"), Some(&admin), json!(null), "job").await;
    let build = jobs.body.as_array().and_then(|a| a.iter().find(|j| j["name"] == "build")).and_then(|j| j["id"].as_i64()).unwrap_or(0);
    s.step("build running", GET, &format!("/projects/{pid}/jobs/{build}"), Some(&admin), json!(null), "job").await;
    s.step("build trace empty", GET, &format!("/projects/{pid}/jobs/{build}/trace"), Some(&admin), json!(null), "trace").await;
    s.ci_job_log("build", "\u{1b}[0KRunning build\nbuilt app\n").await;
    s.step("build trace", GET, &format!("/projects/{pid}/jobs/{build}/trace"), Some(&admin), json!(null), "trace").await;
    s.ci_job_artifacts("build", &[("dist/app.txt", "app v1\n"), ("dist/docs/readme.md", "# App\n")], Some("1 week")).await;
    s.ci_job_finish("build", "success", None).await;
    s.step("build done", GET, &format!("/projects/{pid}/jobs/{build}"), Some(&admin), json!(null), "job").await;
    s.step("jobs after build", GET, &format!("{base}/jobs"), Some(&admin), json!(null), "job").await;
    s.deliveries("build hooks", &mut rx, 5).await;

    s.ci_job_start("unit").await;
    s.ci_job_start("lint").await;
    s.ci_job_log("unit", "Coverage: 81.25%\n").await;
    s.ci_job_finish("unit", "success", Some(81.25)).await;
    s.ci_job_log("lint", "lint failed\n").await;
    s.ci_job_finish("lint", "failed", None).await;
    s.step("pipeline passed with warnings", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.step("jobs after test", GET, &format!("{base}/jobs"), Some(&admin), json!(null), "job").await;
    s.step("failed jobs", GET, &format!("{base}/jobs"), Some(&admin), json!({"scope[]": "failed"}), "job").await;
    s.step("manual jobs", GET, &format!("/projects/{pid}/jobs"), Some(&admin), json!({"scope": "manual"}), "job").await;
    s.step("latest", GET, &format!("{pls}/latest"), Some(&admin), json!(null), "pipeline").await;
    s.step("commit", GET, &format!("/projects/{pid}/repository/commits/main"), Some(&admin), json!(null), "commit").await;
    s.step("statuses", GET, &format!("/projects/{pid}/repository/commits/main/statuses"), Some(&admin), json!(null), "status").await;
    s.deliveries("test hooks", &mut rx, 5).await;

    // The manual job is played, runs and passes.
    let deploy = s
        .step("deploy job", GET, &format!("{base}/jobs"), Some(&admin), json!({"scope": "manual"}), "job")
        .await
        .body
        .as_array()
        .and_then(|a| a.first())
        .and_then(|j| j["id"].as_i64())
        .unwrap_or(0);
    s.step("play deploy", POST, &format!("/projects/{pid}/jobs/{deploy}/play"), Some(&admin), json!(null), "job").await;
    s.settle().await;
    s.step("pipeline after play", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.ci_job_start("deploy").await;
    s.ci_job_log("deploy", "deployed\n").await;
    s.ci_job_finish("deploy", "success", None).await;
    s.step("pipeline done", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.step("deploy done", GET, &format!("/projects/{pid}/jobs/{deploy}"), Some(&admin), json!(null), "job").await;
    s.deliveries("deploy hooks", &mut rx, 5).await;
    s.finish();
}
