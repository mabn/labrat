# Quick actions audit (MergeRequest): GitLab CE 18.11 source vs labrat

Source-only audit (no requests, no recordings re-run). GitLab tree: the CE 18.11 source copy. labrat:
`crates/labrat-core/src/services/quick_actions.rs` (`QA`). Paths below are relative to the GitLab tree unless noted.
Confidence is "source-derived" everywhere; items marked (verify) hinge on a runtime detail (ChronicDuration, DB ordering).

## 0. Mechanics that apply to every command

| Topic | GitLab | labrat | Diff |
|---|---|---|---|
| Names the extractor recognises | `Extractor#command_names` (extractor.rb:209-219) = names+aliases of **every** non-noop definition in `InterpretService.command_definitions`, regardless of `types`. So `/due`, `/relate`, `/epic`, `/tag`, `/run_pipeline`, ... are extracted and removed from MR notes | `COMMANDS` (QA:19-65) = MR names only; `extract` (QA:68) leaves any other `/word` in the note | **Differs** (#1, #2) |
| Unavailable / wrong-type command | `CommandDefinition#execute_message` (command_definition.rb:79-82): `valid_type?` or `condition` false -> `Could not apply <definition.name> command.`; `count_commands_executed_in` ran first (l.70) so the note is "a command note" | fallthrough arm QA:611 for names in `COMMANDS` only; `canonical_command` (QA:764) | Differs: alias mapping wrong for reviewer aliases (#10), `epic` -> `set_parent` missing |
| Arg-less call of a command whose block takes a required arg | `execute_block` (command_definition.rb:147-154): blank arg and `block.arity != 0` -> block NOT run, `execution_message` NOT run (returns nil, `compact`ed). Optional-arg blocks (`\|x = nil\|`) are procs, arity 0, so they run | arms guarded on args (`title`, QA:235) fall through to "Could not apply" | Differs (#7) |
| Parse failure | `failed_parse` -> `ParseError`: swallowed in `execute` (l.75), turned into `Could not apply <name> command. <message>.` in `execute_message` (l.88-89) | none; silently no message | Differs (#3) |
| Message assembly | `InterpretService#map_commands` (interpret_service.rb:192-213): per command, `@execution_message[definition.name]` (once per definition name) else `definition.execute_message`; joined with a single space; an unset message is `''` (not nil), so it leaves a double space in the join | `messages.join(" ")` (comments.rs:210) | labrat never emits `''` parts |
| Empty message + commands only | `Notes::CreateService#do_commands` (create_service.rb:135): `Failed to apply commands.` error -> REST 400 | comments.rs:204-206 same | matches |
| `commands_changes` | `Note#commands_changes` whitelist (note.rb:679-711); only set when `@updates.present?` (create_service.rb:139-145) | `ORDER` QA:717-748 | matches (order and whitelist) |
| Executes in order, state live | `extract_updates` (interpret_service.rb:224-232): each block runs immediately in order; services called inside blocks (approve, unapprove, submit_review, request_changes, request_review re-request) change the DB before the next command's condition is evaluated. `@updates` are applied after the note | `deferred` list run after the loop (QA:651); conditions read the pre-note DB | Differs for same-note sequences (#16) |
| Description (new record) | `quick_action_target.persisted?` is false while the MR is being created | `ctx.new_record` skip list (QA:203-211) | labrat skip list incomplete (#20) |
| Single assignee/reviewer | `allows_multiple_assignees?`/`allows_multiple_reviewers?` false (project.rb:3726-3733) | modelled as single assignee/reviewer | matches |

## 1. Per-command table

`GL` = file:line of the `command` call. "matches" = same condition, params, `@updates`, message in all cases I could derive.

| Command (aliases) | GL | labrat | Result |
|---|---|---|---|
| `/close` | issuable_actions.rb:28 (cond `open? && can update_merge_request`; `state_event: 'close'`; "Closed this merge request.") | QA:221 | matches |
| `/reopen` | issuable_actions.rb:50 (cond `persisted? && closed? && can update_merge_request`; "Reopened this merge request.") | QA:226 | labrat adds `discussion_locked != true` to the condition; GL does not (policy `reopen_merge_request` is only checked later by `ReopenService`) (#17) |
| `/title` | issuable_actions.rb:67 (cond `persisted? && can update`; arg verbatim; "Changed the title to \"%s\".") | QA:235 | no-arg case differs (#7); trailing whitespace of the arg is kept by GL, labrat trims; new-record not skipped (#20) |
| `/label` `/labels` | issuable_actions.rb:86, `run_label_command` :283 (cond: `set_merge_request_metadata && find_labels.any?`, i.e. the project has any label; `add_label_ids`; message "Added %s label(s)." sorted, refs `~"name"`, or `~id` when the name has `"`) | QA:242 | tildeless names and not-found message differ (#4, #5) |
| `/unlabel` `/remove_label` | issuable_actions.rb:107 (cond `persisted? && labels.any? && metadata perm`) | QA:270 | tildeless names wipe all labels in labrat (#4); not-found message/key differ (#5) |
| `/relabel` | issuable_actions.rb:144 (`label_ids`, message "Replaced all labels with %s label(s)."; accumulates over repeated commands) | QA:301 | no-arg/not-found wipes labels in labrat (#5); repeated `/relabel` not accumulated; tildeless names (#4) |
| `/todo` | issuable_actions.rb:156 | QA:314 | matches |
| `/done` | issuable_actions.rb:168 | QA:319 | matches |
| `/subscribe` | issuable_actions.rb:180 | QA:325 | matches (not skipped for new record, #20) |
| `/unsubscribe` | issuable_actions.rb:192 | QA:330 | matches |
| `/react` `/award` | issuable_actions.rb:212 (cond `persisted?`; parse = first `:name:` via `EmojiFilter.emoji_pattern`; `emoji_award` only if `user_can_award?`; message ignores validity and permission) | QA:335 | colon requirement, validity, multi-token differ (#14) |
| `/shrug` `/tableflip` | issuable_actions.rb:218-228, substitution_definition.rb:14-20 (block takes no arg: the whole `/shrug ...` text is replaced by the constant, argument text is dropped) | QA:91-94 | labrat keeps the argument text (#15) |
| `/assign` | issue_and_merge_request_actions.rb:40 (cond `supports_assignee? && metadata perm`; `assignee_ids = [users.first.id]`; message "Assigned @u."; no users -> "Failed to assign a user because no user was found.") | QA:345 | user parsing differs (#3) |
| `/unassign` | issue_and_merge_request_actions.rb:85 (cond `persisted? && assignees.any? && perm`; ignores args; `assignee_ids = []`; "Removed assignee(s) @a.") | QA:356 | matches (single assignee; pluralisation `assignee`/`assignees` and `to_sentence` irrelevant) |
| `/milestone` | issue_and_merge_request_actions.rb:116 (cond active milestone exists in project or ancestor groups; parse = reference (`%v1`, `%"a b"`, `%iid`) or title via `MilestonesFinder` over project + group ids; no match -> silent) | QA:370 | labrat queries `project_id = p.id` only (#19) |
| `/remove_milestone` | issue_and_merge_request_actions.rb:138 | QA:402 | matches |
| `/copy_metadata` | issue_and_merge_request_actions.rb:162 (parse: `#issue`, work item, epic, `!mr`, URL; else `failed_parse("Failed to find work item or merge request")`; same project required; no message when it is another project) | QA:386 | only `!N`, source must differ, no failed-parse message (#18) |
| `/estimate` `/estimate_time` | issue_and_merge_request_actions.rb:201 (parse `TimeTrackingFormatter.parse(raw, keep_zero: true)`: ChronicDuration, default unit hours; `0` -> 0 -> "Removed time estimate."; invalid -> nil -> `@updates[:time_estimate] = nil`) | QA:408, `parse_duration` QA:133 | duration grammar and zero message (#8) |
| `/spend` `/spent` `/spend_time` | issue_and_merge_request_actions.rb:233 (parse `SpendTimeAndDateSeparator`: duration with optional date `YYYY-MM-DD` (must not be in the future, else whole command ignored, spent_at = date midday in user tz) and `[timecategory:x]`) | QA:420 | only first word is the duration; date/category ignored; zero message (#8) |
| `/remove_estimate` `/remove_time_estimate` | issue_and_merge_request_actions.rb:256 | QA:415 | matches |
| `/remove_time_spent` | issue_and_merge_request_actions.rb:272 | QA:435 | matches (`{"duration":"reset","user_id":..}`) |
| `/lock` | issue_and_merge_request_actions.rb:289 | QA:443 | matches (new-record gap #20) |
| `/unlock` | issue_and_merge_request_actions.rb:306 | QA:448 | matches (new-record gap #20) |
| `/merge` | merge_request_actions.rb:51 (cond `persisted? && MergeOrchestrationService#can_merge?` = auto-merge strategy available OR (`can_be_merged_by && mergeable?`); messages l.35-45) | QA:482 | backticks missing in SHA message (#11); auto-merge message/condition (#12) |
| `/rebase` | merge_request_actions.rb:81 (cond open, source branch exists, can push to it; failures l.82-95 set messages) | QA:498 | protected-branch and unmergeable messages missing (#13) |
| `/draft` | merge_request_actions.rb:122 | QA:453 (new record QA:213) | matches |
| `/ready` | merge_request_actions.rb:155 | QA:465 | matches |
| `/target_branch` | merge_request_actions.rb:180 (message is unconditional; update only if branch exists) | QA:474 | message suppressed in labrat (#6) |
| `/submit_review` | merge_request_actions.rb:194 | QA:530 | missing "Approved the current merge request." for `approve` (#9) |
| `/request_changes` | merge_request_actions.rb:242 | QA:542 | matches except service-failure message (`result[:message]`) not modelled |
| `/approve` | merge_request_actions.rb:263 | QA:514 | same-note sequencing (#16) |
| `/unapprove` | merge_request_actions.rb:300 (cond also `can approve_merge_request`) | QA:526 | same-note sequencing (#16); no `can_approve` check |
| `/request_review` `/assign_reviewer` `/reviewer` | merge_request_actions.rb:355 (cond `can admin_merge_request` on project; first user only; `reviewer_ids = existing + new`) | QA:546 | user parsing (#3); alias name in "Could not apply" (#10); failure message |
| `/unassign_reviewer` `/remove_reviewer` | merge_request_actions.rb:411 (cond reviewers in DB or pending in `@updates`; parse ignores users) | QA:570 | same-note pending reviewers (#16); otherwise matches |
| `/run_pipeline` | merge_request_actions.rb:438 (cond `CreatePipelineService#allowed?`: MR has commits and user can `create_pipeline`; message "New pipeline has been triggered and will appear shortly."; no `@updates`) | **absent** | missing (#2) |
| `/confidential` | issuable_actions.rb:260 (MR: `supports_confidentiality?` false) | absent | "Could not apply confidential command." (#1) |
| `/severity` | issuable_actions.rb:240 (`types Issue`) | absent | "Could not apply severity command." (#1) |
| Issue-only: `/due` `/remove_due_date` `/board_move` `/duplicate` `/clone` `/move` `/create_merge_request` `/zoom` `/remove_zoom` `/add_email` `/remove_email` `/convert_to_ticket` `/promote_to_incident` `/add_contacts` `/remove_contacts` `/timeline` `/relate` | issue_actions.rb:27-345, relate_actions.rb:29 | absent | "Could not apply <name> command." (#1) |
| `/unlink` | relate_actions.rb:46 (`types Issue, MergeRequest`, but `dependency_service.can_admin_link?` is false unless the target is an Issue, work_items/quick_actions/dependency_service.rb) | absent | "Could not apply unlink command." (#1) |
| Work item: `/type` `/promote_to` `/set_parent` (`/epic`) `/remove_parent` `/add_child` `/remove_child` | work_item_actions.rb:17-87 | absent | "Could not apply <definition name> command."; `/epic` reports `set_parent` (#1) |
| Commit: `/tag` | commit_actions.rb:34 | absent | "Could not apply tag command." (#1) |
| `/cc`, `/wip`, `/unwip` etc. | not defined in 18.11 | n/a | stay in note text in both |

## 2. Ranked discrepancies

Notation: `POST notes` = `POST /projects/:id/merge_requests/:iid/notes`. "Alone" means the note body contains only the command (REST 202 `{commands_changes, summary}`); otherwise a 201 Note.

### 1. Non-MR commands are not extracted (high)
- GitLab: extractor recognises every command name. The line is removed from the note and `Could not apply <name> command.` is the message (`valid_type?` false or `condition` false, command_definition.rb:162-170). Full list in table above; message uses the definition name (`/epic` -> `set_parent`).
- labrat: line kept in the note, no message.
- Repro: open MR, owner `POST notes` body `/due tomorrow`: GitLab 202 `{"commands_changes":{},"summary":["Could not apply due command."]}`; labrat 201 Note with `body:"/due tomorrow"`. With `hi\n/due tomorrow`: GitLab 201 Note `body:"hi"`; labrat `body:"hi\n/due tomorrow"`. Same for `/relate #1`, `/confidential`, `/epic &1`, `/severity 1`.
- Fix: add all names to `COMMANDS` (or a second list) and route to the fallthrough arm; extend `canonical_command` with `epic -> set_parent`.

### 2. `/run_pipeline` missing (high)
- GitLab: available when the MR has commits and the user may `create_pipeline` (developer+); `@execution_message[:run_pipeline] = "New pipeline has been triggered and will appear shortly."`; no `@updates`.
- labrat: not in `COMMANDS`; text stays in the note.
- Repro: developer, MR with commits, body `/run_pipeline`: GitLab 202 `{"commands_changes":{},"summary":["New pipeline has been triggered and will appear shortly."]}`; labrat 201 Note with body `/run_pipeline`. Reporter: GitLab "Could not apply run_pipeline command.".

### 3. User argument parsing for `/assign`, `/request_review` (high)
- GitLab (users_extractor.rb:29-90, interpret_service.rb:102-125): args split on whitespace and commas, `and` ignored, `me` = current user, `@` optional, case-insensitive username lookup; every token must resolve, otherwise the whole command fails with `Could not apply assign command. Failed to find users for '@nobody'.` (missing tokens are printed as typed, quoted, `to_sentence`d: `'@a' and '@b'`, `'@a', '@b', and '@c'`). Only `users.first` is used, and the order is the DB order of `User.by_username` (me first), not argument order. Empty list (e.g. `/assign ,`) -> "Failed to assign a user because no user was found." (assign) .
- labrat (`usernames`, QA:108): only `@`-prefixed whitespace tokens, takes the first one that exists, silent when none.
- Repro (MR open, owner, bob is a developer):
  - `/assign me` alone: GitLab 202 `{"commands_changes":{"assignee_ids":[owner.id]},"summary":["Assigned @owner."]}`; labrat 400 `Failed to apply commands.`.
  - `/assign bob` (no `@`): GitLab assigns bob; labrat 400.
  - `/assign @nobody`: GitLab 202 `{"commands_changes":{},"summary":["Could not apply assign command. Failed to find users for '@nobody'."]}`; labrat 400.
  - `/assign @bob thanks`: GitLab 202 `Could not apply assign command. Failed to find users for 'thanks'.`; labrat assigns bob.
  - `/assign @bob,@carol` / `/request_review me` / `/request_review @nobody`: same pattern.

### 4. Label names without `~` (data-destroying for `/unlabel`) (high)
- GitLab: `find_labels` (interpret_service.rb:146-170): `~` references plus, when the arg contains no `~`, a name lookup (`"A B" C` -> names `A B`, `C`) across project and ancestor groups. So `/label bug`, `/unlabel bug`, `/relabel bug` work.
- labrat: `label_titles` (QA:115) only parses `~tokens`. `/unlabel bug` has an empty title list, which labrat treats as "remove all" (QA:272-275).
- Repro: MR labelled `bug` and `ui`; body `/unlabel bug`: GitLab `{"remove_label_ids":[bug.id]}`, `Removed ~"bug" label.`, MR keeps `ui`; labrat `{"label_ids":[]}`, `Removed all labels.`, MR loses both. `/label bug` alone: GitLab `{"add_label_ids":[bug.id]}`, `Added ~"bug" label.`; labrat 202 `Could not apply label command.`.

### 5. Label not-found paths and `/relabel` without args (high)
- GitLab:
  - `/label ~nonexistent` when the project has other labels: condition true, `run_label_command` returns early, no message (`''`) -> alone: 400 `Failed to apply commands.`; with another command the join leaves a double space. If the project has no labels at all: `Could not apply label command.`.
  - `/unlabel ~nonexistent` (MR has labels): no `@updates`; message is `Removed all labels.` (empty reference list falls to the "all" text, issuable_actions.rb:311-318); `commands_changes` `{}`.
  - `/relabel` (no arg) or `/relabel ~nonexistent`: nothing happens, no message.
- labrat: `/label ~nonexistent` -> `Could not apply label command.` (QA:245); `/unlabel ~nonexistent` -> `remove_label_ids: []` and `Removed  labels.` (two spaces; len 0 pluralised, QA:259/291); `/relabel` or `/relabel ~nonexistent` -> `label_ids: []`, `Replaced all labels with  labels.` and the MR loses all labels.
- Repro: MR labelled `bug`, project label `ui` exists; bodies `/label ~zzz`, `/unlabel ~zzz`, `/relabel`.

### 6. `/target_branch` message is unconditional in GitLab (medium-high)
- GitLab: `Set target branch to <branch>.` is printed even when the branch does not exist; `target_branch` only enters `@updates` if `branch_exists?` (merge_request_actions.rb:167-181).
- labrat: no message and no change for a missing branch.
- Repro: body `/target_branch nope` alone: GitLab 202 `{"commands_changes":{},"summary":["Set target branch to nope."]}`; labrat 400 `Failed to apply commands.`.

### 7. Required-arg commands with no argument (medium-high)
- GitLab: command available, block and message skipped, nothing reported (command_definition.rb:147-154). `/title` alone -> no summary -> 400 `Failed to apply commands.`.
- labrat: `/title` alone hits the guard `!args.is_empty()` (QA:235) and falls to `Could not apply title command.` (202).
- Repro: body `/title` on an open MR. (Check the other arms the same way: `milestone`, `target_branch`, `react`, `estimate`, `spend` already produce no message in labrat.)

### 8. Time tracking grammar and zero/negative values (medium-high, verify ChronicDuration)
- GitLab: `Gitlab::TimeTrackingFormatter.parse` (time_tracking_formatter.rb:8-19): ChronicDuration with `default_unit: 'hours'`, 8h day, 20-day month (40h week). Accepts `2` (2h), `1.5h`, `1h 30m`, `1 hour 30 minutes`, `1mo`. `/estimate 0` -> `time_estimate: 0`, `Removed time estimate.`. `/estimate -1h` -> `-3600`, message `Set time estimate to -1h.`. Unparseable `/estimate foo` -> parse nil -> `@updates[:time_estimate] = nil` (key present, `"time_estimate":null`) and no message. `/spend` takes the whole arg minus date and `[timecategory:x]`: `/spend 1h 30m` = 5400s; `/spend 1h 2024-01-02` -> `spent_at` = that date at 12:00 in the user's timezone (future date: command ignored); `[timecategory:x]` -> `category:"x"`; `/spend 0h` ignored (no keep_zero).
- labrat: `parse_duration` (QA:133) needs a unit letter per number and only `w d h m s`; `/estimate 0h` yields `Set time estimate to .`; `/estimate 0` ignored; `/estimate 30` ignored; `/spend` takes only the first word (`/spend 1h 30m` = 3600s, `Added 1h spent time.`), ignores date and category (`spent_at` = now, `category` null); `/spend 0h` -> `Added  spent time.`.
- Repro: `/estimate 0` alone: GitLab `{"time_estimate":0}`, `Removed time estimate.`; labrat 400. `/estimate 30`: GitLab `{"time_estimate":108000}`, `Set time estimate to 3d 6h.` (30h with 8h days); labrat 400. `/spend 1h 30m`: GitLab `spend_time.duration` 5400, `Added 1h 30m spent time.`; labrat 3600.

### 9. `/submit_review approve` omits the approval message (medium-high)
- GitLab: `@execution_message[:submit_review]` is an array `['Submitted the current review.', 'Approved the current merge request.']` (or `'Failed to approve the current merge request.'` when `ApprovalService` returns false) (merge_request_actions.rb:202-223). Joined summary: `Submitted the current review. Approved the current merge request.`.
- labrat: only `Submitted the current review.` (QA:534); approval performed silently.
- Repro: reviewer D (not yet approved) body `/submit_review approve`: GitLab `["Submitted the current review. Approved the current merge request."]`; labrat `["Submitted the current review."]`. Second run (already approved): GitLab `... Failed to approve the current merge request.` (if the service returns false); labrat no failure text.

### 10. Alias name in "Could not apply" for reviewer aliases (medium)
- GitLab: `Could not apply %{name} command.` uses `definition.name`: `request_review` for `/assign_reviewer` and `/reviewer`.
- labrat: `canonical_command` (QA:773) maps `reviewer -> assign_reviewer` and leaves `assign_reviewer`, both wrong.
- Repro: reporter body `/assign_reviewer @bob`: GitLab `Could not apply request_review command.`; labrat `Could not apply assign_reviewer command.`. Fix: `"assign_reviewer" | "reviewer" => "request_review"`.

### 11. `/merge` SHA message loses backticks (medium)
- GitLab (merge_request_actions.rb:37): ``The `/merge` quick action requires the SHA of the head of the branch.``
- labrat (QA:486): `The /merge quick action requires the SHA of the head of the branch.` (docs/gitlab-ref/notes-discussions-reviews.md:341 dropped the backticks too, likely a markdown artifact).
- Repro: mergeable MR, body `/merge` without `merge_request_diff_head_sha`: compare `summary`.

### 12. `/merge` auto-merge feedback and condition (medium)
- GitLab: message is `Set to auto-merge.` when `AutoMergeService#preferred_strategy` is non-nil, i.e. the MR is not immediately mergeable or has a running head pipeline, but mergeability checks the strategy needs pass (auto_merge_service.rb:79-84, merge_when_checks_pass_service.rb:36-45); the condition is `can_merge?` (strategy available OR `can_be_merged_by && mergeable?`), not just `can_be_merged_by`. An MR failing a non-skippable check (e.g. conflicts) -> `Could not apply merge command.`.
- labrat: always `Merged this merge request.` (QA:494) and enables auto-merge when not `mergeable` (QA:690-694); condition is `can_be_merged_by` only.
- Repro: owner, MR with unresolved discussions / draft handled by auto-merge (or running pipeline), body `/merge` with correct `merge_request_diff_head_sha`: GitLab `{"merge":"<sha>"}`, `Set to auto-merge.`; labrat `Merged this merge request.`. Conflicted MR: GitLab `Could not apply merge command.`.

### 13. `/rebase` failure messages (medium)
- GitLab (merge_request_actions.rb:9-11, 82-95): protected source branch without force push -> `This merge request branch is protected from force push.`; `cannot_be_merged?` -> `This merge request is currently in an unmergeable state, and cannot be rebased.`; in progress -> `A rebase is already in progress.`; none set `rebase`.
- labrat (QA:498-513): only the in-progress message; otherwise always schedules.
- Repro: MR whose source branch is protected (or conflicted) and the user can push: body `/rebase`: GitLab `{"commands_changes":{}}` + failure text; labrat `{"rebase":true}` + `Scheduled a rebase of branch x.`.

### 14. `/react` argument format and validity (medium)
- GitLab: argument must contain `:name:`; first match used. Message `Toggled :name: emoji reaction.` is printed for any `:name:` (even a non-existent emoji or a user who cannot award); `emoji_award` is set only if `user_can_award?`.
- labrat: accepts bare `thumbsup`, fails on `:tada: extra` (QA:336 trims colons only), prints nothing and sets nothing for an unknown emoji.
- Repro: `/react thumbsup` alone: GitLab no summary -> 400; labrat 202 `Toggled :thumbsup: emoji reaction.` + award. `/react :nonexistent:`: GitLab 202 `{"commands_changes":{"emoji_award":"nonexistent"},"summary":["Toggled :nonexistent: emoji reaction."]}`; labrat 400. `/react :tada: thanks`: GitLab toggles tada; labrat nothing.

### 15. `/shrug` and `/tableflip` drop their argument in GitLab (low-medium)
- GitLab: replacement text is only the constant (`substitution :shrug do SHRUG end`, issuable_actions.rb:220; perform_substitution `content.sub(..., execute_block(...))`). `/shrug hello` -> `¯\＿(ツ)＿/¯`.
- labrat (QA:91): `hello ¯\＿(ツ)＿/¯`.
- Repro: body `/shrug hello`: compare `body`.

### 16. Same-note command sequencing (medium-low)
GitLab runs each block in order and evaluates later conditions against live state; labrat defers and evaluates against the pre-note DB:
- `/approve\n/approve`: GitLab second one unavailable (stored message deduplicated): one `Approved the current merge request.`; labrat two messages (QA:514-525, `has_approved` still false).
- `/approve\n/unapprove` (not previously approved): GitLab `Approved the current merge request. Unapproved the current merge request.`; labrat `Approved ... Could not apply unapprove command.`.
- `/request_review @bob\n/unassign_reviewer` on an MR with no reviewers: GitLab condition true via pending `@updates`; messages `Requested a review from @bob. Removed reviewer @bob.`, `reviewer_ids: []`; labrat `Requested ... Could not apply unassign_reviewer command.`, `reviewer_ids:[bob]`.
- `/unassign` after a pending `/assign` is unchanged (GitLab conditions use DB assignees there too).

### 17. `/reopen` on a discussion-locked closed MR (low-medium)
- GitLab: condition uses `update_merge_request` (issuable_actions.rb:46-49), so the command is "available": `{"state_event":"reopen"}`, `Reopened this merge request.`; the later `ReopenService` (`reopen_merge_request` denied by `locked`, reopen_service.rb:6) silently leaves it closed (consistent with edge-cases-mr-lifecycle.md C4 for REST).
- labrat (QA:229): `Could not apply reopen command.`.
- Repro: MR with `discussion_locked=true`, closed; owner body `/reopen`.

### 18. `/copy_metadata` source forms and errors (low-medium)
- GitLab: `#issue`, `!mr`, URLs, cross-project refs; unresolved reference -> `Could not apply copy_metadata command. Failed to find work item or merge request.`; another project -> silent; copying from itself allowed.
- labrat (QA:386): `!N` of this project only, not itself, no message otherwise.
- Repro: `/copy_metadata #1` (issue with label+milestone): GitLab `Copied labels and milestone from #1.` + `add_label_ids`,`milestone_id`; labrat 400. `/copy_metadata !999`: GitLab the failed-parse text; labrat 400.

### 19. `/milestone` lookup scope (low-medium)
- GitLab: milestones of the project and its ancestor groups, by reference (`%v1`, `%"v 1"`, `%<iid>`) or title.
- labrat (QA:372-378): `project_id = p.id AND title = ?` only, no iid reference.
- Repro: group milestone `rel-1`, body `/milestone %"rel-1"`: GitLab `{"milestone_id":id}`, `Set the milestone to %"rel-1".`; labrat 400 (the `any_active_milestone` condition passes, then the title lookup fails). `/milestone %1` (iid): same.

### 20. Description (new record) skip list incomplete (low-medium)
- GitLab: every command whose condition includes `persisted?` is unavailable while creating: also `/title`, `/lock`, `/unlock`, `/subscribe`, `/unsubscribe`, `/react`, `/remove_milestone`.
- labrat (QA:203-211) skips only `approve unapprove todo done merge rebase relabel unlabel remove_label remove_estimate remove_time_estimate remove_time_spent unassign submit_review request_changes`.
- Repro: `POST /projects/:id/merge_requests` with `description:"x\n/title Other\n/lock"`: GitLab MR keeps its `title` and `discussion_locked:false`; labrat applies both (verify `ctx.new_record` path at routes/merge_requests.rs:869).

### Smaller items (not in the top 20)
- Repeated `/relabel`: GitLab accumulates into one `label_ids` and one message; labrat the last wins with two messages.
- `/unlabel ~a` then `/unlabel` (all): GitLab one message (`:unlabel` key, last wins: `Removed all labels.`); labrat two.
- Labels whose name contains `"` are referenced as `~<id>` in messages; labrat always quotes.
- `/title X  ` keeps trailing spaces in GitLab (arg is not stripped); labrat trims.
- Unquoted label tokens: GitLab reference regex stops at `,` and a trailing `.`/`?`; labrat keeps them in the title (`~bug,` -> `bug,`). `~123` is an id reference in GitLab, a title in labrat.
- `/request_changes` / `/approve` failure messages from the services (`result[:message]`, `Failed to approve...`) not modelled.
- `/unapprove` omits the `approve_merge_request` ability in labrat (QA:526).
