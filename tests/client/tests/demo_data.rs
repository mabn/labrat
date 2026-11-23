//! `POST /-/labrat/demo` (the web UI's "Generate demo data") in EE and CE mode: every merge
//! request ends up in the state its title describes, and a second run is refused.

use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;

const TOKEN: &str = "glpat-labrat-demo-data-tests-000";

fn start(edition: labrat_core::Edition) -> String {
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
                edition,
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

fn call(addr: &str, method: &str, path: &str) -> (u16, Value) {
    let out = Command::new("curl")
        .args(["-s", "-w", "\n%{http_code}", "-X", method, "-H", &format!("PRIVATE-TOKEN: {TOKEN}"), &format!("http://{addr}{path}")])
        .output()
        .unwrap();
    let out = String::from_utf8(out.stdout).unwrap();
    let (body, code) = out.rsplit_once('\n').unwrap();
    (code.trim().parse().unwrap(), serde_json::from_str(body).unwrap_or(Value::Null))
}

/// `project!iid` → (detailed merge status, head pipeline status).
fn states(addr: &str, projects: &[&str]) -> Vec<(String, String, String)> {
    // Reading an MR starts its mergeability check; read again once the checks are done.
    for _ in 0..2 {
        for p in projects {
            let (_, list) = call(addr, "GET", &format!("/api/v4/projects/demo%2F{p}/merge_requests?per_page=100"));
            for m in list.as_array().unwrap() {
                call(addr, "GET", &format!("/api/v4/projects/demo%2F{p}/merge_requests/{}", m["iid"]));
            }
        }
        call(addr, "GET", "/-/labrat/idle");
    }
    let mut out = vec![];
    for p in projects {
        let (_, list) = call(addr, "GET", &format!("/api/v4/projects/demo%2F{p}/merge_requests?per_page=100&sort=asc"));
        for m in list.as_array().unwrap() {
            let (_, mr) = call(addr, "GET", &format!("/api/v4/projects/demo%2F{p}/merge_requests/{}", m["iid"]));
            let pipeline = mr["head_pipeline"]["status"].as_str().unwrap_or("-").to_string();
            out.push((format!("{p}!{}", m["iid"]), mr["detailed_merge_status"].as_str().unwrap().to_string(), pipeline));
        }
    }
    out
}

fn expect(actual: Vec<(String, String, String)>, expected: &[(&str, &str, &str)]) {
    let expected: Vec<(String, String, String)> = expected.iter().map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string())).collect();
    assert_eq!(actual, expected);
}

const WEB_APP: &[(&str, &str, &str)] = &[
    ("web-app!1", "mergeable", "-"),
    ("web-app!2", "mergeable", "-"),
    ("web-app!3", "draft_status", "-"),
    ("web-app!4", "conflict", "-"),
    ("web-app!5", "<changes requested>", "-"),
    ("web-app!6", "mergeable", "-"),
    ("web-app!7", "not_open", "-"),
    ("web-app!8", "not_open", "-"),
];

fn web_app(ee: bool) -> Vec<(&'static str, &'static str, &'static str)> {
    WEB_APP.iter().map(|&(m, s, p)| (m, if s == "<changes requested>" { if ee { "requested_changes" } else { "mergeable" } } else { s }, p)).collect()
}

#[test]
fn demo_data_ee() {
    let addr = start(labrat_core::Edition::Ee);
    let (status, body) = call(&addr, "POST", "/-/labrat/demo");
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["projects"].as_array().unwrap().len(), 4);
    let mut expected = web_app(true);
    expected.extend_from_slice(&[
        ("deploy-service!1", "mergeable", "success"),
        ("deploy-service!2", "ci_must_pass", "failed"),
        ("deploy-service!3", "ci_still_running", "running"),
        ("deploy-service!4", "ci_still_running", "pending"),
        ("deploy-service!5", "not_open", "success"),
        ("deploy-service!6", "ci_still_running", "running"),
        ("deploy-service!7", "ci_still_running", "pending"),
        ("api-service!1", "mergeable", "-"),
        ("api-service!2", "not_approved", "-"),
        ("api-service!3", "not_approved", "-"),
        ("api-service!4", "not_approved", "-"),
        ("api-service!5", "not_approved", "-"),
        ("api-service!6", "merge_request_blocked", "-"),
        ("payments!1", "security_policy_violations", "-"),
        ("payments!2", "security_policy_violations", "-"),
        ("payments!3", "mergeable", "-"),
    ]);
    expect(states(&addr, &["web-app", "deploy-service", "api-service", "payments"]), &expected);

    // The code owner MR only lacks Dave's approval; the reset one lost both approvals.
    let (_, a) = call(&addr, "GET", "/api/v4/projects/demo%2Fapi-service/merge_requests/3/approvals");
    assert_eq!(a["approvals_left"], 1, "{a}");
    let (_, a) = call(&addr, "GET", "/api/v4/projects/demo%2Fapi-service/merge_requests/4/approvals");
    assert_eq!(a["approved_by"], json!([]), "{a}");
    // Two cars on the train, one merged by it.
    let (_, cars) = call(&addr, "GET", "/api/v4/projects/demo%2Fdeploy-service/merge_trains?scope=active&sort=asc");
    let iids: Vec<&Value> = cars.as_array().unwrap().iter().map(|c| &c["merge_request"]["iid"]).collect();
    assert_eq!(iids, [&json!(6), &json!(7)]);
    // root has to-dos from the review request and the mention.
    let (_, todos) = call(&addr, "GET", "/api/v4/todos");
    assert_eq!(todos.as_array().unwrap().len(), 2, "{todos}");

    let (status, body) = call(&addr, "POST", "/-/labrat/demo");
    assert_eq!(status, 409, "{body}");
}

#[test]
fn demo_data_ce() {
    let addr = start(labrat_core::Edition::Ce);
    let (status, body) = call(&addr, "POST", "/-/labrat/demo");
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["projects"].as_array().unwrap().len(), 2);
    let mut expected = web_app(false);
    expected.extend_from_slice(&[
        ("deploy-service!1", "mergeable", "success"),
        ("deploy-service!2", "ci_must_pass", "failed"),
        ("deploy-service!3", "ci_still_running", "running"),
        ("deploy-service!4", "ci_still_running", "pending"),
    ]);
    expect(states(&addr, &["web-app", "deploy-service"]), &expected);
}
