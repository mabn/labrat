//! A fixed-seed random walk over what creates and resolves to-dos: mentions, assignments,
//! review requests, approvals, failed statuses, merges, comments and explicit done/restore,
//! with each user's to-dos compared at checkpoints.

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
async fn random_walk_todos() {
    let mut s = Scenario::new("random_walk_todos").await;
    let admin = s.admin();
    let gpath = s.name("rt");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut people: Vec<(String, i64, String)> = vec![];
    for who in ["bob", "carol"] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} developer"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": 30}), "user").await;
        people.push((username, id, tok));
    }
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, b) in ["t1", "t2"].iter().enumerate() {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    let actors: Vec<(&str, String)> = vec![("admin", admin.clone()), ("bob", people[0].2.clone()), ("carol", people[1].2.clone())];
    let mut rng = Rng(0x1d8e4e27c47d124f);
    for step in 0..40 {
        let a = rng.pick(3);
        let (who, tok) = (actors[a].0, actors[a].1.clone());
        let iid = rng.pick(2) + 1;
        let mr = format!("{mrs}/{iid}");
        let target = &people[rng.pick(2)];
        let op = rng.pick(12);
        let label = format!("{step:02} {who} mr{iid} op{op}");
        match op {
            0 => {
                s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": format!("Ping @{} please look", target.0)}), "note").await;
            }
            1 => {
                s.step(&label, PUT, &mr, Some(&tok), json!({"description": format!("cc @{} step {step}", target.0)}), "mr").await;
            }
            2 => {
                let ids = if rng.pick(3) == 0 { json!([]) } else { json!([target.1]) };
                s.step(&label, PUT, &mr, Some(&tok), json!({"assignee_ids": ids}), "mr").await;
            }
            3 => {
                let ids = if rng.pick(3) == 0 { json!([]) } else { json!([target.1]) };
                s.step(&label, PUT, &mr, Some(&tok), json!({"reviewer_ids": ids}), "mr").await;
            }
            4 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&tok), json!(null), "approval").await;
            }
            5 => {
                let sha = s.send(GET, &mr, Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
                s.step(&label, POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": (["failed", "success"][rng.pick(2)]), "name": "ci"}), "status").await;
            }
            6 => {
                s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": format!("A comment {step}")}), "note").await;
            }
            7 => {
                let todos = s.send(GET, "/todos", Some(&tok), json!(null)).await;
                match todos.body.as_array().and_then(|t| t.first()).and_then(|t| t["id"].as_i64()) {
                    Some(id) => {
                        s.step(&label, POST, &format!("/todos/{id}/mark_as_done"), Some(&tok), json!(null), "todo").await;
                    }
                    None => {
                        s.step(&label, GET, "/todos", Some(&tok), json!(null), "todo").await;
                    }
                }
            }
            8 => {
                s.step(&label, POST, "/todos/mark_as_done", Some(&tok), json!(null), "todo").await;
            }
            9 => {
                s.step(&label, POST, &format!("{mr}/todo"), Some(&tok), json!(null), "todo").await;
            }
            10 => {
                s.step(&label, PUT, &mr, Some(&tok), json!({"state_event": (["close", "reopen"][rng.pick(2)])}), "mr").await;
            }
            _ if step > 25 => {
                s.poll_mr(pid, iid as i64).await;
                s.step(&label, PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
            }
            _ => {
                s.step(&label, POST, &format!("{mr}/award_emoji"), Some(&tok), json!({"name": "thumbsup"}), "award").await;
            }
        }
        s.settle().await;
        if step % 10 == 9 {
            for (name, tok) in &actors[1..] {
                for state in ["pending", "done"] {
                    let r = s.send(GET, "/todos", Some(tok), json!({"state": state, "per_page": 100})).await;
                    let compact: Vec<Value> = r
                        .body
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|t| json!({"action": t["action_name"], "target": t["target"]["iid"], "author": t["author"]["username"], "body": t["body"]}))
                        .collect();
                    let res = labrat_conformance::Res { status: r.status, headers: Default::default(), body: json!(compact) };
                    s.record(&format!("cp{step} {name} {state}"), &GET, "/todos", &res, "list");
                }
                s.graphql(
                    &format!("cp{step} {name} graphql"),
                    Some(tok),
                    "query { currentUser { todos(state: [pending]) { count nodes { action state target { ... on MergeRequest { iid } } } } } }",
                    json!({}),
                )
                .await;
            }
        }
    }
    let _ = full;
    s.finish();
}
