//! A fixed-seed random walk over several merge requests, compared through merge request list
//! filters and sort orders (REST and GraphQL) at checkpoints.

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
async fn random_walk_lists() {
    let mut s = Scenario::new("random_walk_lists").await;
    let admin = s.admin();
    let gpath = s.name("rl");
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
    for l in ["bug", "ui"] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": "#00ff00"}), "label").await;
    }
    let ms = s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await.id();
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let titles = ["Fix parser", "Add feature", "Refactor core", "Update docs", "Fix tests", "Improve speed"];
    for (i, t) in titles.iter().enumerate() {
        let b = format!("b{i}");
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": t, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        let author = if i % 2 == 0 { &admin } else { &bob_t };
        s.step("mr", POST, &mrs, Some(author), json!({"source_branch": b, "target_branch": "main", "title": t}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    let users = [("admin", admin.clone()), ("bob", bob_t.clone())];
    let mut rng = Rng(0x8cb92ba72f3d8dd7);
    for step in 0..36 {
        let (who, tok) = users[rng.pick(2)].clone();
        let iid = rng.pick(titles.len()) + 1;
        let mr = format!("{mrs}/{iid}");
        let op = rng.pick(11);
        let label = format!("{step:02} {who} mr{iid} op{op}");
        let body: Value = match op {
            0 => json!({"state_event": (["close", "reopen"][rng.pick(2)])}),
            1 => json!({"add_labels": (["bug", "ui"][rng.pick(2)])}),
            2 => json!({"remove_labels": (["bug", "ui"][rng.pick(2)])}),
            3 => json!({"assignee_ids": if rng.pick(2) == 0 { json!([bob]) } else { json!([]) }}),
            4 => json!({"reviewer_ids": if rng.pick(2) == 0 { json!([bob]) } else { json!([]) }}),
            5 => json!({"milestone_id": if rng.pick(2) == 0 { json!(ms) } else { json!(0) }}),
            6 => {
                let t = titles[iid - 1];
                json!({"title": if rng.pick(2) == 0 { format!("Draft: {t}") } else { t.to_string() }})
            }
            7 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&tok), json!(null), "approval").await;
                s.settle().await;
                continue;
            }
            8 => {
                s.step(&label, POST, &format!("{mr}/award_emoji"), Some(&tok), json!({"name": (["thumbsup", "thumbsdown"][rng.pick(2)])}), "award").await;
                s.settle().await;
                continue;
            }
            9 if step > 20 => {
                s.poll_mr(pid, iid as i64).await;
                s.step(&label, PUT, &format!("{mr}/merge"), Some(&admin), json!(null), "mr").await;
                s.settle().await;
                continue;
            }
            _ => json!({"description": format!("Notes about speed and parser, step {step}")}),
        };
        s.step(&label, PUT, &mr, Some(&tok), body, "mr").await;
        s.settle().await;
        if step % 12 == 11 {
            checkpoint(&mut s, &mrs, &full, &bob_t, &username, step).await;
        }
    }
    checkpoint(&mut s, &mrs, &full, &bob_t, &username, 99).await;
    s.finish();
}

/// Lists compared at a checkpoint: only iids, so differences point at filtering and order.
async fn checkpoint(s: &mut Scenario, mrs: &str, full: &str, bob_t: &str, bob_name: &str, step: usize) {
    let admin = s.admin();
    let rest: Vec<(&str, Value)> = vec![
        ("default", json!({})),
        ("all", json!({"state": "all"})),
        ("closed", json!({"state": "closed"})),
        ("merged", json!({"state": "merged"})),
        ("label bug", json!({"labels": "bug", "state": "all"})),
        ("labels none", json!({"labels": "None"})),
        ("labels any", json!({"labels": "Any", "state": "all"})),
        ("not label ui", json!({"not[labels]": "ui", "state": "all"})),
        ("reviewer bob", json!({"reviewer_username": bob_name, "state": "all"})),
        ("reviewer none", json!({"reviewer_id": "None"})),
        ("assignee any", json!({"assignee_id": "Any", "state": "all"})),
        ("author bob", json!({"author_username": bob_name, "state": "all"})),
        ("milestone v1", json!({"milestone": "v1", "state": "all"})),
        ("milestone none", json!({"milestone": "None"})),
        ("drafts", json!({"wip": "yes", "state": "all"})),
        ("not drafts", json!({"wip": "no"})),
        ("search fix", json!({"search": "fix", "state": "all"})),
        ("search speed in description", json!({"search": "speed", "in": "description", "state": "all"})),
        ("updated asc", json!({"order_by": "updated_at", "sort": "asc", "state": "all"})),
        ("title", json!({"order_by": "title", "sort": "asc", "state": "all"})),
        ("merged at", json!({"order_by": "merged_at", "state": "merged"})),
        ("popularity", json!({"order_by": "popularity", "state": "all"})),
        ("my reaction", json!({"my_reaction_emoji": "thumbsup", "state": "all"})),
    ];
    for (name, mut q) in rest {
        q["per_page"] = json!(100);
        let token = if name == "my reaction" { bob_t.to_string() } else { admin.clone() };
        let r = s.send(GET, mrs, Some(&token), q.clone()).await;
        let iids: Vec<Value> = r.body.as_array().map(|a| a.iter().map(|m| m["iid"].clone()).collect()).unwrap_or_default();
        // Recorded as a compact list of iids (plus the total) rather than full bodies.
        let res = labrat_conformance::Res { status: r.status, headers: Default::default(), body: json!({"iids": iids, "total": r.headers.get("x-total").and_then(|v| v.to_str().ok())}) };
        s.record(&format!("cp{step} {name}"), &GET, mrs, &res, "list");
    }
    let gql = [
        ("gql reviewer", format!("mergeRequests(reviewerUsername: \"{bob_name}\") {{ nodes {{ iid }} }}")),
        ("gql labels", "mergeRequests(labels: [\"bug\"], state: all) { nodes { iid } }".to_string()),
        ("gql draft", "mergeRequests(draft: true) { nodes { iid } }".to_string()),
        ("gql sort", "mergeRequests(sort: TITLE_ASC, state: all) { count nodes { iid } }".to_string()),
        ("gql not", "mergeRequests(not: {labels: [\"ui\"]}, state: opened) { nodes { iid } }".to_string()),
    ];
    for (name, sel) in gql {
        let q = format!("query($p: ID!) {{ project(fullPath: $p) {{ {sel} }} }}");
        s.graphql(&format!("cp{step} {name}"), Some(&admin), &q, json!({"p": full})).await;
    }
}
