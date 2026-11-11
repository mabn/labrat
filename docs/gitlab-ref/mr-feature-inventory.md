# labrat vs GitLab 18.11 CE: MR / code-review feature inventory

Static research only (nothing run). Note: the GitLab CE source copy has no `doc/` directory, so the inventory is built
from GitLab source (`lib/api/*.rb`, `lib/gitlab/merge_requests/message_generator.rb`, `lib/api/entities/project.rb`),
the 18.11 introspection (`docs/gitlab-ref/graphql-introspection.json`) and labrat code. The older docs in this folder
(`coverage-gaps.md`, `missing-rest-endpoints.md`) are largely stale: nearly everything they list as missing is now routed.

Legend: COVERED = implemented and at least one scenario in `tests/conformance/tests/scenarios/*.rs` exercises it;
IMPLEMENTED-UNTESTED = code exists, no scenario hits it (or only hits it incidentally); PARTIAL = known behavioural gap; MISSING = not implemented.
Scenario names are file stems; "sc:" lists a representative few, not all.

## 1. REST endpoints

| Endpoint | labrat impl | Status | Scenarios |
|---|---|---|---|
| `GET /merge_requests`, `/groups/:id/merge_requests`, `/projects/:id/merge_requests` (filters) | routes/merge_requests.rs:25-30, list_* | COVERED (PARTIAL for cross-project/deployment filters) | mr_filters, mr_list_params, random_walk_lists, pagination_extremes |
| `POST/GET/PUT/DELETE /projects/:id/merge_requests[/:iid]` | merge_requests.rs:27-31 | COVERED | merge_requests, edge_cases, closed_mr_updates, mr_touch_rules |
| `target_project_id` / fork MRs, `allow_collaboration` | merge_requests.rs:968-980 (422 only) | MISSING (no fork API at all) | one error-path step |
| `.../participants`, `/reviewers`, `/commits`, `/changes`, `/diffs`, `/raw_diffs` | :32-37 | COVERED | participants_sources, participant_order, diffs, mr_diff_edges, mr_diff_git |
| `.../versions`, `/versions/:id` | :49-50 | COVERED | diffs, push_side_effects |
| `GET .../pipelines`, `POST .../pipelines` | :39, pipelines.rs create_mr_pipeline | COVERED | pipeline_endpoints, skip_ci_pipelines |
| `PUT .../merge` (`merge_when_pipeline_succeeds`/`auto_merge`, `sha`, `squash`, `squash_commit_message`, `merge_commit_message`, `should_remove_source_branch`) | :40, services/merge_requests/merge.rs | COVERED | auto_merge_abort, merge_schedule, mr_action_params, merge_status_matrix, random_walk_merge |
| `GET .../merge_ref` | :41 | COVERED | (2 scenarios) |
| `POST .../cancel_merge_when_pipeline_succeeds` | :42 | COVERED | auto_merge_abort |
| `PUT .../rebase` (`skip_ci`) | :46 | COVERED | push_side_effects, skip_ci_*, random_walk_merge |
| `GET/POST/DELETE .../context_commits` | routes/context_commits.rs | COVERED | mr_context_commits |
| `GET .../closes_issues`, `/related_issues` | :47-48 always `[]` (no issues model) | PARTIAL by design (no scenario) | none |
| `POST .../subscribe`, `/unsubscribe` | :51-52 | COVERED | group_mention_subscription, random_walk_* |
| `POST .../todo` | :53 | COVERED | todo_* |
| time tracking: `time_estimate`, `reset_time_estimate`, `add_spent_time`, `reset_spent_time`, `time_stats` | :54-61 | COVERED | time_tracking_extremes |
| notes CRUD, discussions CRUD, resolve, replies | routes/notes.rs:28-82 | COVERED | reviews, review_edges, random_walk_notes |
| draft notes CRUD, `publish`, `bulk_publish` | notes.rs:45-58 | COVERED | reviews, review_commit_positions |
| award emoji on MR and MR notes | notes.rs:59-72 | COVERED | emoji-related, webhooks_more |
| `PUT /suggestions/:id/apply`, `PUT /suggestions/batch_apply` | notes.rs:83-84, services/suggestions.rs | COVERED (but see 3.5 for custom `suggestion_commit_message`) | reviews, review_edges, review_permissions |
| approvals: `GET approvals`, `approve`, `unapprove`, `reset_approvals` | notes.rs:85-90, merge_requests.rs:38 | COVERED | approval_persistence |
| `resource_label_events`, `resource_state_events`, `resource_milestone_events` (+ single) | routes/milestones.rs:31-36 | COVERED | milestones_events, events |
| `GET /projects/:id/repository/commits/:sha/merge_requests` | repository.rs:56 | IMPLEMENTED-UNTESTED (no scenario references it; revert_cherry_pick_mr/webhooks_custom only mention the MR list) | none |
| commit comments + commit discussions (CRUD) | routes/commit_notes.rs | COVERED | commit_comments, commit_discussions, commit_note_effects |
| `POST /commits/:sha/cherry_pick`, `/revert` (incl. `create_merge_request` flow via quick action) | repository.rs:67-74 | COVERED | revert_cherry_pick_mr |
| `GET /repository/compare`, `/merge_base`, `/commits/:sha/diff` | repository.rs:59-91 | COVERED | repository, diff_git |
| `/projects/:id/statuses/:sha`, commit statuses, pipelines (list/show/create/retry/cancel/jobs...) | routes/pipelines.rs | COVERED | statuses, pipeline_endpoints |
| `/projects/:id/templates/merge_requests[/:name]` | routes/templates.rs | COVERED | mr_templates |
| project hooks (MR, note, emoji, pipeline events) | routes/hooks.rs, services/hooks.rs | COVERED | webhooks, webhooks_more, webhooks_custom, webhooks_diff_note, quick_action_hooks |
| `/todos`, `mark_as_done`, `mark_as_done` all | routes/todos.rs | COVERED | todo_lifecycle, todo_resolution, todo_snooze |
| `/search?scope=merge_requests|notes|commits` (+ project/group, `/-/search`) | routes/search.rs | COVERED (partial: basic only) | search |
| `/projects/:id/events`, `/events` | routes/events.rs | COVERED | events |
| labels / milestones used by MRs | routes/labels.rs, milestones.rs | COVERED | milestones_events |
| protected branches (affect merge/push) | repository.rs:92-100 | COVERED | various |
| `POST /projects/:id/fork`, `GET /forks`, `PUT /fork/:id` | none | MISSING | none |
| Issues API (`closed_by`, `related_merge_requests`) | none | MISSING (out of scope, issues) | none |
| Deployments/environments (`deployed_before/after`, `environment` MR filters) | accepted, ignored (merge_requests.rs:249) | PARTIAL | mr_list_params (no-op only) |

