# GitLab 18.11 CE: `detailed_merge_status` and mergeability checks vs labrat

Source-reading only (no instance contacted, nothing run). GitLab paths are relative to
the GitLab CE 18.11 source copy; labrat paths relative to `/home/mabn/dev/shed/labrat`.
Fixture refs are `fixtures/<file>.json step <index> "<label>"` under `tests/conformance/`.
`[unverified]` = inferred from source, not confirmed by a recording.

## 1. How the value is computed

`MergeRequests::Mergeability::DetailedMergeStatusService#execute`
(`app/services/merge_requests/mergeability/detailed_merge_status_service.rb`):

1. `merge_status == preparing` -> `preparing`.
2. `cannot_be_merged_rechecking` or `checking` -> `checking`.
3. `unchecked` -> `unchecked`. (`cannot_be_merged_recheck` is NOT caught here and falls through to the checks.)
   These three come BEFORE `not_open`, so a closed MR whose `merge_status` is `unchecked` reports `unchecked`.
4. Run `MergeRequest.all_mergeability_checks` with `skip_ci_check: true` `execute_all: false`: first check whose result is `failed` OR `checking` stops the run and its identifier is the
   value. (`RunChecksService#execute`, `CheckResult#unsuccessful?` = failed or checking.)
5. If all pass: run `CheckCiStatusService` separately; if it is `checking` or `failed` -> `ci_still_running` when
   `diff_head_pipeline_considered_in_progress?` else `ci_must_pass`; otherwise (`success`/`inactive`) -> `mergeable`.
   So **CI is always last**: any other failure (even `conflict`/`need_rebase`) beats `ci_must_pass`.

GraphQL `mergeabilityChecks` = `all_mergeability_checks_results` = same list, `execute_all: true`, no skip params, so it
includes `ci_must_pass` and reports every check.

### Check order (`MergeRequest.mergeable_state_checks_by_tier`; `app/models/merge_request.rb:1649-1675`)

trivial: `not_open`, `draft_status`
light: `merge_time`
heavy: `locked_lfs_files`, `discussions_not_resolved`, `ci_must_pass`, `commits_status`
git: `need_rebase`, `conflict`

labrat's `mergeabilityChecks` list follows this order (`crates/labrat-api/src/graphql/resolvers.rs` "mergeabilityChecks", ~L1629) and matches the
recording (`fixtures/mergeability_checks.json` steps 10/11/13).

## 2. Table

Status column = what `mergeabilityChecks` reports in the typical case. "Impl" = labrat.

