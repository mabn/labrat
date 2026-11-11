//! A fixed-seed random walk mixing everything around merge requests: commits, quick-action
//! notes, mentions, review requests, approvals, draft reviews, labels, state changes and merges
//! across three MRs and three users. Full MR state, notes, to-dos and participants are compared
//! at checkpoints.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[tokio::test]
async fn random_walk_mixed() {
    walk("random_walk_mixed", 0x510e527fade682d1, false, false).await;
}

#[tokio::test]
async fn random_walk_mixed_2() {
    walk("random_walk_mixed_2", 0x9b05688c2b3e6c1f, false, false).await;
}

/// The mixed walk with a project webhook for every event type, its deliveries compared after
/// each step.
#[tokio::test]
async fn random_walk_mixed_hooks() {
    walk("random_walk_mixed_hooks", 0x1f83d9abfb41bd6b, true, false).await;
}

#[tokio::test]
async fn random_walk_mixed_hooks_2() {
    walk("random_walk_mixed_hooks_2", 0x428a2f98d728ae22, true, false).await;
}

/// The mixed walk through GraphQL mutations where one exists (notes with quick actions, MR
/// updates, labels, reviewers, award emoji, merges), with webhooks.
#[tokio::test]
async fn random_walk_mixed_graphql() {
    walk("random_walk_mixed_graphql", 0x5be0cd19137e2179, true, true).await;
}

#[tokio::test]
async fn random_walk_mixed_graphql_2() {
    walk("random_walk_mixed_graphql_2", 0x6a09e667f3bcc908, false, true).await;
}

#[tokio::test]
async fn random_walk_mixed_3() {
    walk("random_walk_mixed_3", 0xbb67ae8584caa73b, true, false).await;
}

#[tokio::test]
async fn random_walk_mixed_4() {
    walk("random_walk_mixed_4", 0x3c6ef372fe94f82b, true, true).await;
}

#[tokio::test]
async fn random_walk_mixed_5() {
    walk("random_walk_mixed_5", 0xa54ff53a5f1d36f1, true, false).await;
}

#[tokio::test]
async fn random_walk_mixed_graphql_3() {
    walk("random_walk_mixed_graphql_3", 0x1f83d9abfb41bd6c, true, true).await;
}

#[tokio::test]
async fn random_walk_mixed_6() {
    walk("random_walk_mixed_6", 0x5be0cd19137e217a, true, false).await;
}

