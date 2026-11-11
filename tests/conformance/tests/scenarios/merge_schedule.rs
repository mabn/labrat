//! Scheduled merges (`merge_after`) and the request-changes mutation.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn merge_schedule() {
    let mut s = Scenario::new("merge_schedule").await;
    let admin = s.admin();
    let gpath = s.name("ms");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob reporter", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 20}), "user").await;
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for b in ["a", "b"] {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
    }
    s.settle().await;
    s.step("create scheduled", POST, &mrs, Some(&admin), json!({"source_branch": "a", "target_branch": "main", "title": "A", "merge_after": "2099-01-01T10:00:00Z"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("show scheduled", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("merge now refused", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.step("set auto-merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.step("move schedule to the past", PUT, &format!("{mrs}/1"), Some(&admin), json!({"merge_after": "2020-01-01T00:00:00Z"}), "mr").await;
    // The scheduled-merge cron runs every minute on GitLab.
    let token = admin.clone();
    for _ in 0..150 {
        let r = s.send(GET, &format!("{mrs}/1"), Some(&token), json!(null)).await;
        if r.body["state"] == "merged" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    s.settle().await;
    s.step("merged by schedule", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;

    s.step("create plain", POST, &mrs, Some(&admin), json!({"source_branch": "b", "target_branch": "main", "title": "B"}), "mr").await;
    s.poll_mr(pid, 2).await;
    for (label, v) in [
        ("date only", json!("2099-02-03")),
        ("with offset", json!("2099-01-01T10:00:00+02:00")),
        ("space separated", json!("2099-03-04 05:06:07")),
        ("garbage", json!("not a date")),
        ("empty clears", json!("")),
        ("set again", json!("2099-05-06T07:08:09.123Z")),
    ] {
        s.step(&format!("update {label}"), PUT, &format!("{mrs}/2"), Some(&admin), json!({"merge_after": v}), "mr").await;
    }
    s.step("list shows schedule", GET, &mrs, Some(&admin), json!({"state": "opened"}), "mr").await;

    let mr_out = "mergeRequest { iid mergeAfter detailedMergeStatus reviewers { nodes { username mergeRequestInteraction { reviewState } } } } errors";
    let gql = |name: &str, extra: &str| format!("mutation($path: ID!) {{ {name}(input: {{projectPath: $path, iid: \"2\"{extra}}}) {{ {mr_out} }} }}");
    s.graphql("gql show", Some(&admin), &format!("query($path: ID!) {{ project(fullPath: $path) {{ {mr_out_q} }} }}", mr_out_q = "mergeRequest(iid: \"2\") { iid mergeAfter detailedMergeStatus }"), json!({"path": full})).await;
    s.graphql("gql update mergeAfter", Some(&admin), &gql("mergeRequestUpdate", ", mergeAfter: \"2098-01-01T00:00:00Z\""), json!({"path": full})).await;
    s.graphql("gql clear mergeAfter", Some(&admin), &gql("mergeRequestUpdate", ", mergeAfter: null"), json!({"path": full})).await;
    s.step("bob reviewer", PUT, &format!("{mrs}/2"), Some(&admin), json!({"reviewer_ids": [bob]}), "mr").await;
    s.graphql("bob requests changes", Some(&bob_t), &gql("mergeRequestRequestChanges", ""), json!({"path": full})).await;
    s.settle().await;
    s.step("after", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
