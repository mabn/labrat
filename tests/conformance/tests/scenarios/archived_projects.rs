//! Merge requests of archived projects in search, group lists and GraphQL.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn archived_projects() {
    let mut s = Scenario::new("archived_projects").await;
    let admin = s.admin();
    let gpath = s.name("ar");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let word = s.name("walrus");
    let mut pids = vec![];
    for name in ["live", "old"] {
        let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": name, "namespace_id": gid, "initialize_with_readme": true}), "project").await;
        let pid = p.id();
        pids.push(pid);
        s.settle().await;
        s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": format!("{word} in {name}")}), "mr").await;
        s.poll_mr(pid, 1).await;
    }
    s.step("archive", POST, &format!("/projects/{}/archive", pids[1]), Some(&admin), json!(null), "project").await;
    s.settle().await;
    s.step("global search", GET, "/search", Some(&admin), json!({"scope": "merge_requests", "search": word}), "mr").await;
    s.step("global search with archived", GET, "/search", Some(&admin), json!({"scope": "merge_requests", "search": word, "include_archived": true}), "mr").await;
    s.step("group search", GET, &format!("/groups/{gid}/search"), Some(&admin), json!({"scope": "merge_requests", "search": word}), "mr").await;
    s.step("group projects search", GET, &format!("/groups/{gid}/search"), Some(&admin), json!({"scope": "projects", "search": "old"}), "project").await;
    s.step("project search in archived", GET, &format!("/projects/{}/search", pids[1]), Some(&admin), json!({"scope": "merge_requests", "search": word}), "mr").await;
    s.step("group mrs", GET, &format!("/groups/{gid}/merge_requests"), Some(&admin), json!(null), "mr").await;
    s.step("group mrs with archived", GET, &format!("/groups/{gid}/merge_requests"), Some(&admin), json!({"non_archived": false}), "mr").await;
    s.step("global mrs", GET, "/merge_requests", Some(&admin), json!({"scope": "all", "search": word}), "mr").await;
    let q = |args: &str| format!("query($path: ID!) {{ group(fullPath: $path) {{ mergeRequests{args} {{ count nodes {{ title }} }} }} }}");
    s.graphql("graphql group", Some(&admin), &q(""), json!({"path": gpath})).await;
    s.graphql("graphql group with archived", Some(&admin), &q("(includeArchived: true)"), json!({"path": gpath})).await;
    s.finish();
}
