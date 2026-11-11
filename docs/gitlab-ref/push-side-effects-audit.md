# Push side effects on merge requests: GitLab CE 18.11 vs labrat

Audit of what a branch push does to merge requests, read from the GitLab source and compared with
labrat. Status: IMPLEMENTED, PARTIAL, MISSING, N/A (feature does not exist in labrat).
"Scenario" = conformance scenario in `tests/conformance/tests/scenarios/` that exercises it
(fixtures in `tests/conformance/fixtures/` are recordings of real GitLab).

Earlier notes: `mr-services-audit.md` (some of its "missing" rows are stale: pusher to-do resolution
and the merge-commit analyzer are now implemented).

## 0. Pipeline of a push in GitLab

`PostReceive` -> `Git::ProcessRefChangesService` -> `Git::BranchPushService` (`app/services/git/branch_push_service.rb`):

1. `UpdateMergeRequestsWorker` (async, Sidekiq) -> `MergeRequests::RefreshService`.
2. `Git::BranchHooksService` (`BaseHooksService#execute`): push `Event`, `Ci::CreatePipelineService.execute_async(:push)`,
   push webhooks/integrations, `ProcessCommitWorker` per commit that matches the cross-reference regex (max 100),
   `ProjectCacheWorker`.
3. Remove branch: `Environments::StopService`, `RefDeleteUnlockArtifactsWorker`.

`RefreshService#refresh_merge_requests!` order (`app/services/merge_requests/refresh_service.rb:34-66`):

1. `find_new_commits` -> `@commits` (oldest first; `commits_between(old,new)`; branch added: commits since merge-base with the
   first open MR for that source branch, else `[]`; branch removed: `[]`).
2. `close_upon_missing_source_branch_ref` (CloseService as the pusher).
3. `post_merge_manually_merged` (merged by push).
4. `link_forks_lfs_objects`.
5. `reload_merge_requests`: open MRs with source or target == branch (plus fork MRs): source match or force push ->
   `reload_diff` + `merge_error = nil`; otherwise if the MR's diff contains any pushed commit -> `reload_diff`; then
   `mark_as_unchecked` for all of them.
6. For each open MR of the source branch: `outdate_suggestions`, `abort_auto_merges`, `mark_pending_todos_done`.
7. `abort_ff_merge_requests_with_auto_merges` (target branch).
8. `cache_merge_requests_closing_issues`.
9. For each open MR of the source branch: branch-presence note, `notify_about_push` (`add_commits` system note +
   `NotifyAboutPushWorker` email), `mark_mr_as_draft_from_commits`.
10. Async: `Refresh::WebHooksWorker` (MR `update` hook with `oldrev`), `Refresh::PipelineWorker` (MR pipelines).

labrat runs all of this synchronously inside `post_receive` (`push.rs:29`, hook `refresh::on_push`, `refresh.rs:15`), except that the
refresh of other MRs after an MR merge is a queued job (`DEFER_REFRESH`, `refresh.rs:16-29`). In GitLab an immediate GET after a
push can still see the old state; labrat scenarios use `settle()/poll_mr` so this is not asserted.

## 1. (a) Push to an MR's source branch

