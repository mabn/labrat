//! MR list parameters from docs/gitlab-ref/mr-endpoint-params-audit.md: `my_reaction_emoji`
//! wildcards, milestone special values, validation errors, merged_at ordering, archived
//! projects, deployment filters, empty params, assignee/reviewer
//! validation and negation, date bounds, scopes, label priority with a label filter, `iids` on
//! group lists, and note list filters.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn mr_list_params() {
    let mut s = Scenario::new("mr_list_params").await;
    let admin = s.admin();
    let gpath = s.name("lp");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let arch = s.step("archived project", POST, "/projects", Some(&admin), json!({"name": "old", "namespace_id": gid, "initialize_with_readme": true}), "project").await.id();
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    let past = s.date_in(-5);
    let soon = s.date_in(5);
    let later = s.date_in(20);
    let ms = format!("/projects/{pid}/milestones");
    let started = s.step("milestone started", POST, &ms, Some(&admin), json!({"title": "started", "start_date": past, "due_date": soon}), "milestone").await.id();
    let upcoming = s.step("milestone upcoming", POST, &ms, Some(&admin), json!({"title": "upcoming", "start_date": soon, "due_date": later}), "milestone").await.id();
    let nodates = s.step("milestone no dates", POST, &ms, Some(&admin), json!({"title": "nodates"}), "milestone").await.id();
    for (l, prio) in [("p1", 1), ("p2", 2)] {
        s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": l, "color": "#ff0000", "priority": prio}), "label").await;
    }
    let commits = format!("/projects/{pid}/repository/commits");
    for b in ["a", "b", "c", "d", "e"] {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
    }
    s.step("archived commit", POST, &format!("/projects/{arch}/repository/commits"), Some(&admin), json!({"branch": "z", "start_branch": "main", "commit_message": "z", "actions": [{"action": "create", "file_path": "z.txt", "content": "z\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let specs: Vec<Value> = vec![
        json!({"source_branch": "a", "target_branch": "main", "title": "A", "milestone_id": started, "labels": "p1"}),
        json!({"source_branch": "b", "target_branch": "main", "title": "B", "milestone_id": upcoming, "labels": "p1,p2", "assignee_ids": [bob]}),
        json!({"source_branch": "c", "target_branch": "main", "title": "C", "milestone_id": nodates, "reviewer_ids": [bob]}),
        json!({"source_branch": "d", "target_branch": "main", "title": "D"}),
    ];
    for (i, spec) in specs.into_iter().enumerate() {
        s.step("mr", POST, &mrs, Some(&admin), spec, "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("bob mr", POST, &mrs, Some(&bob_t), json!({"source_branch": "e", "target_branch": "main", "title": "E by bob"}), "mr").await;
    s.poll_mr(pid, 5).await;
    s.step("archived mr", POST, &format!("/projects/{arch}/merge_requests"), Some(&admin), json!({"source_branch": "z", "target_branch": "main", "title": format!("Zarchived {gpath}")}), "mr").await;
    s.poll_mr(arch, 1).await;
    s.step("archive", POST, &format!("/projects/{arch}/archive"), Some(&admin), json!(null), "project").await;
    s.step("award", POST, &format!("{mrs}/1/award_emoji"), Some(&admin), json!({"name": "thumbsup"}), "award").await;
    s.step("merge d", PUT, &format!("{mrs}/4/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;

    let cases: Vec<(&str, Value)> = vec![
        ("emoji None", json!({"my_reaction_emoji": "None"})),
        ("emoji Any", json!({"my_reaction_emoji": "Any"})),
        ("emoji any", json!({"my_reaction_emoji": "any"})),
        ("emoji ANY", json!({"my_reaction_emoji": "ANY"})),
        ("emoji thumbsup", json!({"my_reaction_emoji": "thumbsup"})),
        ("milestone #started", json!({"milestone": "#started"})),
        ("milestone #upcoming", json!({"milestone": "#upcoming"})),
        ("milestone Started", json!({"milestone": "Started"})),
        ("milestone No Milestone", json!({"milestone": "No Milestone"})),
        ("milestone Any Milestone", json!({"milestone": "Any Milestone"})),
        ("milestone None", json!({"milestone": "None"})),
        ("milestone empty", json!({"milestone": ""})),
        ("not milestone #started", json!({"not[milestone]": "#started"})),
        ("approved maybe", json!({"approved": "maybe"})),
        ("approved yes", json!({"approved": "yes"})),
        ("view bogus", json!({"view": "bogus"})),
        ("merged_at asc", json!({"state": "all", "order_by": "merged_at", "sort": "asc"})),
        ("merged_at desc", json!({"state": "all", "order_by": "merged_at", "sort": "desc"})),
        ("environment", json!({"environment": "production"})),
        ("deployed_after", json!({"deployed_after": "2020-01-01T00:00:00Z"})),
        ("created_after empty", json!({"created_after": ""})),
        ("author_id empty", json!({"author_id": ""})),
        ("merge_user_id empty", json!({"merge_user_id": ""})),
        ("target_branch empty", json!({"target_branch": ""})),
        ("source_branch empty", json!({"source_branch": ""})),
        ("author_username empty", json!({"author_username": ""})),
        ("assignee_id abc", json!({"assignee_id": "abc"})),
        ("assignee id and username", json!({"assignee_id": bob, "assignee_username": username})),
        ("assignee_username comma", json!({"assignee_username": format!("{username},x")})),
        ("not reviewer_username ghost", json!({"not[reviewer_username]": "ghost-user-x"})),
        ("not reviewer_id missing", json!({"not[reviewer_id]": 999999})),
        ("created_after past", json!({"created_after": "2000-01-01"})),
        ("created_before future", json!({"created_before": "2100-01-01"})),
        ("created_before past", json!({"created_before": "2000-01-01"})),
        ("created_after yesterday", json!({"created_after": "yesterday"})),
        ("updated_after offset", json!({"updated_after": "2000-01-01T00:00:00+02:00"})),
        ("label priority with label", json!({"labels": "p1", "order_by": "label_priority", "sort": "asc"})),
        ("assignee_username array", json!({"assignee_username[]": [username.clone(), "x".to_string()]})),
    ];
    for (label, q) in cases {
        s.step(label, GET, &mrs, Some(&admin), q, "mr").await;
    }
    for (label, q) in [
        ("bob created-by-me", json!({"scope": "created_by_me"})),
        ("bob assigned-to-me", json!({"scope": "assigned_to_me"})),
        ("bob reviews_for_me", json!({"scope": "reviews_for_me"})),
        ("bob created-by-me dashed", json!({"scope": "created-by-me"})),
    ] {
        s.step(label, GET, &mrs, Some(&bob_t), q, "mr").await;
    }
    s.step("anonymous project list", GET, &mrs, None, json!(null), "mr").await;
    s.step("group iids", GET, &format!("/groups/{gid}/merge_requests"), Some(&admin), json!({"iids[]": [1], "state": "all"}), "mr").await;
    s.step("group non_archived false", GET, &format!("/groups/{gid}/merge_requests"), Some(&admin), json!({"non_archived": false, "state": "all"}), "mr").await;
    s.step("global non_archived", GET, "/merge_requests", Some(&admin), json!({"scope": "all", "non_archived": true, "search": format!("Zarchived {gpath}")}), "mr").await;
    s.step("global archived included", GET, "/merge_requests", Some(&admin), json!({"scope": "all", "non_archived": false, "search": format!("Zarchived {gpath}")}), "mr").await;
    // Note list filters.
    s.step("note", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": "hello"}), "note").await;
    for (label, q) in [
        ("notes only comments", json!({"activity_filter": "only_comments"})),
        ("notes only activity", json!({"activity_filter": "only_activity"})),
        ("notes bogus filter", json!({"activity_filter": "bogus"})),
        ("notes updated_at asc", json!({"order_by": "updated_at", "sort": "asc"})),
    ] {
        s.step(label, GET, &format!("{mrs}/1/notes"), Some(&admin), q, "note").await;
    }
    s.finish();
}
