//! A third fixed-seed random walk over two MRs, mixing REST and GraphQL mutations, commit
//! statuses and merges, with every webhook delivery compared after each step.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const GET: Method = Method::GET;

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
async fn random_walk_hooks() {
    walk("random_walk_hooks", 0xd1b54a32d192ed03, 40, 14, false).await;
}

/// A longer walk with more kinds of operations: quick actions, auto-merge, draft reviews,
/// rebases and squash merges.
#[tokio::test]
async fn random_walk_hooks_2() {
    walk("random_walk_hooks_2", 0x94d049bb133111eb, 50, 20, false).await;
}

/// Another seed, longer.
#[tokio::test]
async fn random_walk_hooks_3() {
    walk("random_walk_hooks_3", 0xbf58476d1ce4e5b9, 60, 20, false).await;
}

/// With a Reporter as a third user.
#[tokio::test]
async fn random_walk_hooks_4() {
    walk("random_walk_hooks_4", 0x9e3779b97f4a7c55, 50, 20, true).await;
}

async fn walk(name: &str, seed: u64, steps: usize, ops: usize, reporter: bool) {
    let mut s = Scenario::new(name).await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("rh");
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
    let mut users = vec![("admin", admin.clone()), ("bob", bob_t.clone())];
    if reporter {
        let cname = s.name("carol");
        let carol = s
            .step("carol", POST, "/users", Some(&admin), json!({"username": cname, "name": "Carol", "email": format!("{cname}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let carol_t = s.step("carol token", POST, &format!("/users/{carol}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step("carol reporter", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": carol, "access_level": 20}), "user").await;
        users.push(("carol", carol_t));
    }
    let mut label_ids = vec![];
    for l in ["bug", "ui"] {
        label_ids.push(s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": "#00ff00"}), "label").await.id());
    }
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let mut mr_gids = vec![];
    for (i, b) in ["f1", "f2"].iter().enumerate() {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Add {b}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        let m = s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        mr_gids.push(format!("gid://gitlab/MergeRequest/{}", m.id()));
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": format!("{}/hook", rx.url), "push_events": true, "merge_requests_events": true, "note_events": true, "emoji_events": true, "pipeline_events": true}),
        "hook",
    )
    .await;

    let mut rng = Rng(seed);
    let mut pushes = 0;
    for step in 0..steps {
        let (who, tok) = users[rng.pick(users.len())].clone();
        let k = rng.pick(2);
        let iid = k + 1;
        let mr = format!("{mrs}/{iid}");
        let branch = ["f1", "f2"][k];
        let op = rng.pick(ops);
        let label = format!("{step:02} {who} mr{iid} op{op}");
        s.poll_mr(pid, iid as i64).await;
        match op {
            0 => {
                s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": format!("Note {step}")}), "note").await;
            }
            1 => {
                let q = "mutation($id: NoteableID!, $b: String!) { createNote(input: {noteableId: $id, body: $b}) { errors note { body } } }";
                s.graphql(&label, Some(&tok), q, json!({"id": mr_gids[k], "b": format!("GraphQL note {step}")})).await;
            }
            2 => {
                let name = ["thumbsup", "rocket"][rng.pick(2)];
                let q = "mutation($id: AwardableID!, $n: String!) { awardEmojiToggle(input: {awardableId: $id, name: $n}) { errors toggledOn } }";
                s.graphql(&label, Some(&tok), q, json!({"id": mr_gids[k], "n": name})).await;
            }
            3 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&tok), json!(null), "approval").await;
            }
            4 => {
                s.step(&label, POST, &format!("{mr}/unapprove"), Some(&tok), json!(null), "approval").await;
            }
            5 => {
                let mode = ["APPEND", "REMOVE", "REPLACE"][rng.pick(3)];
                let l = label_ids[rng.pick(2)];
                let q = "mutation($p: ID!, $iid: String!, $l: [LabelID!]!, $m: MutationOperationMode) { mergeRequestSetLabels(input: {projectPath: $p, iid: $iid, labelIds: $l, operationMode: $m}) { errors mergeRequest { labels { nodes { title } } } } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full, "iid": iid.to_string(), "l": [format!("gid://gitlab/ProjectLabel/{l}")], "m": mode})).await;
            }
            6 => {
                let mode = ["APPEND", "REMOVE"][rng.pick(2)];
                let q = "mutation($p: ID!, $iid: String!, $u: [String!]!, $m: MutationOperationMode) { mergeRequestSetReviewers(input: {projectPath: $p, iid: $iid, reviewerUsernames: $u, operationMode: $m}) { errors mergeRequest { reviewers { nodes { username } } } } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full, "iid": iid.to_string(), "u": [username], "m": mode})).await;
            }
            7 => {
                pushes += 1;
                let c = s
                    .step(&label, POST, &commits, Some(&admin), json!({"branch": branch, "commit_message": format!("Push {pushes}"), "actions": [{"action": "create", "file_path": format!("p{pushes}.txt"), "content": "p\n"}]}), "commit")
                    .await;
                let sha = c.get("id").clone();
                let path = mr.clone();
                s.poll(&path, move |b| b["state"] != "opened" || (b["sha"] == sha && b["merge_status"] == "can_be_merged")).await;
            }
            8 => {
                let sha = s.send(GET, &mr, Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
                let state = ["running", "success", "failed"][rng.pick(3)];
                s.step(&label, POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": state, "name": "ci", "ref": branch}), "status").await;
            }
            9 => {
                let q = "mutation($p: ID!, $iid: String!, $d: Boolean!) { mergeRequestSetDraft(input: {projectPath: $p, iid: $iid, draft: $d}) { errors mergeRequest { title draft } } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full, "iid": iid.to_string(), "d": rng.pick(2) == 0})).await;
            }
            10 => {
                s.step(&label, PUT, &mr, Some(&tok), json!({"description": format!("Updated in step {step}")}), "mr").await;
            }
            11 => {
                let d = s.step(&label, POST, &format!("{mr}/discussions"), Some(&tok), json!({"body": format!("Thread {step}")}), "discussion").await;
                if let Some(id) = d.body["id"].as_str() {
                    s.step(&format!("{label} resolve"), PUT, &format!("{mr}/discussions/{id}"), Some(&admin), json!({"resolved": true}), "discussion").await;
                }
            }
            12 => {
                s.step(&label, PUT, &format!("{mr}/merge"), Some(&tok), json!(null), "mr").await;
            }
            13 => {
                let ev = ["close", "reopen"][rng.pick(2)];
                s.step(&label, PUT, &mr, Some(&tok), json!({"state_event": ev}), "mr").await;
            }
            14 | 15 => {
                let cmds = [
                    "/label ~bug".to_string(),
                    "/unlabel ~ui".to_string(),
                    format!("/assign @{username}"),
                    "/unassign".to_string(),
                    format!("/assign_reviewer @{username}"),
                    "/draft".to_string(),
                    "/ready".to_string(),
                    "/approve".to_string(),
                    "/unapprove".to_string(),
                    format!("/title Retitled {step}"),
                    "/estimate 1h".to_string(),
                    "/spend 30m".to_string(),
                    "/todo".to_string(),
                    "/lock".to_string(),
                    "/unlock".to_string(),
                    "/close".to_string(),
                    "/reopen".to_string(),
                    "/submit_review requested_changes".to_string(),
                ];
                let cmd = &cmds[rng.pick(cmds.len())];
                let body = if op == 14 { cmd.clone() } else { format!("Text {step}\n{cmd}") };
                s.step(&label, POST, &format!("{mr}/notes"), Some(&tok), json!({"body": body}), "note").await;
            }
            16 => {
                s.step(&label, PUT, &format!("{mr}/merge"), Some(&tok), json!({"merge_when_pipeline_succeeds": true}), "mr").await;
            }
            17 => {
                s.step(&label, POST, &format!("{mr}/draft_notes"), Some(&tok), json!({"note": format!("Draft {step}")}), "draft_note").await;
                if rng.pick(2) == 0 {
                    s.step(&format!("{label} publish"), POST, &format!("{mr}/draft_notes/bulk_publish"), Some(&tok), json!(null), "draft_note").await;
                }
            }
            18 => {
                s.step(&label, PUT, &format!("{mr}/rebase"), Some(&tok), json!(null), "rebase").await;
                s.settle().await;
                let path = mr.clone();
                s.poll(&path, |b| b["rebase_in_progress"] != true && b["merge_status"] != "checking" && b["merge_status"] != "unchecked").await;
            }
            _ => {
                s.step(&label, PUT, &format!("{mr}/merge"), Some(&tok), json!({"squash": true}), "mr").await;
            }
        }
        s.settle().await;
        s.deliveries(&format!("{label} hooks"), &mut rx, 0).await;
    }
    for iid in 1..=2 {
        s.step("final mr", GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null), "mr").await;
        s.step("final notes", GET, &format!("{mrs}/{iid}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    }
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.finish();
}
