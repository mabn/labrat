//! EE merge trains: the project settings (REST and GraphQL `ciCdSettings`), the merge train
//! endpoints while trains are off, and merge request pipelines from fake CI: detached ones
//! without merged results pipelines, merged results ones with them.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

/// A group and project (as admin) with fake CI; returns `(project id, full path)`.
pub async fn ci_project(s: &mut Scenario, prefix: &str) -> (i64, String) {
    let admin = s.admin();
    let gpath = s.name(prefix);
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    s.ci_enable("ci config", pid, &admin).await;
    s.settle().await;
    (pid, format!("{gpath}/app"))
}

/// Turns merged results pipelines and merge trains on.
pub async fn enable_trains(s: &mut Scenario, pid: i64) {
    let admin = s.admin();
    s.step("enable trains", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_pipelines_enabled": true, "merge_trains_enabled": true}), "project").await;
    s.settle().await;
}

/// Commits `{name}.txt` on a new branch `name` and opens an MR from it as `token`.
pub async fn branch_mr(s: &mut Scenario, pid: i64, iid: i64, name: &str, token: &str) {
    s.step(
        &format!("commit {name}"),
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(token),
        json!({"branch": name, "start_branch": "main", "commit_message": format!("Add {name}"), "actions": [{"action": "create", "file_path": format!("{name}.txt"), "content": format!("{name}\n")}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.step(
        &format!("mr {name}"),
        POST,
        &format!("/projects/{pid}/merge_requests"),
        Some(token),
        json!({"source_branch": name, "target_branch": "main", "title": format!("Add {name}")}),
        "mr",
    )
    .await;
    s.poll_mr(pid, iid).await;
}

const SETTINGS_QUERY: &str = "query($path: ID!) { project(fullPath: $path) { ciCdSettings {
  mergePipelinesEnabled mergeTrainsEnabled mergeTrainsSkipTrainAllowed } } }";
const SETTINGS_UPDATE: &str = "mutation($path: ID!, $mp: Boolean, $mt: Boolean, $skip: Boolean) {
  projectCiCdSettingsUpdate(input: {fullPath: $path, mergePipelinesEnabled: $mp, mergeTrainsEnabled: $mt, mergeTrainsSkipTrainAllowed: $skip}) {
    errors ciCdSettings { mergePipelinesEnabled mergeTrainsEnabled mergeTrainsSkipTrainAllowed } } }";

#[tokio::test]
async fn merge_train_settings() {
    let mut s = Scenario::new("merge_train_settings").await;
    let admin = s.admin();
    let (pid, full) = ci_project(&mut s, "mts").await;
    let (maint, maint_t) = user_with_token(&mut s, "maint").await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    let (_out, out_t) = user_with_token(&mut s, "outsider").await;
    s.step("maint member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": maint, "access_level": 40}), "user").await;
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    s.settle().await;
    let proj = format!("/projects/{pid}");

    // REST: trains read as off until merged results pipelines are on.
    s.step("project", GET, &proj, Some(&admin), json!(null), "project").await;
    s.step("trains only", PUT, &proj, Some(&maint_t), json!({"merge_trains_enabled": true}), "project").await;
    s.step("skip train only", PUT, &proj, Some(&maint_t), json!({"merge_trains_skip_train_allowed": true}), "project").await;
    s.step("settings as dev", PUT, &proj, Some(&dev_t), json!({"merge_pipelines_enabled": true}), "project").await;
    s.step("project as dev", GET, &proj, Some(&dev_t), json!(null), "project").await;
    let vars = json!({"path": full});
    s.graphql("graphql settings", Some(&admin), SETTINGS_QUERY, vars.clone()).await;
    s.graphql("graphql settings as dev", Some(&dev_t), SETTINGS_QUERY, vars.clone()).await;
    s.graphql("graphql settings as outsider", Some(&out_t), SETTINGS_QUERY, vars.clone()).await;

    // The merge train endpoints while trains are off.
    let trains = format!("/projects/{pid}/merge_trains");
    s.step("trains off list", GET, &trains, Some(&admin), json!(null), "car").await;
    s.step("trains off branch", GET, &format!("{trains}/main"), Some(&admin), json!(null), "car").await;
    s.step("trains off list as dev", GET, &trains, Some(&dev_t), json!(null), "car").await;
    s.step("trains off list as outsider", GET, &trains, Some(&out_t), json!(null), "car").await;
    s.step("trains bad scope", GET, &trains, Some(&admin), json!({"scope": "bad"}), "car").await;

    // Without merged results pipelines, MRs get detached pipelines.
    branch_mr(&mut s, pid, 1, "one", &admin).await;
    s.ci_pick(1).await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr detached running", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("trains off car", GET, &format!("{trains}/merge_requests/1"), Some(&admin), json!(null), "car").await;
    s.step("trains off add", POST, &format!("{trains}/merge_requests/1"), Some(&admin), json!({}), "car").await;
    s.step("trains off add missing mr", POST, &format!("{trains}/merge_requests/99"), Some(&admin), json!({}), "car").await;
    s.ci_finish("refs/merge-requests/1/head", "success").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr detached done", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr pipelines detached", GET, &format!("{mrs}/1/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("pipelines detached", GET, &format!("{proj}/pipelines"), Some(&admin), json!(null), "pipeline").await;

    // Merged results pipelines on: trains read as on, and so does the skip-train setting.
    s.step("merge pipelines on", PUT, &proj, Some(&maint_t), json!({"merge_pipelines_enabled": true}), "project").await;
    s.graphql("graphql settings on", Some(&admin), SETTINGS_QUERY, vars.clone()).await;
    s.graphql("graphql trains off", Some(&maint_t), SETTINGS_UPDATE, json!({"path": full, "mt": false})).await;
    s.graphql("graphql update as dev", Some(&dev_t), SETTINGS_UPDATE, json!({"path": full, "mt": true})).await;
    s.graphql("graphql update as outsider", Some(&out_t), SETTINGS_UPDATE, json!({"path": full, "mt": true})).await;
    s.graphql("graphql update missing", Some(&admin), SETTINGS_UPDATE, json!({"path": "nope/nope", "mt": true})).await;
    s.step("project trains off", GET, &proj, Some(&admin), json!(null), "project").await;
    s.graphql("graphql all on", Some(&maint_t), SETTINGS_UPDATE, json!({"path": full, "mp": true, "mt": true, "skip": false})).await;
    s.step("project trains on", GET, &proj, Some(&admin), json!(null), "project").await;

    // A push to the MR now runs a merged results pipeline.
    s.step(
        "push one",
        POST,
        &format!("{proj}/repository/commits"),
        Some(&admin),
        json!({"branch": "one", "commit_message": "More one", "actions": [{"action": "update", "file_path": "one.txt", "content": "one\ntwo\n"}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.ci_pick(1).await;
    s.step("mr merged results running", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.ci_finish("refs/merge-requests/1/merge", "failed").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("mr merged results failed", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr pipelines", GET, &format!("{mrs}/1/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("trains on add after failure", POST, &format!("{trains}/merge_requests/1"), Some(&admin), json!({}), "car").await;
    s.step("trains on list", GET, &trains, Some(&admin), json!(null), "car").await;
    s.ci_drain().await;
    s.finish();
}
