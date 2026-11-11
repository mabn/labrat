use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn repository_basics() {
    let mut s = Scenario::new("repository_basics").await;
    let admin = s.admin();
    let gpath = s.name("repo");
    let g = s
        .step(
            "create group",
            POST,
            "/groups",
            Some(&admin),
            json!({"name": gpath, "path": gpath}),
            "namespace",
        )
        .await;
    let gid = g.id();
    let p = s
        .step(
            "create project with readme",
            POST,
            "/projects",
            Some(&admin),
            json!({"name": "code", "namespace_id": gid, "initialize_with_readme": true, "description": "Demo code"}),
            "project",
        )
        .await;
    let pid = p.id();
    s.settle().await;
    let base = format!("/projects/{pid}/repository");
    s.step(
        "branches",
        GET,
        &format!("{base}/branches"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.step(
        "protected branches",
        GET,
        &format!("/projects/{pid}/protected_branches"),
        Some(&admin),
        Value::Null,
        "pb",
    )
    .await;
    s.step(
        "readme file",
        GET,
        &format!("{base}/files/README.md"),
        Some(&admin),
        json!({"ref": "main"}),
        "file",
    )
    .await;
    s.step(
        "commit with actions",
        POST,
        &format!("{base}/commits"),
        Some(&admin),
        json!({
            "branch": "feature/one",
            "start_branch": "main",
            "commit_message": "Add library\n\nWith a body.",
            "actions": [
                {"action": "create", "file_path": "lib/calc.py", "content": "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n"},
                {"action": "create", "file_path": "docs/notes.txt", "content": "notes\n"},
                {"action": "update", "file_path": "README.md", "content": "# code\n\nShort readme.\n"}
            ]
        }),
        "commit",
    )
    .await;
    s.step(
        "second commit",
        POST,
        &format!("{base}/commits"),
        Some(&admin),
        json!({
            "branch": "feature/one",
            "commit_message": "Change calc",
            "actions": [
                {"action": "update", "file_path": "lib/calc.py", "content": "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return b - a\n\n\ndef mul(a, b):\n    return a * b\n"},
                {"action": "move", "previous_path": "docs/notes.txt", "file_path": "docs/NOTES.md"},
                {"action": "chmod", "file_path": "lib/calc.py", "execute_filemode": true}
            ]
        }),
        "commit",
    )
    .await;
    s.step("bad action", POST, &format!("{base}/commits"), Some(&admin),
        json!({"branch": "feature/one", "commit_message": "x", "actions": [{"action": "create", "file_path": "README.md", "content": "x"}]}), "commit").await;
    s.step(
        "missing actions",
        POST,
        &format!("{base}/commits"),
        Some(&admin),
        json!({"branch": "feature/one", "commit_message": "x"}),
        "commit",
    )
    .await;
    s.step(
        "branch list",
        GET,
        &format!("{base}/branches"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.step(
        "branch single",
        GET,
        &format!("{base}/branches/feature%2Fone"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.step(
        "missing branch",
        GET,
        &format!("{base}/branches/nope"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.step(
        "commits on feature",
        GET,
        &format!("{base}/commits"),
        Some(&admin),
        json!({"ref_name": "feature/one"}),
        "commit",
    )
    .await;
    s.step(
        "commit detail",
        GET,
        &format!("{base}/commits/feature%2Fone"),
        Some(&admin),
        Value::Null,
        "commit",
    )
    .await;
    s.step(
        "commit diff",
        GET,
        &format!("{base}/commits/feature%2Fone/diff"),
        Some(&admin),
        Value::Null,
        "diff",
    )
    .await;
    s.step(
        "compare",
        GET,
        &format!("{base}/compare"),
        Some(&admin),
        json!({"from": "main", "to": "feature/one"}),
        "compare",
    )
    .await;
    s.step(
        "tree",
        GET,
        &format!("{base}/tree"),
        Some(&admin),
        json!({"ref": "feature/one", "recursive": true}),
        "tree",
    )
    .await;
    s.step(
        "file raw meta",
        GET,
        &format!("{base}/files/lib%2Fcalc.py"),
        Some(&admin),
        json!({"ref": "feature/one"}),
        "file",
    )
    .await;
    s.step(
        "create branch",
        POST,
        &format!("{base}/branches"),
        Some(&admin),
        json!({"branch": "other", "ref": "main"}),
        "branch",
    )
    .await;
    s.step(
        "create branch dup",
        POST,
        &format!("{base}/branches"),
        Some(&admin),
        json!({"branch": "other", "ref": "main"}),
        "branch",
    )
    .await;
    s.step(
        "create branch bad ref",
        POST,
        &format!("{base}/branches"),
        Some(&admin),
        json!({"branch": "x", "ref": "nope"}),
        "branch",
    )
    .await;
    s.step(
        "create file",
        POST,
        &format!("{base}/files/new%2Ffile.txt"),
        Some(&admin),
        json!({"branch": "other", "content": "hello\n", "commit_message": "Add file"}),
        "file",
    )
    .await;
    s.step(
        "create file again",
        POST,
        &format!("{base}/files/new%2Ffile.txt"),
        Some(&admin),
        json!({"branch": "other", "content": "hello\n", "commit_message": "Add file"}),
        "file",
    )
    .await;
    s.step(
        "update file",
        PUT,
        &format!("{base}/files/new%2Ffile.txt"),
        Some(&admin),
        json!({"branch": "other", "content": "hello world\n", "commit_message": "Update file"}),
        "file",
    )
    .await;
    s.step(
        "tag",
        POST,
        &format!("{base}/tags"),
        Some(&admin),
        json!({"tag_name": "v1.0", "ref": "main", "message": "Release 1"}),
        "tag",
    )
    .await;
    s.step(
        "light tag",
        POST,
        &format!("{base}/tags"),
        Some(&admin),
        json!({"tag_name": "v1.1", "ref": "other"}),
        "tag",
    )
    .await;
    s.step(
        "tags",
        GET,
        &format!("{base}/tags"),
        Some(&admin),
        Value::Null,
        "tag",
    )
    .await;
    s.step(
        "delete default branch",
        DELETE,
        &format!("{base}/branches/main"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.step(
        "delete branch",
        DELETE,
        &format!("{base}/branches/other"),
        Some(&admin),
        Value::Null,
        "branch",
    )
    .await;
    s.step(
        "protect wildcard",
        POST,
        &format!("/projects/{pid}/protected_branches"),
        Some(&admin),
        json!({"name": "release/*", "push_access_level": 30, "merge_access_level": 40}),
        "pb",
    )
    .await;
    s.step(
        "delete group",
        DELETE,
        &format!("/groups/{gid}"),
        Some(&admin),
        Value::Null,
        "namespace",
    )
    .await;
    s.finish();
}
