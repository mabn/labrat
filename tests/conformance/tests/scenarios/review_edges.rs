//! Edge cases from `docs/gitlab-ref/edge-cases-reviews.md`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

struct F0 {
    pid: i64,
    mr: String,
    refs: Value,
    dev: (i64, String),
    dev2: (i64, String),
    rep: (i64, String),
    guest: (i64, String),
    out: (i64, String),
}

async fn user_with_token(s: &mut Scenario, base: &str) -> (i64, String) {
    let admin = s.admin();
    let username = s.name(base);
    let u = s
        .step(
            &format!("create user {base}"),
            POST,
            "/users",
            Some(&admin),
            json!({"username": username, "name": format!("User {base}"), "email": format!("{username}@example.com"),
                   "password": "Password-123-labrat", "skip_confirmation": true}),
            "user",
        )
        .await;
    let id = u.id();
    let t = s
        .step(&format!("token {base}"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token")
        .await;
    (id, t.get("token").as_str().unwrap().to_string())
}

fn lines(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix} {i}\n")).collect()
}

/// The fixture repository and MR of the notes (section 0): a public project, MR authored by `dev`.
async fn f0(s: &mut Scenario, prefix: &str) -> F0 {
    let admin = s.admin();
    let gpath = s.name(prefix);
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s
        .step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "visibility": "public"}), "project")
        .await;
    let pid = p.id();
    let dev = user_with_token(s, "dev").await;
    let dev2 = user_with_token(s, "dev2").await;
    let rep = user_with_token(s, "rep").await;
    let guest = user_with_token(s, "guest").await;
    let out = user_with_token(s, "out").await;
    for (who, level) in [(dev.0, 30), (dev2.0, 30), (rep.0, 20), (guest.0, 10)] {
        s.step("add member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": who, "access_level": level}), "user").await;
    }
    let commits = format!("/projects/{pid}/repository/commits");
    s.step(
        "main files",
        POST,
        &commits,
        Some(&admin),
        json!({"branch": "main", "commit_message": "Initial files", "actions": [
            {"action": "create", "file_path": "README.md", "content": lines("readme line", 10)},
            {"action": "create", "file_path": "big.txt", "content": lines("line", 100)},
            {"action": "create", "file_path": "old_name.txt", "content": lines("old", 3)},
            {"action": "create", "file_path": "pure_old.txt", "content": lines("pure", 3)},
            {"action": "create", "file_path": "gone.txt", "content": lines("gone", 3)},
            {"action": "create", "file_path": "run.sh", "content": "echo hi\n"},
        ]}),
        "commit",
    )
    .await;
    let mut readme = lines("readme line", 10);
    readme = readme.replace("readme line 5\n", "readme line five\n");
    let big = lines("line", 100).replace("line 50\n", "line fifty\n");
    s.step(
        "feature changes",
        POST,
        &commits,
        Some(&admin),
        json!({"branch": "feature", "start_branch": "main", "commit_message": "Feature changes", "actions": [
            {"action": "update", "file_path": "README.md", "content": readme},
            {"action": "update", "file_path": "big.txt", "content": big},
            {"action": "move", "previous_path": "old_name.txt", "file_path": "new_name.txt", "content": "old 1\nold two\nold 3\n"},
            {"action": "move", "previous_path": "pure_old.txt", "file_path": "pure_new.txt"},
            {"action": "delete", "file_path": "gone.txt"},
            {"action": "create", "file_path": "new.txt", "content": lines("new", 3)},
            {"action": "chmod", "file_path": "run.sh", "execute_filemode": true},
            {"action": "create", "file_path": "logo.png", "encoding": "base64", "content": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="},
        ]}),
        "commit",
    )
    .await;
    // Let push processing finish, or its refresh adds a second diff version to the new MR.
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("create mr", POST, &mrs, Some(&dev.1), json!({"source_branch": "feature", "target_branch": "main", "title": "T"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let v = s.step("versions", GET, &format!("{mr}/versions"), Some(&admin), json!(null), "version").await;
    let v0 = &v.body[0];
    let refs = json!({"base_sha": v0["base_commit_sha"], "start_sha": v0["start_commit_sha"], "head_sha": v0["head_commit_sha"]});
    F0 { pid, mr, refs, dev, dev2, rep, guest, out }
}

fn pos(refs: &Value, extra: Value) -> Value {
    let mut p = refs.clone();
    p["position_type"] = json!("text");
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    p
}

#[tokio::test]
async fn review_positions() {
    let mut s = Scenario::new("review_positions").await;
    let f = f0(&mut s, "rpos").await;
    let admin = s.admin();
    let d = format!("{}/discussions", f.mr);
    let r = &f.refs;
    let cases: Vec<(&str, Value)> = vec![
        ("rename added line", pos(r, json!({"old_path": "old_name.txt", "new_path": "new_name.txt", "new_line": 2}))),
        ("rename context line", pos(r, json!({"old_path": "old_name.txt", "new_path": "new_name.txt", "old_line": 1, "new_line": 1}))),
        ("deleted file old line", pos(r, json!({"old_path": "gone.txt", "new_path": "gone.txt", "old_line": 1}))),
        ("deleted file both lines", pos(r, json!({"old_path": "gone.txt", "new_path": "gone.txt", "old_line": 1, "new_line": 1}))),
        ("added file new line", pos(r, json!({"old_path": "new.txt", "new_path": "new.txt", "new_line": 2}))),
        ("added file both lines", pos(r, json!({"old_path": "new.txt", "new_path": "new.txt", "old_line": 2, "new_line": 2}))),
        ("unchanged outside hunk", pos(r, json!({"old_path": "big.txt", "new_path": "big.txt", "old_line": 20, "new_line": 20}))),
        ("outside hunk new only", pos(r, json!({"old_path": "big.txt", "new_path": "big.txt", "new_line": 20}))),
        ("outside hunk offset", pos(r, json!({"old_path": "big.txt", "new_path": "big.txt", "old_line": 20, "new_line": 21}))),
        ("after hunk", pos(r, json!({"old_path": "big.txt", "new_path": "big.txt", "old_line": 80, "new_line": 80}))),
        ("beyond eof", pos(r, json!({"old_path": "big.txt", "new_path": "big.txt", "old_line": 101, "new_line": 101}))),
        ("pure rename text", pos(r, json!({"old_path": "pure_old.txt", "new_path": "pure_new.txt", "old_line": 1, "new_line": 1}))),
        ("mode only text", pos(r, json!({"old_path": "run.sh", "new_path": "run.sh", "old_line": 1, "new_line": 1}))),
        ("wrong case path", pos(r, json!({"old_path": "readme.md", "new_path": "readme.md", "new_line": 5}))),
        ("missing head sha", json!({"base_sha": r["base_sha"], "start_sha": r["start_sha"], "position_type": "text", "new_path": "new.txt", "new_line": 1})),
    ];
    for (label, p) in cases {
        s.step(label, POST, &d, Some(&admin), json!({"body": label, "position": p}), "discussion").await;
    }
    let mut file = r.clone();
    file["position_type"] = json!("file");
    file["old_path"] = json!("pure_old.txt");
    file["new_path"] = json!("pure_new.txt");
    s.step("file position", POST, &d, Some(&admin), json!({"body": "file", "position": file}), "discussion").await;
    let mut file1 = r.clone();
    file1["position_type"] = json!("file");
    file1["new_path"] = json!("pure_new.txt");
    s.step("file position one path", POST, &d, Some(&admin), json!({"body": "file1", "position": file1}), "discussion").await;
    let mut nowhere = r.clone();
    nowhere["position_type"] = json!("file");
    nowhere["old_path"] = json!("nope/zzz.txt");
    nowhere["new_path"] = json!("nope/zzz.txt");
    s.step("file position nonexistent", POST, &d, Some(&admin), json!({"body": "nowhere", "position": nowhere}), "discussion").await;
    let mut image = r.clone();
    image["position_type"] = json!("image");
    image["old_path"] = json!("logo.png");
    image["new_path"] = json!("logo.png");
    image["x"] = json!(0);
    image["y"] = json!(20);
    image["width"] = json!(100);
    image["height"] = json!(50);
    s.step("image position", POST, &d, Some(&admin), json!({"body": "image", "position": image.clone()}), "discussion").await;
    image.as_object_mut().unwrap().remove("height");
    s.step("image incomplete", POST, &d, Some(&admin), json!({"body": "image", "position": image}), "discussion").await;
    let ranged = pos(r, json!({"old_path": "new.txt", "new_path": "new.txt", "new_line": 3,
        "line_range": {"start": {"line_code": "zzz", "type": "new", "new_line": 99}, "end": {"line_code": "zzz", "type": "new", "new_line": 3}}}));
    s.step("bogus line range", POST, &d, Some(&admin), json!({"body": "range", "position": ranged}), "discussion").await;
    let dup = pos(r, json!({"old_path": "new.txt", "new_path": "new.txt", "new_line": 2}));
    s.step("duplicate thread", POST, &d, Some(&admin), json!({"body": "again", "position": dup}), "discussion").await;
    s.step("all discussions", GET, &d, Some(&admin), json!({"per_page": 100}), "discussion").await;
    s.finish();
}

#[tokio::test]
async fn review_discussions() {
    let mut s = Scenario::new("review_discussions").await;
    let f = f0(&mut s, "rdisc").await;
    let admin = s.admin();
    let notes = format!("{}/notes", f.mr);
    let disc = format!("{}/discussions", f.mr);
    let dev_t = f.dev.1.clone();

    // B1: replying converts an individual note into a thread.
    let n1 = s.step("individual note", POST, &notes, Some(&dev_t), json!({"body": "a"}), "note").await;
    let d1 = n1.get("noteable_id").clone();
    let _ = d1;
    let list = s.step("discussions", GET, &disc, Some(&dev_t), json!(null), "discussion").await;
    let did = list.body[0]["id"].as_str().unwrap().to_string();
    s.step("reply", POST, &format!("{disc}/{did}/notes"), Some(&dev_t), json!({"body": "b"}), "note").await;
    s.step("converted note", GET, &format!("{notes}/{}", n1.id()), Some(&dev_t), json!(null), "note").await;
    s.step("reply to note id", POST, &format!("{disc}/{}/notes", n1.id()), Some(&dev_t), json!({"body": "c"}), "note").await;

    // B3: partial resolution and parameter errors.
    let thread = s.step("thread", GET, &format!("{disc}/{did}"), Some(&dev_t), json!(null), "discussion").await;
    let n2 = thread.body["notes"][1]["id"].as_i64().unwrap();
    s.step("resolve reply note", PUT, &format!("{disc}/{did}/notes/{n2}"), Some(&dev_t), json!({"resolved": true}), "note").await;
    s.step("thread partially resolved", GET, &format!("{disc}/{did}"), Some(&dev_t), json!(null), "discussion").await;
    s.step("body and resolved", PUT, &format!("{disc}/{did}/notes/{n2}"), Some(&dev_t), json!({"resolved": true, "body": "x"}), "note").await;
    s.step("neither", PUT, &format!("{disc}/{did}/notes/{n2}"), Some(&dev_t), json!({}), "note").await;
    // B4: resolving twice adds the note twice.
    s.step("resolve thread", PUT, &format!("{disc}/{did}"), Some(&dev_t), json!({"resolved": true}), "discussion").await;
    s.step("resolve thread again", PUT, &format!("{disc}/{did}"), Some(&dev_t), json!({"resolved": true}), "discussion").await;
    // B6: reply to a resolved thread is created resolved.
    s.step("reply resolved", POST, &format!("{disc}/{did}/notes"), Some(&dev_t), json!({"body": "more"}), "note").await;
    // B3: individual note resolution errors.
    let solo = s.step("solo note", POST, &notes, Some(&dev_t), json!({"body": "solo"}), "note").await;
    let solo_list = s.step("discussions 2", GET, &disc, Some(&dev_t), json!(null), "discussion").await;
    let solo_did = solo_list.body.as_array().unwrap().iter().find(|d| d["notes"][0]["id"].as_i64() == Some(solo.id())).unwrap()["id"].as_str().unwrap().to_string();
    s.step("resolve individual note", PUT, &format!("{disc}/{solo_did}/notes/{}", solo.id()), Some(&dev_t), json!({"resolved": true}), "note").await;
    s.step("resolve individual discussion", PUT, &format!("{disc}/{solo_did}"), Some(&dev_t), json!({"resolved": true}), "discussion").await;
    // B5: deleting the open note leaves the thread resolved silently.
    let t2 = s.step("new thread", POST, &disc, Some(&dev_t), json!({"body": "t2"}), "discussion").await;
    let t2id = t2.get("id").as_str().unwrap().to_string();
    let t2n1 = t2.body["notes"][0]["id"].as_i64().unwrap();
    s.step("resolve first", PUT, &format!("{disc}/{t2id}/notes/{t2n1}"), Some(&dev_t), json!({"resolved": true}), "note").await;
    let r2 = s.step("open reply", POST, &format!("{disc}/{t2id}/notes"), Some(&admin), json!({"body": "open"}), "note").await;
    s.step("unresolve reply", PUT, &format!("{disc}/{t2id}/notes/{}", r2.id()), Some(&admin), json!({"resolved": false}), "note").await;
    s.step("delete open reply", DELETE, &format!("{disc}/{t2id}/notes/{}", r2.id()), Some(&admin), json!(null), "note").await;
    s.step("thread after delete", GET, &format!("{disc}/{t2id}"), Some(&dev_t), json!(null), "discussion").await;

    // B8-B10: quick actions in replies and edits.
    s.step("approve in reply", POST, &format!("{disc}/{did}/notes"), Some(&f.dev2.1), json!({"body": "/approve"}), "note").await;
    s.settle().await;
    s.step("approve again", POST, &notes, Some(&f.dev2.1), json!({"body": "/approve"}), "note").await;
    s.settle().await;
    s.step("close closed", POST, &notes, Some(&admin), json!({"body": "/unapprove"}), "note").await;
    let h = s.step("hello", POST, &notes, Some(&dev_t), json!({"body": "hello"}), "note").await;
    s.step("edit adds lock", PUT, &format!("{notes}/{}", h.id()), Some(&dev_t), json!({"body": "hello\n/lock"}), "note").await;
    s.step("edit to bare command", PUT, &format!("{notes}/{}", h.id()), Some(&dev_t), json!({"body": "/unlock"}), "note").await;
    s.step("edited note gone", GET, &format!("{notes}/{}", h.id()), Some(&dev_t), json!(null), "note").await;
    s.step("edit blank", PUT, &format!("{notes}/{}", n1.id()), Some(&dev_t), json!({"body": ""}), "note").await;
    s.settle().await;

    // B12: locked MR by role.
    s.step("lock", PUT, &f.mr, Some(&admin), json!({"discussion_locked": true}), "mr").await;
    s.step("outsider note locked", POST, &notes, Some(&f.out.1), json!({"body": "x"}), "note").await;
    s.step("outsider reply locked", POST, &format!("{disc}/{did}/notes"), Some(&f.out.1), json!({"body": "x"}), "note").await;
    s.step("outsider award locked", POST, &format!("{}/award_emoji", f.mr), Some(&f.out.1), json!({"name": "thumbsup"}), "award").await;
    s.step("guest note locked", POST, &notes, Some(&f.guest.1), json!({"body": "guest"}), "note").await;
    s.step("unlock", PUT, &f.mr, Some(&admin), json!({"discussion_locked": false}), "mr").await;

    // B7: internal notes.
    let internal = s.step("internal note", POST, &notes, Some(&f.rep.1), json!({"body": "secret", "internal": true}), "note").await;
    let all = s.step("discussions as reporter", GET, &disc, Some(&f.rep.1), json!({"per_page": 100}), "discussion").await;
    let idid = all.body.as_array().unwrap().iter().find(|d| d["notes"][0]["id"].as_i64() == Some(internal.id())).unwrap()["id"].as_str().unwrap().to_string();
    s.step("internal reply", POST, &format!("{disc}/{idid}/notes"), Some(&f.rep.1), json!({"body": "reply"}), "note").await;
    s.step("guest discussions", GET, &disc, Some(&f.guest.1), json!({"per_page": 100}), "discussion").await;
    s.step("guest internal thread", GET, &format!("{disc}/{idid}"), Some(&f.guest.1), json!(null), "discussion").await;
    s.step("guest reply internal", POST, &format!("{disc}/{idid}/notes"), Some(&f.guest.1), json!({"body": "x"}), "note").await;
    s.step("guest internal flag", POST, &notes, Some(&f.guest.1), json!({"body": "g", "internal": true}), "note").await;

    // E: award emoji.
    let awards = format!("{}/award_emoji", f.mr);
    s.step("alias thumbs_up", POST, &awards, Some(&dev_t), json!({"name": "thumbs_up"}), "award").await;
    s.step("alias +1 duplicate", POST, &awards, Some(&dev_t), json!({"name": "+1"}), "award").await;
    s.step("upper case", POST, &awards, Some(&f.dev2.1), json!({"name": "THUMBSUP"}), "award").await;
    let a = s.step("colons", POST, &awards, Some(&f.dev2.1), json!({"name": ":thumbsup:"}), "award").await;
    s.step("delete other user's", DELETE, &format!("{awards}/{}", a.id()), Some(&dev_t), json!(null), "award").await;
    s.step("delete own", DELETE, &format!("{awards}/{}", a.id()), Some(&f.dev2.1), json!(null), "award").await;
    s.step("delete again", DELETE, &format!("{awards}/{}", a.id()), Some(&f.dev2.1), json!(null), "award").await;
    s.step("award again", POST, &awards, Some(&f.dev2.1), json!({"name": "thumbsup"}), "award").await;
    let sys = s.step("notes", GET, &notes, Some(&admin), json!({"per_page": 100, "sort": "asc"}), "note").await;
    let sys_id = sys.body.as_array().unwrap().iter().find(|n| n["system"] == true).unwrap()["id"].as_i64().unwrap();
    s.step("award system note", POST, &format!("{notes}/{sys_id}/award_emoji"), Some(&dev_t), json!({"name": "tada"}), "award").await;
    s.step("award internal as guest", POST, &format!("{notes}/{}/award_emoji", internal.id()), Some(&f.guest.1), json!({"name": "tada"}), "award").await;
    s.step("award unknown note", POST, &format!("{notes}/999999/award_emoji"), Some(&dev_t), json!({"name": "tada"}), "award").await;
    s.step("react toggle on", POST, &notes, Some(&dev_t), json!({"body": "/react :rocket:"}), "note").await;
    s.step("react toggle off", POST, &notes, Some(&dev_t), json!({"body": "/react :rocket:"}), "note").await;
    s.step("mr awards", GET, &awards, Some(&admin), json!(null), "award").await;
    s.settle().await;
    s.step("final discussions", GET, &disc, Some(&admin), json!({"per_page": 100}), "discussion").await;
    s.finish();
}

#[tokio::test]
async fn review_drafts_approvals() {
    let mut s = Scenario::new("review_drafts_approvals").await;
    let f = f0(&mut s, "rdraft").await;
    let admin = s.admin();
    let mr = f.mr.clone();
    let drafts = format!("{mr}/draft_notes");
    let disc = format!("{mr}/discussions");
    let notes = format!("{mr}/notes");
    let d2 = f.dev2.1.clone();
    s.step("dev2 reviewer", PUT, &mr, Some(&admin), json!({"reviewer_ids": [f.dev2.0]}), "mr").await;

    // C1: reviewer state follows drafts.
    let dr = s.step("first draft", POST, &drafts, Some(&d2), json!({"note": "a"}), "draft").await;
    s.step("reviewers review_started", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;
    s.step("delete draft", DELETE, &format!("{drafts}/{}", dr.id()), Some(&d2), json!(null), "draft").await;
    s.step("reviewers unreviewed", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;

    // C2: publish ordering with replies and threads.
    let t = s.step("existing thread", POST, &disc, Some(&f.dev.1), json!({"body": "existing"}), "discussion").await;
    let tid = t.get("id").as_str().unwrap().to_string();
    let p1 = pos(&f.refs, json!({"old_path": "new.txt", "new_path": "new.txt", "new_line": 1}));
    s.step("draft reply", POST, &drafts, Some(&d2), json!({"note": "r1", "in_reply_to_discussion_id": tid}), "draft").await;
    s.step("draft plain", POST, &drafts, Some(&d2), json!({"note": "t2"}), "draft").await;
    s.step("second reply same thread", POST, &drafts, Some(&d2), json!({"note": "r2", "in_reply_to_discussion_id": tid}), "draft").await;
    s.step("blank draft", POST, &drafts, Some(&d2), json!({"note": ""}), "draft").await;
    // Created last: GitLab lists drafts without ORDER BY, and a positioned draft created earlier
    // was observed to come back last.
    s.step("draft thread on line", POST, &drafts, Some(&d2), json!({"note": "t1", "position": p1}), "draft").await;
    s.step("list drafts", GET, &drafts, Some(&d2), json!(null), "draft").await;
    s.step("bulk publish", POST, &format!("{drafts}/bulk_publish"), Some(&d2), json!(null), "draft").await;
    s.settle().await;
    s.step("discussions after publish", GET, &disc, Some(&admin), json!({"per_page": 100}), "discussion").await;
    s.step("reviewers after publish", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;

    // C3: draft reply to a deleted thread vanishes.
    let gone = s.step("thread to delete", POST, &disc, Some(&f.dev.1), json!({"body": "doomed"}), "discussion").await;
    let gid = gone.get("id").as_str().unwrap().to_string();
    let gnote = gone.body["notes"][0]["id"].as_i64().unwrap();
    s.step("draft reply to doomed", POST, &drafts, Some(&d2), json!({"note": "late", "in_reply_to_discussion_id": gid}), "draft").await;
    s.step("delete doomed note", DELETE, &format!("{disc}/{gid}/notes/{gnote}"), Some(&f.dev.1), json!(null), "note").await;
    s.step("draft reply after delete", POST, &drafts, Some(&d2), json!({"note": "later", "in_reply_to_discussion_id": gid}), "draft").await;
    s.step("publish vanishing", POST, &format!("{drafts}/bulk_publish"), Some(&d2), json!(null), "draft").await;
    s.step("drafts empty", GET, &drafts, Some(&d2), json!(null), "draft").await;

    // C6: resolve_discussion on publish.
    s.step("draft resolving reply", POST, &drafts, Some(&d2), json!({"note": "fixed", "in_reply_to_discussion_id": tid, "resolve_discussion": true}), "draft").await;
    s.step("draft resolve without thread", POST, &drafts, Some(&d2), json!({"note": "x", "resolve_discussion": true}), "draft").await;
    s.step("publish resolving", POST, &format!("{drafts}/bulk_publish"), Some(&d2), json!(null), "draft").await;
    s.settle().await;
    s.step("thread resolved by publish", GET, &format!("{disc}/{tid}"), Some(&admin), json!(null), "discussion").await;

    // C10: draft update rules.
    let pd = s.step("positioned draft", POST, &drafts, Some(&d2), json!({"note": "p", "position": pos(&f.refs, json!({"old_path": "new.txt", "new_path": "new.txt", "new_line": 2}))}), "draft").await;
    s.step("update without note", PUT, &format!("{drafts}/{}", pd.id()), Some(&d2), json!({"position": pos(&f.refs, json!({"old_path": "new.txt", "new_path": "new.txt", "new_line": 3}))}), "draft").await;
    s.step("update note drops position", PUT, &format!("{drafts}/{}", pd.id()), Some(&d2), json!({"note": "y"}), "draft").await;

    // C7, C8: quick actions in drafts and /submit_review.
    s.step("draft approve", POST, &drafts, Some(&d2), json!({"note": "LGTM\n/approve"}), "draft").await;
    s.step("submit review", POST, &notes, Some(&d2), json!({"body": "/submit_review"}), "note").await;
    s.settle().await;
    s.settle().await;
    s.step("approvals after review", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("reviewers after review", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;
    // C9: non-reviewer submit_review requested_changes.
    s.step("non-reviewer requests changes", POST, &notes, Some(&f.dev.1), json!({"body": "/submit_review requested_changes"}), "note").await;
    s.step("submit approve bogus", POST, &notes, Some(&f.dev.1), json!({"body": "/submit_review bogus"}), "note").await;

    // F: approvals.
    s.step("approve again rest", POST, &format!("{mr}/approve"), Some(&d2), json!(null), "approval").await;
    s.settle().await;
    s.step("approve stale sha", POST, &format!("{mr}/approve"), Some(&f.dev.1), json!({"sha": "0000000000000000000000000000000000000000"}), "approval").await;
    s.step("unapprove without approval", POST, &format!("{mr}/unapprove"), Some(&f.dev.1), json!(null), "approval").await;
    s.step("unapprove quick action without approval", POST, &notes, Some(&f.dev.1), json!({"body": "/unapprove"}), "note").await;
    s.step("author approves", POST, &format!("{mr}/approve"), Some(&f.dev.1), json!(null), "approval").await;
    s.settle().await;
    s.step("reporter approves", POST, &format!("{mr}/approve"), Some(&f.rep.1), json!(null), "approval").await;
    s.settle().await;
    // F4: re-request review removes the approval.
    s.step("re-request approved reviewer", POST, &notes, Some(&admin), json!({"body": format!("/request_review @{}", s.name("dev2"))}), "note").await;
    s.settle().await;
    s.step("approvals after re-request", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("reviewers after re-request", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;
    // F5: a second reviewer via quick action is ignored.
    s.step("request second reviewer", POST, &notes, Some(&admin), json!({"body": format!("/request_review @{}", s.name("rep"))}), "note").await;
    s.step("reviewers single", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;
    s.step("two reviewers keeps first", PUT, &mr, Some(&admin), json!({"reviewer_ids": [f.rep.0, f.dev2.0]}), "mr").await;
    // F6: removed approving reviewer keeps the approval and returns approved.
    s.step("dev2 reviewer again", PUT, &mr, Some(&admin), json!({"reviewer_ids": [f.dev2.0]}), "mr").await;
    s.step("dev2 approves", POST, &format!("{mr}/approve"), Some(&d2), json!(null), "approval").await;
    s.settle().await;
    s.step("remove reviewers", PUT, &mr, Some(&admin), json!({"reviewer_ids": [0]}), "mr").await;
    s.step("approvals kept", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("re-add reviewer", PUT, &mr, Some(&admin), json!({"reviewer_ids": [f.dev2.0]}), "mr").await;
    s.step("reviewer approved again", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;
    // F7: request changes after approving.
    s.step("request changes", POST, &notes, Some(&d2), json!({"body": "/request_changes"}), "note").await;
    s.step("request changes twice", POST, &notes, Some(&d2), json!({"body": "/request_changes"}), "note").await;
    s.step("non-reviewer request changes", POST, &notes, Some(&f.rep.1), json!({"body": "/request_changes"}), "note").await;
    s.settle().await;
    s.step("approvals after changes", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("reviewers after changes", GET, &format!("{mr}/reviewers"), Some(&admin), json!(null), "reviewer").await;
    // F2: approve a closed MR.
    s.step("close", PUT, &mr, Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("approve closed", POST, &format!("{mr}/approve"), Some(&d2), json!(null), "approval").await;
    s.settle().await;
    s.settle().await;
    s.step("notes at end", GET, &notes, Some(&admin), json!({"per_page": 100, "sort": "asc"}), "note").await;
    s.finish();
}

#[tokio::test]
async fn review_suggestions() {
    let mut s = Scenario::new("review_suggestions").await;
    let f = f0(&mut s, "rsug").await;
    let admin = s.admin();
    let disc = format!("{}/discussions", f.mr);
    let sugg = |body: &str, line: i64| -> Value {
        json!({"body": body, "position": pos(&f.refs, json!({"old_path": "big.txt", "new_path": "big.txt", "old_line": line, "new_line": line}))})
    };
    let suggestion_ids = |v: &Value| -> Vec<i64> {
        v["notes"][0]["suggestions"].as_array().map(|a| a.iter().filter_map(|x| x["id"].as_i64()).collect()).unwrap_or_default()
    };
    let s1 = s.step("suggestion clamped above", POST, &disc, Some(&admin), sugg("```suggestion:-5+0\nTOP\n```", 2), "discussion").await;
    let _ = s1;
    s.step("suggestion past eof", POST, &disc, Some(&admin), json!({"body": "```suggestion:+9\nEND\n```",
        "position": pos(&f.refs, json!({"old_path": "big.txt", "new_path": "big.txt", "old_line": 98, "new_line": 98}))}), "discussion").await;
    let empty = s.step("empty suggestion", POST, &disc, Some(&admin), sugg("```suggestion\n\n```", 10), "discussion").await;
    let same = s.step("identical suggestion", POST, &disc, Some(&admin), sugg("```suggestion\nline 12\n```", 12), "discussion").await;
    let a = s.step("suggestion line 20", POST, &disc, Some(&admin), sugg("```suggestion\nTWENTY\n```", 20), "discussion").await;
    let b = s.step("suggestion line 30", POST, &disc, Some(&admin), sugg("```suggestion:-1+1\nTHIRTY\n```", 30), "discussion").await;
    let c = s.step("suggestion line 31", POST, &disc, Some(&admin), sugg("```suggestion\nTHIRTY-ONE\n```", 31), "discussion").await;
    let ctx = s.step("plain note", POST, &format!("{}/notes", f.mr), Some(&admin), json!({"body": "```suggestion\nx\n```"}), "note").await;
    let _ = ctx;
    let sid = |v: &Value| *suggestion_ids(v).first().unwrap_or_else(|| panic!("no suggestion in {v}"));
    s.step("apply identical", PUT, &format!("/suggestions/{}/apply", sid(&same.body)), Some(&admin), json!(null), "suggestion").await;
    s.step("apply by reporter", PUT, &format!("/suggestions/{}/apply", sid(&a.body)), Some(&f.rep.1), json!(null), "suggestion").await;
    s.step("batch overlap", PUT, "/suggestions/batch_apply", Some(&admin), json!({"ids": [sid(&b.body), sid(&c.body)]}), "suggestion").await;
    s.step("batch unknown", PUT, "/suggestions/batch_apply", Some(&admin), json!({"ids": [sid(&a.body), 999999]}), "suggestion").await;
    s.step(
        "batch apply",
        PUT,
        "/suggestions/batch_apply",
        Some(&admin),
        json!({"ids": [sid(&a.body), sid(&empty.body)], "commit_message": "Fix %{file_paths} by %{username} on %{branch_name} (%{suggestions_count}/%{files_count}) %{project_path} %{project_name} %{user_full_name} %{foo}"}),
        "suggestion",
    )
    .await;
    s.settle().await;
    s.step("file after batch", GET, &format!("/projects/{}/repository/files/big.txt/raw", f.pid), Some(&admin), json!({"ref": "feature"}), "file").await;
    s.step("commit after batch", GET, &format!("/projects/{}/repository/commits/feature", f.pid), Some(&admin), json!(null), "commit").await;
    s.step("apply stale", PUT, &format!("/suggestions/{}/apply", sid(&b.body)), Some(&admin), json!(null), "suggestion").await;
    s.settle().await;
    s.step("apply applied", PUT, &format!("/suggestions/{}/apply", sid(&a.body)), Some(&admin), json!(null), "suggestion").await;
    s.settle().await;
    // D6: editing a note recreates its suggestions.
    let note_id = c.body["notes"][0]["id"].as_i64().unwrap();
    let cdid = c.get("id").as_str().unwrap().to_string();
    s.step("edit suggestion note", PUT, &format!("{disc}/{cdid}/notes/{note_id}"), Some(&admin), json!({"body": "```suggestion\nTHIRTY-ONE\n```"}), "note").await;
    s.step("apply old id", PUT, &format!("/suggestions/{}/apply", sid(&c.body)), Some(&admin), json!(null), "suggestion").await;
    s.step("discussions", GET, &disc, Some(&admin), json!({"per_page": 100}), "discussion").await;
    s.finish();
}
