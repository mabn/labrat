//! Listing pipelines with every filter, scope and order, the latest pipeline of a ref, GraphQL
//! pipeline lists and counts, and webhook test deliveries for job and pipeline events.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn ci_pipeline_lists() {
    let mut s = Scenario::new("ci_pipeline_lists").await;
    let admin = s.admin();
    if !s.target.recording {
        // The reference instance allows webhooks to local addresses.
        s.send(Method::PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    }
    let gpath = s.name("cipl");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = p.body["path_with_namespace"].as_str().unwrap_or("").to_string();
    s.settle().await;
    let username = s.name("dev");
    let u = s
        .step("dev", POST, "/users", Some(&admin), json!({"username": username, "name": "Dev", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let dev = s.step("dev token", POST, &format!("/users/{u}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": u, "access_level": 30}), "user").await;

    // main passes, feature (by the developer) fails, and main's second pipeline runs.
    s.ci_pipeline("main ci", pid, &admin, "main", None, json!([{"name": "build", "stage": "build"}])).await;
    s.ci_job_start("build").await;
    s.ci_job_finish("build", "success", None).await;
    s.ci_pipeline("feature ci", pid, &dev, "feature", Some("main"), json!([{"name": "build", "stage": "build"}])).await;
    s.ci_job_start("build").await;
    s.ci_job_finish("build", "failed", None).await;
    s.ci_pipeline("main ci 2", pid, &admin, "main", None, json!([{"name": "build", "stage": "build"}, {"name": "test", "stage": "test"}])).await;
    s.ci_job_start("build").await;

    let pls = format!("/projects/{pid}/pipelines");
    let queries = [
        ("all", json!(null)),
        ("running", json!({"scope": "running"})),
        ("pending", json!({"scope": "pending"})),
        ("finished", json!({"scope": "finished"})),
        ("branches", json!({"scope": "branches"})),
        ("tags", json!({"scope": "tags"})),
        ("failed", json!({"status": "failed"})),
        ("feature ref", json!({"ref": "feature"})),
        ("by dev", json!({"username": username})),
        ("by nobody", json!({"username": "nobody-at-all"})),
        ("by status", json!({"order_by": "status", "sort": "asc"})),
        ("by ref", json!({"order_by": "ref"})),
        ("by user", json!({"order_by": "user_id", "sort": "asc"})),
        ("by updated", json!({"order_by": "updated_at", "sort": "asc"})),
        ("oldest first", json!({"sort": "asc", "per_page": 2})),
        ("updated after", json!({"updated_after": "2020-01-01T00:00:00Z"})),
        ("created before", json!({"created_before": "2020-01-01T00:00:00Z"})),
        ("push", json!({"source": "push"})),
        ("web", json!({"source": "web"})),
        ("no yaml errors", json!({"yaml_errors": false})),
        ("yaml errors", json!({"yaml_errors": true})),
        ("by name", json!({"name": "nightly"})),
        ("bad scope", json!({"scope": "nope"})),
        ("bad order", json!({"order_by": "sha"})),
        ("bad sort", json!({"sort": "up"})),
    ];
    for (label, q) in queries {
        s.step(label, GET, &pls, Some(&admin), q, "pipeline").await;
    }
    s.step("latest main", GET, &format!("{pls}/latest"), Some(&admin), json!(null), "pipeline").await;
    s.step("latest feature", GET, &format!("{pls}/latest"), Some(&admin), json!({"ref": "feature"}), "pipeline").await;
    s.step("latest nowhere", GET, &format!("{pls}/latest"), Some(&admin), json!({"ref": "nope"}), "pipeline").await;
    s.graphql(
        "graphql lists",
        Some(&admin),
        "query($p: ID!) { project(fullPath: $p) {
          all: pipelines { count nodes { iid status ref } }
          failed: pipelines(status: FAILED) { nodes { iid } }
          finished: pipelines(scope: FINISHED) { nodes { iid } }
          feature: pipelines(ref: \"feature\") { nodes { iid } }
          main: pipelineCounts(ref: \"main\") { all finished pending running }
          counts: pipelineCounts { all finished pending running }
        } }",
        json!({"p": full}),
    )
    .await;

    // Test deliveries use the newest job and pipeline.
    let mut rx = s.receiver().await;
    let url = rx.url.clone();
    let h = s.step("hook", POST, &format!("/projects/{pid}/hooks"), Some(&admin), json!({"url": url, "job_events": true, "pipeline_events": true, "push_events": false}), "hook").await;
    let hid = h.id();
    s.step("test job hook", POST, &format!("/projects/{pid}/hooks/{hid}/test/job_events"), Some(&admin), json!(null), "hook").await;
    s.step("test pipeline hook", POST, &format!("/projects/{pid}/hooks/{hid}/test/pipeline_events"), Some(&admin), json!(null), "hook").await;
    s.deliveries("test deliveries", &mut rx, 2).await;
    s.ci_job_finish("build", "success", None).await;
    s.ci_drain().await;
    s.finish();
}
