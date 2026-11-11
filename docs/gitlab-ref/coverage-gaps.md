# labrat vs GitLab 18.11 CE: MR / code-review coverage gaps

Inventory only. Sources: the GitLab CE 18.11 source copy (paths below are relative to it),
labrat routes `crates/labrat-api/src/routes/*.rs`, GraphQL `crates/labrat-api/src/graphql/resolvers.rs`, and the
18.11 introspection (`docs/gitlab-ref/graphql-introspection.json`) for the GraphQL diff. Method for params: grep of what each labrat handler reads
(`params.str/int/bool/one_of/list/...`); a param that is only listed in a `require_any` key list or as a
pagination default counts as ignored.

Everything in the MR core is routed in labrat: list (global/group/project), create/show/update/delete, participants,
reviewers, commits, changes, diffs, raw_diffs, pipelines (GET), merge, merge_ref, cancel_merge_when_pipeline_succeeds,
rebase, versions, versions/:id, subscribe/unsubscribe, todo, time tracking (5 routes), notes, discussions, draft_notes
(+publish/bulk_publish), award_emoji (MR and MR notes), suggestions apply/batch_apply, approvals/approve/unapprove/reset_approvals,
resource_{label,state,milestone}_events (+single), commit comments/statuses/refs/diff, project events, hooks (project and system), labels, milestones.

## (A) Missing REST endpoints

| # | Endpoint | Params (GitLab) | GitLab source | labrat state |
|---|---|---|---|---|
| A1 | `POST /projects/:id/merge_requests/:iid/pipelines` | `async` Boolean default false | lib/api/merge_requests.rb:705 (params 701) | not routed |
| A2 | `POST /projects/:id/merge_requests/:iid/context_commits` | `commits` Array[String] required | lib/api/merge_requests.rb:550 (params 534) | not routed |
| A3 | `DELETE /projects/:id/merge_requests/:iid/context_commits` | `commits` Array[String] required | lib/api/merge_requests.rb:588 (params 572) | not routed |
| A4 | `GET .../merge_requests/:iid/context_commits` | pagination | lib/api/merge_requests.rb:525 | routed but always `[]` (`empty_list`, merge_requests.rs:46-48) |
| A5 | `GET .../merge_requests/:iid/closes_issues`, `related_issues` | pagination | lib/api/merge_requests.rb:934, 959 | always `[]`; labrat has no issues model |
| A6 | `GET /projects/:id/repository/commits/:sha/merge_requests` | `state` String, pagination | lib/api/commits.rb:689 (params 682) | not routed |
| A7 | `GET /projects/:id/repository/commits/:sha/discussions` and `.../discussions/:id[/notes[/:note_id]]`, POST/PUT/DELETE (Commit noteable) | same as MR discussions, `sha` is String | lib/api/discussions.rb:31-36, 54-262 | not routed (only `/comments`) |
| A8 | `GET /projects/:id/repository/commits/:sha/sequence` | `first_parent` Boolean | lib/api/commits.rb:441 | not routed |
| A9 | `GET /projects/:id/repository/commits/:sha/signature` | - | lib/api/commits.rb:716 | not routed |
| A10 | `GET /search`, `/projects/:id/search`, `/groups/:id/search` (`scope=merge_requests`, `notes`, `commits`) | `scope`, `search` required; `state`, `confidential`, `ref`, `order_by`, `sort`, `include_archived`, `fields` | lib/api/search.rb:245, 279, 316 (params 190-230, 264, 300) | no search routes at all |
| A11 | `POST /projects/:id/fork`, `GET /projects/:id/forks` (needed for cross-project MRs, `target_project_id`) | `namespace*`, `path`, `name`, ... | lib/api/projects.rb:547, 591 | not routed; MRs are same-project only |
| A12 | `POST /projects/:id/pipeline`, `GET /projects/:id/pipelines/latest`, `/pipelines/:pid/jobs`, `/bridges`, `/variables`, `/test_report`, `/test_report_summary`, `POST .../retry`, `.../cancel`, `DELETE /pipelines/:pid`, `PUT .../metadata` | `ref`, `variables`, `inputs`, `include_retried`, `name` | lib/api/ci/pipelines.rb:110, 146, 198, 232, 263, 284, 307, 328, 360, 393, 424 | only `GET /pipelines` and `GET /pipelines/:id` routed (pipelines.rs:21-22) |
| A13 | `PUT /projects/:id/labels/promote`, `PUT /projects/:id/labels/:name/promote` | `name` | lib/api/labels.rb:113, 153 | not routed |
| A14 | `GET /projects/:id/milestones/:milestone_id/issues`, `POST .../milestones/:milestone_id/promote` | pagination | lib/api/project_milestones.rb:102, 129 | not routed |
| A15 | Issues API (`/projects/:id/issues`, notes/discussions/award_emoji/resource events on issues, `related_merge_requests`, `closed_by`) | many | lib/api/issues.rb | not routed; every issuable-generic endpoint (notes, award_emoji, subscriptions, todo, time tracking, resource events) is MR-only |
| A16 | `POST /projects/:id/repository/commits/authorize` (workhorse) | - | lib/api/commits.rb:299 | not routed (low value) |

