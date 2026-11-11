//! Creating pipelines in a project without CI configuration (REST `POST /pipeline`, GraphQL
//! `pipelineCreate` and `mergeRequestCreatePipeline`), and how project settings that only
//! matter with CI are accepted and echoed.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn pipeline_create_settings() {
    let mut s = Scenario::new("pipeline_create_settings").await;
    let admin = s.admin();
    let gpath = s.name("pc");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "f", "actions": [{"action": "create", "file_path": "f.txt", "content": "x\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("rest pipeline", POST, &format!("/projects/{pid}/pipeline"), Some(&admin), json!({"ref": "main"}), "pipeline").await;
    s.step("rest pipeline unknown ref", POST, &format!("/projects/{pid}/pipeline"), Some(&admin), json!({"ref": "nope"}), "pipeline").await;
    s.step("rest pipeline no ref", POST, &format!("/projects/{pid}/pipeline"), Some(&admin), json!({}), "pipeline").await;
    s.graphql("gql pipelineCreate", Some(&admin), "mutation($p: ID!) { pipelineCreate(input: {projectPath: $p, ref: \"main\"}) { errors pipeline { iid } } }", json!({"p": full})).await;
    s.step("pipelines", GET, &format!("/projects/{pid}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step(
        "ci settings",
        PUT,
        &format!("/projects/{pid}"),
        Some(&admin),
        json!({
            "ci_config_path": "ci/build.yml", "ci_default_git_depth": 7, "ci_forward_deployment_enabled": false,
            "ci_separated_caches": false, "ci_allow_fork_pipelines_to_run_in_parent_project": false,
            "auto_cancel_pending_pipelines": "disabled", "build_timeout": 7200, "merge_pipelines_enabled": true,
            "merge_trains_enabled": true, "merge_requests_template": "Template"
        }),
        "project",
    )
    .await;
    s.step("project after", GET, &format!("/projects/{pid}"), Some(&admin), json!(null), "project").await;
    s.graphql("gql ci settings", Some(&admin), "query($p: ID!) { project(fullPath: $p) { ciConfigPathOrDefault mergeRequestsFfOnlyEnabled onlyAllowMergeIfPipelineSucceeds allowMergeOnSkippedPipeline } }", json!({"p": full})).await;
    s.finish();
}
