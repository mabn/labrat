//! `git push -o merge_request.*` through the real git client against an in-process labrat.

use serde_json::Value;
use std::process::Command;
use std::sync::Arc;

const TOKEN: &str = "glpat-labrat-push-tests-0000";

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

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "protocol.version=0"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn api(addr: &str, method: &str, path: &str, body: Option<&str>) -> Value {
    let mut cmd = Command::new("curl");
    cmd.args(["-s", "-X", method, "-H", &format!("PRIVATE-TOKEN: {TOKEN}"), "-H", "Content-Type: application/json"]);
    if let Some(b) = body {
        cmd.args(["-d", b]);
    }
    cmd.arg(format!("http://{addr}/api/v4{path}"));
    serde_json::from_slice(&cmd.output().unwrap().stdout).unwrap_or(Value::Null)
}

#[test]
fn push_options_create_and_update_merge_requests() {
    let addr = start();
    api(&addr, "POST", "/projects", Some(r#"{"name":"po","initialize_with_readme":true}"#));
    let dir = std::env::temp_dir().join(format!("labrat-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["clone", "-q", &format!("http://root:{TOKEN}@{addr}/root/po.git"), "repo"]);
    let repo = dir.join("repo");
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-qm", "Add a"]);
    let out = git(&repo, &["push", "-o", "merge_request.create", "-o", "merge_request.title=From push", "-o", "mr.label=bug", "-o", "merge_request.description=Line1\\nLine2", "origin", "feat"]);
    assert!(out.contains("View merge request for feat:"), "{out}");

    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    git(&repo, &["add", "b.txt"]);
    git(&repo, &["commit", "-qm", "Add b"]);
    git(&repo, &["push", "-o", "merge_request.draft", "-o", "merge_request.unlabel=bug", "origin", "feat"]);

    git(&repo, &["checkout", "-q", "-b", "feat2"]);
    std::fs::write(repo.join("c.txt"), "c\n").unwrap();
    git(&repo, &["add", "c.txt"]);
    git(&repo, &["commit", "-qm", "Add c"]);
    let out = git(&repo, &["push", "-o", "merge_request.create", "-o", "merge_request.target=nope", "origin", "feat2"]);
    assert!(out.contains("WARNINGS: Error encountered with push options"), "{out}");
    assert!(out.contains("root/po:nope does not exist"), "{out}");

    let mrs = api(&addr, "GET", "/projects/1/merge_requests", None);
    let list = mrs.as_array().unwrap();
    assert_eq!(list.len(), 1);
    let mr = &list[0];
    assert_eq!(mr["title"], "Draft: From push");
    assert_eq!(mr["description"], "Line1\nLine2");
    assert_eq!(mr["labels"].as_array().unwrap().len(), 0);
    assert_eq!(mr["assignees"][0]["username"], "root");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `push_event_hooks_limit`: a push creating more than three branches sends no push hooks for
/// them; three or fewer each get one.
#[test]
fn push_hooks_limit() {
    let addr = start();
    let p = api(&addr, "POST", "/projects", Some(r#"{"name":"lim","initialize_with_readme":true}"#));
    let pid = p["id"].as_i64().unwrap();
    // The receiver doesn't exist; deliveries are still logged.
    api(&addr, "PUT", "/application/settings", Some(r#"{"allow_local_requests_from_web_hooks_and_services":true}"#));
    let hook = api(&addr, "POST", &format!("/projects/{pid}/hooks"), Some(r#"{"url":"http://127.0.0.1:9/hook","push_events":true}"#));
    let hid = hook["id"].as_i64().unwrap();
    let dir = std::env::temp_dir().join(format!("labrat-limit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("http://root:{TOKEN}@{addr}/root/lim.git");
    git(&dir, &["clone", "-q", &url, "repo"]);
    let repo = dir.join("repo");
    for b in ["a1", "a2", "a3", "a4", "b1", "b2"] {
        git(&repo, &["branch", b]);
    }
    let wait = || {
        Command::new("curl").args(["-s", &format!("http://{addr}/-/labrat/idle")]).output().unwrap();
        api(&addr, "GET", &format!("/projects/{pid}/hooks/{hid}/events"), None).as_array().map(Vec::len).unwrap_or(0)
    };
    git(&repo, &["push", "-q", "origin", "a1", "a2", "a3", "a4"]);
    assert_eq!(wait(), 0, "four new branches in one push: no push hooks");
    git(&repo, &["push", "-q", "origin", "b1", "b2"]);
    assert_eq!(wait(), 2, "two new branches: one hook each");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shallow clones with the real git client, as CI jobs and review bots make them: `--depth`,
/// `--deepen`, `--unshallow`, and a push from a shallow clone. Uses git's default protocol
/// (it asks for v2 and falls back).
#[test]
fn shallow_clone_fetch_and_push() {
    let addr = start();
    api(&addr, "POST", "/projects", Some(r#"{"name":"sh","initialize_with_readme":true}"#));
    for i in 1..=4 {
        let body = format!(r#"{{"branch":"main","commit_message":"C{i}","actions":[{{"action":"create","file_path":"f{i}.txt","content":"{i}\n"}}]}}"#);
        api(&addr, "POST", "/projects/1/repository/commits", Some(&body));
    }
    let dir = std::env::temp_dir().join(format!("labrat-shallow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |cwd: &std::path::Path, args: &[&str]| -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", &format!("protocol.version={}", std::env::var("LABRAT_GIT_PROTO").unwrap_or("2".into()))])
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let url = format!("http://root:{TOKEN}@{addr}/root/sh.git");
    run(&dir, &["clone", "-q", "--depth", "1", &url, "repo"]);
    let repo = dir.join("repo");
    let count = |r: &std::path::Path| run(r, &["rev-list", "--count", "HEAD"]).trim().parse::<u32>().unwrap();
    assert_eq!(count(&repo), 1);
    assert!(repo.join(".git/shallow").exists());
    assert!(repo.join("f4.txt").exists());
    run(&repo, &["fetch", "-q", "--deepen=2"]);
    assert_eq!(count(&repo), 3);
    run(&repo, &["fetch", "-q", "--unshallow"]);
    assert_eq!(count(&repo), 5);
    assert!(!repo.join(".git/shallow").exists());
    // A push from a fresh shallow clone.
    run(&dir, &["clone", "-q", "--depth", "1", &url, "repo2"]);
    let repo2 = dir.join("repo2");
    std::fs::write(repo2.join("g.txt"), "g\n").unwrap();
    run(&repo2, &["add", "g.txt"]);
    run(&repo2, &["commit", "-qm", "From shallow"]);
    run(&repo2, &["push", "-q", "origin", "HEAD:shallow-branch"]);
    let branch = api(&addr, "GET", "/projects/1/repository/branches/shallow-branch", None);
    assert_eq!(branch["commit"]["title"], "From shallow");
    let _ = std::fs::remove_dir_all(&dir);
}