async fn walk(name: &str, seed: u64, hooks: bool, gql: bool) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let mut rx = if hooks {
        let rx = s.receiver().await;
        let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        if s.target.recording {
            tokio::time::sleep(std::time::Duration::from_secs(65)).await;
        }
        Some(rx)
    } else {
        None
    };
    let gpath = s.name("rx");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
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
        s.step(&format!("{who} member"), POST, &format!("/groups/{gid}/members"), Some(&admin), json!({"user_id": id, "access_level": 30}), "user").await;
        people.push((username, id, tok));
    }
    let mut label_gids = vec![];
    for (l, col) in [("bug", "#ff0000"), ("ui", "#00ff00")] {
        label_gids.push(format!("gid://gitlab/ProjectLabel/{}", s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": col}), "label").await.id()));
    }
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let mut mr_gids: Vec<String> = vec![];
    for (i, b) in ["m1", "m2", "m3"].iter().enumerate() {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "1\n2\n3\n"}]}), "commit").await;
        s.settle().await;
        let m = s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        mr_gids.push(format!("gid://gitlab/MergeRequest/{}", m.id()));
        s.poll_mr(pid, i as i64 + 1).await;
    }
    if let Some(rx) = &rx {
        let url = format!("{}/hook", rx.url);
        s.step(
            "hook",
            POST,
            &format!("/projects/{pid}/hooks"),
            Some(&admin),
            json!({"url": url, "push_events": true, "merge_requests_events": true, "note_events": true, "emoji_events": true, "pipeline_events": true}),
            "hook",
        )
        .await;
    }
    let actors: Vec<(String, String)> = vec![("admin".into(), admin.clone()), ("bob".into(), people[0].2.clone()), ("cat".into(), people[1].2.clone())];
    let names = [s.name("claude").replace(&format!("-{}", s.run), ""), people[0].0.clone(), people[1].0.clone()];
    let quick = [
        "/label ~bug", "/label ~ui", "/unlabel ~bug", "/assign_reviewer @{u}", "/unassign_reviewer @{u}", "/assign @{u}", "/unassign",
        "/approve", "/unapprove", "/draft", "/ready", "/todo", "/done", "/request_review @{u}", "/title Retitled {n}", "/close", "/reopen",
        "/submit_review", "/subscribe",
    ];
    let mut rng = Rng(seed);
    let mut merged = [false; 3];
    let mut pushes = 0;
    for step in 0..60 {
        let a = rng.pick(3);
        let (who, tok) = actors[a].clone();
        let k = rng.pick(3);
        let iid = k + 1;
        let mr = format!("{mrs}/{iid}");
        let target = names[rng.pick(3)].clone();
        let op = rng.pick(10);
        let label = format!("{step:02} {who} mr{iid} op{op}");
        if merged[k] && op != 9 {
            s.step(&label, GET, &mr, Some(&tok), json!(null), "mr").await;
            continue;
        }
        match op {
            0..=3 => {
                let q = quick[rng.pick(quick.len())].replace("{u}", &target).replace("{n}", &step.to_string());
                let text = if rng.pick(3) == 0 { format!("@{target} {q}") } else { q };
                if gql {
                    s.graphql(
                        &label,
                        Some(&tok),
                        "mutation($id: NoteableID!, $b: String!) { createNote(input: {noteableId: $id, body: $b}) { errors note { body system } quickActionsStatus { messages errorMessages commandNames } } }",
                        json!({"id": mr_gids[k], "b": text}),
                    )
                    .await;
                } else {
                    s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": text}), "note").await;
                }
            }
            4 => {
                pushes += 1;
                let file = format!("m{iid}.txt");
                s.step(&label, POST, &commits, Some(&tok), json!({"branch": format!("m{iid}"), "commit_message": format!("Push {pushes}"), "actions": [{"action": "update", "file_path": file, "content": format!("1\n2 {pushes}\n3\n")}]}), "commit").await;
                s.settle().await;
                s.poll_mr(pid, iid as i64).await;
            }
            5 => {
                s.step(&label, POST, &format!("{mr}/draft_notes"), Some(&tok), json!({"note": format!("Draft {step} for @{target}")}), "draft").await;
                if rng.pick(2) == 0 {
                    s.step(&format!("{label} publish"), POST, &format!("{mr}/draft_notes/bulk_publish"), Some(&tok), json!(null), "draft").await;
                }
            }
            6 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&tok), json!(null), "approval").await;
            }
            7 if gql => {
                let iid_s = iid.to_string();
                match rng.pick(4) {
                    0 => {
                        s.graphql(&label, Some(&tok), "mutation($p: ID!, $i: String!, $d: String) { mergeRequestUpdate(input: {projectPath: $p, iid: $i, description: $d}) { errors mergeRequest { description } } }",
                            json!({"p": full, "i": iid_s, "d": format!("Desc {step} cc @{target}")})).await;
                    }
                    1 => {
                        s.graphql(&label, Some(&tok), "mutation($p: ID!, $i: String!, $s: MergeRequestNewState) { mergeRequestUpdate(input: {projectPath: $p, iid: $i, state: $s}) { errors mergeRequest { state } } }",
                            json!({"p": full, "i": iid_s, "s": (["CLOSED", "OPENED"][rng.pick(2)])})).await;
                    }
                    2 => {
                        s.graphql(&label, Some(&tok), "mutation($p: ID!, $i: String!, $u: [String!]!) { mergeRequestSetReviewers(input: {projectPath: $p, iid: $i, reviewerUsernames: $u}) { errors mergeRequest { reviewers { nodes { username } } } } }",
                            json!({"p": full, "i": iid_s, "u": [people[rng.pick(2)].0]})).await;
                    }
                    _ => {
                        s.graphql(&label, Some(&tok), "mutation($p: ID!, $i: String!, $l: [LabelID!]!, $m: MutationOperationMode) { mergeRequestSetLabels(input: {projectPath: $p, iid: $i, labelIds: $l, operationMode: $m}) { errors mergeRequest { labels { nodes { title } } } } }",
                            json!({"p": full, "i": iid_s, "l": [label_gids[rng.pick(2)]], "m": (["APPEND", "REMOVE", "REPLACE"][rng.pick(3)])})).await;
                    }
                }
            }
            7 => {
                let body = match rng.pick(3) {
                    0 => json!({"add_labels": "ui", "description": format!("Desc {step} cc @{target}")}),
                    1 => json!({"reviewer_ids": [people[rng.pick(2)].1]}),
                    _ => json!({"state_event": (["close", "reopen"][rng.pick(2)])}),
                };
                s.step(&label, PUT, &mr, Some(&tok), body, "mr").await;
            }
            8 => {
                s.graphql(
                    &label,
                    Some(&tok),
                    "mutation($p: ID!, $i: String!, $d: Boolean!) { mergeRequestSetDraft(input: {projectPath: $p, iid: $i, draft: $d}) { errors mergeRequest { draft title } } }",
                    json!({"p": full, "i": iid.to_string(), "d": rng.pick(2) == 0}),
                )
                .await;
            }
            _ => {
                if step > 40 && !merged[k] {
                    s.poll_mr(pid, iid as i64).await;
                    if gql {
                        let sha = s.send(GET, &mr, Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
                        let r = s
                            .graphql(&label, Some(&admin), "mutation($p: ID!, $i: String!, $s: String!) { mergeRequestAccept(input: {projectPath: $p, iid: $i, sha: $s}) { errors mergeRequest { state } } }", json!({"p": full, "i": iid.to_string(), "s": sha}))
                            .await;
                        if r.body["data"]["mergeRequestAccept"]["mergeRequest"]["state"] == "merged" {
                            merged[k] = true;
                        }
                    } else {
                        let r = s.step(&label, PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
                        if r.status == 200 {
                            merged[k] = true;
                        }
                    }
                } else if gql {
                    s.graphql(&label, Some(&tok), "mutation($id: AwardableID!) { awardEmojiToggle(input: {awardableId: $id, name: \"thumbsup\"}) { errors toggledOn } }", json!({"id": mr_gids[k]})).await;
                } else {
                    s.step(&label, POST, &format!("{mr}/award_emoji"), Some(&tok), json!({"name": "thumbsup"}), "award").await;
                }
            }
        }
        s.settle().await;
        if let Some(rx) = rx.as_mut() {
            s.deliveries(&format!("{label} hooks"), rx, 0).await;
        }
        if step % 15 == 14 || step == 59 {
            for i in 1..=3 {
                s.step(&format!("cp{step} mr{i}"), GET, &format!("{mrs}/{i}"), Some(&admin), json!(null), "mr").await;
                let r = s.send(GET, &format!("{mrs}/{i}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100})).await;
                let compact: Vec<Value> = r.body.as_array().into_iter().flatten().map(|n| json!({"body": n["body"], "author": n["author"]["username"]})).collect();
                let res = labrat_conformance::Res { status: r.status, headers: Default::default(), body: json!(compact) };
                s.record(&format!("cp{step} mr{i} notes"), &GET, "/notes", &res, "list");
                s.step(&format!("cp{step} mr{i} participants"), GET, &format!("{mrs}/{i}/participants"), Some(&admin), json!(null), "user").await;
                s.step(&format!("cp{step} mr{i} approvals"), GET, &format!("{mrs}/{i}/approvals"), Some(&admin), json!(null), "approval").await;
            }
            for (name, tok) in &actors {
                let r = s.send(GET, "/todos", Some(tok), json!({"per_page": 100, "project_id": pid})).await;
                let compact: Vec<Value> = r.body.as_array().into_iter().flatten().map(|t| json!({"action": t["action_name"], "iid": t["target"]["iid"], "author": t["author"]["username"]})).collect();
                let res = labrat_conformance::Res { status: r.status, headers: Default::default(), body: json!(compact) };
                s.record(&format!("cp{step} {name} todos"), &GET, "/todos", &res, "list");
                // The admin's counts cover every project on the reference instance.
                if name != "admin" {
                    s.step(&format!("cp{step} {name} counts"), GET, "/user_counts", Some(tok), json!(null), "counts").await;
                }
            }
        }
    }
    s.finish();
}
