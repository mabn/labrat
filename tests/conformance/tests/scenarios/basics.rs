use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

async fn create_user(s: &mut Scenario, base: &str) -> (i64, String) {
    let admin = s.admin();
    let username = s.name(base);
    let u = s
        .step(
            &format!("create user {base}"),
            POST,
            "/users",
            Some(&admin),
            json!({"username": username, "name": format!("User {base}"), "email": format!("{username}@example.com"),
                   "password": "Password-123-labrat", "skip_confirmation": true}),
            "user",
        )
        .await;
    let id = u.id();
    let t = s
        .step(
            &format!("token for {base}"),
            POST,
            &format!("/users/{id}/personal_access_tokens"),
            Some(&admin),
            json!({"name": "test", "scopes": ["api"]}),
            "token",
        )
        .await;
    (id, t.get("token").as_str().unwrap().to_string())
}

#[tokio::test]
async fn users_and_tokens() {
    let mut s = Scenario::new("users_and_tokens").await;
    let admin = s.admin();
    s.step(
        "current user (admin)",
        GET,
        "/user",
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    let (alice, alice_token) = create_user(&mut s, "alice").await;
    let username = s.name("alice");
    s.step(
        "duplicate username",
        POST,
        "/users",
        Some(&admin),
        json!({"username": username, "name": "Dup", "email": format!("dup-{username}@example.com"), "password": "Password-123-labrat"}),
        "user",
    )
    .await;
    s.step(
        "duplicate email",
        POST,
        "/users",
        Some(&admin),
        json!({"username": s.name("other"), "name": "Dup", "email": format!("{username}@example.com"), "password": "Password-123-labrat"}),
        "user",
    )
    .await;
    s.step(
        "missing params",
        POST,
        "/users",
        Some(&admin),
        json!({"email": "x@example.com"}),
        "user",
    )
    .await;
    s.step(
        "reserved username",
        POST,
        "/users",
        Some(&admin),
        json!({"username": "admin", "name": "Admin", "email": format!("a-{username}@example.com"), "password": "Password-123-labrat"}),
        "user",
    )
    .await;
    s.step(
        "get user (admin)",
        GET,
        &format!("/users/{alice}"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "current user (alice)",
        GET,
        "/user",
        Some(&alice_token),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "get self (alice)",
        GET,
        &format!("/users/{alice}"),
        Some(&alice_token),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "users by username (alice)",
        GET,
        "/users",
        Some(&alice_token),
        json!({"username": username}),
        "user",
    )
    .await;
    s.step(
        "token self",
        GET,
        "/personal_access_tokens/self",
        Some(&alice_token),
        Value::Null,
        "token",
    )
    .await;
    s.step(
        "bad token",
        GET,
        "/user",
        Some("glpat-not-a-real-token-xx"),
        Value::Null,
        "user",
    )
    .await;
    s.step("anonymous user", GET, "/user", None, Value::Null, "user")
        .await;
    s.step(
        "anonymous users list",
        GET,
        "/users",
        None,
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "update bio",
        PUT,
        &format!("/users/{alice}"),
        Some(&admin),
        json!({"bio": "Hello", "job_title": "Tester"}),
        "user",
    )
    .await;
    s.step(
        "non-admin update",
        PUT,
        &format!("/users/{alice}"),
        Some(&alice_token),
        json!({"bio": "x"}),
        "user",
    )
    .await;
    s.step(
        "block",
        POST,
        &format!("/users/{alice}/block"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "blocked user request",
        GET,
        "/user",
        Some(&alice_token),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "unblock",
        POST,
        &format!("/users/{alice}/unblock"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "unknown user",
        GET,
        "/users/999999999",
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "unknown route",
        GET,
        "/no/such/route",
        Some(&admin),
        Value::Null,
        "x",
    )
    .await;
    s.step(
        "delete user",
        DELETE,
        &format!("/users/{alice}"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.finish();
}

#[tokio::test]
async fn groups_projects_members() {
    let mut s = Scenario::new("groups_projects_members").await;
    let admin = s.admin();
    let gpath = s.name("grp");
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
    s.step(
        "duplicate group",
        POST,
        "/groups",
        Some(&admin),
        json!({"name": gpath, "path": gpath}),
        "namespace",
    )
    .await;
    s.step(
        "get group",
        GET,
        &format!("/groups/{gid}"),
        Some(&admin),
        Value::Null,
        "namespace",
    )
    .await;
    let p = s
        .step(
            "create project",
            POST,
            "/projects",
            Some(&admin),
            json!({"name": "demo", "namespace_id": gid}),
            "project",
        )
        .await;
    let pid = p.id();
    s.step(
        "duplicate project",
        POST,
        "/projects",
        Some(&admin),
        json!({"name": "demo", "namespace_id": gid}),
        "project",
    )
    .await;
    s.step(
        "get project (admin)",
        GET,
        &format!("/projects/{pid}"),
        Some(&admin),
        Value::Null,
        "project",
    )
    .await;
    let enc = format!("{gpath}%2Fdemo");
    s.step(
        "get project by path",
        GET,
        &format!("/projects/{enc}"),
        Some(&admin),
        Value::Null,
        "project",
    )
    .await;
    let (bob, bob_token) = create_user(&mut s, "bob").await;
    s.step(
        "bob cannot see private project",
        GET,
        &format!("/projects/{pid}"),
        Some(&bob_token),
        Value::Null,
        "project",
    )
    .await;
    s.step(
        "add bob to group",
        POST,
        &format!("/groups/{gid}/members"),
        Some(&admin),
        json!({"user_id": bob, "access_level": 30}),
        "user",
    )
    .await;
    s.settle().await;
    s.step(
        "add bob again",
        POST,
        &format!("/groups/{gid}/members"),
        Some(&admin),
        json!({"user_id": bob, "access_level": 30}),
        "user",
    )
    .await;
    s.step(
        "project as bob",
        GET,
        &format!("/projects/{pid}"),
        Some(&bob_token),
        Value::Null,
        "project",
    )
    .await;
    s.step(
        "group members",
        GET,
        &format!("/groups/{gid}/members"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "project members",
        GET,
        &format!("/projects/{pid}/members"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "project members all",
        GET,
        &format!("/projects/{pid}/members/all"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "missing member",
        GET,
        &format!("/projects/{pid}/members/{bob}"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "bob cannot update project",
        PUT,
        &format!("/projects/{pid}"),
        Some(&bob_token),
        json!({"merge_method": "ff"}),
        "project",
    )
    .await;
    s.step(
        "update merge settings",
        PUT,
        &format!("/projects/{pid}"),
        Some(&admin),
        json!({"merge_method": "rebase_merge", "squash_option": "always", "only_allow_merge_if_all_discussions_are_resolved": true}),
        "project",
    )
    .await;
    s.step(
        "anonymous project",
        GET,
        &format!("/projects/{pid}"),
        None,
        Value::Null,
        "project",
    )
    .await;
    s.step(
        "group projects",
        GET,
        &format!("/groups/{gid}/projects"),
        Some(&admin),
        Value::Null,
        "project",
    )
    .await;
    s.step(
        "bob's groups",
        GET,
        "/groups",
        Some(&bob_token),
        Value::Null,
        "namespace",
    )
    .await;
    s.step(
        "change bob level",
        PUT,
        &format!("/groups/{gid}/members/{bob}"),
        Some(&admin),
        json!({"access_level": 40}),
        "user",
    )
    .await;
    s.step(
        "remove bob",
        DELETE,
        &format!("/groups/{gid}/members/{bob}"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.step(
        "delete project",
        DELETE,
        &format!("/projects/{pid}"),
        Some(&admin),
        Value::Null,
        "project",
    )
    .await;
    s.step(
        "deleted project",
        GET,
        &format!("/projects/{pid}"),
        Some(&admin),
        Value::Null,
        "project",
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
    s.step(
        "delete bob",
        DELETE,
        &format!("/users/{bob}"),
        Some(&admin),
        Value::Null,
        "user",
    )
    .await;
    s.finish();
}
