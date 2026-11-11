//! `Project.repository` (tree, blobs, commits, branch names), members, pipelines and branch rules.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_repository() {
    let mut s = Scenario::new("graphql_repository").await;
    let admin = s.admin();
    let gpath = s.name("gr");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    let c = s
        .step(
            "commit",
            POST,
            &format!("/projects/{pid}/repository/commits"),
            Some(&admin),
            json!({"branch": "main", "commit_message": "Add files", "actions": [
                {"action": "create", "file_path": "src/lib.rs", "content": "pub fn f() {}\n"},
                {"action": "create", "file_path": "docs/guide.md", "content": "# Guide\n"}
            ]}),
            "commit",
        )
        .await;
    let sha = c.get("id").as_str().unwrap().to_string();
    s.step("branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "feature-x", "ref": "main"}), "branch").await;
    s.step("status", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "success", "name": "ci"}), "status").await;
    s.settle().await;
    let q = |sel: &str| format!("query($path: ID!) {{ project(fullPath: $path) {{ {sel} }} }}");
    let cases = [
        ("tree", "repository { tree(ref: \"main\") { blobs { nodes { name path type mode flatPath webPath } } trees { nodes { name path type flatPath webPath } } } }"),
        ("subtree", "repository { tree(ref: \"main\", path: \"src\") { blobs { nodes { name path } } } }"),
        ("blobs", "repository { blobs(paths: [\"README.md\", \"src/lib.rs\", \"missing.txt\"], ref: \"main\") { nodes { path name rawTextBlob size oid mode fileType webPath } } }"),
        ("commit", "repository { commit(ref: \"main\") { title message } lastCommit(path: \"src/lib.rs\", ref: \"main\") { title } }"),
        ("branch names pattern", "repository { branchNames(searchPattern: \"feat*\", offset: 0, limit: 10) }"),
        ("members", "projectMembers { nodes { user { username } accessLevel { integerValue stringValue } } }"),
        ("pipelines", "pipelines { nodes { status ref source } } pipeline(sha: \"__SHA__\") { status }"),
        ("branch rules", "branchRules { nodes { name isDefault isProtected matchingBranchesCount branchProtection { allowForcePush } } }"),
    ];
    for (label, sel) in cases {
        s.graphql(label, Some(&admin), &q(&sel.replace("__SHA__", &sha)), json!({"path": full})).await;
    }
    s.finish();
}
