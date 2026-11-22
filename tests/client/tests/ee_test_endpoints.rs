//! labrat's EE test endpoints for code owners and approval policies, against an in-process EE
//! labrat: what they accept and what merge requests get from them. (The MR-facing behaviour is
//! compared with the EE reference in the conformance scenarios `code_owners` and
//! `approval_policies`.)

use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;

const TOKEN: &str = "glpat-labrat-ee-endpoint-tests-00";

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

/// A request to labrat (`path` from the host root); returns (status, JSON body).
fn call(addr: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut cmd = Command::new("curl");
    cmd.args(["-s", "-w", "\n%{http_code}", "-X", method, "-H", &format!("PRIVATE-TOKEN: {TOKEN}"), "-H", "Content-Type: application/json"]);
    if let Some(b) = body {
        cmd.args(["-d", &b.to_string()]);
    }
    cmd.arg(format!("http://{addr}{path}"));
    let out = String::from_utf8(cmd.output().unwrap().stdout).unwrap();
    let (body, code) = out.rsplit_once('\n').unwrap();
    (code.trim().parse().unwrap(), serde_json::from_str(body).unwrap_or(Value::Null))
}

fn api(addr: &str, method: &str, path: &str, body: Option<Value>) -> Value {
    let (status, v) = call(addr, method, &format!("/api/v4{path}"), body);
    assert!(status < 300, "{method} {path}: {status} {v}");
    v
}

fn idle(addr: &str) {
    call(addr, "GET", "/-/labrat/idle", None);
}

