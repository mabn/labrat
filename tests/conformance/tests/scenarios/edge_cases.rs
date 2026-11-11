//! Edge cases from `docs/gitlab-ref/edge-cases-mr-lifecycle.md` (sections A-G).

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
        .step(&format!("token {base}"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token")
        .await;
    (id, t.get("token").as_str().unwrap().to_string())
}

/// A private project with a README, plus `dev` (Developer) and `rep` (Reporter).
async fn setup(s: &mut Scenario, prefix: &str) -> (i64, (i64, String), (i64, String)) {
    let admin = s.admin();
    let gpath = s.name(prefix);
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s
        .step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project")
        .await;
    let pid = p.id();
    s.settle().await;
    let dev = user_with_token(s, "dev").await;
    let rep = user_with_token(s, "rep").await;
    for (who, level) in [(dev.0, 30), (rep.0, 20)] {
        s.step("add member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": who, "access_level": level}), "user").await;
    }
    (pid, dev, rep)
}

async fn commit(s: &mut Scenario, pid: i64, branch: &str, start: Option<&str>, msg: &str, file: &str) -> Value {
    let admin = s.admin();
    let mut body = json!({"branch": branch, "commit_message": msg, "actions": [{"action": "create", "file_path": file, "content": "x\n"}]});
    if let Some(st) = start {
        body["start_branch"] = json!(st);
    }
    s.step(&format!("commit {msg}"), POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), body, "commit").await.body
}

