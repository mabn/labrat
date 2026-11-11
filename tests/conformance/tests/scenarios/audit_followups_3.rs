//! Follow-ups from the MR services audit (docs/gitlab-ref/mr-services-audit.md): to-dos resolved
//! by pushes, "mentioned in" notes, timestamps after note changes, unmergeable to-dos, review
//! publishing that resolves the last thread, manual merges by push, reviewer APPEND,
//! group mentions, draft prefixes and `/copy_metadata`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn audit_followups_3() {
    let mut s = Scenario::new("audit_followups_3").await;
    let admin = s.admin();
    let gpath = s.name("a3");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut people: Vec<(String, i64, String)> = vec![];
    for who in ["bob", "cat"] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} group member"), POST, &format!("/groups/{}/members", g.id()), Some(&admin), json!({"user_id": id, "access_level": 30}), "user").await;
        people.push((username, id, tok));
    }
    let (bob_name, bob, bob_t) = people[0].clone();
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let mk = |b: &str, file: &str, content: &str| json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {file}"), "actions": [{"action": "create", "file_path": file, "content": content}]});
    s.step("c1", POST, &commits, Some(&bob_t), mk("one", "one.txt", "1\n"), "commit").await;
    s.step("c2", POST, &commits, Some(&admin), mk("two", "two.txt", "2\n"), "commit").await;
    s.step("c3", POST, &commits, Some(&admin), mk("conf", "README.md", "conflict\n"), "commit").await;
    s.settle().await;
    let labels = format!("/projects/{pid}/labels");
    s.step("label", POST, &labels, Some(&admin), json!({"name": "bug", "color": "#ff0000"}), "label").await;
    let ms = s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await.id();
    s.step("mr1", POST, &mrs, Some(&bob_t), json!({"source_branch": "one", "target_branch": "main", "title": "Draft: Draft: One", "labels": "bug", "milestone_id": ms}), "mr").await;
    s.poll_mr(pid, 1).await;
    // Mentions of an MR in another MR's description and in a note.
    s.step("mr2", POST, &mrs, Some(&admin), json!({"source_branch": "two", "target_branch": "main", "title": "Two fixup! later", "description": "Depends on !1"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("note mentions mr", POST, &format!("{mrs}/2/notes"), Some(&admin), json!({"body": "See also !1"}), "note").await;
    s.settle().await;
    s.step("mr1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    // Group and @all mentions.
    s.step("group mention", POST, &format!("{mrs}/2/notes"), Some(&admin), json!({"body": format!("Ping @{gpath}")}), "note").await;
    s.settle().await;
    for (name, _, tok) in people.clone() {
        let short = name.split('-').next().unwrap_or("").to_string();
        s.step(&format!("{short} todos"), GET, "/todos", Some(&tok), json!(null), "todo").await;
    }
    s.step("participants", GET, &format!("{mrs}/2/participants"), Some(&admin), json!(null), "user").await;
    // A pending to-do of bob's on his own MR, then bob pushes to it.
    s.step("todo for bob", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": format!("@{bob_name} please check")}), "note").await;
    s.settle().await;
    s.step("bob pushes", POST, &commits, Some(&bob_t), json!({"branch": "one", "commit_message": "More", "actions": [{"action": "update", "file_path": "one.txt", "content": "1b\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("bob todos after push", GET, "/todos", Some(&bob_t), json!({"state": "pending"}), "todo").await;
    // Timestamps after note create, edit and delete.
    let before = s.send(GET, &format!("{mrs}/2"), Some(&admin), json!(null)).await.body["updated_at"].clone();
    let n = s.step("note", POST, &format!("{mrs}/2/notes"), Some(&admin), json!({"body": "A note"}), "note").await.id();
    s.step("edit note", PUT, &format!("{mrs}/2/notes/{n}"), Some(&admin), json!({"body": "Edited"}), "note").await;
    s.step("delete note", DELETE, &format!("{mrs}/2/notes/{n}"), Some(&admin), json!(null), "note").await;
    let after = s.step("mr2 after notes", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await.body["updated_at"].clone();
    let res = labrat_conformance::Res { status: 200, headers: Default::default(), body: json!({"changed": before != after}) };
    s.record("updated_at changed", &GET, "/compare", &res, "list");
    // A review that resolves nothing while all threads are already resolved.
    let d = s.step("thread", POST, &format!("{mrs}/2/discussions"), Some(&admin), json!({"body": "Thread"}), "discussion").await;
    let did = d.body["id"].as_str().unwrap_or_default().to_string();
    s.step("resolve", PUT, &format!("{mrs}/2/discussions/{did}"), Some(&admin), json!({"resolved": true}), "discussion").await;
    s.step("draft", POST, &format!("{mrs}/2/draft_notes"), Some(&bob_t), json!({"note": "Review comment"}), "draft").await;
    s.step("publish", POST, &format!("{mrs}/2/draft_notes/bulk_publish"), Some(&bob_t), json!(null), "draft").await;
    s.settle().await;
    s.step("mr2 notes", GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    // Reviewer APPEND.
    let set = "mutation($p: ID!, $u: [String!]!, $m: MutationOperationMode) { mergeRequestSetReviewers(input: {projectPath: $p, iid: \"2\", reviewerUsernames: $u, operationMode: $m}) { errors mergeRequest { reviewers { nodes { username } } } } }";
    s.graphql("set reviewer", Some(&admin), set, json!({"p": full, "u": [bob_name], "m": "REPLACE"})).await;
    s.graphql("append reviewer", Some(&admin), set, json!({"p": full, "u": [people[1].0], "m": "APPEND"})).await;
    s.step("mr2 after append", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    // Quick action /copy_metadata.
    s.step("copy metadata", POST, &format!("{mrs}/2/notes"), Some(&admin), json!({"body": "/copy_metadata !1"}), "note").await;
    s.step("mr2 after copy", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    // A conflicting MR with auto-merge: the unmergeable to-do.
    s.step("mr conf", POST, &mrs, Some(&admin), json!({"source_branch": "conf", "target_branch": "main", "title": "Conflict"}), "mr").await;
    s.poll_mr(pid, 3).await;
    s.step("main changes readme", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Readme", "actions": [{"action": "update", "file_path": "README.md", "content": "main\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 3).await;
    s.step("mr conf after", GET, &format!("{mrs}/3"), Some(&admin), json!({"with_merge_status_recheck": true}), "mr").await;
    s.settle().await;
    s.step("admin todos", GET, "/todos", Some(&admin), json!({"project_id": pid}), "todo").await;
    let _: Value = json!(bob);
    s.finish();
}
