//! The web UI's session: password sign-in, API calls authenticated by the session cookie (with
//! the CSRF token on writes, as GitLab's frontend does), sign-out, and the request log leaving
//! the UI's own calls out.

use serde_json::Value;
use std::process::Command;
use std::sync::Arc;

const TOKEN: &str = "glpat-labrat-web-tests-00000";

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

/// Runs curl and returns (status, headers, body).
fn curl(args: &[&str]) -> (u16, String, String) {
    let out = Command::new("curl").args(["-s", "-i"]).args(args).output().expect("curl runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

fn cookie(headers: &str, name: &str) -> Option<String> {
    headers
        .lines()
        .filter_map(|l| l.strip_prefix("set-cookie: ").or_else(|| l.strip_prefix("Set-Cookie: ")))
        .find_map(|c| c.strip_prefix(&format!("{name}="))?.split(';').next().map(str::to_string))
        .filter(|v| !v.is_empty())
}

#[test]
fn session_sign_in_and_api_access() {
    let addr = start();
    let url = |p: &str| format!("http://{addr}{p}");

    // A wrong password goes back to the sign-in page.
    let (status, head, _) = curl(&["-X", "POST", "-d", "user[login]=root&user[password]=nope", &url("/users/sign_in")]);
    assert_eq!(status, 302);
    assert!(head.contains("location: /users/sign_in?error=invalid"), "{head}");

    // The right one sets the session and CSRF cookies and redirects to a local path only.
    let (status, head, _) = curl(&[
        "-X",
        "POST",
        "-d",
        "user[login]=root&user[password]=labrat-root-password&redirect=//evil.example",
        &url("/users/sign_in"),
    ]);
    assert_eq!(status, 302);
    assert!(head.contains("location: /\r\n"), "{head}");
    let session = cookie(&head, "_labrat_session").expect("session cookie");
    let csrf = cookie(&head, "_labrat_csrf").expect("csrf cookie");
    let jar = format!("_labrat_session={session}; _labrat_csrf={csrf}");

    // Reads work with the session alone.
    let (status, _, body) = curl(&["-H", &format!("Cookie: {jar}"), &url("/api/v4/user")]);
    assert_eq!(status, 200);
    let me: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(me["username"], "root");

    // A write without the CSRF token is anonymous; with it, it is the session's user.
    let create = |extra: &[&str]| {
        let target = url("/api/v4/projects");
        let mut args = vec!["-X", "POST", "-H", "Content-Type: application/json", "-d", r#"{"name":"web"}"#];
        args.extend_from_slice(extra);
        args.push(&target);
        curl(&args)
    };
    let cookie_header = format!("Cookie: {jar}");
    let (status, _, _) = create(&["-H", &cookie_header]);
    assert_eq!(status, 401);
    let csrf_header = format!("X-CSRF-Token: {csrf}");
    let (status, _, body) = create(&["-H", &cookie_header, "-H", &csrf_header, "-H", "X-Labrat-UI: 1"]);
    assert_eq!(status, 201, "{body}");

    // GraphQL accepts the session too.
    let (status, _, body) = curl(&[
        "-X",
        "POST",
        "-H",
        &cookie_header,
        "-H",
        &csrf_header,
        "-H",
        "Content-Type: application/json",
        "-d",
        r#"{"query":"{ currentUser { username } }"}"#,
        &url("/api/graphql"),
    ]);
    assert_eq!(status, 200);
    assert!(body.contains(r#""username":"root""#), "{body}");

    // Token calls are logged; the UI's own calls are not.
    curl(&["-H", &format!("PRIVATE-TOKEN: {TOKEN}"), &url("/api/v4/version")]);
    let (_, _, log) = curl(&[&url("/-/labrat/requests.json")]);
    let log: Value = serde_json::from_str(&log).unwrap();
    let uris: Vec<&str> = log.as_array().unwrap().iter().filter_map(|e| e["uri"].as_str()).collect();
    assert_eq!(uris, vec!["/api/v4/version"], "{log}");

    // Signing out needs the CSRF token, then the session no longer works.
    curl(&["-X", "POST", "-H", &cookie_header, "-d", "authenticity_token=wrong", &url("/users/sign_out")]);
    let (status, _, _) = curl(&["-H", &cookie_header, &url("/api/v4/user")]);
    assert_eq!(status, 200);
    let (status, _, _) = curl(&["-X", "POST", "-H", &cookie_header, "-d", &format!("authenticity_token={csrf}"), &url("/users/sign_out")]);
    assert_eq!(status, 302);
    let (status, _, _) = curl(&["-H", &cookie_header, &url("/api/v4/user")]);
    assert_eq!(status, 401);
}