| detailed value / identifier | Condition | GraphQL status typical | Impl in labrat | Fixture |
|---|---|---|---|---|
| `preparing` (not a check) | `merge_status = preparing` (MR just created, AfterCreate not yet done). `public_merge_status` shows `checking` | n/a (checks still evaluated: CONFLICT=CHECKING) | `mergeability.rs::detailed_status`, `public_status`; set in `merge_requests/mod.rs` create / `after_create` | `mergeability_checks.json` 4 "mr"; present in nearly all |
| `checking` (not a check) | `checking` or `cannot_be_merged_rechecking` | CONFLICT=CHECKING | `detailed_status` | `closed_mr_updates.json`, `merge_request_fast_forward.json` |
| `unchecked` (not a check) | `merge_status = unchecked` (e.g. viewer cannot `update_merge_request`, so no async recheck; or after push before recheck) | CONFLICT=CHECKING | `detailed_status` | `random_walk_hooks_2.json` 27 "06 admin mr2 op7 hooks", `random_walk_mixed_3.json` |
| `mergeable` | all pass | SUCCESS/INACTIVE mix | `detailed_status` | many, e.g. `mergeability_checks.json` 10 "clean" |
| `not_open` | `state != opened` (closed, merged, locked) | NOT_OPEN=FAILED | `first_failed_check` | `mergeability_checks.json` 13 "closed" |
| `draft_status` | title matches `\A(?i)(\[draft\]\|\(draft\)\|draft:)` | DRAFT_STATUS=FAILED | `first_failed_check`, `MergeRequest::is_draft_title` | `mergeability_checks.json` 11 "blocked" |
| `merge_time` | `merge_schedule.merge_after` in future (fails), nil -> INACTIVE, past -> SUCCESS | INACTIVE / FAILED / SUCCESS | `first_failed_check` + resolvers (`merge_after`) | `merge_schedule.json`, `audit_followups_2.json` 7-9; GraphQL `mergeability_checks.json` 11 "blocked" (FAILED) |
| `locked_lfs_files` (CE check) | `project.lfs_enabled?` false -> INACTIVE; else FAILED if another user holds an LFS lock on a changed path (`project.lfs_file_locks`), else SUCCESS. detailed value `locked_lfs_files` | SUCCESS (lfs on, no locks) | **hard-coded SUCCESS**; never INACTIVE, never fails; no LFS lock store found (`git_http.rs` only has the pointer pre-receive check) | SUCCESS in `mergeability_checks.json`, `graphql_complexity.json` 22 |
| `discussions_not_resolved` | `only_allow_merge_if_all_discussions_are_resolved` and any `resolvable && !resolved` note; setting off -> INACTIVE | INACTIVE / SUCCESS / FAILED | `first_failed_check`, `unresolved_discussions`, `blocking_discussions_resolved` | `random_walk_merge_2.json` 47-50 "21 mr1 op11"; GraphQL FAILED `mergeability_checks.json` 11 |
| `ci_must_pass` | CI check failed, not in progress. `CheckCiStatusService`: INACTIVE unless (`auto_merge_enabled?` or `only_allow_merge_if_pipeline_succeeds?`); pipeline_must_succeed = setting OR (auto-merge strategy merge_when_checks_pass AND `has_ci_enabled?`); fails if no `diff_head_pipeline`, `pipeline_creating?`, or pipeline not `success` (skipped OK only with `allow_merge_on_skipped_pipeline`); status `checking` if `pipeline_creating? \|\| diff_head_pipeline.active?` (statuses waiting_for_resource, preparing, waiting_for_callback, pending, running) | INACTIVE / SUCCESS / CHECKING / FAILED | `services/ci.rs::ci_check`, `in_progress`; resolvers.rs CI mapping | `commit_statuses.json` 26 "mr ci must pass" (failed), 32 "mr after push" (no pipeline on new head); GraphQL CHECKING `mergeability_checks.json` 11 |
| `ci_still_running` | all other checks pass AND CI check checking/failed AND `diff_head_pipeline_considered_in_progress?` = `pipeline_creating?` OR (setting on: `!pipeline.complete?` [i.e. also created/manual/scheduled/canceling]; setting off: `active? \|\| created?`) | CI_MUST_PASS=CHECKING (or FAILED, see gap G1) | `ci_check` + `ci::in_progress` | `commit_statuses.json` 34 "auto merge"; `auto_merge_abort.json` 5 "auto merge"; `graphql_accept_strategy.json` 19 "mr 1" |
| `commits_status` | `has_no_commits?` (diff not persisted/0 commits) OR source or target branch missing | COMMITS_STATUS=FAILED | `mergeability::broken` (excludes conflict part, correct) | `merge_request_lifecycle.json` 11, `audit_followups_3.json` 41 "mr conf after"; GraphQL none |
| `need_rebase` | `project.ff_merge_must_be_possible?` (`merge_requests_ff_only_enabled \|\| merge_requests_rebase_enabled`, i.e. merge_method `ff` or `rebase_merge`) and not `project_setting.automatic_rebase_available?` (flag `rebase_on_merge_automatic`, default off) and target head not ancestor of diff head sha. Else INACTIVE (merge method `merge`) | INACTIVE / SUCCESS / FAILED | `mergeability::should_be_rebased`, `ff_required` | `merge_request_fast_forward.json`, `auto_merge_abort.json` 7, `graphql_mr_all_scalars.json` 25 "open fields" (NEED_REBASE enum), `random_walk_merge.json` (42x) |
| `conflict` | `source_branch_sha != merge_request_diff.head_commit_sha` -> CHECKING; `can_be_merged` -> SUCCESS; `cannot_be_merged` -> FAILED; any other state (unchecked, checking, *_recheck(ing), preparing) -> CHECKING | SUCCESS / FAILED / CHECKING | `first_failed_check` + resolvers | `merge_request_close_reopen_conflict.json` 5 "get conflict", `push_side_effects.json` |

Identifiers that do NOT exist in 18.11: `broken_status`, `ci_status`, `blocked_status`; the legacy ones were replaced by
`commits_status`. Reachable as `detailed_merge_status`: `preparing checking unchecked mergeable
not_open draft_status merge_time locked_lfs_files discussions_not_resolved ci_must_pass ci_still_running commits_status need_rebase
conflict`.

