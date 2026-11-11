//! Who counts as a merge request participant, and in which order: author, assignees,
//! reviewers, note authors, award emoji givers (on the MR and on notes) and users mentioned
//! in the description or in notes. Compared through the participants list and `subscribed`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn participants_sources() {
    let mut s = Scenario::new("participants_sources").await;
    let admin = s.admin();
    let gpath = s.name("ps");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut users = vec![];
    for who in ["ann", "ben", "cat", "dan", "eve", "fay", "gus"] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} developer"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": 30}), "user").await;
        users.push((username, id, tok));
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let mr = format!("{mrs}/1");
    // ann: mentioned in the description.
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "description": format!("For @{}", users[0].0)}), "mr").await;
    s.poll_mr(pid, 1).await;
    // ben: awards the MR. cat: comments. dan: awards cat's note. eve: mentioned in cat's note.
    s.step("ben award", POST, &format!("{mr}/award_emoji"), Some(&users[1].2), json!({"name": "thumbsup"}), "award").await;
    let n = s.step("cat note", POST, &format!("{mr}/notes"), Some(&users[2].2), json!({"body": format!("Thoughts, @{}?", users[4].0)}), "note").await;
    s.step("dan note award", POST, &format!("{mr}/notes/{}/award_emoji", n.id()), Some(&users[3].2), json!({"name": "heart"}), "award").await;
    // fay: reviewer. gus: assignee.
    s.step("reviewer", PUT, &mr, Some(&admin), json!({"reviewer_ids": [users[5].1], "assignee_ids": [users[6].1]}), "mr").await;
    s.settle().await;
    s.step("participants", GET, &format!("{mr}/participants"), Some(&admin), json!(null), "user").await;
    s.graphql(
        "graphql participants",
        Some(&admin),
        "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { participants { nodes { username } } } } }",
        json!({"p": full}),
    )
    .await;
    for (name, _, tok) in users.clone() {
        let short = name.split('-').next().unwrap_or("").to_string();
        s.step(&format!("{short} sees"), GET, &mr, Some(&tok), json!(null), "mr").await;
    }
    // Removing the reviewer and the mention keeps only the remaining sources.
    s.step("edit", PUT, &mr, Some(&admin), json!({"reviewer_ids": [], "description": "No mentions"}), "mr").await;
    s.settle().await;
    s.step("participants after", GET, &format!("{mr}/participants"), Some(&admin), json!(null), "user").await;
    s.step("ann after", GET, &mr, Some(&users[0].2), json!(null), "mr").await;
    s.step("fay after", GET, &mr, Some(&users[5].2), json!(null), "mr").await;
    s.finish();
}
