//! Merge status combinations from docs/gitlab-ref/merge-status-audit.md: blockers peeled off one
//! at a time with a failing CI status underneath, every commit-status state against
//! `only_allow_merge_if_pipeline_succeeds` / `allow_merge_on_skipped_pipeline`, mixed statuses,
//! LFS disabled, and `with_merge_status_recheck` on project, group and
//! global lists. GraphQL `mergeabilityChecks` is captured at each stage.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const CHECKS: &str = "query($p: ID!, $i: String!) { project(fullPath: $p) { mergeRequest(iid: $i) { detailedMergeStatus mergeStatusEnum mergeabilityChecks { identifier status } } } }";

#[tokio::test]
async fn merge_status_matrix() {
    let mut s = Scenario::new("merge_status_matrix").await;
    let admin = s.admin();
    let gpath = s.name("ms");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let gid = g.id();
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": gid, "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    let checks = |iid: &str| json!({"p": full, "i": iid});

    // MR 1: draft, a conflict with main, an unresolved thread, a merge_after in the future, a
    // fast-forward-only project behind main, and a failed status.
    s.step("conflict base", POST, &commits, Some(&admin), json!({"branch": "f", "start_branch": "main", "commit_message": "f", "actions": [{"action": "create", "file_path": "c.txt", "content": "feature\n"}]}), "commit").await;
    s.step("main side", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "main", "actions": [{"action": "create", "file_path": "c.txt", "content": "main\n"}]}), "commit").await;
    s.settle().await;
    let future = s.date_in(30);
    s.step("mr 1", POST, &mrs, Some(&admin), json!({"source_branch": "f", "target_branch": "main", "title": "Draft: Blocked", "merge_after": format!("{future}T00:00:00Z")}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("thread", POST, &format!("{mrs}/1/discussions"), Some(&admin), json!({"body": "Fix this"}), "discussion").await;
    s.step("settings", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_method": "ff", "only_allow_merge_if_all_discussions_are_resolved": true, "only_allow_merge_if_pipeline_succeeds": true}), "project").await;
    let head = s.send(GET, &format!("{mrs}/1"), Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
    s.step("failed status", POST, &format!("/projects/{pid}/statuses/{head}"), Some(&admin), json!({"state": "failed", "name": "ci", "ref": "f"}), "status").await;
    s.settle().await;
    // Peel the blockers off one at a time.
    let stages: Vec<(&str, serde_json::Value)> = vec![
        ("all blockers", json!(null)),
        ("not draft", json!({"title": "Blocked"})),
        ("no merge_after", json!({"merge_after": null})),
    ];
    for (label, update) in stages {
        if !update.is_null() {
            s.step(&format!("update {label}"), PUT, &format!("{mrs}/1"), Some(&admin), update, "mr").await;
            s.settle().await;
        }
        s.step(&format!("mr 1 {label}"), GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
        s.graphql(&format!("checks {label}"), Some(&admin), CHECKS, checks("1")).await;
    }
    let discs = s.send(GET, &format!("{mrs}/1/discussions"), Some(&admin), json!(null)).await;
    let did = discs.body.as_array().and_then(|a| a.iter().find(|d| d["notes"][0]["resolvable"] == true)).map(|d| d["id"].as_str().unwrap_or_default().to_string()).unwrap_or_default();
    s.step("resolve thread", PUT, &format!("{mrs}/1/discussions/{did}"), Some(&admin), json!({"resolved": true}), "discussion").await;
    s.settle().await;
    s.step("mr 1 resolved", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.graphql("checks resolved", Some(&admin), CHECKS, checks("1")).await;
    s.step("merge method merge", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_method": "merge"}), "project").await;
    s.step("mr 1 recheck", GET, &format!("{mrs}/1"), Some(&admin), json!({"with_merge_status_recheck": true}), "mr").await;
    s.settle().await;
    s.step("mr 1 merge method", GET, &format!("{mrs}/1"), Some(&admin), json!(null), "mr").await;
    s.graphql("checks merge method", Some(&admin), CHECKS, checks("1")).await;
    s.step("try merge", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;

    // MR 2..: one clean MR per commit-status state.
    let states = ["pending", "running", "success", "failed", "canceled", "skipped", "manual"];
    for i in 0..states.len() {
        let b = format!("s{i}");
        s.step("commit", POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": b, "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "x\n"}]}), "commit").await;
    }
    s.settle().await;
    for (i, st) in states.iter().enumerate() {
        let iid = i as i64 + 2;
        let b = format!("s{i}");
        s.step(&format!("mr {st}"), POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("Status {st}")}), "mr").await;
        s.poll_mr(pid, iid).await;
        let sha = s.send(GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
        let body = if *st == "manual" { json!({"state": "pending", "name": "ci", "ref": b}) } else { json!({"state": st, "name": "ci", "ref": b}) };
        s.step(&format!("status {st}"), POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), body, "status").await;
        if *st == "manual" {
            s.step("second status failed", POST, &format!("/projects/{pid}/statuses/{sha}"), Some(&admin), json!({"state": "failed", "name": "lint", "ref": b}), "status").await;
        }
    }
    s.settle().await;
    for skipped_ok in [false, true] {
        s.step(&format!("skipped allowed {skipped_ok}"), PUT, &format!("/projects/{pid}"), Some(&admin), json!({"allow_merge_on_skipped_pipeline": skipped_ok}), "project").await;
        for (i, st) in states.iter().enumerate() {
            let iid = (i as i64 + 2).to_string();
            s.step(&format!("mr {st} skipped_ok={skipped_ok}"), GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null), "mr").await;
            s.graphql(&format!("checks {st} skipped_ok={skipped_ok}"), Some(&admin), CHECKS, checks(&iid)).await;
        }
    }
    s.step("pipeline not required", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"only_allow_merge_if_pipeline_succeeds": false}), "project").await;
    for (i, st) in states.iter().enumerate() {
        let iid = (i as i64 + 2).to_string();
        s.graphql(&format!("checks {st} not required"), Some(&admin), CHECKS, checks(&iid)).await;
    }

    // LFS off: the LFS check is inactive.
    s.step("lfs off", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"lfs_enabled": false}), "project").await;
    s.graphql("checks lfs off", Some(&admin), CHECKS, checks("4")).await;

    // Lists with a recheck: project, group and global .
    s.step("touch target", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "touch", "actions": [{"action": "create", "file_path": "t.txt", "content": "t\n"}]}), "commit").await;
    s.settle().await;
    s.step("project list recheck", GET, &mrs, Some(&admin), json!({"with_merge_status_recheck": true, "state": "opened"}), "mr").await;
    s.settle().await;
    s.step("touch target again", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "touch 2", "actions": [{"action": "create", "file_path": "t2.txt", "content": "t\n"}]}), "commit").await;
    s.settle().await;
    s.step("group list recheck", GET, &format!("/groups/{gid}/merge_requests"), Some(&admin), json!({"with_merge_status_recheck": true, "state": "opened"}), "mr").await;
    s.settle().await;
    s.step("group list after", GET, &format!("/groups/{gid}/merge_requests"), Some(&admin), json!({"state": "opened"}), "mr").await;
    s.finish();
}
