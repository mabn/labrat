//! More error paths from docs/gitlab-ref/mr-api-error-coverage.md: suggestions that can't be
//! applied, MR creation and updates rejected by project settings or sizes, conditional deletes,
//! merges blocked by unresolved threads, merge refs of conflicting MRs, rebases of protected
//! branches, and drafts on locked MRs.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

#[tokio::test]
async fn api_error_paths_2() {
    let mut s = Scenario::new("api_error_paths_2").await;
    let admin = s.admin();
    let gpath = s.name("ef");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath, "visibility": "public"}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true, "visibility": "public"}), "project").await;
    let pid = p.id();
    let q = s.step("project 2", POST, "/projects", Some(&admin), json!({"name": "noreq", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await.id();
    s.settle().await;
    let username = s.name("out");
    let out = s
        .step("outsider", POST, "/users", Some(&admin), json!({"username": username, "name": "Out", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let out_t = s.step("outsider token", POST, &format!("/users/{out}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    for b in ["s1", "s2", "c1", "c2", "pr"] {
        let content = if b == "c1" || b == "c2" { format!("{b}\n") } else { "one\ntwo\n".to_string() };
        let file = if b == "c1" || b == "c2" { "conflict.txt".to_string() } else { format!("{b}.txt") };
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": file, "content": content}]}), "commit").await;
    }
    s.settle().await;
    for (i, b) in ["s1", "s2", "c1", "c2", "pr"].iter().enumerate() {
        s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}")}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    // Suggestions: on MR 1 the source branch is deleted, on MR 2 the line changes afterwards.
    let mut sugg = vec![];
    for (iid, file) in [(1, "s1.txt"), (2, "s2.txt")] {
        let m = s.send(GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null)).await;
        let refs = m.body["diff_refs"].clone();
        let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": file, "new_path": file, "new_line": 1});
        let d = s.step(&format!("suggestion {iid}"), POST, &format!("{mrs}/{iid}/discussions"), Some(&admin), json!({"body": "```suggestion:-0+1\nONE\nTWO\n```", "position": pos}), "discussion").await;
        let did = d.body["id"].as_str().unwrap_or_default().to_string();
        let full = s.send(GET, &format!("{mrs}/{iid}/discussions/{did}"), Some(&admin), json!(null)).await;
        sugg.push(full.body["notes"][0]["suggestions"][0]["id"].clone());
    }
    s.step("delete s1", reqwest::Method::DELETE, &format!("/projects/{pid}/repository/branches/s1"), Some(&admin), json!(null), "branch").await;
    s.step("change s2", POST, &commits, Some(&admin), json!({"branch": "s2", "commit_message": "change", "actions": [{"action": "update", "file_path": "s2.txt", "content": "uno\ndos\n"}]}), "commit").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.step("apply on deleted branch", PUT, &format!("/suggestions/{}/apply", sugg[0]), Some(&admin), json!(null), "error").await;
    s.step("apply outdated", PUT, &format!("/suggestions/{}/apply", sugg[1]), Some(&admin), json!(null), "error").await;
    // Settings and sizes.
    let big = "x".repeat(1_048_577);
    s.step("huge description", POST, &mrs, Some(&admin), json!({"source_branch": "pr", "target_branch": "main", "title": "big", "description": big}), "error").await;
    s.step("huge note", POST, &format!("{mrs}/5/notes"), Some(&admin), json!({"body": big}), "error").await;
    s.step("assign outsider", PUT, &format!("{mrs}/5"), Some(&admin), json!({"assignee_ids": [out]}), "mr").await;
    let mrs_q = format!("/projects/{q}/merge_requests");
    s.step("q branch", POST, &format!("/projects/{q}/repository/commits"), Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "f", "actions": [{"action": "create", "file_path": "f.txt", "content": "f\n"}]}), "commit").await;
    s.step("disable mrs", PUT, &format!("/projects/{q}"), Some(&admin), json!({"merge_requests_access_level": "disabled"}), "project").await;
    s.step("create with mrs disabled", POST, &mrs_q, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "x"}), "error").await;
    // Merge with an unresolved thread when the project requires resolution.
    s.step("require resolved", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_all_discussions_are_resolved": true}), "project").await;
    s.step("open thread", POST, &format!("{mrs}/5/discussions"), Some(&admin), json!({"body": "Please fix"}), "discussion").await;
    s.settle().await;
    s.step("merge unresolved", PUT, &format!("{mrs}/5/merge"), Some(&admin), json!(null), "error").await;
    // A conflicting MR's merge ref.
    s.step("merge c1", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 4).await;
    s.step("merge ref of conflict", GET, &format!("{mrs}/4/merge_ref"), Some(&admin), json!(null), "error").await;
    // Rebasing a protected source branch that doesn't allow force pushes.
    s.step("protect pr", POST, &format!("/projects/{pid}/protected_branches"), Some(&admin), json!({"name": "pr", "push_access_level": 40, "merge_access_level": 40}), "branch").await;
    s.step("rebase protected", PUT, &format!("{mrs}/5/rebase"), Some(&admin), json!(null), "error").await;
    // Drafts by an outsider on a locked MR of a public project.
    s.step("lock", PUT, &format!("{mrs}/5"), Some(&admin), json!({"discussion_locked": true}), "mr").await;
    s.step("outsider draft locked", POST, &format!("{mrs}/5/draft_notes"), Some(&out_t), json!({"note": "x"}), "error").await;
    s.step("outsider note locked", POST, &format!("{mrs}/5/notes"), Some(&out_t), json!({"body": "x"}), "error").await;
    s.finish();
}
