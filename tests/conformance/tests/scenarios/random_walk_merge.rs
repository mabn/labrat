//! A fixed-seed random walk over merging: project merge settings change under the MR while
//! statuses, threads, conflicting pushes, rebases, merges and auto-merges happen. The MR is
//! read after every step, so mergeability and `detailed_merge_status` are compared throughout.

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
async fn random_walk_merge() {
    walk("random_walk_merge", 0xc4ceb9fe1a85ec53, 50).await;
}

#[tokio::test]
async fn random_walk_merge_2() {
    walk("random_walk_merge_2", 0x2127599bf4325c37, 70).await;
}

async fn walk(name: &str, seed: u64, steps: usize) {
    let mut s = Scenario::new(name).await;
    let admin = s.admin();
    let gpath = s.name("rm");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("shared file", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Shared", "actions": [{"action": "create", "file_path": "shared.txt", "content": "a\nb\nc\n"}]}), "commit").await;
    s.settle().await;

    let mut rng = Rng(seed);
    let mut iid = 0i64;
    let mut branch = String::new();
    let mut open = false;
    let mut thread: Option<String> = None;
    let mut n = 0;
    for step in 0..steps {
        if !open {
            iid += 1;
            branch = format!("f{iid}");
            s.step(&format!("{step:02} new branch"), POST, &commits, Some(&admin), json!({"branch": branch, "start_branch": "main", "commit_message": format!("Start {branch}"), "actions": [{"action": "create", "file_path": format!("{branch}.txt"), "content": "x\n"}]}), "commit").await;
            s.settle().await;
            s.step(&format!("{step:02} new mr"), POST, &mrs, Some(&admin), json!({"source_branch": branch, "target_branch": "main", "title": format!("MR {iid}")}), "mr").await;
            s.poll_mr(pid, iid).await;
            open = true;
            thread = None;
        }
        let mr = format!("{mrs}/{iid}");
        let op = rng.pick(12);
        let label = format!("{step:02} mr{iid} op{op}");
        s.poll_mr(pid, iid).await;
        match op {
            0 | 1 => {
                // Pushes to the source or the target; touching `shared.txt` on both sides conflicts.
                n += 1;
                let (target_branch, file) = if op == 0 { (branch.clone(), "shared.txt") } else { ("main".to_string(), if rng.pick(2) == 0 { "shared.txt" } else { "main-only.txt" }) };
                let content = format!("{}\nb\nc\n", if op == 0 { format!("source {n}") } else { format!("target {n}") });
                let exists = s.send(GET, &format!("/projects/{pid}/repository/files/{}", file.replace('.', "%2E")), Some(&admin), json!({"ref": target_branch})).await.status == 200;
                let action = if exists { "update" } else { "create" };
                let c = s
                    .step(&label, POST, &commits, Some(&admin), json!({"branch": target_branch, "commit_message": format!("Push {n}"), "actions": [{"action": action, "file_path": file, "content": content}]}), "commit")
                    .await;
                if op == 0 {
                    let sha = c.get("id").clone();
                    let path = mr.clone();
                    s.poll(&path, move |b| b["sha"] == sha && b["merge_status"] != "checking" && b["merge_status"] != "unchecked").await;
                }
            }
            2 => {
                let sha = s.send(GET, &mr, Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
                let state = ["success", "failed", "running", "pending", "canceled"][rng.pick(5)];
                s.step(&label, POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": state, "name": "ci", "ref": branch}), "status").await;
            }
            3 => {
                let d = s.step(&label, POST, &format!("{mr}/discussions"), Some(&admin), json!({"body": format!("Thread {step}")}), "discussion").await;
                thread = d.body["id"].as_str().map(str::to_string).or(thread);
            }
            4 => match &thread {
                Some(t) => {
                    let t = t.clone();
                    s.step(&label, PUT, &format!("{mr}/discussions/{t}"), Some(&admin), json!({"resolved": rng.pick(2) == 0}), "discussion").await;
                }
                None => {
                    s.step(&label, GET, &format!("{mr}/discussions"), Some(&admin), json!(null), "discussion").await;
                }
            },
            5 | 6 => {
                let body = if op == 6 { json!({"merge_when_pipeline_succeeds": true}) } else { json!({"should_remove_source_branch": rng.pick(2) == 0}) };
                let r = s.step(&label, PUT, &format!("{mr}/merge"), Some(&admin), body, "mr").await;
                if r.body["state"] == "merged" {
                    open = false;
                }
            }
            7 => {
                s.step(&label, POST, &format!("{mr}/cancel_merge_when_pipeline_succeeds"), Some(&admin), json!(null), "mr").await;
            }
            8 => {
                s.step(&label, PUT, &format!("{mr}/rebase"), Some(&admin), json!(null), "rebase").await;
                s.settle().await;
                let path = mr.clone();
                s.poll(&path, |b| b["merge_status"] != "checking" && b["merge_status"] != "unchecked").await;
            }
            9 | 10 => {
                let settings: [(&str, Value); 8] = [
                    ("only_allow_merge_if_pipeline_succeeds", json!(rng.pick(2) == 0)),
                    ("only_allow_merge_if_all_discussions_are_resolved", json!(rng.pick(2) == 0)),
                    ("merge_method", json!(["merge", "rebase_merge", "ff"][rng.pick(3)])),
                    ("squash_option", json!(["never", "always", "default_on", "default_off"][rng.pick(4)])),
                    ("allow_merge_on_skipped_pipeline", json!(rng.pick(2) == 0)),
                    ("remove_source_branch_after_merge", json!(rng.pick(2) == 0)),
                    ("merge_pipelines_enabled", json!(false)),
                    ("resolve_outdated_diff_discussions", json!(rng.pick(2) == 0)),
                ];
                let (k, v) = settings[rng.pick(settings.len())].clone();
                s.step(&label, PUT, &format!("/projects/{pid}"), Some(&admin), json!({k: v}), "project").await;
            }
            _ => {
                let title = if rng.pick(2) == 0 { format!("Draft: MR {iid}") } else { format!("MR {iid}") };
                s.step(&label, PUT, &mr, Some(&admin), json!({"title": title}), "mr").await;
            }
        }
        s.settle().await;
        s.poll_mr(pid, iid).await;
        let st = s.step(&format!("{label} state"), GET, &mr, Some(&admin), json!(null), "mr").await;
        if st.body["state"] == "merged" || st.body["state"] == "closed" {
            open = false;
        }
    }
    s.step("final mrs", GET, &mrs, Some(&admin), json!({"per_page": 100}), "mr").await;
    s.finish();
}
