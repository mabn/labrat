//! Repository and group endpoints around merge requests: protected tags, blame, contributors,
//! branch existence checks, group label CRUD and group milestone updates.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;
const HEAD: Method = Method::HEAD;

#[tokio::test]
async fn repo_extras() {
    let mut s = Scenario::new("repo_extras").await;
    let admin = s.admin();
    let gpath = s.name("rx");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("bob", POST, "/users", Some(&admin), json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s.step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("bob maintainer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 40}), "user").await;
    let commits = format!("/projects/{pid}/repository/commits");
    s.step("c1", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Add f", "actions": [{"action": "create", "file_path": "dir/f.txt", "content": "one\ntwo\nthree\nfour\n"}]}), "commit").await;
    s.step("c2", POST, &commits, Some(&bob_t), json!({"branch": "main", "commit_message": "Edit f", "actions": [{"action": "update", "file_path": "dir/f.txt", "content": "one\nTWO\nthree\nfour\nfive\n"}]}), "commit").await;
    s.step("c3", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Edit again", "actions": [{"action": "update", "file_path": "dir/f.txt", "content": "zero\none\nTWO\nthree\nfour\nfive\n"}]}), "commit").await;
    s.settle().await;
    let blame = format!("/projects/{pid}/repository/files/dir%2Ff.txt/blame");
    s.step("blame", GET, &blame, Some(&admin), json!({"ref": "main"}), "blame").await;
    s.step("blame range", GET, &blame, Some(&admin), json!({"ref": "main", "range[start]": 2, "range[end]": 3}), "blame").await;
    s.step("blame bad range", GET, &blame, Some(&admin), json!({"ref": "main", "range[start]": 5, "range[end]": 2}), "blame").await;
    s.step("blame no ref", GET, &blame, Some(&admin), json!(null), "blame").await;
    s.step("blame missing", GET, &format!("/projects/{pid}/repository/files/nope.txt/blame"), Some(&admin), json!({"ref": "main"}), "blame").await;
    s.step("blame head", HEAD, &blame, Some(&admin), json!({"ref": "main"}), "blame").await;
    s.step("contributors", GET, &format!("/projects/{pid}/repository/contributors"), Some(&admin), json!(null), "contributor").await;
    s.step("contributors by commits", GET, &format!("/projects/{pid}/repository/contributors"), Some(&admin), json!({"order_by": "commits", "sort": "desc"}), "contributor").await;
    s.step("head branch", HEAD, &format!("/projects/{pid}/repository/branches/main"), Some(&admin), json!(null), "branch").await;
    s.step("head missing branch", HEAD, &format!("/projects/{pid}/repository/branches/nope"), Some(&admin), json!(null), "branch").await;
    // Protected tags.
    let pt = format!("/projects/{pid}/protected_tags");
    s.step("protect tag", POST, &pt, Some(&admin), json!({"name": "v*", "create_access_level": 40}), "protected_tag").await;
    s.step("protect again", POST, &pt, Some(&admin), json!({"name": "v*"}), "protected_tag").await;
    s.step("protect bad level", POST, &pt, Some(&admin), json!({"name": "r*", "create_access_level": 35}), "protected_tag").await;
    s.step("protected tags", GET, &pt, Some(&admin), json!(null), "protected_tag").await;
    s.step("protected tag", GET, &format!("{pt}/v*"), Some(&admin), json!(null), "protected_tag").await;
    s.step("protected tag missing", GET, &format!("{pt}/x"), Some(&admin), json!(null), "protected_tag").await;
    s.step("protect no access", POST, &pt, Some(&admin), json!({"name": "rel-*", "create_access_level": 0}), "protected_tag").await;
    s.step("bob tag blocked", POST, &format!("/projects/{pid}/repository/tags"), Some(&bob_t), json!({"tag_name": "rel-1", "ref": "main"}), "tag").await;
    s.step("bob tag allowed", POST, &format!("/projects/{pid}/repository/tags"), Some(&bob_t), json!({"tag_name": "v1", "ref": "main"}), "tag").await;
    s.step("bob delete protected tag", DELETE, &format!("/projects/{pid}/repository/tags/v1"), Some(&bob_t), json!(null), "tag").await;
    s.step("tag list", GET, &format!("/projects/{pid}/repository/tags"), Some(&admin), json!(null), "tag").await;
    s.step("unprotect", DELETE, &format!("{pt}/v*"), Some(&admin), json!(null), "protected_tag").await;
    s.step("unprotect missing", DELETE, &format!("{pt}/v*"), Some(&admin), json!(null), "protected_tag").await;
    // Group labels.
    let gl = format!("/groups/{gid}/labels");
    s.step("group label", POST, &gl, Some(&admin), json!({"name": "team", "color": "#123456", "description": "Team"}), "label").await;
    s.step("group label dup", POST, &gl, Some(&admin), json!({"name": "team", "color": "#123456"}), "label").await;
    s.step("group label show", GET, &format!("{gl}/team"), Some(&admin), json!(null), "label").await;
    s.step("group label rename", PUT, &format!("{gl}/team"), Some(&admin), json!({"new_name": "squad", "color": "red"}), "label").await;
    s.step("group label update by name", PUT, &gl, Some(&admin), json!({"name": "squad", "description": "Squad"}), "label").await;
    s.step("group label update nothing", PUT, &format!("{gl}/squad"), Some(&admin), json!({}), "label").await;
    s.step("group label subscribe", POST, &format!("{gl}/squad/subscribe"), Some(&admin), json!(null), "label").await;
    s.step("group label unsubscribe", POST, &format!("{gl}/squad/unsubscribe"), Some(&admin), json!(null), "label").await;
    s.step("group labels", GET, &gl, Some(&admin), json!(null), "label").await;
    s.step("group label delete", DELETE, &format!("{gl}/squad"), Some(&admin), json!(null), "label").await;
    s.step("group label delete missing", DELETE, &gl, Some(&admin), json!({"name": "squad"}), "label").await;
    // Group milestones.
    let gm = format!("/groups/{gid}/milestones");
    let m = s.step("group milestone", POST, &gm, Some(&admin), json!({"title": "G1", "due_date": "2027-01-01"}), "milestone").await;
    let mid = m.id();
    s.step("mr", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "main", "target_branch": "main", "title": "Self"}), "mr").await;
    s.step("c4", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Feature", "actions": [{"action": "create", "file_path": "g.txt", "content": "g\n"}]}), "commit").await;
    s.settle().await;
    s.step("mr with milestone", POST, &format!("/projects/{pid}/merge_requests"), Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Feature", "milestone_id": mid}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("milestone mrs", GET, &format!("{gm}/{mid}/merge_requests"), Some(&admin), json!(null), "mr").await;
    s.step("milestone close", PUT, &format!("{gm}/{mid}"), Some(&admin), json!({"state_event": "close", "description": "Done"}), "milestone").await;
    s.step("milestone bad date", PUT, &format!("{gm}/{mid}"), Some(&admin), json!({"start_date": "2027-02-01"}), "milestone").await;
    s.step("milestone delete", DELETE, &format!("{gm}/{mid}"), Some(&admin), json!(null), "milestone").await;
    s.step("mr after milestone delete", GET, &format!("/projects/{pid}/merge_requests/1"), Some(&admin), json!(null), "mr").await;
    s.finish();
}