### Fixture evidence of precedence (REST)

- draft > merge_time > discussions > CI(GraphQL only): `mergeability_checks.json` 11 "blocked" (draft + merge_after future + open thread + running status): `DRAFT_STATUS`, while checks list shows MERGE_TIME/DISCUSSIONS FAILED and CI CHECKING.
- discussions_not_resolved > conflict: `random_walk_merge_2.json` 47 (`has_conflicts: true`, `merge_status: cannot_be_merged`, detailed `discussions_not_resolved`).
- draft > discussions > conflict: `random_walk_merge_2.json` 46 "20 mr1 op8 state" (`draft_status` with has_conflicts true).
- need_rebase beats conflict/CI: `random_walk_merge.json` (cannot_be_merged x49, need_rebase x42).
- `ci_must_pass` only shown when merge_status `can_be_merged`: `commit_statuses.json` 26/32.

### Setting-off / non-blocking behaviour with commit statuses

`only_allow_merge_if_pipeline_succeeds=false` and no auto-merge: CI check INACTIVE, detailed `mergeable` regardless of
pending/running/failed pipeline (`commit_statuses.json` 13 "mr with pending pipeline", 18 "mr with running pipeline"). With the
setting on: success -> mergeable (30), failed -> `ci_must_pass` (26), pending/running -> `ci_still_running` (34 after
auto-merge request; auto merge response itself). Statuses become an `external` pipeline matched to the MR when the POST `ref`
equals the MR source branch and sha equals the diff head (`first_matching_pipeline`, see `docs/gitlab-ref/commit-statuses.md`).
Commit-status states accepted by the API: `pending running success failed canceled skipped` (no `manual`/`created`).
Composite pipeline status: any failed -> `failed`; else running/pending -> active; all canceled -> `canceled`; all skipped -> `skipped`;
`success` with `allow_failure` jobs is still `success`.

## 3. Gaps

Legend: NOT IMPLEMENTED, WRONG (labrat deviates from the 18.11 source), UNTESTED (implemented, no recording). All recipes use
admin or Maintainer token, project `P`, MR with feature branch `b` (commit via `POST /projects/:id/repository/commits`),
`sha` = the MR `diff_head_pipeline`-relevant head (`GET /merge_requests/:iid` `sha`), status via
`POST /projects/:id/statuses/:sha {"state":..,"name":"ci","ref":"b"}`, then `settle`, then
`GET /projects/:id/merge_requests/:iid` (detailed_merge_status) and the GraphQL query used in
`tests/scenarios/mergeability_checks.rs` (`detailedMergeStatus mergeabilityChecks { identifier status }`).

### G1. WRONG: GraphQL `CI_MUST_PASS` status when pipeline is "in progress" but not `active?`
`resolvers.rs` maps `ci_check == Some("ci_still_running")` -> CHECKING. GitLab's check returns `checking` only when
`pipeline_creating? || diff_head_pipeline.active?` (`check_ci_status_service.rb pipeline_pending?`; ACTIVE = waiting_for_resource,
preparing, waiting_for_callback, pending, running). `diff_head_pipeline_considered_in_progress?` is broader (setting on: anything
not complete, so `created`, `manual`, `scheduled`, `canceling`). Detailed value is `ci_still_running` there but the check is
**FAILED**. Also `Pipeline::is_active` (`services/ci.rs:66`) lacks `waiting_for_callback`.
Repro: `only_allow_merge_if_pipeline_succeeds=true`; create an external pipeline in status `created` or `manual` (not directly
creatable through the statuses API; only reachable if labrat can create manual/created pipelines, e.g. a pipeline with a manual job via
CI config; otherwise test at unit level). With statuses API only, `pending`/`running` give CHECKING (matches), so this
only bites for `created`/`manual`/`scheduled`. Severity low.

### G2. NOT IMPLEMENTED: `LOCKED_LFS_FILES` INACTIVE / FAILED and detailed `locked_lfs_files`
Hard-coded SUCCESS. GitLab: INACTIVE when `!project.lfs_enabled?`; FAILED when another user holds an LFS lock on a changed path
(`check_lfs_file_locks_service.rb`); detailed value `locked_lfs_files` at position after `merge_time` and before
`discussions_not_resolved`.
Repro (cheap half): `PUT /projects/:id {"lfs_enabled": false}`; query GraphQL -> expect `LOCKED_LFS_FILES: INACTIVE`
(labrat returns SUCCESS). Full half needs the git-lfs lock API
(`POST /<ns>/<proj>.git/info/lfs/locks` with `{"path":"f.bin"}` as user A, then MR by user B changing `f.bin` on a project with
lfs_enabled) -> detailed `locked_lfs_files`, merge attempt 405. Check whether `lfs_enabled` project param exists in labrat
(`entities/project.rs` mentions lfs).

