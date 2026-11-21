//! A merge request flow driven by the `gitlab` crate (a real client: form-encoded params,
//! pagination, its own URL building) against an in-process labrat.

use gitlab::api::projects::merge_requests::discussions::{CreateMergeRequestDiscussion, Position, TextPosition};
use gitlab::api::projects::merge_requests::notes::CreateMergeRequestNote;
use gitlab::api::projects::merge_requests::awards::CreateMergeRequestAward;
use gitlab::api::projects::merge_requests::{
    ApproveMergeRequest, CreateMergeRequest, EditMergeRequest, MergeMergeRequest, MergeRequest, MergeRequestState, MergeRequests,
};
use gitlab::api::projects::repository::commits::{CommitAction, CommitActionType, CreateCommit};
use gitlab::api::projects::CreateProject;
use gitlab::api::{self, Query};
use gitlab::hooktypes::{HookDate, HookKind, WebHookType};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ROOT_TOKEN: &str = "glpat-labrat-client-tests-000";

/// `PUT /application/settings`: allow webhooks to local addresses (off by default, as in GitLab).
struct AllowLocalWebhooks;

impl gitlab::api::Endpoint for AllowLocalWebhooks {
    fn method(&self) -> gitlab::api::endpoint_prelude::Method {
        gitlab::api::endpoint_prelude::Method::PUT
    }
    fn endpoint(&self) -> std::borrow::Cow<'static, str> {
        "application/settings".into()
    }
    fn body(&self) -> Result<Option<(&'static str, Vec<u8>)>, gitlab::api::BodyError> {
        let mut p = gitlab::api::FormParams::default();
        p.push("allow_local_requests_from_web_hooks_and_services", true);
        p.into_body()
    }
}

/// Starts labrat and a webhook receiver on a background runtime; returns their addresses.
fn start() -> (String, String, Arc<Mutex<Vec<Value>>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let hooks: Arc<Mutex<Vec<Value>>> = Default::default();
    let store = hooks.clone();
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
                root_token: Some(ROOT_TOKEN.into()),
                edition: labrat_core::Edition::Ce,
            };
            let app = Arc::new(labrat_core::App { db: labrat_core::Db::open(":memory:").unwrap(), config });
            labrat_api::bootstrap(&app).unwrap();
            tokio::spawn(async move { axum::serve(listener, labrat_api::router(app)).await.unwrap() });
            let hl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let haddr = hl.local_addr().unwrap().to_string();
            let receiver = axum::Router::new().fallback(move |body: axum::body::Bytes| {
                let store = store.clone();
                async move {
                    if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                        store.lock().unwrap().push(v);
                    }
                    "ok"
                }
            });
            tokio::spawn(async move { axum::serve(hl, receiver).await.unwrap() });
            tx.send((addr, haddr)).unwrap();
            std::future::pending::<()>().await;
        });
    });
    let (addr, haddr) = rx.recv().unwrap();
    (addr, haddr, hooks)
}

