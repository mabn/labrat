//! Conformance harness: runs a scenario against GitLab (recording) or labrat (checking) and
//! compares normalized responses.
//!
//! * Record: `LABRAT_RECORD=1 GITLAB_URL=... GITLAB_TOKEN=... cargo test -p labrat-conformance -- --test-threads=1`
//!   runs scenarios against the reference GitLab and writes `fixtures/<scenario>.json`.
//! * Check (default): starts labrat in-process and compares each step with the fixture.
//!
//! Scenarios use names derived from a run token (`s.name("alice")`), so recordings don't collide
//! with existing data on the reference instance. Normalization replaces the run token, base URL,
//! timestamps, tokens, SHAs and database ids with stable placeholders.

pub mod normalize;

use normalize::Normalizer;
use reqwest::Method;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Target {
    /// Where requests are sent.
    pub base: String,
    /// The base URL the server puts into responses.
    pub public_base: String,
    pub admin_token: String,
    pub recording: bool,
    /// The in-process labrat, when replaying.
    pub app: Option<Arc<labrat_core::App>>,
}

pub struct Scenario {
    pub name: String,
    pub target: Target,
    pub run: String,
    pub http: reqwest::Client,
    norm: Normalizer,
    steps: Vec<Value>,
    expected: Option<Vec<Value>>,
    failures: Vec<String>,
    /// When the scenario started; step times are measured from here.
    started: std::time::Instant,
    /// Milliseconds since the start at which each recorded step's request was sent.
    times: Vec<i64>,
    /// When each recorded step's response arrived: GitLab's writes land near the end of a request.
    done_times: Vec<i64>,
    pending_at: Option<i64>,
    /// The recorded step times; replaying moves labrat's clock forward to keep up with them,
    /// so time-based behaviour (GitLab's one-minute touch throttle) replays faithfully.
    expected_times: Option<Vec<i64>>,
    clock_offset: i64,
    /// Wall-clock time (ms) the scenario started at; in replay, the recording's start, which
    /// labrat's clock is set to so that dates in responses match the recording day.
    started_at: i64,
    /// labrat's clock minus the real clock at the start of a replay.
    base_offset: i64,
    /// The SSH port the instance puts into `ssh_url_to_repo`.
    ssh_port: u16,
    /// `GITLAB_EXTRA_HEADERS` while recording; empty otherwise.
    extra_headers: Vec<(String, String)>,
}

/// One HTTP response with its parsed JSON body.
#[derive(Debug, Clone)]
pub struct Res {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
}

impl Res {
    pub fn id(&self) -> i64 {
        self.body["id"]
            .as_i64()
            .unwrap_or_else(|| panic!("no id in {}", self.body))
    }

    pub fn get(&self, key: &str) -> &Value {
        &self.body[key]
    }
}

/// Headers compared between GitLab and labrat.
const COMPARED_HEADERS: &[&str] = &[
    "content-type",
    "x-total",
    "x-total-pages",
    "x-page",
    "x-per-page",
    "x-next-page",
    "x-prev-page",
    "link",
    "www-authenticate",
    "location",
];

/// The edition a scenario runs as. The `conformance_ee` test binary runs the EE recordings
/// (`fixtures-ee/`); every other binary runs the CE ones (`fixtures/`). `LABRAT_EDITION=ce|ee`
/// overrides that.
pub fn edition() -> labrat_core::Edition {
    match std::env::var("LABRAT_EDITION").as_deref() {
        Ok("ee") => return labrat_core::Edition::Ee,
        Ok("ce") => return labrat_core::Edition::Ce,
        _ => {}
    }
    let exe = std::env::current_exe().ok();
    let name = exe.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with("conformance_ee") {
        labrat_core::Edition::Ee
    } else {
        labrat_core::Edition::Ce
    }
}

fn fixture_path(name: &str) -> PathBuf {
    let dir = if edition().is_ee() { "fixtures-ee" } else { "fixtures" };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(dir)
        .join(format!("{name}.json"))
}

