//! Small endpoints: commit sequence and signature, label and milestone promotion, and milestone
//! issues.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn small_endpoints() {
    let mut s = Scenario::new("small_endpoints").await;
    let admin = s.admin();
    let gpath = s.name("se");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let c1 = s.step("commit", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Second", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    let sha = c1.get("id").as_str().unwrap().to_string();
    s.step("sequence", GET, &format!("{commits}/{sha}/sequence"), Some(&admin), json!(null), "commit").await;
    s.step("sequence first parent", GET, &format!("{commits}/{sha}/sequence"), Some(&admin), json!({"first_parent": true}), "commit").await;
    s.step("sequence of branch", GET, &format!("{commits}/main/sequence"), Some(&admin), json!(null), "commit").await;
    s.step("signature", GET, &format!("{commits}/{sha}/signature"), Some(&admin), json!(null), "commit").await;

    s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000", "description": "Broken"}), "label").await;
    s.step("promote label", PUT, &format!("/projects/{pid}/labels/bug/promote"), Some(&admin), json!(null), "label").await;
    s.step("group labels", GET, &format!("/groups/{gid}/labels"), Some(&admin), json!(null), "label").await;
    s.step("project labels", GET, &format!("/projects/{pid}/labels"), Some(&admin), json!(null), "label").await;
    s.step("promote missing label", PUT, &format!("/projects/{pid}/labels/nope/promote"), Some(&admin), json!(null), "label").await;

    let m = s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await;
    let mid = m.id();
    s.step("milestone issues", GET, &format!("/projects/{pid}/milestones/{mid}/issues"), Some(&admin), json!(null), "issue").await;
    s.step("promote milestone", POST, &format!("/projects/{pid}/milestones/{mid}/promote"), Some(&admin), json!(null), "milestone").await;
    s.step("group milestones", GET, &format!("/groups/{gid}/milestones"), Some(&admin), json!(null), "milestone").await;

    s.step("commit b", POST, &commits, Some(&admin), json!({"branch": "b", "start_branch": "main", "commit_message": "B", "actions": [{"action": "create", "file_path": "b.txt", "content": "b\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "b", "target_branch": "main", "title": "B"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.finish();
}
