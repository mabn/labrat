//! The search API for merge-request-related scopes (basic search, no Elasticsearch).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn search() {
    let mut s = Scenario::new("search").await;
    let admin = s.admin();
    let gpath = s.name("sr");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let word = s.name("zebra");
    for (i, (b, title, desc)) in [("b1", format!("Fix the {word} bug"), "plain"), ("b2", "Other change".to_string(), "mentions the zebra-free code"), ("b3", format!("Draft: {word} draft"), "x")].iter().enumerate() {
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("Commit about {word} {i}"), "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": format!("{word}\n")}]}), "commit").await;
        s.settle().await;
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": title, "description": desc}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("note", POST, &format!("{mrs}/2/notes"), Some(&admin), json!({"body": format!("The {word} is here")}), "note").await;
    s.step("milestone", POST, &format!("/projects/{pid}/milestones"), Some(&admin), json!({"title": format!("{word} milestone")}), "milestone").await;
    s.step("close one", PUT, &format!("{mrs}/3"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.settle().await;

    let ps = format!("/projects/{pid}/search");
    s.step("project mrs", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": word}), "mr").await;
    s.step("project mrs opened", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": word, "state": "opened"}), "mr").await;
    s.step("project mrs asc", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": word, "order_by": "created_at", "sort": "asc"}), "mr").await;
    s.step("project mrs paged", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": word, "per_page": 1}), "mr").await;
    s.step("project mrs by iid", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": "!2"}), "mr").await;
    s.step("project notes", GET, &ps, Some(&admin), json!({"scope": "notes", "search": word}), "note").await;
    s.step("project commits", GET, &ps, Some(&admin), json!({"scope": "commits", "search": word}), "commit").await;
    s.step("project milestones", GET, &ps, Some(&admin), json!({"scope": "milestones", "search": word}), "milestone").await;
    s.step("project users", GET, &ps, Some(&admin), json!({"scope": "users", "search": "claude"}), "user").await;
    s.step("project bad scope", GET, &ps, Some(&admin), json!({"scope": "projects", "search": word}), "mr").await;
    s.step("project missing search", GET, &ps, Some(&admin), json!({"scope": "merge_requests"}), "mr").await;
    s.step("project missing scope", GET, &ps, Some(&admin), json!({"search": word}), "mr").await;
    s.step("project short term", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": "a"}), "mr").await;
    s.step("group mrs", GET, &format!("/groups/{gid}/search"), Some(&admin), json!({"scope": "merge_requests", "search": word}), "mr").await;
    s.step("group projects", GET, &format!("/groups/{gid}/search"), Some(&admin), json!({"scope": "projects", "search": "app"}), "project").await;
    s.step("global mrs", GET, "/search", Some(&admin), json!({"scope": "merge_requests", "search": word}), "mr").await;
    s.step("global notes", GET, "/search", Some(&admin), json!({"scope": "notes", "search": word}), "note").await;
    s.step("global unauthenticated", GET, "/search", None, json!({"scope": "merge_requests", "search": word}), "mr").await;
    // Group milestones, full-path project matches, sort fallbacks, users.
    s.step("group milestone", POST, &format!("/groups/{gid}/milestones"), Some(&admin), json!({"title": format!("{word} group milestone")}), "milestone").await;
    s.step("group milestones", GET, &format!("/groups/{gid}/search"), Some(&admin), json!({"scope": "milestones", "search": word}), "milestone").await;
    s.step("project milestones with group", GET, &ps, Some(&admin), json!({"scope": "milestones", "search": word}), "milestone").await;
    s.step("projects by full path", GET, "/search", Some(&admin), json!({"scope": "projects", "search": format!("{gpath}/app")}), "project").await;
    s.step("mrs unknown order", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": word, "order_by": "title", "sort": "asc"}), "mr").await;
    s.step("mrs sort only", GET, &ps, Some(&admin), json!({"scope": "merge_requests", "search": word, "sort": "asc"}), "mr").await;
    s.step("global users", GET, "/search", Some(&admin), json!({"scope": "users", "search": "@claude"}), "user").await;
    s.step("group users", GET, &format!("/groups/{gid}/search"), Some(&admin), json!({"scope": "users", "search": "claude"}), "user").await;
    s.finish();
}
