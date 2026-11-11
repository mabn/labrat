//! Project settings and arguments from docs/gitlab-ref/mr-feature-inventory.md:
//! `remove_source_branch_after_merge` for MRs created through REST and GraphQL,
//! `resolve_outdated_diff_discussions` on and off, discussions after a merge, `closes_issues` /
//! `related_issues`, and GraphQL arguments (`notes(filter:)`, `reference(full:)`,
//! `mergeRequests(sort:, not:, reviewStates:)`).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn mr_settings_behaviour() {
    let mut s = Scenario::new("mr_settings_behaviour").await;
    let admin = s.admin();
    let gpath = s.name("sb");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public", "remove_source_branch_after_merge": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("out");
    let out = s
        .step("outsider", POST, "/users", Some(&admin), json!({"username": username, "name": "Out", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let out_t = s.step("outsider token", POST, &format!("/users/{out}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for b in ["rest", "gql", "keep", "outd", "outd2"] {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "1\n2\n3\n"}]}), "commit").await;
    }
    s.settle().await;
    // remove_source_branch_after_merge: REST, GraphQL, an explicit false.
    s.step("mr rest", POST, &mrs, Some(&admin), json!({"source_branch": "rest", "target_branch": "main", "title": "REST"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.graphql("mr gql", Some(&admin), "mutation($p: ID!) { mergeRequestCreate(input: {projectPath: $p, title: \"GQL\", sourceBranch: \"gql\", targetBranch: \"main\"}) { errors mergeRequest { iid forceRemoveSourceBranch shouldRemoveSourceBranch } } }", json!({"p": full})).await;
    s.poll_mr(pid, 2).await;
    s.step("mr keep", POST, &mrs, Some(&admin), json!({"source_branch": "keep", "target_branch": "main", "title": "Keep", "remove_source_branch": false}), "mr").await;
    s.poll_mr(pid, 3).await;
    for iid in [1, 2, 3] {
        s.step(&format!("merge {iid}"), PUT, &format!("{mrs}/{iid}/merge"), Some(&admin), json!(null), "mr").await;
        s.settle().await;
    }
    s.step("branches after merges", GET, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!(null), "branch").await;
    // Discussions on a merged MR: a non-member comments and replies.
    s.step("outsider comments merged", POST, &format!("{mrs}/1/notes"), Some(&out_t), json!({"body": "After merge"}), "note").await;
    s.step("lock merged", PUT, &format!("{mrs}/1"), Some(&admin), json!({"discussion_locked": true}), "mr").await;
    s.step("outsider comments locked merged", POST, &format!("{mrs}/1/notes"), Some(&out_t), json!({"body": "Locked"}), "note").await;
    s.step("closes_issues", GET, &format!("{mrs}/1/closes_issues"), Some(&admin), json!(null), "issue").await;
    s.step("related_issues", GET, &format!("{mrs}/1/related_issues"), Some(&admin), json!(null), "issue").await;
    // resolve_outdated_diff_discussions on, then off.
    for (iid, branch, on) in [(4, "outd", true), (5, "outd2", false)] {
        s.step(&format!("resolve outdated {on}"), PUT, &format!("/projects/{pid}"), Some(&admin), json!({"resolve_outdated_diff_discussions": on}), "project").await;
        s.step(&format!("mr {branch}"), POST, &mrs, Some(&admin), json!({"source_branch": branch, "target_branch": "main", "title": branch}), "mr").await;
        s.poll_mr(pid, iid).await;
        let m = s.send(GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null)).await;
        let refs = m.body["diff_refs"].clone();
        let file = format!("{branch}.txt");
        let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": file, "new_path": file, "new_line": 2});
        s.step(&format!("thread {branch}"), POST, &format!("{mrs}/{iid}/discussions"), Some(&admin), json!({"body": "Line 2", "position": pos}), "discussion").await;
        s.step(&format!("change {branch}"), POST, &commits, Some(&admin), json!({"branch": branch, "commit_message": "change", "actions": [{"action": "update", "file_path": file, "content": "1\nTWO\n3\n"}]}), "commit").await;
        s.settle().await;
        s.poll_mr(pid, iid).await;
        s.step(&format!("discussions {branch}"), GET, &format!("{mrs}/{iid}/discussions"), Some(&admin), json!(null), "discussion").await;
        s.step(&format!("mr {branch} after"), GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null), "mr").await;
    }
    // GraphQL arguments.
    s.step("note", POST, &format!("{mrs}/4/notes"), Some(&admin), json!({"body": "Plain comment"}), "note").await;
    for (label, q) in [
        ("notes only comments", "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"4\") { notes(filter: ONLY_COMMENTS) { nodes { body system } } } } }"),
        ("notes only activity", "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"4\") { notes(filter: ONLY_ACTIVITY) { nodes { body system } } } } }"),
        ("reference full", "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"4\") { short: reference full: reference(full: true) } } }"),
        ("sort merged_at", "query($p: ID!) { project(fullPath: $p) { mergeRequests(state: all, sort: MERGED_AT_ASC) { nodes { iid } } } }"),
        ("sort title", "query($p: ID!) { project(fullPath: $p) { mergeRequests(state: all, sort: TITLE_DESC) { nodes { iid title } } } }"),
        ("not source branch", "query($p: ID!) { project(fullPath: $p) { mergeRequests(state: all, not: {sourceBranches: [\"rest\", \"gql\"]}) { nodes { iid } } } }"),
        ("review states", "query($p: ID!) { project(fullPath: $p) { mergeRequests(state: all, reviewStates: [UNREVIEWED]) { nodes { iid } } } }"),
    ] {
        s.graphql(label, Some(&admin), q, json!({"p": full})).await;
    }
    s.finish();
}
