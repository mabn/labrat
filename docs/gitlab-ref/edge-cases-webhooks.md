# Webhook edge cases: GitLab CE 18.11 vs labrat

Method: static read of `/home/mabn/dev/gitlab-run-ce/gitlab-src` against labrat. No requests were made. Paths: GL = gitlab-src, GB = `/home/mabn/dev/shed/labrat/crates`. Already conformance-verified against a real instance (`tests/conformance/fixtures/webhooks_flow.json`) and NOT re-reported: open/update/no-op update/draft/ready, push with open MR (`oldrev`), approved/unapproved, close(+update), reopen(+update), merge, note/confidential/edit, emoji award/revoke on MR and note, test endpoint for push/tag/MR/note/emoji, 429 rate limit, header set. Auto-disabling is behind `auto_disabling_web_hooks` (`default_enabled: false`, `config/feature_flags/ops/auto_disabling_web_hooks.yml`), so labrat's constant `alert_status: executable` is correct. The approval flow (`approved` on approve, `unapproved` on unapprove) matches `app/services/merge_requests/execute_approval_hooks_service.rb` and `remove_approval_service.rb`.

Confidence: H = code read end to end, M = inferred from Rails semantics, L = plausible, unverified.

## Summary table (most impactful first)

| # | Area | Conf | One-line difference |
|---|------|------|---------------------|
| 1 | Time tracking | H | No MR `update` hook for `time_estimate`, `reset_time_estimate`, `add_spent_time`, `reset_spent_time`, `/estimate`, `/spend`, GraphQL `timeEstimate` |
| 2 | Reviewers | H | `re_requested` is never `true`; re-request hook has empty `changes` |
| 3 | Reviewers | H | `changes.reviewers` lists force `state: "unreviewed"` for everyone |
| 4 | Emoji | H | No emoji hook from GraphQL `awardEmoji*` or `/react` quick action |
| 5 | System hooks | H | `repository_update` system hook never sent (default-on for system hooks) |
| 6 | Push hooks | H | `push_event_hooks_limit` (3) not implemented |
| 7 | Branch filter | H | `regex` strategy uses a toy matcher; no regex validation on create/update |
| 8 | Note hooks | M | MR `DiffNote` with no `change_position` serialises `null`, GitLab sends an empty Position object |
| 9 | Delivery | H | No webhook recursion detection (`X-Gitlab-Event-UUID` echo) |
| 10 | Hook test | M | `note_events`/`emoji_events` test fails if the newest project note is not an MR note |
| 11 | Threads | M | Publishing a review/draft never fires "resolved all threads" `update` unless a resolve happened |
| 12 | Forks | L | Source-branch push only refreshes MRs whose target is the pushed project |
| 13 | Delivery | M | No URL re-validation at delivery time |

---

## 1. Time tracking fires no MR hook (H)

GitLab: every call ends in `MergeRequests::UpdateService`, which runs `execute_hooks(mr, 'update', ...)`.
- `POST .../time_estimate` and `.../reset_time_estimate` -> `update_issuable(time_estimate: N)` -> general update (`lib/api/time_tracking_endpoints.rb:72-107`, `app/services/issuable_base_service.rb:362`). `changes`: `time_estimate`, `updated_at`, `updated_by_id` (safe-listed order).
- `POST .../add_spent_time` -> `use_specialized_service` -> `AddSpentTimeService` (`:119-137`, `app/services/merge_requests/add_spent_time_service.rb:20`): `execute_hooks(mr,'update', old_associations: {total_time_spent: old})` -> `changes: {total_time_spent: {previous, current}, time_change: {previous: 0, current: <delta secs>}}`; `object_attributes.time_change` = delta, `human_time_change` = e.g. `"1h"` (`app/models/concerns/time_trackable.rb:80-86`, `issuable.rb:561-564`).
- `POST .../reset_spent_time` -> general update (`:147`), `changes.total_time_spent`/`time_change`.
- Note quick actions `/estimate`, `/spend`, `/remove_estimate`, `/remove_time_spent` -> UpdateService -> hook. GraphQL `mergeRequestUpdate(timeEstimate:)` likewise.

