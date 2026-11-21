//! EE project approval rules (`/projects/:id/approval_rules`): create, list, show, update and
//! delete, with users, groups, usernames and protected branches, validation errors and
//! permissions.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

#[tokio::test]
async fn approval_rules_project() {
    let mut s = Scenario::new("approval_rules_project").await;
    let admin = s.admin();
    let gpath = s.name("arp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    let (maint, maint_t) = user_with_token(&mut s, "maint").await;
    let (rep, _rep_t) = user_with_token(&mut s, "reporter").await;
    let (out, out_t) = user_with_token(&mut s, "outsider").await;
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    s.step("maint member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": maint, "access_level": 40}), "user").await;
    s.step("reporter member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": rep, "access_level": 20}), "user").await;
    // An approver group with one member.
    let apath = s.name("approvers");
    let ag = s.step("approver group", POST, "/groups", Some(&admin), json!({"name": apath, "path": apath, "visibility": "public"}), "namespace").await;
    let agid = ag.id();
    s.step("approver group member", POST, &format!("/groups/{agid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    let ppath = s.name("private");
    let pg = s.step("private group", POST, "/groups", Some(&admin), json!({"name": ppath, "path": ppath}), "namespace").await;
    let pgid = pg.id();
    // Membership reaches project authorizations through a background job.
    s.settle().await;
    s.step("branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "release", "ref": "main"}), "branch").await;
    let pb = s.step("protect release", POST, &format!("/projects/{pid}/protected_branches"), Some(&admin), json!({"name": "release"}), "branch").await;
    let pbid = pb.id();
    let branches = s.step("protected branches", GET, &format!("/projects/{pid}/protected_branches"), Some(&admin), json!(null), "branch").await;
    let main_pb = branches.body.as_array().unwrap().iter().find(|b| b["name"] == "main").unwrap()["id"].as_i64().unwrap();

    let rules = format!("/projects/{pid}/approval_rules");
    s.step("list empty", GET, &rules, Some(&admin), json!(null), "rule").await;
    s.step("list dev", GET, &rules, Some(&dev_t), json!(null), "rule").await;
    s.step("list outsider", GET, &rules, Some(&out_t), json!(null), "rule").await;
    s.step("list anonymous", GET, &rules, None, json!(null), "rule").await;

    // Validation.
    s.step("missing params", POST, &rules, Some(&admin), json!({}), "rule").await;
    s.step("missing count", POST, &rules, Some(&admin), json!({"name": "x"}), "rule").await;
    s.step("negative count", POST, &rules, Some(&admin), json!({"name": "x", "approvals_required": -1}), "rule").await;
    s.step("count over 100", POST, &rules, Some(&admin), json!({"name": "x", "approvals_required": 101}), "rule").await;
    s.step("blank name", POST, &rules, Some(&admin), json!({"name": "", "approvals_required": 1}), "rule").await;
    s.step("long name", POST, &rules, Some(&admin), json!({"name": "n".repeat(1025), "approvals_required": 1}), "rule").await;
    s.step("bad rule type", POST, &rules, Some(&admin), json!({"name": "x", "approvals_required": 1, "rule_type": "nope"}), "rule").await;
    s.step("report approver", POST, &rules, Some(&admin), json!({"name": "x", "approvals_required": 1, "rule_type": "report_approver"}), "rule").await;
    s.step("create as dev", POST, &rules, Some(&dev_t), json!({"name": "x", "approvals_required": 1}), "rule").await;
    s.step("create as outsider", POST, &rules, Some(&out_t), json!({"name": "x", "approvals_required": 1}), "rule").await;

    // Rules.
    let r1 = s
        .step("create users", POST, &rules, Some(&maint_t), json!({"name": "Backend", "approvals_required": 2, "user_ids": [dev, maint, rep, out]}), "rule")
        .await;
    let r1id = r1.body["id"].as_i64().unwrap_or(0);
    s.step("duplicate name", POST, &rules, Some(&admin), json!({"name": "Backend", "approvals_required": 1}), "rule").await;
    let r2 = s
        .step(
            "create groups",
            POST,
            &rules,
            Some(&admin),
            json!({"name": "Groups", "approvals_required": 1, "group_ids": [agid, pgid], "usernames": [s.name("maint")]}),
            "rule",
        )
        .await;
    let r2id = r2.body["id"].as_i64().unwrap_or(0);
    let r3 = s
        .step("create branch rule", POST, &rules, Some(&admin), json!({"name": "Release", "approvals_required": 1, "user_ids": [maint], "protected_branch_ids": [pbid, main_pb]}), "rule")
        .await;
    let r3id = r3.body["id"].as_i64().unwrap_or(0);
    s.step("create all protected", POST, &rules, Some(&admin), json!({"name": "All protected", "approvals_required": 0, "applies_to_all_protected_branches": true, "protected_branch_ids": [pbid]}), "rule").await;
    s.step("bad branch", POST, &rules, Some(&admin), json!({"name": "Bad branch", "approvals_required": 1, "protected_branch_ids": [999999999]}), "rule").await;
    s.step("missing user", POST, &rules, Some(&admin), json!({"name": "Ghost", "approvals_required": 1, "user_ids": [999999999]}), "rule").await;
    s.step("missing group", POST, &rules, Some(&admin), json!({"name": "Ghost group", "approvals_required": 1, "group_ids": [999999999]}), "rule").await;
    s.step("any approver", POST, &rules, Some(&admin), json!({"name": "Any", "approvals_required": 1, "rule_type": "any_approver"}), "rule").await;
    s.step("second any approver", POST, &rules, Some(&admin), json!({"name": "Any 2", "approvals_required": 1, "rule_type": "any_approver"}), "rule").await;
    s.step("regular with no approvers", POST, &rules, Some(&admin), json!({"name": "Nobody", "approvals_required": 3}), "rule").await;
    s.step("list", GET, &rules, Some(&admin), json!(null), "rule").await;
    s.step("list page", GET, &rules, Some(&admin), json!({"per_page": 2, "page": 2}), "rule").await;
    s.step("list as dev", GET, &rules, Some(&dev_t), json!(null), "rule").await;
    s.step("show", GET, &format!("{rules}/{r2id}"), Some(&admin), json!(null), "rule").await;
    s.step("show as dev", GET, &format!("{rules}/{r2id}"), Some(&dev_t), json!(null), "rule").await;
    s.step("show missing", GET, &format!("{rules}/999999999"), Some(&admin), json!(null), "rule").await;
    s.step("show outsider", GET, &format!("{rules}/{r2id}"), Some(&out_t), json!(null), "rule").await;

    // Updates replace the approver lists.
    s.step("update name", PUT, &format!("{rules}/{r1id}"), Some(&admin), json!({"name": "Backend team"}), "rule").await;
    s.step("update users", PUT, &format!("{rules}/{r1id}"), Some(&admin), json!({"user_ids": [maint], "approvals_required": 1}), "rule").await;
    s.step("update usernames", PUT, &format!("{rules}/{r1id}"), Some(&admin), json!({"usernames": [s.name("dev")]}), "rule").await;
    s.step("update clear groups", PUT, &format!("{rules}/{r2id}"), Some(&admin), json!({"group_ids": []}), "rule").await;
    s.step("update hidden groups", PUT, &format!("{rules}/{r2id}"), Some(&admin), json!({"group_ids": [pgid], "remove_hidden_groups": true}), "rule").await;
    s.step("update branches", PUT, &format!("{rules}/{r3id}"), Some(&admin), json!({"protected_branch_ids": [pbid]}), "rule").await;
    s.step("update all protected", PUT, &format!("{rules}/{r3id}"), Some(&admin), json!({"applies_to_all_protected_branches": true}), "rule").await;
    s.step("update bad count", PUT, &format!("{rules}/{r3id}"), Some(&admin), json!({"approvals_required": -5}), "rule").await;
    s.step("update duplicate", PUT, &format!("{rules}/{r3id}"), Some(&admin), json!({"name": "Groups"}), "rule").await;
    s.step("update nothing", PUT, &format!("{rules}/{r3id}"), Some(&admin), json!({}), "rule").await;
    s.step("update as dev", PUT, &format!("{rules}/{r3id}"), Some(&dev_t), json!({"name": "x"}), "rule").await;
    s.step("update missing", PUT, &format!("{rules}/999999999"), Some(&admin), json!({"name": "x"}), "rule").await;
    s.step("delete as dev", DELETE, &format!("{rules}/{r3id}"), Some(&dev_t), json!(null), "rule").await;
    s.step("delete", DELETE, &format!("{rules}/{r3id}"), Some(&admin), json!(null), "rule").await;
    s.step("delete again", DELETE, &format!("{rules}/{r3id}"), Some(&admin), json!(null), "rule").await;
    s.step("list after", GET, &rules, Some(&admin), json!(null), "rule").await;
    // Rules show up in the project's approval settings and its protected branches.
    s.step("approvals", GET, &format!("/projects/{pid}/approvals"), Some(&admin), json!(null), "x").await;
    s.step("protected after", GET, &format!("/projects/{pid}/protected_branches/release"), Some(&admin), json!(null), "branch").await;
    s.step("unprotect release", DELETE, &format!("/projects/{pid}/protected_branches/release"), Some(&admin), json!(null), "branch").await;
    s.step("list after unprotect", GET, &rules, Some(&admin), json!(null), "rule").await;
    s.step("remove member", DELETE, &format!("/projects/{pid}/members/{rep}"), Some(&admin), json!(null), "user").await;
    s.step("list after member removal", GET, &rules, Some(&admin), json!(null), "rule").await;
    s.finish();
}