#[derive(Debug, Deserialize)]
struct Project {
    id: u64,
    path_with_namespace: String,
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct Mr {
    iid: u64,
    title: String,
    state: String,
    draft: bool,
    detailed_merge_status: String,
    diff_refs: Option<DiffRefs>,
    merge_commit_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiffRefs {
    base_sha: String,
    start_sha: String,
    head_sha: String,
}

#[derive(Debug, Deserialize)]
struct Discussion {
    id: String,
    notes: Vec<Note>,
}

#[derive(Debug, Deserialize)]
struct Note {
    body: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    position: Option<Value>,
}

fn wait_mergeable(client: &gitlab::Gitlab, path: &str, iid: u64) -> Mr {
    for _ in 0..100 {
        let mr: Mr = MergeRequest::builder().project(path).merge_request(iid).build().unwrap().query(client).unwrap();
        if mr.detailed_merge_status == "mergeable" {
            return mr;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("merge request never became mergeable");
}

#[test]
fn merge_request_flow() {
    let (addr, hook_addr, hooks) = start();
    let client = gitlab::Gitlab::new_insecure(&addr, ROOT_TOKEN).expect("client connects and checks /user");

    let project: Project = CreateProject::builder()
        .name("client-demo")
        .initialize_with_readme(true)
        .build()
        .unwrap()
        .query(&client)
        .unwrap();
    assert_eq!(project.default_branch, "main");
    let path = project.path_with_namespace.clone();

    // Webhooks for MR and note events.
    let _: Value = AllowLocalWebhooks.query(&client).unwrap();
    let _: Value = api::raw(
        gitlab::api::projects::hooks::CreateHook::builder()
            .project(project.id)
            .url(format!("http://{hook_addr}/hook"))
            .merge_requests_events(true)
            .note_events(true)
            .push_events(true)
            .build()
            .unwrap(),
    )
    .query(&client)
    .map(|b: Vec<u8>| serde_json::from_slice(&b).unwrap())
    .expect("hook created");

    // A commit on a new branch (form-encoded `actions[][...]`).
    let _: Value = CreateCommit::builder()
        .project(project.id)
        .branch("feature")
        .start_branch("main")
        .commit_message("Add app")
        .action(CommitAction::builder().action(CommitActionType::Create).file_path("app.txt").content(&b"one\ntwo\nthree\n"[..]).build().unwrap())
        .build()
        .unwrap()
        .query(&client)
        .unwrap();

    let mr: Mr = CreateMergeRequest::builder()
        .project(&path)
        .source_branch("feature")
        .target_branch("main")
        .title("Draft: Add app")
        .build()
        .unwrap()
        .query(&client)
        .unwrap();
    assert_eq!(mr.iid, 1);
    assert!(mr.draft);

    let listed: Vec<Mr> = MergeRequests::builder().project(&path).state(MergeRequestState::Opened).build().unwrap().query(&client).unwrap();
    assert_eq!(listed.len(), 1);

    // Comment, a diff thread on the new file (form-encoded `position[...]`), and an award.
    let _: Value = CreateMergeRequestNote::builder().project(&path).merge_request(1).body("Looks good").build().unwrap().query(&client).unwrap();
    let mut mr = MergeRequest::builder().project(&path).merge_request(1).build().unwrap().query(&client).unwrap();
    for _ in 0..100 {
        let m: Mr = MergeRequest::builder().project(&path).merge_request(1).build().unwrap().query(&client).unwrap();
        if m.diff_refs.is_some() {
            mr = m;
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let mr: Mr = mr;
    let refs = mr.diff_refs.expect("diff refs");
    let discussion: Discussion = CreateMergeRequestDiscussion::builder()
        .project(&path)
        .merge_request(1)
        .body("Why three?")
        .position(
            Position::builder()
                .base_sha(refs.base_sha.as_str())
                .start_sha(refs.start_sha.as_str())
                .head_sha(refs.head_sha.as_str())
                .text_position(TextPosition::builder().new_path("app.txt").old_path("app.txt").new_line(3).build().unwrap())
                .build()
                .unwrap(),
        )
        .build()
        .unwrap()
        .query(&client)
        .unwrap();
    assert_eq!(discussion.notes[0].kind.as_deref(), Some("DiffNote"));
    assert_eq!(discussion.notes[0].body, "Why three?");
    assert_eq!(discussion.notes[0].position.as_ref().unwrap()["new_line"], json!(3));
    assert_eq!(discussion.id.len(), 40);
    let _: Value = CreateMergeRequestAward::builder().project(&path).merge_request(1).name("thumbsup").build().unwrap().query(&client).unwrap();

    // Ready, approve, merge.
    let edited: Mr = EditMergeRequest::builder().project(&path).merge_request(1).title("Add app").build().unwrap().query(&client).unwrap();
    assert!(!edited.draft);
    let _: Value = ApproveMergeRequest::builder().project(&path).merge_request(1).build().unwrap().query(&client).unwrap();
    let ready = wait_mergeable(&client, &path, 1);
    assert_eq!(ready.title, "Add app");
    let merged: Mr = MergeMergeRequest::builder().project(&path).merge_request(1).build().unwrap().query(&client).unwrap();
    assert_eq!(merged.state, "merged");
    assert!(merged.merge_commit_sha.is_some());

    // The webhook payloads are recognized by the crate's hook types.
    std::thread::sleep(Duration::from_millis(500));
    let got = hooks.lock().unwrap().clone();
    let kinds: Vec<Option<HookKind>> = got.iter().map(HookKind::kind_of).collect();
    assert!(kinds.contains(&Some(HookKind::Web(Some(WebHookType::MergeRequest)))), "{kinds:?}");
    assert!(kinds.contains(&Some(HookKind::Web(Some(WebHookType::Note)))), "{kinds:?}");
    assert!(kinds.contains(&Some(HookKind::Web(Some(WebHookType::Push)))), "{kinds:?}");
    for h in &got {
        if h["object_kind"] == "merge_request" {
            let created = h["object_attributes"]["created_at"].clone();
            let _: HookDate = serde_json::from_value(created).expect("hook dates parse");
        }
    }
}
