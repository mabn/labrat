//! Error outcomes of the GraphQL merge request, note, discussion, emoji and to-do mutations
//! (docs/gitlab-ref/graphql-mr-error-coverage.md): unknown and unreadable targets, wrong GlobalID
//! types, Reporter and outsider callers, validation failures and closed or merged MRs.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn graphql_mutation_errors() {
    let mut s = Scenario::new("graphql_mutation_errors").await;
    let admin = s.admin();
    let gpath = s.name("gme");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut people: Vec<(String, i64, String)> = vec![];
    for (who, level) in [("rep", 20), ("out", 0)] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        if level > 0 {
            s.step(&format!("{who} member"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": level}), "user").await;
        }
        people.push((username, id, tok));
    }
    let (rep_name, rep_id, rep) = people[0].clone();
    let (out_name, out_id, out) = people[1].clone();
    let _ = (rep_name, out_id);
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for b in ["a", "b", "c"] {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "one\ntwo\n"}]}), "commit").await;
    }
    s.settle().await;
    for (i, b) in ["a", "b", "c"].iter().enumerate() {
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    let m1 = s.send(GET, &format!("{mrs}/1"), Some(&admin), json!(null)).await;
    let mr1_gid = format!("gid://gitlab/MergeRequest/{}", m1.body["id"]);
    let head = m1.body["sha"].as_str().unwrap_or_default().to_string();
    let refs = m1.body["diff_refs"].clone();
    let m2 = s.send(GET, &format!("{mrs}/2"), Some(&admin), json!(null)).await;
    let m2_sha = m2.body["sha"].as_str().unwrap_or_default().to_string();
    let m3 = s.send(GET, &format!("{mrs}/3"), Some(&admin), json!(null)).await;
    let m3_sha = m3.body["sha"].as_str().unwrap_or_default().to_string();
    s.step("merge 2", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!(null), "mr").await;
    s.step("close 3", PUT, &format!("{mrs}/3"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.settle().await;
    // A system note (from the merge) and a user note to work on.
    let notes2 = s.send(GET, &format!("{mrs}/2/notes"), Some(&admin), json!({"sort": "asc"})).await;
    let sys_note = notes2.body.as_array().and_then(|a| a.iter().find(|n| n["system"] == true)).map(|n| n["id"].clone()).unwrap_or(Value::Null);
    let disc2 = s.send(GET, &format!("{mrs}/2/discussions"), Some(&admin), json!(null)).await;
    let sys_disc = disc2.body.as_array().and_then(|a| a.iter().find(|d| d["notes"][0]["system"] == true)).map(|d| d["id"].as_str().unwrap_or_default().to_string()).unwrap_or_default();
    let admin_note = s.step("admin note", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "By admin"}), "note").await.id();
    let note_gid = format!("gid://gitlab/Note/{admin_note}");
    let sys_gid = format!("gid://gitlab/Note/{sys_note}");
    let project_gid = format!("gid://gitlab/Project/{pid}");
    let missing_mr = "gid://gitlab/MergeRequest/999999999";

    let mr_mut = |name: &str, args: &str, sel: &str| format!("mutation($p: ID!, $i: String!) {{ {name}(input: {{projectPath: $p, iid: $i{args}}}) {{ errors {sel} }} }}");
    let pv = |iid: &str| json!({"p": full, "i": iid});
    let pvx = |iid: &str, extra: Value| {
        let mut v = json!({"p": full, "i": iid});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };

    // A1: unknown MR and project.
    s.graphql("draft unknown iid", Some(&admin), &mr_mut("mergeRequestSetDraft", ", draft: true", "mergeRequest { draft }"), pv("999")).await;
    s.graphql("update unknown project", Some(&admin), &mr_mut("mergeRequestUpdate", ", title: \"x\"", "mergeRequest { title }"), json!({"p": "nope/nope", "i": "1"})).await;
    // A2: a Reporter.
    s.graphql("reporter update", Some(&rep), &mr_mut("mergeRequestUpdate", ", title: \"x\"", "mergeRequest { title }"), pv("1")).await;
    s.graphql("reporter assignees", Some(&rep), &mr_mut("mergeRequestSetAssignees", ", assigneeUsernames: [\"root\"]", "mergeRequest { iid }"), pv("1")).await;
    s.graphql("reporter accept", Some(&rep), "mutation($p: ID!, $i: String!, $s: String!) { mergeRequestAccept(input: {projectPath: $p, iid: $i, sha: $s}) { errors mergeRequest { state } } }", pvx("1", json!({"s": head}))).await;
    s.graphql("reporter rereview", Some(&rep), "mutation($p: ID!, $i: String!, $u: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $p, iid: $i, userId: $u}) { errors } }", pvx("1", json!({"u": format!("gid://gitlab/User/{rep_id}")}))).await;
    s.graphql("reporter subscription", Some(&rep), &mr_mut("mergeRequestSetSubscription", ", subscribedState: true", "mergeRequest { subscribed }"), pv("1")).await;
    // A3, A4: accept on merged and closed MRs, and without a sha.
    let accept = "mutation($p: ID!, $i: String!, $s: String!) { mergeRequestAccept(input: {projectPath: $p, iid: $i, sha: $s}) { errors mergeRequest { state } } }";
    s.graphql("accept merged", Some(&admin), accept, pvx("2", json!({"s": m2_sha}))).await;
    s.graphql("accept closed", Some(&admin), accept, pvx("3", json!({"s": m3_sha}))).await;
    s.graphql("accept without sha", Some(&admin), &mr_mut("mergeRequestAccept", "", "mergeRequest { state }"), pv("1")).await;
    // A5: people who can't read the MR, and unknown usernames.
    s.graphql("reviewer outsider", Some(&admin), &mr_mut("mergeRequestSetReviewers", &format!(", reviewerUsernames: [\"{out_name}\"]"), "mergeRequest { reviewers { nodes { username } } }"), pv("1")).await;
    s.graphql("assignee outsider", Some(&admin), &mr_mut("mergeRequestSetAssignees", &format!(", assigneeUsernames: [\"{out_name}\"]"), "mergeRequest { assignees { nodes { username } } }"), pv("1")).await;
    s.graphql("reviewer unknown", Some(&admin), &mr_mut("mergeRequestSetReviewers", ", reviewerUsernames: [\"no-such-user-x\"]", "mergeRequest { reviewers { nodes { username } } }"), pv("1")).await;
    // A6: request changes on a merged MR, rereview with wrong ids.
    s.graphql("request changes merged", Some(&admin), &mr_mut("mergeRequestRequestChanges", "", "mergeRequest { iid }"), pv("2")).await;
    let rereview = "mutation($p: ID!, $i: String!, $u: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $p, iid: $i, userId: $u}) { errors } }";
    s.graphql("rereview project gid", Some(&admin), rereview, pvx("1", json!({"u": project_gid}))).await;
    s.graphql("rereview missing user", Some(&admin), rereview, pvx("1", json!({"u": "gid://gitlab/User/999999999"}))).await;
    // A7: update validation.
    let update = "mutation($p: ID!, $i: String!, $t: String, $tb: String, $st: MergeRequestNewState) { mergeRequestUpdate(input: {projectPath: $p, iid: $i, title: $t, targetBranch: $tb, state: $st}) { errors mergeRequest { title targetBranch state } } }";
    s.graphql("update long title", Some(&admin), update, pvx("1", json!({"t": "t".repeat(256)}))).await;
    s.graphql("update target is source", Some(&admin), update, pvx("1", json!({"tb": "a"}))).await;
    s.graphql("update bad target name", Some(&admin), update, pvx("1", json!({"tb": "bad..name"}))).await;
    s.graphql("update merged target", Some(&admin), update, pvx("2", json!({"tb": "b"}))).await;
    s.graphql("close merged", Some(&admin), update, pvx("2", json!({"st": "CLOSED"}))).await;
    s.graphql("bad state enum", Some(&admin), update, pvx("1", json!({"st": "MERGE"}))).await;
    // A8: wrong milestone and label ids.
    let milestone = "mutation($p: ID!, $i: String!, $m: MilestoneID) { mergeRequestSetMilestone(input: {projectPath: $p, iid: $i, milestoneId: $m}) { errors mergeRequest { milestone { id } } } }";
    s.graphql("milestone missing", Some(&admin), milestone, pvx("1", json!({"m": "gid://gitlab/Milestone/999999999"}))).await;
    s.graphql("milestone wrong type", Some(&admin), milestone, pvx("1", json!({"m": project_gid}))).await;
    let labels = "mutation($p: ID!, $i: String!, $l: [LabelID!]!) { mergeRequestSetLabels(input: {projectPath: $p, iid: $i, labelIds: $l}) { errors mergeRequest { labels { nodes { title } } } } }";
    s.graphql("labels missing", Some(&admin), labels, pvx("1", json!({"l": ["gid://gitlab/ProjectLabel/999999999"]}))).await;
    s.graphql("labels wrong type", Some(&admin), labels, pvx("1", json!({"l": [project_gid]}))).await;
    // A9: create.
    let create = "mutation($p: ID!, $t: String!, $s: String!, $tb: String!) { mergeRequestCreate(input: {projectPath: $p, title: $t, sourceBranch: $s, targetBranch: $tb}) { errors mergeRequest { iid } } }";
    s.graphql("create same branches", Some(&admin), create, json!({"p": full, "t": "x", "s": "a", "tb": "a"})).await;
    s.graphql("create blank title", Some(&admin), create, json!({"p": full, "t": "", "s": "c", "tb": "main"})).await;
    s.graphql("create duplicate", Some(&admin), create, json!({"p": full, "t": "dup", "s": "a", "tb": "main"})).await;
    s.graphql("create outsider", Some(&out), create, json!({"p": full, "t": "x", "s": "c", "tb": "main"})).await;
    s.graphql("create unknown project", Some(&admin), create, json!({"p": "nope/nope", "t": "x", "s": "c", "tb": "main"})).await;
    // A10: no token.
    s.graphql("anonymous draft", None, &mr_mut("mergeRequestSetDraft", ", draft: true", "mergeRequest { draft }"), pv("1")).await;

    // B1: notes.
    let create_note = "mutation($n: NoteableID!, $b: String!, $d: DiscussionID) { createNote(input: {noteableId: $n, body: $b, discussionId: $d}) { errors note { body } } }";
    s.graphql("note blank", Some(&admin), create_note, json!({"n": mr1_gid, "b": ""})).await;
    s.graphql("note missing mr", Some(&admin), create_note, json!({"n": missing_mr, "b": "x"})).await;
    s.graphql("note project gid", Some(&admin), create_note, json!({"n": project_gid, "b": "x"})).await;
    s.graphql("note missing discussion", Some(&admin), create_note, json!({"n": mr1_gid, "b": "x", "d": "gid://gitlab/Discussion/0123456789abcdef0123456789abcdef01234567"})).await;
    s.graphql("note reply to system", Some(&admin), create_note, json!({"n": format!("gid://gitlab/MergeRequest/{}", m2.body["id"]), "b": "x", "d": format!("gid://gitlab/Discussion/{sys_disc}")})).await;
    s.graphql("note outsider", Some(&out), create_note, json!({"n": mr1_gid, "b": "x"})).await;
    s.graphql("note only bad command", Some(&admin), create_note, json!({"n": mr1_gid, "b": "/target_branch nope"})).await;
    let diff_note = "mutation($n: NoteableID!, $b: String!, $pos: DiffPositionInput!) { createDiffNote(input: {noteableId: $n, body: $b, position: $pos}) { errors note { body } } }";
    let pos = |path: &str, line: Option<i64>| {
        let mut v = json!({"baseSha": refs["base_sha"], "headSha": refs["head_sha"], "startSha": refs["start_sha"], "paths": {"oldPath": path, "newPath": path}});
        if let Some(l) = line {
            v["newLine"] = json!(l);
        }
        v
    };
    s.graphql("diff note no line", Some(&admin), diff_note, json!({"n": mr1_gid, "b": "x", "pos": pos("a.txt", None)})).await;
    s.graphql("diff note unknown file", Some(&admin), diff_note, json!({"n": mr1_gid, "b": "x", "pos": pos("zzz.txt", Some(1))})).await;
    // B2: update and destroy.
    let update_note = "mutation($id: NoteID!, $b: String) { updateNote(input: {id: $id, body: $b}) { errors note { body } } }";
    let destroy_note = "mutation($id: NoteID!) { destroyNote(input: {id: $id}) { errors note { id } } }";
    s.graphql("reporter updates admin note", Some(&rep), update_note, json!({"id": note_gid, "b": "x"})).await;
    s.graphql("update system note", Some(&admin), update_note, json!({"id": sys_gid, "b": "x"})).await;
    s.graphql("update missing note", Some(&admin), update_note, json!({"id": "gid://gitlab/Note/999999999", "b": "x"})).await;
    s.graphql("update blank body", Some(&admin), update_note, json!({"id": note_gid, "b": ""})).await;
    s.graphql("destroy system note", Some(&admin), destroy_note, json!({"id": sys_gid})).await;
    s.graphql("reporter destroys admin note", Some(&rep), destroy_note, json!({"id": note_gid})).await;
    // B3: discussions.
    let toggle = "mutation($id: DiscussionID!, $r: Boolean!) { discussionToggleResolve(input: {id: $id, resolve: $r}) { errors discussion { resolved } } }";
    s.graphql("resolve system discussion", Some(&admin), toggle, json!({"id": format!("gid://gitlab/Discussion/{sys_disc}"), "r": true})).await;
    s.graphql("resolve missing discussion", Some(&admin), toggle, json!({"id": "gid://gitlab/Discussion/0123456789abcdef0123456789abcdef01234567", "r": true})).await;
    let convert = "mutation($id: NoteID!) { noteConvertToThread(input: {id: $id}) { errors note { discussion { resolvable } } } }";
    s.graphql("convert by outsider", Some(&out), convert, json!({"id": note_gid})).await;
    s.graphql("convert missing", Some(&admin), convert, json!({"id": "gid://gitlab/Note/999999999"})).await;
    s.graphql("convert system", Some(&admin), convert, json!({"id": sys_gid})).await;
    // B4: emoji.
    let award = "mutation($id: AwardableID!, $n: String!) { awardEmojiAdd(input: {awardableId: $id, name: $n}) { errors awardEmoji { name } } }";
    s.graphql("award bad name", Some(&admin), award, json!({"id": mr1_gid, "n": "not_an_emoji_x"})).await;
    s.graphql("award system note", Some(&admin), award, json!({"id": sys_gid, "n": "thumbsup"})).await;
    s.graphql("award outsider", Some(&out), award, json!({"id": mr1_gid, "n": "thumbsup"})).await;
    s.graphql("award missing", Some(&admin), award, json!({"id": missing_mr, "n": "thumbsup"})).await;
    s.graphql("award wrong type", Some(&admin), award, json!({"id": project_gid, "n": "thumbsup"})).await;
    s.graphql("award plus one", Some(&admin), award, json!({"id": mr1_gid, "n": "+1"})).await;
    // B5: to-dos.
    let many: Vec<String> = (1..=101).map(|i| format!("gid://gitlab/Todo/{}", 900000000 + i)).collect();
    s.graphql("resolve too many", Some(&admin), "mutation($ids: [TodoID!]!) { todoResolveMany(input: {ids: $ids}) { errors } }", json!({"ids": many})).await;
    s.graphql("done missing todo", Some(&admin), "mutation($id: TodoID!) { todoMarkDone(input: {id: $id}) { errors todo { state } } }", json!({"id": "gid://gitlab/Todo/999999999"})).await;
    let todo_create = "mutation($t: TodoableID!) { todoCreate(input: {targetId: $t}) { errors todo { state } } }";
    s.graphql("todo missing mr", Some(&admin), todo_create, json!({"t": missing_mr})).await;
    s.graphql("todo unreadable mr", Some(&out), todo_create, json!({"t": mr1_gid})).await;
    s.graphql("all done missing target", Some(&admin), "mutation($t: TodoableID) { todosMarkAllDone(input: {targetId: $t}) { errors } }", json!({"t": missing_mr})).await;
    s.step("mr 1 after", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("mr 1 notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}
