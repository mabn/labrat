# REST params / edge cases labrat gets wrong or lacks (MR merge, update, notes, drafts, suggestions)

GitLab CE 18.11, source `/home/mabn/dev/gitlab-run-ce/gitlab-src` (paths relative to it). labrat paths relative to `/home/mabn/dev/shed/labrat`.
Method: source reading only, no HTTP. Confidence: H = straight code path, M = one assumption, L = minor or guess.
Not repeated: anything already listed in `coverage-gaps.md` (A/B/D), `edge-cases-mr-lifecycle.md`, `edge-cases-reviews.md`, `edge-cases-*.md`.
The GitLab-side behaviours below for items 1, 3 and 10 are described in `merge-requests.md` / `notes-discussions-reviews.md`, but labrat was never checked against them and does not implement them.
Checked and found already correct (no finding): assignee_id vs assignee_ids precedence and 0/[] handling, labels vs add/remove_labels, milestone_id 0, reviewer_ids [], squash persistence on a refused merge, sha 409 ordering, cancel_merge_when_pipeline_succeeds shape, rebase validation order, PUT draft_notes clearing `position`, bulk publish ordering, suggestion commit-message placeholders, notes `order_by=created_at` (GitLab really sorts by id, `lib/api/helpers.rb:560-569`), `created_at` owner-only.

## 1. Quick actions in an MR `description` are never run (REST create/update, also GraphQL mergeRequestCreate/Update)  [H]
- GitLab: `app/services/issuable_base_service.rb:184-205` (`handle_quick_actions` / `merge_quick_actions_into_params!`), called from `create` (:318) and `update` (:~318); MR overrides `app/services/merge_requests/update_service.rb:289-315` (`/rebase`, `/merge`) and `base_service.rb:168-173` (`/draft`, `/ready`). REST passes `use_specialized_service` but still goes through `update`; `QuickActionsNotAllowedError` is rescued at `lib/api/merge_requests.rb:757`.
- labrat: missing. `mr_params` (`crates/labrat-api/src/routes/merge_requests.rs:803`) and `services/merge_requests/mod.rs` `create` (:~560) / `update` (:763) never call `quick_actions::extract/execute` (only `comments.rs` does).
- Difference: `POST /projects/1/merge_requests {title, source_branch, target_branch, description:"Fix\n/label ~bug\n/assign @bob\n/draft"}`.
  GitLab: `description:"Fix"`, labels `[bug]`, assignee bob, title `Draft: ...`, plus the label/assignee system notes. labrat: description stored verbatim including the `/label` lines, no labels, no assignee, not draft.
  Same for `PUT .../merge_requests/1 {description:"x\n/target_branch other"}`. `/merge` in a REST description needs `merge_request_diff_head_sha`, which these endpoints do not declare, so GitLab only answers "requires the SHA" (no merge).
- Also: when the description changes, the `changed the description` note and `last_edited_*` use the stripped text. A description that is only commands becomes `""` (GitLab) and is still saved.