| # | Side effect (GitLab) | labrat status | Evidence | Scenario |
|---|---|---|---|---|
| S1 | New diff version (`ReloadDiffsService`: `refs/merge-requests/N/head` updated, `merge_request_diffs` row, `latest_merge_request_diff_id`) for every open MR whose source branch is the pushed one; also after a force push | IMPLEMENTED | `refresh.rs:118-130` -> `diffs::create_version` (`diffs.rs:125`) | `mr_diff_git`, `random_walk_force` (versions after FF/force pushes) |
| S1b | Version limits `DIFF_VERSION_LIMIT` (1000 versions) / diff commits limit: no new version once reached (`reload_diffs_service.rb:13-14`, behind flags `merge_requests_diffs_limit`/`merge_requests_diff_commits_limit`) | MISSING | `create_version` has no limit | none (impractical) |
| S1c | Empty diff (`state: empty`) when source head equals target; versions API shows it | IMPLEMENTED | `diffs.rs:138-139` | `mr_diff_git` ("after empty commit") |
| S2 | `merge_error = NULL` for source-branch pushes and for any force push | IMPLEMENTED | `refresh.rs:130-135` | none asserts it |
| S3 | `merge_status` -> `unchecked` / `cannot_be_merged_recheck` (`mark_as_unchecked`, no real-time trigger for non-source) | IMPLEMENTED | `mergeability::mark_as_unchecked` (`mergeability.rs:23`), called `refresh.rs:149` | `mr_diff_git`, `auto_merge_abort`, many random walks |
| S4 | Discussion positions re-traced, outdated threads get `change_position`, `resolved_by_push` (project setting `resolve_outdated_diff_discussions`), system note "changed this line in version N of the diff" | IMPLEMENTED | `discussions::update_diff_positions` (`discussions.rs:11`), called `refresh.rs:136-143` | `random_walk_force`, `review_commit_positions` |
| S5 | `Suggestions::OutdateService`: active suggestions whose lines changed get `outdated: true` | PARTIAL | `discussions.rs:121-125` flags suggestions of notes that have a `change_position`; GitLab recomputes `suggestion.outdated?(cached: false)` for every active suggestion of the MR (content compare), also when the discussion position still traces | `api_error_paths_2` ("apply outdated") |
| S6 | Auto-merge abort when `merge_params['sha'] != newrev` (all open MRs of the source branch): clears auto-merge fields, note `aborted the automatic merge because the source branch was updated. [Learn more](<help_page_url ci/pipelines/merge_trains.md#merge-request-dropped-from-the-merge-train>).` (note the trailing `.`; URL is the instance's own `/help/...`) | DONE | `refresh.rs`: same text, trailing `.` and help URL | none (fixtures only contain "target branch was updated/merged in !N") |
| S7 | `mark_pending_todos_done`: `TodoService#merge_request_push` -> pusher's pending to-dos on the MR -> `done` (`resolved_by_action: system_done`, `snoozed_until: nil`), count cache refreshed | PARTIAL | `refresh.rs:155-158` -> `todos::resolve_for_target` (`todos.rs:149`); count cache by SQL triggers (`schema.rs:640`); `snoozed_until` is NOT cleared, no `resolved_by_action` column | none (the push path is not exercised; `todo_resolution` covers label/thread resolution only) |
| S8 | Branch deleted: `CloseService` as pusher: state `closed`, `latest_closed_by/at`, state event, `closed` event, `merge_error` cleared, to-dos of the pusher resolved, auto-merge abort `merge request was closed`, counts invalidated, `merge_request` hook `close` | IMPLEMENTED | `refresh.rs:83-92` -> `merge_requests::close` (`mod.rs:1301`) | `api_error_paths_2` and `graphql_mr_all_scalars` (REST `DELETE .../branches/:b`; no `git push origin :b`) |
| S8b | Close side effects not modelled: `cleanup_refs` (cleanup schedule 14 days), `cleanup_environments`, notification email | MISSING / N/A | grep finds no cleanup schedule; environments/email not in labrat | none |
| S9 | Branch re-created (`oldrev` = 0) while the MR is closed: nothing happens to the closed MR (only OPEN MRs are refreshed; MR is not reopened); `restored source branch` note only for an MR still open | IMPLEMENTED (note unreachable for normal flows, as in GitLab) | `refresh.rs:119-123` (open only), `refresh.rs:190-198` | none |
| S9b | Re-created branch, open MR (only possible if MR stayed open): commits = `commits_between(merge_base(mr.diff_head_sha,newrev), newrev)` | PARTIAL | labrat uses "commits not on default branch" (`refresh.rs:57-72`) | none (unreachable) |
| S10 | `add_commits` system note (`SystemNotes::CommitService#add_commits`): body `added N commit(s)\n\n<ul><li>SHA8 - title</li>...</ul>\n\n[Compare with previous version](/<path>/-/merge_requests/<iid>/diffs?diff_id=<latest diff id>&start_sha=<oldrev>)`; commits already in the target (e.g. merged from main) summarised as `<li>OLD8...LAST8 - N commits from branch <code>main</code></li>` first; more than 10 new commits: first N-10 collapsed to `<li>A...B - K earlier commits</li>`; `metadata.action = commit`, `commit_count` | IMPLEMENTED | `add_commits_note` (`refresh.rs:255-343`) | fixtures: 1-commit, "N commits from branch main" (`random_walk_force`, `closed_mr_updates`...); the ">10, earlier commits" form has no scenario |
| S10b | Commit title in the `<li>`: `Commit#title` = first line, cut to 81 chars + `...` when >= 100 chars; blank message -> `No commit message` | PARTIAL | `refresh.rs:305` uses `Commit::title()` = raw first line (`labrat-git/src/object.rs:201`) | none |
| S10c | Force push that only rewinds (new is an ancestor of old): `@commits` empty -> no `added` note, but a new diff version | IMPLEMENTED | `pushed.is_empty()` guard `refresh.rs:206` | `random_walk_force` (reset-like steps) |
| S10d | `NotifyAboutPushWorker`: `push_to_merge_request` emails to participants | N/A (no email) | | |
| S11 | Draft from commit titles (`Draft:`, `[Draft]`, `(Draft)`, `fixup! `, `squash! `): title -> `Draft: <title>` via plain `merge_request.update(title:)` (touches `updated_at`, no hook of its own), note `marked this merge request as **draft** from <full 40-char sha>` (`commit.to_reference(project)` = `commit.id`), only when a draft commit in the push is part of the MR (`commit_shas.include?`), MR not already draft; first such commit in push order | PARTIAL | `auto_draft_from_commits` (`refresh.rs:345-383`): (1) sha cut to 8 chars `&o.hex()[..8]`; (2) pushed commits are not filtered by the MR's commit set (a `fixup!` commit brought in from main by a merge counts); (3) raw SQL title update does not bump `updated_at`; (4) `fixup!`/`squash!` only recognised at title start with a space (`DRAFT_REGEX` second alternative is unanchored: `wip fixup!\tx` matches) | none |
| S12 | MR `update` webhook with `object_attributes.oldrev` (omitted when branch created), `action: update`, sent after everything above (async worker, so already includes the new title/draft state, `last_commit`, `merge_status`) | IMPLEMENTED | `hooks::merge_request_push_update` (`hooks.rs:574`), called `refresh.rs:218-224` | `webhooks` ("push with open mr": push hook + MR hook) |
| S13 | Push event (`Event` action pushed) and push webhook (also for tags); `push_event_hooks_limit` | IMPLEMENTED | `hooks::on_push` (`hooks.rs:887`), `push.rs` grouping | `webhooks`, `system_hooks_repo`, `events` |
| S14 | `mentioned in commit <sha>` notes on MRs referenced (`!N`, `group/proj!N`, URL) in pushed commit messages, via `ProcessCommitWorker` (async), only for commits matching the cross-reference regex, last 100 commits, once per MR/commit | PARTIAL | `cross_refs.rs:56-100` (queue job, dedupe); no 100-commit cap, no pre-filter | `random_walk_hooks`, `closed_mr_updates`, `revert_cherry_pick_mr` fixtures contain the note |
| S15 | MR head pipeline / MR pipelines: `Refresh::PipelineWorker` -> `CreatePipelineService` for MRs with CI config; head pipeline keeps pointing to the old pipeline until a new one is created; `UpdateHeadPipelineForMergeRequestWorker` | N/A (labrat has no pipeline execution, only external commit statuses); `diff_head_pipeline` is derived from the new head sha | `ci.rs:320-352` | `statuses`, `pipeline_endpoints` |
| S18 | Cached closing issues (`MergeRequestsClosingIssues`), `closes_issues` | N/A (no issue tracker; `closes_issues` returns `[]`, `routes/merge_requests.rs:47`) | | |
| S19 | Forks: MRs whose source project is the pushed (fork) project and target upstream are refreshed (`merge_requests_for_forks`), LFS link | N/A (no forks) | | |
| S20 | Locked MRs, `stale_replica`, `merge_jid` reset | N/A | | |

### Force push specifics (source branch)

Same as S1-S12. `force_push?` = not ancestor (`Gitlab::Checks::ForcePush`, false for create/delete) and labrat matches (`refresh.rs:55`).
Additionally a force push to a branch that is only the TARGET of an MR also reloads the diff and clears `merge_error` (see T2/T3).

### Push of a merge commit containing other MRs' commits (source branch)

Pushed commits = `old..new`. The `added` note partitions them by the MR's commits (latest diff commit shas): commits in the MR are
"new", the rest (target-branch commits pulled in by the merge) are "existing" (S10, covered by fixtures). Other MRs:

* MR B whose TARGET is this branch (stacked): its diff contains a pushed commit -> `reload_diff`, `unchecked`, no notes (T1/T2); if B's
  head is among the pushed commits -> merged by push (T4).
* MR C whose source is a different branch: not touched, even if its head is in the push.

## 2. (b) Push to an MR's target branch (the MR's source branch is not pushed)

| # | Side effect (GitLab) | labrat status | Evidence | Scenario |
|---|---|---|---|---|
| T1 | New diff version only when the push is a force push, or the MR's current diff contains at least one pushed commit (`includes_any_commits?`), e.g. a rebase or merge-in; plain target pushes keep the MR's diff | IMPLEMENTED | `refresh.rs:144-152` | `mr_diff_git` ("target push", "versions after target push") |
| T1b | `ReloadDiffsService` also re-traces discussion positions for those new versions | PARTIAL | the `else if` branch (`refresh.rs:144-152`) calls `create_version` but not `update_diff_positions` (only source/force branch does, `refresh.rs:136`) | none |
| T2 | `merge_status` -> `unchecked` / `cannot_be_merged_recheck` for ALL open MRs with that target (no real-time GraphQL trigger) | IMPLEMENTED | `refresh.rs:149` | `mr_diff_git`, `auto_merge_abort` |
| T3 | `merge_error` is NOT cleared for non-force target pushes | IMPLEMENTED | `refresh.rs:130` condition | none |
| T4 | Merged by push (`post_merge_manually_merged`): open MRs targeting the branch, diff state != empty, `diff_head_sha` among pushed commits -> `PostMergeService`: `mark_as_merged`, `merge_commit_sha = merged_commit_sha = BranchPushMergeCommitAnalyzer` result, `merged_by` = pusher, `merged` event, state event whose source is an earlier merged MR with that sha (`merged_without_event_source`) else the commit, pusher's to-dos resolved, `merge` webhook, counts invalidated, `merge_error` cleared, first_contribution | PARTIAL | `mark_merged_by_push` (`refresh.rs:229-253`), analyzer `merge_commit_analysis` (`refresh.rs:385`). Gaps: no `source_merge_request` lookup (`source_merge_request_id` never written, schema has the column); target branch just created: GitLab sets `@commits = []` (nothing merged) vs labrat computing commits against default; `delete_non_latest_diffs` not done; refs cleanup schedule not done | `push_followups` ("merged by push" + notes), nothing for stacked/merge commit lookup |
| T4b | `DeleteNonLatestDiffsService` (merged MR, in 5-minute batches via `DeleteDiffFilesWorker`): old versions lose files | MISSING (also for the normal merge path) | grep: no hits | none (needs a 5+ min wait) |
| T5 | Fast-forward / rebase-merge projects: `abort_ff_merge_requests_with_auto_merges`: for MRs with auto-merge `merge_when_checks_pass` targeting the branch that now `should_be_rebased?`: abort with `aborted the automatic merge because target branch was updated` and `unmergeable` to-do for `merge_participants` (author only, auto-merge was just cleared) | IMPLEMENTED | `refresh.rs:157-176` (not run when the target branch is deleted: GitLab does run it, `should_be_rebased?` then evaluated on a missing branch) | `auto_merge_abort` |
| T6 | No notes, no todos resolved, no `update` webhook, no suggestion outdating for MRs that only target the branch | IMPLEMENTED | `if is_source` guard `refresh.rs:178` | `mr_diff_git` |
| T7 | `unmergeable` to-do (+email) when the recheck moves `unchecked/checking` -> `cannot_be_merged` for an open MR that has commits and a real conflict (`merge_request.rb:317-323`, `notify_conflict?`) | MISSING | `mergeability::check` (`mergeability.rs:66-95`) only writes the status; `UNMERGEABLE` is created only in `refresh.rs:172` and `merge.rs:419` | `audit_followups_3` ends with "admin todos" but the recorded MR is `commits_status` (no commits), so GitLab created no to-do there: the case is untested |
| T8 | Target branch deleted: MRs stay open, become `unchecked`, later `cannot_be_merged` (`branch_missing?`), `detailed_merge_status` `broken`/`branch_missing`... | IMPLEMENTED (status), not asserted | `mergeability::broken` | none |
| T9 | Target branch re-created: open MRs targeting it are marked unchecked and re-diffed only through the force/commit rules (create = not force) | PARTIAL | `refresh.rs:149` marks unchecked; labrat treats `created` with commits as `pushed` set (T4 divergence) | none |
| T10 | MR merged through the API/UI: the push to the target is processed later by a job (refresh of other MRs: stacked MR retarget, ff-abort) | IMPLEMENTED | `DEFER_REFRESH` `refresh.rs:16`, `merge.rs:388-398` | `stacked_mrs`, `auto_merge_abort` |
| T11 | Default-branch push: close issues (`Closes #N` in commit messages, `ProcessCommitWorker`), `DetectRepositoryLanguages`, ProjectCacheWorker | N/A (no issues) | | |
| T12 | `mentioned in commit` notes | see S14 | | |

## 3. Prioritised gaps (PARTIAL / MISSING) with reproduction steps

All steps use a fresh group + project (`initialize_with_readme: true`), admin token `A`, developer `bob` token `B`,
`MR1` = `feature` -> `main` created by `POST /projects/:id/merge_requests`, then `poll_mr`. "git" = clone, commit, push as in
`push_followups.rs` / `mr_diff_git.rs`. Compare via `GET /projects/:id/merge_requests/1/notes?sort=asc`, `GET .../merge_requests/1`,
`GET /todos`, webhook deliveries.

1. **Draft-from-commit note and eligibility (S11).** Highest value: real GitLab behaviour is deterministic and cheap to record.
   a. `git commit -m "Draft: wip"` on `feature`, `git push origin feature`. Expect title `Draft: <title>`, a system note
      `marked this merge request as **draft** from <40-char sha>` (check length), `updated_at` changed, MR `update` hook shows the new title.
   b. Variants: `fixup! First change`; `squash! x`; `wip fixup!<TAB>x`; push two commits (`Draft: a`, then `Draft: b`) -> sha of the first;
      MR already `Draft:` -> no note.
   c. Eligibility: on `main` (via `POST /repository/commits`) add a commit titled `fixup! other`, then `git merge main` into `feature`
      and push. Expect NO draft change in GitLab (commit not in the MR's commit set; the merge commit itself is the only MR commit).
2. **Auto-merge abort on source push (S6).** Needs a head pipeline: `POST /projects/:id/statuses/:sha` `state=running`, then
   `PUT .../merge_requests/1/merge` `{"auto_merge": true}`; push a new commit to `feature` (commits API or git). Compare the full note body
   (`aborted the automatic merge because the source branch was updated. [Learn more](...help/ci/pipelines/merge_trains#merge-request-dropped-from-the-merge-train).`),
   `merge_when_pipeline_succeeds`/`auto_merge_enabled` false, `merge_user` null, `merge_params`, the `added 1 commit` note ordering (abort note first).
3. **Pusher's to-dos resolved by push (S7).** `bob` is reviewer/assignee -> pending `review_requested`/`assigned` to-do; admin gets a
   `mentioned` to-do (`POST .../notes {"body":"@admin look"}` as bob) and a `marked` to-do on MR1 (`POST /merge_requests/1/todo` as admin).
   Admin pushes to `feature` -> admin's to-dos on MR1 are `done`, bob's stay `pending`; `GET /todos?state=done`. Snooze variant: snooze
   admin's to-do first (GraphQL `todoSnooze`) and check `snoozed_until` after the push (GitLab nils it); also `GET /user_counts`
   `todos` count.
4. **Merged by push: event source (T4).** Create `dev` branch from `main`. MR1 `feature`->`main`, MR2 `feature`->`dev`
   (same source branch, different target). Merge MR1 via `PUT /merge` (no source). Then `git push origin main:dev` (fast-forward `dev` to
   include the merge commit). Expect MR2 `merged`, `merge_commit_sha` = MR1's merge commit; compare `GET .../merge_requests/2/resource_state_events`
   (`source_merge_request_id` = MR1 id, `source_commit` null) and MR2 notes. Second case: manual merge `git merge --no-ff`,
   push -> `source_commit` = merge commit (already covered).
5. **Commit titles in the `added` note (S10b).** Push a commit with a 120-character first line and an empty-message commit
   (`git commit --allow-empty --allow-empty-message -m ""`): GitLab shows an 81-char truncated title with `...` and `No commit message`.
   Also push 12 commits at once (">10": `<li>OLD8...SHA8 - 2 earlier commits</li>`) and 11 commits (single earlier commit uses the short sha).
6. **Unmergeable to-do on conflict (T7).** `feature` commits `README.md` = `feature\n` (so the MR has commits), MR1 created and
   `can_be_merged`; commit `README.md` = `main\n` on `main` via commits API; `GET .../merge_requests/1?with_merge_status_recheck=true`, settle,
   `GET /todos?action=unmergeable` for the author (and `merge_user` if auto-merge was on - but see T5: it is cleared only on ff projects).
   Check `detailed_merge_status` is `conflict`, not `commits_status`, in the recording first.
7. **Positions on target-push reloads (T1b) and suggestion outdating (S5).** Create a diff discussion + a suggestion on MR1 line 1
   (`POST .../discussions` with `position`, body ```` ```suggestion:-0+0 ````); then push to `main` a commit that is also part of the MR's
   diff (`git rebase main` + `git push -f origin feature` covers source; for target: make MR1 target `release`, merge `feature` commit
   into `release` by `git push origin <featuresha>:release` ff, i.e. the MR's commits appear in the target). Compare
   `GET .../discussions` (`position`, `change_position`, system note "changed this line in version 2") and `GET /suggestions`
   `outdated`. Also a push that keeps the thread's line unchanged but edits the suggestion lines elsewhere in the hunk.
8. **Branch lifecycle via git (S8, S9).** `git push origin :feature` (delete) -> MR1 closed, `closed` hook, `close` event, notes
   (state event "closed"), `GET /todos` of pusher resolved, `DELETE` auto-merge note; then `git push origin feature` (re-create with new
   commits) -> MR1 stays `closed`, no `restored source branch` note, no new version; then `PUT state_event=reopen` and compare versions /
   notes (reopen creates a new diff). Currently only covered through the REST branch-delete endpoint.
9. **Cross-reference cap / filter (S14).** Push 101 commits where the first references `!1` in its message: GitLab processes only the
    last 100 (no mention note). Low priority (heavy).
10. **Version limit and non-latest diff cleanup (S1b, T4b).** 1000 pushes / merge then wait 5+ minutes: `GET .../versions/:id` `diffs` empty
    for old versions. Impractical for the harness; document only.
11. **Target branch re-created / deleted (T8, T9).** `DELETE /repository/branches/release` with MR targeting `release`: MR stays open,
    `detailed_merge_status` / `merge_status` after recheck; re-create with different content and compare versions. Low priority.
