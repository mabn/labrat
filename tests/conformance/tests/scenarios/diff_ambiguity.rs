//! Diffs where several hunk placements are valid (docs/gitlab-ref/labrat-shortcuts-audit.md item 2):
//! a function inserted between similar functions (git's indent heuristic), duplicated blocks,
//! moved blocks, blank-line runs, whitespace-only changes, CRLF lines, a lost final newline, and a
//! partly similar rename. Compared through MR diffs and the compare API.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn diff_ambiguity() {
    let mut s = Scenario::new("diff_ambiguity").await;
    let admin = s.admin();
    let gpath = s.name("da");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let func = |name: &str| format!("def {name}(x):\n    if x:\n        return 1\n    return 0\n\n");
    let funcs = format!("{}{}", func("a"), func("b"));
    let funcs_new = format!("{}{}{}", func("a"), func("inserted"), func("b"));
    let dup = "start\nblock\nblock\nblock\nend\n".to_string();
    let dup_new = "start\nblock\nblock\nblock\nblock\nend\n".to_string();
    let moved = "one\ntwo\nthree\nfour\nfive\nsix\n".to_string();
    let moved_new = "four\nfive\nsix\none\ntwo\nthree\n".to_string();
    let blanks = "a\n\n\nb\n\n\nc\n".to_string();
    let blanks_new = "a\n\n\nb\n\n\n\nc\n".to_string();
    let ws = "x = 1\ny = 2\n".to_string();
    let ws_new = "x = 1 \ny  = 2\n".to_string();
    let crlf = "one\r\ntwo\r\nthree\r\n".to_string();
    let crlf_new = "one\r\nTWO\r\nthree\r\n".to_string();
    let eof = "last line\n".to_string();
    let eof_new = "last line".to_string();
    let ren: String = (1..=10).map(|i| format!("row {i}\n")).collect();
    let ren_new: String = (1..=10).map(|i| if i % 3 == 0 { format!("ROW {i}\n") } else { format!("row {i}\n") }).collect();
    let files = [("funcs.py", &funcs, &funcs_new), ("dup.txt", &dup, &dup_new), ("moved.txt", &moved, &moved_new), ("blanks.txt", &blanks, &blanks_new), ("ws.txt", &ws, &ws_new), ("crlf.txt", &crlf, &crlf_new), ("eof.txt", &eof, &eof_new)];
    let commits = format!("/projects/{pid}/repository/commits");
    let mut create: Vec<serde_json::Value> = files.iter().map(|(f, a, _)| json!({"action": "create", "file_path": f, "content": a})).collect();
    create.push(json!({"action": "create", "file_path": "old_name.txt", "content": ren}));
    s.step("base", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "base", "actions": create}), "commit").await;
    let mut update: Vec<serde_json::Value> = files.iter().map(|(f, _, b)| json!({"action": "update", "file_path": f, "content": b})).collect();
    update.push(json!({"action": "move", "file_path": "new_name.txt", "previous_path": "old_name.txt", "content": ren_new}));
    s.step("change", POST, &commits, Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "change", "actions": update}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Diffs"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr diffs", GET, &format!("/projects/{pid}/merge_requests/1/diffs"), Some(&admin), json!({"per_page": 50}), "diff").await;
    s.step("compare", GET, &format!("/projects/{pid}/repository/compare"), Some(&admin), json!({"from": "main", "to": "f"}), "diff").await;
    s.step("raw diffs", GET, &format!("/projects/{pid}/merge_requests/1/raw_diffs"), Some(&admin), json!(null), "diff").await;
    s.finish();
}
