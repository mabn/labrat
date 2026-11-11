//! Every scalar field of GraphQL `Project`, `UserCore` (author, reviewer, assignee),
//! `UserMergeRequestInteraction`, `Pipeline`, `DiffRefs` and `DiffStatsSummary`.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const PROJECT: &str = " adminEditPath adminShowPath allowMergeOnSkippedPipeline allowsMultipleMergeRequestAssignees allowsMultipleMergeRequestReviewers archived autocloseReferencedIssues avatarUrl ciConfigPathOrDefault containerRegistryEnabled containerRepositoriesCount createdAt description descriptionHtml editPath forksCount fullPath  httpUrlToRepo id importStatus isForked issuesEnabled jiraImportStatus jobsEnabled lastActivityAt lfsEnabled mergeCommitTemplate    mergeRequestsEnabled mergeRequestsFfOnlyEnabled name nameWithNamespace onlyAllowMergeIfAllDiscussionsAreResolved  onlyAllowMergeIfPipelineSucceeds openIssuesCount openMergeRequestsCount pagesForceHttps pagesUseUniqueDomain path   printingMergeRequestLinkEnabled publicJobs removeSourceBranchAfterMerge  requestAccessEnabled  serviceDeskAddress serviceDeskEnabled sharedRunnersEnabled snippetsEnabled squashCommitTemplate squashReadOnly sshUrlToRepo starCount suggestionCommitMessage topics updatedAt visibility webPath webUrl wikiEnabled";
const USER: &str = "active avatarUrl bio bot commitEmail compositeIdentityEnforced createdAt discord github gitpodEnabled human id jobTitle lastActivityOn linkedin location name organization preferencesGitpodPath profileEnableGitpodPath pronouns publicEmail state twitter type username webPath webUrl";
const INTERACTION: &str = "approved canMerge canUpdate reviewState reviewed";
const PIPELINE: &str = "active beforeSha cancelable child commitPath committedAt complete  configSource coverage createdAt duration failedJobsCount failureReason finishedAt hasManualActions hasScheduledActions id iid latest mergeRequestEventType name path queuedDuration ref refPath refText retryable sha source startedAt status stuck totalJobs trigger triggeredByPath  type updatedAt usesNeeds warnings yamlErrorMessages yamlErrors";

#[tokio::test]
async fn graphql_more_all_scalars() {
    let mut s = Scenario::new("graphql_more_all_scalars").await;
    let admin = s.admin();
    let gpath = s.name("ms");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s
        .step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "description": "The *app*", "topics": ["rust"], "squash_option": "always"}), "project")
        .await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let mut tokens = vec![];
    let mut ids = vec![];
    for (who, level) in [("bob", 30), ("carol", 20)] {
        let username = s.name(who);
        let id = s
            .step(who, POST, "/users", Some(&admin), json!({"username": username, "name": who, "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
            .await
            .id();
        s.step(&format!("{who} profile"), PUT, &format!("/users/{id}"), Some(&admin), json!({"bio": "Writes code", "location": "Kraków", "pronouns": "they/them", "job_title": "Engineer", "organization": "Acme", "linkedin": "someone", "twitter": "someone", "discord": "123456789012345678", "github": "someone"}), "user").await;
        let tok = s.step(&format!("{who} token"), POST, &format!("/users/{id}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
        s.step(&format!("{who} member"), POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": id, "access_level": level}), "user").await;
        tokens.push(tok);
        ids.push(id);
    }
    let c = s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\n"}]}), "commit").await;
    let sha = c.get("id").as_str().unwrap().to_string();
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [ids[0]], "assignee_ids": [ids[1]]}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("status", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "success", "name": "ci", "ref": "feature"}), "status").await;
    s.step("bob approves", POST, &format!("{mrs}/1/approve"), Some(&tokens[0]), json!(null), "approval").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    // Split so each query stays under GitLab's complexity limits.
    let queries = [
        ("project", format!("query($p: ID!) {{ project(fullPath: $p) {{ {PROJECT} }} }}")),
        ("users", format!("query($p: ID!) {{ project(fullPath: $p) {{ mergeRequest(iid: \"1\") {{ author {{ {USER} }} reviewers {{ nodes {{ {USER} mergeRequestInteraction {{ {INTERACTION} }} }} }} assignees {{ nodes {{ {USER} mergeRequestInteraction {{ {INTERACTION} }} }} }} }} }} }}")),
        ("pipeline", format!("query($p: ID!) {{ project(fullPath: $p) {{ mergeRequest(iid: \"1\") {{ headPipeline {{ {PIPELINE} }} diffRefs {{ baseSha headSha startSha }} diffStatsSummary {{ additions changes deletions fileCount }} }} }} }}")),
    ];
    let bob_t = tokens[0].clone();
    let carol_t = tokens[1].clone();
    for (part, q) in &queries {
        s.graphql(&format!("{part} as admin"), Some(&admin), q, json!({"p": full})).await;
        s.graphql(&format!("{part} as reviewer"), Some(&bob_t), q, json!({"p": full})).await;
        s.graphql(&format!("{part} as reporter"), Some(&carol_t), q, json!({"p": full})).await;
    }
    s.finish();
}
