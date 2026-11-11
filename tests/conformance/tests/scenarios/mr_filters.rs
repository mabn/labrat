//! More MR list filters (negations, approvals, merge user), priority orderings, and commit lookups.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn mr_more_filters() {
    let mut s = Scenario::new("mr_more_filters").await;
    let admin = s.admin();
    let gpath = s.name("mf");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
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
    let specs: [(&str, &str, Value); 4] = [
        ("b1", "One", json!({"labels": "p2", "milestone_id": m_late, "assignee_ids": [bob]})),
        ("b2", "Two", json!({"labels": "p1", "milestone_id": m_soon, "reviewer_ids": [bob]})),
        ("b3", "Three", json!({})),
        ("b4", "Four", json!({"labels": "p1,p2"})),
    ];
    let mut shas = vec![];
    for (i, (branch, title, extra)) in specs.iter().enumerate() {
        let c = s
            .step("commit", POST, &commits, Some(&admin), json!({"branch": branch, "start_branch": "main", "commit_message": format!("Commit {branch}"), "actions": [{"action": "create", "file_path": format!("{branch}.txt"), "content": "x\n"}]}), "commit")
            .await;
        shas.push(c.get("id").as_str().unwrap().to_string());
        s.settle().await;
        let mut body = json!({"source_branch": branch, "target_branch": "main", "title": title});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        s.step("mr", POST, &mrs, Some(&admin), body, "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("upvote two", POST, &format!("{mrs}/2/award_emoji"), Some(&bob_t), json!({"name": "thumbsup"}), "award").await;
    s.step("upvote two again", POST, &format!("{mrs}/2/award_emoji"), Some(&admin), json!({"name": "thumbsup"}), "award").await;
    s.step("upvote three", POST, &format!("{mrs}/3/award_emoji"), Some(&admin), json!({"name": "thumbsup"}), "award").await;
    s.step("bob approves one", POST, &format!("{mrs}/1/approve"), Some(&bob_t), json!(null), "approval").await;
    s.step("admin approves one", POST, &format!("{mrs}/1/approve"), Some(&admin), json!(null), "approval").await;
    s.step("admin approves three", POST, &format!("{mrs}/3/approve"), Some(&admin), json!(null), "approval").await;
    s.settle().await;
    s.step("bob merges four", PUT, &format!("{mrs}/4/merge"), Some(&bob_t), json!(null), "mr").await;
    s.settle().await;

    let queries: Vec<(&str, Value)> = vec![
        ("not author_username", json!({"not[author_username]": "claude"})),
        ("not assignee_id", json!({"not[assignee_id]": bob})),
        ("not reviewer_username", json!({"not[reviewer_username]": username})),
        ("not milestone", json!({"not[milestone]": "late"})),
        ("not my_reaction_emoji", json!({"not[my_reaction_emoji]": "thumbsup"})),
        ("source_project_id", json!({"source_project_id": pid})),
        ("merge_user_username", json!({"merge_user_username": username})),
        ("merge_user_id", json!({"merge_user_id": bob})),
        ("order popularity", json!({"order_by": "popularity"})),
        ("order popularity asc", json!({"order_by": "popularity", "sort": "asc"})),
        ("order milestone_due", json!({"order_by": "milestone_due", "sort": "asc"})),
        ("order label_priority", json!({"order_by": "label_priority"})),
        ("order priority", json!({"order_by": "priority"})),
    ];
    for (label, q) in queries {
        s.step(&format!("list {label}"), GET, &mrs, Some(&admin), q, "mr").await;
    }
    s.step("commit mrs", GET, &format!("{commits}/{}/merge_requests", shas[0]), Some(&admin), json!(null), "mr").await;
    s.step("merged commit mrs", GET, &format!("{commits}/{}/merge_requests", shas[3]), Some(&admin), json!({"state": "merged"}), "mr").await;
    s.step("changes unidiff", GET, &format!("{mrs}/1/changes"), Some(&admin), json!({"unidiff": true}), "mr").await;
    let v = s.send(GET, &format!("{mrs}/1/versions"), Some(&admin), json!(null)).await;
    let vid = v.body[0]["id"].as_i64().unwrap_or(0);
    s.step("version unidiff", GET, &format!("{mrs}/1/versions/{vid}"), Some(&admin), json!({"unidiff": true}), "version").await;
    s.finish();
}