#[tokio::test]
async fn mr_create_update_edges() {
    let mut s = Scenario::new("mr_create_update_edges").await;
    let admin = s.admin();
    let (pid, (dev, dev_t), (_rep, rep_t)) = setup(&mut s, "edge").await;
    commit(&mut s, pid, "feature", Some("main"), "Add a", "a.txt").await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let mr_body = |src: &str, tgt: &str, title: &str| json!({"source_branch": src, "target_branch": tgt, "title": title});

    // A1-A3: titles
    s.step("blank title", POST, &mrs, Some(&admin), mr_body("feature", "main", "   "), "mr").await;
    s.step("empty title", POST, &mrs, Some(&admin), mr_body("feature", "main", ""), "mr").await;
    s.step("missing title", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main"}), "mr").await;
    s.step("title too long", POST, &mrs, Some(&admin), mr_body("feature", "main", &"a".repeat(256)), "mr").await;
    s.step("title 255 multibyte", POST, &mrs, Some(&admin), mr_body("feature", "main", &"é".repeat(255)), "mr").await;
    s.step("close long", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    let padded = s.step("padded draft title", POST, &mrs, Some(&admin), mr_body("feature", "main", "  Draft: Add a  "), "mr").await;
    let iid = padded.get("iid").as_i64().unwrap();
    let mr = format!("{mrs}/{iid}");
    s.poll_mr(pid, iid).await;

    // A5, A7, A8: branches
    s.step("invalid branch", POST, &mrs, Some(&admin), mr_body("a..b", "main", "t"), "mr").await;
    s.step("both branches invalid", POST, &mrs, Some(&admin), mr_body("a b", "x.lock", "t"), "mr").await;
    s.step("same source and target", POST, &mrs, Some(&admin), mr_body("main", "main", "t"), "mr").await;
    s.step("duplicate open mr", POST, &mrs, Some(&admin), mr_body("feature", "main", "two"), "mr").await;
    let ghost = s.step("missing source branch", POST, &mrs, Some(&admin), mr_body("ghost", "main", "ghost"), "mr").await;
    let ghost_iid = ghost.get("iid").as_i64().unwrap();
    s.poll_mr(pid, ghost_iid).await;
    s.step("ghost mr after check", GET, &format!("{mrs}/{ghost_iid}"), Some(&admin), json!(null), "mr").await;

    // A10, A11: labels and assignees
    let labels_mr = s
        .step("create label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000"}), "label")
        .await;
    let _ = labels_mr;
    s.step("labels trimmed and created", PUT, &mr, Some(&admin), json!({"labels": " bug , new one ,bug"}), "mr").await;
    s.step("developer labels", PUT, &mr, Some(&dev_t), json!({"labels": "bug,devlabel"}), "mr").await;
    s.step("labels cleared", PUT, &mr, Some(&admin), json!({"labels": ""}), "mr").await;
    s.step("two assignees keeps first", PUT, &mr, Some(&admin), json!({"assignee_ids": [dev, 1]}), "mr").await;
    s.step("unknown assignee", PUT, &mr, Some(&admin), json!({"assignee_ids": [999999]}), "mr").await;
    s.step("legacy assignee_id wins", PUT, &mr, Some(&admin), json!({"assignee_id": 0, "assignee_ids": [dev]}), "mr").await;
    s.step("reporter cannot update", PUT, &mr, Some(&rep_t), json!({"title": "x"}), "mr").await;

    // B1-B4: update validation
    s.step("no params", PUT, &mr, Some(&admin), json!({}), "mr").await;
    s.step("empty title on update", PUT, &mr, Some(&admin), json!({"title": ""}), "mr").await;
    s.step("target to source", PUT, &mr, Some(&admin), json!({"target_branch": "feature"}), "mr").await;
    s.step("invalid target on update", PUT, &mr, Some(&admin), json!({"target_branch": "a..b"}), "mr").await;
    s.step("missing target on update", PUT, &mr, Some(&admin), json!({"target_branch": "nope"}), "mr").await;
    s.settle().await;
    s.step("mr with missing target", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("target back", PUT, &mr, Some(&admin), json!({"target_branch": "main"}), "mr").await;
    s.poll_mr(pid, iid).await;

    // E2: draft toggles
    s.step("still draft", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("undraft", PUT, &mr, Some(&admin), json!({"title": "Add a"}), "mr").await;
    s.step("redraft brackets", PUT, &mr, Some(&admin), json!({"title": "[Draft] Add a"}), "mr").await;
    s.step("wip is not draft", PUT, &mr, Some(&admin), json!({"title": "WIP: Add a"}), "mr").await;
    s.step("title only draft", PUT, &mr, Some(&admin), json!({"title": "Draft:"}), "mr").await;
    s.step("title back", PUT, &mr, Some(&admin), json!({"title": "Add a"}), "mr").await;

    // B6: partial application
    s.step("close", PUT, &mr, Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("close again", PUT, &mr, Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("reopen with long title", PUT, &mr, Some(&admin), json!({"state_event": "reopen", "title": "b".repeat(256)}), "mr").await;
    s.settle().await;
    s.step("reopened anyway", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("close with title", PUT, &mr, Some(&admin), json!({"state_event": "close", "title": "Closed title"}), "mr").await;

    // C3: reopen while another MR is open for the same branches
    let other = s.step("new mr while closed", POST, &mrs, Some(&admin), mr_body("feature", "main", "second"), "mr").await;
    s.poll_mr(pid, other.get("iid").as_i64().unwrap()).await;
    s.step("reopen conflicting", PUT, &mr, Some(&admin), json!({"state_event": "reopen"}), "mr").await;

    // C4: discussion locked blocks reopen
    let other_mr = format!("{mrs}/{}", other.get("iid").as_i64().unwrap());
    s.step("lock discussion", PUT, &other_mr, Some(&admin), json!({"discussion_locked": true}), "mr").await;
    s.step("close locked", PUT, &other_mr, Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("reopen locked", PUT, &other_mr, Some(&admin), json!({"state_event": "reopen"}), "mr").await;

    // C7, C8: merge closed / without permission
    s.step("merge closed", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
    s.step("reporter merge", PUT, &format!("{other_mr}/merge"), Some(&rep_t), json!(null), "mr").await;

    // F3: delete and iid reuse
    s.step("developer delete", DELETE, &mr, Some(&dev_t), json!(null), "mr").await;
    s.step("owner delete", DELETE, &mr, Some(&admin), json!(null), "mr").await;
    let next = s.step("iid not reused", POST, &mrs, Some(&admin), mr_body("feature", "other", "third"), "mr").await;
    s.poll_mr(pid, next.get("iid").as_i64().unwrap()).await;
    s.step("list", GET, &mrs, Some(&admin), json!(null), "mr").await;
    s.finish();
}

#[tokio::test]
async fn mr_merge_edges() {
    let mut s = Scenario::new("mr_merge_edges").await;
    let admin = s.admin();
    let (pid, (_dev, _dev_t), (_rep, _rep_t)) = setup(&mut s, "medge").await;
    let mrs = format!("/projects/{pid}/merge_requests");

    // D1: sha mismatch beats draft; D4: squash flag sticks on a failed merge.
    commit(&mut s, pid, "f1", Some("main"), "Add a", "a.txt").await;
    s.step("draft mr", POST, &mrs, Some(&admin), json!({"source_branch": "f1", "target_branch": "main", "title": "Draft: Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("merge wrong sha", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"sha": "0000000000000000000000000000000000000000"}), "mr").await;
    s.step("merge draft with squash", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"squash": true}), "mr").await;
    s.step("squash stuck", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("auto merge draft", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.step("undraft", PUT, &format!("{mrs}/1"), Some(&admin), json!({"title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("auto merge without ci", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    // D5: one commit whose message equals the title: no squash commit.
    s.step("merge squash single", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!({"squash": true}), "mr").await;
    s.settle().await;
    s.step("merged single", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("merge merged", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.step("close merged", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("reopen merged", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "reopen"}), "mr").await;

    // D9: no new commits
    s.step("empty branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "empty", "ref": "main"}), "branch").await;
    s.step("empty mr", POST, &mrs, Some(&admin), json!({"source_branch": "empty", "target_branch": "main", "title": "Nothing"}), "mr").await;
    s.poll_mr(pid, 2).await;
    s.step("empty mr checked", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("merge empty", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!(null), "mr").await;
    s.step("close empty", PUT, &format!("{mrs}/2"), Some(&admin), json!({"state_event": "close"}), "mr").await;

    // D6: squash_option always + explicit squash false
    s.step("squash always", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"squash_option": "always"}), "project").await;
    commit(&mut s, pid, "f3", Some("main"), "Add c", "c.txt").await;
    s.step("squash false mr", POST, &mrs, Some(&admin), json!({"source_branch": "f3", "target_branch": "main", "title": "Add c", "squash": false}), "mr").await;
    s.poll_mr(pid, 3).await;
    s.step("merge without squash", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!(null), "mr").await;
    s.step("after failed merge", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    s.step("merge with squash", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!({"squash": true}), "mr").await;
    s.settle().await;

    // D7: protected source branch is kept
    s.step("squash default", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"squash_option": "default_off"}), "project").await;
    commit(&mut s, pid, "keep", Some("main"), "Add d", "d.txt").await;
    s.step("protect source", POST, &format!("/projects/{pid}/protected_branches"), Some(&admin), json!({"name": "keep"}), "protected_branch").await;
    s.step("protected source mr", POST, &mrs, Some(&admin), json!({"source_branch": "keep", "target_branch": "main", "title": "Keep"}), "mr").await;
    s.poll_mr(pid, 4).await;
    s.step("merge removing protected", PUT, &format!("{mrs}/4/merge"), Some(&admin), json!({"should_remove_source_branch": true}), "mr").await;
    s.settle().await;
    s.step("protected branch kept", GET, &format!("/projects/{pid}/repository/branches/keep"), Some(&admin), json!(null), "branch").await;
    s.finish();
}

#[tokio::test]
async fn mr_list_filters() {
    let mut s = Scenario::new("mr_list_filters").await;
    let admin = s.admin();
    let (pid, _, _) = setup(&mut s, "lists").await;
    let mrs = format!("/projects/{pid}/merge_requests");
    for (name, color) in [("bug", "#ff0000"), ("ui", "#00ff00")] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": name, "color": color}), "label").await;
    }
    let specs = [
        ("b1", "alpha", "", "bug,ui"),
        ("b2", "Alpha", "", "bug"),
        ("b3", "beta", "needle in text", ""),
        ("b4", "alpha", "", "ui"),
    ];
    for (i, (branch, title, desc, labels)) in specs.iter().enumerate() {
        commit(&mut s, pid, branch, Some("main"), &format!("Commit {branch}"), &format!("{branch}.txt")).await;
        s.step("mr", POST, &mrs, Some(&admin),
            json!({"source_branch": branch, "target_branch": "main", "title": title, "description": desc, "labels": labels}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("close mr4", PUT, &format!("{mrs}/4"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    let q = |s: &str| -> Value {
        let mut m = serde_json::Map::new();
        for pair in s.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap();
            m.insert(k.to_string(), json!(v));
        }
        Value::Object(m)
    };
    for query in [
        "labels=None",
        "labels=none",
        "labels=Any",
        "labels=None,bug",
        "labels=bug,ui",
        "labels=bug,ghost",
        "labels=",
        "labels= bug ",
        "not[labels]=bug,ui",
        "labels=bug&not[labels]=ui",
        "not[labels]=ghost",
        "search=alpha",
        "search=needle text",
        "search=alpha needle",
        "search=needle&in=title",
        "search=needle&in=bogus",
        "search=al",
        "search=al&order_by=title",
        "order_by=title&sort=asc",
        "order_by=title&sort=desc",
        "iids[]=1&iids[]=3&per_page=1&page=2",
        "iids[]=999",
        "iids[]=a",
        "wip=maybe",
        "state=locked",
        "state=bogus",
        "order_by=bogus",
    ] {
        s.step(&format!("list {query}"), GET, &mrs, Some(&admin), q(query), "mr").await;
    }
    s.finish();
}
