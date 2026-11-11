//! Webhook triggers beyond the main flow: time tracking, re-requested reviews, emoji from
//! GraphQL and quick actions, push hook limits and regex branch filters.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn webhooks_more() {
    let mut s = Scenario::new("webhooks_more").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    if s.target.recording {
        tokio::time::sleep(std::time::Duration::from_secs(65)).await;
    }
    let gpath = s.name("wm");
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
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("commit", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Add a", "actions": [{"action": "create", "file_path": "a.txt", "content": "one\ntwo\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "reviewer_ids": [bob]}), "mr").await;
    s.poll_mr(pid, 1).await;
    let hooks = format!("/projects/{pid}/hooks");
    s.step("bad regex filter", POST, &hooks, Some(&admin), json!({"url": format!("{}/hook", rx.url), "push_events": true, "branch_filter_strategy": "regex", "push_events_branch_filter": "feat(ure"}), "hook").await;
    s.step(
        "hook",
        POST,
        &hooks,
        Some(&admin),
        json!({"url": format!("{}/hook", rx.url), "push_events": true, "merge_requests_events": true, "emoji_events": true, "note_events": false,
               "branch_filter_strategy": "regex", "push_events_branch_filter": "^(feat|fix)-[0-9]+$"}),
        "hook",
    )
    .await;

    // Time tracking sends merge request `update` hooks.
    s.step("estimate", POST, &format!("{mrs}/1/time_estimate"), Some(&admin), json!({"duration": "2h"}), "mr").await;
    s.deliveries("estimate hook", &mut rx, 1).await;
    s.step("spend", POST, &format!("{mrs}/1/add_spent_time"), Some(&admin), json!({"duration": "1h"}), "mr").await;
    s.deliveries("spend hook", &mut rx, 1).await;
    s.step("reset spent", POST, &format!("{mrs}/1/reset_spent_time"), Some(&admin), json!(null), "mr").await;
    s.deliveries("reset spent hook", &mut rx, 1).await;

    // Re-requesting a review that was given.
    s.step("bob approves", POST, &format!("{mrs}/1/approve"), Some(&bob_t), json!(null), "approval").await;
    s.deliveries("approval hooks", &mut rx, 2).await;
    let gid_bob = format!("gid://gitlab/User/{bob}");
    s.graphql("rereview", Some(&admin), "mutation($path: ID!, $user: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $path, iid: \"1\", userId: $user}) { errors } }", json!({"path": full, "user": gid_bob})).await;
    s.deliveries("rereview hook", &mut rx, 1).await;

    // Emoji from GraphQL and from a quick action.
    let mr_gid = {
        let r = s.graphql("mr gid", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { id } } }", json!({"path": full})).await;
        r.body["data"]["project"]["mergeRequest"]["id"].as_str().unwrap_or_default().to_string()
    };
    s.graphql("graphql award", Some(&admin), "mutation($id: AwardableID!) { awardEmojiAdd(input: {awardableId: $id, name: \"rocket\"}) { errors } }", json!({"id": mr_gid})).await;
    s.deliveries("graphql award hook", &mut rx, 1).await;
    s.step("react", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "/react :tada:"}), "note").await;
    s.deliveries("react hook", &mut rx, 1).await;

    // Regex branch filter and the limit of three push hooks per push.
    for b in ["feat-1", "docs-1"] {
        s.step("branch", POST, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!({"branch": b, "ref": "main"}), "branch").await;
    }
    s.deliveries("regex filtered pushes", &mut rx, 1).await;
    s.finish();
}