## (B) REST params GitLab accepts that labrat ignores, per endpoint

| Endpoint | Ignored param(s) | GitLab source |
|---|---|---|
| `GET /merge_requests`, `/groups/:id/merge_requests`, `/projects/:id/merge_requests` | `source_project_id`; `deployed_before`, `deployed_after`, `environment`; `approved` (yes/no); `merge_user_id`, `merge_user_username` (mutually exclusive) | lib/api/helpers/merge_requests_helpers.rb:103-116 |
| same three | `not[...]`: labrat handles only `not[author_id]` and `not[labels]` (merge_requests.rs:442-457). Ignored: `not[author_username]`, `assignee_id`, `assignee_username`, `reviewer_id`, `reviewer_username`, `milestone`, `my_reaction_emoji` | lib/api/helpers/merge_requests_helpers.rb:15-40, 96-102 |
| same three | `order_by` values `label_priority`, `milestone_due`, `popularity`, `priority` are accepted but sorted as `created_at` (merge_requests.rs:410-421); only `created_at`, `updated_at`, `title`, `merged_at` sort for real | lib/api/helpers/merge_requests_helpers.rb:53 (`sort_options`) |
| `POST /projects/:id/merge_requests` | `target_project_id` (cross-project MR); `merge_after` (not read by `mr_params`, merge_requests.rs:639-672) | lib/api/merge_requests.rb:363, 295 |
| `PUT /projects/:id/merge_requests/:iid` | `merge_after` (counted for `at_least_one_of`, merge_requests.rs:730, but never applied) | lib/api/merge_requests.rb:295, 63-82 |
| `GET .../merge_requests/:iid/changes` | `unidiff` (read only by `diffs`, merge_requests.rs:857; `changes` at :832 does not) | lib/api/merge_requests.rb:613 (`use :with_unidiff`) |
| `GET .../merge_requests/:iid/versions/:version_id` | `unidiff` (merge_requests.rs:1100-1117) | lib/api/merge_request_diffs.rb:44 |
| `GET /projects/:id/pipelines` | `scope`, `yaml_errors`, `username`, `updated_before/after`, `created_before/after`, `order_by` (only `id` default), `name`; `sort` read only via paginate defaults. Read: `status`, `ref`, `sha`, `source` | lib/api/ci/pipelines.rb:37-68 |
| `GET /todos` | `group_id` (todos.rs reads action, author_id, project_id, state, type) | lib/api/todos.rb:55 |
| `GET /projects/:id/milestones` | `updated_before`, `updated_after`, `include_parent_milestones` (deprecated alias of `include_ancestors`) | lib/api/milestone_responses.rb:23-26 |
| `GET /projects/:id/repository/commits` | `follow`; `order` (`topo`) and `trailers` only appear as defaults (repository.rs:688), not applied | lib/api/commits.rb:225, 235-236 |
| `GET /projects/:id/repository/commits/:sha/statuses` | none found | - |
| Notes / discussions / draft_notes / award_emoji / suggestions / subscriptions / events / hooks / labels (list+write) | none found (all declared params are read) | lib/api/notes.rb, discussions.rb, draft_notes.rb |

## (C) GraphQL gaps

Schema is the real 18.11 introspection, so every field validates; unimplemented fields fall to `not_implemented`
(resolvers.rs:18).

### C1 Query and container types

