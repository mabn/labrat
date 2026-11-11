//! GraphQL mutations around reviews: createLatestDiffNote, image diff notes, labels and timelogs.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_more_mutations() {
    let mut s = Scenario::new("graphql_more_mutations").await;
    let admin = s.admin();
    let gpath = s.name("gm");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    // A 1x1 PNG.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    s.step(
        "commit",
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(&admin),
        json!({"branch": "feature", "start_branch": "main", "commit_message": "Add files", "actions": [
            {"action": "create", "file_path": "a.txt", "content": "one\ntwo\nthree\n"},
            {"action": "create", "file_path": "pic.png", "content": png, "encoding": "base64"}
        ]}),
        "commit",
    )
    .await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add files"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let info = s.graphql("mr", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { id diffRefs { baseSha startSha headSha } } } }", json!({"path": full})).await;
    let mr = info.body["data"]["project"]["mergeRequest"].clone();
    let gid = mr["id"].as_str().unwrap_or_default().to_string();
    let refs = mr["diffRefs"].clone();
    let head = refs["headSha"].as_str().unwrap_or_default().to_string();

    let note_out = "note { body position { filePath newLine oldLine positionType width height x y } discussion { id } } errors";
    let latest = format!("mutation($id: MergeRequestID!, $head: String!, $line: Int) {{ createLatestDiffNote(input: {{noteableId: $id, headSha: $head, filePath: \"a.txt\", newLine: $line, body: \"Latest\"}}) {{ {note_out} }} }}");
    s.graphql("latest diff note", Some(&admin), &latest, json!({"id": gid, "head": head, "line": 2})).await;
    s.graphql("latest diff note stale head", Some(&admin), &latest, json!({"id": gid, "head": "0000000000000000000000000000000000000000", "line": 2})).await;
    s.graphql("latest diff note missing line", Some(&admin), &latest, json!({"id": gid, "head": head, "line": 99})).await;
    let image = format!(
        "mutation($id: NoteableID!, $pos: DiffImagePositionInput!) {{ createImageDiffNote(input: {{noteableId: $id, body: \"On the image\", position: $pos}}) {{ {note_out} }} }}"
    );
    let pos = json!({"baseSha": refs["baseSha"], "startSha": refs["startSha"], "headSha": head, "paths": {"newPath": "pic.png", "oldPath": "pic.png"}, "width": 1, "height": 1, "x": 0, "y": 0});
    let img = s.graphql("image diff note", Some(&admin), &image, json!({"id": gid, "pos": pos})).await;
    s.step("discussions", GET, &format!("{mrs}/1/discussions"), Some(&admin), json!(null), "discussion").await;
    let _ = img;

    let label = "label { title color textColor description } errors";
    let create = |extra: &str| format!("mutation($path: ID!) {{ labelCreate(input: {{projectPath: $path{extra}}}) {{ {label} }} }}");
    s.graphql("label create", Some(&admin), &create(", title: \"bug\", color: \"#ff0000\", description: \"Broken\""), json!({"path": full})).await;
    s.graphql("label create duplicate", Some(&admin), &create(", title: \"bug\""), json!({"path": full})).await;
    s.graphql("label create bad color", Some(&admin), &create(", title: \"x\", color: \"nope\""), json!({"path": full})).await;
    s.graphql("label create default color", Some(&admin), &create(", title: \"plain\""), json!({"path": full})).await;

    let tl = "timelog { timeSpent summary user { username } spentAt } errors";
    let log = s
        .graphql("timelog create", Some(&admin), &format!("mutation($id: IssuableID!) {{ timelogCreate(input: {{issuableId: $id, timeSpent: \"1h 30m\", summary: \"Reviewing\"}}) {{ {tl} }} }}"), json!({"id": gid}))
        .await;
    let _ = log;
    s.graphql("timelog create invalid", Some(&admin), &format!("mutation($id: IssuableID!) {{ timelogCreate(input: {{issuableId: $id, timeSpent: \"soon\", summary: \"x\"}}) {{ {tl} }} }}"), json!({"id": gid})).await;
    let logs = s.graphql("timelogs", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { totalTimeSpent timelogs { nodes { id timeSpent summary } } } } }", json!({"path": full})).await;
    let log_id = logs.body["data"]["project"]["mergeRequest"]["timelogs"]["nodes"][0]["id"].as_str().unwrap_or_default().to_string();
    s.graphql("timelog delete", Some(&admin), &format!("mutation($id: TimelogID!) {{ timelogDelete(input: {{id: $id}}) {{ {tl} }} }}"), json!({"id": log_id})).await;
    s.step("time stats", GET, &format!("{mrs}/1/time_stats"), Some(&admin), json!(null), "mr").await;
    s.step("notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}
