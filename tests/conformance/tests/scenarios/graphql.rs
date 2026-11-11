use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;
const PUT: Method = Method::PUT;

const MR_FIELDS: &str = r#"
  id iid title description state draft createdAt updatedAt mergedAt sourceBranch targetBranch
  webUrl webPath reference diffHeadSha mergeCommitSha squash squashOnMerge shouldRemoveSourceBranch
  forceRemoveSourceBranch discussionLocked mergeStatus mergeStatusEnum detailedMergeStatus
  mergeable conflicts approved userNotesCount userDiscussionsCount
  upvotes downvotes subscribed commitCount sourceBranchExists targetBranchExists preparedAt
  timeEstimate totalTimeSpent mergeWhenPipelineSucceeds
  author { id username name state webUrl avatarUrl }
  assignees { nodes { id username } }
  reviewers { nodes { id username mergeRequestInteraction { reviewState approved canMerge canUpdate reviewed } } }
  approvedBy { nodes { username } }
  labels { nodes { id title color textColor description } }
  diffRefs { baseSha startSha headSha }
  diffStatsSummary { additions deletions changes fileCount }
  userPermissions { readMergeRequest updateMergeRequest adminMergeRequest createNote pushToSourceBranch removeSourceBranch canMerge canApprove }
  awardEmoji { nodes { name user { username } } }
"#;

