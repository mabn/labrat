use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn commit_comments() {
    let mut s = Scenario::new("commit_comments").await;
    let admin = s.admin();
    let gpath = s.name("cc");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("base", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Base", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\nthree\nfour\nfive\n"}]}), "commit").await;
    let c = s
        .step("change", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Change", "actions": [{"action": "update", "file_path": "a.txt", "content": "one\nTWO\nthree\nfour\nfive\nsix\n"}]}), "commit")
        .await;
    let sha = c.get("id").as_str().unwrap().to_string();
    let url = format!("{commits}/{sha}/comments");
    s.step("plain comment", POST, &url, Some(&admin), json!({"note": "Nice"}), "note").await;
    s.step("added line", POST, &url, Some(&admin), json!({"note": "On two", "path": "a.txt", "line": 2, "line_type": "new"}), "note").await;
    s.step("removed line", POST, &url, Some(&admin), json!({"note": "Old two", "path": "a.txt", "line": 2, "line_type": "old"}), "note").await;
    s.step("context line", POST, &url, Some(&admin), json!({"note": "Context", "path": "a.txt", "line": 3}), "note").await;
    s.step("unknown path", POST, &url, Some(&admin), json!({"note": "Nowhere", "path": "nope.txt", "line": 1}), "note").await;
    s.step("path without line", POST, &url, Some(&admin), json!({"note": "x", "path": "a.txt"}), "note").await;
    s.step("bad line type", POST, &url, Some(&admin), json!({"note": "x", "path": "a.txt", "line": 2, "line_type": "both"}), "note").await;
    s.step("missing note", POST, &url, Some(&admin), json!({}), "note").await;
    s.step("unknown commit", POST, &format!("{commits}/0000000000000000000000000000000000000000/comments"), Some(&admin), json!({"note": "x"}), "note").await;
    s.step("list", GET, &url, Some(&admin), json!(null), "note").await;
    s.step("list by branch", GET, &format!("{commits}/main/comments"), Some(&admin), json!({"per_page": 2}), "note").await;
    s.finish();
}
