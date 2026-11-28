//! Pipelines and jobs through GraphQL: a pipeline with its stages, groups and jobs (and their
//! detailed statuses and actions) as it runs, the project's jobs and pipeline counts, and the
//! job and pipeline mutations.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const POST: Method = Method::POST;

const STATUS: &str = "id name label text group icon tooltip hasDetails detailsPath favicon deploymentDetailsPath action { id buttonTitle icon method path title confirmationMessage }";

fn pipeline_query() -> String {
    format!(
        "query($p: ID!, $iid: ID!) {{ project(fullPath: $p) {{ pipeline(iid: $iid) {{
          id iid status active complete cancelable retryable duration queuedDuration startedAt finishedAt
          failedJobsCount totalJobs coverage warnings hasManualActions path refPath source ref sha
          detailedStatus {{ {STATUS} }}
          stages {{ nodes {{ id name status detailedStatus {{ {STATUS} }}
            groups {{ nodes {{ id name size detailedStatus {{ {STATUS} }} jobs {{ nodes {{ id name status }} }} }} }}
            jobs {{ nodes {{ id name status }} }} }} }}
          jobs {{ count nodes {{ id name status retried }} }}
          retriedJobs: jobs(retried: true) {{ nodes {{ id name status }} }}
          manualJobs: jobs(whenExecuted: [\"manual\"]) {{ nodes {{ name }} }}
          job(name: \"build\") {{ id name }}
        }} }} }}"
    )
}

fn jobs_query() -> String {
    format!(
        "query($p: ID!) {{ project(fullPath: $p) {{
          pipelineCounts {{ all finished pending running }}
          pipelines {{ count nodes {{ iid status }} }}
          jobs {{ count nodes {{
            id name status kind active allowFailure browseArtifactsPath canPlayJob cancelable commitPath coverage
            createdAt createdByTag duration erasedAt exitCode failureMessage finishedAt manualJob playPath playable
            queuedAt queuedDuration refName refPath retried retryPath retryable scheduled scheduledAt schedulingType
            shortSha source startedAt stuck tags triggered webPath
            stage {{ id name status }} pipeline {{ id }} runner {{ id description }}
            detailedStatus {{ {STATUS} }}
            artifacts {{ nodes {{ id name fileType size downloadPath expireAt expired }} }}
            trace {{ htmlSummary(lastLines: 10) }}
            userPermissions {{ cancelBuild readBuild readJobArtifacts updateBuild }}
          }} }}
          failed: jobs(statuses: [FAILED]) {{ nodes {{ name }} }}
          withArtifacts: jobs(withArtifacts: true) {{ nodes {{ name }} }}
        }} }}"
    )
}

async fn job_gid(s: &mut Scenario, admin: &str, name: &str) -> String {
    format!("gid://gitlab/Ci::Build/{}", s.ci_job_id(admin, name).await)
}

