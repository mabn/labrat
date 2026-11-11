use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

async fn user_with_token(s: &mut Scenario, base: &str) -> (i64, String) {
    let admin = s.admin();
    let username = s.name(base);
    let u = s
        .step(
            &format!("create user {base}"),
            POST,
            "/users",
            Some(&admin),
            json!({"username": username, "name": format!("User {base}"), "email": format!("{username}@example.com"),
                   "password": "Password-123-labrat", "skip_confirmation": true}),
            "user",
        )
        .await;
    let id = u.id();
    let t = s
        .step(
            &format!("token {base}"),
            POST,
            &format!("/users/{id}/personal_access_tokens"),
            Some(&admin),
            json!({"name": "t", "scopes": ["api"]}),
            "token",
        )
        .await;
    (id, t.get("token").as_str().unwrap().to_string())
}

#[tokio::test]
async fn review_flow() {
    let mut s = Scenario::new("review_flow").await;
    let admin = s.admin();
    let gpath = s.name("rev");
    let g = s
        .step(
            "group",
            POST,
            "/groups",
            Some(&admin),
            json!({"name": gpath, "path": gpath}),
            "namespace",
        )
        .await;
    let p = s
        .step(
            "project",
            POST,
            "/projects",
            Some(&admin),
            json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}),
            "project",
        )
        .await;
    let pid = p.id();
    s.settle().await;
    let (bob, bob_t) = user_with_token(&mut s, "bob").await;
    s.step(
        "bob developer",
        POST,
        &format!("/groups/{}/members", g.id()),
        Some(&admin),
        json!({"user_id": bob, "access_level": 30}),
        "user",
    )
    .await;
    s.step(
        "feature commit",
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        json!({"branch": "feature", "start_branch": "main", "commit_message": "Add code",
               "actions": [{"action": "create", "file_path": "src/app.py", "content": "def main():\n    print('hi')\n    return 1\n\n\nif __name__ == '__main__':\n    main()\n"}]}),
        "commit",
    )
    .await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("create mr", POST, &mrs, Some(&admin),
        json!({"title": "Add app", "source_branch": "feature", "target_branch": "main", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = s
        .step(
            "get mr",
            GET,
            &format!("{mrs}/1"),
            Some(&admin),
            Value::Null,
            "mr",
        )
        .await;
    let refs = mr.get("diff_refs").clone();
    let notes = format!("{mrs}/1/notes");
    let discussions = format!("{mrs}/1/discussions");

    // Plain notes and quick actions.
    s.step(
        "note",
        POST,
        &notes,
        Some(&bob_t),
        json!({"body": "Looks good overall, @root?"}),
        "note",
    )
    .await;
    s.step(
        "empty note",
        POST,
        &notes,
        Some(&bob_t),
        json!({"body": ""}),
        "note",
    )
    .await;
    s.step(
        "commands only",
        POST,
        &notes,
        Some(&admin),
        json!({"body": "/label ~needs-review\n/assign_reviewer @".to_string() + &s.name("bob")}),
        "note",
    )
    .await;
    s.step(
        "text and command",
        POST,
        &notes,
        Some(&admin),
        json!({"body": "Locking for now\n/lock"}),
        "note",
    )
    .await;
    s.step(
        "unlock",
        POST,
        &notes,
        Some(&admin),
        json!({"body": "/unlock"}),
        "note",
    )
    .await;
    s.settle().await;

    // Threads.
    let d = s
        .step(
            "thread",
            POST,
            &discussions,
            Some(&bob_t),
            json!({"body": "Why print here?"}),
            "discussion",
        )
        .await;
    let did = d.get("id").as_str().unwrap().to_string();
    s.step(
        "reply",
        POST,
        &format!("{discussions}/{did}/notes"),
        Some(&admin),
        json!({"body": "For debugging."}),
        "note",
    )
    .await;
    s.step(
        "get thread",
        GET,
        &format!("{discussions}/{did}"),
        Some(&admin),
        Value::Null,
        "discussion",
    )
    .await;
    s.step(
        "resolve thread",
        PUT,
        &format!("{discussions}/{did}"),
        Some(&bob_t),
        json!({"resolved": true}),
        "discussion",
    )
    .await;
    s.step(
        "unresolve thread",
        PUT,
        &format!("{discussions}/{did}"),
        Some(&bob_t),
        json!({"resolved": false}),
        "discussion",
    )
    .await;

    // Diff notes.
    let pos = |new_line: i64| {
        json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"],
               "position_type": "text", "new_path": "src/app.py", "old_path": "src/app.py", "new_line": new_line})
    };
    let dn = s
        .step(
            "diff note",
            POST,
            &discussions,
            Some(&bob_t),
            json!({"body": "Use logging.", "position": pos(2)}),
            "discussion",
        )
        .await;
    let dnid = dn.get("id").as_str().unwrap().to_string();
    s.step(
        "diff note bad line",
        POST,
        &discussions,
        Some(&bob_t),
        json!({"body": "Nope", "position": pos(99)}),
        "discussion",
    )
    .await;
    s.step(
        "suggestion",
        POST,
        &discussions,
        Some(&bob_t),
        json!({"body": "Suggest:\n```suggestion:-0+0\n    return 0\n```", "position": pos(3)}),
        "discussion",
    )
    .await;
    s.step(
        "reply to diff note",
        POST,
        &format!("{discussions}/{dnid}/notes"),
        Some(&admin),
        json!({"body": "Will do."}),
        "note",
    )
    .await;
    s.settle().await;
    let all = s
        .step(
            "discussions",
            GET,
            &discussions,
            Some(&admin),
            Value::Null,
            "discussion",
        )
        .await;
    let sug_id = all
        .body
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|d| d["notes"].as_array().unwrap().clone())
        .find_map(|n| {
            n["suggestions"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|x| x["id"].as_i64())
        })
        .unwrap();

    // Award emoji.
    let awards = format!("{mrs}/1/award_emoji");
    s.step(
        "award +1",
        POST,
        &awards,
        Some(&bob_t),
        json!({"name": "+1"}),
        "award",
    )
    .await;
    s.step(
        "award dup",
        POST,
        &awards,
        Some(&bob_t),
        json!({"name": "thumbsup"}),
        "award",
    )
    .await;
    s.step(
        "award invalid",
        POST,
        &awards,
        Some(&bob_t),
        json!({"name": "not_an_emoji_x"}),
        "award",
    )
    .await;
    let a2 = s
        .step(
            "award heart",
            POST,
            &awards,
            Some(&admin),
            json!({"name": "heart"}),
            "award",
        )
        .await;
    s.step(
        "list awards",
        GET,
        &awards,
        Some(&admin),
        Value::Null,
        "award",
    )
    .await;
    s.step(
        "delete others award",
        DELETE,
        &format!("{awards}/{}", a2.id()),
        Some(&bob_t),
        Value::Null,
        "award",
    )
    .await;

    // Drafts and review submission.
    let drafts = format!("{mrs}/1/draft_notes");
    let dr = s
        .step(
            "draft",
            POST,
            &drafts,
            Some(&bob_t),
            json!({"note": "Draft comment"}),
            "draft",
        )
        .await;
    s.step("draft reply", POST, &drafts, Some(&bob_t), json!({"note": "Draft reply", "in_reply_to_discussion_id": did, "resolve_discussion": true}), "draft").await;
    s.step(
        "list drafts",
        GET,
        &drafts,
        Some(&bob_t),
        Value::Null,
        "draft",
    )
    .await;
    s.step(
        "update draft",
        PUT,
        &format!("{drafts}/{}", dr.id()),
        Some(&bob_t),
        json!({"note": "Draft comment, edited"}),
        "draft",
    )
    .await;
    s.step(
        "reviewers during review",
        GET,
        &format!("{mrs}/1/reviewers"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "bulk publish",
        POST,
        &format!("{drafts}/bulk_publish"),
        Some(&bob_t),
        Value::Null,
        "draft",
    )
    .await;
    s.settle().await;

    // Approvals.
    s.step(
        "approvals before",
        GET,
        &format!("{mrs}/1/approvals"),
        Some(&bob_t),
        Value::Null,
        "approval",
    )
    .await;
    s.step(
        "approve",
        POST,
        &format!("{mrs}/1/approve"),
        Some(&bob_t),
        Value::Null,
        "approval",
    )
    .await;
    s.settle().await;
    s.step(
        "approve again",
        POST,
        &format!("{mrs}/1/approve"),
        Some(&bob_t),
        Value::Null,
        "approval",
    )
    .await;
    s.step(
        "reviewers after approve",
        GET,
        &format!("{mrs}/1/reviewers"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "unapprove",
        POST,
        &format!("{mrs}/1/unapprove"),
        Some(&bob_t),
        Value::Null,
        "approval",
    )
    .await;
    s.step(
        "unapprove again",
        POST,
        &format!("{mrs}/1/unapprove"),
        Some(&bob_t),
        Value::Null,
        "approval",
    )
    .await;
    s.step(
        "approve command",
        POST,
        &notes,
        Some(&bob_t),
        json!({"body": "/approve"}),
        "note",
    )
    .await;
    s.settle().await;

    // Apply the suggestion.
    s.step(
        "apply suggestion",
        PUT,
        &format!("/suggestions/{sug_id}/apply"),
        Some(&admin),
        Value::Null,
        "suggestion",
    )
    .await;
    s.settle().await;
    s.step(
        "notes asc",
        GET,
        &notes,
        Some(&admin),
        json!({"sort": "asc", "per_page": 100}),
        "note",
    )
    .await;
    s.step(
        "mr after review",
        GET,
        &format!("{mrs}/1"),
        Some(&admin),
        Value::Null,
        "mr",
    )
    .await;
    s.finish();
}