| Item | Detail | GitLab source |
|---|---|---|
| `Group.mergeRequests` | `NamespaceObj` (resolvers.rs:456) has only description/fullName/fullPath/id/name/path/visibility/webUrl. Same args as the project field (`includeSubgroups` etc.) | app/graphql/types/group_type.rb:130 |
| `Query.todo(id)`, `Query.milestone(id)`, `Query.issue`, `Query.issues`, `Query.runners`, ... | Query resolves only `currentUser, group, mergeRequest, metadata, namespace, note, project, projects, user, users` (resolvers.rs:227-348); the other root fields are unimplemented | app/graphql/types/query_type.rb |
| `CurrentUser.todos(action, authorId, projectId, groupId, state, isSnoozed, type, sort)` and `UserCore.todos` | missing | app/graphql/types/user_interface.rb:120 |
| `CurrentUser.assigneeOrReviewerMergeRequests` | missing (the three sibling connections are implemented) | app/graphql/types/current_user_type.rb |
| `CurrentUser.groups`, `starredProjects`, `contributedProjects`, `admin`, `userPermissions`, `snippets`, `savedReplies`, `timelogs` | missing | app/graphql/types/current_user_type.rb, user_interface.rb |
| `Project` fields (most missing), MR-relevant ones: `branchRules`, `mergeRequestsAccessLevel`, `openMergeRequestsCount`, `mergeCommitTemplate`, `squashCommitTemplate`, `suggestionCommitMessage`, `squashReadOnly`, `allowsMultipleMergeRequestAssignees`, `allowsMultipleMergeRequestReviewers`, `allowMergeOnSkippedPipeline`, `pipelines(status,ref,sha,source,...)`, `pipeline(id,iid,sha)`, `label(title)`, `projectMembers`, `autocompleteUsers`, `isForked`, `forkedFrom`, `forksCount`, `lfsEnabled`, `topics`, `userPermissions`, `commitReferences`, `languages`, `starCount` | app/graphql/types/project_type.rb |
| `Repository` (10 of 13 missing) | only `empty, exists, rootRef`; missing `tree`, `blobs`, `branchNames`, `commit`, `diskPath`, `paginatedTree`, `lastCommit`, `rootRef` is OK | app/graphql/types/repository_type.rb |
| `Commit` | missing `descriptionHtml`, `diffs`, `name`, `parentSha`, `tags` | app/graphql/types/repositories/commit_type.rb |
| `Pipeline` | 45 of 70 missing (jobs, stages, `downstream`, `upstream`, `testReportSummary`, `mergeRequest`, `commit`, `retryable`, `cancelable`, ...) | app/graphql/types/ci/pipeline_type.rb |

### C2 `MergeRequest` fields not resolved

Non-deprecated first.

| Field | GitLab source |
|---|---|
| `commenters: UserCoreConnection!` | app/graphql/types/merge_request_type.rb (field `commenters`) |
| `committers: UserCoreConnection` | app/graphql/types/merge_request_type.rb:255 |
| `currentUserTodos(state): TodoConnection!` | app/graphql/types/merge_request_type.rb (field `current_user_todos`, via todoable) |
| `sourceBranchProtected: Boolean!` | app/graphql/types/merge_request_type.rb:55 |
| `targetBranchPath: String` | app/graphql/types/merge_request_type.rb:66 |
| `allowsMultipleAssignees`, `allowsMultipleReviewers: Boolean!` | app/graphql/types/merge_request_type.rb:301, 307 |
| `supportsLockOnMerge: Boolean!` | app/graphql/types/merge_request_type.rb:231 |
| `pipelineCreationRequests: [CiPipelineCreationRequest!]!` | app/graphql/types/merge_request_type.rb:176 |
| `name: String` | app/graphql/types/merge_request_type.rb |
| deprecated/experimental: `linkedWorkItems`, `securityAutoFix` | - |

Field arguments: `MergeRequest.diffStats(path)` is handled; `MergeRequest.pipelines(status, scope, ref, refType, sha, source, ids, updatedAfter, updatedBefore, username)`,
`MergeRequest.notes(filter)` and `MergeRequest.reference(full)` take arguments, and the `MrObj::resolve` arms (resolvers.rs:684-925) only read `path`; i.e. these three arguments are ignored (not verified to be read elsewhere).
`Note` (29/29 fields) and `Discussion` (10/10) are fully covered.

### C3 `Project.mergeRequests` arguments (resolvers.rs:557-632)

Read: `state` (not `all` only), `draft`, `iids`, `labels`, `sourceBranches`, `targetBranches`, `authorUsername`, `assigneeUsername`, `reviewerUsername`, `search`, `sort` (CREATED/UPDATED asc/desc only).

