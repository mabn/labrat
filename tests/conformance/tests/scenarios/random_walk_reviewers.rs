//! A fixed-seed random walk over review cycles with three reviewers: review requests and
//! re-requests, approvals and unapprovals, requested changes, submitted reviews (REST quick
//! actions and GraphQL mutations), reviewer removal, and pushes by the author. Reviewer states,
//! approvals, merge status, user counts and to-dos are compared at checkpoints.

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
async fn random_walk_reviewers() {
    let mut s = Scenario::new("random_walk_reviewers").await;
    let admin = s.admin();
    let gpath = s.name("rv");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    // ann is the author; bob, cat and dan review.
    let mut people: Vec<(String, i64, String)> = vec![];
    for who in ["ann", "bob", "cat", "dan"] {
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
    let ann = people[0].2.clone();
    s.step("commit", POST, &commits, Some(&ann), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let mr = format!("{mrs}/1");
    let reviewer_ids: Vec<i64> = people[1..].iter().map(|p| p.1).collect();
    s.step("mr", POST, &mrs, Some(&ann), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": reviewer_ids}), "mr").await;
    s.poll_mr(pid, 1).await;

    let mut rng = Rng(0x3c6ef372fe94f82b);
    let mut pushes = 0;
    for step in 0..42 {
        let r = 1 + rng.pick(3);
        let (rname, rid, rtok) = people[r].clone();
        let short = rname.split('-').next().unwrap_or("").to_string();
        let op = rng.pick(11);
        let label = format!("{step:02} {short} op{op}");
        match op {
            0 => {
                s.step(&label, POST, &format!("{mr}/approve"), Some(&rtok), json!(null), "approval").await;
            }
            1 => {
                s.step(&label, POST, &format!("{mr}/unapprove"), Some(&rtok), json!(null), "approval").await;
            }
            2 => {
                s.graphql(
                    &label,
                    Some(&rtok),
                    "mutation($p: ID!) { mergeRequestRequestChanges(input: {projectPath: $p, iid: \"1\"}) { errors mergeRequest { detailedMergeStatus } } }",
                    json!({"p": full}),
                )
                .await;
            }
            3 => {
                let kind = ["", " approve", " requested_changes", " reviewed"][rng.pick(4)];
                s.step(&label, POST, &format!("{mr}/notes"), Some(&rtok), json!({"body": format!("Review {step}\n/submit_review{kind}")}), "note").await;
            }
            4 => {
                s.step(&label, POST, &format!("{mr}/draft_notes"), Some(&rtok), json!({"note": format!("Draft {step}")}), "draft").await;
                let note = ["", "/submit_review approve", "/submit_review requested_changes"][rng.pick(3)];
                if !note.is_empty() {
                    s.step(&format!("{label} quick"), POST, &format!("{mr}/draft_notes"), Some(&rtok), json!({"note": note}), "draft").await;
                }
                s.step(&format!("{label} publish"), POST, &format!("{mr}/draft_notes/bulk_publish"), Some(&rtok), json!(null), "draft").await;
            }
            // The author re-requests a review (GraphQL) or via the quick action.
            5 => {
                if rng.pick(2) == 0 {
                    s.graphql(
                        &label,
                        Some(&ann),
                        "mutation($p: ID!, $u: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $p, iid: \"1\", userId: $u}) { errors } }",
                        json!({"p": full, "u": format!("gid://gitlab/User/{rid}")}),
                    )
                    .await;
                } else {
                    s.step(&label, POST, &format!("{mr}/notes"), Some(&ann), json!({"body": format!("/request_review @{rname}")}), "note").await;
                }
            }
            // The reviewer set changes.
            6 => {
                let mut ids: Vec<i64> = people[1..].iter().map(|p| p.1).filter(|_| rng.pick(3) != 0).collect();
                if ids.is_empty() {
                    ids.push(rid);
                }
                s.step(&label, PUT, &mr, Some(&ann), json!({"reviewer_ids": ids}), "mr").await;
            }
            7 => {
                s.step(&label, POST, &format!("{mr}/notes"), Some(&ann), json!({"body": format!("/remove_reviewer @{rname}")}), "note").await;
            }
            // The author pushes.
            8 => {
                pushes += 1;
                s.step(&label, POST, &commits, Some(&ann), json!({"branch": "feature", "commit_message": format!("Push {pushes}"), "actions": [{"action": "update", "file_path": "a.txt", "content": format!("a {pushes}\n")}]}), "commit").await;
                s.settle().await;
                s.poll_mr(pid, 1).await;
            }
            9 => {
                let mode = ["APPEND", "REMOVE", "REPLACE"][rng.pick(3)];
                s.graphql(
                    &label,
                    Some(&ann),
                    "mutation($p: ID!, $u: [String!]!, $m: MutationOperationMode) { mergeRequestSetReviewers(input: {projectPath: $p, iid: \"1\", reviewerUsernames: $u, operationMode: $m}) { errors mergeRequest { reviewers { nodes { username } } } } }",
                    json!({"p": full, "u": [rname], "m": mode}),
                )
                .await;
            }
            _ => {
                s.step(&label, POST, &format!("{mr}/notes"), Some(&rtok), json!({"body": format!("Comment {step}")}), "note").await;
            }
        }
        s.settle().await;
        if step % 7 == 6 {
            s.step(&format!("cp{step} reviewers"), GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "user").await;
            s.step(&format!("cp{step} approvals"), GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
            s.step(&format!("cp{step} mr"), GET, &mr, Some(&admin), json!(null), "mr").await;
            s.graphql(
                &format!("cp{step} graphql"),
                Some(&admin),
                "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { detailedMergeStatus approved approvedBy { nodes { username } }
                   reviewers { nodes { username mergeRequestInteraction { reviewState approved reviewed } } } } } }",
                json!({"p": full}),
            )
            .await;
            for (name, _, tok) in people.clone() {
                let short = name.split('-').next().unwrap_or("").to_string();
                s.step(&format!("cp{step} {short} counts"), GET, "/user_counts", Some(&tok), json!(null), "counts").await;
                let t = s.send(GET, "/todos", Some(&tok), json!({"per_page": 100})).await;
                let compact: Vec<Value> = t.body.as_array().into_iter().flatten().map(|x| json!({"action": x["action_name"], "author": x["author"]["username"]})).collect();
                let res = labrat_conformance::Res { status: t.status, headers: Default::default(), body: json!(compact) };
                s.record(&format!("cp{step} {short} todos"), &GET, "/todos", &res, "list");
            }
        }
    }
    s.finish();
}
