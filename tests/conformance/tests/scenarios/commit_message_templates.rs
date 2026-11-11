//! Project settings that shape commit messages (docs/gitlab-ref/mr-feature-inventory.md):
//! `merge_commit_template` and `squash_commit_template` with every placeholder, `squash_option`,
//! `suggestion_commit_message`, and the MRs of a commit (`commits/:sha/merge_requests`).

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const PLACEHOLDERS: &str = "source=%{source_branch} target=%{target_branch} title=%{title}\nissues=%{issues}\ndesc=%{description}\nref=%{reference} local=%{local_reference}\nurl=%{url}\nfirst=%{first_commit}\nmulti=%{first_multiline_commit}\nmerged=%{merged_by}\napproved:\n%{approved_by}\nreviewed:\n%{reviewed_by}\nco:\n%{co_authored_by}\nall:\n%{all_commits}";

#[tokio::test]
async fn commit_message_templates() {
    let mut s = Scenario::new("commit_message_templates").await;
    let admin = s.admin();
    let gpath = s.name("ct");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    s.settle().await;
    let username = s.name("dev");
    let dev = s
        .step("dev", POST, "/users", Some(&admin), json!({"username": username, "name": "Dev Person", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let dev_t = s.step("dev token", POST, &format!("/users/{dev}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token").await.get("token").as_str().unwrap().to_string();
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    s.step("templates", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"merge_commit_template": PLACEHOLDERS, "squash_commit_template": PLACEHOLDERS, "suggestion_commit_message": "Suggest %{branch_name} %{files_count} %{file_paths} %{project_name} %{project_path} %{user_full_name} %{username} %{suggestions_count} %{co_authored_by}"}), "project").await;
    let commits = format!("/projects/{pid}/repository/commits");
    let mrs = format!("/projects/{pid}/merge_requests");
    // Branches with commits by two other authors (one of them twice) and a multi-line message.
    for b in ["m", "q", "x", "y", "z", "sg"] {
        s.step(&format!("{b} first"), POST, &commits, Some(&admin), json!({"branch": b, "start_branch": "main", "commit_message": format!("{b}: one line"), "author_name": "Alice Author", "author_email": "alice@example.com", "actions": [{"action": "create", "file_path": format!("{b}.txt"), "content": "1\n2\n3\n"}]}), "commit").await;
        s.settle().await;
        s.step(&format!("{b} second"), POST, &commits, Some(&admin), json!({"branch": b, "commit_message": format!("{b}: subject\n\nBody line\nCo-authored-by: Carol <carol@example.com>"), "author_name": "Bob Builder", "author_email": "bob@example.com", "actions": [{"action": "update", "file_path": format!("{b}.txt"), "content": "1\n2\n3\n4\n"}]}), "commit").await;
        s.step(&format!("{b} third"), POST, &commits, Some(&admin), json!({"branch": b, "commit_message": format!("{b}: again"), "author_name": "Alice Author", "author_email": "alice@example.com", "actions": [{"action": "update", "file_path": format!("{b}.txt"), "content": "1\n2\n3\n4\n5\n"}]}), "commit").await;
    }
    s.settle().await;
    for (i, b) in ["m", "q", "x", "y", "z", "sg"].iter().enumerate() {
        s.step(&format!("mr {b}"), POST, &mrs, Some(&admin), json!({"source_branch": b, "target_branch": "main", "title": format!("MR {b}"), "description": format!("Describe {b}\r\nCloses #1"), "reviewer_ids": [dev]}), "mr").await;
        s.poll_mr(pid, i as i64 + 1).await;
    }
    s.step("dev approves m", POST, &format!("{mrs}/1/approve"), Some(&dev_t), json!(null), "approval").await;
    s.step("dev approves q", POST, &format!("{mrs}/2/approve"), Some(&dev_t), json!(null), "approval").await;
    s.settle().await;
    s.graphql("default messages", Some(&admin), "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"1\") { defaultMergeCommitMessage defaultSquashCommitMessage } } }", json!({"p": format!("{gpath}/app")})).await;
    // Merge with the template, squash with the template, and an explicit message.
    s.step("merge m", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.step("squash q", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!({"squash": true}), "mr").await;
    s.settle().await;
    let log = s.send(GET, &commits, Some(&admin), json!({"ref_name": "main", "per_page": 3})).await;
    for (i, c) in log.body.as_array().cloned().unwrap_or_default().iter().enumerate() {
        let sha = c["id"].as_str().unwrap_or_default().to_string();
        s.step(&format!("main commit {i}"), GET, &format!("{commits}/{sha}"), Some(&admin), json!(null), "commit").await;
    }
    // `squash_option` variants.
    for (opt, iid, squash) in [("always", 3, false), ("never", 4, true), ("default_on", 5, false)] {
        s.step(&format!("squash_option {opt}"), PUT, &format!("/projects/{pid}"), Some(&admin), json!({"squash_option": opt}), "project").await;
        s.step(&format!("mr {iid} with {opt}"), GET, &format!("{mrs}/{iid}"), Some(&admin), json!(null), "mr").await;
        // Let the recheck the read started finish, so the merge sees a checked MR.
        s.settle().await;
        s.step(&format!("merge {iid} squash={squash}"), PUT, &format!("{mrs}/{iid}/merge"), Some(&admin), json!({"squash": squash}), "mr").await;
        s.settle().await;
    }
    s.step("main log after options", GET, &commits, Some(&admin), json!({"ref_name": "main", "per_page": 5}), "commit").await;
    // Suggestions with the project's commit message.
    s.step("squash_option back", PUT, &format!("/projects/{pid}"), Some(&admin), json!({"squash_option": "default_off"}), "project").await;
    let m = s.send(GET, &format!("{mrs}/6"), Some(&admin), json!(null)).await;
    let refs = m.body["diff_refs"].clone();
    let mut ids = vec![];
    for line in [1, 2, 3] {
        let pos = json!({"base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "position_type": "text", "old_path": "sg.txt", "new_path": "sg.txt", "new_line": line});
        let d = s.step(&format!("suggestion {line}"), POST, &format!("{mrs}/6/discussions"), Some(&dev_t), json!({"body": format!("```suggestion:-0+0\nLINE {line}\n```"), "position": pos}), "discussion").await;
        let did = d.body["id"].as_str().unwrap_or_default().to_string();
        let full = s.send(GET, &format!("{mrs}/6/discussions/{did}"), Some(&admin), json!(null)).await;
        ids.push(full.body["notes"][0]["suggestions"][0]["id"].clone());
    }
    s.step("apply one", PUT, &format!("/suggestions/{}/apply", ids[0]), Some(&admin), json!(null), "suggestion").await;
    s.settle().await;
    s.step("batch two", PUT, "/suggestions/batch_apply", Some(&admin), json!({"ids": [ids[1], ids[2]]}), "suggestion").await;
    s.settle().await;
    s.step("sg log", GET, &commits, Some(&admin), json!({"ref_name": "sg", "per_page": 2}), "commit").await;
    // The MRs of a commit.
    let head = s.send(GET, &format!("{mrs}/6"), Some(&admin), json!(null)).await.body["sha"].as_str().unwrap_or_default().to_string();
    let merged = s.send(GET, &format!("{mrs}/1"), Some(&admin), json!(null)).await;
    let merge_sha = merged.body["merge_commit_sha"].as_str().unwrap_or_default().to_string();
    let first_of_m = s.send(GET, &format!("{mrs}/1/commits"), Some(&admin), json!(null)).await.body[2]["id"].as_str().unwrap_or_default().to_string();
    for (label, sha, q) in [
        ("open head", head.clone(), json!(null)),
        ("merge sha", merge_sha.clone(), json!(null)),
        ("merged mr commit", first_of_m.clone(), json!(null)),
        ("merged mr commit opened", first_of_m.clone(), json!({"state": "opened"})),
        ("merged mr commit merged", first_of_m.clone(), json!({"state": "merged"})),
        ("unknown sha", "0".repeat(40), json!(null)),
    ] {
        s.step(&format!("commit mrs {label}"), GET, &format!("{commits}/{sha}/merge_requests"), Some(&admin), q, "mr").await;
    }
    s.finish();
}