#[tokio::test]
async fn graphql_merge_requests() {
    let mut s = Scenario::new("graphql_merge_requests").await;
    let admin = s.admin();
    let gpath = s.name("gql");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s
        .step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project")
        .await;
    let pid = p.id();
    let full = format!("{gpath}/app");
    s.settle().await;
    let username = s.name("bob");
    let bob = s
        .step("create bob", POST, "/users", Some(&admin),
            json!({"username": username, "name": "Bob", "email": format!("{username}@example.com"), "password": "Password-123-labrat", "skip_confirmation": true}), "user")
        .await
        .id();
    let bob_t = s
        .step("bob token", POST, &format!("/users/{bob}/personal_access_tokens"), Some(&admin), json!({"name": "t", "scopes": ["api"]}), "token")
        .await
        .get("token")
        .as_str()
        .unwrap()
        .to_string();
    s.step("bob developer", POST, &format!("/projects/{pid}/members"), Some(&admin), json!({"user_id": bob, "access_level": 30}), "user").await;
    s.step("label", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "bug", "color": "#ff0000", "description": "Bugs"}), "label").await;
    s.step("label 2", POST, &format!("/projects/{pid}/labels"), Some(&admin), json!({"name": "ui", "color": "#00ff00"}), "label").await;
    for (branch, file) in [("feature", "a.txt"), ("second", "b.txt")] {
        s.step(
            "commit",
            POST,
            &format!("/projects/{pid}/repository/commits"),
            Some(&admin),
            json!({"branch": branch, "start_branch": "main", "commit_message": format!("Add {file}"),
                   "actions": [{"action": "create", "file_path": file, "content": "one\ntwo\nthree\n"}]}),
            "commit",
        )
        .await;
    }
    s.settle().await;
    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr 1", POST, &mrs, Some(&admin),
        json!({"source_branch": "feature", "target_branch": "main", "title": "Add a", "description": "Adds **a**", "labels": "bug", "reviewer_ids": [bob], "assignee_ids": [bob]}), "mr").await;
    s.step("mr 2", POST, &mrs, Some(&admin), json!({"source_branch": "second", "target_branch": "main", "title": "Draft: Add b"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.poll_mr(pid, 2).await;
    s.step("note", POST, &format!("{mrs}/1/notes"), Some(&bob_t), json!({"body": "Looks good"}), "note").await;
    s.step("thumbsup", POST, &format!("{mrs}/1/award_emoji"), Some(&bob_t), json!({"name": "thumbsup"}), "award").await;
    s.step("approve", POST, &format!("{mrs}/1/approve"), Some(&bob_t), json!(null), "approval").await;
    s.settle().await;

    // Queries
    let q_mr = format!("query($path: ID!, $iid: String!) {{ project(fullPath: $path) {{ id name fullPath mergeRequest(iid: $iid) {{ {MR_FIELDS} }} }} }}");
    s.graphql("mr query", Some(&admin), &q_mr, json!({"path": full, "iid": "1"})).await;
    s.graphql("mr query as bob", Some(&bob_t), &q_mr, json!({"path": full, "iid": "1"})).await;
    s.graphql("draft mr", Some(&admin), &q_mr, json!({"path": full, "iid": "2"})).await;
    s.graphql("missing mr", Some(&admin), &q_mr, json!({"path": full, "iid": "99"})).await;
    s.graphql("missing project", Some(&admin), &q_mr, json!({"path": "nope/nope", "iid": "1"})).await;
    s.graphql("anonymous private project", None, &q_mr, json!({"path": full, "iid": "1"})).await;
    let q_list = "query($path: ID!, $after: String) { project(fullPath: $path) { mergeRequests(first: 1, after: $after) { count pageInfo { hasNextPage hasPreviousPage startCursor endCursor } edges { cursor node { iid title } } nodes { iid } } } }";
    let page1 = s.graphql("list page 1", Some(&admin), q_list, json!({"path": full})).await;
    let cursor = page1.body["data"]["project"]["mergeRequests"]["pageInfo"]["endCursor"].clone();
    s.graphql("list page 2", Some(&admin), q_list, json!({"path": full, "after": cursor})).await;
    s.graphql("list filtered", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequests(state: opened, draft: true) { nodes { iid title draft } } } }", json!({"path": full})).await;
    s.graphql("current user", Some(&bob_t), "{ currentUser { id username name state } }", json!(null)).await;
    s.graphql("user by username", Some(&admin), "query($u: String!) { user(username: $u) { id username name } }", json!({"u": username})).await;
    s.graphql("mr by gid", Some(&admin), "query($id: MergeRequestID!) { mergeRequest(id: $id) { iid title } }", json!({"id": "gid://gitlab/MergeRequest/999999"})).await;

    // Errors
    s.graphql("unknown field", Some(&admin), "{ currentUser { nope } }", json!(null)).await;
    s.graphql("missing argument", Some(&admin), "{ project { id } }", json!(null)).await;
    s.graphql("syntax error", Some(&admin), "{ currentUser { id ", json!(null)).await;
    s.graphql("scalar with selection", Some(&admin), "{ currentUser { id { x } } }", json!(null)).await;
    s.graphql("object without selection", Some(&admin), "{ currentUser }", json!(null)).await;
    s.graphql("unknown argument", Some(&admin), "{ currentUser(foo: 1) { id } }", json!(null)).await;
    s.graphql("bad gid", Some(&admin), "{ mergeRequest(id: \"nope\") { iid } }", json!(null)).await;
    s.graphql("missing variable", Some(&admin), "query($path: ID!) { project(fullPath: $path) { id } }", json!(null)).await;
    s.graphql("empty query", Some(&admin), "", json!(null)).await;
    s.graphql("typename and alias", Some(&admin), "query { me: currentUser { __typename username } __typename }", json!(null)).await;
    s.graphql(
        "fragments",
        Some(&admin),
        "query($path: ID!) { project(fullPath: $path) { ...P mergeRequest(iid: \"1\") { ... on MergeRequest { iid } author { ...U } } } } fragment P on Project { name } fragment U on User { username }",
        json!({"path": full}),
    )
    .await;
    s.graphql("skip include", Some(&admin), "query($yes: Boolean!) { currentUser { id @skip(if: $yes) username @include(if: $yes) } }", json!({"yes": true})).await;

    // Introspection is served from GitLab's static file (checked byte for byte in check mode);
    // with another operation name it runs dynamically, where GitLab disables it.
    s.graphql_named("dynamic introspection", Some(&admin), "query Foo { __schema { queryType { name } } }", "Foo").await;

    // Breadth: commits, diff stats, emoji details, note metadata, users, projects, labels.
    s.graphql(
        "mr details",
        Some(&admin),
        "query($path: ID!) { project(fullPath: $path) { labels { nodes { title } } milestones { nodes { title } } mergeRequest(iid: \"1\") { commitCount commits { nodes { id sha shortId title fullTitle description message authorName authorEmail authoredDate committedDate webPath } pageInfo { endCursor } } diffStats { path additions deletions } awardEmoji { nodes { name emoji unicode description unicodeVersion } } resolvableDiscussionsCount resolvedDiscussionsCount projectId squashReadOnly mergeOngoing milestone { title } } } }",
        json!({"path": full}),
    )
    .await;
    s.graphql(
        "note details",
        Some(&admin),
        "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { notes { count nodes { id noteableId noteableType imported externalAuthor maxAccessLevelOfAuthor systemNoteMetadata { action } discussion { replyId userPermissions { resolveNote } } } } } } }",
        json!({"path": full}),
    )
    .await;
    s.graphql("users by username", Some(&admin), "query($u: [String!]) { users(usernames: $u) { nodes { username name } } }", json!({"u": [username]})).await;
    s.graphql("users anonymous", None, "{ users { nodes { username } } }", json!(null)).await;
    s.graphql("projects search", Some(&admin), "query($q: String) { projects(search: $q, membership: true) { nodes { fullPath } } }", json!({"q": gpath})).await;
    s.graphql("metadata", Some(&admin), "{ metadata { version enterprise } }", json!(null)).await;

    // Discussions and notes
    let q_disc = "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { discussions { nodes { id resolvable resolved notes { nodes { id body system internal author { username } resolvable resolved url position { filePath newLine oldLine positionType diffRefs { headSha } } } } } } notes { nodes { id body system systemNoteIconName } } } } }";
    s.graphql("discussions", Some(&admin), q_disc, json!({"path": full})).await;

    // Mutations
    let mr_gid = {
        let r = s.graphql("mr gid", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { id diffRefs { baseSha startSha headSha } } } }", json!({"path": full})).await;
        r.body["data"]["project"]["mergeRequest"].clone()
    };
    let gid = mr_gid["id"].as_str().unwrap().to_string();
    let refs = mr_gid["diffRefs"].clone();
    let mr_out = "mergeRequest { iid title draft state discussionLocked labels { nodes { title } } assignees { nodes { username } } reviewers { nodes { username } } subscribed } errors clientMutationId";
    let set = |name: &str, extra: &str| format!("mutation($path: ID!) {{ {name}(input: {{projectPath: $path, iid: \"1\", clientMutationId: \"x\"{extra}}}) {{ {mr_out} }} }}");
    s.graphql("set draft", Some(&admin), &set("mergeRequestSetDraft", ", draft: true"), json!({"path": full})).await;
    s.graphql("unset draft", Some(&admin), &set("mergeRequestSetDraft", ", draft: false"), json!({"path": full})).await;
    s.graphql("set locked", Some(&admin), &set("mergeRequestSetLocked", ", locked: true"), json!({"path": full})).await;
    s.graphql("set unlocked", Some(&admin), &set("mergeRequestSetLocked", ", locked: false"), json!({"path": full})).await;
    s.graphql("unsubscribe", Some(&admin), &set("mergeRequestSetSubscription", ", subscribedState: false"), json!({"path": full})).await;
    s.graphql("append assignee", Some(&admin), &set("mergeRequestSetAssignees", ", assigneeUsernames: [\"claude\"], operationMode: APPEND"), json!({"path": full})).await;
    s.graphql("replace reviewers", Some(&admin), &set("mergeRequestSetReviewers", ", reviewerUsernames: [\"claude\"]"), json!({"path": full})).await;
    s.graphql("update title", Some(&admin), &set("mergeRequestUpdate", ", title: \"Renamed\", description: \"New\""), json!({"path": full})).await;
    s.graphql("update blank title", Some(&admin), &set("mergeRequestUpdate", ", title: \"\""), json!({"path": full})).await;
    s.graphql("bob can lock", Some(&bob_t), &set("mergeRequestSetLocked", ", locked: true"), json!({"path": full})).await;
    s.graphql("unlock again", Some(&admin), &set("mergeRequestSetLocked", ", locked: false"), json!({"path": full})).await;
    let note_out = "note { id body system internal author { username } discussion { id resolvable } } errors quickActionsStatus { messages commandNames commandsOnly errorMessages }";
    let note = s
        .graphql("create note", Some(&bob_t), &format!("mutation($id: NoteableID!) {{ createNote(input: {{noteableId: $id, body: \"Hello\"}}) {{ {note_out} }} }}"), json!({"id": gid}))
        .await;
    let note_gid = note.body["data"]["createNote"]["note"]["id"].as_str().unwrap_or_default().to_string();
    s.graphql("quick action note", Some(&bob_t), &format!("mutation($id: NoteableID!) {{ createNote(input: {{noteableId: $id, body: \"/label ~ui\"}}) {{ {note_out} }} }}"), json!({"id": gid})).await;
    let disc = s
        .graphql("create discussion", Some(&bob_t), &format!("mutation($id: NoteableID!) {{ createDiscussion(input: {{noteableId: $id, body: \"Thread\"}}) {{ {note_out} }} }}"), json!({"id": gid}))
        .await;
    let disc_id = disc.body["data"]["createDiscussion"]["note"]["discussion"]["id"].as_str().unwrap_or_default().to_string();
    s.graphql(
        "diff note",
        Some(&bob_t),
        &format!("mutation($id: NoteableID!, $pos: DiffPositionInput!) {{ createDiffNote(input: {{noteableId: $id, body: \"On line\", position: $pos}}) {{ {note_out} }} }}"),
        json!({"id": gid, "pos": {"baseSha": refs["baseSha"], "startSha": refs["startSha"], "headSha": refs["headSha"], "paths": {"oldPath": "a.txt", "newPath": "a.txt"}, "newLine": 2}}),
    )
    .await;
    s.graphql(
        "resolve discussion",
        Some(&bob_t),
        "mutation($id: DiscussionID!) { discussionToggleResolve(input: {id: $id, resolve: true}) { discussion { id resolved resolvedBy { username } } errors } }",
        json!({"id": disc_id}),
    )
    .await;
    s.graphql("update note", Some(&bob_t), &format!("mutation($id: NoteID!) {{ updateNote(input: {{id: $id, body: \"Edited\"}}) {{ {note_out} }} }}"), json!({"id": note_gid})).await;
    s.graphql("update note as other", Some(&admin), &format!("mutation($id: NoteID!) {{ updateNote(input: {{id: $id, body: \"x\"}}) {{ {note_out} }} }}"), json!({"id": note_gid})).await;
    s.graphql("convert to thread", Some(&bob_t), &format!("mutation($id: NoteID!) {{ noteConvertToThread(input: {{id: $id}}) {{ {note_out} }} }}"), json!({"id": note_gid})).await;
    let award = "awardEmoji { name user { username } } errors";
    s.graphql("award add", Some(&admin), &format!("mutation($id: AwardableID!) {{ awardEmojiAdd(input: {{awardableId: $id, name: \"rocket\"}}) {{ {award} }} }}"), json!({"id": gid})).await;
    s.graphql("award add again", Some(&admin), &format!("mutation($id: AwardableID!) {{ awardEmojiAdd(input: {{awardableId: $id, name: \"rocket\"}}) {{ {award} }} }}"), json!({"id": gid})).await;
    s.graphql("award toggle off", Some(&admin), &format!("mutation($id: AwardableID!) {{ awardEmojiToggle(input: {{awardableId: $id, name: \"rocket\"}}) {{ {award} toggledOn }} }}"), json!({"id": gid})).await;
    s.graphql("award remove missing", Some(&admin), &format!("mutation($id: AwardableID!) {{ awardEmojiRemove(input: {{awardableId: $id, name: \"rocket\"}}) {{ {award} }} }}"), json!({"id": gid})).await;
    s.graphql("award on note", Some(&admin), &format!("mutation($id: AwardableID!) {{ awardEmojiAdd(input: {{awardableId: $id, name: \"tada\"}}) {{ {award} }} }}"), json!({"id": note_gid})).await;
    s.graphql("destroy note", Some(&bob_t), &format!("mutation($id: NoteID!) {{ destroyNote(input: {{id: $id}}) {{ {note_out} }} }}"), json!({"id": note_gid})).await;
    s.settle().await;
    s.graphql("after mutations", Some(&admin), &q_mr, json!({"path": full, "iid": "1"})).await;
    s.graphql("close", Some(&admin), &set("mergeRequestUpdate", ", state: CLOSED"), json!({"path": full})).await;
    s.graphql("reopen", Some(&admin), &set("mergeRequestUpdate", ", state: OPEN"), json!({"path": full})).await;
    s.poll_mr(pid, 1).await;
    let head = s.graphql("head", Some(&admin), "query($path: ID!) { project(fullPath: $path) { mergeRequest(iid: \"1\") { diffHeadSha } } }", json!({"path": full})).await;
    let sha = head.body["data"]["project"]["mergeRequest"]["diffHeadSha"].clone();
    s.graphql("accept wrong sha", Some(&admin), &format!("mutation($path: ID!) {{ mergeRequestAccept(input: {{projectPath: $path, iid: \"1\", sha: \"0000000000000000000000000000000000000000\"}}) {{ {mr_out} }} }}"), json!({"path": full})).await;
    s.graphql("accept", Some(&admin), &format!("mutation($path: ID!, $sha: String!) {{ mergeRequestAccept(input: {{projectPath: $path, iid: \"1\", sha: $sha}}) {{ {mr_out} }} }}"), json!({"path": full, "sha": sha})).await;
    s.settle().await;
    s.graphql("merged", Some(&admin), &q_mr, json!({"path": full, "iid": "1"})).await;
    s.graphql(
        "create mr",
        Some(&admin),
        &format!("mutation($path: ID!) {{ mergeRequestCreate(input: {{projectPath: $path, title: \"Via GraphQL\", sourceBranch: \"second\", targetBranch: \"main\"}}) {{ {mr_out} }} }}"),
        json!({"path": full}),
    )
    .await;
    s.step("close mr 2", PUT, &format!("{mrs}/2"), Some(&admin), json!({"state_event": "close"}), "mr").await;
    s.graphql(
        "create mr now",
        Some(&admin),
        &format!("mutation($path: ID!) {{ mergeRequestCreate(input: {{projectPath: $path, title: \"Via GraphQL\", sourceBranch: \"second\", targetBranch: \"main\"}}) {{ {mr_out} }} }}"),
        json!({"path": full}),
    )
    .await;
    s.finish();
}
