//! GraphQL branch mutations (docs/gitlab-ref/labrat-shortcuts-audit.md item 9): `createBranch`,
//! `branchDelete`, `branchRuleCreate`, and the
//! `Commit.tags` / `signature` fields.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_branch_mutations() {
    let mut s = Scenario::new("graphql_branch_mutations").await;
    let admin = s.admin();
    let gpath = s.name("gb");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("rep");
    let rep = s
        .step("reporter", POST, "/users", Some(&admin), json!({"username": username, "name": "Rep", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let rep_t = s.step("reporter token", POST, &format!("/users/{rep}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("reporter member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": rep, "access_level": 20}), "user").await;
    let create = "mutation($p: ID!, $n: String!, $r: String!) { createBranch(input: {projectPath: $p, name: $n, ref: $r}) { errors branch { name commit { sha title } } } }";
    let delete = "mutation($p: ID!, $n: String!) { branchDelete(input: {projectPath: $p, name: $n}) { errors branch { name } } }";
    let v = |n: &str, r: &str| json!({"p": full, "n": n, "r": r});
    s.graphql("create", Some(&admin), create, v("feature", "main")).await;
    s.graphql("create existing", Some(&admin), create, v("feature", "main")).await;
    s.graphql("create bad ref", Some(&admin), create, v("other", "nope")).await;
    s.graphql("create bad name", Some(&admin), create, v("bad..name", "main")).await;
    s.graphql("create from sha", Some(&admin), create, v("from-sha", &"0".repeat(40))).await;
    s.graphql("create as reporter", Some(&rep_t), create, v("rep", "main")).await;
    s.graphql("create unknown project", Some(&admin), create, json!({"p": "nope/nope", "n": "x", "r": "main"})).await;
    s.step("branches", GET, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!(null), "branch").await;
    s.graphql("delete", Some(&admin), delete, json!({"p": full, "n": "feature"})).await;
    s.graphql("delete missing", Some(&admin), delete, json!({"p": full, "n": "feature"})).await;
    s.graphql("delete default", Some(&admin), delete, json!({"p": full, "n": "main"})).await;
    s.graphql("delete as reporter", Some(&rep_t), delete, json!({"p": full, "n": "main"})).await;
    let rule = "mutation($p: ID!, $n: String!) { branchRuleCreate(input: {projectPath: $p, name: $n}) { errors branchRule { name isDefault isProtected } } }";
    s.graphql("branch rule create", Some(&admin), rule, json!({"p": full, "n": "release/*"})).await;
    s.graphql("branch rule create existing", Some(&admin), rule, json!({"p": full, "n": "release/*"})).await;
    s.step("protected branches", GET, &format!("/projects/{pid}/protected_branches"), Some(&admin), json!(null), "branch").await;
    let head = s.send(GET, &format!("/projects/{pid}/repository/branches/main"), Some(&admin), json!(null)).await.body["commit"]["id"].as_str().unwrap_or_default().to_string();
    s.step("tag", POST, &format!("/projects/{pid}/repository/tags"), Some(&admin), json!({"tag_name": "v1", "ref": head}), "tag").await;
    s.step("tag 2", POST, &format!("/projects/{pid}/repository/tags"), Some(&admin), json!({"tag_name": "v0", "ref": head, "message": "annotated"}), "tag").await;
    s.graphql("commit tags", Some(&admin), "query($p: ID!, $r: String!) { project(fullPath: $p) { repository { commit(ref: $r) { sha tags signature { __typename } signatureHtml } } } }", json!({"p": full, "r": head})).await;
    s.finish();
}
