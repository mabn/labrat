//! Job actions through REST: cancelling pending and running jobs, retrying (a new attempt,
//! skipped jobs after it reset), playing manual jobs, erasing, cancelling and retrying whole
//! pipelines, and the errors and permissions around them.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn ci_job_actions() {
    let mut s = Scenario::new("ci_job_actions").await;
    let admin = s.admin();
    let gpath = s.name("cija");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let mut tokens = vec![];
    for (who, level) in [("rep", 20), ("guest", 10)] {
        let username = s.name(who);
        let u = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let t = s.step(&format!("{who} token"), POST, &format!("/users/{u}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} member"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": u, "access_level": level}), "user").await;
        tokens.push(t);
    }
    let (rep, guest) = (tokens[0].clone(), tokens[1].clone());
    let jobs = format!("/projects/{pid}/jobs");

    // A pipeline whose build fails: the test stage is skipped.
    s.ci_pipeline("ci config", pid, &admin, "main", None, json!([
        {"name": "build", "stage": "build"},
        {"name": "test", "stage": "test"},
        {"name": "deploy", "stage": "deploy", "when": "manual"},
    ])).await;
    let pl = s.ci_pipeline_id();
    let base = format!("/projects/{pid}/pipelines/{pl}");
    let build = s.ci_job_id(&admin, "build").await;
    s.step("guest cannot list jobs", GET, &jobs, Some(&guest), json!(null), "job").await;
    s.step("reporter lists jobs", GET, &jobs, Some(&rep), json!(null), "job").await;
    s.step("reporter reads the log", GET, &format!("{jobs}/{build}/trace"), Some(&rep), json!(null), "trace").await;
    s.step("reporter cannot cancel", POST, &format!("{jobs}/{build}/cancel"), Some(&rep), json!(null), "job").await;
    s.step("unknown job", GET, &format!("{jobs}/999999"), Some(&admin), json!(null), "job").await;
    s.step("not retryable", POST, &format!("{jobs}/{build}/retry"), Some(&admin), json!(null), "job").await;
    s.step("not playable", POST, &format!("{jobs}/{build}/play"), Some(&admin), json!(null), "job").await;
    s.step("not erasable", POST, &format!("{jobs}/{build}/erase"), Some(&admin), json!(null), "job").await;
    s.ci_job_start("build").await;
    s.ci_job_log("build", "compiling\nerror: boom\n").await;
    s.ci_job_finish("build", "failed", None).await;
    s.step("failed pipeline", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.step("skipped jobs", GET, &format!("{base}/jobs"), Some(&admin), json!(null), "job").await;
    s.step("failed build", GET, &format!("{jobs}/{build}"), Some(&admin), json!(null), "job").await;

    // Retrying the build: a new attempt, and the skipped jobs are created again.
    let retried = s.step("retry build", POST, &format!("{jobs}/{build}/retry"), Some(&admin), json!(null), "job").await;
    let build2 = retried.body["id"].as_i64().unwrap_or(0);
    s.settle().await;
    s.step("jobs after retry", GET, &format!("{base}/jobs"), Some(&admin), json!(null), "job").await;
    s.step("jobs with retried", GET, &format!("{base}/jobs"), Some(&admin), json!({"include_retried": true}), "job").await;
    s.step("pipeline after retry", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.step("old attempt", GET, &format!("{jobs}/{build}"), Some(&admin), json!(null), "job").await;

    // Cancelling the running build: canceling until the runner reports back.
    s.ci_job_start("build").await;
    s.step("cancel running", POST, &format!("{jobs}/{build2}/cancel"), Some(&admin), json!(null), "job").await;
    s.settle().await;
    s.step("canceling pipeline", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.ci_job_finish("build", "failed", None).await;
    s.step("canceled build", GET, &format!("{jobs}/{build2}"), Some(&admin), json!(null), "job").await;
    s.step("canceled pipeline", GET, &base, Some(&admin), json!(null), "pipeline").await;

    // Retrying the whole pipeline: the build runs and passes, then the test is queued.
    s.step("retry pipeline", POST, &format!("{base}/retry"), Some(&admin), json!(null), "pipeline").await;
    s.settle().await;
    s.step("jobs after pipeline retry", GET, &format!("{base}/jobs"), Some(&admin), json!(null), "job").await;
    s.ci_job_start("build").await;
    s.ci_job_log("build", "compiled\n").await;
    s.ci_job_artifacts("build", &[("out/app.bin", "binary\n")], None).await;
    s.ci_job_finish("build", "success", None).await;
    let test = s.ci_job_id(&admin, "test").await;
    s.step("test pending", GET, &format!("{jobs}/{test}"), Some(&admin), json!(null), "job").await;

    // Cancelling a pending job: canceled at once; the pipeline is canceled.
    s.step("cancel pending", POST, &format!("{jobs}/{test}/cancel"), Some(&admin), json!(null), "job").await;
    s.settle().await;
    s.step("pipeline canceled", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.step("cancel again", POST, &format!("{jobs}/{test}/cancel"), Some(&admin), json!(null), "job").await;

    // Erasing the passed build: no log, no artifacts.
    let build3 = s.ci_job_id(&admin, "build").await;
    s.step("erase", POST, &format!("{jobs}/{build3}/erase"), Some(&admin), json!(null), "job").await;
    s.step("erased log", GET, &format!("{jobs}/{build3}/trace"), Some(&admin), json!(null), "trace").await;
    s.step("erase again", POST, &format!("{jobs}/{build3}/erase"), Some(&admin), json!(null), "job").await;

    // Retrying a passed job, and the test.
    s.step("retry passed", POST, &format!("{jobs}/{build3}/retry"), Some(&admin), json!(null), "job").await;
    s.settle().await;
    s.step("retry canceled test", POST, &format!("{jobs}/{test}/retry"), Some(&admin), json!(null), "job").await;
    s.settle().await;
    s.step("jobs at the end", GET, &format!("{base}/jobs"), Some(&admin), json!({"include_retried": true}), "job").await;
    s.step("cancel pipeline", POST, &format!("{base}/cancel"), Some(&admin), json!(null), "pipeline").await;
    s.settle().await;
    s.step("pipeline at the end", GET, &base, Some(&admin), json!(null), "pipeline").await;
    s.step("all jobs at the end", GET, &jobs, Some(&admin), json!(null), "job").await;
    s.finish();
}