labrat: `routes/merge_requests.rs:1468-1550` `time_op` writes the column and a system note, never calls `hooks::merge_request_event`. Quick actions `services/quick_actions.rs:553-570` (`"estimate"|"spend"|"reset_spent"`) write columns with no note and no hook. GraphQL `graphql/resolvers.rs:~1873` calls `set_time_estimate` (no hook) and only then `mrs::update` (hook, but with `changes` lacking `time_estimate` because it was already written before the snapshot). `hooks.rs:382,397` hard-code `human_time_change: null`, `time_change: 0`.

Repro: hook with `merge_requests_events`; `POST /projects/1/merge_requests/1/add_spent_time duration=1h`. GitLab delivers one `update` with `changes.total_time_spent {previous:0,current:3600}`, `changes.time_change {previous:0,current:3600}`, `object_attributes.time_change:3600`. labrat delivers nothing. Same for `POST .../time_estimate duration=2h` (expect `changes.time_estimate` + `updated_at`/`updated_by_id`).

## 2. `re_requested` never true; re-request `changes` empty (H)

GitLab `RequestReviewService#execute` (`app/services/merge_requests/request_review_service.rb:9-28`): captures `reviewers_hook_attrs` before, resets reviewer to `unreviewed`, then `execute_hooks(mr,'update', old_associations: {reviewers_hook_attrs: old, re_requested_reviewer_id: reviewer.user_id})`. Result (`issuable.rb:570-590`, `lib/gitlab/data_builder/issuable.rb:28-34,77-87`): `changes = {reviewers: {previous: [<users with real states, re_requested false>], current: [<users, the re-requested one state "unreviewed", re_requested true>]}}` (only `reviewers`; the MR row is untouched so no `updated_at`), and top-level `reviewers[].re_requested == true` for that user.

labrat: `services/reviews.rs:240-290` `rerequest` calls `merge_request_event(..., "update", None)`; `hooks.rs:531` always `reviewer_attrs(c, mr, None)` (param `rerequested` is never passed anywhere), `changes` is `{}`. Observable: `reviewers[].re_requested` is always `false`, `changes` empty.

Repro: MR with reviewer bob who has approved (or reviewed). Alice: GraphQL `mergeRequestReviewerRereview(userId: bob)` or note `/request_review @bob`. GitLab hook: `changes.reviewers.previous[0].state == "approved"`, `current[0] == {state:"unreviewed", re_requested:true}`; labrat: `changes:{}`, `re_requested:false`. A review bot keyed on `re_requested` never wakes.

## 3. `changes.reviewers` states are fabricated (H)

GitLab: `old_associations[:reviewers_hook_attrs]` (taken before the update, `issuable_base_service.rb:557-560`) and `reviewers_hook_attrs` after (`issuable.rb:580-588`): real `state` per reviewer (`unreviewed|reviewed|requested_changes|approved|unapproved|review_started`). A reviewer who already approved and is (re)added gets state `approved` via `set_reviewers_approved` (`base_service.rb:~115`), which the payload shows.

labrat: `hooks.rs:471-490` closure `rs` forces `state:"unreviewed"`, `re_requested:false` on both lists (the top-level `reviewers` uses real states, so the payload is internally inconsistent).

Repro (only one reviewer is allowed): bob is the reviewer and has approved; `PUT /projects/1/merge_requests/1 reviewer_ids=<carol>`. GitLab `changes.reviewers.previous = [bob, state approved]`, labrat `previous = [bob, state unreviewed]`.

## 4. Emoji hooks missing on GraphQL and `/react` (H)

GitLab: all awards go through `AwardEmojis::AddService/DestroyService` -> `execute_hooks` (`app/services/award_emojis/base_service.rb:14-19`, `add_service.rb:~33`, `destroy_service.rb`, `toggle_service.rb`). That covers REST, GraphQL `awardEmojiAdd|Remove|Toggle`, and the quick action `/react :tada:` (`IssuableBaseService#toggle_award`, `issuable_base_service.rb:536`).

labrat: hook fired only in `routes/notes.rs:720-735` (add) and `:758-768` (remove). Missing in `graphql/resolvers.rs:2215-2262`, `services/quick_actions.rs:551` (`emoji::toggle`), `ui.rs:1154`.

Repro: hook with `emoji_events`; GraphQL `awardEmojiToggle(input:{awardableId:"gid://gitlab/MergeRequest/1", name:"thumbsup"})`, or note body `/react :thumbsup:`: GitLab sends `Emoji Hook` (`event_type: award`, second call `revoke`), labrat sends nothing.

## 5. `repository_update` system hook not sent (H)

