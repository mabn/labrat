//! Branch pipelines with jobs on a merge request: its head pipeline and pipelines, the CI
//! merge check while jobs run, fail and are retried, and an auto-merge that merges when the
//! new head's pipeline passes.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn ci_mr_pipelines() {
    let mut s = Scenario::new("ci_mr_pipelines").await;
    let admin = s.admin();
    let gpath = s.name("cimr");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = p.body["path_with_namespace"].as_str().unwrap_or("").to_string();
    s.settle().await;
    s.step("pipelines must pass", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_pipeline_succeeds": true}), "project").await;
    let jobs = json!([{"name": "build", "stage": "build"}, {"name": "test", "stage": "test"}]);
    s.ci_pipeline("feature ci", pid, &admin, "feature", Some("main"), jobs.clone()).await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add CI"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    s.step("mr pending", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("mr pipelines", GET, &format!("{mr}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.ci_job_start("build").await;
    s.ci_job_finish("build", "success", None).await;
    s.ci_job_start("test").await;
    s.poll_mr(pid, 1).await;
    s.step("mr running", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.ci_job_finish("test", "failed", None).await;
    s.poll_mr(pid, 1).await;
    s.step("mr failed", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("merge refused", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
    let q = "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { detailedMergeStatus
      headPipeline { iid status detailedStatus { label icon group } stages { nodes { name status } } jobs { nodes { name status } } }
      pipelines { count nodes { iid status } } } } }";
    s.graphql("mr graphql failed", Some(&admin), q, json!({"p": full})).await;

    // The test is retried and passes.
    let test = s.ci_job_id(&admin, "test").await;
    s.step("retry test", POST, &format!("/projects/{pid}/jobs/{test}/retry"), Some(&admin), json!(null), "job").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr rerunning", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.ci_job_start("test").await;
    s.ci_job_finish("test", "success", None).await;
    s.poll_mr(pid, 1).await;
    s.step("mr passed", GET, &mr, Some(&admin), json!(null), "mr").await;

    // A new push: its pipeline runs while an auto-merge is armed, and the MR merges when it
    // passes.
    s.ci_pipeline("feature ci 2", pid, &admin, "feature", None, json!([{"name": "build", "stage": "build"}, {"name": "test", "stage": "test"}, {"name": "lint", "stage": "test", "allow_failure": true}])).await;
    s.poll_mr(pid, 1).await;
    s.step("mr after push", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("arm", PUT, &format!("{mr}/merge"), Some(&admin), json!({"merge_when_pipeline_succeeds": true}), "mr").await;
    s.ci_job_start("build").await;
    s.ci_job_finish("build", "success", None).await;
    s.ci_job_start("test").await;
    s.ci_job_start("lint").await;
    s.ci_job_finish("lint", "failed", None).await;
    s.poll_mr(pid, 1).await;
    s.step("mr still running", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.ci_job_finish("test", "success", None).await;
    s.poll(&mr, |b| b["state"] == "merged").await;
    s.step("mr merged", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("mr pipelines at the end", GET, &format!("{mr}/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}
