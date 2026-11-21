//! EE approval settings: the legacy project endpoint (`/projects/:id/approvals`), the
//! newer `merge_request_approval_setting` endpoints for projects and groups, their validation
//! and who may read or change them.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

/// A user with an `api` token.
pub async fn user_with_token(s: &mut Scenario, base: &str) -> (i64, String) {
    let admin = s.admin();
    let username = s.name(base);
    let id = s
        .step(&format!("user {base}"), POST, "/users", Some(&admin), json!({"username": username, "name": base, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let token = s
        .step(&format!("token {base}"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token")
        .await
        .get("token")
        .as_str()
        .unwrap()
        .to_string();
    (id, token)
}

#[tokio::test]
async fn approval_settings() {
    let mut s = Scenario::new("approval_settings").await;
    let admin = s.admin();
    let gpath = s.name("as");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    let (maint, maint_t) = user_with_token(&mut s, "maint").await;
    let (_out, out_t) = user_with_token(&mut s, "outsider").await;
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    s.step("maint member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": maint, "access_level": 40}), "user").await;
    s.settle().await;

    // The legacy project endpoint.
    let approvals = format!("/projects/{pid}/approvals");
    s.step("project approvals", GET, &approvals, Some(&admin), json!(null), "x").await;
    s.step("project approvals dev", GET, &approvals, Some(&dev_t), json!(null), "x").await;
    s.step("project approvals outsider", GET, &approvals, Some(&out_t), json!(null), "x").await;
    s.step("project approvals anonymous", GET, &approvals, None, json!(null), "x").await;
    s.step("change as dev", POST, &approvals, Some(&dev_t), json!({"reset_approvals_on_push": false}), "x").await;
    s.step("change as maintainer", POST, &approvals, Some(&maint_t), json!({"reset_approvals_on_push": false}), "x").await;
    s.step("change nothing", POST, &approvals, Some(&admin), json!({}), "x").await;
    s.step(
        "change all",
        POST,
        &approvals,
        Some(&admin),
        json!({"approvals_before_merge": 2, "disable_overriding_approvers_per_merge_request": true, "merge_requests_author_approval": true,
               "merge_requests_disable_committers_approval": true, "require_password_to_approve": true, "reset_approvals_on_push": true}),
        "x",
    )
    .await;
    s.step("selective with reset", POST, &approvals, Some(&admin), json!({"selective_code_owner_removals": true}), "x").await;
    s.step("selective without reset", POST, &approvals, Some(&admin), json!({"reset_approvals_on_push": false, "selective_code_owner_removals": true}), "x").await;
    s.step("reauthentication", POST, &approvals, Some(&admin), json!({"require_reauthentication_to_approve": true}), "x").await;
    s.step("bad boolean", POST, &approvals, Some(&admin), json!({"reset_approvals_on_push": "maybe"}), "x").await;
    s.step("negative count", POST, &approvals, Some(&admin), json!({"approvals_before_merge": -1}), "x").await;
    s.step("huge count", POST, &approvals, Some(&admin), json!({"approvals_before_merge": 1000}), "x").await;
    s.step("put is not allowed", PUT, &approvals, Some(&admin), json!({"reset_approvals_on_push": true}), "x").await;
    s.step("after changes", GET, &approvals, Some(&admin), json!(null), "x").await;
    s.step("project after changes", GET, &format!("/projects/{pid}"), Some(&admin), json!(null), "project").await;

    // The newer setting endpoint shows the same settings with other names.
    let setting = format!("/projects/{pid}/merge_request_approval_setting");
    s.step("setting", GET, &setting, Some(&admin), json!(null), "x").await;
    s.step("setting maintainer", GET, &setting, Some(&maint_t), json!(null), "x").await;
    s.step("setting dev", GET, &setting, Some(&dev_t), json!(null), "x").await;
    s.step("setting outsider", GET, &setting, Some(&out_t), json!(null), "x").await;
    s.step("setting anonymous", GET, &setting, None, json!(null), "x").await;
    s.step("put setting dev", PUT, &setting, Some(&dev_t), json!({"allow_author_approval": false}), "x").await;
    s.step(
        "put setting",
        PUT,
        &setting,
        Some(&maint_t),
        json!({"allow_author_approval": false, "allow_committer_approval": true, "allow_overrides_to_approver_list_per_merge_request": true,
               "retain_approvals_on_push": true, "require_reauthentication_to_approve": false}),
        "x",
    )
    .await;
    s.step("put selective", PUT, &setting, Some(&maint_t), json!({"selective_code_owner_removals": true}), "x").await;
    s.step("put selective retained off", PUT, &setting, Some(&maint_t), json!({"retain_approvals_on_push": false, "selective_code_owner_removals": true}), "x").await;
    s.step("put nothing", PUT, &setting, Some(&maint_t), json!({}), "x").await;
    s.step("put bad boolean", PUT, &setting, Some(&maint_t), json!({"allow_author_approval": "maybe"}), "x").await;
    s.step("legacy after setting", GET, &approvals, Some(&admin), json!(null), "x").await;

    // Group settings: owners only, and they lock the project's settings.
    let gsetting = format!("/groups/{gid}/merge_request_approval_setting");
    s.step("group setting", GET, &gsetting, Some(&admin), json!(null), "x").await;
    s.step("group setting dev", GET, &gsetting, Some(&dev_t), json!(null), "x").await;
    s.step("group setting outsider", GET, &gsetting, Some(&out_t), json!(null), "x").await;
    s.step("group put", PUT, &gsetting, Some(&admin), json!({"allow_author_approval": false, "allow_committer_approval": false, "retain_approvals_on_push": false, "allow_overrides_to_approver_list_per_merge_request": false}), "x").await;
    s.step("group put selective", PUT, &gsetting, Some(&admin), json!({"selective_code_owner_removals": true}), "x").await;
    s.step("group put bad", PUT, &gsetting, Some(&admin), json!({"allow_author_approval": "x"}), "x").await;
    s.step("project setting locked", GET, &setting, Some(&admin), json!(null), "x").await;
    s.step("legacy locked", GET, &approvals, Some(&admin), json!(null), "x").await;
    s.step("put locked setting", PUT, &setting, Some(&maint_t), json!({"allow_author_approval": true, "allow_committer_approval": true}), "x").await;
    s.step("post locked legacy", POST, &approvals, Some(&maint_t), json!({"merge_requests_author_approval": true, "merge_requests_disable_committers_approval": false}), "x").await;
    s.step("legacy after locked change", GET, &approvals, Some(&admin), json!(null), "x").await;
    s.step("group put back", PUT, &gsetting, Some(&admin), json!({"allow_author_approval": true, "allow_committer_approval": true, "retain_approvals_on_push": true, "allow_overrides_to_approver_list_per_merge_request": true}), "x").await;
    s.step("project setting unlocked", GET, &setting, Some(&admin), json!(null), "x").await;

    // Subgroup projects inherit from the top-level group.
    let sub = s.step("subgroup", POST, "/groups", Some(&admin), json!({"name": "sub", "path": "sub", "parent_id": gid}), "namespace").await;
    s.step("subgroup setting", GET, &format!("/groups/{}/merge_request_approval_setting", sub.id()), Some(&admin), json!(null), "x").await;
    s.step("group approval rules", GET, &format!("/groups/{gid}/approval_rules"), Some(&admin), json!(null), "x").await;
    s.step("group approval rules create", POST, &format!("/groups/{gid}/approval_rules"), Some(&admin), json!({"name": "r", "approvals_required": 1}), "x").await;
    s.step("missing project", GET, "/projects/999999999/approvals", Some(&admin), json!(null), "x").await;
    s.step("missing project setting", GET, "/projects/999999999/merge_request_approval_setting", Some(&admin), json!(null), "x").await;
    s.finish();
}
