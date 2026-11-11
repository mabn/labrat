//! How note text is split into quick actions and remaining text: commands inside code blocks
//! and quotes, case, whitespace, quoted labels, several commands, unknown commands and paths.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn quick_action_parsing() {
    let mut s = Scenario::new("quick_action_parsing").await;
    let admin = s.admin();
    let gpath = s.name("qp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    for (l, col) in [("bug", "#ff0000"), ("needs review", "#00ff00"), ("ui", "#0000ff")] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": col}), "label").await;
    }
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "F", "actions": [{"action": "create", "file_path": "f.txt", "content": "f\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let notes = format!("{mrs}/1/notes");
    let bodies = [
        ("fenced", "Text\n```\n/label ~bug\n```\nmore"),
        ("tilde fence", "~~~\n/label ~bug\n~~~"),
        ("indented code", "Text\n\n    /label ~bug\n\nafter"),
        ("blockquote", "> /label ~bug\n\nquoted"),
        ("multiline quote", ">>>\n/label ~bug\n>>>\nafter"),
        ("upper case", "/LABEL ~bug"),
        ("leading spaces", "   /label ~ui"),
        ("trailing text", "/label ~ui please"),
        ("mid line", "please /label ~ui"),
        ("quoted label", "/label ~\"needs review\""),
        ("two labels one line", "/label ~bug ~ui"),
        ("two commands", "/unlabel ~bug\n/label ~\"needs review\"\nAnd a comment"),
        ("command then text", "/title New title\nSome text after"),
        ("unknown command", "/frobnicate now"),
        ("lone slash", "/"),
        ("path", "/usr/bin/env is a path"),
        ("command with crlf", "/label ~bug\r\nwindows line"),
        ("repeated command", "/label ~ui\n/label ~bug"),
        ("inline code", "`/label ~bug` inline"),
        ("html comment", "<!-- /label ~bug -->\nvisible"),
        ("command and mention", "/assign @claude\ncc @claude"),
        ("title with slash", "/title A/B testing"),
        ("tab separated", "/label\t~ui"),
    ];
    for (label, body) in bodies {
        s.step(label, POST, &notes, Some(&admin), json!({"body": body}), "note").await;
        s.settle().await;
    }
    s.step("mr after", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.step("notes after", GET, &notes, Some(&admin), json!({"sort": "asc", "per_page": 100}), "note").await;
    s.graphql(
        "explain",
        Some(&admin),
        "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { labels { nodes { title } } title assignees { nodes { username } } } } }",
        json!({"p": format!("{gpath}/app")}),
    )
    .await;
    s.finish();
}
