//! EE merge request dependencies: the `blocks` and `blockees` endpoints with their errors and
//! permissions, blockers in other projects (including ones the user can't see), the blocked merge
//! status and check, the system notes, auto-merge waiting for blockers, and the GraphQL field and
//! mutation.

use super::approval_settings::user_with_token;
use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const DELETE: Method = Method::DELETE;

const MR_QUERY: &str = "query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
  iid detailedMergeStatus mergeable
  blockingMergeRequests { hiddenCount totalCount visibleMergeRequests { iid reference state title project { fullPath } } }
  mergeabilityChecks { identifier status }
} } }";

const SET_BLOCKING: &str = "mutation($path: ID!, $iid: String!, $refs: [String!]!) {
  mergeRequestSetBlockingMergeRequests(input: {projectPath: $path, iid: $iid, blockingMergeRequestReferences: $refs}) {
    errors
    mergeRequest { iid detailedMergeStatus blockingMergeRequests { hiddenCount totalCount visibleMergeRequests { iid reference } } }
  } }";

async fn branch_mr(s: &mut Scenario, name: &str, pid: i64, i: i64, token: &str) {
    s.step(
        &format!("commit {name} {i}"),
        POST,
        &format!("/projects/{pid}/repository/commits"),
        Some(token),
        json!({"branch": format!("b{i}"), "start_branch": "main", "commit_message": format!("Add {i}"), "actions": [{"action": "create", "file_path": format!("{i}.txt"), "content": format!("{i}\n")}]}),
        "commit",
    )
    .await;
    s.settle().await;
    s.step(
        &format!("mr {name} {i}"),
        POST,
        &format!("/projects/{pid}/merge_requests"),
        Some(token),
        json!({"source_branch": format!("b{i}"), "target_branch": "main", "title": format!("MR {i}")}),
        "mr",
    )
    .await;
    s.poll_mr(pid, i).await;
}