## 2. GraphQL

| Item | labrat impl | Status | Scenarios |
|---|---|---|---|
| `Query.mergeRequest(id)`, `Project.mergeRequest(iid)`, `Project.mergeRequests`, `Group.mergeRequests`, `currentUser.{authored,assigned,reviewRequested,assigneeOrReviewer}MergeRequests` | resolvers.rs ("mergeRequests" arms, mr_filters.rs) | COVERED | graphql_mr_filters, graphql_pagination, graphql_root_fields |
| All `MergeRequest` fields (string-match check vs introspection: none unresolved) | resolvers.rs MrObj | COVERED | graphql_mr_fields, graphql_mr_all_scalars, graphql_type_fields |
| `Note`/`Discussion`/`DiffNote`/`Commit`/`Todo`/`Timelog`/`DiffRefs` all fields | resolvers.rs | COVERED | graphql_note_all_scalars, graphql_remaining_fields |
| `MergeRequest.pipelines(...)`, `notes(filter)`, `reference(full)` args | arms exist; arg handling unverified | IMPLEMENTED-UNTESTED (args) | graphql_field_semantics partial |
| Project MR settings fields (`mergeCommitTemplate`, `squashCommitTemplate`, `suggestionCommitMessage`, `squashReadOnly`, `printingMergeRequestLinkEnabled`, ...) | resolvers.rs:931-983 | COVERED | graphql_project_fields, graphql_more_all_scalars |
| Mutations: `mergeRequestCreate/Update/Accept/SetAssignees/SetReviewers/SetLabels/SetMilestone/SetDraft/SetLocked/SetSubscription/RequestChanges/ReviewerRereview` | resolvers.rs:2820-3100 | COVERED | graphql_more_mutations, graphql_mutations_3, graphql_mutation_errors, random_walk_graphql |
| `createNote/createDiffNote/createImageDiffNote/createLatestDiffNote/createDiscussion/updateNote/destroyNote/noteConvertToThread/discussionToggleResolve/updateImageDiffNote/repositionImageDiffNote` | resolvers.rs:3230-3330 | COVERED | graphql, graphql_note_permissions |
| `awardEmojiAdd/Remove/Toggle`, `todoCreate/MarkDone/Restore/Snooze/.../todosMarkAllDone`, `timelogCreate/Delete`, `labelCreate/Update`, `commitCreate` | resolvers.rs:3292-3500 | COVERED (commitCreate: 1 scenario) | graphql_more_mutations, todo_mutations |
| `pipelineCreate` | falls to `not_implemented` | MISSING | none |
| `createBranch`, `branchDelete`, `branchRule*`, `projectTargetBranchRule*` | falls to `not_implemented` | MISSING (low) | none |
| Absent in 18.11 so correctly absent: approve/unapprove mutation, `mergeRequestSetDraft` is present | | n/a | |

