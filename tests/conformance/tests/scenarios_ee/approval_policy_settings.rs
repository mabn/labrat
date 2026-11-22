//! EE approval policies' role approvers and approval settings, as merge requests and branches
//! see them: rules naming roles, committers kept from approving, approvals that need the
//! user's password, approvals removed by a push although the project keeps them, and covered
//! protected branches that can't be changed, removed or pushed to.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const PATCH: Method = Method::PATCH;
const DELETE: Method = Method::DELETE;

const MR_QUERY: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  approved approvalsRequired approvalsLeft detailedMergeStatus
  approvalState { suggestedApprovers { nodes { username } }
    rules { name type approvalsRequired approved eligibleApprovers { username } users { nodes { username } } approvedBy { nodes { username } } } }
  policiesOverridingApprovalSettings { name settings }
  userPermissions { canApprove }
} } }";

fn policies() -> Value {
    json!([
        {
            "name": "Maintainers approve",
            "description": "",
            "enabled": true,
            "rules": [{"type": "any_merge_request", "branch_type": "protected", "commits": "any"}],
            "actions": [{"type": "require_approval", "approvals_required": 1, "role_approvers": ["maintainer"]}],
            "approval_settings": {
                "prevent_approval_by_commit_author": true,
                "remove_approvals_with_new_commit": true,
                "require_password_to_approve": true,
                "block_branch_modification": true,
                "prevent_pushing_and_force_pushing": true,
            },
        },
        {
            "name": "Developers approve",
            "description": "",
            "enabled": true,
            "rules": [{"type": "any_merge_request", "branch_type": "protected", "commits": "any"}],
            "actions": [{"type": "require_approval", "approvals_required": 1, "role_approvers": ["developer"]}],
        },
    ])
}

