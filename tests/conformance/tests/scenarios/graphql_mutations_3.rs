//! More GraphQL mutations around merge requests: `commitCreate`, `labelUpdate` and image diff
//! note updates.

use base64::Engine;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_mutations_3() {
    let mut s = Scenario::new("graphql_mutations_3").await;
    let admin = s.admin();
    let gpath = s.name("m3");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let commit = "mutation($p: ID!, $b: String!, $sb: String, $m: String!, $a: [CommitAction!]!, $e: Boolean) {
        commitCreate(input: {projectPath: $p, branch: $b, startBranch: $sb, message: $m, actions: $a, allowEmpty: $e}) {
          errors content commitPipelinePath commit { title message } } }";
    s.graphql("commit new branch", Some(&admin), commit, json!({"p": full, "b": "feature", "sb": "main", "m": "Add files", "a": [
        {"action": "CREATE", "filePath": "a.txt", "content": "a\n"},
        {"action": "CREATE", "filePath": "s.sh", "content": "echo\n"},
        {"action": "CREATE", "filePath": "img.png", "content": base64::engine::general_purpose::STANDARD.encode([0x89u8, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0]), "encoding": "BASE64"}
    ]})).await;
    s.graphql("commit update move chmod", Some(&admin), commit, json!({"p": full, "b": "feature", "m": "Change files", "a": [
        {"action": "UPDATE", "filePath": "a.txt", "content": "a2\n"},
        {"action": "MOVE", "filePath": "b.txt", "previousPath": "a.txt"},
        {"action": "CHMOD", "filePath": "s.sh", "executeFilemode": true}
    ]})).await;
    s.graphql("commit missing branch", Some(&admin), commit, json!({"p": full, "b": "nope", "m": "x", "a": [{"action": "CREATE", "filePath": "x.txt", "content": "x"}]})).await;
    s.graphql("commit stale last id", Some(&admin), commit, json!({"p": full, "b": "feature", "m": "x", "a": [{"action": "UPDATE", "filePath": "b.txt", "content": "y", "lastCommitId": "0000000000000000000000000000000000000000"}]})).await;
    s.graphql("commit delete missing", Some(&admin), commit, json!({"p": full, "b": "feature", "m": "x", "a": [{"action": "DELETE", "filePath": "missing.txt"}]})).await;
    s.graphql("commit empty", Some(&admin), commit, json!({"p": full, "b": "feature", "m": "Empty", "a": [], "e": true})).await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Files"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let m = s.send(GET, &format!("{mrs}/1"), Some(&admin), json!(null)).await;
    let refs = m.body["diff_refs"].clone();
    let mr_gid = format!("gid://gitlab/MergeRequest/{}", m.body["id"]);
    let pos = |x: i64| json!({"baseSha": refs["base_sha"], "startSha": refs["start_sha"], "headSha": refs["head_sha"], "paths": {"newPath": "img.png", "oldPath": "img.png"}, "x": x, "y": 5, "width": 100, "height": 100});
    let created = s
        .graphql(
            "image note",
            Some(&admin),
            "mutation($id: NoteableID!, $pos: DiffImagePositionInput!) { createImageDiffNote(input: {noteableId: $id, body: \"On the image\", position: $pos}) { errors note { id body position { x y } } } }",
            json!({"id": mr_gid, "pos": pos(10)}),
        )
        .await;
    let note_id = created.body["data"]["createImageDiffNote"]["note"]["id"].as_str().unwrap_or_default().to_string();
    s.graphql(
        "update image note",
        Some(&admin),
        "mutation($id: DiffNoteID!, $pos: UpdateDiffImagePositionInput) { updateImageDiffNote(input: {id: $id, body: \"Moved\", position: $pos}) { errors note { body position { x y width height } } } }",
        json!({"id": note_id, "pos": {"x": 20}}),
    )
    .await;
    s.graphql(
        "reposition image note",
        Some(&admin),
        "mutation($id: DiffNoteID!, $pos: UpdateDiffImagePositionInput!) { repositionImageDiffNote(input: {id: $id, position: $pos}) { errors note { body position { x y } } } }",
        json!({"id": note_id, "pos": {"x": 30, "y": 40}}),
    )
    .await;
    s.graphql(
        "reposition empty",
        Some(&admin),
        "mutation($id: DiffNoteID!) { repositionImageDiffNote(input: {id: $id, position: {}}) { errors note { body } } }",
        json!({"id": note_id}),
    )
    .await;
    let l = s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "old", "color": "#ff0000"}), "label").await;
    s.graphql(
        "archive label",
        Some(&admin),
        "mutation($id: LabelID!) { labelUpdate(input: {id: $id, archived: true}) { errors label { title archived } } }",
        json!({"id": format!("gid://gitlab/ProjectLabel/{}", l.id())}),
    )
    .await;
    s.finish();
}