## 2. `POST .../discussions` honours `internal` / `confidential` and `merge_request_diff_head_sha`; GitLab ignores both  [H]
- GitLab: `lib/api/discussions.rb:101-117` declares only `body`, `created_at`, `position`; `opts` (:105-112) has no `internal`/`confidential`/`merge_request_diff_head_sha`. Reply route (:161-181) likewise (reply inherits the parent's confidentiality, `notes/build_service.rb:62-66`).
- labrat: `crates/labrat-api/src/routes/notes.rs:251-262` `create_params` is shared by `create_note`, `create_discussion` and `reply`, so `internal` and `merge_request_diff_head_sha` are read for discussions too (`create_discussion` :317, `reply` :346).
- Difference: `POST /discussions {body:"secret", internal:true}` as Reporter+. GitLab: 201 public thread (`internal:false`) visible to guests. labrat: internal thread hidden from guests (confidentiality leak in the opposite direction). `POST /discussions {body:"/merge", merge_request_diff_head_sha:<sha>}`: GitLab answers that the SHA is required; labrat merges.
- Fix: only read `internal`/`confidential`/`merge_request_diff_head_sha` in `create_note`.

## 3. Draft-note endpoints skip the `create_note` check  [M]
- GitLab: `lib/api/draft_notes.rb:46-48,141` (`authorize_create_note!` -> `access_denied!`, which is only defined on `ApplicationController`, so in Grape it raises NoMethodError and the response is 500 `{"message":"500 Internal Server Error"}`, per NDR 4.2). Publish: `draft_notes/publish_service.rb:8` returns `error('Not allowed to create notes')` first, even with zero drafts; API turns any non-success into bare `status 500` with an empty body (`draft_notes.rb:243-248,271-276`). `POST approve` with `publish_review=true`: `merge_request_approvals.rb:68-72` -> `render_api_error('Failed to publish review', 500)`.
- labrat: `create_draft` (`routes/notes.rs:569`, `services/reviews.rs:346`) has no `can_create_note` check. `bulk_publish` (`routes/notes.rs:~640`, `reviews::publish_drafts` :507) has none. `approve` with `publish_review` (`routes/notes.rs:901`) has none. Only single `publish_draft` checks (`reviews.rs:498`), and it returns a JSON `{"message":"Not allowed to create notes"}` body instead of an empty 500.
- Difference: MR with `discussion_locked=true`, caller is a logged-in non-member (or the project is archived). `POST draft_notes {note:"x"}` -> labrat 201, GitLab 500. `POST draft_notes/bulk_publish` -> labrat 204 (publishes), GitLab 500. A draft created earlier (before the lock) is still published by labrat.

## 4. `GET merge_ref`: commit id is not stable, and non-open MRs answer differently  [M]
- GitLab: `lib/api/merge_requests.rb:856-874` -> `MergeabilityCheckService#execute(recheck: true)` (`app/services/merge_requests/mergeability_check_service.rb:46-60,133-139`). The merge ref is rewritten only when `merge_status` is unchecked/checking or the ref's parents differ from `[target_sha, source_sha]` (`outdated_merge_ref?`, which is `false` for non-open MRs). Otherwise the existing commit id is returned. `merge_status` is not touched by close/merge (`merge_request.rb:211-238`), so a merged/closed MR that was `can_be_merged` still answers 200 `{"commit_id": <old merge ref commit>}` until the 14-day ref cleanup.
- labrat: `routes/merge_requests.rs:1188-1213` + `mergeability.rs` `test_merge`: always writes a new commit (`repo::signature` uses `time::now()/1000`, `repo.rs:17-23`), so two calls more than 1 s apart return different `commit_id` with no push in between. Non-open MR -> always 400 `Merge request is not mergeable` (`!mr.is_open()` at :1191).
- Difference: `GET merge_ref` twice (2 s apart) -> GitLab same id, labrat two ids. `GET merge_ref` on a merged MR -> GitLab 200 (while the ref lives), labrat 400. Tools that cache or compare merge-ref ids (CI "test the merge result") see churn.
- Fix: reuse `refs/merge-requests/<iid>/merge` when its parents equal `[target, source]` (and the MR is open or the ref exists).

## 5. Re-arming auto-merge adds a duplicate system note  [M]
- GitLab: `app/services/auto_merge/merge_when_checks_pass_service.rb:62-70`: the note `enabled an automatic merge when all merge checks for <sha> pass` (and the notification) is only added `if merge_request.saved_change_to_auto_merge_enabled?`. A second `PUT /merge {auto_merge:true}` while already armed only refreshes merge_params/merge_user (`auto_merge/base_service.rb:117-123`).
- labrat: `services/merge_requests/merge.rs:517-527` `enable_auto_merge` always inserts the note.
- Difference: two successive `PUT .../merge {auto_merge:true, sha:S}` -> GitLab 1 note, labrat 2 notes (also a second `updated_at` bump). Same when a different user re-arms (merge_user changes, still only the first note in GitLab).

## 6. `position` integer fields are not type-checked on discussions and drafts  [M]
- GitLab: Grape declares `new_line, old_line, width, height, x, y` as `Integer` and the same inside `line_range.start/end` (`lib/api/discussions.rb:112-150`, `lib/api/draft_notes.rb:58-92`). A non-integer value is a 400 `{"error":"position[new_line] is invalid"}` (`position[line_range][start][new_line]` for nested ones). `line_range` and its `start`/`end` must be Hashes.
- labrat: `position_param` (`routes/notes.rs:~285`) only checks presence of `base_sha/start_sha/head_sha/position_type` and that `position` is an object; `Position::from_json` (`services/positions.rs:41-60`) silently turns `"abc"` or `true` into `None`.
- Difference: `POST discussions {position:{..., position_type:"text", new_path:"a", new_line:"abc"}}` -> GitLab 400, labrat goes on and answers `position is incomplete` style errors (or a file-level note). `line_range:"x"` -> GitLab 400, labrat stores it.

## 7. `created_at` on notes/discussions: garbage value errors for everyone in labrat  [L-M]
- GitLab: `created_at` is a plain String (`notes.rb:73`), dropped for users without `set_note_created_at` (`notes_helpers.rb:157`) before any parsing; for owners an unparseable string is cast to nil by ActiveRecord and the note gets `now`.
- labrat: `create_params` -> `req.params.time("created_at")?` (`routes/notes.rs:251-262`, `params.rs:225`) fails first with 400 `created_at is invalid` for every user, and for owners too.
- Difference: Developer sends `created_at:"yesterday"`: GitLab 201 (ignored), labrat 400. Owner sends `created_at:"foo"`: GitLab 201 with now, labrat 400.

## 8. `remove_source_branch` needs `push_code` on the source project  [L-M]
- GitLab: `app/services/concerns/merge_requests/assigns_merge_params.rb:11-15` drops `force_remove_source_branch` unless `current_user.can?(:push_code, source_project)`; applies to create and update.
- labrat: `services/merge_requests/mod.rs:~870` (update) and create store the flag with no `push_code` check (only the closed/merged gate, which does match GitLab).
- Difference: a Reporter-level MR author (may update their MR) sends `PUT {remove_source_branch:true}`: GitLab 200 with `force_remove_source_branch:false`, labrat `true`. (Counts for `at_least_one_of`, so no 400.)

## 9. `If-Unmodified-Since` on MR, note and award-emoji DELETE  [M, low impact]
- GitLab: `destroy_conditionally!` -> `check_unmodified_since!` (`lib/api/helpers.rb:39-64`) used by `DELETE merge_requests/:iid` (`merge_requests.rb:413`), `DELETE notes/:id` and `discussions/:id/notes/:id` (`notes_helpers.rb:delete_note`), `DELETE award_emoji/:id` (`award_emoji.rb:105`). 412 `{"message":"412 Precondition Failed"}` when the resource `updated_at` is newer than the header.
- labrat: missing (no `Unmodified` anywhere under `crates/`); `delete_note` `routes/notes.rs:~220`, `destroy` `routes/merge_requests.rs:934`, `remove_award` `routes/notes.rs:~730`. (Already noted only for commit notes: `edge-cases-search-filters-commit-notes.md` C12.)
- Difference: `DELETE /notes/5` with `If-Unmodified-Since: <time before last edit>` -> GitLab 412 and the note stays; labrat 204 and deletes.

## 10. Suggestion batch "same branch" check compares MR identity, GitLab compares branch names  [L]
- GitLab: `lib/gitlab/suggestions/suggestion_set.rb:94-96` (`branch == suggestion.branch`), `branch` = the MR source branch name of the first suggestion.
- labrat: `services/suggestions.rs:~275` rejects `mr.id != items[0].mr.id` with `Suggestions must all be on the same branch.`
- Difference: two open MRs with the same source branch and different targets, each with a suggestion: `PUT /suggestions/batch_apply {ids:[a,b]}` -> GitLab applies both in one commit, labrat 400. Rare.

## Suggested order of work
1 (quick actions), 2 (`internal` leak), 3 (draft create_note gate), 4 (merge_ref), 6, 5, 7, 8, 9, 10.