## 3. Behaviours and project settings

| Behaviour / setting | labrat | Status | Scenarios |
|---|---|---|---|
| merge methods `merge`, `rebase_merge` (semi-linear), `ff` incl. "should be rebased" gating | merge.rs:229,308; mergeability.rs:170 | COVERED | basics, merge_requests, merge_status_matrix |
| `squash_option` never/always/default_on/default_off, per-MR `squash` | merge.rs:185,238; mod.rs:642 | COVERED | edge_cases (D6), mr_action_params |
| `merge_commit_template` / `squash_commit_template` applied at merge time | merge.rs:12-160 `expand_template` | IMPLEMENTED-UNTESTED (only read back through GraphQL fields; no scenario merges with a template). PARTIAL: `approved_by`/`reviewed_by` always empty, `co_authored_by` excludes MR author even when not squashing, `all_commits` includes merge commits, `issues` always empty | graphql_field_semantics (squash_commit_template set + `defaultSquashCommitMessage`) |
| `merge_commit_message` / `squash_commit_message` request params | merge.rs MergeParams | COVERED | mr_action_params:44 |
| `suggestion_commit_message` (project) | suggestions.rs:360 | IMPLEMENTED-UNTESTED (apply with custom setting not exercised; only default text) | none |
| `remove_source_branch_after_merge` (default for new MRs), `force_remove_source_branch`, `should_remove_source_branch` | mod.rs:648, merge.rs cleanup | COVERED (random_walk_merge) but default-from-project untested for MR created via GraphQL/push option/quick action | random_walk_merge |
| `printing_merge_request_link_enabled` ("To create a merge request..." remote messages) | push.rs:90 `mr_link_messages` | IMPLEMENTED-UNTESTED (no fixture contains the message) | none |
| `resolve_outdated_diff_discussions` | discussions.rs:69 | IMPLEMENTED-UNTESTED (one incidental scenario sets it) | none |
| `only_allow_merge_if_pipeline_succeeds`, `allow_merge_on_skipped_pipeline`, `only_allow_merge_if_all_discussions_are_resolved` | ci.rs:547-575, mergeability.rs | COVERED | merge_status_matrix, mergeability_checks |
| auto-merge ("merge when pipeline succeeds"/"checks pass"), `merge_after` | merge.rs, refresh.rs, `merge_after` x30 refs | COVERED | auto_merge_abort, merge_schedule, graphql_auto_merge_fields |
| `autoclose_referenced_issues` | stored/echoed (projects.rs:342) only | n/a (no issues) | |
| `mr_default_target_self` | absent from entity | n/a (GitLab exposes only when `forked?`) | |
| `ci_*` project settings (`ci_config_path`, `ci_default_git_depth`, `ci_forward_deployment_*`, `auto_cancel_pending_pipelines`, `restrict_user_defined_variables`...) | stored in settings blob, echoed (projects.rs:325-380) | PARTIAL: round-trip only, no behavioural effect (no CI runner) | project echo only |
| MR description templates (`.gitlab/merge_request_templates`) | templates.rs (API only) | COVERED. Note GitLab does not auto-apply templates on API MR creation, so none needed | mr_templates |
| Draft / mark ready / `Draft:` title prefix, `wip` | mod.rs, quick_actions.rs | COVERED | draft_quick_actions |
| Reviewers, re-request review, request changes, `reviewer.state` | reviews.rs | COVERED | reviews, random_walk_reviewers |
| Approvals (approve/unapprove; not reset on push) | reviews.rs | COVERED | approval_persistence |
| Suggestions (single, multi-line, batch) | suggestions.rs | COVERED | reviews, review_edges |
| Comment on file / image / multi-line (`line_range`) / outdated positions | positions.rs, discussions.rs | COVERED (line_range in 1 scenario) | diff_position_variants, review_commit_positions |
| Resolve threads, convert to thread, internal notes | discussions.rs | COVERED | review_resolved_notice, graphql_note_permissions |
| Quick actions on MR (assign, reviewer, label, milestone, draft/ready, target_branch, merge, rebase, approve/unapprove, submit_review, request_changes, copy_metadata, todo, subscribe, lock, title, react, estimate/spend, relate, ...) | quick_actions.rs | COVERED | quick_action_audit/combos/parsing, description_quick_actions |
| Push options `merge_request.{create,target,title,description,draft,label,unlabel,assign,unassign,milestone,remove_source_branch,squash}` | push_options.rs | PARTIAL-TEST: only `create`, `draft`, `draft=false` covered | push_followups |
| Push creates/updates MR diffs, auto-closes MR on merged branch | refresh.rs, push.rs | COVERED | push_side_effects(_2), push_followups |
| Subscriptions, todos on assign/mention/review request | subscriptions.rs, todos.rs, mentions.rs | COVERED | participants_sources, todo_lifecycle |
| System notes (all kinds) | notes.rs | COVERED | random_walk_*, quick_action_audit |
| Webhooks: merge_request, note, emoji, pipeline events (incl. `changes`, `object_attributes`) | hooks.rs | COVERED | webhooks*, quick_action_hooks, skip_ci_hooks |
| Lock on merge / discussion lock | resolvers.rs:2552 | COVERED | (discussion_locked x5); `lock_on_merge` label-level untested |
| Archived project read-only | | COVERED | archived_projects, archived_readonly |
| Time tracking | | COVERED | time_tracking_extremes |
| Fork-based MRs, cross-project pipelines | none | MISSING (no fork model) | |

