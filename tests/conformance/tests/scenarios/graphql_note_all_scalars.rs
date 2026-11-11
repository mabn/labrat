//! Every scalar field of GraphQL `Note`, `Discussion` and `DiffPosition` across many kinds of
//! notes: Markdown, mentions, references, code, diff notes, suggestions, system notes, internal
//! notes, edited and resolved ones.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const NOTE: &str = "authorIsContributor body bodyFirstLineHtml bodyHtml createdAt externalAuthor id imported internal lastEditedAt maxAccessLevelOfAuthor noteableId noteableType resolvable resolved resolvedAt system systemNoteIconName updatedAt url position { filePath height newLine newPath oldLine oldPath positionType width x y }";

#[tokio::test]
async fn graphql_note_all_scalars() {
    let mut s = Scenario::new("graphql_note_all_scalars").await;
    let admin = s.admin();
    let gpath = s.name("ns");
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
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\nthree\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let notes = format!("{mr}/notes");
    let bodies = [
        "Plain **bold** and _italic_\n\nSecond paragraph with `code !1` and a [link](https://example.com).",
        "Hey @{bob}, see !1 and ~missing and #5",
        "- [ ] task one\n- [x] task two\n\n| a | b |\n|---|---|\n| 1 | 2 |",
        "```\nputs 'hi' & <b>\nsecond line\n```",
        "Line one\nline two\n> quoted",
    ];
    for (i, b) in bodies.iter().enumerate() {
        let body = b.replace("{bob}", &username);
        s.step(&format!("note {i}"), POST, &notes, Some(&bob_t), json!({"body": body}), "note").await;
    }
    s.step("internal", POST, &notes, Some(&admin), json!({"body": "Internal remark", "internal": true}), "note").await;
    let refs = s.send(GET, &mr, Some(&admin), json!(null)).await.body["diff_refs"].clone();
    let pos = |line: i64| json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "new_line": line});
    let d = s.step("diff note", POST, &format!("{mr}/discussions"), Some(&bob_t), json!({"body": "On line 2", "position": pos(2)}), "discussion").await;
    let did = d.body["id"].as_str().unwrap_or_default().to_string();
    s.step("suggestion", POST, &format!("{mr}/discussions"), Some(&bob_t), json!({"body": "```suggestion:-0+0\nTHREE\n```", "position": pos(3)}), "discussion").await;
    s.step("reply", POST, &format!("{mr}/discussions/{did}/notes"), Some(&admin), json!({"body": "Fixed"}), "note").await;
    s.step("resolve", PUT, &format!("{mr}/discussions/{did}"), Some(&admin), json!({"resolved": true}), "discussion").await;
    let first = s.send(GET, &notes, Some(&admin), json!({"sort": "asc"})).await;
    let first_user_note = first.body.as_array().and_then(|a| a.iter().find(|n| n["system"] == false).cloned()).unwrap_or_default();
    s.step("edit", PUT, &format!("{notes}/{}", first_user_note["id"]), Some(&bob_t), json!({"body": "Plain **bold**, edited"}), "note").await;
    s.step("award", POST, &format!("{notes}/{}/award_emoji", first_user_note["id"]), Some(&admin), json!({"name": "thumbsup"}), "award").await;
    s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000"}), "label").await;
    s.step("labelled", PUT, &mr, Some(&admin), json!({"add_labels": "bug", "milestone_id": null}), "mr").await;
    s.settle().await;
    let q = format!("query($p: ID!) {{ project(fullPath: $p) {{ mergeRequest(iid: \"1\") {{ notes {{ nodes {{ {NOTE} }} }} discussions {{ nodes {{ createdAt id replyId resolvable resolved resolvedAt }} }} }} }} }}");
    s.graphql("notes as admin", Some(&admin), &q, json!({"p": full})).await;
    s.graphql("notes as bob", Some(&bob_t), &q, json!({"p": full})).await;
    s.finish();
}