#[tokio::test]
async fn ci_graphql() {
    let mut s = Scenario::new("ci_graphql").await;
    let admin = s.admin();
    let gpath = s.name("cigq");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = p.body["path_with_namespace"].as_str().unwrap_or("").to_string();
    s.settle().await;
    s.ci_pipeline("ci config", pid, &admin, "main", None, json!([
        {"name": "build", "stage": "build"},
        {"name": "unit", "stage": "test", "coverage": true},
        {"name": "lint", "stage": "test", "allow_failure": true},
        {"name": "deploy", "stage": "deploy", "when": "manual"},
    ])).await;
    let pl = s.ci_pipeline_id();
    let vars = json!({"p": full, "iid": "1"});
    let pq = pipeline_query();
    let jq = jobs_query();
    s.graphql("pipeline created", Some(&admin), &pq, vars.clone()).await;
    s.ci_job_start("build").await;
    s.ci_job_log("build", "line 1\n\u{1b}[32mgreen\u{1b}[0m line 2\nline 3\n").await;
    s.graphql("pipeline running", Some(&admin), &pq, vars.clone()).await;
    s.graphql("jobs running", Some(&admin), &jq, json!({"p": full})).await;
    s.ci_job_artifacts("build", &[("dist/app.txt", "app\n")], None).await;
    s.ci_job_finish("build", "success", None).await;
    s.ci_job_start("unit").await;
    s.ci_job_start("lint").await;
    s.ci_job_log("unit", "Coverage: 90.50%\n").await;
    s.ci_job_finish("unit", "success", Some(90.5)).await;
    s.ci_job_finish("lint", "failed", None).await;
    s.graphql("pipeline with manual", Some(&admin), &pq, vars.clone()).await;
    s.graphql("jobs with manual", Some(&admin), &jq, json!({"p": full})).await;
    let build = job_gid(&mut s, &admin, "build").await;
    s.graphql("one job", Some(&admin), "query($p: ID!, $id: JobID!) { project(fullPath: $p) { job(id: $id) { id name status artifacts { nodes { fileType name } } } } }", json!({"p": full, "id": build})).await;

    // Mutations.
    let deploy = job_gid(&mut s, &admin, "deploy").await;
    // Playing and retrying take any processable; cancelling takes a build.
    let job_mutation = |name: &str| {
        let ty = if name == "jobCancel" { "CiBuildID!" } else { "CiProcessableID!" };
        format!("mutation($id: {ty}) {{ {name}(input: {{id: $id}}) {{ job {{ id name status }} errors }} }}")
    };
    s.graphql("play deploy", Some(&admin), &job_mutation("jobPlay"), json!({"id": deploy})).await;
    s.settle().await;
    s.graphql("cancel deploy", Some(&admin), &job_mutation("jobCancel"), json!({"id": deploy})).await;
    s.settle().await;
    let lint = job_gid(&mut s, &admin, "lint").await;
    s.graphql("retry lint", Some(&admin), &job_mutation("jobRetry"), json!({"id": lint})).await;
    s.settle().await;
    s.graphql("retry lint again", Some(&admin), &job_mutation("jobRetry"), json!({"id": lint})).await;
    s.graphql("pipeline after mutations", Some(&admin), &pq, vars.clone()).await;
    let pgid = format!("gid://gitlab/Ci::Pipeline/{pl}");
    s.graphql("cancel pipeline", Some(&admin), "mutation($id: CiPipelineID!) { pipelineCancel(input: {id: $id}) { errors } }", json!({"id": pgid})).await;
    s.settle().await;
    s.graphql("retry pipeline", Some(&admin), "mutation($id: CiPipelineID!) { pipelineRetry(input: {id: $id}) { pipeline { id status } errors } }", json!({"id": pgid})).await;
    s.settle().await;
    let arts = s
        .graphql("artifacts", Some(&admin), "query($p: ID!, $id: JobID!) { project(fullPath: $p) { job(id: $id) { artifacts { nodes { id fileType } } } } }", json!({"p": full, "id": build}))
        .await;
    let meta: Option<String> = arts.body["data"]["project"]["job"]["artifacts"]["nodes"]
        .as_array()
        .and_then(|a| a.iter().find(|n| n["fileType"] == "METADATA"))
        .and_then(|n| n["id"].as_str().map(str::to_string));
    if let Some(m) = meta {
        s.graphql("destroy metadata", Some(&admin), "mutation($id: CiJobArtifactID!) { artifactDestroy(input: {id: $id}) { artifact { id fileType } errors } }", json!({"id": m})).await;
    }
    s.graphql("destroy artifacts", Some(&admin), "mutation($id: CiBuildID!) { jobArtifactsDestroy(input: {id: $id}) { job { id } destroyedArtifactsCount errors } }", json!({"id": build})).await;
    s.graphql("jobs at the end", Some(&admin), &jq, json!({"p": full})).await;
    s.graphql("destroy pipeline", Some(&admin), "mutation($id: CiPipelineID!) { pipelineDestroy(input: {id: $id}) { errors } }", json!({"id": pgid})).await;
    s.settle().await;
    s.graphql("no pipelines", Some(&admin), "query($p: ID!) { project(fullPath: $p) { pipelines { count } pipelineCounts { all } jobs { count } } }", json!({"p": full})).await;
    let _ = Value::Null;
    s.finish();
}
