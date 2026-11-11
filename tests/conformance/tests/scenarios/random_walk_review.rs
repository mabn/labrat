//! A second fixed-seed random walk, focused on the review flow: draft notes, diff threads,
//! suggestions, pushes that outdate positions, and a merge at the end.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

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
async fn random_walk_review() {
    walk("random_walk_review", 0x2545f4914f6cdd1d, 40).await;
}

#[tokio::test]
async fn random_walk_review_2() {
    walk("random_walk_review_2", 0x6a09e667f3bcc909, 50).await;
}

#[tokio::test]
async fn random_walk_review_3() {
    walk("random_walk_review_3", 0x510e527fade682d2, 60).await;
}

#[tokio::test]
async fn random_walk_review_4() {
    walk("random_walk_review_4", 0x9b05688c2b3e6c21, 60).await;
}

async fn walk(name: &str, seed: u64, steps: usize) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let gpath = s.name("rv");
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
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let content = |n: usize| (1..=8).map(|i| format!("line {i} v{n}\n")).collect::<String>();
    s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": content(0)}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let mr = format!("{mrs}/1");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;

    let users = [("admin", admin.clone()), ("bob", bob_t.clone())];
    let mut rng = Rng(seed);
    let mut threads: Vec<String> = vec![];
    let mut push_n = 0;
    for step in 0..steps {
        let (who, tok) = users[rng.pick(2)].clone();
        let op = rng.pick(12);
        let label = format!("{step:02} {who} op{op}");
        s.poll_mr(pid, 1).await;
        match op {
            0 => {
                s.step(&label, POST, &format!("{mr}/draft_notes"), Some(&tok), json!({"note": format!("Draft {step}")}), "draft_note").await;
            }
            1 if !threads.is_empty() => {
                let t = threads[rng.pick(threads.len())].clone();
                s.step(&label, POST, &format!("{mr}/draft_notes"), Some(&tok), json!({"note": format!("Draft reply {step}"), "in_reply_to_discussion_id": t}), "draft_note").await;
            }
            2 => {
                s.step(&label, POST, &format!("{mr}/draft_notes/bulk_publish"), Some(&tok), json!(null), "draft_note").await;
            }
            3 | 4 => {
                let refs = s.send(GET, &mr, Some(&admin), json!(null)).await.body["diff_refs"].clone();
                let line = rng.pick(8) as i64 + 1;
                let body = if op == 3 {
                    format!("Diff comment {step}")
                } else {
                    format!("Try:\n```suggestion:-0+0\nline {line} suggested {step}\n```")
                };
                let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"],
                                 "position_type": "text", "new_path": "a.txt", "old_path": "a.txt", "new_line": line});
                let d = s.step(&label, POST, &format!("{mr}/discussions"), Some(&tok), json!({"body": body, "position": pos}), "discussion").await;
                if let Some(id) = d.body["id"].as_str() {
                    threads.push(id.to_string());
                }
            }
            5 => {
                let all = s.send(GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100})).await;
                let open: Vec<Value> = all
                    .body
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|d| d["notes"].as_array().cloned().unwrap_or_default())
                    .flat_map(|n| n["suggestions"].as_array().cloned().unwrap_or_default())
                    .filter(|sg| sg["applicable"] == true)
                    .collect();
                if open.is_empty() {
                    s.step(&label, GET, &format!("{mr}/discussions"), Some(&tok), json!({"per_page": 100}), "discussion").await;
                } else {
                    let sg = &open[rng.pick(open.len())];
                    s.step(&label, PUT, &format!("/suggestions/{}/apply", sg["id"]), Some(&tok), json!(null), "suggestion").await;
                    let sha = s.send(GET, &format!("/projects/{pid}/repository/branches/feature"), Some(&admin), json!(null)).await.body["commit"]["id"].clone();
                    s.poll(&mr, move |b| b["sha"] == sha && b["merge_status"] == "can_be_merged").await;
                }
            }
            6 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&tok), json!(null), "approval").await;
            }
            7 => {
                push_n += 1;
                let c = s
                    .step(&label, POST, &commits, Some(&admin), json!({"branch": "feature", "commit_message": format!("Rewrite {push_n}"), "actions": [{"action": "update", "file_path": "a.txt", "content": content(push_n)}]}), "commit")
                    .await;
                let sha = c.get("id").clone();
                s.poll(&mr, move |b| b["sha"] == sha && b["merge_status"] == "can_be_merged").await;
            }
            8 if !threads.is_empty() => {
                let t = threads[rng.pick(threads.len())].clone();
                s.step(&label, PUT, &format!("{mr}/discussions/{t}"), Some(&tok), json!({"resolved": rng.pick(2) == 0}), "discussion").await;
            }
            9 => {
                let drafts = s.send(GET, &format!("{mr}/draft_notes"), Some(&tok), json!(null)).await;
                match drafts.body.as_array().and_then(|a| a.first().cloned()) {
                    Some(d) => {
                        s.step(&label, DELETE, &format!("{mr}/draft_notes/{}", d["id"]), Some(&tok), json!(null), "draft_note").await;
                    }
                    None => {
                        s.step(&label, GET, &format!("{mr}/draft_notes"), Some(&tok), json!(null), "draft_note").await;
                    }
                }
            }
            10 => {
                s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": format!("/submit_review requested_changes\nNeeds work {step}")}), "note").await;
            }
            _ => {
                s.step(&label, GET, &format!("{mr}/reviewers"), Some(&tok), json!(null), "reviewer").await;
            }
        }
        s.settle().await;
        if step % 10 == 9 {
            s.poll_mr(pid, 1).await;
            s.step(&format!("state {step}"), GET, &mr, Some(&admin), json!(null), "mr").await;
        }
    }
    s.poll_mr(pid, 1).await;
    s.step("final mr", GET, &mr, Some(&admin), json!(null), "mr").await;
    s.step("final notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.step("final discussions", GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100}), "discussion").await;
    s.step("final reviewers", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;
    s.step("final versions", GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
    s.step("bob drafts", GET, &format!("{mr}/draft_notes"), Some(&bob_t), json!(null), "draft_note").await;
    s.step("admin drafts", GET, &format!("{mr}/draft_notes"), Some(&admin), json!(null), "draft_note").await;
    s.step("merge", PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.finish();
}