fn rules(addr: &str, pid: i64, iid: i64, rule_type: &str) -> Vec<Value> {
    api(addr, "GET", &format!("/projects/{pid}/merge_requests/{iid}/approval_state"), None)["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["rule_type"] == rule_type)
        .cloned()
        .collect()
}

#[test]
fn code_owners_and_approval_policies() {
    let addr = start();
    let mut users = vec![];
    for name in ["alice", "bob"] {
        let u = api(&addr, "POST", "/users", Some(json!({"username": name, "name": name, "email": format!("{name}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true})));
        users.push(u["id"].as_i64().unwrap());
    }
    let p = api(&addr, "POST", "/projects", Some(json!({"name": "app", "initialize_with_readme": true})));
    let pid = p["id"].as_i64().unwrap();
    for u in &users {
        api(&addr, "POST", &format!("/projects/{pid}/members"), Some(json!({"user_id": u, "access_level": 30})));
    }

    // Code owners: set, read back, and the MR's rule.
    let sections = json!([
        {"name": null, "entries": [{"pattern": "*", "owners": ["@alice"]}]},
        {"name": "Docs", "approvals": 2, "entries": [{"pattern": "/docs/", "owners": ["@alice", "@bob", "@nobody"]}]},
    ]);
    let (status, set) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/code_owners"), Some(json!({"sections": sections})));
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["branch"], "main");
    assert_eq!(set["sections"][1]["entries"][0]["owners"], json!(["@alice", "@bob"]));
    let (_, got) = call(&addr, "GET", &format!("/-/labrat/projects/{pid}/code_owners"), None);
    assert_eq!(got, set);
    let (status, _) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/code_owners"), Some(json!({"sections": [{"entries": [{"owners": []}]}]})));
    assert_eq!(status, 400);

    api(&addr, "POST", &format!("/projects/{pid}/repository/commits"), Some(json!({"branch": "f", "start_branch": "main", "commit_message": "a", "actions": [{"action": "create", "file_path": "src/a.txt", "content": "a"}]})));
    api(&addr, "POST", &format!("/projects/{pid}/merge_requests"), Some(json!({"source_branch": "f", "target_branch": "main", "title": "A"})));
    idle(&addr);
    let co = rules(&addr, pid, 1, "code_owner");
    assert_eq!(co.len(), 1, "{co:?}");
    assert_eq!(co[0]["name"], "*");
    assert_eq!(co[0]["section"], "codeowners");
    // Not required: main doesn't require code owner approval.
    assert_eq!(co[0]["approvals_required"], 0);
    api(&addr, "PATCH", &format!("/projects/{pid}/protected_branches/main"), Some(json!({"code_owner_approval_required": true})));
    assert_eq!(rules(&addr, pid, 1, "code_owner")[0]["approvals_required"], 1);
    // Clearing the code owners drops the rule.
    let (status, _) = call(&addr, "DELETE", &format!("/-/labrat/projects/{pid}/code_owners"), None);
    assert_eq!(status, 204);
    assert!(rules(&addr, pid, 1, "code_owner").is_empty());

    // Approval policies.
    let policies = json!([
        {"name": "Any MR", "enabled": true, "rules": [{"type": "any_merge_request", "branch_type": "protected", "commits": "any"}],
         "actions": [{"type": "require_approval", "approvals_required": 1, "user_approvers": ["bob"]}]},
        {"name": "Scans", "enabled": true, "rules": [{"type": "scan_finding", "branches": [], "scanners": ["sast"], "vulnerabilities_allowed": 0}],
         "actions": [{"type": "require_approval", "approvals_required": 2, "user_approvers": ["alice", "bob"]}]},
    ]);
    let (status, set) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/approval_policies"), Some(json!({"approval_policy": policies})));
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["approval_policy"].as_array().unwrap().len(), 2);
    let (status, _) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/approval_policies"), Some(json!({"approval_policy": [{"name": "x", "rules": [{"type": "nope"}]}]})));
    assert_eq!(status, 400);
    idle(&addr);
    let pr = rules(&addr, pid, 1, "report_approver");
    let required = |name: &str, pr: &[Value]| pr.iter().find(|r| r["name"] == name).unwrap()["approvals_required"].clone();
    assert_eq!(required("Any MR", &pr), 1);
    // No security reports: the scan policy fails closed.
    assert_eq!(required("Scans", &pr), 2);
    let notes = api(&addr, "GET", &format!("/projects/{pid}/merge_requests/1/notes"), None);
    assert!(notes.as_array().unwrap().iter().any(|n| n["author"]["username"] == "GitLab-Security-Bot"), "{notes}");
    let mr = api(&addr, "GET", &format!("/projects/{pid}/merge_requests/1"), None);
    assert_eq!(mr["detailed_merge_status"], "security_policy_violations");

    // The scans pass on this MR.
    let (status, _) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/merge_requests/1/policy_results"), Some(json!({"policies": {"Scans": {"violated": false}}})));
    assert_eq!(status, 204);
    assert_eq!(required("Scans", &rules(&addr, pid, 1, "report_approver")), 0);
    let (status, _) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/merge_requests/1/policy_results"), Some(json!({"policies": {"Nope": {"violated": false}}})));
    assert_eq!(status, 400);

    // Findings, in GraphQL's shapes.
    let findings = json!({"policies": {"Scans": {
        "newScanFinding": [{"name": "SQL injection", "path": "app/db.rb", "reportType": "SAST", "severity": "CRITICAL", "location": {"file": "app/db.rb", "start_line": 3}}],
        "previousScanFinding": [{"name": "Old XSS", "reportType": "SAST", "severity": "LOW"}],
        "licenseScanning": [{"license": "GPL-3.0", "dependencies": ["libfoo"], "url": "https://spdx.org/licenses/GPL-3.0"}],
        "errors": [{"error": "SCAN_NOT_SUCCEEDED", "message": "The SAST job failed."}],
        "comparisonPipelines": [{"source": ["gid://gitlab/Ci::Pipeline/2"], "target": ["gid://gitlab/Ci::Pipeline/1"]}],
    }}});
    let (status, body) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/merge_requests/1/policy_results"), Some(findings));
    assert_eq!(status, 204, "{body}");
    let (status, body) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/merge_requests/1/policy_results"), Some(json!({"policies": {"Scans": {"newScanFinding": [{"reportType": "NOPE"}]}}})));
    assert_eq!(status, 400, "{body}");
    let q = "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { detailedMergeStatus policyViolations { violationsCount
        policies { name status reportType } newScanFinding { name path reportType severity location } previousScanFinding { name severity }
        licenseScanning { license dependencies url } errors { error reportType message data } comparisonPipelines { reportType source target } } } } }";
    let (_, g) = call(&addr, "POST", "/api/graphql", Some(json!({"query": q, "variables": {"p": p["path_with_namespace"]}})));
    let pv = &g["data"]["project"]["mergeRequest"]["policyViolations"];
    assert_eq!(pv["violationsCount"], 4, "{g}");
    assert_eq!(pv["newScanFinding"][0]["severity"], "CRITICAL");
    assert_eq!(pv["newScanFinding"][0]["location"]["start_line"], 3);
    assert_eq!(pv["licenseScanning"][0]["dependencies"], json!(["libfoo"]));
    assert_eq!(pv["errors"], json!([{"error": "SCAN_NOT_SUCCEEDED", "reportType": "SCAN_FINDING", "message": "The SAST job failed.", "data": {}}]));
    assert_eq!(pv["comparisonPipelines"][0]["reportType"], "SCAN_FINDING");
    assert_eq!(required("Scans", &rules(&addr, pid, 1, "report_approver")), 2);
    // Back to passing.
    let (status, _) = call(&addr, "PUT", &format!("/-/labrat/projects/{pid}/merge_requests/1/policy_results"), Some(json!({"policies": {"Scans": {"violated": false}}})));
    assert_eq!(status, 204);
    // Bob's approval satisfies the rest.
    let bob_token = api(&addr, "POST", &format!("/users/{}/personal_access_tokens", users[1]), Some(json!({"name": "t", "scopes": ["api"]})))["token"].as_str().unwrap().to_string();
    let out = Command::new("curl")
        .args(["-s", "-X", "POST", "-H", &format!("PRIVATE-TOKEN: {bob_token}"), &format!("http://{addr}/api/v4/projects/{pid}/merge_requests/1/approve")])
        .output()
        .unwrap();
    let approved: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(approved["approved"], true, "{approved}");
    idle(&addr);
    let mr = api(&addr, "GET", &format!("/projects/{pid}/merge_requests/1"), None);
    assert_eq!(mr["detailed_merge_status"], "mergeable", "{mr}");

    // Removing the policies removes their rules.
    let (status, _) = call(&addr, "DELETE", &format!("/-/labrat/projects/{pid}/approval_policies"), None);
    assert_eq!(status, 204);
    assert!(rules(&addr, pid, 1, "report_approver").is_empty());
}
