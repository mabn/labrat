//! A fixed-seed random walk over merge request and review operations, with the MR's full state
//! checked along the way. It finds interactions that hand-written scenarios miss.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

/// A tiny deterministic generator (xorshift), so the walk is the same on every run.
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
async fn random_walk() {
    let mut s = Scenario::new("random_walk").await;
    let admin = s.admin();
    let gpath = s.name("rw");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    for l in ["bug", "ui"] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": "#00ff00"}), "label").await;
    }
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\nthree\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let mr = format!("{mrs}/1");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;

    let users = [("admin", admin.clone()), ("bob", bob_t.clone())];
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut threads: Vec<String> = vec![];
    let mut commit_n = 0;
    for step in 0..40 {
        let (who, tok) = users[rng.pick(2)].clone();
        let op = rng.pick(14);
        let label = format!("{step:02} {who} op{op}");
        // Late background work on GitLab can leave the MR unchecked; settle it first.
        s.poll_mr(pid, 1).await;
        match op {
            0 => {
                s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": format!("Comment {step}")}), "note").await;
            }
            1 => {
                let d = s.step(&label, POST, &format!("{mr}/discussions"), Some(&tok), json!({"body": format!("Thread {step}")}), "discussion").await;
                if let Some(id) = d.body["id"].as_str() {
                    threads.push(id.to_string());
                }
            }
            2 if !threads.is_empty() => {
                let t = threads[rng.pick(threads.len())].clone();
                s.step(&label, POST, &format!("{mr}/discussions/{t}/notes"), Some(&tok), json!({"body": format!("Reply {step}")}), "note").await;
            }
            3 if !threads.is_empty() => {
                let t = threads[rng.pick(threads.len())].clone();
                let resolved = rng.pick(2) == 0;
                s.step(&label, PUT, &format!("{mr}/discussions/{t}"), Some(&tok), json!({"resolved": resolved}), "discussion").await;
            }
            4 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&tok), json!(null), "approval").await;
            }
            5 => {
                s.step(&label, POST, &format!("{mr}/unapprove"), Some(&tok), json!(null), "approval").await;
            }
            6 => {
                let l = ["bug", "ui"][rng.pick(2)];
                let key = if rng.pick(2) == 0 { "add_labels" } else { "remove_labels" };
                s.step(&label, PUT, &mr, Some(&tok), json!({key: l}), "mr").await;
            }
            7 => {
                let body = if rng.pick(2) == 0 { json!({"reviewer_ids": [bob]}) } else { json!({"assignee_ids": [bob]}) };
                s.step(&label, PUT, &mr, Some(&tok), body, "mr").await;
            }
            8 => {
                let body = if rng.pick(2) == 0 { "/draft" } else { "/ready" };
                s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": body}), "note").await;
            }
            9 => {
                let name = ["thumbsup", "tada", "eyes"][rng.pick(3)];
                s.step(&label, POST, &format!("{mr}/award_emoji"), Some(&tok), json!({"name": name}), "award").await;
            }
            10 => {
                commit_n += 1;
                let c = s.step(&label, POST, &commits, Some(&admin), json!({"branch": "feature", "commit_message": format!("More {commit_n}"), "actions": [{"action": "create", "file_path": format!("more{commit_n}.txt"), "content": "m\n"}]}), "commit").await;
                // Wait for the refresh to see the push, then for the mergeability check.
                let sha = c.get("id").clone();
                s.poll(&mr, move |b| b["sha"] == sha && b["merge_status"] == "can_be_merged").await;
            }
            11 => {
                let ev = if rng.pick(2) == 0 { "close" } else { "reopen" };
                s.step(&label, PUT, &mr, Some(&tok), json!({"state_event": ev}), "mr").await;
            }
            12 => {
                s.step(&label, PUT, &mr, Some(&tok), json!({"title": format!("Title {step}"), "description": format!("cc @{username} step {step}")}), "mr").await;
            }
            _ => {
                let v = s.send(GET, &format!("{mr}/award_emoji"), Some(&tok), json!(null)).await;
                let mine = v.body.as_array().and_then(|a| a.iter().find(|x| x["user"]["username"] == if who == "bob" { json!(username) } else { json!("claude") }).cloned());
                match mine {
                    Some(a) => {
                        s.step(&label, DELETE, &format!("{mr}/award_emoji/{}", a["id"]), Some(&tok), json!(null), "award").await;
                    }
                    None => {
                        s.step(&label, GET, &format!("{mr}/award_emoji"), Some(&tok), json!(null), "award").await;
                    }
                }
            }
        }
        s.settle().await;
        if step % 10 == 9 {
            s.step(&format!("state {step}"), GET, &mr, Some(&admin), json!(null), "mr").await;
        }
    }
    s.step("final mr", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("final notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.step("final discussions", GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100}), "discussion").await;
    s.step("final approvals", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("final label events", GET, &format!("{mr}/resource_label_events"), Some(&admin), json!(null), "label_event").await;
    s.step("final state events", GET, &format!("{mr}/resource_state_events"), Some(&admin), json!(null), "state_event").await;
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!({"project_id": pid}), "todo").await;
    s.finish();
}
