//! Every `ProjectPermissions` and `MergeRequestPermissions` field for each role (admin,
//! Maintainer, Developer, Reporter, Guest, anonymous on a public project), plus
//! `CurrentUser.admin` and `Commit.parentSha`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

const PROJECT_PERMS: &str = "  adminAllResources adminIssue adminOperations  adminProject adminRemoteMirror adminWiki  archiveProject canLeave changeNamespace changeVisibilityLevel createDeployment createDesign createIssue createLabel createMergeRequestFrom createMergeRequestIn createPages  createPipeline createPipelineSchedule createSnippet createWiki createWorkItem destroyDesign destroyPages destroyWiki downloadCode downloadWikiCode forkProject  importIssues  moveDesign pushCode pushToDeleteProtectedBranch   readCommitStatus readCrmContact readCrmOrganization readCycleAnalytics readDesign readEnvironment readMergeRequest readPagesContent  readProject readProjectMember  readWiki removeForkProject removePages removeProject renameProject requestAccess updateDesign updatePages updateWiki uploadFile viewEditPage";
const MR_PERMS: &str = "adminMergeRequest canApprove canMerge cherryPickOnCurrentMergeRequest createNote pushToSourceBranch readMergeRequest removeSourceBranch revertOnCurrentMergeRequest updateMergeRequest";

#[tokio::test]
async fn graphql_permissions_all() {
    let mut s = Scenario::new("graphql_permissions_all").await;
    let admin = s.admin();
    let gpath = s.name("pa");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public"}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut tokens = vec![("admin".to_string(), Some(admin.clone()))];
    for (who, level) in [("maint", 40), ("dev", 30), ("rep", 20), ("guest", 10)] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} member"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": level}), "user").await;
        tokens.push((who.to_string(), Some(tok)));
    }
    tokens.push(("anonymous".to_string(), None));
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a"}), "mr").await;
    s.poll_mr(pid, 1).await;
    let q = format!("query($p: ID!) {{ currentUser {{ admin }} project(fullPath: $p) {{ userPermissions {{ {PROJECT_PERMS} }} mergeRequest(iid: \"1\") {{ userPermissions {{ {MR_PERMS} }} commits {{ nodes {{ parentSha }} }} }} }} }}");
    for (who, tok) in &tokens {
        s.graphql(&format!("as {who}"), tok.as_deref(), &q, json!({"p": full})).await;
    }
    s.finish();
}
