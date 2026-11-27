//! Auto-merge and merge train behaviour recorded on the EE reference (the `merge_train_gaps`,
//! `auto_merge_approval` and `auto_merge_triggers` scenarios), checked end to end on labrat in EE
//! mode:
//! - an MR pipeline that passes completes the MR's auto-merge;
//! - an approval completes an auto-merge that waited for it;
//! - in fast-forward and rebase-merge projects with trains, an MR behind its target is mergeable
//!   and the train rebases it;
//! - without a pipeline, no train strategy is offered;
//! - `mergeRequestAccept` onto a train keeps branch removal and the commit message.

use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;

const TOKEN: &str = "glpat-labrat-auto-merge-gaps-0000";

fn start() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let config = labrat_core::Config {
                external_url: format!("http://{addr}"),
                ssh_host: "localhost".into(),
                ssh_port: 22,
                root_password: "labrat-root-password".into(),
                root_token: Some(TOKEN.into()),
                edition: labrat_core::Edition::Ee,
            };
            let app = Arc::new(labrat_core::App { db: labrat_core::Db::open(":memory:").unwrap(), config });
            labrat_api::bootstrap(&app).unwrap();
            tokio::spawn(async move { axum::serve(listener, labrat_api::router(app)).await.unwrap() });
            tx.send(addr).unwrap();
            std::future::pending::<()>().await;
        });
    });
    rx.recv().unwrap()
}

struct Api {
    addr: String,
}

impl Api {
    fn call_as(&self, token: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
        let url = if path.starts_with("/-/") || path == "/api/graphql" { format!("http://{}{path}", self.addr) } else { format!("http://{}/api/v4{path}", self.addr) };
        let mut cmd = Command::new("curl");
        cmd.args(["-s", "-w", "\n%{http_code}", "-X", method, "-H", &format!("PRIVATE-TOKEN: {token}"), "-H", "Content-Type: application/json"]);
        if !body.is_null() {
            cmd.args(["--data-binary", &body.to_string()]);
        }
        let out = cmd.arg(&url).output().unwrap();
        let out = String::from_utf8(out.stdout).unwrap();
        let (body, code) = out.rsplit_once('\n').unwrap();
        (code.trim().parse().unwrap(), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        let (status, v) = self.call_as(TOKEN, method, path, body);
        assert!(status < 300, "{method} {path}: {status} {v}");
        v
    }

    fn idle(&self) {
        self.call("GET", "/-/labrat/idle", Value::Null);
    }

    fn graphql(&self, query: &str, variables: Value) -> Value {
        self.call("POST", "/api/graphql", json!({"query": query, "variables": variables}))
    }

    /// A project; with fake CI (manual runner) when `ci`.
    fn project(&self, path: &str, ci: bool, settings: Value) -> i64 {
        let p = self.call("POST", "/projects", json!({"name": path, "path": path, "initialize_with_readme": true}));
        let id = p["id"].as_i64().unwrap();
        if ci {
            self.call("PUT", &format!("/-/labrat/projects/{id}/ci"), json!({"enabled": true, "jobs": ["test"], "runner": {"mode": "manual"}}));
        }
        if !settings.is_null() {
            self.call("PUT", &format!("/projects/{id}"), settings);
        }
        self.idle();
        id
    }

    fn commit(&self, pid: i64, branch: &str, file: &str) -> Value {
        let start = if branch == "main" { json!(null) } else { json!("main") };
        let r = self.call(
            "POST",
            &format!("/projects/{pid}/repository/commits"),
            json!({"branch": branch, "start_branch": start, "commit_message": format!("Add {file}"), "actions": [{"action": "create", "file_path": file, "content": format!("{file}\n")}]}),
        );
        self.idle();
        r
    }

    fn mr(&self, pid: i64, branch: &str) -> i64 {
        self.commit(pid, branch, &format!("{branch}.txt"));
        let m = self.call("POST", &format!("/projects/{pid}/merge_requests"), json!({"source_branch": branch, "target_branch": "main", "title": format!("Add {branch}")}));
        self.idle();
        self.show(pid, m["iid"].as_i64().unwrap());
        m["iid"].as_i64().unwrap()
    }

    fn show(&self, pid: i64, iid: i64) -> Value {
        // A read starts the mergeability check; read again once it ran.
        self.call("GET", &format!("/projects/{pid}/merge_requests/{iid}"), Value::Null);
        self.idle();
        self.call("GET", &format!("/projects/{pid}/merge_requests/{iid}"), Value::Null)
    }

    fn ci(&self, action: &str, pid: i64, r#ref: Option<&str>, status: &str) {
        let mut body = json!({"project_id": pid});
        if let Some(r) = r#ref {
            body["ref"] = json!(r);
        }
        if action == "finish" {
            body["status"] = json!(status);
        }
        self.call("POST", &format!("/-/labrat/ci/{action}"), body);
        self.idle();
    }
}