/// Extra request headers for the reference instance, from `GITLAB_EXTRA_HEADERS`
/// (`Name: value` pairs separated by `;`). They are for a proxy in front of the instance, such
/// as an access gateway. They are sent only while recording and are never recorded: every
/// mention of them is removed from the fixture, and the fixture isn't written if one remains.
pub fn extra_headers() -> Vec<(String, String)> {
    std::env::var("GITLAB_EXTRA_HEADERS")
        .unwrap_or_default()
        .split(';')
        .filter_map(|p| {
            let (k, v) = p.split_once(':')?;
            let (k, v) = (k.trim(), v.trim());
            (!k.is_empty() && !v.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// Checks once per process that the instance being recorded is the edition being recorded, so
/// CE responses never land in `fixtures-ee/` or the other way round.
async fn check_recorded_edition(http: &reqwest::Client, base: &str, token: &str) {
    static CHECKED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    CHECKED
        .get_or_init(|| async {
            let v: Value = http
                .get(format!("{base}/api/v4/version"))
                .header("PRIVATE-TOKEN", token)
                .send()
                .await
                .expect("version request failed")
                .json()
                .await
                .expect("version is not JSON (is the access gateway letting requests through?)");
            let ee = v["enterprise"].as_bool().unwrap_or(false);
            assert_eq!(
                ee,
                edition().is_ee(),
                "recording {:?} fixtures, but {base} is {} ({})",
                edition(),
                if ee { "EE" } else { "CE" },
                v["version"]
            );
        })
        .await;
}

/// Starts labrat in-process. `external_url` is the recorded instance's URL, so generated
/// content (READMEs, links inside files) is byte-identical to the recording.
async fn start_labrat(external_url: &str, ssh_port: u16) -> Target {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let root_token = "glpat-labrat-root-token-000".to_string();
    let config = labrat_core::Config {
        external_url: external_url.to_string(),
        ssh_host: "localhost".into(),
        ssh_port,
        root_password: "labrat-root-password".into(),
        root_token: Some(root_token.clone()),
        edition: edition(),
    };
    let app = Arc::new(labrat_core::App {
        db: labrat_core::Db::open(":memory:").unwrap(),
        config,
    });
    labrat_api::bootstrap(&app).unwrap();
    // Sidekiq picks jobs up after a moment: a request sent right after creating an MR still
    // sees it `preparing`, and one right after a read sees `checking`.
    let mut per_kind = std::collections::HashMap::new();
    per_kind.insert("mr_after_create".to_string(), 300);
    per_kind.insert("mr_check_mergeability".to_string(), 300);
    per_kind.insert("mr_check_mergeability_closed".to_string(), 1500);
    per_kind.insert("approval_events".to_string(), 200);
    per_kind.insert("note_event".to_string(), 300);
    // `PostReceive` -> `ProcessCommitWorker`: two Sidekiq hops before "mentioned in commit".
    per_kind.insert("commit_cross_references".to_string(), 750);
    // `PostReceive` -> `UpdateMergeRequestsWorker`: the MR refresh after a push.
    per_kind.insert("update_merge_requests".to_string(), 750);
    labrat_core::jobs::set_delays(labrat_core::jobs::Delays { default_ms: 0, per_kind_ms: per_kind, jitter_ms: 0 });
    // GitLab's sample award for emoji hook tests has id 1; keep real award ids clear of it,
    // as they are on any instance that has seen awards.
    app.db
        .tx(|c| {
            c.execute("INSERT INTO sqlite_sequence (name, seq) VALUES ('award_emoji', 1000)", [])?;
            Ok::<_, labrat_core::Error>(())
        })
        .unwrap();
    let served = app.clone();
    tokio::spawn(async move {
        axum::serve(listener, labrat_api::router(served)).await.unwrap();
    });
    let base = format!("http://localhost:{port}");
    // Mirror the reference instance: an admin user `claude` whose token drives the scenarios.
    let http = reqwest::Client::new();
    let r: Value = http
        .post(format!("{base}/api/v4/users"))
        .header("PRIVATE-TOKEN", &root_token)
        .json(
            &json!({"username": "claude", "name": "Claude", "email": "claude@gitlab.unhood.org",
                      "password": "claude-password-123", "admin": true, "skip_confirmation": true}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uid = r["id"].as_i64().expect("claude user");
    let t: Value = http
        .post(format!("{base}/api/v4/users/{uid}/personal_access_tokens"))
        .header("PRIVATE-TOKEN", &root_token)
        .json(&json!({"name": "claude-code", "scopes": ["api", "read_repository", "write_repository", "admin_mode"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    Target {
        base,
        public_base: external_url.to_string(),
        admin_token: t["token"].as_str().unwrap().to_string(),
        recording: false,
        app: Some(app),
    }
}

impl Scenario {
    pub async fn new(name: &str) -> Scenario {
        let recording = std::env::var("LABRAT_RECORD").is_ok_and(|v| v == "1");
        let fixture: Option<Value> = if recording {
            None
        } else {
            let path = fixture_path(name);
            let data = std::fs::read_to_string(&path).unwrap_or_else(|_| {
                panic!(
                    "missing fixture {}; record it with LABRAT_RECORD=1",
                    path.display()
                )
            });
            Some(serde_json::from_str(&data).unwrap())
        };
        let extra_headers = if recording { extra_headers() } else { vec![] };
        let mut default_headers = reqwest::header::HeaderMap::new();
        for (k, v) in &extra_headers {
            default_headers.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).expect("header name"),
                reqwest::header::HeaderValue::from_str(v).expect("header value"),
            );
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(default_headers)
            .build()
            .unwrap();
        let mut ssh_port = 2424;
        let (target, run) = if recording {
            let base = std::env::var("GITLAB_URL").expect("GITLAB_URL");
            let token = std::env::var("GITLAB_TOKEN").expect("GITLAB_TOKEN");
            // The URL the instance puts into its responses, when it differs from the one it is
            // reached at (for example behind a proxy).
            let public_base = std::env::var("GITLAB_PUBLIC_URL").unwrap_or_else(|_| base.clone());
            if let Ok(p) = std::env::var("GITLAB_SSH_PORT") {
                ssh_port = p.parse().expect("GITLAB_SSH_PORT");
            }
            check_recorded_edition(&http, &base, &token).await;
            let run = format!("r{:06x}", rand::random::<u32>() & 0xffffff);
            let target = Target {
                public_base,
                base,
                admin_token: token,
                recording: true,
                app: None,
            };
            (target, run)
        } else {
            let f = fixture.as_ref().unwrap();
            let public = f["base"]
                .as_str()
                .unwrap_or("http://localhost:8929")
                .to_string();
            let run = f["run"].as_str().unwrap_or("r000000").to_string();
            ssh_port = f["ssh_port"].as_u64().map(|p| p as u16).unwrap_or(2424);
            (start_labrat(&public, ssh_port).await, run)
        };
        // Prefer completion times; older recordings only have send times.
        let expected_times = fixture
            .as_ref()
            .and_then(|v| v["done_times"].as_array().or(v["times"].as_array()))
            .map(|a| a.iter().filter_map(Value::as_i64).collect());
        let wall_now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        // Recordings made before `started_at` existed were all made on 2026-11-07.
        let started_at = match &fixture {
            Some(v) => v["started_at"].as_i64().unwrap_or(LEGACY_RECORDING_START),
            None => wall_now,
        };
        let base_offset = started_at - wall_now;
        if !recording {
            let url = format!("{}/-/labrat/time?offset_ms={base_offset}", target.base);
            let r = reqwest::Client::new().post(&url).send().await.expect("clock request failed");
            assert!(r.status().is_success(), "setting the labrat clock failed: {}", r.status());
        }
        let expected = fixture.map(|v| v["steps"].as_array().unwrap().clone());
        let mut norm = Normalizer::new(&target.public_base, &run);
        norm.set_today(&days_to_date(started_at.div_euclid(86_400_000)));
        Scenario {
            name: name.to_string(),
            target,
            run,
            http,
            norm,
            steps: vec![],
            expected,
            failures: vec![],
            started: std::time::Instant::now(),
            times: vec![],
            done_times: vec![],
            pending_at: None,
            expected_times,
            clock_offset: 0,
            started_at,
            base_offset,
            ssh_port,
            extra_headers,
        }
    }

    /// A name unique to this run: `alice` becomes `alice-r1a2b3c`.
    pub fn name(&self, base: &str) -> String {
        format!("{base}-{}", self.run)
    }

    /// Runs `git` in `dir` (unrecorded) and returns its stdout; panics when git fails.
    /// It is async so the in-process labrat (on the same runtime) can serve git's requests meanwhile.
    pub async fn git(&self, dir: &std::path::Path, args: &[&str]) -> String {
        let out = self
            .git_command()
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Labrat Test")
            .env("GIT_AUTHOR_EMAIL", "labrat-test@example.com")
            .env("GIT_COMMITTER_NAME", "Labrat Test")
            .env("GIT_COMMITTER_EMAIL", "labrat-test@example.com")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .await
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Runs `git push` (or any git command) and records the `remote:` lines it prints as a step.
    pub async fn git_remote_lines(&mut self, label: &str, dir: &std::path::Path, args: &[&str]) {
        let out = self
            .git_command()
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .await
            .expect("git runs");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let lines: Vec<Value> = text
            .lines()
            .filter_map(|l| l.strip_prefix("remote:"))
            .map(|l| Value::String(l.trim_end().to_string()))
            .collect();
        let res = Res { status: if out.status.success() { 200 } else { 500 }, headers: Default::default(), body: Value::Array(lines) };
        self.record(label, &Method::POST, "/<git push>", &res, "push");
    }

    /// A `git` command that sends the extra request headers (`http.extraHeader`) while recording.
    fn git_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("git");
        if !self.extra_headers.is_empty() {
            cmd.env("GIT_CONFIG_COUNT", self.extra_headers.len().to_string());
            for (i, (k, v)) in self.extra_headers.iter().enumerate() {
                cmd.env(format!("GIT_CONFIG_KEY_{i}"), "http.extraHeader");
                cmd.env(format!("GIT_CONFIG_VALUE_{i}"), format!("{k}: {v}"));
            }
        }
        cmd
    }

    /// The HTTP clone URL of a project on the current target, with `token` as the password.
    pub fn clone_url(&self, full_path: &str, token: &str) -> String {
        let base = &self.target.base;
        let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
        format!("{scheme}://oauth2:{token}@{rest}/{full_path}.git")
    }

    /// Replaces `text` with `placeholder` wherever it appears in recorded responses (for values
    /// that depend on the day of the run, such as token expiry dates).
    pub fn alias(&mut self, text: &str, placeholder: &str) {
        self.norm.aliases.push((text.to_string(), placeholder.to_string()));
    }

    /// A date `days` from today, as `YYYY-MM-DD`, aliased to `<TODAY+days>` in recordings.
    pub fn date_in(&mut self, days: i64) -> String {
        let s = days_to_date(self.started_at.div_euclid(86_400_000) + days);
        self.alias(&s, &format!("<TODAY+{days}>"));
        s
    }

    pub fn admin(&self) -> String {
        self.target.admin_token.clone()
    }

    /// Waits for background work: GitLab's Sidekiq when recording, labrat's job queue otherwise.
    pub async fn settle(&mut self) {
        if self.target.recording {
            // Sidekiq's busy counter only updates on heartbeats, so wait until no job is
            // enqueued and the processed counter has been still for a few seconds.
            let url = format!("{}/api/v4/sidekiq/compound_metrics", self.target.base);
            let mut last = -1i64;
            let mut still_since = std::time::Instant::now();
            for _ in 0..200 {
                let m: Value = match self
                    .http
                    .get(&url)
                    .header("PRIVATE-TOKEN", &self.target.admin_token)
                    .send()
                    .await
                {
                    Ok(r) => r.json().await.unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                };
                let enqueued = m["jobs"]["enqueued"].as_i64().unwrap_or(0);
                let processed = m["jobs"]["processed"].as_i64().unwrap_or(0);
                if processed != last || enqueued > 0 {
                    last = processed;
                    still_since = std::time::Instant::now();
                } else if still_since.elapsed() >= std::time::Duration::from_millis(5500) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        } else {
            let _ = self
                .http
                .get(format!("{}/-/labrat/idle", self.target.base))
                .send()
                .await;
        }
    }

    /// Polls `path` (unrecorded) until `done` holds, so slow GitLab background work finishes.
    /// labrat gets the same single request so side effects of the GET match.
    pub async fn poll(&mut self, path: &str, done: impl Fn(&Value) -> bool) {
        let token = self.target.admin_token.clone();
        for _ in 0..if self.target.recording { 40 } else { 1 } {
            let r = self
                .send(Method::GET, path, Some(&token), Value::Null)
                .await;
            if done(&r.body) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        self.settle().await;
    }

    /// Waits until an MR is prepared and its merge status settled.
    pub async fn poll_mr(&mut self, project: i64, iid: i64) {
        let path = format!("/projects/{project}/merge_requests/{iid}");
        self.poll(&path, |b| {
            !b["prepared_at"].is_null()
                && !matches!(
                    b["merge_status"].as_str(),
                    Some("checking") | Some("unchecked") | Some("preparing")
                )
        })
        .await;
    }

    /// Sends a request; `body` goes as JSON for non-GET requests and as query params for GET.
    pub async fn send(
        &mut self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> Res {
        let url = if path.starts_with("/api/graphql") {
            format!("{}{}", self.target.base, path)
        } else {
            format!("{}/api/v4{}", self.target.base, path)
        };
        let mut rb = self.http.request(method.clone(), &url);
        if let Some(t) = token {
            rb = rb.header("PRIVATE-TOKEN", t);
        }
        if method == Method::GET || method == Method::DELETE {
            if let Value::Object(m) = &body {
                let pairs: Vec<(String, String)> = m
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            match v {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            },
                        )
                    })
                    .collect();
                rb = rb.query(&pairs);
            }
        } else if !body.is_null() {
            rb = rb.json(&body);
        }
        let resp = rb.send().await.expect("request failed");
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        Res {
            status,
            headers,
            body,
        }
    }

    /// Sends a request and records the step for comparison. `kind` names the type of the
    /// top-level object(s), used to map ids ("user", "project", "namespace", ...).
    pub async fn step(
        &mut self,
        label: &str,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Value,
        kind: &str,
    ) -> Res {
        self.sync_clock().await;
        let res = self.send(method.clone(), path, token, body.clone()).await;
        self.record(label, &method, path, &res, kind);
        self.hook_extra_headers(&method, path, &res).await;
        res
    }

    /// While recording through a proxy that needs `GITLAB_EXTRA_HEADERS`, webhook deliveries to
    /// the receiver need them too: after a hook is created or updated, they are added to it as
    /// custom headers (unrecorded). `record` hides them from every recorded response.
    async fn hook_extra_headers(&mut self, method: &Method, path: &str, res: &Res) {
        if self.extra_headers.is_empty() || !(200..300).contains(&res.status) || !(*method == Method::POST || *method == Method::PUT) {
            return;
        }
        let path = path.split('?').next().unwrap_or(path);
        let re = regex::Regex::new(r"^((?:/projects/[^/]+)?/hooks)(?:/(\d+))?$").unwrap();
        let Some(c) = re.captures(path) else { return };
        let id = match c.get(2) {
            Some(m) => m.as_str().to_string(),
            None => match res.body["id"].as_i64() {
                Some(id) => id.to_string(),
                None => return,
            },
        };
        let token = self.target.admin_token.clone();
        for (k, v) in self.extra_headers.clone() {
            let r = self
                .send(Method::PUT, &format!("{}/{id}/custom_headers/{k}", &c[1]), Some(&token), json!({"value": v}))
                .await;
            assert!((200..300).contains(&r.status), "adding {k} to hook {id}: {} {}", r.status, r.body);
        }
    }

    /// Before a recorded step: remember when it was sent (recording), or move labrat's clock so
    /// at least as much time has passed as when GitLab answered it (checking). GitLab writes near
    /// the end of a request, so labrat handles the step at GitLab's completion time.
    async fn sync_clock(&mut self) {
        let elapsed = self.started.elapsed().as_millis() as i64;
        if self.target.recording {
            self.pending_at = Some(elapsed);
            return;
        }
        let Some(want) = self.expected_times.as_ref().and_then(|t| t.get(self.steps.len()).copied()) else { return };
        let have = elapsed + self.clock_offset;
        if want > have {
            self.clock_offset += want - have;
            let url = format!("{}/-/labrat/time?offset_ms={}", self.target.base, self.base_offset + self.clock_offset);
            let r = self.http.post(&url).send().await.expect("clock request failed");
            assert!(r.status().is_success(), "setting the labrat clock failed: {}", r.status());
        }
    }

    /// A multipart upload (`file` plus text fields) to an API path, recorded as a step.
    #[allow(clippy::too_many_arguments)]
    pub async fn step_upload(&mut self, label: &str, path: &str, token: Option<&str>, field: &str, filename: &str, content_type: &str, data: &[u8], kind: &str) -> Res {
        self.sync_clock().await;
        let url = format!("{}/api/v4{}", self.target.base, path);
        let part = reqwest::multipart::Part::bytes(data.to_vec()).file_name(filename.to_string()).mime_str(content_type).unwrap();
        let form = reqwest::multipart::Form::new().part(field.to_string(), part);
        let mut rb = self.http.post(&url).multipart(form);
        if let Some(t) = token {
            rb = rb.header("PRIVATE-TOKEN", t);
        }
        let res = Self::finish_response(rb.send().await.expect("request failed")).await;
        self.record(label, &Method::POST, path, &res, kind);
        res
    }

    /// A GET outside the API (web paths such as uploaded files), recorded as a step.
    pub async fn step_web(&mut self, label: &str, path: &str, token: Option<&str>) -> Res {
        self.sync_clock().await;
        let url = format!("{}{}", self.target.base, path);
        let mut rb = self.http.get(&url);
        if let Some(t) = token {
            rb = rb.header("PRIVATE-TOKEN", t);
        }
        let res = Self::finish_response(rb.send().await.expect("request failed")).await;
        self.record(label, &Method::GET, path, &res, "upload");
        res
    }

    async fn finish_response(resp: reqwest::Response) -> Res {
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        Res { status, headers, body }
    }

    /// A GraphQL request (`POST /api/graphql`), recorded as a step.
    pub async fn graphql(&mut self, label: &str, token: Option<&str>, query: &str, variables: Value) -> Res {
        let mut body = json!({"query": query});
        if !variables.is_null() {
            body["variables"] = variables;
        }
        self.step(label, Method::POST, "/api/graphql", token, body, "graphql").await
    }

    /// A GraphQL request with an `operationName`.
    pub async fn graphql_named(&mut self, label: &str, token: Option<&str>, query: &str, op: &str) -> Res {
        let body = json!({"query": query, "operationName": op});
        self.step(label, Method::POST, "/api/graphql", token, body, "graphql").await
    }

    pub fn record(&mut self, label: &str, method: &Method, path: &str, res: &Res, kind: &str) {
        let mut headers = Map::new();
        for h in COMPARED_HEADERS {
            if let Some(v) = res.headers.get(*h) {
                headers.insert(
                    h.to_string(),
                    Value::String(v.to_str().unwrap_or("").to_string()),
                );
            }
        }
        let path = self.norm.string(path, "");
        let mut body = res.body.clone();
        sort_rule_users(&mut body);
        let mut step = json!({
            "label": label,
            "request": format!("{method} {path}"),
            "status": res.status,
            "headers": Value::Object(headers),
            "body": body,
        });
        self.norm.value(&mut step["headers"], "header");
        self.norm.value_kind(&mut step["body"], kind);
        if !self.extra_headers.is_empty() {
            let names: Vec<String> = self.extra_headers.iter().map(|(k, _)| k.to_ascii_lowercase()).collect();
            strip_headers(&mut step["body"], &names);
        }
        if self.target.recording && std::env::var("HOOK_PUBLIC_URL").is_ok() {
            receiver_response_headers(&mut step["body"]);
        }
        let idx = self.steps.len();
        let at = self.pending_at.take().unwrap_or_else(|| self.started.elapsed().as_millis() as i64);
        self.times.push(at);
        self.done_times.push(self.started.elapsed().as_millis() as i64);
        if let Some(expected) = &self.expected {
            match expected.get(idx) {
                Some(exp) => {
                    if let Some(diff) = normalize::diff(exp, &step, "") {
                        self.failures.push(format!("step {idx} `{label}`: {diff}"));
                    }
                }
                None => self
                    .failures
                    .push(format!("step {idx} `{label}` is not in the fixture")),
            }
        }
        self.steps.push(step);
    }

    /// Writes the fixture (recording) or fails the test with all differences (checking).
    pub fn finish(self) {
        if self.target.recording {
            let path = fixture_path(&self.name);
            let data = json!({"scenario": self.name, "base": self.target.public_base, "ssh_port": self.ssh_port, "run": self.run, "steps": self.steps, "times": self.times, "done_times": self.done_times, "started_at": self.started_at});
            let text = serde_json::to_string_pretty(&data).unwrap() + "\n";
            let lower = text.to_ascii_lowercase();
            for marker in ["CF_Authorization", "Cf-Ray", "cloudflareaccess"] {
                assert!(!text.contains(marker), "not writing {}: it mentions {marker}", path.display());
            }
            for (k, v) in &self.extra_headers {
                assert!(
                    !text.contains(v.as_str()) && !lower.contains(&k.to_ascii_lowercase()),
                    "not writing {}: it mentions the extra header {k}",
                    path.display()
                );
            }
            if let Ok(base) = std::env::var("GITLAB_URL") {
                assert!(
                    base == self.target.public_base || !text.contains(&base),
                    "not writing {}: it contains the proxy URL {base}",
                    path.display()
                );
            }
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            eprintln!("recorded {}", path.display());
            return;
        }
        let expected = self.expected.unwrap();
        let mut failures = self.failures;
        // A failing background job is a labrat bug even when the responses happen to match.
        if let Some(app) = &self.target.app {
            let errors: Vec<String> = app
                .db
                .read(|c| {
                    let mut st = c.prepare("SELECT kind || ': ' || error FROM job_failures ORDER BY id")?;
                    let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
                    Ok::<_, labrat_core::Error>(v)
                })
                .unwrap_or_default();
            for e in errors {
                failures.push(format!("background job failed: {e}"));
            }
        }
        if expected.len() > self.steps.len() {
            failures.push(format!(
                "fixture has {} steps, run produced {}",
                expected.len(),
                self.steps.len()
            ));
        }
        if !failures.is_empty() {
            let actual = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target-actual")
                .join(format!("{}.json", self.name));
            let _ = std::fs::create_dir_all(actual.parent().unwrap());
            let _ = std::fs::write(
                &actual,
                serde_json::to_string_pretty(&json!({"steps": self.steps})).unwrap(),
            );
            panic!(
                "{} differences in scenario `{}` (actual output: {}):\n  {}",
                failures.len(),
                self.name,
                actual.display(),
                failures.join("\n  ")
            );
        }
    }
}

/// A webhook receiver reachable from the reference GitLab (in Docker, via the bridge
/// gateway) and from in-process labrat. Records every request it gets.
pub struct Receiver {
    pub url: String,
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
    seen: usize,
}

/// Request headers of webhook deliveries that are compared.
const HOOK_HEADERS: &[&str] = &[
    "content-type",
    "user-agent",
    "idempotency-key",
    "x-gitlab-event",
    "x-gitlab-webhook-uuid",
    "x-gitlab-instance",
    "x-gitlab-token",
    "x-gitlab-event-uuid",
    "authorization",
    "x-custom",
];

impl Scenario {
    /// Starts a webhook receiver. Its URL is normalized to `<HOOK>`.
    ///
    /// While recording, the reference instance has to reach it. By default it listens on all
    /// interfaces and GitLab (in Docker) reaches it through the bridge gateway. When the instance
    /// is elsewhere, set `HOOK_PUBLIC_URL` to a URL that forwards to `HOOK_LISTEN_PORT` on this
    /// machine (a tunnel, for example): every receiver then shares one listener on that port
    /// and gets its own path prefix, which is removed again before deliveries are recorded.
    pub async fn receiver(&mut self) -> Receiver {
        let requests: Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
        let tunnel = std::env::var("HOOK_PUBLIC_URL").ok().filter(|_| self.target.recording);
        let url = if let Some(public) = tunnel {
            let port: u16 = std::env::var("HOOK_LISTEN_PORT").expect("HOOK_LISTEN_PORT").parse().expect("HOOK_LISTEN_PORT");
            let routes = shared_receiver(port).await;
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
            let prefix = format!("/rx{}-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), self.run);
            routes.lock().unwrap().insert(prefix.clone(), requests.clone());
            format!("{}{prefix}", public.trim_end_matches('/'))
        } else {
            let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let host = if self.target.recording { "172.17.0.1" } else { "127.0.0.1" };
            let store = requests.clone();
            let app = axum::Router::new().fallback(move |method: axum::http::Method, uri: axum::http::Uri, headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let store = store.clone();
                async move { receive(&store, method, uri.to_string(), headers, body) }
            });
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            format!("http://{host}:{port}")
        };
        self.norm.aliases.push((url.clone(), "<HOOK>".into()));
        Receiver { url, requests, seen: 0 }
    }

    /// Waits for `count` new webhook deliveries and records them as one step, sorted by
    /// a stable key (GitLab delivers concurrently, so arrival order is not meaningful).
    pub async fn deliveries(&mut self, label: &str, rx: &mut Receiver, count: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(if self.target.recording { 90 } else { 10 });
        loop {
            let have = rx.requests.lock().unwrap().len() - rx.seen;
            if have >= count || std::time::Instant::now() > deadline {
                break;
            }
            if !self.target.recording {
                self.settle().await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Anything beyond `count` that arrives shortly after is a difference too.
        tokio::time::sleep(std::time::Duration::from_millis(if self.target.recording { 3000 } else { 200 })).await;
        let mut got: Vec<Value> = rx.requests.lock().unwrap()[rx.seen..].to_vec();
        rx.seen += got.len();
        got.sort_by_key(|d| {
            let b = &d["body"];
            let oa = &b["object_attributes"];
            format!(
                "{}|{}|{}|{}|{}|{}",
                d["headers"]["x-gitlab-event"], b["object_kind"], oa["action"], b["event_type"], b["ref"], oa["note"]
            )
        });
        let res = Res {
            status: 200,
            headers: Default::default(),
            body: Value::Array(got),
        };
        self.record(label, &Method::POST, "/<webhook deliveries>", &res, "hook");
    }
}

type Store = Arc<std::sync::Mutex<Vec<Value>>>;

/// Records one delivery and answers it: `500` for paths ending in `/fail`, `200` otherwise.
fn receive(
    store: &Store,
    method: axum::http::Method,
    uri: String,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> (axum::http::StatusCode, [(&'static str, &'static str); 2], &'static str) {
    let mut h = Map::new();
    for name in HOOK_HEADERS {
        if let Some(v) = headers.get(*name) {
            h.insert(name.to_string(), json!(v.to_str().unwrap_or("")));
        }
    }
    let body: Value = serde_json::from_slice(&body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into()));
    let path = uri.split('?').next().unwrap_or("");
    let status = if path.ends_with("/fail") {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    } else {
        axum::http::StatusCode::OK
    };
    store.lock().unwrap().push(json!({
        "request": format!("{method} {uri}"),
        "headers": Value::Object(h),
        "body": body,
    }));
    (status, [("content-type", "text/plain"), ("x-receiver", "conformance")], "received")
}

/// The listener shared by all receivers when recording through `HOOK_PUBLIC_URL`, routing by
/// the first path segment.
async fn shared_receiver(port: u16) -> Arc<std::sync::Mutex<std::collections::HashMap<String, Store>>> {
    type Routes = Arc<std::sync::Mutex<std::collections::HashMap<String, Store>>>;
    static ROUTES: tokio::sync::OnceCell<Routes> = tokio::sync::OnceCell::const_new();
    ROUTES
        .get_or_init(|| async move {
            let routes: Routes = Default::default();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("HOOK_LISTEN_PORT is in use");
            let r = routes.clone();
            let app = axum::Router::new().fallback(move |method: axum::http::Method, uri: axum::http::Uri, headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let r = r.clone();
                async move {
                    let full = uri.to_string();
                    let rest = full.trim_start_matches('/');
                    let (first, tail) = rest.split_once(['/', '?']).map(|(a, _)| (a, &rest[a.len()..])).unwrap_or((rest, ""));
                    let store = r.lock().unwrap().get(&format!("/{first}")).cloned();
                    match store {
                        Some(store) => {
                            let tail = if tail.is_empty() || tail.starts_with('?') { format!("/{tail}") } else { tail.to_string() };
                            receive(&store, method, tail, headers, body)
                        }
                        None => (axum::http::StatusCode::NOT_FOUND, [("content-type", "text/plain"), ("x-receiver", "conformance")], "unknown receiver"),
                    }
                }
            });
            // The runtime of the first scenario that needs it may end with that scenario, so the
            // listener gets a thread and runtime of its own.
            let std_listener = listener.into_std().unwrap();
            std::thread::spawn(move || {
                tokio::runtime::Runtime::new().unwrap().block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
                    axum::serve(listener, app).await.unwrap();
                });
            });
            routes
        })
        .await
        .clone()
}

/// A hook delivery log (`response_headers`) recorded through the tunnel shows the tunnel's
/// response headers (cookies, ray ids and so on), not the receiver's. Puts back exactly what the
/// receiver sends, in the order GitLab logs it when it reaches the receiver directly.
fn receiver_response_headers(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Object(h)) = m.get_mut("response_headers") {
                let get = |name: &str| h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
                if get("X-Receiver").is_some() {
                    let mut out = Map::new();
                    out.insert("Content-Type".into(), get("Content-Type").unwrap_or(json!("text/plain")));
                    out.insert("X-Receiver".into(), json!("conformance"));
                    out.insert("Content-Length".into(), get("Content-Length").unwrap_or(json!("8")));
                    out.insert("Connection".into(), json!("close"));
                    if let Some(d) = get("Date") {
                        out.insert("Date".into(), d);
                    }
                    *h = out;
                }
            }
            for x in m.values_mut() {
                receiver_response_headers(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(receiver_response_headers),
        _ => {}
    }
}

/// GitLab EE lists an approval rule's users and eligible approvers without an `ORDER BY`, so
/// their order varies from call to call. Sorts them by username (before ids are normalized).
fn sort_rule_users(v: &mut Value) {
    match v {
        Value::Object(m) => {
            let rule = m.contains_key("approvals_required") || m.contains_key("approvalsRequired");
            for (k, x) in m.iter_mut() {
                if rule && matches!(k.as_str(), "users" | "eligible_approvers" | "eligibleApprovers" | "approvers") {
                    let list = match x {
                        Value::Array(a) => Some(a),
                        Value::Object(o) => match o.get_mut("nodes") {
                            Some(Value::Array(a)) => Some(a),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(a) = list
                        && a.iter().all(|u| u["username"].is_string()) {
                            a.sort_by(|p, q| p["username"].as_str().cmp(&q["username"].as_str()));
                        }
                }
                sort_rule_users(x);
            }
        }
        Value::Array(a) => {
            // Lists of approval rules: EE copies project rules onto an MR in no fixed order,
            // so the MR's rules (and their ids) come in varying order.
            let key = |r: &Value| -> Option<(String, String)> {
                let t = r.get("rule_type").or_else(|| r.get("type"))?.as_str()?.to_lowercase();
                let n = r.get("name")?.as_str()?.to_string();
                (r.get("approvals_required").is_some() || r.get("approvalsRequired").is_some() || r.get("rule_type").is_some()).then_some((t, n))
            };
            if !a.is_empty() && a.iter().all(|r| key(r).is_some()) {
                a.sort_by_key(|r| key(r).unwrap());
            }
            a.iter_mut().for_each(sort_rule_users)
        }
        _ => {}
    }
}

/// Removes every mention of the headers `names` (lowercase) from a recorded body: object keys,
/// and `{"key": name}` entries such as a hook's `custom_headers`.
fn strip_headers(v: &mut Value, names: &[String]) {
    match v {
        Value::Object(m) => {
            m.retain(|k, _| !names.contains(&k.to_ascii_lowercase()));
            for x in m.values_mut() {
                strip_headers(x, names);
            }
        }
        Value::Array(a) => {
            a.retain(|x| !x["key"].as_str().is_some_and(|k| names.contains(&k.to_ascii_lowercase())));
            for x in a.iter_mut() {
                strip_headers(x, names);
            }
        }
        _ => {}
    }
}

/// 2026-11-07 12:00 UTC: the start assumed for recordings without `started_at`.
const LEGACY_RECORDING_START: i64 = 1_794_052_800_000;

/// `YYYY-MM-DD` for a day number (days since 1970-01-01), using the civil-from-days algorithm.
fn days_to_date(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}
