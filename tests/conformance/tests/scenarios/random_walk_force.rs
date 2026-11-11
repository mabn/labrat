//! A fixed-seed random walk over review threads while the source branch is rewritten with git:
//! plain pushes, amended commits force-pushed, and rebases onto an updated target branch.
//! Diff threads, replies, resolutions, suggestions and draft reviews are interleaved, and the
//! discussions (with their positions) and diff versions are compared at checkpoints.

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

/// Lines of a unified diff a comment can target: `(old_line, new_line)`.
fn diff_lines(diff: &str) -> Vec<(Option<i64>, Option<i64>)> {
    let mut out = vec![];
    let (mut o, mut n) = (0i64, 0i64);
    for l in diff.lines() {
        if let Some(h) = l.strip_prefix("@@ -") {
            let mut parts = h.split(' ');
            o = parts.next().and_then(|x| x.split(',').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
            n = parts.next().and_then(|x| x.trim_start_matches('+').split(',').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
            continue;
        }
        match l.chars().next() {
            Some('+') => {
                out.push((None, Some(n)));
                n += 1;
            }
            Some('-') => {
                out.push((Some(o), None));
                o += 1;
            }
            Some(' ') => {
                out.push((Some(o), Some(n)));
                o += 1;
                n += 1;
            }
            _ => {}
        }
    }
    out
}

#[tokio::test]
async fn random_walk_force() {
    walk("random_walk_force", 0x6a09e667f3bcc909).await;
}

#[tokio::test]
async fn random_walk_force_2() {
    walk("random_walk_force_2", 0x3c6ef372fe94f82c).await;
}

async fn walk(name: &str, seed: u64) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let gpath = s.name("rf");
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

    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("rf"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    let write = |rel: &str, lines: &[String]| {
        std::fs::write(repo.join(rel), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    };
    let mut a: Vec<String> = (1..=30).map(|i| format!("alpha {i}")).collect();
    let mut b: Vec<String> = (1..=12).map(|i| format!("beta {i}")).collect();
    write("a.txt", &a);
    write("b.txt", &b);
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Base"]).await;
    s.git(&repo, &["push", "-q", "origin", "HEAD:main"]).await;
    s.settle().await;
    s.git(&repo, &["checkout", "-q", "-b", "feature"]).await;
    a[4] = "alpha 5 changed".into();
    a[17] = "alpha 18 changed".into();
    a.insert(25, "alpha inserted".into());
    write("a.txt", &a);
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Feature"]).await;
    s.git(&repo, &["push", "-q", "origin", "feature"]).await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let mr = format!("{mrs}/1");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Feature", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;

    let users = [("admin", admin.clone()), ("bob", bob_t.clone())];
    let mut rng = Rng(seed);
    let mut edits = 0;
    let mut main_edits = 0;
    for step in 0..36 {
        let (who, tok) = users[rng.pick(2)].clone();
        let op = rng.pick(10);
        let label = format!("{step:02} {who} op{op}");
        match op {
            // A diff thread, a suggestion or a draft on a line of the current diff.
            0 | 1 | 2 => {
                let m = s.send(GET, &mr, Some(&admin), json!(null)).await;
                let refs = m.body["diff_refs"].clone();
                let diffs = s.send(GET, &format!("{mr}/diffs"), Some(&admin), json!(null)).await;
                let Some(file) = diffs.body.as_array().and_then(|d| d.iter().find(|f| f["new_path"] == "a.txt")).cloned() else {
                    s.step(&label, GET, &format!("{mr}/diffs"), Some(&admin), json!(null), "diff").await;
                    continue;
                };
                let lines = diff_lines(file["diff"].as_str().unwrap_or(""));
                if lines.is_empty() {
                    continue;
                }
                let (old, new) = lines[rng.pick(lines.len())];
                let position = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"],
                    "position_type": "text", "old_path": "a.txt", "new_path": "a.txt", "old_line": old, "new_line": new});
                match op {
                    0 => {
                        s.step(&label, POST, &format!("{mr}/discussions"), Some(&tok), json!({"body": format!("Thread {step}"), "position": position}), "discussion").await;
                    }
                    1 if new.is_some() => {
                        s.step(&label, POST, &format!("{mr}/discussions"), Some(&tok), json!({"body": format!("Suggest\n```suggestion:-0+0\nsuggested {step}\n```"), "position": position}), "discussion").await;
                    }
                    _ => {
                        s.step(&label, POST, &format!("{mr}/draft_notes"), Some(&tok), json!({"note": format!("Draft {step}"), "position": position}), "draft").await;
                        if rng.pick(2) == 0 {
                            s.step(&format!("{label} publish"), POST, &format!("{mr}/draft_notes/bulk_publish"), Some(&tok), json!(null), "draft").await;
                        }
                    }
                }
            }
            // Reply to, resolve or unresolve a diff thread.
            3 => {
                let d = s.send(GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100})).await;
                let threads: Vec<Value> = d.body.as_array().into_iter().flatten().filter(|x| x["notes"][0]["type"] == "DiffNote").cloned().collect();
                if threads.is_empty() {
                    s.step(&label, GET, &format!("{mr}/discussions"), Some(&admin), json!(null), "discussion").await;
                    continue;
                }
                let t = &threads[rng.pick(threads.len())];
                let id = t["id"].as_str().unwrap().to_string();
                match rng.pick(3) {
                    0 => {
                        s.step(&label, POST, &format!("{mr}/discussions/{id}/notes"), Some(&tok), json!({"body": format!("Reply {step}")}), "note").await;
                    }
                    k => {
                        s.step(&label, PUT, &format!("{mr}/discussions/{id}"), Some(&tok), json!({"resolved": k == 1}), "discussion").await;
                    }
                }
            }
            // Apply a suggestion (a commit on the source branch, which the clone then pulls).
            4 => {
                let d = s.send(GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100})).await;
                let open: Vec<Value> = d
                    .body
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|x| x["notes"].as_array().cloned().unwrap_or_default())
                    .flat_map(|n| n["suggestions"].as_array().cloned().unwrap_or_default())
                    .filter(|sg| sg["applicable"] == true)
                    .collect();
                if open.is_empty() {
                    s.step(&label, GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
                } else {
                    let sg = &open[rng.pick(open.len())];
                    s.step(&label, PUT, &format!("/suggestions/{}/apply", sg["id"]), Some(&tok), json!(null), "suggestion").await;
                    s.settle().await;
                    s.poll_mr(pid, 1).await;
                }
            }
            // A new commit pushed normally.
            5 | 6 => {
                edits += 1;
                s.git(&repo, &["pull", "-q", "--rebase", "origin", "feature"]).await;
                a = std::fs::read_to_string(repo.join("a.txt")).unwrap().lines().map(str::to_string).collect();
                let i = rng.pick(a.len());
                a[i] = format!("{} edit {edits}", a[i]);
                write("a.txt", &a);
                s.git(&repo, &["commit", "-q", "-am", &format!("Edit {edits}")]).await;
                s.git(&repo, &["push", "-q", "origin", "feature"]).await;
                s.settle().await;
                s.poll_mr(pid, 1).await;
                s.step(&label, GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
            }
            // Amend the last commit and force-push.
            7 | 8 => {
                edits += 1;
                s.git(&repo, &["pull", "-q", "--rebase", "origin", "feature"]).await;
                a = std::fs::read_to_string(repo.join("a.txt")).unwrap().lines().map(str::to_string).collect();
                let i = rng.pick(a.len());
                if rng.pick(3) == 0 {
                    a.remove(i);
                } else {
                    a.insert(i, format!("amended {edits}"));
                }
                write("a.txt", &a);
                s.git(&repo, &["commit", "-q", "-a", "--amend", "-m", &format!("Amended {edits}")]).await;
                s.git(&repo, &["push", "-q", "-f", "origin", "feature"]).await;
                s.settle().await;
                s.poll_mr(pid, 1).await;
                s.step(&label, GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
            }
            // The target branch moves; the feature branch is rebased onto it and force-pushed.
            _ => {
                main_edits += 1;
                s.git(&repo, &["pull", "-q", "--rebase", "origin", "feature"]).await;
                s.git(&repo, &["checkout", "-q", "main"]).await;
                s.git(&repo, &["pull", "-q", "--rebase", "origin", "main"]).await;
                b = std::fs::read_to_string(repo.join("b.txt")).unwrap().lines().map(str::to_string).collect();
                let i = rng.pick(b.len());
                b[i] = format!("{} main {main_edits}", b[i]);
                write("b.txt", &b);
                s.git(&repo, &["commit", "-q", "-am", &format!("Main {main_edits}")]).await;
                s.git(&repo, &["push", "-q", "origin", "main"]).await;
                s.git(&repo, &["checkout", "-q", "feature"]).await;
                s.git(&repo, &["rebase", "-q", "main"]).await;
                s.git(&repo, &["push", "-q", "-f", "origin", "feature"]).await;
                s.settle().await;
                s.poll_mr(pid, 1).await;
                s.step(&label, GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
            }
        }
        s.settle().await;
        if step % 9 == 8 || step == 35 {
            s.step(&format!("cp{step} discussions"), GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100}), "discussion").await;
            s.step(&format!("cp{step} mr"), GET, &mr, Some(&admin), json!(null), "mr").await;
            s.graphql(
                &format!("cp{step} graphql"),
                Some(&admin),
                "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { resolvableDiscussionsCount resolvedDiscussionsCount
                   discussions { nodes { resolved notes { nodes { body position { diffRefs { headSha } newLine oldLine } } } } } } } }",
                json!({"p": full}),
            )
            .await;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    s.finish();
}
