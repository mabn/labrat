//! More behaviours from the source audits: time formats for `merge_after`, context commit
//! limits, to-dos from failed pipelines, and `ignoredReviewerUsername`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn audit_followups_2() {
    let mut s = Scenario::new("audit_followups_2").await;
    let admin = s.admin();
    let gpath = s.name("ag");
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
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    let sha = c.get("id").as_str().unwrap().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("bob mr", POST, &mrs, Some(&bob_t), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;

    for (label, v) in [
        ("space with offset", "2099-03-04 05:06:07 +0200"),
        ("compact offset", "2099-03-04T05:06:07+0200"),
        ("named zone", "2099-03-04 05:06:07 UTC"),
        ("date and hour", "2099-03-04T05:06"),
    ] {
        s.step(&format!("merge_after {label}"), PUT, &format!("{mrs}/1"), Some(&admin), json!({"merge_after": v}), "mr").await;
    }

    let cc = format!("{mrs}/1/context_commits");
    let many: Vec<String> = (0..21).map(|_| sha.clone()).collect();
    s.step("context commits over limit", POST, &cc, Some(&admin), json!({"commits": many}), "commit").await;
    s.step("context commit by ref", POST, &cc, Some(&admin), json!({"commits": ["main"]}), "commit").await;

    // A failed status on the MR's head commit gives the author a build_failed to-do.
    s.step("failed status", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "failed", "name": "ci", "ref": "feature"}), "status").await;
    s.settle().await;
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!(null), "todo").await;

    // ignoredReviewerUsername: bob is the only reviewer and is unreviewed.
    let q = |args: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequests({args}) {{ count }} }} }}");
    s.graphql("review state", Some(&admin), &q("reviewStates: [UNREVIEWED]"), json!({"path": full})).await;
    s.graphql("review state ignoring bob", Some(&admin), &q(&format!("reviewStates: [UNREVIEWED], ignoredReviewerUsername: \"{username}\"")), json!({"path": full})).await;
    s.graphql("not review state ignoring bob", Some(&admin), &q(&format!("not: {{reviewStates: [APPROVED]}}, ignoredReviewerUsername: \"{username}\"")), json!({"path": full})).await;
    s.finish();
}
