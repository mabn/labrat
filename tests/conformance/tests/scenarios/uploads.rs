//! Markdown uploads (`/projects/:id/uploads`): what bots attach to review comments.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn uploads() {
    let mut s = Scenario::new("uploads").await;
    let admin = s.admin();
    let gpath = s.name("up");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob reporter", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 20}), "user").await;
    s.settle().await;

    let png: &[u8] = b"\\x89PNG\\r\\n\\x1a\\nnot really a png";
    let ups = format!("/projects/{pid}/uploads");
    let up = s.step_upload("upload", &ups, Some(&admin), "file", "shot one.png", "image/png", png, "upload").await;
    let up_id = up.id();
    let url = up.get("url").as_str().unwrap_or_default().to_string();
    let full_path = up.get("full_path").as_str().unwrap_or_default().to_string();
    s.step_upload("upload text", &ups, Some(&admin), "file", "notes.txt", "text/plain", b"hello\\n", "upload").await;
    s.step_upload("reporter uploads", &ups, Some(&bob_t), "file", "r.txt", "text/plain", b"x", "upload").await;
    s.step_upload("wrong field", &ups, Some(&admin), "attachment", "w.txt", "text/plain", b"x", "upload").await;
    s.step("list", GET, &ups, Some(&admin), json!(null), "upload").await;
    s.step("list as reporter", GET, &ups, Some(&bob_t), json!(null), "upload").await;
    s.step("show", GET, &format!("{ups}/{up_id}"), Some(&admin), json!(null), "upload").await;
    let secret_name = url.trim_start_matches("/uploads/").to_string();
    s.step("show by secret", GET, &format!("{ups}/{secret_name}"), Some(&admin), json!(null), "upload").await;
    // Web downloads ignore tokens (GitLab answers with HTML pages), so they aren't compared.
    let _ = (&full_path, &gpath);
    s.step("comment with image", POST, &format!("/projects/{pid}/repository/commits/main/comments"), Some(&admin), json!({"note": up.get("markdown")}), "note").await;
    s.step("delete", DELETE, &format!("{ups}/{up_id}"), Some(&admin), json!(null), "upload").await;
    s.step("show deleted", GET, &format!("{ups}/{up_id}"), Some(&admin), json!(null), "upload").await;
    s.finish();
}