const MR_FIELDS: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  state detailedMergeStatus shouldBeRebased availableAutoMergeStrategies } } }";
const ACCEPT: &str = "mutation($path: ID!, $iid: String!, $sha: String!, $strategy: MergeStrategyEnum, $remove: Boolean, $msg: String) {
  mergeRequestAccept(input: {projectPath: $path, iid: $iid, sha: $sha, strategy: $strategy, shouldRemoveSourceBranch: $remove, commitMessage: $msg}) {
    errors mergeRequest { state autoMergeEnabled autoMergeStrategy } } }";

#[test]
fn mr_pipeline_completes_auto_merge() {
    let api = Api { addr: start() };
    let pid = api.project("pipe", true, Value::Null);
    let iid = api.mr(pid, "feature");
    api.ci("pick", pid, None, "");
    let mr = api.call("PUT", &format!("/projects/{pid}/merge_requests/{iid}/merge"), json!({"auto_merge": true}));
    assert_eq!(mr["merge_when_pipeline_succeeds"], true, "{mr}");
    assert_eq!(mr["head_pipeline"]["ref"], "refs/merge-requests/1/head", "{mr}");
    api.ci("finish", pid, Some("refs/merge-requests/1/head"), "success");
    assert_eq!(api.show(pid, iid)["state"], "merged");
}

#[test]
fn approval_completes_auto_merge() {
    let api = Api { addr: start() };
    let pid = api.project("appr", false, Value::Null);
    let u = api.call("POST", "/users", json!({"username": "approver", "name": "Approver", "email": "approver@example.com", "password": "Password-123-labrat", "skip_confirmation": true}));
    let uid = u["id"].as_i64().unwrap();
    let t = api.call("POST", &format!("/users/{uid}/personal_access_tokens"), json!({"name": "t", "scopes": ["api"]}));
    let ut = t["token"].as_str().unwrap().to_string();
    api.call("POST", &format!("/projects/{pid}/members"), json!({"user_id": uid, "access_level": 30}));
    api.call("POST", &format!("/projects/{pid}/approval_rules"), json!({"name": "Review", "approvals_required": 1, "user_ids": [uid]}));
    let iid = api.mr(pid, "feature");
    assert_eq!(api.show(pid, iid)["detailed_merge_status"], "not_approved");
    let mr = api.call("PUT", &format!("/projects/{pid}/merge_requests/{iid}/merge"), json!({"auto_merge": true}));
    assert_eq!(mr["merge_when_pipeline_succeeds"], true, "{mr}");
    let (status, _) = api.call_as(&ut, "POST", &format!("/projects/{pid}/merge_requests/{iid}/approve"), Value::Null);
    assert_eq!(status, 201);
    api.idle();
    assert_eq!(api.show(pid, iid)["state"], "merged");
}

/// A train project (with CI) whose MRs 1 and 2 have passed their pipelines and whose main has
/// moved on since; `merge_method` set before main moves.
fn behind_project(api: &Api, path: &str, method: &str) -> i64 {
    let pid = api.project(path, true, json!({"merge_pipelines_enabled": true, "merge_trains_enabled": true}));
    for b in ["one", "two"] {
        api.mr(pid, b);
    }
    api.ci("pick", pid, None, "");
    for i in 1..=2 {
        api.ci("finish", pid, Some(&format!("refs/merge-requests/{i}/merge")), "success");
    }
    api.call("PUT", &format!("/projects/{pid}"), json!({"merge_method": method}));
    api.commit(pid, "main", "upstream.txt");
    pid
}

