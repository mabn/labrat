//! GitLab's GraphQL query complexity: scores of typical merge request client queries (each also
//! asks for `queryComplexity { score limit }`), the per-user limits, and the errors for queries
//! over the limit or too deep.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_complexity() {
    let mut s = Scenario::new("graphql_complexity").await;
    let admin = s.admin();
    let gpath = s.name("cx");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public"}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.step("commit", POST, &format!("/projects/{pid}/repository/commits"), Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "a\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;

    let qc = "queryComplexity { score limit }";
    let mr = "project(fullPath: $p) { mergeRequest(iid: \"1\")";
    let cases: Vec<(&str, String)> = vec![
        ("bare", format!("query {{ {qc} }}")),
        ("project id", format!("query($p: ID!) {{ {qc} project(fullPath: $p) {{ id name }} }}")),
        ("mr scalars", format!("query($p: ID!) {{ {qc} {mr} {{ title description state draft }} }} }}")),
        ("mr html", format!("query($p: ID!) {{ {qc} {mr} {{ titleHtml descriptionHtml }} }} }}")),
        ("mrs first 5", format!("query($p: ID!) {{ {qc} project(fullPath: $p) {{ mergeRequests(first: 5) {{ nodes {{ iid title }} pageInfo {{ hasNextPage endCursor }} }} }} }}")),
        ("mrs default", format!("query($p: ID!) {{ {qc} project(fullPath: $p) {{ mergeRequests {{ nodes {{ iid title }} pageInfo {{ hasNextPage endCursor }} }} }} }}")),
        ("mrs filtered", format!("query($p: ID!) {{ {qc} project(fullPath: $p) {{ mergeRequests(state: opened, sort: CREATED_DESC, search: \"Add\", first: 20) {{ count nodes {{ iid }} }} }} }}")),
        ("mrs iids", format!("query($p: ID!) {{ {qc} project(fullPath: $p) {{ mergeRequests(iids: [\"1\", \"2\"]) {{ nodes {{ iid }} }} }} }}")),
        ("notes", format!("query($p: ID!) {{ {qc} {mr} {{ notes {{ nodes {{ body author {{ username }} }} }} }} }} }}")),
        ("notes first 5", format!("query($p: ID!) {{ {qc} {mr} {{ notes(first: 5) {{ nodes {{ body author {{ username }} }} }} }} }} }}")),
        ("discussions", format!("query($p: ID!) {{ {qc} {mr} {{ discussions {{ nodes {{ id resolved notes {{ nodes {{ body position {{ newLine newPath }} }} }} }} }} }} }} }}")),
        ("reviewers", format!("query($p: ID!) {{ {qc} {mr} {{ reviewers {{ nodes {{ username mergeRequestInteraction {{ reviewState approved }} }} }} assignees {{ nodes {{ username }} }} author {{ username }} }} }} }}")),
        ("permissions", format!("query($p: ID!) {{ {qc} {mr} {{ userPermissions {{ canMerge canApprove pushToSourceBranch removeSourceBranch adminMergeRequest updateMergeRequest createNote readMergeRequest }} }} }} }}")),
        ("approvals labels participants", format!("query($p: ID!) {{ {qc} {mr} {{ approvedBy {{ nodes {{ username }} }} labels {{ nodes {{ title }} }} participants {{ nodes {{ username }} }} }} }} }}")),
        ("pipeline diffs commits", format!("query($p: ID!) {{ {qc} {mr} {{ headPipeline {{ status }} diffStats {{ path additions }} diffStatsSummary {{ additions }} commits {{ nodes {{ sha title }} }} }} }} }}")),
        ("mergeability", format!("query($p: ID!) {{ {qc} {mr} {{ mergeable mergeStatusEnum detailedMergeStatus conflicts shouldBeRebased mergeabilityChecks {{ identifier status }} }} }} }}")),
        ("current user", format!("query {{ {qc} currentUser {{ username assignedMergeRequests(first: 10) {{ nodes {{ iid }} }} reviewRequestedMergeRequests {{ nodes {{ iid }} }} }} }}")),
        ("repository", format!("query($p: ID!) {{ {qc} project(fullPath: $p) {{ repository {{ rootRef tree {{ blobs {{ nodes {{ name }} }} }} blobs(paths: [\"README.md\"]) {{ nodes {{ rawTextBlob }} }} }} }} }}")),
        ("typename and fragment", format!("query($p: ID!) {{ {qc} {mr} {{ __typename ...F }} }} }} fragment F on MergeRequest {{ iid title }}")),
        ("duplicate fields", format!("query($p: ID!) {{ {qc} {mr} {{ title title t2: title }} }} }}")),
    ];
    let vars = json!({"p": full});
    for (label, q) in &cases {
        s.graphql(&format!("{label} as bob"), Some(&bob_t), q, vars.clone()).await;
    }
    s.graphql("bare as admin", Some(&admin), &format!("query {{ {qc} }}"), json!({})).await;
    s.graphql("bare anonymous", None, &format!("query {{ {qc} }}"), json!({})).await;
    s.graphql("project anonymous", None, &format!("query($p: ID!) {{ {qc} project(fullPath: $p) {{ id }} }}"), vars.clone()).await;
    // A mutation and a query over the limit.
    s.graphql(
        "mutation",
        Some(&bob_t),
        "mutation($p: ID!) { mergeRequestSetDraft(input: {projectPath: $p, iid: \"1\", draft: false}) { errors mergeRequest { title } } }",
        vars.clone(),
    )
    .await;
    // `Repository.branchNames` costs 170: two are over every limit, one is over the anonymous one.
    let bn = |alias: &str| format!("{alias}: branchNames(searchPattern: \"mai*\", offset: 0, limit: 5)");
    let over_all = format!("query($p: ID!) {{ project(fullPath: $p) {{ repository {{ {} {} }} }} }}", bn("b1"), bn("b2"));
    let over_anon = format!(
        "query($p: ID!) {{ {qc} project(fullPath: $p) {{ repository {{ {} }} mergeRequests {{ nodes {{ notes {{ nodes {{ body }} }} participants {{ nodes {{ username }} }} }} }} }} }}",
        bn("b1")
    );
    s.graphql("over all as bob", Some(&bob_t), &over_all, vars.clone()).await;
    s.graphql("over all as admin", Some(&admin), &over_all, vars.clone()).await;
    s.graphql("over anonymous limit as bob", Some(&bob_t), &over_anon, vars.clone()).await;
    s.graphql("over anonymous limit anonymous", None, &over_anon, vars.clone()).await;
    // Depth limits: 15 anonymous, 20 signed in.
    let nest = |levels: usize| {
        let mut q = String::from("username");
        for _ in 0..levels {
            q = format!("authoredMergeRequests {{ nodes {{ author {{ {q} }} }} }}");
        }
        format!("query($p: ID!) {{ project(fullPath: $p) {{ mergeRequest(iid: \"1\") {{ author {{ {q} }} }} }} }}")
    };
    s.graphql("depth 16 as bob", Some(&bob_t), &nest(4), vars.clone()).await;
    s.graphql("depth 16 anonymous", None, &nest(4), vars.clone()).await;
    s.graphql("depth 22 as bob", Some(&bob_t), &nest(6), vars.clone()).await;
    s.finish();
}