| Missing argument | GitLab source |
|---|---|
| `approvedBy`, `mergedBy`, `releaseTag`, `myReactionEmoji`, `labelName`, `milestoneTitle`, `milestoneWildcardId`, `subscribed` | app/graphql/resolvers/merge_requests_resolver.rb:40-56, 131-148, 163 |
| `assigneeUsernames`, `assigneeWildcardId`, `reviewerWildcardId`, `ignoredReviewerUsername` | merge_requests_resolver.rb:17-35, 127 |
| `createdAfter/Before`, `updatedAfter/Before`, `closedAfter/Before`, `mergedAfter/Before`, `deployedAfter/Before`, `deploymentId`, `environmentName` | merge_requests_resolver.rb:95-126, 139-143 |
| `reviewState`, `reviewStates` (UNREVIEWED, REVIEWED, REQUESTED_CHANGES, APPROVED, UNAPPROVED, REVIEW_STARTED) | merge_requests_resolver.rb:151-158 |
| `in`, `blobPath`, `or` (UnionedMergeRequestFilterInput) | merge_requests_resolver.rb:41-62, 87 |
| `not: MergeRequestsResolverNegatedParams` (13 keys: approvedBy, assigneeUsernames, authorUsername, labelName, labels, milestoneTitle, myReactionEmoji, onlyReviewer, releaseTag, reviewStates, reviewerUsername, sourceBranches, targetBranches) | merge_requests_resolver.rb:166-212 |
| `sort` values `MERGED_AT_*`, `CLOSED_AT_*`, `TITLE_*`, `POPULARITY_*`, `PRIORITY_*`, `LABEL_PRIORITY_*`, `MILESTONE_DUE_*` (schema accepts them; labrat treats unknown values as default order) | app/graphql/types/merge_request_sort_enum.rb |

### C4 Mutations (610 in schema; MR/review-related below)

Implemented: `mergeRequestAccept, Create, Update, SetAssignees, SetReviewers, SetLabels, SetMilestone, SetDraft, SetLocked, SetSubscription, RequestChanges, ReviewerRereview`,
`createNote, createDiffNote, createDiscussion, updateNote, destroyNote, noteConvertToThread, discussionToggleResolve`, `awardEmojiAdd/Remove/Toggle`.

| Missing mutation | Input | GitLab source |
|---|---|---|
| `createLatestDiffNote` | noteableId!, body!, filePath!, headSha!, newLine, oldLine, internal | app/graphql/types/mutation_type.rb:124 |
| `createImageDiffNote`, `updateImageDiffNote`, `repositionImageDiffNote` | position!, ... | mutation_type.rb:125, 130 |
| `todoCreate`, `todoMarkDone`, `todoRestore`, `todosMarkAllDone`, `todoResolveMany`, `todoRestoreMany`, `todoSnooze(Many)`, `todoUnSnooze/UnsnoozeMany`, `todoDeleteMany`, `todoDeleteAllDone` | see schema | mutation_type.rb:156-167 |
| `timelogCreate`, `timelogDelete` | timeSpent!, summary!, issuableId!, spentAt | app/graphql/mutations/timelogs/ |
| `pipelineCreate(mergeRequestIid, ref!, variables, inputs, async)`, `pipelineRetry`, `pipelineCancel`, `pipelineDestroy` | see schema | app/graphql/mutations/ci/pipeline/ |
| `labelCreate`, `labelUpdate` | projectPath/groupPath, title!, color, description | app/graphql/mutations/labels/ |
| `commitCreate`, `createBranch`, `branchDelete`, `branchRule*` | see schema | app/graphql/mutations/commits/create.rb, branches/ |

Mutation input fields accepted by the schema but ignored (per arms at resolvers.rs:1585-1740, 1749-1844):

| Mutation | Ignored input |
|---|---|
| `mergeRequestCreate` | `mergeAfter` (title, description, source/target branch, labels, removeSourceBranch are read) |
| `mergeRequestUpdate` | `timeEstimate`, `mergeAfter` |
| `mergeRequestAccept` | `strategy` (auto-merge strategy); sha, commitMessage, squash, squashCommitMessage, shouldRemoveSourceBranch are read |
| `mergeRequestSetAssignees/Reviewers/Labels` | `operationMode` is handled for labels; assignee/reviewer path not re-verified |

