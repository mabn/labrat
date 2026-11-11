//! Review actions by role on a public project: a Guest, a Reporter and a non-member comment,
//! draft, approve, request changes, resolve threads, apply suggestions, react, use quick actions
//! and are requested as reviewers.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn review_permissions() {
    let mut s = Scenario::new("review_permissions").await;
    let admin = s.admin();
    let gpath = s.name("rp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public"}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut people: Vec<(String, i64, String)> = vec![];
    for (who, level) in [("guest", 10), ("reporter", 20), ("outsider", 0)] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        if level > 0 {
            s.step(&format!("{who} member"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": level}), "user").await;
        }
        people.push((username, id, tok));
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "F", "actions": [{"action": "create", "file_path": "f.txt", "content": "one\ntwo\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let m = s.send(GET, &mr, Some(&admin), json!(null)).await;
    let refs = m.body["diff_refs"].clone();
    let position = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": "f.txt", "new_path": "f.txt", "new_line": 1});
    let thread = s.step("admin thread", POST, &format!("{mr}/discussions"), Some(&admin), json!({"body": "Suggest\n```suggestion:-0+0\nONE\n```", "position": position}), "discussion").await;
    let tid = thread.body["id"].as_str().unwrap_or_default().to_string();
    let d = s.send(GET, &format!("{mr}/discussions/{tid}"), Some(&admin), json!(null)).await;
    let sugg = d.body["notes"][0]["suggestions"][0]["id"].clone();
    for (name, uid, tok) in people.clone() {
        let short = name.split('-').next().unwrap_or("").to_string();
        let t = Some(tok.as_str());
        s.step(&format!("{short} reads mr"), GET, &mr, t, json!(null), "mr").await;
        s.step(&format!("{short} comments"), POST, &format!("{mr}/notes"), t, json!({"body": format!("Comment by {short}")}), "note").await;
        s.step(&format!("{short} diff thread"), POST, &format!("{mr}/discussions"), t, json!({"body": "On a line", "position": position}), "discussion").await;
        s.step(&format!("{short} draft"), POST, &format!("{mr}/draft_notes"), t, json!({"note": "Draft"}), "draft").await;
        s.step(&format!("{short} publish"), POST, &format!("{mr}/draft_notes/bulk_publish"), t, json!(null), "draft").await;
        s.step(&format!("{short} approve"), POST, &format!("{mr}/approve"), t, json!(null), "approval").await;
        s.graphql(
            &format!("{short} request changes"),
            t,
            "mutation($p: ID!) { mergeRequestRequestChanges(input: {projectPath: $p, iid: \"1\"}) { errors } }",
            json!({"p": full}),
        )
        .await;
        s.step(&format!("{short} resolve"), PUT, &format!("{mr}/discussions/{tid}"), t, json!({"resolved": true}), "discussion").await;
        s.step(&format!("{short} unresolve back"), PUT, &format!("{mr}/discussions/{tid}"), Some(&admin), json!({"resolved": false}), "discussion").await;
        s.step(&format!("{short} apply suggestion"), PUT, &format!("/suggestions/{sugg}/apply"), t, json!(null), "suggestion").await;
        s.step(&format!("{short} award"), POST, &format!("{mr}/award_emoji"), t, json!({"name": "thumbsup"}), "award").await;
        s.step(&format!("{short} quick label"), POST, &format!("{mr}/notes"), t, json!({"body": "/label ~bug\n/todo"}), "note").await;
        s.step(&format!("{short} as reviewer"), PUT, &mr, Some(&admin), json!({"reviewer_ids": [uid]}), "mr").await;
        s.step(&format!("{short} reviewers"), GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "user").await;
        s.graphql(
            &format!("{short} permissions"),
            t,
            "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { userPermissions { adminMergeRequest canApprove canMerge createNote pushToSourceBranch readMergeRequest removeSourceBranch resolveNote updateMergeRequest } } } }",
            json!({"p": full}),
        )
        .await;
        s.settle().await;
    }
    s.step("final notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.step("final approvals", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.finish();
}