## 4. Prioritised probes (MISSING / PARTIAL / IMPLEMENTED-UNTESTED)

Each probe is a recordable conformance scenario (run against real GitLab, then labrat). Preconditions assume an admin + a second developer, project with `initialize_with_readme`.

### P1 (bots/CLIs hit these constantly; a mismatch changes commit history)

1. **merge_commit_template at merge time** (IMPLEMENTED-UNTESTED, PARTIAL). Precondition: `PUT /projects/:id {"merge_commit_template": "Merge %{source_branch} into %{target_branch}\n\n%{title}\n\n%{description}\n\n%{reference}\n\n%{url}\n\n%{merged_by}\n\n%{merge_request_author}\n\n%{approved_by}\n\n%{reviewed_by}\n\n%{co_authored_by}\n\n%{all_commits}\n\n%{first_commit}\n\n%{first_multiline_commit}\n\n%{title_from_branch}\n\n%{issues}\n\n%{local_reference}\n\n%{source_project_id}"}`. MR with 2 commits by two different authors, dev approves (`POST /approve`) and is also a reviewer, admin merges (`PUT /merge`). Then `GET /repository/commits/:merge_commit_sha` and compare `message`. Variants: empty placeholders (blank-line collapsing), CRLF in template, `merge_commit_message` param overriding the template, template >500 chars, GraphQL `defaultMergeCommitMessage` before/after. Known labrat gap: `approved_by`/`reviewed_by` are hardcoded empty (merge.rs:120-121); GitLab emits `Approved-by: Name <email>` lines. `co_authored_by` exclusion rule (squash ? MR author : merger) and `all_commits` excluding merge commits also differ.
2. **squash_commit_template** (same file, squash path). `PUT /projects/:id {"squash_commit_template": "%{title}\n\n%{co_authored_by}\n\n%{all_commits}"}`, MR `squash: true`, 3 commits, `PUT /merge` with no `squash_commit_message`; also `squash_option=always` with MR created `squash:false` (labrat returns merge_error; verify exact REST status/message vs 18.11), `default_on` + explicit `squash:false`, `never` + `squash:true`. Read `squash_commit_sha` and the commit message.
3. **suggestion_commit_message** (IMPLEMENTED-UNTESTED). `PUT /projects/:id {"suggestion_commit_message": "Fix %{file_paths} (%{suggestions_count}) by %{username}/%{user_full_name} on %{branch_name} in %{project_name}/%{project_path}\n\n%{co_authored_by}"}`; create diff discussions with ```` ```suggestion ```` by the dev, apply one via `PUT /suggestions/:id/apply` and two via `PUT /suggestions/batch_apply` as admin, also with the request-level `commit_message` param; compare commit messages.
4. **printing_merge_request_link_enabled** (IMPLEMENTED-UNTESTED). Real git push of a new branch with the setting true then false, and again after an MR exists (message becomes "View merge request for <branch>:"). Compare stderr `remote:` lines; also default-branch push (no message) and push with `-o merge_request.create` (no link message).
5. **Push options beyond create/draft**. `git push -o merge_request.create -o merge_request.target=dev -o merge_request.title="T" -o merge_request.description="D" -o merge_request.label=a -o merge_request.label=b -o merge_request.assign=<user> -o merge_request.milestone=<title> -o merge_request.remove_source_branch -o merge_request.squash`; then push again with `merge_request.unlabel`, `unassign`, `merge_request.title=` update on the existing MR; `-o mr.create` alias; invalid target / unknown assignee (remote: messages). Compare `GET /merge_requests/:iid` and the push stderr.
6. **`GET /projects/:id/repository/commits/:sha/merge_requests`** (IMPLEMENTED-UNTESTED, used by danger/reviewdog/release tooling). Precondition: merged MR and an open MR whose branch contains the commit; probe with the head sha, the merge commit sha, the squash sha, a sha only on target branch, `?state=opened|merged|closed|all`, unknown sha (404), a non-code-reader user.

### P2 (setting honoured at create-time / edges)

7. **remove_source_branch_after_merge default**: project setting true; create MR through `POST /merge_requests`, `mergeRequestCreate`, push option `merge_request.create`, and quick action `/create_merge_request`; compare `force_remove_source_branch` and whether the source branch is gone after merge (also with `should_remove_source_branch:false` override and protected source branch).
8. **resolve_outdated_diff_discussions**: setting true; open a diff thread on line N, push a commit that changes that line (outdated), check `resolved`, `resolvable`, system note, and webhook; setting false for contrast.
9. **ci settings echo and effects**: `PUT /projects/:id` with `ci_config_path`, `ci_default_git_depth`, `ci_forward_deployment_enabled`, `restrict_user_defined_variables`, `auto_cancel_pending_pipelines`, `ci_separated_caches`, `ci_allow_fork_pipelines_to_run_in_parent_project`; diff `GET /projects/:id` fields.
10. **Fork and cross-project MR** (MISSING): `POST /projects/:id/fork`, then `POST /projects/:fork/merge_requests {target_project_id}`, `allow_collaboration`, `mr_default_target_self` (only exposed when forked), `GET /projects/:id/forks`. Large feature; decide scope first. At minimum probe the existing 422/404 for `target_project_id` pointing at an unrelated project and a nonexistent id.
11. **GraphQL `pipelineCreate`** (MISSING; CI tools). Mutation `pipelineCreate(input:{projectPath, ref, mergeRequestIid?})` on a project without `.gitlab-ci.yml` (expected error payload) and with one. Also `createBranch`, `branchDelete` (low).
12. **MR list `deployed_before/after`, `environment`** (accepted but ignored): with no environments, GitLab returns empty for `environment=x`; verify labrat returns empty rather than ignoring (merge_requests.rs:249 appears to handle the no-deployment case; add a scenario with a real deployment if deployments ever exist).
13. **GraphQL field args**: `MergeRequest.pipelines(status, sha, ref)`, `notes(filter: ONLY_COMMENTS|ONLY_ACTIVITY)`, `reference(full: true)`; `Project.mergeRequests(reviewStates, mergedAfter, milestoneWildcardId, not:{...}, or:{...}, sort: MERGED_AT_DESC/TITLE_ASC/PRIORITY_*)`; many are covered by graphql_mr_filters, confirm `sort` variants and `not`/`or`.
14. **lock_on_merge / `lockOnMerge` label behaviour** and `discussion_locked` + `PUT mergeRequestSetLocked` against notes permissions after merge.
15. **`closes_issues` / `related_issues`**: add a trivial scenario asserting `[]` and 404/403 behaviours (no issues involved), plus quick actions `/relate`.
16. **MR `merge` error matrix via REST status codes**: 405 (not mergeable/draft/discussions open/checks failing), 406 (`sha` mismatch), 409, 422 for each project setting combination (`merge_method=ff` diverged, `only_allow_merge_if_pipeline_succeeds` with running/failed/skipped pipeline and `allow_merge_on_skipped_pipeline`). Much exists in merge_status_matrix; add auto-merge + each setting.
