//! Unusual diff positions: file-level comments, renamed, deleted and binary files, multibyte
//! lines, the line without a trailing newline, lines past the end, and suggestions whose range
//! runs past the file, then applied.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn diff_position_variants() {
    let mut s = Scenario::new("diff_position_variants").await;
    let admin = s.admin();
    let gpath = s.name("dp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let content: String = (1..=12).map(|i| format!("line {i} ünï 🎉\n")).collect::<String>() + "last without newline";
    s.step("base", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "base", "actions": [
        {"action": "create", "file_path": "a.txt", "content": content},
        {"action": "create", "file_path": "bin.dat", "content": "AAEC/w==", "encoding": "base64"},
        {"action": "create", "file_path": "del.txt", "content": "gone\n"},
        {"action": "create", "file_path": "keep.txt", "content": "k1\nk2\nk3\n"}
    ]}), "commit").await;
    let changed = content.replace("line 6 ", "LINE 6 ") + "\nadded";
    s.step("change", POST, &commits, Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "change", "actions": [
        {"action": "move", "file_path": "b.txt", "previous_path": "a.txt", "content": changed},
        {"action": "update", "file_path": "bin.dat", "content": "AAECAw==", "encoding": "base64"},
        {"action": "delete", "file_path": "del.txt"},
        {"action": "update", "file_path": "keep.txt", "content": "k1\nK2\nk3\n"}
    ]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let mr = format!("{mrs}/1");
    let m = s.send(GET, &mr, Some(&admin), json!(null)).await;
    let refs = m.body["diff_refs"].clone();
    let pos = |kind: &str, old: &str, new: &str, extra: Value| {
        let mut v = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": kind, "old_path": old, "new_path": new});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    let cases: Vec<(&str, String, Value)> = vec![
        ("file comment", "On the file".into(), pos("file", "a.txt", "b.txt", json!({}))),
        ("file comment deleted", "On the deleted file".into(), pos("file", "del.txt", "del.txt", json!({}))),
        ("file comment binary", "On the binary".into(), pos("file", "bin.dat", "bin.dat", json!({}))),
        ("renamed new line", "Renamed".into(), pos("text", "a.txt", "b.txt", json!({"new_line": 6}))),
        ("renamed old line", "Old side".into(), pos("text", "a.txt", "b.txt", json!({"old_line": 6}))),
        ("renamed unchanged line", "Context".into(), pos("text", "a.txt", "b.txt", json!({"old_line": 2, "new_line": 2}))),
        ("no newline line", "Last".into(), pos("text", "a.txt", "b.txt", json!({"new_line": 13}))),
        ("added after no newline", "Added".into(), pos("text", "a.txt", "b.txt", json!({"new_line": 14}))),
        ("past end", "Far".into(), pos("text", "a.txt", "b.txt", json!({"new_line": 500}))),
        ("deleted file line", "Gone".into(), pos("text", "del.txt", "del.txt", json!({"old_line": 1}))),
        ("binary line", "Bin".into(), pos("text", "bin.dat", "bin.dat", json!({"new_line": 1}))),
        ("image on text", "Img".into(), pos("image", "keep.txt", "keep.txt", json!({"x": 1, "y": 1, "width": 10, "height": 10}))),
        ("negative image", "Img".into(), pos("image", "bin.dat", "bin.dat", json!({"x": -5, "y": 1, "width": 10, "height": 10}))),
        ("suggestion past end", "```suggestion:-0+50\nX\n```".into(), pos("text", "keep.txt", "keep.txt", json!({"old_line": 3, "new_line": 3}))),
        ("suggestion before start", "```suggestion:-50+0\nX\n```".into(), pos("text", "keep.txt", "keep.txt", json!({"new_line": 2}))),
        ("suggestion multibyte", "```suggestion:-0+0\nligne 6 🎉🎉\n```".into(), pos("text", "a.txt", "b.txt", json!({"new_line": 6}))),
        ("suggestion on no newline", "```suggestion:-0+0\nfinal\n```".into(), pos("text", "a.txt", "b.txt", json!({"new_line": 13}))),
        ("two suggestions", "```suggestion:-0+0\none\n```\nand\n```suggestion:-0+0\ntwo\n```".into(), pos("text", "keep.txt", "keep.txt", json!({"new_line": 2}))),
    ];
    for (label, body, position) in cases {
        s.step(label, POST, &format!("{mr}/discussions"), Some(&admin), json!({"body": body, "position": position}), "discussion").await;
    }
    s.step("discussions", GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100}), "discussion").await;
    // Apply the suggestions one by one, in creation order.
    let ds = s.send(GET, &format!("{mr}/discussions"), Some(&admin), json!({"per_page": 100})).await;
    let mut ids = vec![];
    for d in ds.body.as_array().cloned().unwrap_or_default() {
        for n in d["notes"].as_array().cloned().unwrap_or_default() {
            for sg in n["suggestions"].as_array().cloned().unwrap_or_default() {
                ids.push(sg["id"].clone());
            }
        }
    }
    for (i, id) in ids.iter().enumerate() {
        s.step(&format!("apply {i}"), PUT, &format!("/suggestions/{id}/apply"), Some(&admin), json!(null), "suggestion").await;
        s.settle().await;
    }
    s.step("b.txt after", GET, &format!("/projects/{pid}/repository/files/b.txt/raw"), Some(&admin), json!({"ref": "f"}), "file").await;
    s.step("keep.txt after", GET, &format!("/projects/{pid}/repository/files/keep.txt/raw"), Some(&admin), json!({"ref": "f"}), "file").await;
    s.step("notes after", GET, &format!("{mr}/notes"), Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.finish();
}