### G3. UNTESTED: the full precedence matrix with CI failing
Only draft/discussions/conflict combos exist. No recording where CI fails together with: (a) `conflict`, (b) `need_rebase`,
(c) `discussions_not_resolved`, (d) `commits_status`, (e) merge_time. Expected per source: the non-CI one wins in every case.
Repro: project `only_allow_merge_if_pipeline_succeeds=true`, `only_allow_merge_if_all_discussions_are_resolved=true`,
`merge_method` `ff`. MR 1 (branch b1) with `Draft:` title + conflicting file edited on `main` + `POST /discussions` thread + failed
status on b1 head: expect `draft_status`. Then remove draft (PUT title) -> expect `merge_time` if `merge_after` future, remove it ->
`discussions_not_resolved`, resolve (`PUT .../discussions/:id {"resolved":true}`) -> `commits_status` only if branch deleted/empty,
else `need_rebase` (ff) -> switch merge_method `merge` -> `conflict` -> resolve conflict -> `ci_must_pass`, then `POST
statuses` success -> `mergeable`. Assert at each step both REST and GraphQL checks list.

### G4. UNTESTED: commit-status variants vs `ci_must_pass` / `ci_still_running` / `mergeable`
Recorded: pending, running, failed, success only (statuses.rs / `commit_statuses.json`; `random_walk_merge` also uses canceled).
Missing deterministic scenarios (setting on):
- `canceled` status -> pipeline `canceled` -> `ci_must_pass` (complete). GraphQL CI_MUST_PASS=FAILED.
- `skipped` status -> pipeline `skipped`: `allow_merge_on_skipped_pipeline=false` -> `ci_must_pass`; `true` -> `mergeable`
  (`PUT /projects/:id {"allow_merge_on_skipped_pipeline":true}`).
- two statuses (names `a`,`b`): `a` failed + `b` running -> pipeline status `failed`? (GitLab composite: running wins over failed
  in `Gitlab::Ci::Status::Composite` only if no failed... verify: failed has priority in ORDERED_STATUSES, so `failed` ->
  `ci_must_pass`, not `ci_still_running`). Check labrat composite matches.
- status posted for sha with a different `ref` (default branch / other branch) -> not the MR's head pipeline -> with setting on
  `ci_must_pass` (no head pipeline), `has_ci` semantics for `merge_when_pipeline_succeeds` case.
- `only_allow_merge_if_pipeline_succeeds=false` + auto merge requested (`PUT .../merge {"auto_merge":true}` / `merge_when_pipeline_succeeds`)
  with pending status: CI check not INACTIVE (auto_merge_enabled), `ci_still_running`; with NO pipeline at all and setting off:
  `pipeline_must_succeed?` false when `has_ci_enabled?` false -> success -> `mergeable`.
- `allow_failure` is not settable via statuses API (`allow_failure` param absent) -> n/a.

### G5. UNTESTED: `ci_still_running` precedence over `ci_must_pass` when setting is OFF but auto-merge on
`in_progress` for setting off = `active? || created?`. Covered nowhere. Repro above (auto merge, setting off, running status).

### G6. UNTESTED / possibly WRONG: REST `?with_merge_status_recheck` on lists
Only single-MR GETs pass the param (`push_side_effects.json` "recheck", `audit_followups_3.json` 100 "mr conf after" - a single
GET ignores the param since single GET always rechecks). No recording on lists.
GitLab behaviour (`lib/api/merge_requests.rb:162,249,338`, `entities/merge_request_basic.rb:66`):
- project list `GET /projects/:id/merge_requests?with_merge_status_recheck=true`: `recheck_mergeability_of` (only if user can
  `update_merge_request`) calls `check_mergeability(async: true)` -> response already shows `merge_status: "checking"` for
  `unchecked` MRs (and `cannot_be_merged_recheck` -> shows `checking` via `public_status`). labrat `render_list` matches this.