GitLab: `Repositories::PostReceiveWorker#after_project_changes_hooks` (`app/workers/repositories/post_receive_worker.rb:184-187`) sends `Gitlab::DataBuilder::Repository.update` (`event_name: "repository_update"`, `user_*`, `project`, `changes:[{before,after,ref}]`, `refs`) to every `SystemHook` with `repository_update_events` (default `true`, `app/models/hooks/system_hook.rb:23`), header `X-Gitlab-Event: System Hook`, trigger `system_hooks`.

labrat: `SYSTEM_EVENTS` (`routes/hooks.rs:32`) and `Hook::event` default `repository_update_events=true` (`hooks.rs:94`) exist, but `fire()` (`hooks.rs:163`) only forwards `push_hooks|tag_push_hooks|merge_request_hooks` to system hooks; nothing builds `repository_update`.

Repro: `POST /hooks url=...` (defaults: `push_events` false, `repository_update_events` true), then `git push` any branch. GitLab delivers one `System Hook` with `event_name: repository_update`; labrat delivers nothing.

## 6. `push_event_hooks_limit` (H)

GitLab: `Git::ProcessRefChangesService#execute_project_hooks?` (`app/services/git/process_ref_changes_service.rb:38-40`): per action group (created / removed / pushed) and per ref type, if `changes.size > push_event_hooks_limit` (default 3, `db/structure.sql:14523`) no project push/tag-push hooks and no system push hooks for those refs. MR refresh still runs.

labrat: `services/push.rs:36-49` loops every ref and `hooks::on_push` fires each. Repro: `git push origin b1 b2 b3 b4` (4 new branches): GitLab 0 push hooks, labrat 4.

## 7. Push `regex` branch filter (H)

GitLab: `ActiveHookFilter#matches_branch?` (`app/models/hooks/active_hook_filter.rb:15-33`): `regex` -> `Gitlab::UntrustedRegexp (RE2) === branch`; invalid regex -> no match; unknown strategy -> no match. Create/update validation: `validates :push_events_branch_filter, untrusted_regexp: true, if: regex?` (`app/models/concerns/web_hooks/hook.rb:78`) -> API `422 {"error":"Invalid branch filter given"}` (`lib/api/helpers/web_hooks_helpers.rb:80`).

labrat: `hooks.rs:129-162` `regex_lite_match` supports only literal, `.`, `x*`, leading `^`, trailing `$`. Alternation `(a|b)`, classes `[a-z]`, `+`, `?`, `\d`, `{n}`, groups are matched literally. `routes/hooks.rs:190-195` validates only the wildcard strategy.

Repro: `POST /projects/1/hooks url=... branch_filter_strategy=regex push_events_branch_filter='^(main|release/.+)$'`; push `main`. GitLab delivers; labrat does not. `push_events_branch_filter='('` returns 422 on GitLab, 201 on labrat. Wildcard (`RefMatcher`) and `all_branches` are equivalent; tag push and MR hooks are unfiltered in both.

## 8. MR DiffNote `change_position` (M)

GitLab: `serialize :change_position, type: Gitlab::Diff::Position` in `DiffPositionableNote` (`app/models/concerns/diff_positionable_note.rb:11-13`, included by `DiffNote`/`DraftNote`). AR's class-typed YAML column returns `Position.new` for NULL, so `Note#attributes` (hook slice, `lib/gitlab/hook_data/note_builder.rb`) emits `{"base_sha":null,"start_sha":null,"head_sha":null,"old_path":null,"new_path":null,"position_type":"text","old_line":null,"new_line":null,"line_range":null}` for a fresh MR DiffNote. labrat already does this for commit notes (`hooks.rs:900-905`, recorded in `commit_note_effects.json`) but the MR path `note_attrs` (`hooks.rs:577`) uses `pos(&n.change_position)` = `null`. Affects note hooks and the `note` key of emoji hooks on diff notes. Repro: `POST .../merge_requests/1/discussions` with `position[...]`, observe `object_attributes.change_position`.

## 9. No recursion detection (H)

GitLab: `Gitlab::WebHooks::RecursionDetection` (`lib/gitlab/web_hooks/recursion_detection.rb`, `.../uuid.rb`): an API request carrying `X-Gitlab-Event-UUID: <uuid>` sets the request UUID; hooks fired during it are registered in Redis set per UUID (TTL 30 min); `block?` is true if the hook id is already in the set or the set has >= 100 ids. Blocked deliveries are dropped (`WebHookService#async_execute`, `web_hook_service.rb:~112`). The same UUID is re-sent in the outgoing header.

