//! GraphQL query complexity of CI queries: pipelines with their stages, groups and jobs,
//! project jobs, detailed statuses and actions, and a query over the limit. Each query also
//! asks for `queryComplexity { score limit }`. The queries run as a developer, so they also
//! show that a developer can't change jobs of a protected branch's pipeline; REST and the
//! mutations refuse it too, and allow it for an unprotected branch.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn ci_complexity() {
    let mut s = Scenario::new("ci_complexity").await;
    let admin = s.admin();
    let gpath = s.name("cicx");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = p.body["path_with_namespace"].as_str().unwrap_or("").to_string();
    let username = s.name("dave");
    let dave = s
        .step("developer", POST, "/users", Some(&admin), json!({"username": username, "name": "Dave", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let dave_t = s.step("developer token", POST, &format!("/users/{dave}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("developer member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dave, "access_level": 30}), "member").await;
    s.settle().await;
    let commit = s
        .ci_pipeline("ci config", pid, &admin, "main", None, json!([
            {"name": "build", "stage": "build"},
            {"name": "test", "stage": "test"},
            {"name": "deploy", "stage": "deploy", "when": "manual"},
        ]))
        .await;
    let sha = commit.body["id"].as_str().unwrap().to_string();
    s.ci_job_start("build").await;
    s.ci_job_finish("build", "success", None).await;
    let build = format!("gid://gitlab/Ci::Build/{}", s.ci_job_id(&admin, "build").await);

    let qc = "queryComplexity { score limit }";
    let st = "detailedStatus { id name label icon tooltip hasDetails detailsPath action { id path title buttonTitle method icon } }";
    let pr = "project(fullPath: $p)";
    let cases: Vec<(&str, String)> = vec![
        ("pipeline by iid", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ id status }} }} }}")),
        ("pipeline by sha", format!("query($p: ID!, $sha: String!) {{ {qc} {pr} {{ pipeline(sha: $sha) {{ id status }} }} }}")),
        ("latest pipeline", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline {{ id status }} }} }}")),
        ("pipeline status", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ {st} }} }} }}")),
        ("pipeline scalars", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ active complete cancelable retryable duration queuedDuration coverage warnings hasManualActions totalJobs failedJobsCount path refPath latest refText beforeSha }} }} }}")),
        ("stages", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ stages {{ nodes {{ id name status {st} }} }} }} }} }}")),
        ("stages first 2", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ stages(first: 2) {{ nodes {{ name }} }} }} }} }}")),
        ("groups", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ stages {{ nodes {{ name groups {{ nodes {{ id name size {st} jobs {{ nodes {{ id name {st} }} }} }} }} }} }} }} }} }}")),
        ("stage jobs", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ stages {{ nodes {{ jobs {{ nodes {{ name status }} }} }} }} }} }} }}")),
        ("pipeline jobs", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ jobs {{ count nodes {{ id name status canPlayJob playable }} }} }} }} }}")),
        ("pipeline jobs filtered", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ jobs(first: 5, retried: false, statuses: [SUCCESS]) {{ nodes {{ name }} }} }} }} }}")),
        ("pipeline job", format!("query($p: ID!) {{ {qc} {pr} {{ pipeline(iid: \"1\") {{ job(name: \"build\") {{ id name stage {{ name }} }} jobArtifacts {{ name fileType }} }} }} }}")),
        ("pipelines with stages", format!("query($p: ID!) {{ {qc} {pr} {{ pipelines(first: 10) {{ nodes {{ iid status stages {{ nodes {{ name status }} }} }} }} }} }}")),
        ("project jobs", format!("query($p: ID!) {{ {qc} {pr} {{ jobs {{ count nodes {{ id name status duration {st} }} pageInfo {{ hasNextPage endCursor }} }} }} }}")),
        ("project jobs first 10", format!("query($p: ID!) {{ {qc} {pr} {{ jobs(first: 10, statuses: [SUCCESS, MANUAL]) {{ nodes {{ name artifacts {{ nodes {{ fileType downloadPath }} }} trace {{ htmlSummary }} runner {{ id }} pipeline {{ id }} }} }} }} }}")),
        ("project job", format!("query($p: ID!, $id: JobID!) {{ {qc} {pr} {{ job(id: $id) {{ id name status canPlayJob userPermissions {{ readBuild updateBuild cancelBuild }} }} }} }}")),
        ("pipeline counts", format!("query($p: ID!) {{ {qc} {pr} {{ pipelineCounts {{ all running pending finished }} }} }}")),
        ("pipeline counts by ref", format!("query($p: ID!) {{ {qc} {pr} {{ pipelineCounts(ref: \"main\") {{ all }} }} }}")),
        (
            "over the limit",
            format!(
                "query($p: ID!) {{ {qc} {pr} {{ pipelines {{ nodes {{ stages {{ nodes {{ {st} groups {{ nodes {{ {st} jobs {{ nodes {{ {st} }} }} }} }} }} }} }} }} }} }}"
            ),
        ),
    ];
    for (label, q) in &cases {
        let mut vars = json!({"p": full});
        if q.contains("$sha") {
            vars["sha"] = json!(sha);
        }
        if q.contains("$id") {
            vars["id"] = json!(build);
        }
        s.graphql(&format!("{label} as developer"), Some(&dave_t), q, vars).await;
    }
    s.graphql("over the limit as admin", Some(&admin), &cases.last().unwrap().1, json!({"p": full})).await;
    let big = format!(
        "query($p: ID!) {{ {qc} {pr} {{ pipelines {{ nodes {{ {st} stages {{ nodes {{ {st} groups {{ nodes {{ {st} jobs {{ nodes {{ {st} }} }} }} }} }} }} }} }} }} }}"
    );
    s.graphql("really over the limit as developer", Some(&dave_t), &big, json!({"p": full})).await;
    s.graphql("really over the limit as admin", Some(&admin), &big, json!({"p": full})).await;

    // The developer and the protected main branch.
    let pl = s.ci_pipeline_id();
    let test = s.ci_job_id(&admin, "test").await;
    let build_id = s.ci_job_id(&admin, "build").await;
    let deploy = s.ci_job_id(&admin, "deploy").await;
    let jobs = format!("/projects/{pid}/jobs");
    s.step("developer retries on main", POST, &format!("{jobs}/{build_id}/retry"), Some(&dave_t), json!({}), "job").await;
    s.step("developer cancels on main", POST, &format!("{jobs}/{test}/cancel"), Some(&dave_t), json!({}), "job").await;
    s.step("developer plays on main", POST, &format!("{jobs}/{deploy}/play"), Some(&dave_t), json!({}), "job").await;
    s.step("developer retries the pipeline", POST, &format!("/projects/{pid}/pipelines/{pl}/retry"), Some(&dave_t), json!({}), "pipeline").await;
    s.step("developer cancels the pipeline", POST, &format!("/projects/{pid}/pipelines/{pl}/cancel"), Some(&dave_t), json!({}), "pipeline").await;
    s.graphql("developer jobRetry on main", Some(&dave_t), "mutation($id: CiProcessableID!) { jobRetry(input: {id: $id}) { job { id } errors } }", json!({"id": build})).await;
    let pgid = format!("gid://gitlab/Ci::Pipeline/{pl}");
    s.graphql("developer pipelineCancel on main", Some(&dave_t), "mutation($id: CiPipelineID!) { pipelineCancel(input: {id: $id}) { errors } }", json!({"id": pgid})).await;
    s.graphql(
        "developer permissions on main",
        Some(&dave_t),
        "query($p: ID!) { project(fullPath: $p) { pipeline(iid: \"1\") { userPermissions { updatePipeline cancelPipeline destroyPipeline } jobs { nodes { name canPlayJob userPermissions { updateBuild cancelBuild } } } } } }",
        json!({"p": full}),
    )
    .await;
    s.step("admin cancels the pipeline", POST, &format!("/projects/{pid}/pipelines/{pl}/cancel"), Some(&admin), json!({}), "pipeline").await;
    s.settle().await;

    // An unprotected branch.
    s.ci_pipeline("feature config", pid, &admin, "feature", Some("main"), json!([{"name": "lint", "stage": "test"}])).await;
    let pl2 = s.ci_pipeline_id();
    let lint = s.ci_job_id(&admin, "lint").await;
    s.graphql(
        "developer permissions on feature",
        Some(&dave_t),
        "query($p: ID!) { project(fullPath: $p) { pipeline(iid: \"2\") { userPermissions { updatePipeline cancelPipeline } jobs { nodes { name canPlayJob userPermissions { updateBuild cancelBuild } detailedStatus { action { title } } } } } } }",
        json!({"p": full}),
    )
    .await;
    s.step("developer cancels on feature", POST, &format!("{jobs}/{lint}/cancel"), Some(&dave_t), json!({}), "job").await;
    s.settle().await;
    s.step("developer retries the feature pipeline", POST, &format!("/projects/{pid}/pipelines/{pl2}/retry"), Some(&dave_t), json!({}), "pipeline").await;
    s.settle().await;
    s.step("admin cancels the feature pipeline", POST, &format!("/projects/{pid}/pipelines/{pl2}/cancel"), Some(&admin), json!({}), "pipeline").await;
    s.settle().await;
    s.finish();
}
