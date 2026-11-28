//! Stage and job detailed statuses beyond plain CI jobs: pipelines made only of commit statuses
//! (their `external` stage and generic jobs), commit statuses added to a CI pipeline, and a
//! blocked stage whose manual job asks for confirmation, seen by a developer and a reporter.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

const STATUS: &str = "id name label text group icon tooltip hasDetails detailsPath favicon deploymentDetailsPath action { id buttonTitle icon method path title confirmationMessage }";

fn pipeline_query() -> String {
    format!(
        "query($p: ID!, $iid: ID!) {{ project(fullPath: $p) {{ pipeline(iid: $iid) {{
          status hasManualActions warnings totalJobs failedJobsCount
          detailedStatus {{ {STATUS} }}
          stages {{ nodes {{ id name status detailedStatus {{ {STATUS} }}
            groups {{ nodes {{ id name size detailedStatus {{ {STATUS} }} jobs {{ nodes {{ id name status }} }} }} }}
            jobs {{ nodes {{ id name status }} }} }} }}
          jobs {{ count nodes {{
            id name status kind active allowFailure cancelable retryable playable manualJob retried
            coverage duration queuedDuration startedAt finishedAt createdAt refName shortSha tags webPath
            stage {{ name status }}
            detailedStatus {{ {STATUS} }}
          }} }}
          extJob: job(name: \"ext-a\") {{ id name status }}
        }} }} }}"
    )
}

#[tokio::test]
async fn ci_stage_statuses() {
    let mut s = Scenario::new("ci_stage_statuses").await;
    let admin = s.admin();
    let gpath = s.name("cist");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let username = s.name("rita");
    let rita = s
        .step("reporter", POST, "/users", Some(&admin), json!({"username": username, "name": "Rita", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let rita_t = s.step("reporter token", POST, &format!("/users/{rita}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();

    // A pipeline made only of commit statuses.
    let ext = s.step("external project", POST, "/projects", Some(&admin), json!({"name": "ext", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public"}), "project").await;
    let ext_id = ext.id();
    let ext_full = ext.body["path_with_namespace"].as_str().unwrap_or("").to_string();
    s.settle().await;
    let sha = s.step("main", GET, &format!("/projects/{ext_id}/repository/branches/main"), Some(&admin), json!({}), "branch").await.body["commit"]["id"].as_str().unwrap().to_string();
    let statuses = format!("/projects/{ext_id}/statuses/{sha}");
    s.step("ext-a running", POST, &statuses, Some(&admin), json!({"state": "running", "name": "ext-a", "target_url": "https://ci.example.com/a", "description": "Checking things"}), "status").await;
    s.step("ext-b success", POST, &statuses, Some(&admin), json!({"state": "success", "name": "ext-b"}), "status").await;
    s.step("ext-c pending", POST, &statuses, Some(&admin), json!({"state": "pending", "name": "ext-c", "target_url": "https://ci.example.com/c"}), "status").await;
    s.settle().await;
    let vars = json!({"p": ext_full, "iid": "1"});
    let pq = pipeline_query();
    s.graphql("external running", Some(&admin), &pq, vars.clone()).await;
    s.graphql("external running as non-member", Some(&rita_t), &pq, vars.clone()).await;
    s.step("ext-a failed", POST, &statuses, Some(&admin), json!({"state": "failed", "name": "ext-a", "target_url": "https://ci.example.com/a"}), "status").await;
    s.step("ext-c canceled", POST, &statuses, Some(&admin), json!({"state": "canceled", "name": "ext-c"}), "status").await;
    s.settle().await;
    s.graphql("external finished", Some(&admin), &pq, vars.clone()).await;

    // A CI pipeline with a blocked stage, and then a commit status.
    let p = s.step("ci project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = p.body["path_with_namespace"].as_str().unwrap_or("").to_string();
    s.step("reporter member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": rita, "access_level": 20}), "member").await;
    s.settle().await;
    let commit = s
        .ci_pipeline("ci config", pid, &admin, "main", None, json!([
            {"name": "build", "stage": "build"},
            {"name": "deploy", "stage": "deploy", "when": "manual", "allow_failure": false, "manual_confirmation": "Deploy to production?"},
            {"name": "notify", "stage": "deploy", "when": "manual"},
            {"name": "cleanup", "stage": "post"},
        ]))
        .await;
    let ci_sha = commit.body["id"].as_str().unwrap().to_string();
    let pl = s.ci_pipeline_id();
    let vars = json!({"p": full, "iid": "1"});
    s.ci_job_start("build").await;
    s.ci_job_finish("build", "success", None).await;
    s.graphql("blocked", Some(&admin), &pq, vars.clone()).await;
    s.graphql("blocked as reporter", Some(&rita_t), &pq, vars.clone()).await;
    let ci_statuses = format!("/projects/{pid}/statuses/{ci_sha}");
    s.step("status on ci pipeline", POST, &ci_statuses, Some(&admin), json!({"state": "running", "name": "ext-a", "pipeline_id": pl, "target_url": "https://ci.example.com/x"}), "status").await;
    s.settle().await;
    s.graphql("mixed running", Some(&admin), &pq, vars.clone()).await;
    s.step("mixed pipeline", GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&admin), json!({}), "pipeline").await;
    s.step("mixed jobs", GET, &format!("/projects/{pid}/pipelines/{pl}/jobs"), Some(&admin), json!({}), "job").await;
    s.step("mixed statuses", GET, &format!("/projects/{pid}/repository/commits/{ci_sha}/statuses"), Some(&admin), json!({}), "status").await;
    s.step("status success", POST, &ci_statuses, Some(&admin), json!({"state": "success", "name": "ext-a", "pipeline_id": pl}), "status").await;
    s.settle().await;
    s.graphql("mixed finished", Some(&admin), &pq, vars.clone()).await;
    s.step("mixed pipeline finished", GET, &format!("/projects/{pid}/pipelines/{pl}"), Some(&admin), json!({}), "pipeline").await;
    s.finish();
}
