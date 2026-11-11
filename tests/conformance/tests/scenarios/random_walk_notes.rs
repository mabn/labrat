//! A fixed-seed random walk over note content: bodies combine user and group mentions, merge
//! request references and quick actions; notes are created, edited and deleted. Notes, to-dos
//! and participants are compared at checkpoints.

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
async fn random_walk_notes() {
    walk("random_walk_notes", 0xbb67ae8584caa73b, false).await;
}

#[tokio::test]
async fn random_walk_notes_2() {
    // Description edits mention no groups: GitLab races between resolving the editor's to-dos and
    // creating their self-mention to-do (see PLAN.md), so the outcome isn't reproducible.
    walk("random_walk_notes_2", 0x3956c25bf348b538, true).await;
}

#[tokio::test]
async fn random_walk_notes_3() {
    // Without group mentions and self-mentions in description edits: GitLab races there (PLAN.md).
    walk("random_walk_notes_3", 0x9b05688c2b3e6c20, true).await;
}

#[tokio::test]
async fn random_walk_notes_4() {
    // Without group mentions and self-mentions in description edits: GitLab races there (PLAN.md).
    walk("random_walk_notes_4", 0x1f83d9abfb41bd6d, true).await;
}

async fn walk(name: &str, seed: u64, no_group_in_descriptions: bool) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let gpath = s.name("rn");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
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
    s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000"}), "label").await;
    s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for (i, b) in ["n1", "n2", "n3"].iter().enumerate() {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    let actors: Vec<(String, String)> = vec![("admin".into(), admin.clone()), ("bob".into(), people[0].2.clone()), ("cat".into(), people[1].2.clone())];
    let mention = |rng: &mut Rng| -> String {
        match rng.pick(5) {
            0 => format!("@{}", people[0].0),
            1 => format!("@{}", people[1].0),
            2 => format!("@{gpath}"),
            3 => format!("!{}", 1 + rng.pick(3)),
            _ => "plain words".to_string(),
        }
    };
    let quick = ["", "", "/label ~bug", "/unlabel ~bug", "/milestone %v1", "/remove_milestone", "/todo", "/done", "/draft", "/ready", "/copy_metadata !1", "/title Renamed", "/subscribe", "/unsubscribe"];
    let mut rng = Rng(seed);
    let mut mine: Vec<(String, i64, usize)> = vec![]; // (iid path, note id, actor)
    for step in 0..45 {
        let a = rng.pick(3);
        let (who, tok) = actors[a].clone();
        let iid = 1 + rng.pick(3);
        let mr = format!("{mrs}/{iid}");
        let op = rng.pick(6);
        let label = format!("{step:02} {who} mr{iid} op{op}");
        match op {
            0..=2 => {
                let lead = if rng.pick(3) == 0 { format!("{} ", mention(&mut rng)) } else { String::new() };
                let body = format!("{lead}Note {step} about {} and {}\n{}", mention(&mut rng), mention(&mut rng), quick[rng.pick(quick.len())]);
                let r = s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": body.trim_end()}), "note").await;
                if r.status == 201
                    && let Some(id) = r.body["id"].as_i64()
                {
                    mine.push((mr.clone(), id, a));
                }
            }
            3 if !mine.is_empty() => {
                let (path, id, owner) = mine[rng.pick(mine.len())].clone();
                let body = format!("Edited {step}: {} {}", mention(&mut rng), mention(&mut rng));
                s.step(&label, PUT, &format!("{path}/notes/{id}"), Some(&actors[owner].1), json!({"body": body}), "note").await;
            }
            4 if !mine.is_empty() => {
                let k = rng.pick(mine.len());
                let (path, id, owner) = mine.remove(k);
                s.step(&label, DELETE, &format!("{path}/notes/{id}"), Some(&actors[owner].1), json!(null), "note").await;
            }
            _ => {
                let mut desc = format!("Description {step} {} {}", mention(&mut rng), mention(&mut rng));
                if no_group_in_descriptions {
                    // No group and no self-mention: both can reach the editor, whose to-do GitLab
                    // may or may not resolve (a race).
                    desc = desc.replace(&format!("@{gpath}"), "the group");
                    let me = match a {
                        0 => "claude".to_string(),
                        n => people[n - 1].0.clone(),
                    };
                    desc = desc.replace(&format!("@{me}"), "myself");
                }
                s.step(&label, PUT, &mr, Some(&tok), json!({"description": desc}), "mr").await;
            }
        }
        s.settle().await;
        if step % 15 == 14 {
            for i in 1..=3 {
                let r = s.send(GET, &format!("{mrs}/{i}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100})).await;
                let compact: Vec<Value> = r.body.as_array().into_iter().flatten().map(|n| json!({"body": n["body"], "system": n["system"], "author": n["author"]["username"]})).collect();
                let res = labrat_conformance::Res { status: r.status, headers: Default::default(), body: json!(compact) };
                s.record(&format!("cp{step} mr{i} notes"), &GET, "/notes", &res, "list");
                s.step(&format!("cp{step} mr{i}"), GET, &format!("{mrs}/{i}"), Some(&admin), json!(null), "mr").await;
                s.step(&format!("cp{step} mr{i} participants"), GET, &format!("{mrs}/{i}/participants"), Some(&admin), json!(null), "user").await;
            }
            for (name, tok) in &actors {
                let r = s.send(GET, "/todos", Some(tok), json!({"per_page": 100, "project_id": pid})).await;
                let compact: Vec<Value> = r.body.as_array().into_iter().flatten().map(|t| json!({"action": t["action_name"], "iid": t["target"]["iid"], "author": t["author"]["username"], "body": t["body"]})).collect();
                let res = labrat_conformance::Res { status: r.status, headers: Default::default(), body: json!(compact) };
                s.record(&format!("cp{step} {name} todos"), &GET, "/todos", &res, "list");
            }
        }
    }
    s.finish();
}