labrat: header is sent (`api/src/hooks.rs:143`, fresh random uuid per delivery) but incoming header is never read (grep "recursion" empty). Repro: hook `H` with `merge_requests_events` and `note_events`; receiver replies to each delivery with `POST .../notes` and header `X-Gitlab-Event-UUID` copied from the delivery. GitLab: note hook for `H` is suppressed (H already registered); labrat: delivers, a bot that comments on every note loops forever.

## 10. Hook test payload selection (M)

GitLab `Integrations::ProjectTestData#note_events_data` / `emoji_events_data` (`app/services/concerns/integrations/project_test_data.rb:20-27,95-107`): newest note of the whole project (`project.notes`, any noteable: commit, issue, MR, system included). Payload builder switches on `note.for_commit?` etc.

labrat `routes/hooks.rs:~416-440`: newest note row, then `MergeRequest::find(note.noteable_id)`; if the newest note is a commit note it returns `422 "Ensure the project has notes."` instead of a commit-note payload. Repro: comment on a commit, then `POST .../hooks/1/test/note_events`.

## 11. "Resolved all threads" after publishing drafts (M)

GitLab `DraftNotes::PublishService#handle_notifications` (`app/services/draft_notes/publish_service.rb:116-122`) always runs `ResolvedDiscussionNotificationService` after a draft or review publish; if `discussions_resolved?` (resolvable threads exist and none open) it adds the system note and sends an MR `update` hook, even when the published drafts resolved nothing. labrat triggers `all_resolved_notification` only from the resolve paths (`comments.rs:457,487`). Repro: MR with one resolved thread; publish a plain review comment. GitLab: extra "resolved all threads" note + `update` hook; labrat none.

## 12. Fork MRs on source push (L)

GitLab `merge_requests_for(branch)` uses `@project.source_of_merge_requests` (MRs whose SOURCE project is the pushed one, hook goes to the target project's hooks). labrat `refresh.rs:90` selects `target_project_id = ?1`, so a push to a fork branch does not update/hook the upstream MR. Only relevant if forks are enabled.

## 13. Delivery-time URL checks (M)

GitLab re-validates the URL at request time (`Gitlab::HTTP` with `allow_local_requests: hook.allow_local_requests?`, `web_hook_service.rb:~17,150`), so after `allow_local_requests_from_web_hooks_and_services` is switched off a previously accepted local hook fails with a logged internal error. labrat checks only in `url_ok` at create/update. Low impact.

---

## Checked and equivalent (no action)

- Trigger matrix: open (AfterCreateService), update (every general `PUT`, even no-op), close/reopen/merge, push `update` with `oldrev`, assignee/label/milestone/target-branch/draft/lock via `PUT` (lock is not in `changes`), retarget chain `update`, "all resolved" `update`, `approved`/`unapproved`. No hooks on auto-merge set/abort, reviewer state changes (`requested_changes`, `reviewed`), `merge_after` alone, rebase itself.
- Extra top-level keys: `labels`, `changes`, `repository` (4 keys), `assignees` and `reviewers` only when non-empty; `object_attributes.action/actioned_at` (test delivery omits `actioned_at`), `oldrev` omitted for blank ref, `system:false`.
- Note hooks: never for system notes; internal -> `confidential_note_events` only; edit hook only when `note` changed and `user` is the note author; project hooks only (no integrations).
- Emoji payload keys `merge_request` / `note`, `awarded_on_url`.
- System hooks: only `push`, `tag_push`, `merge_request` scopes get project payloads; opt-in via `push_events` (default false), `tag_push_events`, `merge_requests_events` (default false); header `System Hook`, test endpoint `POST /hooks/:id` sample.
- Headers and ordering of `Idempotency-Key`, `X-Gitlab-Event`, `X-Gitlab-Webhook-UUID`, `X-Gitlab-Instance`, `X-Gitlab-Token`, `X-Gitlab-Event-UUID`, custom headers overridden by built-ins; test-endpoint rate limit 5/min (`web_hook_test_api_endpoint_rate_limit` default on); 422 body = response body on non-2xx.
- Auto-disable / `recent_failures` / `disabled_until`: flag off on self-managed, so `alert_status: executable` and `disabled_until: null` are right.
