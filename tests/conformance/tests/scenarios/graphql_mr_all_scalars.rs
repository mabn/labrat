//! Every argument-free scalar and enum field of GraphQL `MergeRequest`, across MRs in distinct
//! states, so no field is left answering a placeholder.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

const FIELDS: &str = "allowCollaboration allowsMultipleAssignees allowsMultipleReviewers   approved autoMergeEnabled autoMergeStrategy availableAutoMergeStrategies closedAt commitCount conflicts createdAt defaultMergeCommitMessage defaultSquashCommitMessage description descriptionHtml detailedMergeStatus diffHeadSha discussionLocked divergedFromTargetBranch downvotes draft forceRemoveSourceBranch hasCi  hidden humanTimeEstimate humanTotalTimeSpent id iid inProgressMergeCommitSha mergeAfter mergeCommitSha mergeError mergeOngoing mergeStatusEnum mergeable mergeableDiscussionsState mergedAt name preparedAt projectId rebaseCommitSha rebaseInProgress reference resolvableDiscussionsCount resolvedDiscussionsCount retargeted  shouldBeRebased shouldRemoveSourceBranch sourceBranch sourceBranchExists sourceBranchProtected sourceProjectId squash squashOnMerge squashReadOnly state subscribed supportsLockOnMerge targetBranch targetBranchExists targetBranchPath targetProjectId timeEstimate title titleHtml totalTimeSpent updatedAt upvotes userDiscussionsCount userNotesCount webPath webUrl";

#[tokio::test]
async fn graphql_mr_all_scalars() {
    let mut s = Scenario::new("graphql_mr_all_scalars").await;
    let admin = s.admin();
    let gpath = s.name("as");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("shared", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Shared", "actions": [{"action": "create", "file_path": "shared.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    for (b, file, content) in [("open", "o.txt", "o\n"), ("draft", "shared.txt", "draft side\n"), ("merged", "m.txt", "m\n"), ("closed", "c.txt", "c\n"), ("gone", "g.txt", "g\n"), ("behind", "b.txt", "b\n")] {
        let action = if file == "shared.txt" { "update" } else { "create" };
        s.step(&format!("{b} commit"), POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("{b} change\n\nLonger body for {b}."), "actions": [{"action": action, "file_path": file, "content": content}]}), "commit").await;
    }
    s.settle().await;
    s.step("protect open", POST, &format!("/projects/{pid}/protected_branches"), Some(&admin), json!({"name": "open"}), "protected_branch").await;
    let titles = [("open", "Open **one**"), ("draft", "Draft: Second"), ("merged", "Merged"), ("closed", "Closed"), ("gone", "Gone"), ("behind", "Behind")];
    for (i, (b, title)) in titles.iter().enumerate() {
        s.step(&format!("{b} mr"), POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": title, "description": "Some *text* for !1", "squash": *b == "merged"}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    // State changes.
    s.step("estimate", POST, &format!("{mrs}/1/time_estimate"), Some(&admin), json!({"duration": "3h30m"}), "mr").await;
    s.step("spend", POST, &format!("{mrs}/1/add_spent_time"), Some(&admin), json!({"duration": "45m"}), "mr").await;
    s.step("upvote", POST, &format!("{mrs}/1/award_emoji"), Some(&admin), json!({"name": "thumbsup"}), "award").await;
    s.step("thread", POST, &format!("{mrs}/1/discussions"), Some(&admin), json!({"body": "A thread"}), "discussion").await;
    s.step("conflicting main", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Main side", "actions": [{"action": "update", "file_path": "shared.txt", "content": "main side\n"}]}), "commit").await;
    s.settle().await;
    s.step("merge", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!({"should_remove_source_branch": true, "squash": true}), "mr").await;
    s.step("close", PUT, &format!("{mrs}/4"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.step("delete gone branch", DELETE, &format!("/projects/{pid}/repository/branches/gone"), Some(&admin), json!(null), "branch").await;
    s.step("ff only", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_method": "ff"}), "project").await;
    s.settle().await;
    for iid in 1..=6 {
        s.poll_mr(pid, iid).await;
    }
    let q = format!("query($p: ID!, $iid: String!) {{ project(fullPath: $p) {{ mergeRequest(iid: $iid) {{ {FIELDS} }} }} }}");
    for (i, (b, _)) in titles.iter().enumerate() {
        s.graphql(&format!("{b} fields"), Some(&admin), &q, json!({"p": full, "iid": (i + 1).to_string()})).await;
    }
    s.finish();
}
