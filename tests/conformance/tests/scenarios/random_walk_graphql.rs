//! A fixed-seed random walk that reviews through GraphQL mutations and, at checkpoints, compares
//! one large merge request query as both users.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;

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

const MR_QUERY: &str = "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") {
  state mergeStatus detailedMergeStatus approved approvedBy { nodes { username } }
  reviewers { nodes { username mergeRequestInteraction { reviewState approved canMerge } } }
  subscribed milestone { title } discussionLocked userNotesCount userDiscussionsCount
  resolvableDiscussionsCount resolvedDiscussionsCount participants { nodes { username } }
  currentUserTodos { nodes { action state } } awardEmoji { nodes { name user { username } } }
  userPermissions { canMerge resolveNote createNote adminMergeRequest updateMergeRequest }
  discussions { nodes { resolved resolvable resolvedBy { username } notes { nodes {
    body system author { username } position { newLine oldLine newPath positionType }
    awardEmoji { nodes { name } } userPermissions { adminNote resolveNote awardEmoji } } } } }
} } }";

struct NoteRef {
    gid: String,
    author: usize,
    discussion: String,
}

#[tokio::test]
async fn random_walk_graphql() {
    walk("random_walk_graphql", 0x5851f42d4c957f2d, 48, false).await;
}

/// Adds a Reporter, so permission errors come up across all operations.
#[tokio::test]
async fn random_walk_graphql_2() {
    walk("random_walk_graphql_2", 0x14057b7ef767814f, 48, true).await;
}