#[test]
fn trains_rebase_mrs_behind_target() {
    let api = Api { addr: start() };
    for method in ["ff", "rebase_merge"] {
        let path = format!("behind-{}", method.replace('_', "-"));
        let pid = behind_project(&api, &path, method);
        let full = format!("root/{path}");
        let mr = api.show(pid, 1);
        assert_eq!(mr["detailed_merge_status"], "mergeable", "{method}: {mr}");
        let g = api.graphql(MR_FIELDS, json!({"path": full, "iid": "1"}));
        let m = &g["data"]["project"]["mergeRequest"];
        // As on the EE reference: still "should be rebased", but the train takes it.
        assert_eq!(m["shouldBeRebased"], true, "{method}: {g}");
        assert_eq!(m["availableAutoMergeStrategies"], json!(["merge_train", "add_to_merge_train_when_checks_pass"]), "{method}: {g}");

        // REST adds MR 1, GraphQL MR 2; both get train pipelines on rebased refs.
        let (status, body) = api.call_as(TOKEN, "POST", &format!("/projects/{pid}/merge_trains/merge_requests/1"), json!({}));
        assert_eq!(status, 201, "{method}: {body}");
        let sha2 = api.show(pid, 2)["sha"].as_str().unwrap().to_string();
        let a = api.graphql(ACCEPT, json!({"path": full, "iid": "2", "sha": sha2, "strategy": "MERGE_TRAIN"}));
        assert_eq!(a["data"]["mergeRequestAccept"]["errors"], json!([]), "{method}: {a}");
        api.idle();
        api.ci("pick", pid, None, "");
        api.ci("finish", pid, Some("refs/merge-requests/1/train"), "success");
        api.ci("finish", pid, Some("refs/merge-requests/2/train"), "success");
        for iid in 1..=2 {
            assert_eq!(api.show(pid, iid)["state"], "merged", "{method}: mr {iid}");
        }
        // main has upstream.txt, one.txt and two.txt; fast-forward leaves no merge commits.
        let tree = api.call("GET", &format!("/projects/{pid}/repository/tree"), Value::Null);
        let names: Vec<&str> = tree.as_array().unwrap().iter().filter_map(|e| e["name"].as_str()).collect();
        for f in ["upstream.txt", "one.txt", "two.txt"] {
            assert!(names.contains(&f), "{method}: {names:?}");
        }
        let commits = api.call("GET", &format!("/projects/{pid}/repository/commits"), json!(null));
        let merges = commits.as_array().unwrap().iter().filter(|c| c["parent_ids"].as_array().unwrap().len() > 1).count();
        assert_eq!(merges, if method == "ff" { 0 } else { 2 }, "{method}: {commits}");
    }
}

#[test]
fn no_train_strategy_without_pipeline() {
    let api = Api { addr: start() };
    let pid = api.project("noci", false, json!({"merge_pipelines_enabled": true, "merge_trains_enabled": true}));
    let iid = api.mr(pid, "solo");
    let g = api.graphql(MR_FIELDS, json!({"path": "root/noci", "iid": iid.to_string()}));
    assert_eq!(g["data"]["project"]["mergeRequest"]["availableAutoMergeStrategies"], json!([]), "{g}");
    // It merges directly.
    let mr = api.call("PUT", &format!("/projects/{pid}/merge_requests/{iid}/merge"), Value::Null);
    assert_eq!(mr["state"], "merged", "{mr}");
}

#[test]
fn train_accept_keeps_merge_options() {
    let api = Api { addr: start() };
    let pid = api.project("opts", true, json!({"merge_pipelines_enabled": true, "merge_trains_enabled": true}));
    let iid = api.mr(pid, "feature");
    api.call("PUT", &format!("/projects/{pid}/merge_requests/{iid}"), json!({"remove_source_branch": false}));
    api.ci("pick", pid, None, "");
    api.ci("finish", pid, Some("refs/merge-requests/1/merge"), "success");
    let sha = api.show(pid, iid)["sha"].as_str().unwrap().to_string();
    let a = api.graphql(ACCEPT, json!({"path": "root/opts", "iid": "1", "sha": sha, "strategy": "MERGE_TRAIN", "remove": true, "msg": "Train merge of feature"}));
    assert_eq!(a["data"]["mergeRequestAccept"]["errors"], json!([]), "{a}");
    api.idle();
    api.ci("pick", pid, None, "");
    api.ci("finish", pid, Some("refs/merge-requests/1/train"), "success");
    let mr = api.show(pid, iid);
    assert_eq!(mr["state"], "merged", "{mr}");
    let commit = api.call("GET", &format!("/projects/{pid}/repository/commits/{}", mr["merge_commit_sha"].as_str().unwrap()), Value::Null);
    assert_eq!(commit["message"], "Train merge of feature", "{commit}");
    let (status, _) = api.call_as(TOKEN, "GET", &format!("/projects/{pid}/repository/branches/feature"), Value::Null);
    assert_eq!(status, 404, "the source branch is removed");
}
