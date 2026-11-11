//! `Project.mergeRequests` / `Group.mergeRequests` arguments: filters, negation, `or`, sorts.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn graphql_mr_filters() {
    let mut s = Scenario::new("graphql_mr_filters").await;
    let admin = s.admin();
    let gpath = s.name("gmf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
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
    s.settle().await;
    for (name, prio) in [("p1", 1), ("p2", 2)] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": name, "color": "#00ff00", "priority": prio}), "label").await;
    }
    let ms = format!("/projects/{pid}/milestones");
    let m_late = s.step("late milestone", POST, &ms, Some(&admin), json!({"title": "late", "due_date": "2031-01-01"}), "milestone").await.id();
    let m_soon = s.step("soon milestone", POST, &ms, Some(&admin), json!({"title": "soon", "due_date": "2030-01-01"}), "milestone").await.id();
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let specs: [(&str, &str, Value); 5] = [
        ("b1", "One", json!({"labels": "p2", "milestone_id": m_late, "assignee_ids": [bob]})),
        ("b2", "Draft: Two", json!({"labels": "p1", "milestone_id": m_soon, "reviewer_ids": [bob]})),
        ("b3", "Three", json!({"reviewer_ids": [bob]})),
        ("b4", "Four", json!({"labels": "p1,p2"})),
        ("b5", "five", json!({"description": "find me"})),
    ];
    for (i, (branch, title, extra)) in specs.iter().enumerate() {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": branch, "start_branch": "main", "commit_message": format!("Commit {branch}"), "actions": [{"action": "create", "file_path": format!("{branch}.txt"), "content": "x\n"}]}), "commit").await;
        s.settle().await;
        let mut body = json!({"source_branch": branch, "target_branch": "main", "title": title});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        s.step("mr", POST, &mrs, Some(&admin), body, "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("upvote two", POST, &format!("{mrs}/2/award_emoji"), Some(&bob_t), json!({"name": "thumbsup"}), "award").await;
    s.step("upvote three", POST, &format!("{mrs}/3/award_emoji"), Some(&admin), json!({"name": "thumbsup"}), "award").await;
    s.step("bob approves one", POST, &format!("{mrs}/1/approve"), Some(&bob_t), json!(null), "approval").await;
    s.settle().await;
    let set = |name: &str, iid: i64| format!("mutation($path: ID!) {{ {name}(input: {{projectPath: $path, iid: \"{iid}\"}}) {{ errors }} }}");
    s.graphql("bob requests changes on three", Some(&bob_t), &set("mergeRequestRequestChanges", 3), json!({"path": full})).await;
    s.settle().await;
    s.step("bob unsubscribes two", POST, &format!("{mrs}/2/unsubscribe"), Some(&bob_t), json!(null), "mr").await;
    s.step("bob subscribes five", POST, &format!("{mrs}/5/subscribe"), Some(&bob_t), json!(null), "mr").await;
    s.step("bob merges four", PUT, &format!("{mrs}/4/merge"), Some(&bob_t), json!(null), "mr").await;
    s.settle().await;
    s.step("close five", PUT, &format!("{mrs}/5"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.settle().await;

    let q = |args: &str| {
        let a = if args.is_empty() { String::new() } else { format!("({args})") };
        format!("query($path: ID!) {{ project(fullPath: $path) {{ mergeRequests{a} {{ count nodes {{ iid }} pageInfo {{ hasNextPage hasPreviousPage }} }} }} }}")
    };
    let cases: Vec<(&str, String)> = vec![
        ("default", String::new()),
        ("state merged", "state: merged".into()),
        ("state opened", "state: opened".into()),
        ("state all", "state: all".into()),
        ("iids", "iids: [\"1\", \"3\"]".into()),
        ("empty iids", "iids: []".into()),
        ("source branches", "sourceBranches: [\"b1\", \"b2\"]".into()),
        ("target branches", "targetBranches: [\"main\"]".into()),
        ("draft", "draft: true".into()),
        ("not draft", "draft: false".into()),
        ("author", "authorUsername: \"claude\"".into()),
        ("assignee", format!("assigneeUsername: \"{username}\"")),
        ("assignees", format!("assigneeUsernames: [\"{username}\"]")),
        ("assignee none", "assigneeWildcardId: NONE".into()),
        ("assignee any", "assigneeWildcardId: ANY".into()),
        ("assignee exclusive", format!("assigneeUsername: \"{username}\", assigneeWildcardId: ANY")),
        ("reviewer", format!("reviewerUsername: \"{username}\"")),
        ("reviewer none", "reviewerWildcardId: NONE".into()),
        ("labels", "labels: [\"p1\"]".into()),
        ("label name", "labelName: [\"p1\", \"p2\"]".into()),
        ("milestone", "milestoneTitle: \"late\"".into()),
        ("milestone none", "milestoneWildcardId: NONE".into()),
        ("milestone any", "milestoneWildcardId: ANY".into()),
        ("my reaction", "myReactionEmoji: \"thumbsup\"".into()),
        ("approved by", format!("approvedBy: [\"{username}\"]")),
        ("merged by", format!("mergedBy: \"{username}\"")),
        ("created before", "createdBefore: \"2000-01-01T00:00:00Z\"".into()),
        ("created after", "createdAfter: \"2000-01-01T00:00:00Z\"".into()),
        ("merged after", "mergedAfter: \"2000-01-01T00:00:00Z\"".into()),
        ("updated after future", "updatedAfter: \"2099-01-01T00:00:00Z\"".into()),
        ("closed after", "closedAfter: \"2000-01-01T00:00:00Z\"".into()),
        ("review state", "reviewState: REQUESTED_CHANGES".into()),
        ("review states", "reviewStates: [UNREVIEWED, REVIEWED]".into()),
        ("subscribed", "subscribed: EXPLICITLY_SUBSCRIBED".into()),
        ("unsubscribed", "subscribed: EXPLICITLY_UNSUBSCRIBED".into()),
        ("search", "search: \"find\"".into()),
        ("search title", "search: \"three\", in: [TITLE]".into()),
        ("not labels", "not: {labels: [\"p1\"]}".into()),
        ("not author", "not: {authorUsername: \"claude\"}".into()),
        ("not reviewer", format!("not: {{reviewerUsername: \"{username}\"}}")),
        ("not milestone", "not: {milestoneTitle: \"late\"}".into()),
        ("not source branches", "not: {sourceBranches: [\"b1\"]}".into()),
        ("or assignees", format!("or: {{assigneeUsernames: [\"{username}\", \"claude\"]}}")),
        ("sort created asc", "sort: CREATED_ASC".into()),
        // Not `UPDATED_ASC`: notes touch MRs only after a minute (`ThrottledTouch`), so the
        // order depends on how long the recording took.
        ("sort lowercase", "sort: created_asc".into()),
        ("sort merged at desc", "sort: MERGED_AT_DESC".into()),
        ("sort closed at asc", "sort: CLOSED_AT_ASC".into()),
        ("sort title asc", "sort: TITLE_ASC".into()),
        ("sort title desc", "sort: TITLE_DESC".into()),
        ("sort popularity desc", "sort: POPULARITY_DESC".into()),
        ("sort popularity asc", "sort: POPULARITY_ASC".into()),
        ("sort priority asc", "sort: PRIORITY_ASC".into()),
        ("sort label priority desc", "sort: LABEL_PRIORITY_DESC".into()),
        ("sort milestone due asc", "sort: MILESTONE_DUE_ASC".into()),
        ("first two by title", "sort: TITLE_ASC, first: 2".into()),
        ("first two by created", "sort: CREATED_ASC, first: 2".into()),
    ];
    for (label, args) in cases {
        s.graphql(&format!("project {label}"), Some(&admin), &q(&args), json!({"path": full})).await;
    }
    let gq = |args: &str| format!("query($path: ID!) {{ group(fullPath: $path) {{ mergeRequests{args} {{ count nodes {{ iid project {{ name }} }} }} }} }}");
    s.graphql("group default", Some(&admin), &gq(""), json!({"path": gpath})).await;
    s.graphql("group merged", Some(&admin), &gq("(state: merged)"), json!({"path": gpath})).await;
    s.graphql("group no subgroups", Some(&admin), &gq("(includeSubgroups: false, labels: [\"p1\"])"), json!({"path": gpath})).await;
    s.graphql("group as bob", Some(&bob_t), &gq("(reviewerUsername: \"claude\")"), json!({"path": gpath})).await;
    s.finish();
}