async fn walk(name: &str, seed: u64, steps: usize, with_reporter: bool) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let gpath = s.name("rg");
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
    let mut users = vec![admin.clone(), bob_t.clone()];
    if with_reporter {
        let cname = s.name("carol");
        let carol = s
            .step("carol", POST, "/users", Some(&admin), json!({"username": cname, "name": "Carol", "email": format!("{cname}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let carol_t = s.step("carol token", POST, &format!("/users/{carol}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step("carol reporter", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": carol, "access_level": 20}), "user").await;
        users.push(carol_t);
    }
    let ms = s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": "v1"}), "milestone").await.id();
    let commits = format!("/projects/{pid}/repository/commits");
    let content = |n: usize| (1..=6).map(|i| format!("line {i} v{n}\n")).collect::<String>();
    s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": content(0)}]}), "commit").await;
    s.settle().await;
    let mr = format!("/projects/{pid}/merge_requests/1");
    let m = s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    let mr_gid = format!("gid://gitlab/MergeRequest/{}", m.id());
    s.poll_mr(pid, 1).await;

    let names = ["admin", "bob", "carol"];
    let mut rng = Rng(seed);
    let mut notes: Vec<NoteRef> = vec![];
    let mut pushes = 0;
    for step in 0..steps {
        let who = rng.pick(users.len());
        let tok = users[who].clone();
        let op = rng.pick(16);
        let label = format!("{step:02} {} op{op}", names[who]);
        s.poll_mr(pid, 1).await;
        let note_sel = "errors note { id body discussion { id } }";
        let mut created: Option<Value> = None;
        match op {
            0 => {
                let q = format!("mutation($id: NoteableID!, $b: String!) {{ createNote(input: {{noteableId: $id, body: $b}}) {{ {note_sel} }} }}");
                created = Some(s.graphql(&label, Some(&tok), &q, json!({"id": mr_gid, "b": format!("Note {step}")})).await.body);
            }
            1 if !notes.is_empty() => {
                let d = notes[rng.pick(notes.len())].discussion.clone();
                let q = format!("mutation($id: NoteableID!, $d: DiscussionID!, $b: String!) {{ createNote(input: {{noteableId: $id, discussionId: $d, body: $b}}) {{ {note_sel} }} }}");
                created = Some(s.graphql(&label, Some(&tok), &q, json!({"id": mr_gid, "d": d, "b": format!("Reply {step}")})).await.body);
            }
            2 => {
                let refs = s.send(GET, &mr, Some(&admin), json!(null)).await.body["diff_refs"].clone();
                let line = rng.pick(6) as i64 + 1;
                let q = format!("mutation($id: NoteableID!, $b: String!, $pos: DiffPositionInput!) {{ createDiffNote(input: {{noteableId: $id, body: $b, position: $pos}}) {{ {note_sel} }} }}");
                let pos = json!({"baseSha": refs["base_sha"], "startSha": refs["start_sha"], "headSha": refs["head_sha"], "paths": {"newPath": "a.txt", "oldPath": "a.txt"}, "newLine": line});
                created = Some(s.graphql(&label, Some(&tok), &q, json!({"id": mr_gid, "b": format!("Diff note {step}"), "pos": pos})).await.body);
            }
            3 if !notes.is_empty() => {
                let d = notes[rng.pick(notes.len())].discussion.clone();
                let q = "mutation($id: DiscussionID!, $r: Boolean!) { discussionToggleResolve(input: {id: $id, resolve: $r}) { errors discussion { resolved resolvedBy { username } } } }";
                s.graphql(&label, Some(&tok), q, json!({"id": d, "r": rng.pick(2) == 0})).await;
            }
            4 if notes.iter().any(|n| n.author == who) => {
                let mine: Vec<&NoteRef> = notes.iter().filter(|n| n.author == who).collect();
                let n = mine[rng.pick(mine.len())].gid.clone();
                let q = "mutation($id: NoteID!, $b: String!) { updateNote(input: {id: $id, body: $b}) { errors note { body } } }";
                s.graphql(&label, Some(&tok), q, json!({"id": n, "b": format!("Edited {step}")})).await;
            }
            5 if notes.iter().any(|n| n.author == who) => {
                let i = notes.iter().position(|n| n.author == who).unwrap();
                let n = notes.remove(i);
                let q = "mutation($id: NoteID!) { destroyNote(input: {id: $id}) { errors note { id } } }";
                s.graphql(&label, Some(&tok), q, json!({"id": n.gid})).await;
            }
            6 | 7 => {
                let target = if notes.is_empty() || rng.pick(2) == 0 { mr_gid.clone() } else { notes[rng.pick(notes.len())].gid.clone() };
                let name = ["thumbsup", "eyes"][rng.pick(2)];
                let m = if op == 6 { "awardEmojiAdd" } else { "awardEmojiRemove" };
                let q = format!("mutation($id: AwardableID!, $n: String!) {{ {m}(input: {{awardableId: $id, name: $n}}) {{ errors awardEmoji {{ name }} }} }}");
                s.graphql(&label, Some(&tok), &q, json!({"id": target, "n": name})).await;
            }
            8 => {
                let mid = if rng.pick(2) == 0 { Value::Null } else { json!(format!("gid://gitlab/Milestone/{ms}")) };
                let q = "mutation($p: ID!, $m: MilestoneID) { mergeRequestSetMilestone(input: {projectPath: $p, iid: \"1\", milestoneId: $m}) { errors mergeRequest { milestone { title } } } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full, "m": mid})).await;
            }
            9 => {
                let q = "mutation($p: ID!, $s: Boolean!) { mergeRequestSetSubscription(input: {projectPath: $p, iid: \"1\", subscribedState: $s}) { errors mergeRequest { subscribed } } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full, "s": rng.pick(2) == 0})).await;
            }
            10 => {
                let q = "mutation($p: ID!, $u: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $p, iid: \"1\", userId: $u}) { errors } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full, "u": format!("gid://gitlab/User/{bob}")})).await;
            }
            11 => {
                let q = "mutation($p: ID!) { mergeRequestRequestChanges(input: {projectPath: $p, iid: \"1\"}) { errors mergeRequest { approved } } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full})).await;
            }
            12 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&tok), json!(null), "approval").await;
            }
            13 if !notes.is_empty() => {
                let n = notes[rng.pick(notes.len())].gid.clone();
                let q = "mutation($id: NoteID!) { noteConvertToThread(input: {id: $id}) { errors note { discussion { resolvable } } } }";
                s.graphql(&label, Some(&tok), q, json!({"id": n})).await;
            }
            14 => {
                let q = "mutation($p: ID!, $l: Boolean!) { mergeRequestSetLocked(input: {projectPath: $p, iid: \"1\", locked: $l}) { errors mergeRequest { discussionLocked } } }";
                s.graphql(&label, Some(&tok), q, json!({"p": full, "l": rng.pick(3) == 0})).await;
            }
            _ => {
                pushes += 1;
                let c = s
                    .step(&label, POST, &commits, Some(&admin), json!({"branch": "feature", "commit_message": format!("Rewrite {pushes}"), "actions": [{"action": "update", "file_path": "a.txt", "content": content(pushes)}]}), "commit")
                    .await;
                let sha = c.get("id").clone();
                s.poll(&mr, move |b| b["sha"] == sha && b["merge_status"] == "can_be_merged").await;
            }
        }
        if let Some(body) = created {
            let key = ["createNote", "createDiffNote"].into_iter().find(|k| !body["data"][k].is_null());
            if let Some(k) = key {
                let n = &body["data"][k]["note"];
                if let (Some(id), Some(d)) = (n["id"].as_str(), n["discussion"]["id"].as_str()) {
                    notes.push(NoteRef { gid: id.to_string(), author: who, discussion: d.to_string() });
                }
            }
        }
        s.settle().await;
        if step % 8 == 7 {
            s.poll_mr(pid, 1).await;
            s.graphql(&format!("state {step} admin"), Some(&admin), MR_QUERY, json!({"p": full})).await;
            s.graphql(&format!("state {step} bob"), Some(&bob_t), MR_QUERY, json!({"p": full})).await;
            if users.len() > 2 {
                let carol_t = users[2].clone();
                s.graphql(&format!("state {step} carol"), Some(&carol_t), MR_QUERY, json!({"p": full})).await;
            }
        }
    }
    s.step("final notes", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.step("bob todos", GET, "/todos", Some(&bob_t), json!(null), "todo").await;
    s.finish();
}