- global list `GET /merge_requests?...`: entity does the per-MR async check (skip option false) - matches.
- **group list** `GET /groups/:id/merge_requests?with_merge_status_recheck=true`: uses `MergeabilityCheckBatchService`
  (a Sidekiq batch worker) and forces `skip_merge_status_recheck = true`, so the response shows the STORED status (`unchecked`,
  not `checking`) and the worker flips it afterwards. labrat `list_group` goes through the same `render_list`, i.e. returns
  `checking` immediately: WRONG for group lists (inferred; `[unverified]` for exact timing).
- Without the param, lists never recheck: `unchecked` MRs stay `unchecked`/`detailed unchecked`.
- Users without `update_merge_request` (Reporter/Guest) never trigger the recheck.
Repro: project with MR whose merge_status is `unchecked` (create MR as admin, wait for preparing -> unchecked->checked; to get
`unchecked` push a new commit to source branch, then immediately list). Steps: push; `GET /projects/:id/merge_requests` (expect
`unchecked`); `GET .../merge_requests?with_merge_status_recheck=true` (expect `checking`); same for
`/groups/:gid/merge_requests` (expect `unchecked`); same as Reporter (expect `unchecked`). Also `GET /merge_requests?scope=all`.

### G7. UNTESTED: `unchecked`/`checking` vs `not_open`, and `cannot_be_merged_recheck`
Closed/merged MR with `merge_status=unchecked` (e.g. after retarget or after `PUT target_branch`) reports `unchecked` not
`not_open`, but a closed MR with stale `can_be_merged` reports `not_open`. labrat implements it; only partly recorded
(`closed_mr_updates.json`). Repro: open MR, close it (`state_event=close`), push to target branch (marks `unchecked`/
`cannot_be_merged_recheck`), GET as Reporter (no recheck) -> expect `unchecked`; as admin (rechecks) -> `checking`, then `not_open`.
`cannot_be_merged_recheck` (conflicting MR after target push): detailed falls through to checks -> `conflict` (or earlier
failing check), `merge_status` shows `cannot_be_merged_recheck`; `has_conflicts` false (state predicate only) while labrat
`has_conflicts = merge_status == "cannot_be_merged"` ✓ - but confirm no recording of `has_conflicts` in that state.

### G8. UNTESTED: GraphQL `mergeabilityChecks` states beyond the happy path
Recorded: clean, draft+merge_time+discussions+CI-running, closed. Missing recordings of: `NEED_REBASE` SUCCESS/FAILED
(merge_method `ff`; INACTIVE for `merge`), CONFLICT FAILED/CHECKING (`conflict` after a conflicting push; `unchecked` MR
-> CHECKING), COMMITS_STATUS FAILED (delete source branch via `DELETE /projects/:id/repository/branches/b`, or MR with 0 commits),
CI_MUST_PASS FAILED (failed status), CI SUCCESS with setting on, DISCUSSIONS FAILED/SUCCESS after resolving, MERGE_TIME SUCCESS (merge_after in the past:
needs an MR created with past `merge_after`? API validation; or wait), and draft + NOT_OPEN interplay on merged MR.

### G9. NOT IMPLEMENTED (probably unneeded): `pipeline_creating?` and CI-config-driven pipelines
GitLab reports `ci_still_running` / CI_MUST_PASS=CHECKING while `Ci::PipelineCreation::Requests` has the MR creation in flight
(just after MR create/push when the project has a `.gitlab-ci.yml`). labrat has no pipeline creation requests (`pipeline_creating`
absent from `crates/labrat-core`). Only matters if conformance projects get a `.gitlab-ci.yml` and `create_pipeline`.

### G10. NOT IMPLEMENTED (flag off by default): `automatic_rebase_available?`
`need_rebase` is INACTIVE in ff mode when `project_setting.automatic_rebase_enabled` and flag `rebase_on_merge_automatic`
(default false, `config/feature_flags/gitlab_com_derisk/`). Not applicable unless the flag is enabled on the test instance.

### G11. Minor: `mergeable` GraphQL / `mergeable?` paths use the whole check list incl. CI
`mergeability::mergeable` -> `first_failed_check(.., skip_ci=false)` returns `ci_must_pass` for any CI failure (including
in-progress), which GitLab also treats as unmergeable. Fine; but `mergeable?` in GitLab skips `skip_*` only for auto-merge; no gap
other than LFS (G2), which also silently makes labrat's merge endpoint succeed where GitLab returns 405 for locked LFS files.
