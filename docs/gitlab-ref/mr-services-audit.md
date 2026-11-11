# Audit: GitLab 18.11 merge-request services vs labrat

Method: read-only. For every MR-related service, worker and event subscriber in the GitLab CE source that a REST/GraphQL call or a
`git push` can trigger, grep labrat for the observable behaviour (note text, field, error message)
and read the matching code. GitLab paths are relative to the CE 18.11 source root; labrat paths are relative to
`/home/mabn/dev/shed/labrat/crates/labrat-core/src/services` unless they start with `labrat-api`. Nothing was executed, so
"modeled" means "the code path exists and the strings match", not "verified against the live instance".

Out of scope (skipped): CI pipelines/jobs
(`CreatePipelineService`, `AddTodoWhenBuildFails`, `UpdateHeadPipelineWorker`), forks (`LinkLfsObjects`, `from_fork`), issues
(`AssignIssues`, `CloseIssueWorker`, `CreateFromIssue`), emails and notification workers, UI-only controllers, usage counters.

Status: modeled / partially / missing. Test column = scenario to add to the conformance suite.

| GitLab service (file:line) | Trigger | Observable effect | labrat status and evidence | Suggested test |
|---|---|---|---|---|
| `merge_requests/refresh_service.rb:40-44,276-278` `mark_pending_todos_done` -> `todo_service.rb:119-121,219-225` | push to an MR source branch | the pusher's pending todos on that MR become `done` (all actions) | **missing**: `merge_requests/refresh.rs:13-185` never calls `todos::resolve_for_target` for source-branch pushes (only `mark_merged_by_push`, refresh.rs:213) | assign A, push as A, `GET /todos?state=pending` is empty |
| `refresh_service.rb:55-72` `close_upon_missing_source_branch_ref` + `close_service.rb:7-34` | push that deletes the source branch | MR `closed`, state event `closed`, auto-merge aborted, close hook | modeled: `refresh.rs:53-60`, `merge_requests/mod.rs:1249-1275` | delete branch via push, check state/hook/events |
| `refresh_service.rb:75-124` `post_merge_manually_merged` + `branch_push_merge_commit_analyzer.rb:5-61` | push of merge commit/fast-forward to target | MR `merged`, `merge_commit_sha` = the merge commit that contains the MR head, `merged_commit_sha` same, `merged_by` = pusher, state event source = merged MR or commit | partially: refresh.rs:66-86,194-215 sets both shas to the MR head instead of the analyzer's merge commit; no `merged_without_event_source` source MR | `git merge --no-ff` locally, push, compare `merge_commit_sha` |
| `refresh_service.rb:127-176` `reload_merge_requests` | push to source or target branch | new diff version, `merge_error=nil` for source pushes, `unchecked` | modeled: refresh.rs:88-117 | push, `GET /versions` |
| `refresh_service.rb:36-42` `outdate_suggestions`, `discussions/update_diff_position_service.rb:4-36` | push | moved positions, `change_position`, `resolved_by_push`, system note `changed this line in version N of the diff` | modeled: `discussions.rs:11-130` | outdated discussion scenario |
| `refresh_service.rb:259-272` `abort_auto_merges` | push to source branch | `aborted the automatic merge because the source branch was updated. [Learn more](<help url>).` | yes: `refresh.rs` builds the same text and URL | compare the note body byte for byte |
| `refresh_service.rb:274-286` `abort_ff_merge_requests_with_auto_merges` | push to target branch (ff project) | note `... because target branch was updated`, author gets `unmergeable` todo | modeled: refresh.rs:119-138 (todo only for the author; GitLab's `merge_participants` also includes `merge_user` when auto-merge is on) | ff project, 2 MRs, auto-merge |
| `refresh_service.rb:317-328` `comment_mr_branch_presence_changed` | branch removed/added | `deleted source branch`/`restored source branch` notes | modeled (note only reachable for MRs still open, as in GitLab): refresh.rs:153-170 | n/a |
| `refresh_service.rb:331-356` `notify_about_push` / `system_note_service.add_commits` | push | `added N commit(s)` note with `<ul>` list and compare link | modeled: refresh.rs:222-306 | push 12 commits, check collapsed items |
| `refresh_service.rb:358-376` `mark_mr_as_draft_from_commits`; `commit.rb:549-553` | push | title `Draft: ...`, note `marked this merge request as **draft** from <sha>` | partially: refresh.rs:308-338 anchors `fixup! `/`squash! ` at the title start and needs a space; GitLab's `DRAFT_REGEX` is `\A\s*<draft>\|(fixup!\|squash!)\s`, i.e. the second alternative is unanchored | commit titled `wip fixup!\tx` |
| `refresh_service.rb:Refresh::WebHooksWorker` | push | MR `update` hook with `oldrev` | modeled: `hooks.rs:574` | webhook after push |
| `merge_requests/post_merge_service.rb:22-45` | merge (API/auto/push) | state event, merged event, todos resolved for merger, hook, refs cleanup | modeled: merge.rs:380-420 | merge |
| `post_merge_service.rb:121-134` `cancel_auto_merges_targeting_source_branch` | merge with source-branch removal | other MR targeting the merged source branch with auto-merge: `aborted the automatic merge because target branch was merged in !N` + `unmergeable` todo | **missing**: nothing in merge.rs:440-505 | stacked MRs, auto-merge on child, merge parent with remove_source_branch |
| `merge_requests/retarget_chain_service.rb:7-31` via `workers/merge_requests/delete_source_branch_worker.rb:29` | merge with source-branch removal | up to 4 open MRs retargeted via `UpdateService`: note `deleted the \`x\` branch. This merge request now targets the \`y\` branch`, `retargeted=true`, hook | partially: merge.rs:455-488 writes the note, hook, new diff, but skips `can?(:update_merge_request)`, `updated_by_id`, `abort_auto_merge('target branch was changed')` (update_service.rb:236), todo resolution, `head_pipeline_id=nil` | stacked MRs with auto-merge on child |
| `merge_requests/close_service.rb` / `reopen_service.rb` | PUT state_event, `/close`, push | resource_state_events, close/reopen hooks, auto-merge abort, `merge_ref_sha=nil` | modeled: mod.rs:1249-1320 | open/close/reopen |
| `merge_requests/update_service.rb:15-18,36-64` `handle_changes` | PUT, GraphQL, quick actions | draft/ready notes, target-branch note, mentions todos, resolve todos, mark unchecked | modeled: mod.rs:1005-1120 | covered by prior edge-case docs |
| `update_service.rb:204-237` `handle_target_branch_change` | target_branch change | note `changed target branch from \`a\` to \`b\``, abort auto-merge `target branch was changed`, new diff | modeled: mod.rs:1089-1107 | retarget with auto-merge |
| `update_service.rb:277-283` `create_branch_change_note` with `delete` event | retarget after target deleted | note `deleted the \`a\` branch. This merge request now targets the \`b\` branch` | modeled: merge.rs:470-480 | see retarget row |
| `issuable_base_service.rb:361` `create_new_cross_references!`, `after_create_service.rb:50`, `notes/post_process_service.rb:18`, `system_notes/issuables_service.rb:246-264` | MR create/update (title, description), note create/edit | system note `mentioned in merge request !N` (and `mentioned in commit <sha>`) on the referenced MR | partially: `cross_refs.rs:56-99` handles pushed commits only; no `mentioned in merge request` string exists anywhere in labrat | MR A description says `see !B`; GET B notes |
| `system_notes/issuables_service.rb:223-232`, `merge_requests/update_service.rb` | description edit | note `changed the description` | modeled: mod.rs:1026-1035 | edit description |
| `system_notes/issuables_service.rb:178-190` `change_title` | title edit | `changed title from <code class="idiff">...` | modeled: `notes.rs:234` | rename |
| `system_notes/merge_requests_service.rb:29-35` `handle_merge_request_draft` | title Draft toggle | `marked this merge request as **draft**`/`**ready**` | modeled: mod.rs:1006-1017; repeated `Draft: Draft:` is stripped fully by `draftless_title` (mod.rs:249-262) whereas GitLab strips one prefix (`merge_request.rb:916-924`) | PUT title `Draft: Draft: x` draft=false |
| `update_assignees_service.rb:4-34`, `update_reviewers_service.rb:4-30`, `base_service.rb:195-224` | GraphQL `mergeRequestSetAssignees/Reviewers`; REST/PUT | keeps `first(1)`; early return when set unchanged (no hook, no touch); error `<Field> Cannot assign @u to !iid` for users who cannot read the MR | partially: `labrat-api/src/graphql/resolvers.rs:2736-2765` compares the un-truncated list to the current one, then `filter_users` (mod.rs:382-399) truncates, so APPEND fires an `update` hook and bumps `updated_by_id`; unreadable users are dropped silently, no error | APPEND second reviewer |
| `issuable.rb:170-173` `max_number_of_assignees_or_reviewers_message`, `notes/create_service.rb:136-152` | >200 assignees/reviewers | `Reviewers total must be less than or equal to 200` | missing (cap not enforced) | 201 reviewers via quick action |
| `handle_assignees_change_service.rb:13-27` | assignee change | note `assigned to @u`/`unassigned @u`, todo `assigned`, hook | modeled (async job): mod.rs:1121-1180 | assign |
| `request_review_service.rb:4-46`, `system_notes/issuables_service.rb:132-138` | `PUT reviewer_ids`, GraphQL rereview, `/request_review` | state `unreviewed`, approval removed, note `requested review from @u and removed approval`, todo `review_requested`, hook | modeled: `reviews.rs:241-300` | rereview approved reviewer |
| `update_reviewer_state_service.rb:4-52` | review state PUT, `/submit_review`, `/request_changes`, draft publish | `requested changes` / `left review comments` notes, `Reviewer has approved`, `Reviewer not found` | modeled: reviews.rs:202-240 | state matrix |
| `approval_service.rb:4-38`, `workers/merge_requests/create_approval_note_worker.rb`, `resolve_todos_after_approval_worker.rb`, `execute_approval_hooks_worker.rb` | POST approve | approval row, reviewer `approved`, note `approved this merge request`, todos resolved, `approved` hook, event | modeled: reviews.rs:68-122 | approve |
| `remove_approval_service.rb:4-62` | POST unapprove, request_changes | note `unapproved this merge request`, hook `unapproved`; silent when called from request-changes or re-request | modeled: reviews.rs:123-165,202-240 | unapprove |
| `draft_notes/create_service.rb:7-60` | POST draft_notes | `Thread to reply to cannot be found`, `Replies to system notes are not allowed`, `User is not allowed to resolve thread`, reviewer -> `review_started` | modeled: reviews.rs:349-420 | draft errors |
| `draft_notes/publish_service.rb:5-98,100-114` | publish / bulk_publish | review row, notes, todos, `review_submitted` todos, then **always** `ResolvedDiscussionNotificationService` (`resolved all threads` note + `update` hook when every thread is resolved) | partially: reviews.rs:444-554 creates the note only through `resolve_discussion` -> `all_resolved_notification` (comments.rs:487-540), i.e. only when a draft resolves a thread, and once per draft rather than once at the end | publish a plain comment on an MR whose threads are all resolved |
| `discussions/resolve_service.rb:30-65`, `unresolve_service.rb`, `notes/resolve_service.rb` | discussion/note resolve | `resolved all threads` note + hook + auto-merge event | modeled: comments.rs:451-540, `mr_auto_merge` job at :544 | resolve last thread |
| `notes/create_service.rb:9-120`, `notes/post_process_service.rb` | POST notes | quick actions, todos, suggestions, note hook, position capture | modeled: comments.rs:139-352 | note scenarios |
| `notes/create_service.rb:136-152` | quick action + `Reviewers ...` validation | `note.errors validation` | missing (see 200-cap row) | - |
| `notes/update_service.rb:7-80` | PUT note | `last_edited_at`, `updated_by`, mention todos, note hook `update`, suggestions rebuilt, quick-only note deleted | modeled: comments.rs:355-433; MR `updated_at` unthrottled (comments.rs:403) | edit twice within a minute |
| `notes/destroy_service.rb:5-30`, `note.rb:211` (after_save only) | DELETE note | todos for the note destroyed, MR timestamps unchanged | partially: `comments.rs:436-448` sets MR `updated_at = now` | delete note, compare MR `updated_at` |
| `suggestions/create_service.rb`, `apply_service.rb`, `base_service.rb:54-90`, `outdate_service.rb` | note create/edit, apply, batch apply | suggestion rows, all 11 error messages, commit message template, `Co-authored-by` | modeled: `suggestions.rs:131-405` | apply errors |
| `system_notes/time_tracking_service.rb:45-126`, `add_spent_time_service.rb` | `/time_stats`, `/spend`, GraphQL | notes `changed time estimate to ...`, `added 1h of time spent at <date>`, `removed time spent`, timelogs | modeled: `labrat-api/src/routes/merge_requests.rs:1480-1580`, mod.rs:830-840 | spend/estimate |
| `system_notes/merge_requests_service.rb:6-27` | merge with auto-merge, cancel | `enabled an automatic merge when all merge checks for <sha> pass`, `canceled the automatic merge` | modeled: merge.rs:591-695 | auto-merge cycle |
| `auto_merge/base_service.rb:65-72`, `merge_requests/update_service.rb`, `events/merge_requests/*` | draft toggle, discussions resolved, mergeable, title edit | `ProcessAutoMergeFromEvent` merges when unblocked | modeled: mod.rs:1111, comments.rs:544, mergeability.rs:94 | ready toggles auto-merge |
| `merge_requests/rebase_service.rb:4-62`, `lib/api/merge_requests.rb:908-917` | PUT rebase, `/rebase` | `rebase_in_progress`, errors `Source branch does not exist`, `Cannot push to source branch`, `Source branch is protected from force push`, `Rebase failed: ...`, push refresh | partially: `merge.rs:511-590`; the `The rebase pre-receive hook failed: <msg>.` variant is not produced | rebase onto protected-branch hook |
| `merge_requests/merge_service.rb`, `merge_orchestration_service.rb`, `squash_service.rb` | PUT merge, GraphQL accept, `/merge` | merge errors (`Branch has been updated since the merge was requested.`, `You are not allowed to merge this merge request`, squash messages) | partially: all strings present except `The target branch is scheduled for deletion. Please wait for the branch to be retargeted.` and `Can't abort the automatic merge` | - |
| `merge_requests/mergeability_check_service.rb`, `merge_request.rb:317-323` | any mergeability recompute | on `unchecked/checking -> cannot_be_merged` (opened, has commits, branch present): `unmergeable` todo for author (+ `merge_user` if auto-merge) | **missing**: `mergeability.rs:66-95` only writes `merge_status`; `UNMERGEABLE` is created only in refresh.rs:137 | create MR with conflict, `GET /todos` |
| `mergeability/*` (CE checks) | GET MR `detailed_merge_status` | `not_open`, `draft_status`, `merge_time`, `discussions_not_resolved`, `ci_must_pass`, `commits_status`, `need_rebase`, `conflict` | modeled: mergeability.rs:170-260 | prior docs |
| `merge_requests/after_create_service.rb:5-77`, `create_service.rb` | POST MR | diff, refs, `prepared_at`, hooks, mention/reviewer/assign todos, `preparing` -> checked | modeled: mod.rs:567-760 | create |
| `merge_requests/build_service.rb:211-260` | push option create (no title), web | title/description from the single commit, or `first_multiline_commit`; draft when no commits; default template `.gitlab/merge_request_templates/Default.md` | partially: `push_options.rs` uses the single-commit case or humanized branch only | push 3 commits, one with a body |
| `merge_requests/push_options_handler_service.rb:46-190` | `git push -o merge_request.*` | create/update, errors, `draft` | partially: `push_options.rs:90-245` matches errors and limits; `draft=false` on an existing MR strips the prefix (push_options.rs:183-186) while GitLab does nothing (only truthy `draft` is applied, `update_service.rb:15-17`, and `set_draft_status` recomputes the column); `target_project` ignored (forks) | `-o merge_request.draft=false` |
| `merge_requests/get_urls_service.rb` + `internal_post_receive/response.rb:32-34` | push | `View merge request for b:` / `To create a merge request for b, visit:` | modeled: `push.rs:94-100` | clone/push |
| `merge_requests/update_merge_schedule_service.rb` | `merge_after` | schedule row | modeled: mod.rs:826-828,1340-1360 | prior docs |
| `merge_requests/delete_non_latest_diffs_service.rb` + `DeleteDiffFilesWorker` | after merge | older versions lose their files after >= 5 min | missing (no equivalent); only visible after a wait | low value |
| `merge_requests/add_context_service.rb` | POST context_commits | context commits | missing (known, coverage-gaps A2-A4) | - |
| `todo_service.rb` `new_note`, `update_merge_request`, `reassigned_*`, `new_review`, `merge_merge_request`, `close_merge_request` | many | todos with the right action/author | modeled: `todos.rs`, `mentions.rs`; `@group`/`@all` mentions not expanded (`mentions.rs:40-60` resolves only usernames; GitLab `user_parser.rb:9-23,101-106`) | mention a group |
| `quick_actions/merge_request_actions.rb`, `issue_and_merge_request_actions.rb`, `issuable_actions.rb` | notes/description/PUT | commands | partially: `quick_actions.rs:17-64` lacks `/copy_metadata`, `/run_pipeline`, `/board_move`-like issue commands (skipped); everything else present | `/copy_metadata !N` |
| `lib/api/merge_requests.rb` `subscribe`, `todo`, `time_stats` | REST | subscription/todo creation | modeled: `subscriptions.rs`, `todos.rs:164` | prior docs |

## Ranked list: the 15 most likely real gaps

Ranking weighs how plausibly a bot or CLI (glab, python-gitlab, renovate, reviewdog, a review bot) hits it and how visible the difference is over the API.

### 1. Pending todos are not resolved when the user pushes to the MR source branch
- GitLab: `refresh_service.rb:40-44` calls `mark_pending_todos_done(mr)` (`:276-278`) for every open MR whose `source_branch` is the pushed branch; it runs `TodoService#merge_request_push` -> `resolve_todos_for_target(mr, current_user)` (`todo_service.rb:119-121,219-225`) with `resolved_by_action: :system_done`.
- Expected: every pending todo of the pusher on that MR (assigned, review_requested, review_submitted, mentioned, directly_addressed, marked, unmergeable) becomes `state: "done"`; `GET /todos?state=pending` no longer lists it, `GET /todos?state=done` does; `POST /todos/:id/mark_as_done` on it is a no-op.
- labrat: `merge_requests/refresh.rs:13-185` has no such call.

### 2. No `mentioned in merge request !N` cross-reference notes
- GitLab: `after_create_service.rb:50` (`create_cross_references!`), `issuable_base_service.rb:361` (`create_new_cross_references!` on update), `notes/post_process_service.rb:18` (new notes) and `notes/update_service.rb:57`; the note is built by `system_notes/issuables_service.rb:246-264`, body = `"mentioned in " + gfm_reference` where `mentionable.rb:36-41` gives `merge request !5`. Action `cross_reference`, author = the MR author/commenter. Skipped for a self-reference and when the same text already exists on the target (`:496-507`); the reader must be allowed to see the source.
- Expected: after `description: "depends on !5"` on MR !7, `GET /projects/:id/merge_requests/5/notes` contains a system note `mentioned in merge request !7`. Same for a comment `see !5` and for `group/project!5`.
- labrat: `cross_refs.rs:56-99` handles commits only; `grep "mentioned in merge request"` finds nothing.

### 3. MR `updated_at` is bumped wrongly by note edit, create, resolve and especially delete
- GitLab: `note.rb:211` `after_save :touch_noteable` (so not on destroy) -> `MergeRequest#touch` through `ThrottledTouch` (only when `updated_at` is more than one minute old); `Notes::DestroyService` (`notes/destroy_service.rb:5-30`) touches nothing. Single-note resolve is throttled; `Discussions::ResolveService` uses `update_all` and does not touch (`docs/gitlab-ref/edge-cases-mr-timestamps.md` rows 28-31).
- Expected: `DELETE .../notes/:id` leaves the MR `updated_at` unchanged; editing or creating a note within 60 s of the last update leaves it unchanged.
- labrat: `comments.rs:328-332` (create), `:403` (edit), `:444` (delete), `:468,479` (resolve) run an unconditional `UPDATE merge_requests SET updated_at = now`, overriding the throttled `notes::touch_noteable` (`notes.rs:173-182`). Affects `order_by=updated_at`, `updated_after`, and the `updated_at` of webhook payloads. (Confirm create/edit against the live instance; delete is documented in the timestamps file.)

### 4. No `unmergeable` todo when a conflict is detected
- GitLab: `merge_request.rb:317-323`: `after_transition [:unchecked, :checking] => :cannot_be_merged` runs, if `notify_conflict?` (`:1767-1775`: opened/locked, has commits, branch present, `!repository.can_be_merged?`), `TodoService#merge_request_became_unmergeable` -> a todo for each of `merge_participants` (author, plus `merge_user` when auto-merge is on) (`todo_service.rb:137-146,443-447`, `merge_request.rb:1099-1107`).
- Expected: `GET /todos` for the author shows `action_name: "unmergeable"`, `target_type: "MergeRequest"`, `body` = MR title, `author` = the author themself, created when the first mergeability check finds a conflict (not again on re-checks from `cannot_be_merged_recheck`).
- labrat: `mergeability.rs:66-95` just stores the status; the `UNMERGEABLE` constant is used only at `refresh.rs:137`.

### 5. `resolved all threads` note + `update` hook missing on review publish
- GitLab: `draft_notes/publish_service.rb:100-114` `handle_notifications` always runs `ResolvedDiscussionNotificationService#execute(send_notifications: false)` (`merge_requests/resolved_discussion_notification_service.rb:5-14`) once after the whole publish; it adds the system note `resolved all threads` (action `discussion`) and fires the MR `update` webhook whenever `discussions_resolved?` (`noteable.rb:158-160`: at least one resolvable thread and none open).
- Expected: an MR whose threads are all resolved gets another `resolved all threads` note and an `update` hook each time any draft/review is published (even a plain top-level comment), after the review notes; with several resolving drafts it appears once, at the end.
- labrat: `reviews.rs:444-554` only reaches `all_resolved_notification` via `comments::resolve_discussion` (`comments.rs:487-540`), per draft and only for drafts that resolve.

### 6. Manual merge by push stores the wrong `merge_commit_sha`
- GitLab: `refresh_service.rb:75-124` runs `Gitlab::BranchPushMergeCommitAnalyzer` (`branch_push_merge_commit_analyzer.rb:5-61`): `merge_commit_sha = merged_commit_sha = analyzer.get_merge_commit(diff_head_sha)`, the merge commit (first-parent chain) of the push that contains the MR head; `MergeRequests::PostMergeService.execute(mr, source)` uses an earlier merged MR with the same sha (`merged_without_event_source`) or the commit as the state-event source.
- Expected: after `git merge --no-ff feature && git push origin main`, `merge_commit_sha` and `merged_commit_sha` are the new merge commit, `merged_by` is the pusher; with a fast-forward push they equal the MR head.
- labrat: `refresh.rs:194-215` writes the MR head into both fields (`merge_commit_sha = ?3 = head`).

### 7. Merging a parent MR does not abort auto-merge on the stacked child (`target branch was merged in !N`)
- GitLab: `post_merge_service.rb:121-134`: when the merge removes its source branch, each open MR targeting that branch with `merge_when_checks_pass` gets `abort_auto_merge_with_todo(..., "target branch was merged in !#{iid}")` (`base_service.rb:324-330`): system note `aborted the automatic merge because target branch was merged in !12` (author = the merger) and an `unmergeable` todo for the child's `merge_participants`.
- Expected on the child: `merge_when_pipeline_succeeds: false`, `merge_user: null`, that note, then the retarget note.
- labrat: `merge.rs:440-505` retargets without any abort.

### 8. Retargeted child MRs skip most of `UpdateService`
- GitLab: `retarget_chain_service.rb:7-31` calls `UpdateService` with `target_branch_was_deleted: true` only for MRs where the merger `can?(:update_merge_request)`; `update_service.rb:204-237` then runs `abort_auto_merge('target branch was changed')`, sets `retargeted`, nils `head_pipeline_id`, `updated_by_id` = merger, resolves the merger's todos (`resolve_todos` via changed fields) and `mark_as_unchecked`; hook `changes` contain `target_branch`.
- Expected: child `updated_by_id` = merger, auto-merge cleared with note `aborted the automatic merge because target branch was changed`, child not touched if the merger lacks update rights.
- labrat: `merge.rs:455-500` writes `target_branch`, `retargeted`, the note and a hook; no permission check, no `updated_by_id`, no abort.

### 9. GraphQL `mergeRequestSetReviewers/Assignees`: early return and error text
- GitLab: `update_reviewers_service.rb:4-30` / `update_assignees_service.rb:4-34`: ids are truncated to `first(1)` before comparing (`:25` `reviewer_ids`), an unchanged set returns the MR with no update, no hook, no touch; a user who cannot read the MR adds `merge_request.errors` `"Cannot assign @u to !iid"` (`update_service.rb:344-356`) and nothing changes (mutation `errors: ["Reviewers Cannot assign @u to !5"]`-style full message on the attribute).
- Expected: `operationMode: APPEND` with a second reviewer while one exists returns the MR unchanged, no `update` webhook, `updated_at`/`updated_by` unchanged.
- labrat: `labrat-api/src/graphql/resolvers.rs:2736-2765` compares the untruncated list, so it calls `mrs::update`, which fires the hook (`mod.rs:1111-1119`); unreadable users are silently dropped (`mod.rs:382-399`).

### 10. Push options: multi-commit defaults, `draft=false`, empty-branch draft
- GitLab: `build_service.rb:211-224`: with exactly one commit use its title/description, otherwise use `first_multiline_commit` (oldest non-merge commit with a body: `merge_request.rb:1999-2003`) for both title and description; `set_draft_title_if_needed` (`:175-179`) makes the MR a draft when there are no commits; a repository `Default.md` MR template fills an empty description (`:262+`). `update_service.rb:15-17` applies `draft` only when truthy, and `set_draft_status` (`merge_request.rb:3150`) recomputes the column from the title.
- Expected: `git push -o merge_request.create` with 3 commits where the second has a body gives that commit's subject and body; `-o merge_request.draft=false` on a draft MR leaves it draft.
- labrat: `push_options.rs:183-186,227,280-330` handles the single-commit case, uses the humanized branch otherwise, and un-drafts on `false`.

### 11. `@group` and `@all` mentions create no todos and no participants
- GitLab: `lib/banzai/reference_parser/user_parser.rb:9-23,101-106` expands group mentions (not `mentions_disabled`) to the members the author may notify; `TodoService#filter_mentioned_users` (`todo_service.rb:499-510`) turns them into `mentioned` todos; they also appear in `participants`.
- Expected: a comment `@dev-team please review` yields `mentioned` todos for each member who can read the MR.
- labrat: `mentions.rs:40-60` resolves usernames only; `subscriptions.rs:participant_ids` the same.

### 12. Draft detection from commits: unanchored `fixup!`/`squash!` and repeated prefixes
- GitLab: `commit.rb:549` `DRAFT_REGEX = /\A\s*<draft>|(fixup!|squash!)\s/` (second branch matches anywhere in the title); `merge_request.rb:916-924` removes a single prefix (`sub`).
- Expected: a pushed commit titled `Tidy up fixup! of x` counts as a draft commit (the MR gets `Draft: ` and the note `marked this merge request as **draft** from <sha>`); `PUT title="Draft: Draft: x"` with `draft` -> ready leaves `Draft: x`.
- labrat: `refresh.rs:323` requires `fixup! `/`squash! ` at the start; `mod.rs:249-262` loops and strips every prefix.

### 13. `/copy_metadata` (and other MR commands) are not recognised
- GitLab: `lib/gitlab/quick_actions/issue_and_merge_request_actions.rb` `command copy_metadata` copies labels and milestone from another issuable and answers `Copied labels and milestone from !N.`; `/run_pipeline` in `merge_request_actions.rb`.
- Expected: the command line is consumed and applied, `commands_changes` has `label_ids`/`milestone_id`.
- labrat: `quick_actions.rs:17-64` has no `copy_metadata`; the line stays in the note body.

### 14. 200 assignee/reviewer cap not enforced
- GitLab: `Issuable::MAX_NUMBER_OF_ASSIGNEES_OR_REVIEWERS = 200` (`issuable.rb:37,170-173`, `notes/create_service.rb:136-152`): message `total must be less than or equal to 200` prefixed by the attribute, e.g. `Reviewers total must be less than or equal to 200` through quick actions.
- labrat: no such string anywhere.

### 15. `DeleteNonLatestDiffsService` after merge
- GitLab: `post_merge_service.rb:34` schedules `DeleteDiffFilesWorker` in batches of 10 at `index * 5.minutes`; older `merge_request_diffs` of a merged MR end up `without_files`.
- Expected (after at least five minutes): `GET /merge_requests/:iid/versions/:id` for an old version returns empty `diffs` and `commits` for it; `GET /versions` still lists the versions. Not verified against the live instance.
- labrat: no equivalent job. Only observable with time travel, so lowest priority.

## Notes on areas checked and found aligned (no gap found)
Close/reopen, auto-merge enable/cancel/abort (all five call sites except two above), assign/reviewer notes and todos, approvals (including the silent unapprove on request-changes), review state machine and messages, draft-note validation errors, suggestion apply errors, time tracking notes, push-option errors and limits, push URL messages, `added N commits` note format, outdated-position handling, mergeability detailed statuses (CE set).