#[tokio::test]
async fn approval_policy_settings() {
    let mut s = Scenario::new("approval_policy_settings").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let gpath = s.name("pset");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let (author, author_t) = user_with_token(&mut s, "author").await;
    let (committer, committer_t) = user_with_token(&mut s, "committer").await;
    let (maint, maint_t) = user_with_token(&mut s, "maint").await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    for (label, uid, level) in [("author member", author, 30), ("committer member", committer, 30), ("maint member", maint, 40), ("dev member", dev, 30)] {
        s.step(label, POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": uid, "access_level": level}), "user").await;
    }
    // The project keeps approvals on push and lets committers approve; the policy doesn't.
    s.step("project settings", POST, &format!("/projects/{pid}/approvals"), Some(&admin), json!({"reset_approvals_on_push": false, "merge_requests_disable_committers_approval": false}), "approval").await;
    s.step("release protected", POST, &format!("/projects/{pid}/protected_branches"), Some(&admin), json!({"name": "release", "push_access_level": 30, "merge_access_level": 30}), "protected_branch").await;
    s.step("release branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "release", "ref": "main"}), "branch").await;
    s.approval_policies(&full, policies()).await;
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": rx.url, "merge_requests_events": true, "push_events": false}),
        "hook",
    )
    .await;

    // The committer's commit, the author's MR.
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("commit", POST, &commits, Some(&committer_t), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&author_t), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.poll(&format!("{mrs}/1/notes"), |b| b.as_array().is_some_and(|a| a.iter().any(|n| n["system"] == false))).await;
    let mr = format!("{mrs}/1");
    let vars = json!({"path": full, "iid": "1"});
    s.step("approval state", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("mr rules", GET, &format!("{mr}/approval_rules"), Some(&admin), json!(null), "rule").await;
    s.step("approvals", GET, &format!("{mr}/approvals"), Some(&admin), json!(null), "approval").await;
    s.step("approvals as committer", GET, &format!("{mr}/approvals"), Some(&committer_t), json!(null), "approval").await;
    s.step("approvals as maint", GET, &format!("{mr}/approvals"), Some(&maint_t), json!(null), "approval").await;
    s.graphql("graphql", Some(&admin), MR_QUERY, vars.clone()).await;
    s.graphql("graphql as committer", Some(&committer_t), MR_QUERY, vars.clone()).await;

    // Committers can't approve; approving needs the password.
    s.step("committer approves", POST, &format!("{mr}/approve"), Some(&committer_t), json!({"approval_password": "Password-123-labrat"}), "approval").await;
    s.step("maint approves without password", POST, &format!("{mr}/approve"), Some(&maint_t), json!(null), "approval").await;
    s.step("maint approves wrong password", POST, &format!("{mr}/approve"), Some(&maint_t), json!({"approval_password": "wrong"}), "approval").await;
    s.step("maint approves", POST, &format!("{mr}/approve"), Some(&maint_t), json!({"approval_password": "Password-123-labrat"}), "approval").await;
    s.step("dev approves", POST, &format!("{mr}/approve"), Some(&dev_t), json!({"approval_password": "Password-123-labrat"}), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("approval state approved", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.graphql("graphql approved", Some(&admin), MR_QUERY, vars.clone()).await;

    // A push removes the approvals, although the project keeps them.
    s.step("push", POST, &commits, Some(&author_t), json!({"branch": "feature", "commit_message": "Edit a", "actions": [{"action": "update", "file_path": "a.txt", "content": "a2\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.poll(&format!("{mr}/approvals"), |b| b["approved_by"].as_array().is_some_and(|a| a.is_empty())).await;
    s.step("approval state after push", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("notes after push", GET, &format!("{mr}/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.deliveries("hooks", &mut rx, 5).await;

    // The project's own setting does keep committers from approving.
    s.step("no committer approval", POST, &format!("/projects/{pid}/approvals"), Some(&admin), json!({"merge_requests_disable_committers_approval": true}), "approval").await;
    s.step("approvals as committer now", GET, &format!("{mr}/approvals"), Some(&committer_t), json!(null), "approval").await;
    s.step("approval state now", GET, &format!("{mr}/approval_state"), Some(&admin), json!(null), "approval").await;
    s.step("committer approves now", POST, &format!("{mr}/approve"), Some(&committer_t), json!({"approval_password": "Password-123-labrat"}), "approval").await;
    s.step("project after default branch", GET, &format!("/projects/{pid}"), Some(&admin), json!(null), "project").await;

    // Covered protected branches can't be changed, removed or pushed to.
    let pbs = format!("/projects/{pid}/protected_branches");
    s.step("protected branches", GET, &pbs, Some(&admin), json!(null), "protected_branch").await;
    s.step("allow force push", PATCH, &format!("{pbs}/release"), Some(&admin), json!({"allow_force_push": true}), "protected_branch").await;
    s.step("unprotect release", DELETE, &format!("{pbs}/release"), Some(&admin), json!(null), "protected_branch").await;
    s.step("unprotect main", DELETE, &format!("{pbs}/main"), Some(&admin), json!(null), "protected_branch").await;
    s.step("protect again", POST, &pbs, Some(&admin), json!({"name": "release", "push_access_level": 40}), "protected_branch").await;
    s.step("delete release", DELETE, &format!("/projects/{pid}/repository/branches/release"), Some(&admin), json!(null), "branch").await;
    s.step("default branch", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"default_branch": "release"}), "project").await;
    s.step("default branch unchanged", GET, &format!("/projects/{pid}"), Some(&admin), json!(null), "project").await;
    s.step("commit to main", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Direct", "actions": [{"action": "create", "file_path": "d.txt", "content": "d\n"}]}), "commit").await;
    s.step("commit to release as dev", POST, &commits, Some(&dev_t), json!({"branch": "release", "commit_message": "Direct", "actions": [{"action": "create", "file_path": "d.txt", "content": "d\n"}]}), "commit").await;
    s.step("unprotected branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": "scratch", "ref": "main"}), "branch").await;
    s.step("commit to scratch", POST, &commits, Some(&admin), json!({"branch": "scratch", "commit_message": "Scratch", "actions": [{"action": "create", "file_path": "s.txt", "content": "s\n"}]}), "commit").await;
    let dir = std::env::temp_dir().join(format!("labrat-{}-{}", s.name("pset"), if s.target.recording { "gitlab" } else { "labrat" }));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = s.clone_url(&full, &admin);
    s.git(&dir, &["clone", "-q", &url, "repo"]).await;
    let repo = dir.join("repo");
    std::fs::write(repo.join("g.txt"), "g\n").unwrap();
    s.git(&repo, &["add", "-A"]).await;
    s.git(&repo, &["commit", "-q", "-m", "Git push"]).await;
    s.git_remote_lines("git push main", &repo, &["push", "origin", "main"]).await;
    s.git_remote_lines("git force push main", &repo, &["push", "-f", "origin", "HEAD~1:main"]).await;
    s.step("main after pushes", GET, &format!("/projects/{pid}/repository/branches/main"), Some(&admin), json!(null), "branch").await;
    s.ci_drain().await;
    s.finish();
}
