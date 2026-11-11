//! Error responses of the MR, notes, discussions, draft notes, approvals, suggestions, awards,
//! to-dos and resource events endpoints that no other scenario reaches
//! (docs/gitlab-ref/mr-api-error-coverage.md).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn api_error_paths() {
    let mut s = Scenario::new("api_error_paths").await;
    let admin = s.admin();
    let gpath = s.name("ae");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let other = s.step("other project", POST, "/projects", Some(&admin), json!({"name": "other", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await.id();
    s.settle().await;
    let mut people: Vec<(String, i64, String)> = vec![];
    for (who, level) in [("dev", 30), ("guest", 10)] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} member"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": level}), "user").await;
        people.push((username, id, tok));
    }
    let dev = people[0].2.clone();
    let guest = people[1].2.clone();
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for b in ["a", "b", "c"] {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "one\ntwo\n"}]}), "commit").await;
    }
    s.settle().await;
    for (i, b) in ["a", "b", "c"].iter().enumerate() {
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    let mr = format!("{mrs}/1");
    // Suggestions on MR 2 (to be merged) and MR 3 (to be closed).
    let mut suggestions = vec![];
    for iid in [2, 3] {
        let m = s.send(GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null)).await;
        let refs = m.body["diff_refs"].clone();
        let file = if iid == 2 { "b.txt" } else { "c.txt" };
        let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": file, "new_path": file, "new_line": 1});
        let d = s.step(&format!("suggestion on {iid}"), POST, &format!("{mrs}/{iid}/discussions"), Some(&admin), json!({"body": "```suggestion:-0+0\nONE\n```", "position": pos}), "discussion").await;
        let did = d.body["id"].as_str().unwrap_or_default().to_string();
        let full = s.send(GET, &format!("{mrs}/{iid}/discussions/{did}"), Some(&admin), json!(null)).await;
        suggestions.push(full.body["notes"][0]["suggestions"][0]["id"].clone());
    }
    // A system-note thread (from an approval) and notes by different users on MR 1.
    s.step("approve for system note", POST, &format!("{mr}/approve"), Some(&admin), json!(null), "approval").await;
    s.settle().await;
    let notes = s.send(GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc"})).await;
    let sys = notes.body.as_array().and_then(|a| a.iter().find(|n| n["system"] == true)).cloned().unwrap_or_default();
    let sys_id = sys["id"].clone();
    let discussions = s.send(GET, &format!("{mr}/discussions"), Some(&admin), json!(null)).await;
    let sys_disc = discussions
        .body
        .as_array()
        .and_then(|a| a.iter().find(|d| d["notes"][0]["system"] == true))
        .map(|d| d["id"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let admin_note = s.step("admin note", POST, &format!("{mr}/notes"), Some(&admin), json!({"body": "By admin"}), "note").await.id();
    let internal = s.step("internal note", POST, &format!("{mr}/notes"), Some(&admin), json!({"body": "Internal", "internal": true}), "note").await.id();
    let checks: Vec<(&str, Method, String, Option<&str>, serde_json::Value)> = vec![
        ("reply to system note", POST, format!("{mr}/discussions/{sys_disc}/notes"), Some(&dev), json!({"body": "r"})),
        ("edit system note", PUT, format!("{mr}/notes/{sys_id}"), Some(&admin), json!({"body": "x"})),
        ("delete system note", DELETE, format!("{mr}/notes/{sys_id}"), Some(&admin), json!(null)),
        ("dev edits admin note", PUT, format!("{mr}/notes/{admin_note}"), Some(&dev), json!({"body": "x"})),
        ("dev deletes admin note", DELETE, format!("{mr}/notes/{admin_note}"), Some(&dev), json!(null)),
        ("unknown discussion notes", GET, format!("{mr}/discussions/deadbeef/notes"), Some(&admin), json!(null)),
        ("resolve unknown discussion", PUT, format!("{mr}/discussions/deadbeef"), Some(&admin), json!({"resolved": true})),
        ("guest reads internal note", GET, format!("{mr}/notes/{internal}"), Some(&guest), json!(null)),
        ("note without body", POST, format!("{mr}/notes"), Some(&admin), json!({})),
        ("reply without body", POST, format!("{mr}/discussions/{sys_disc}/notes"), Some(&admin), json!({})),
        ("unknown mr notes", GET, format!("{mrs}/9999/notes"), Some(&admin), json!(null)),
        ("unknown mr approve", POST, format!("{mrs}/9999/approve"), Some(&admin), json!(null)),
        ("unknown mr update", PUT, format!("{mrs}/9999"), Some(&admin), json!({"title": "x"})),
        ("non-numeric mr", GET, format!("{mrs}/abc/notes"), Some(&admin), json!(null)),
        ("draft reply to system note", POST, format!("{mr}/draft_notes"), Some(&admin), json!({"note": "x", "in_reply_to_discussion_id": sys_disc})),
        ("draft without note", POST, format!("{mr}/draft_notes"), Some(&admin), json!({})),
        ("unknown draft", GET, format!("{mr}/draft_notes/999999"), Some(&admin), json!(null)),
        ("publish unknown draft", PUT, format!("{mr}/draft_notes/999999/publish"), Some(&admin), json!(null)),
        ("award without name", POST, format!("{mr}/award_emoji"), Some(&admin), json!({})),
        ("award empty name", POST, format!("{mr}/award_emoji"), Some(&admin), json!({"name": ""})),
        ("todos bad state", GET, "/todos".into(), Some(&admin), json!({"state": "bogus"})),
        ("todos bad type", GET, "/todos".into(), Some(&admin), json!({"type": "bogus"})),
        ("unknown todo", POST, "/todos/999999/mark_as_done".into(), Some(&admin), json!(null)),
        ("unknown state event", GET, format!("{mr}/resource_state_events/999999"), Some(&admin), json!(null)),
        ("unknown milestone event", GET, format!("{mr}/resource_milestone_events/999999"), Some(&admin), json!(null)),
        ("state_event merge", PUT, mr.clone(), Some(&admin), json!({"state_event": "merge"})),
        ("mr to unrelated project", POST, mrs.clone(), Some(&admin), json!({"source_branch": "a", "target_branch": "main", "title": "x", "target_project_id": other})),
        ("mr to missing project", POST, mrs.clone(), Some(&admin), json!({"source_branch": "a", "target_branch": "main", "title": "x", "target_project_id": 999999})),
        ("guest creates mr", POST, mrs.clone(), Some(&guest), json!({"source_branch": "a", "target_branch": "main", "title": "x"})),
        ("cancel auto-merge not set", POST, format!("{mr}/cancel_merge_when_pipeline_succeeds"), Some(&admin), json!(null)),
        ("guest cancels auto-merge", POST, format!("{mr}/cancel_merge_when_pipeline_succeeds"), Some(&guest), json!(null)),
        ("batch apply without ids", PUT, "/suggestions/batch_apply".into(), Some(&admin), json!({})),
    ];
    for (label, method, path, tok, body) in checks {
        s.step(label, method, &path, tok, body, "error").await;
    }
    // Approvals and suggestions on merged and closed MRs.
    s.step("merge 2", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!(null), "mr").await;
    s.step("close 3", PUT, &format!("{mrs}/3"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.settle().await;
    s.step("approve merged", POST, &format!("{mrs}/2/approve"), Some(&dev), json!(null), "error").await;
    s.step("unapprove merged", POST, &format!("{mrs}/2/unapprove"), Some(&admin), json!(null), "error").await;
    s.step("reset approvals merged", PUT, &format!("{mrs}/2/reset_approvals"), Some(&admin), json!(null), "error").await;
    s.step("approve closed", POST, &format!("{mrs}/3/approve"), Some(&dev), json!(null), "error").await;
    s.step("apply on merged", PUT, &format!("/suggestions/{}/apply", suggestions[0]), Some(&admin), json!(null), "error").await;
    s.step("apply on closed", PUT, &format!("/suggestions/{}/apply", suggestions[1]), Some(&admin), json!(null), "error").await;
    s.finish();
}
