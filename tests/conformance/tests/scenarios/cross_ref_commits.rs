//! Cross-references to commits (docs/gitlab-ref/labrat-shortcuts-audit.md item 13): full and short
//! SHAs and commit URLs in an MR description and in notes, `project@sha` references, and the
//! `!N+` / `!N+s` MR reference suffixes. The commits' discussions show the system notes.
//! (Rendered HTML is out of scope: PLAN.md, "Exact rendering".)

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;

#[tokio::test]
async fn cross_ref_commits() {
    let mut s = Scenario::new("cross_ref_commits").await;
    let admin = s.admin();
    let gpath = s.name("xr");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mut shas = vec![];
    for i in 1..=4 {
        let c = s.step(&format!("commit {i}"), POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": format!("Main {i}"), "actions": [{"action": "create", "file_path": format!("m{i}.txt"), "content": "x\n"}]}), "commit").await;
        shas.push(c.body["id"].as_str().unwrap_or_default().to_string());
    }
    s.step("feature", POST, &commits, Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "F", "actions": [{"action": "create", "file_path": "f.txt", "content": "x\n"}]}), "commit").await;
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let desc = format!("Builds on {} and {}.\nSee also {}/-/commit/{}", shas[0], &shas[1][..8], "http://example.invalid", shas[2]);
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "F", "description": desc}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.settle().await;
    s.step("note with refs", POST, &format!("{mrs}/1/notes"), Some(&admin), json!({"body": format!("Also {full}@{} and !1+ and !1+s", &shas[3][..12])}), "note").await;
    s.settle().await;
    for (i, sha) in shas.iter().enumerate() {
        s.step(&format!("commit {} discussions", i + 1), GET, &format!("{commits}/{sha}/discussions"), Some(&admin), json!(null), "discussion").await;
    }
    s.step("mr notes", GET, &format!("{mrs}/1/notes"), Some(&admin), json!({"sort": "asc"}), "note").await;
    s.finish();
}
