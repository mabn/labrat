//! Behaviours found by reading GitLab's source: commit comment replies, GraphQL reviewer and
//! assignee semantics, and search authentication and query parsing.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn audit_followups() {
    let mut s = Scenario::new("audit_followups").await;
    let admin = s.admin();
    let gpath = s.name("af");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true, "visibility": "public"}), "project").await;
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

    // Commit comments: a plain comment can't be replied to; a line comment can.
    let commits = format!("/projects/{pid}/repository/commits");
    let c = s.step("commit", POST, &commits, Some(&admin), json!({"branch": "b1", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\n"}]}), "commit").await;
    let sha = c.get("id").as_str().unwrap().to_string();
    s.step("plain comment", POST, &format!("{commits}/{sha}/comments"), Some(&admin), json!({"note": "Plain"}), "note").await;
    s.step("line comment", POST, &format!("{commits}/{sha}/comments"), Some(&admin), json!({"note": "On a line", "path": "a.txt", "line": 2, "line_type": "new"}), "note").await;
    s.settle().await;
    let list = s.step("discussions", GET, &format!("{commits}/{sha}/discussions"), Some(&admin), json!(null), "discussion").await;
    let plain_id = list.body[0]["id"].as_str().unwrap_or_default().to_string();
    let line_id = list.body[1]["id"].as_str().unwrap_or_default().to_string();
    s.step("reply to plain", POST, &format!("{commits}/{sha}/discussions/{plain_id}/notes"), Some(&admin), json!({"body": "Reply"}), "note").await;
    s.step("reply to line", POST, &format!("{commits}/{sha}/discussions/{line_id}/notes"), Some(&admin), json!({"body": "Reply"}), "note").await;
    s.settle().await;
    s.step("discussions after", GET, &format!("{commits}/{sha}/discussions"), Some(&admin), json!(null), "discussion").await;

    // MRs for the GraphQL filters: 1 assigned to admin and bob, 2 to bob, 3 with no reviewers.
    let mrs = format!("/projects/{pid}/merge_requests");
    let admin_id = s.send(GET, "/user", Some(&admin), json!(null)).await.body["id"].as_i64().unwrap();
    let specs = [
        ("b1", "One zz", json!({"assignee_ids": [admin_id, bob], "reviewer_ids": [bob]})),
        ("b2", "ab", json!({"assignee_ids": [bob], "reviewer_ids": [admin_id]})),
        ("b3", "Three about #2", json!({})),
    ];
    for (i, (b, title, extra)) in specs.iter().enumerate() {
        if i > 0 {
            s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        }
        s.settle().await;
        let mut body = json!({"source_branch": b, "target_branch": "main", "title": title});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        s.step("mr", POST, &mrs, Some(&admin), body, "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.graphql("bob requests changes", Some(&bob_t), "mutation($path: ID!) { mergeRequestRequestChanges(input: {projectPath: $path, iid: \"1\"}) { errors } }", json!({"path": full})).await;
    s.settle().await;
    let q = |args: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequests({args}) {{ count nodes {{ iid }} }} }} }}");
    let cases: Vec<(&str, String)> = vec![
        ("assignees all of", format!("assigneeUsernames: [\"claude\", \"{username}\"]")),
        ("not assignees", format!("not: {{assigneeUsernames: [\"claude\", \"{username}\"]}}")),
        ("or assignees", format!("or: {{assigneeUsernames: [\"claude\", \"{username}\"]}}")),
        ("not review states", "not: {reviewStates: [REQUESTED_CHANGES]}".into()),
        ("reviewer with state", "reviewerUsername: \"claude\", reviewState: REQUESTED_CHANGES".into()),
        ("lone or review states", "or: {reviewStates: [REQUESTED_CHANGES]}".into()),
        ("or reviewer any", "or: {reviewerWildcard: ANY}".into()),
        ("or none or states", "or: {reviewerWildcard: NONE, reviewStates: [REQUESTED_CHANGES]}".into()),
    ];
    for (label, args) in cases {
        s.graphql(label, Some(&admin), &q(&args), json!({"path": full})).await;
    }

    // Search: authentication first, empty queries, `#N`, short terms.
    let ps = format!("/projects/{pid}/search");
    s.step("anonymous project search", GET, &ps, None, json!({"scope": "merge_requests", "search": "One"}), "mr").await;
    s.step("anonymous group search", GET, &format!("/groups/{gid}/search"), None, json!({"scope": "merge_requests", "search": "One"}), "mr").await;
    s.step("anonymous bad scope", GET, "/search", None, json!({"scope": "bad", "search": "One"}), "mr").await;
    s.step("empty search", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": ""}), "mr").await;
    s.step("hash iid", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": "anything #2"}), "mr").await;
    s.step("two chars exact", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": "ab"}), "mr").await;
    s.step("two chars inside", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": "zz"}), "mr").await;
    s.step("empty milestones", GET, &ps, Some(&admin), json!({"scope": "milestones", "search": ""}), "milestone").await;
    s.finish();
}
