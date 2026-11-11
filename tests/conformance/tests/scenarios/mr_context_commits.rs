//! MR context commits, and creating pipelines in a project without CI configuration.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn mr_context_commits() {
    let mut s = Scenario::new("mr_context_commits").await;
    let admin = s.admin();
    let gpath = s.name("cc");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let m1 = s.step("main commit", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Earlier work", "actions": [{"action": "create", "file_path": "e.txt", "content": "e\n"}]}), "commit").await;
    let earlier = m1.get("id").as_str().unwrap().to_string();
    s.step("feature commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    let other = s.step("other commit", POST, &commits, Some(&admin), json!({"branch": "other", "start_branch": "main", "commit_message": "Other work", "actions": [{"action": "create", "file_path": "o.txt", "content": "o\n"}]}), "commit").await;
    let other_sha = other.get("id").as_str().unwrap().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;

    let cc = format!("{mrs}/1/context_commits");
    s.step("context commits empty", GET, &cc, Some(&admin), json!(null), "commit").await;
    s.step("add context commit", POST, &cc, Some(&admin), json!({"commits": [other_sha]}), "commit").await;
    s.step("add again", POST, &cc, Some(&admin), json!({"commits": [other_sha]}), "commit").await;
    s.step("add short sha", POST, &cc, Some(&admin), json!({"commits": [&earlier[..8]]}), "commit").await;
    s.step("add unknown", POST, &cc, Some(&admin), json!({"commits": ["0000000000000000000000000000000000000000"]}), "commit").await;
    s.step("add empty", POST, &cc, Some(&admin), json!({"commits": []}), "commit").await;
    s.step("add missing param", POST, &cc, Some(&admin), json!({}), "commit").await;
    s.settle().await;
    s.step("context commits", GET, &cc, Some(&admin), json!(null), "commit").await;
    s.step("mr commits", GET, &format!("{mrs}/1/commits"), Some(&admin), json!(null), "commit").await;
    s.step("mr", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("remove context commit", DELETE, &cc, Some(&admin), json!({"commits": [other_sha]}), "commit").await;
    s.step("remove not added", DELETE, &cc, Some(&admin), json!({"commits": [other_sha]}), "commit").await;
    // The two above send `commits=["…"]` (a string, not an array) in the query.
    s.step("remove as array", DELETE, &cc, Some(&admin), json!({"commits[]": other_sha}), "commit").await;
    s.step("remove as array again", DELETE, &cc, Some(&admin), json!({"commits[]": other_sha}), "commit").await;
    s.step("context commits after", GET, &cc, Some(&admin), json!(null), "commit").await;

    s.step("create mr pipeline", POST, &format!("{mrs}/1/pipelines"), Some(&admin), json!(null), "pipeline").await;
    s.step("create project pipeline", POST, &format!("/projects/{pid}/pipeline"), Some(&admin), json!({"ref": "main"}), "pipeline").await;
    s.step("create pipeline missing ref", POST, &format!("/projects/{pid}/pipeline"), Some(&admin), json!({}), "pipeline").await;
    s.step("create pipeline unknown ref", POST, &format!("/projects/{pid}/pipeline"), Some(&admin), json!({"ref": "nope"}), "pipeline").await;
    s.finish();
}
