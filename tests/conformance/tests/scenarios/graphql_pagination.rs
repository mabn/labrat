//! GraphQL connection arguments at the edges: negative sizes, first with last, stale cursors.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_pagination() {
    let mut s = Scenario::new("graphql_pagination").await;
    let admin = s.admin();
    let gpath = s.name("pg");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    for (i, b) in ["b1", "b2", "b3"].iter().enumerate() {
        s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    let q = |args: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequests({args}) {{ nodes {{ iid }} pageInfo {{ hasNextPage hasPreviousPage }} }} }} }}");
    for (label, args) in [
        ("first negative", "first: -1"),
        ("first zero", "first: 0"),
        ("first and last", "first: 1, last: 1"),
        ("last only", "last: 2"),
        ("garbage cursor", "after: \"nope\""),
        ("unknown cursor", "after: \"eyJpZCI6IjEifQ\""),
    ] {
        s.graphql(label, Some(&admin), &q(args), json!({"path": full})).await;
    }
    s.finish();
}
