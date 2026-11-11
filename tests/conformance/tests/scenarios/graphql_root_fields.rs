//! Root GraphQL fields: echo, groups, milestone and todo.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_root_fields() {
    let mut s = Scenario::new("graphql_root_fields").await;
    let admin = s.admin();
    let gpath = s.name("rf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let m = s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let todo = s.step("mark todo", POST, &format!("/projects/{pid}/merge_requests/1/todo"), Some(&admin), json!(null), "todo").await;
    s.graphql("echo", Some(&admin), "query { echo(text: \"hello\") }", json!({})).await;
    s.graphql("echo anonymous", None, "query { echo(text: \"hi\") }", json!({})).await;
    s.graphql("groups", Some(&admin), "query($q: String) { groups(search: $q) { nodes { fullPath name } } }", json!({"q": gpath})).await;
    s.graphql("milestone", Some(&admin), "query($id: MilestoneID!) { milestone(id: $id) { title state } }", json!({"id": format!("gid://gitlab/Milestone/{}", m.id())})).await;
    s.graphql("todo", Some(&admin), "query($id: TodoID!) { todo(id: $id) { action state } }", json!({"id": format!("gid://gitlab/Todo/{}", todo.id())})).await;
    s.finish();
}
