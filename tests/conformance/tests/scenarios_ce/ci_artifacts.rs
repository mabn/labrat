//! Job artifacts through REST: the job's artifact list, downloading the archive and single
//! files (by job and by ref), browsing the archive, keeping and deleting artifacts.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn ci_artifacts() {
    let mut s = Scenario::new("ci_artifacts").await;
    let admin = s.admin();
    let gpath = s.name("ciar");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("guest");
    let u = s
        .step("guest", POST, "/users", Some(&admin), json!({"username": username, "name": "Guest", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let guest = s.step("guest token", POST, &format!("/users/{u}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("guest member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": u, "access_level": 10}), "user").await;
    let jobs = format!("/projects/{pid}/jobs");

    s.ci_pipeline("ci config", pid, &admin, "main", None, json!([
        {"name": "build", "stage": "build"},
        {"name": "check", "stage": "test"},
    ])).await;
    let build = s.ci_job_id(&admin, "build").await;
    s.step("no artifacts yet", GET, &format!("{jobs}/{build}/artifacts"), Some(&admin), json!(null), "artifact").await;
    s.ci_job_start("build").await;
    s.ci_job_artifacts("build", &[("dist/app.txt", "app v1\n"), ("dist/docs/readme.md", "# App\n"), ("top.txt", "top\n")], Some("2 days")).await;
    s.step("running job artifacts", GET, &format!("{jobs}/{build}"), Some(&admin), json!(null), "job").await;
    s.ci_job_finish("build", "success", None).await;
    s.step("job with artifacts", GET, &format!("{jobs}/{build}"), Some(&admin), json!(null), "job").await;
    s.step("archive", GET, &format!("{jobs}/{build}/artifacts"), Some(&admin), json!(null), "artifact").await;
    s.step("guest archive", GET, &format!("{jobs}/{build}/artifacts"), Some(&guest), json!(null), "artifact").await;
    s.step("one file", GET, &format!("{jobs}/{build}/artifacts/dist/app.txt"), Some(&admin), json!(null), "artifact").await;
    s.step("nested file", GET, &format!("{jobs}/{build}/artifacts/dist/docs/readme.md"), Some(&admin), json!(null), "artifact").await;
    s.step("missing file", GET, &format!("{jobs}/{build}/artifacts/dist/nope.txt"), Some(&admin), json!(null), "artifact").await;
    s.step("tree", GET, &format!("{jobs}/{build}/artifacts/tree"), Some(&admin), json!(null), "artifact").await;
    s.step("tree dist", GET, &format!("{jobs}/{build}/artifacts/tree"), Some(&admin), json!({"path": "dist"}), "artifact").await;
    s.step("tree recursive", GET, &format!("{jobs}/{build}/artifacts/tree"), Some(&admin), json!({"recursive": true}), "artifact").await;
    s.step("tree missing path", GET, &format!("{jobs}/{build}/artifacts/tree"), Some(&admin), json!({"path": "nope"}), "artifact").await;
    s.step("by ref before success", GET, &format!("{jobs}/artifacts/main/download"), Some(&admin), json!({"job": "build"}), "artifact").await;

    let check = s.ci_job_id(&admin, "check").await;
    s.step("check has none", GET, &format!("{jobs}/{check}/artifacts"), Some(&admin), json!(null), "artifact").await;
    s.step("check tree", GET, &format!("{jobs}/{check}/artifacts/tree"), Some(&admin), json!(null), "artifact").await;
    s.ci_job_start("check").await;
    s.ci_job_finish("check", "success", None).await;
    s.step("by ref", GET, &format!("{jobs}/artifacts/main/download"), Some(&admin), json!({"job": "build"}), "artifact").await;
    s.step("raw by ref", GET, &format!("{jobs}/artifacts/main/raw/dist/app.txt"), Some(&admin), json!({"job": "build"}), "artifact").await;
    s.step("by ref no artifacts", GET, &format!("{jobs}/artifacts/main/download"), Some(&admin), json!({"job": "check"}), "artifact").await;
    s.step("by ref unknown job", GET, &format!("{jobs}/artifacts/main/download"), Some(&admin), json!({"job": "nope"}), "artifact").await;
    s.step("by ref unknown ref", GET, &format!("{jobs}/artifacts/nope/download"), Some(&admin), json!({"job": "build"}), "artifact").await;
    s.step("by ref without job", GET, &format!("{jobs}/artifacts/main/download"), Some(&admin), json!(null), "artifact").await;

    s.step("guest cannot keep", POST, &format!("{jobs}/{build}/artifacts/keep"), Some(&guest), json!(null), "job").await;
    s.step("keep", POST, &format!("{jobs}/{build}/artifacts/keep"), Some(&admin), json!(null), "job").await;
    s.step("keep without artifacts", POST, &format!("{jobs}/{check}/artifacts/keep"), Some(&admin), json!(null), "job").await;

    // A second pipeline: by ref finds its artifacts.
    s.ci_pipeline("ci config 2", pid, &admin, "main", None, json!([
        {"name": "build", "stage": "build"},
        {"name": "check", "stage": "test", "allow_failure": true},
    ])).await;
    s.ci_job_start("build").await;
    s.ci_job_artifacts("build", &[("dist/app.txt", "app v2\n")], None).await;
    s.ci_job_finish("build", "success", None).await;
    let build2 = s.ci_job_id(&admin, "build").await;
    s.ci_job_start("check").await;
    s.ci_job_finish("check", "failed", None).await;
    s.step("raw by ref latest", GET, &format!("{jobs}/artifacts/main/raw/dist/app.txt"), Some(&admin), json!({"job": "build"}), "artifact").await;
    s.step("second build", GET, &format!("{jobs}/{build2}"), Some(&admin), json!(null), "job").await;

    s.step("guest cannot delete", DELETE, &format!("{jobs}/{build}/artifacts"), Some(&guest), json!(null), "artifact").await;
    s.step("delete artifacts", DELETE, &format!("{jobs}/{build}/artifacts"), Some(&admin), json!(null), "artifact").await;
    s.step("deleted", GET, &format!("{jobs}/{build}"), Some(&admin), json!(null), "job").await;
    s.step("deleted archive", GET, &format!("{jobs}/{build}/artifacts"), Some(&admin), json!(null), "artifact").await;
    s.step("delete project artifacts", DELETE, &format!("/projects/{pid}/artifacts"), Some(&admin), json!(null), "artifact").await;
    s.settle().await;
    s.step("second build after", GET, &format!("{jobs}/{build2}"), Some(&admin), json!(null), "job").await;
    s.finish();
}