## (D) Top-14 priorities for an integration-testing simulator

Ordered by how often bots, CLIs (glab, lab, python-gitlab), and review tools (reviewdog, danger, renovate, mergify-style bots, Gerrit-like UIs) hit them.

| # | Gap | Why | Implement from |
|---|---|---|---|
| 1 | MR list `not[...]` full set + `source_project_id`, `merge_user_id/username`, `approved`, `deployed_*`, `environment`; real sort for `label_priority/milestone_due/popularity/priority` | renovate/mergify/glab filter by these; today negations silently match everything | lib/api/helpers/merge_requests_helpers.rb:15-116 |
| 2 | `POST .../merge_requests/:iid/pipelines` (and `POST /projects/:id/pipeline`) | CI bots create MR pipelines; glab `ci run` | lib/api/merge_requests.rb:705; lib/api/ci/pipelines.rb:110 |
| 3 | `GET /repository/commits/:sha/merge_requests` | reviewdog, danger, release tooling map commit to MR | lib/api/commits.rb:689 |
| 4 | `Group.mergeRequests` (GraphQL) and `CurrentUser.todos` / `assigneeOrReviewerMergeRequests` | review dashboards and CLIs use group-wide MR queries | app/graphql/types/group_type.rb:130; app/graphql/types/user_interface.rb:120 |
| 5 | `Project.mergeRequests` args: `reviewStates`/`reviewState`, `approvedBy`, `mergedBy`, `labelName`, `milestoneTitle`, `createdAfter/updatedAfter/mergedAfter...`, `not`, `assigneeUsernames`, remaining `sort` enums | UI-style GraphQL clients send these | app/graphql/resolvers/merge_requests_resolver.rb:14-212 |
| 6 | `GET /search` family with `scope=merge_requests` / `notes` / `commits` | bots find MRs by text or branch; glab `mr list --search` falls back | lib/api/search.rb:245, 279, 316 |
| 7 | Commit discussions (`/repository/commits/:sha/discussions*`) and MR `notes(filter)` / `pipelines(...)` GraphQL args | bots that comment on commits (reviewdog) read threads back | lib/api/discussions.rb:31-262; app/graphql/types/merge_request_type.rb |
| 8 | `target_project_id` on create, `POST /projects/:id/fork`, `GET /forks`; `source_project_id` filters | every fork-based contribution flow | lib/api/merge_requests.rb:363; lib/api/projects.rb:547, 591 |
| 9 | `merge_after` on create/update (REST and GraphQL `mergeAfter`) | field already echoed in responses but write is dropped | lib/api/merge_requests.rb:295 |
| 10 | Pipeline API remainder: `pipelines/:id/jobs`, `retry`, `cancel`, `pipelines` list filters (`scope`, `username`, `created_*`, `updated_*`, `order_by`, `name`) | MR gating logic reads job lists and status | lib/api/ci/pipelines.rb:37-68, 198, 393, 424 |
| 11 | `changes` and `versions/:id` `unidiff` param | review tools rendering diffs | lib/api/merge_requests.rb:613; lib/api/merge_request_diffs.rb:44 |
| 12 | MR GraphQL fields `currentUserTodos`, `committers`, `commenters`, `sourceBranchProtected`, `targetBranchPath`, `allowsMultiple*`, `supportsLockOnMerge` | web-UI-like clients request them in one query; any missing one errors the whole selection | app/graphql/types/merge_request_type.rb:55, 66, 231, 255, 301, 307 |
| 13 | GraphQL todo mutations (`todoCreate`, `todoMarkDone`, `todosMarkAllDone`, `todoRestore`) and `createLatestDiffNote` | todo handling and the minimal-parameter diff-comment mutation used by newer tooling | app/graphql/types/mutation_type.rb:124, 156-167 |
| 14 | `POST/DELETE context_commits` (+ real `GET`), `todos?group_id`, milestone `updated_*`, label `promote`, `commits` list `follow/order/trailers` | long tail of params python-gitlab and glab expose | lib/api/merge_requests.rb:550, 588; lib/api/todos.rb:55; lib/api/milestone_responses.rb:23-26; lib/api/labels.rb:113, 153; lib/api/commits.rb:225, 235-236 |

Notes on scope: no issues model exists, so `closes_issues`, `related_issues` and issue-linked behaviour (A5, A15) are
intentionally returned empty; raise priority only if tests need `Closes #n` handling.