#[tokio::test]
async fn mr_dependencies() {
    let mut s = Scenario::new("mr_dependencies").await;
    let mut rx = s.receiver().await;
    let admin = s.admin();
    let r = s.send(PUT, "/application/settings", Some(&admin), json!({"allow_local_requests_from_web_hooks_and_services": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let gpath = s.name("dep");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    let opath = s.name("depo");
    let og = s.step("other group", POST, "/groups", Some(&admin), json!({"name": opath, "path": opath}), "namespace").await;
    let op = s.step("other project", POST, "/projects", Some(&admin), json!({"name": "lib", "namespace_id": og.id(), "initialize_with_readme": true}), "project").await;
    let opid = op.id();
    s.settle().await;
    let (dev, dev_t) = user_with_token(&mut s, "dev").await;
    let (_out, out_t) = user_with_token(&mut s, "outsider").await;
    s.step("dev member", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": dev, "access_level": 30}), "user").await;
    s.settle().await;
    s.step(
        "hook",
        POST,
        &format!("/projects/{pid}/hooks"),
        Some(&admin),
        json!({"url": rx.url, "merge_requests_events": true, "note_events": true, "push_events": false}),
        "hook",
    )
    .await;
    for i in 1..=5 {
        branch_mr(&mut s, "app", pid, i, &admin).await;
    }
    branch_mr(&mut s, "lib", opid, 1, &admin).await;
    s.deliveries("open hooks", &mut rx, 5).await;
    let mrs = format!("/projects/{pid}/merge_requests");
    let blocks2 = format!("{mrs}/2/blocks");

    // Creating blocks, and the errors.
    s.step("blocks none", GET, &blocks2, Some(&admin), json!(null), "block").await;
    s.step("block by 1", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_iid": 1}), "block").await;
    s.step("block by 1 again", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_iid": 1}), "block").await;
    s.step("block by itself", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_iid": 2}), "block").await;
    s.step("block by missing", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_iid": 99}), "block").await;
    s.step("block without params", POST, &blocks2, Some(&admin), json!({}), "block").await;
    let mr3 = s.step("mr 3", GET, &format!("{mrs}/3"), Some(&admin), json!(null), "mr").await;
    let mr3_id = mr3.id();
    s.step("block by id and iid", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_id": mr3_id, "blocking_merge_request_iid": 3}), "block").await;
    s.step("block by id", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_id": mr3_id}), "block").await;
    s.step("block by other project", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_iid": 1, "blocking_project_id": opid}), "block").await;
    s.step("block by missing project", POST, &blocks2, Some(&admin), json!({"blocking_merge_request_iid": 1, "blocking_project_id": 999999}), "block").await;
    s.step("dev blocks by hidden", POST, &format!("{mrs}/4/blocks"), Some(&dev_t), json!({"blocking_merge_request_iid": 1, "blocking_project_id": opid}), "block").await;
    s.step("dev blocks 4 by 1", POST, &format!("{mrs}/4/blocks"), Some(&dev_t), json!({"blocking_merge_request_iid": 1}), "block").await;
    s.step("outsider blocks", POST, &format!("{mrs}/4/blocks"), Some(&out_t), json!({"blocking_merge_request_iid": 3}), "block").await;
    s.settle().await;
    s.deliveries("hooks after blocking", &mut rx, 0).await;

    // Reading them.
    s.step("blocks of 2", GET, &blocks2, Some(&admin), json!(null), "block").await;
    s.step("blocks of 2 as dev", GET, &blocks2, Some(&dev_t), json!(null), "block").await;
    s.step("blocks of 2 as outsider", GET, &blocks2, Some(&out_t), json!(null), "block").await;
    s.step("blockees of 1", GET, &format!("{mrs}/1/blockees"), Some(&admin), json!(null), "block").await;
    s.step("blockees of 2", GET, &format!("{mrs}/2/blockees"), Some(&admin), json!(null), "block").await;
    s.step("blockees of other 1", GET, &format!("/projects/{opid}/merge_requests/1/blockees"), Some(&admin), json!(null), "block").await;
    s.step("blocks of missing mr", GET, &format!("{mrs}/99/blocks"), Some(&admin), json!(null), "block").await;
    s.step("mr 2 blocked", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("mrs list", GET, &format!("{mrs}?order_by=created_at&sort=asc"), Some(&admin), json!(null), "mr").await;
    let vars2 = json!({"path": full, "iid": "2"});
    s.graphql("graphql 2", Some(&admin), MR_QUERY, vars2.clone()).await;
    s.graphql("graphql 2 as dev", Some(&dev_t), MR_QUERY, vars2.clone()).await;
    s.graphql("graphql 1", Some(&admin), MR_QUERY, json!({"path": full, "iid": "1"})).await;
    s.step("merge blocked", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!(null), "mr").await;
    s.step("notes of 2", GET, &format!("{mrs}/2/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.step("notes of 1", GET, &format!("{mrs}/1/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.step("notes of other 1", GET, &format!("/projects/{opid}/merge_requests/1/notes?sort=asc"), Some(&admin), json!(null), "note").await;

    // Removing them.
    let blocks: Vec<i64> = s.step("blocks of 2 again", GET, &blocks2, Some(&admin), json!(null), "block").await.body.as_array().unwrap().iter().map(|b| b["id"].as_i64().unwrap()).collect();
    let cross = blocks[2];
    s.step("dev removes hidden", DELETE, &format!("{blocks2}/{cross}"), Some(&dev_t), json!(null), "block").await;
    s.step("remove on wrong mr", DELETE, &format!("{mrs}/3/blocks/{cross}"), Some(&admin), json!(null), "block").await;
    s.step("remove other project", DELETE, &format!("{blocks2}/{cross}"), Some(&admin), json!(null), "block").await;
    s.step("remove again", DELETE, &format!("{blocks2}/{cross}"), Some(&admin), json!(null), "block").await;
    s.step("notes of 2 after remove", GET, &format!("{mrs}/2/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.step("notes of other 1 after remove", GET, &format!("/projects/{opid}/merge_requests/1/notes?sort=asc"), Some(&admin), json!(null), "note").await;

    // A closed blocker still blocks; auto-merge waits until all blockers merged.
    s.step("close 1", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.settle().await;
    s.step("mr 2 with closed blocker", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.graphql("graphql 2 closed blocker", Some(&admin), MR_QUERY, vars2.clone()).await;
    s.step("reopen 1", PUT, &format!("{mrs}/1"), Some(&admin), json!({"state_event": "reopen"}), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 1).await;
    s.step("auto merge 2", PUT, &format!("{mrs}/2/merge"), Some(&admin), json!({"auto_merge": true}), "mr").await;
    s.settle().await;
    s.deliveries("hooks so far", &mut rx, 2).await;
    s.step("merge 1", PUT, &format!("{mrs}/1/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.poll_mr(pid, 2).await;
    s.poll_mr(pid, 3).await;
    s.step("mr 2 blocked by 3", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("merge 3", PUT, &format!("{mrs}/3/merge"), Some(&admin), json!(null), "mr").await;
    s.settle().await;
    s.poll(&format!("{mrs}/2"), |b| b["state"] == "merged").await;
    s.step("mr 2 merged", GET, &format!("{mrs}/2"), Some(&admin), json!(null), "mr").await;
    s.step("blocks of merged 2", GET, &blocks2, Some(&admin), json!(null), "block").await;
    s.graphql("graphql 2 merged", Some(&admin), MR_QUERY, vars2.clone()).await;
    s.deliveries("merge hooks", &mut rx, 4).await;

    // GraphQL: replacing MR 5's blockers.
    let vars5 = |refs: serde_json::Value| json!({"path": full, "iid": "5", "refs": refs});
    s.graphql("set 4", Some(&admin), SET_BLOCKING, vars5(json!(["!4"]))).await;
    s.graphql("set mixed", Some(&admin), SET_BLOCKING, vars5(json!(["!4", format!("{opath}/lib!1"), "!99", "garbage", "!1"]))).await;
    s.graphql("set itself", Some(&admin), SET_BLOCKING, vars5(json!(["!5"]))).await;
    s.graphql("set by url", Some(&admin), SET_BLOCKING, vars5(json!([format!("{}/{full}/-/merge_requests/4", s.public_base())]))).await;
    s.graphql("set as dev hidden", Some(&dev_t), SET_BLOCKING, vars5(json!([format!("{opath}/lib!1")]))).await;
    s.graphql("set as outsider", Some(&out_t), SET_BLOCKING, vars5(json!(["!4"]))).await;
    s.graphql("graphql 5", Some(&admin), MR_QUERY, json!({"path": full, "iid": "5"})).await;
    s.graphql("set none", Some(&admin), SET_BLOCKING, vars5(json!([]))).await;
    s.step("blocks of 5", GET, &format!("{mrs}/5/blocks"), Some(&admin), json!(null), "block").await;
    s.step("notes of 5", GET, &format!("{mrs}/5/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.step("notes of 4", GET, &format!("{mrs}/4/notes?sort=asc"), Some(&admin), json!(null), "note").await;
    s.settle().await;
    s.deliveries("hooks at the end", &mut rx, 0).await;
    s.finish();
}
