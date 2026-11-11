# Merge request lifecycle: edge cases that real integrations hit (GitLab 18.11 CE)

Read-only derivation from the GitLab CE 18.11 source copy (no requests were sent to any instance).
Complements `merge-requests.md` (MR) and `repository-and-push.md` (RP); facts already stated there are not repeated
unless this file corrects or sharpens them. Where this file disagrees with `merge-requests.md`, this file was re-derived from
source for that specific point and is marked **[CORRECTS MR doc]**.

Source abbreviations (relative to `gitlab-src/`): **API** = `lib/api/merge_requests.rb`, **HLP** = `lib/api/helpers/merge_requests_helpers.rb`,
**M** = `app/models/merge_request.rb`, **ISB** = `app/services/issuable_base_service.rb`, **US** = `app/services/merge_requests/update_service.rb`,
**MS** = `app/services/merge_requests/merge_service.rb`, **RS** = `app/services/merge_requests/refresh_service.rb`.

## Conventions used by every scenario

Fresh state **S0**: user `owner` (Owner/admin), `dev` (Developer), `rep` (Reporter), `out` (non-member; project private unless said).
Project `P` (id `1`), default branch `main` with one commit containing `README.md`; no CI config; project defaults
(merge_method `merge`, squash_option `default_off`, only_allow_merge_if_pipeline_succeeds false, only_allow_merge_if_all_discussions_are_resolved false).
Helper macros (all as `owner`, token in `PRIVATE-TOKEN`):
- `BR(name)` = `POST /projects/1/repository/branches?branch=<name>&ref=main` -> 201.
- `COMMIT(branch, msg, file)` = `POST /projects/1/repository/commits` body `{branch, commit_message: msg, actions:[{action:"create", file_path:file, content:"x"}]}`.
- `MR(src, tgt, title)` = `POST /projects/1/merge_requests {source_branch, target_branch, title}`.
- `feature` := `BR(feature)` then `COMMIT(feature, "Add a", "a.txt")`.
Error body shapes (HLP:126-138, `lib/api/helpers.rb:622-628,656-678`):
- `render_validation_error!` -> 400 `{"message": {"<attr>": ["<msg>", ...]}}` (hash).
- `unprocessable_entity!(errors[key])` / `conflict!(errors[key])` receive an **Array** -> `{"message": ["<msg>"]}` (array, not string) **[CORRECTS MR doc where it implies a string]**.
- Grape param errors -> 400 `{"error": "<param> is missing"}` (key `error`, not `message`).
Timing note: after-create diff creation, push processing (RefreshService), mergeability checks and branch-delete MR closing are all
Sidekiq jobs on a real instance. Scenarios flag where an immediately-following read can observe the transient state.

---------------------------------------------------------------------------------------------------

## A. Create validations

### A1. Whitespace-only title (POST)
1. S0 + `feature`. `POST /projects/1/merge_requests {source_branch:"feature", target_branch:"main", title:"   "}`.
Expected: **400** `{"message":{"title":["can't be blank"]}}`. Grape `requires :title` (API:360) does not reject blanks; `strip_attributes! :title`
(`app/models/concerns/issuable.rb:152`) turns `"   "` into nil, then `validates :title, presence` (issuable.rb:81) fails. No MR row is persisted (iid gaps: see MR doc 9.1).
`title:""` gives the identical response. Omitting `title` -> 400 `{"error":"title is missing"}`.
Confidence: high.

### A2. Title padded with spaces is stored trimmed
1. `MR(feature, main, "  Add a  ")` -> **201**, `"title":"Add a"` (strip_attributes, issuable.rb:152). Interior runs of spaces are NOT collapsed.
2. `MR` with `"  Draft: x"` -> title stored `"Draft: x"` and `draft:true` (the draft regex is anchored at start of the *stripped* title; MR doc 3.5 says leading whitespace
   disables it, which only holds for titles that are persisted untrimmed, which cannot happen via the API). Confidence: medium-high.

### A3. Title length boundary (255 chars, counted in characters not bytes)
1. title = 255 x `"a"` -> 201. 255 x `"é"` (510 bytes) -> 201 (Rails `length` validator counts characters; issuable.rb:35,81).
2. title = 256 chars -> **400** `{"message":{"title":["is too long (maximum is 255 characters)"]}}`.
3. 255 chars + trailing spaces (total 260) -> 201 (stripping happens before validation).
Confidence: high.

### A4. Description exactly at the byte limit
Limit = `ApplicationSetting#description_and_note_max_size` (default 1_048_576 bytes), `BytesizeValidator` (issuable.rb:84, `app/validators/bytesize_validator.rb:17-28`).
1. description of 1_048_576 ASCII bytes -> 201. 1_048_577 bytes -> **400**
   `{"message":{"description":["is too long (1 MB). The maximum size is 1 MB."]}}` (both sizes humanised, so the message looks self-contradictory; reproduce verbatim).
2. 524_289 x `"é"` (2 bytes each = 1_048_578 bytes, only 524_289 characters) -> 400: the limit is bytes, not characters.
3. Quick actions are interpreted BEFORE validation and stripped from the stored text (MR doc 4.1/9.5); size is checked on the stripped text.
Confidence: high (limit/default), medium (exact human-size string; `number_to_human_size` with precision 3 strips zeros -> "1 MB").

### A5. Branch name validity vs. existence
`Gitlab::GitRefValidator.validate_merge_request_branch` (`lib/gitlab/git_ref_validator.rb:28-32,46-62`) works on raw bytes and applies only when the attribute changes
(M:1383-1391). Invalid: empty, leading `-`, ending `/` or `.`, any of `\x00-\x1f \x7f space ~ ^ : ? * [ \`, containing `..` `//` `@{`, any path component starting
with `.` or ending with `.lock`. Valid: unicode (bytes >= 0x80), single dots inside (`rel.1.2`), `@`, `+`, `#`, `%`, deep slashes.
1. `BR("feat/üñï.v2")` (URL-encode in path) then commit; `MR("feat/üñï.v2", main, t)` -> 201 `source_branch:"feat/üñï.v2"` (UTF-8 preserved).
2. `MR("a..b", main, t)` -> **400** `{"message":{"source_branch":["is invalid"]}}`. Same for `"x.lock"`, `"a/.b"`, `"a b"`, `"-x"`, `"a//b"`, `"a@{b"`.
3. Both `source_branch` and `target_branch` invalid -> both keys in the same hash: `{"message":{"source_branch":["is invalid"],"target_branch":["is invalid"]}}`
   (validate_branches loops both attrs, M:1362).
Confidence: high for the rules; medium for ordering of keys.

### A6. Creating an MR from a branch that does not exist (flag off by default)
1. S0 (no `ghost` branch). `MR("ghost", "main", "t")`.
Expected: **201**, not 400/404. `validate_branch_existence` is gated by feature flag `validate_merge_request_branch_existence`
(`default_enabled: false`, `config/feature_flags/gitlab_com_derisk/validate_merge_request_branch_existence.yml`; M:405,1393-1404; `create_service.rb:41`).
Resulting MR: `state:"opened"`, `sha:null`, `diff_refs:null`, later `has_conflicts:true`, `detailed_merge_status` becomes `commits_status`
(no commits / `branch_missing?`, M:2026,2538), `merge_status` ends at `cannot_be_merged`. If the flag were on the response would be
**400** `{"message":{"source_branch":["does not exist"]}}` (and/or target_branch). Same for nonexistent `target_branch:"nope"`.
Confidence: medium-high (depends on the instance not enabling the derisk flag; default is off).

### A7. source == target
`MR("main","main",t)` -> **422** `{"message":["You can't use same project/branch for source and target"]}` (M:1357-1360; HLP:9,131; array). No branch-name checks run
after that (early return). Confidence: high.

### A8. Duplicate open MR for the same source/target -> 409 (array message)
1. `MR(feature, main, "one")` -> 201 (`iid:1`). 2. `MR(feature, main, "two")` -> **409** `{"message":["Another open merge request already exists for this source branch: !1"]}`
(M:1376-1381; `to_reference` is short form `!1`; HLP:135). 3. `close` MR 1 (A/B-section) then `MR(feature, main, "two")` -> 201 `iid:2` (only `opened` MRs conflict).
4. `MR(feature, "other", "three")` (different target) -> 201: the uniqueness is (source_project, source_branch, target_branch) among opened (M:1342-1352).
Confidence: high.

### A9. Milestone from another project / group is silently ignored
S0 + second project `Q` with milestone id `M9` (global id). `POST /projects/1/merge_requests {..., milestone_id:M9}` -> **201**, `"milestone":null`.
`PUT .../merge_requests/1 {milestone_id:M9}` -> 200, milestone unchanged. Lookup is `MilestonesFinder(project_ids:[P], group_ids: P.group ancestors, ids:[id])`;
no match -> assignment skipped, no error (`app/services/issuable/callbacks/milestone.rb:12-28`). `milestone_id:0` or `""` clears (blank or `'0'`).
Nonexistent id behaves like another project's. As `rep` (no `set_merge_request_metadata`) the whole milestone callback is a no-op (callbacks `has_permission?`).
Confidence: high.

### A10. Labels param: spaces, commas, duplicates, unknown
Coercion (`lib/api/validations/types/comma_separated_to_array.rb`): String -> `split(',').map(&:strip)`; Array -> each element split on commas and stripped.
Then `Issuable::Callbacks::Labels` (`app/services/issuable/callbacks/labels.rb:20-60`, `app/services/labels/available_labels_service.rb:12-31`).
1. S0, label `bug` exists. `POST ... {labels:" bug , new one ,bug"}` as `owner` -> 201, `"labels":["bug","new one"]` (duplicates collapsed by `uniq`, whitespace trimmed,
   inner space kept, response sorted by title; label `new one` is **created** as a project label with default color because owner has `admin_label`).
2. Same call as `dev` (Developer: has `admin_label`? Developer does NOT; Maintainer does): unknown names are silently skipped, existing `bug` still applied -> `["bug"]`.
   (If labrat's permission map lacks `admin_label` for Developer this is where it diverges.)
3. `labels:""` on PUT -> `[]` -> removes all labels (not "ignored"). `labels:"a,,b"` -> `["a","","b"]`; the blank name yields an unsaved label (no id) which is dropped by
   `label_ids=`; result only `a`,`b` (medium).
4. Label match is exact/case-sensitive on title: `labels:"Bug"` does not match `bug` and creates a second label `Bug` (medium; depends on Label title uniqueness validation, not re-verified).
5. As `rep` author: labels ignored silently (needs `set_merge_request_metadata`, labels.rb:17).
Confidence: high for 1,3(first half),5; medium otherwise.

### A11. assignee_ids: duplicates, non-members, multiple, zero
`allows_multiple_assignees?` false -> `params.first(1)` (ISB filter_assignees_using_checks, ISB:95-112).
1. `assignee_ids:"5,5"` (coerced `[5,5]`) -> assignee 5 only. `assignee_ids:"5,6"` -> only 5 (second silently dropped). `assignee_ids[]=6&assignee_ids[]=5` -> only 6 (order wins, not validity).
2. Non-member `out` (id 9) on a **private** project: `User.id_in([9]).select(user_can_read?)` -> empty -> `params.delete(:assignee_ids)`; POST returns 201 with `"assignees":[]`, `"assignee":null`.
   On a **public** project a non-member can read, so they ARE assigned.
3. Nonexistent id 99999 -> same as 2, 201/200, unassigned, no error.
4. `PUT {assignee_ids:"0"}` or `{assignee_ids:""}` or `{assignee_id:0}` -> unassigns. `{assignee_id:5, assignee_ids:"6"}` -> `assignee_id` wins (`convert_parameters_from_legacy_format`,
   `lib/api/helpers/common_helpers.rb:6-14`).
5. PUT with ONLY `assignee_ids` takes the specialized path (`US:325-337`, `update_assignees_service.rb`): a non-readable user adds an error that is then wiped by
   `handle_merge_request_errors!` calling `valid?` -> **200**, assignees unchanged. Combined with `title` it goes the general path (ISB filters, same observable result).
6. As `rep` (non-author MR): 403 `"403 Forbidden"` at `find_merge_request_with_access(:update_merge_request)`; as `rep` who is the *author*: 200 and `assignee_ids` silently dropped.
Confidence: high for 1,4; medium for 2/5 detail.

### A12. Reporter/author permission edge: what a Reporter-author can change
Policy `app/policies/issuable_policy.rb:43-46` gives `update_merge_request` and `reopen_merge_request` to the MR author/assignee who can read it.
`rep` can create an MR (needs `create_merge_request_from`, Developer by default; if the role matrix gives Reporter no push, the POST returns 403 -- use a Developer author for this test).
Dev author `dev` demoted to Reporter after creating: `PUT {title:"x", labels:"bug", assignee_ids:"5", milestone_id:1, state_event:"close"}` -> 200, title and state change,
labels/assignee/milestone dropped silently. Confidence: medium (depends on member role matrix in labrat).

---------------------------------------------------------------------------------------------------

## B. Update validations and partial application

### B1. PUT with no mutable params
`PUT /projects/1/merge_requests/1 {}` -> **400** `{"error":"assignee_id, assignee_ids, reviewer_ids, description, labels, add_labels, remove_labels, milestone_id, remove_source_branch, allow_collaboration, allow_maintainer_to_push, squash, target_branch, title, state_event, discussion_locked, merge_after are missing, at least one parameter must be provided"}`
(API:60-80 list `update_params_at_least_one_of`; Grape message format). `PUT {title:""}` -> 400 `{"error":"title is empty"}` (`allow_blank: false`, API:743). `{title:"   "}` -> same "title is empty"
(Grape treats whitespace as blank; medium) -- if it passed it would give the A1 model error. `{target_branch:""}` -> `{"error":"target_branch is empty"}`.
Confidence: high for list/format, medium for whitespace.

### B2. target_branch change to the source branch
`MR(feature, main)`; `PUT {target_branch:"feature"}` -> **422** `{"message":["You can't use same project/branch for source and target"]}`. MR unchanged in DB (save failed), but
`merge_request` in memory is invalid so response is the error. Same if a closed MR is targeted (the branch_conflict check ignores state; M:1357 runs for closed MRs too since
only `allow_broken`/transition skip it; but `CloseService` sets `allow_broken=true` only in memory during close). Confidence: high.

### B3. target_branch change to a branch that already has an open MR from the same source
MR1 `feature -> main`, MR2 `feature -> dev` (BR(dev)). `PUT /merge_requests/2 {target_branch:"main"}` -> **409**
`{"message":["Another open merge request already exists for this source branch: !1"]}`. Confidence: high.

### B4. target_branch to a nonexistent / invalid branch on UPDATE
Existence validation is `on: :create` only (M:405) so `PUT {target_branch:"nope"}` -> **200**, `target_branch:"nope"`, system note "changed target branch from `main` to `nope`"
(US:211-222). No new diff version (`reload_diff_if_branch_changed` requires both heads, M:1513-1518). `mark_as_unchecked` (US:48-52); next check -> `branch_missing?` -> `cannot_be_merged`,
`has_conflicts:true`, `detailed_merge_status:"commits_status"` (once rechecked). `{target_branch:"a..b"}` -> 400 `{"message":{"target_branch":["is invalid"]}}`.
Confidence: medium-high.

### B5. Updating a MERGED MR's target_branch (same project) **[CORRECTS MR doc 4.4 wording]**
Merge MR1, then `PUT {target_branch:"other"}` as owner. `general_fallback` only strips `target_branch` when `closed_or_merged_without_fork?` i.e. fork MR whose source project is gone
(US:122-125, M:1435-1442). For same-project merged MR the change is **accepted**: 200, new `target_branch`, "changed target branch" system note, and because `changed_fields` includes
`target_branch` the code calls `mark_as_unchecked` (US:48-52): `merge_status` flips `can_be_merged -> unchecked` and `detailed_merge_status` shows `"unchecked"` for a merged MR
(unchecked/checking/preparing are tested BEFORE `not_open`, `detailed_merge_status_service.rb:13-16`). A developer GET then moves it to `checking` (async recheck).
Validation still blocks `target_branch == source_branch` (422) and a name that is invalid (400) for merged/closed MRs (branch checks apply in every state). Confidence: medium-high.

### B6. Partial application: state_event persists even when the rest of the PUT fails
`ISB#update` runs `change_additional_attributes` (state change, via `ReopenService`/`CloseService`, which save on their own) BEFORE `assign_attributes`+`save` (ISB:312-325).
1. MR1 closed. `PUT {state_event:"reopen", title:<256 chars>}` -> **400** `{"message":{"title":["is too long (maximum is 255 characters)"]}}`; yet a following GET shows `state:"opened"`
   (reopen committed, "reopened" system note, event, diff reload). Likewise `{state_event:"close", target_branch:"<source>"}` -> 422 but MR is `closed`.
2. Conversely `{state_event:"close", title:"new"}` -> 200 closed with new title.
Confidence: medium-high (derived from control flow; no other transaction wraps the state change).

### B7. Description validation only when the description changes
`validate_description_length?` (issuable.rb:238-246): unchanged description is never re-validated, and a change is also exempt when the previous stored text was already over the limit
(limit lowered later). So `PUT {title:"x"}` on an MR with an oversize legacy description -> 200. Confidence: medium.

---------------------------------------------------------------------------------------------------

## C. State transitions

### C1. Close, then close again (idempotency)
`PUT {state_event:"close"}` on opened -> 200 `state:"closed"`, `closed_at`, `closed_by` = actor (metrics, `merge_request_metrics_service.rb:14`), system note "closed".
Second identical call -> **200** with unchanged body (`close` event only from `opened`; `merge_request.close` returns false so no note/event; `CloseService:7-31`, state machine M:211-214).
Same for `reopen` on an opened MR. Confidence: high.

### C2. Close / reopen a MERGED MR
Merge MR1. `PUT {state_event:"close"}` -> **200**, `state:"merged"` (no-op). `PUT {state_event:"reopen"}` -> **200**, `state:"merged"`. No 4xx. Same for `locked` (only observable mid-merge).
`closed_by`/`closed_at` stay null. Confidence: high.

### C3. Reopen when another open MR exists for the same branches  **[CORRECTS/sharpens MR doc 4.3]**
1. `MR(feature, main, "one")` (iid1); close it; `MR(feature, main, "two")` (iid2, opened). 2. `PUT /merge_requests/1 {state_event:"reopen"}`.
Flow: `ReopenService` calls `merge_request.reopen`; the save re-runs `validate_branches` (the `state_id` machine does not set `transitioning`; only `merge_status` does, M:331-333), which adds
`:validate_branches`, so the event returns false and MR1 stays `closed` (no event, no note, no diff reload). The API then calls `merge_request.valid?` on the *still closed* MR (the duplicate
check is `if opened?`, M:1364) which passes -> **200** with `"state":"closed"`. I.e. the conflict is NOT surfaced as 409 on PUT (it is surfaced in the web UI only).
Confidence: medium (relies on state_machines rolling the in-memory state back after a failed transition save; if it did not, the response would be 409 `["Another open merge request ... !2"]`).

### C4. Reopen blocked by discussion_locked (applies to Owner too)
`PUT {discussion_locked:true}` then close, then `PUT {state_event:"reopen"}` -> **200**, still `closed`. `MergeRequestPolicy` `rule { locked }.policy { prevent :reopen_merge_request }`
(`app/policies/merge_request_policy.rb:6-8`; `locked` = `discussion_locked?`, issuable_policy.rb:6), and `ReopenService` checks `can?(:reopen_merge_request)` (reopen_service.rb:6) and returns silently.
Note `discussion_locked` itself needs `set_merge_request_metadata` (Developer+); author-only Reporter gets it dropped. Confidence: high.

### C5. Delete the source branch of a CLOSED MR, then reopen
1. `feature` MR1; close MR1; `DELETE /projects/1/repository/branches/feature` -> 204. RefreshService only touches **opened** MRs
   (`base_service.rb:267`: `mr_states: [:opened]`), so MR1 gets no "deleted source branch" note and no diff change.
2. `PUT {state_event:"reopen"}` -> **200**, `state:"opened"` (no branch-existence check on reopen; flag only guards create). `ReopenService` calls `reload_diff` (may produce an `empty`
   version or fail softly) and `mark_as_unchecked`. A developer read then recomputes: `merge_status:"cannot_be_merged"`, `has_conflicts:true`, `detailed_merge_status:"commits_status"`.
3. The reopened MR is NOT auto-closed again until a later push event for that branch name (RS:70-87 runs only on pushes touching the branch).
Confidence: medium (step 2 diff-reload failure mode not exercised in source).

### C6. Delete the source branch of an OPEN MR (async closing)
`DELETE .../branches/feature` -> 204. Within a Sidekiq round-trip `RS#close_upon_missing_source_branch_ref` (RS:70-87) closes the MR via `CloseService` as the deleting user:
`state:"closed"`, `closed_by` = deleter, system note "closed" plus (RS:~330) "deleted source branch `feature`". Immediately after the DELETE the MR may still read `opened`. Recreate the branch
(`BR(feature)`) -> system note "restored source branch"; MR stays `closed` (MR doc §8). Confidence: high (async window), medium (note texts).

### C7. Merge a closed or merged MR
`PUT .../merge` on closed MR -> **405** `{"message":"405 Method Not Allowed"}` (`mergeable?` false at `not_open`, API:~828). On an already merged MR -> 405 too (not 422, not idempotent 200).
But with `auto_merge:true` (see E2/F4): closed MR -> 405; merged MR -> 405. Confidence: high.

### C8. Merge without permission
`rep`/`out` (project readable) `PUT .../merge` -> **401** `{"message":"401 Unauthorized"}` (API `unauthorized! unless can_be_merged_by?`, API:~813), NOT 403 and NOT 405.
`dev` on a protected `main` where Developers have "no access to merge" -> 401 too. Invisible project -> 404. Confidence: high.

### C9. Force-push the source branch to the target head (MR becomes empty)
1. `feature` (1 commit), MR1 open. 2. Force push `feature` to `main`'s sha (git push -f origin <main-sha>:refs/heads/feature).
RS (RS:26-37): `branch_and_project_match` -> `reload_diff` creates a new diff version whose compare has zero commits -> `merge_request_diffs.state = "empty"` (`merge_request_diff.rb:945-946`),
`merge_error` cleared, `mark_as_unchecked`. No "added N commits" note (`@commits` empty -> `notify_about_push` returns, RS:~320), no draft marking.
Reads after recheck: `state:"opened"`, `changes_count:null`, `sha` = main's sha, `diff_refs` set (base=start=head), `has_conflicts:true`, `merge_status:"cannot_be_merged"`,
`detailed_merge_status:"commits_status"`. `GET /versions` lists both versions (`merge_request_diffs.order_id_desc`, no `viewable` filter), newest has `state:"empty"`.
`PUT /merge` -> 405. The MR is NOT auto-marked merged (post_merge_manually_merged only runs for pushes to the TARGET and skips `empty` diffs, RS:94-110).
Confidence: medium-high.

### C10. Push to the TARGET so that it contains the source head (manual merge)
`git push origin feature:main` (fast-forward main to feature). RS `post_merge_manually_merged` (RS:94-130): the pushed commits include MR1's `diff_head_sha` and its diff is not empty ->
MR1 -> `state:"merged"`, `merged_at`, `merge_user` = pusher, `merge_commit_sha:null`/`merged_commit_sha` = the head commit (analyzer finds no merge commit), `squash_commit_sha:null`.
If the MR was already empty (C9) it is NOT merged. Source branch is not deleted. Confidence: medium.

### C11. Pushing the same commit twice
Pushing a ref to the sha it already has is a client-side no-op ("Everything up-to-date"); no post-receive. Pushing the same commit to a *second* branch name creates a branch (RS `branch_added`):
if an open MR exists for that name, `find_new_commits` uses merge-base so the "added commits" note only lists commits not already in the MR (`existing_commits` partition, RS:~335-355)
and the system note reads "added N commits" with the *existing* ones summarised separately. Via the API, `POST /repository/branches?branch=feature&ref=<sha>` for an existing name -> 400 `{"message":"Branch already exists"}`. Confidence: low-medium.

### C12. Auto-draft from pushed commit subjects (`fixup!` anywhere)
Push to `feature` a commit with subject `Fix x (squash! see later)`. `Commit::DRAFT_REGEX = /\A\s*#{merge_request_draft}|(fixup!|squash!)\s/` (`app/models/commit.rb:549`): the second alternative is
**not anchored**, so any subject containing `fixup! ` or `squash! ` followed by whitespace matches. RS `mark_mr_as_draft_from_commits` (RS:361-379) only considers commits that are in the MR and updates the title to
`"Draft: <title>"` (direct `update`, system note "marked this merge request as draft from <sha>"); `draft:true`, `detailed_merge_status:"draft_status"`. Does not happen on API creation (no BuildService),
nor if the MR is already draft. Un-drafting by PUT title to the stripped title works; the next push with another draft commit re-drafts. Confidence: high.

---------------------------------------------------------------------------------------------------

## D. Merge edge cases

### D1. sha param checks (409 vs 422 vs 405 ordering)
Order in `PUT /merge` (API:~811-850): 1) `can_be_merged_by?` else 401; 2) `build_merge_params` -> `check_sha_param!` (`lib/api/helpers.rb:600-604`) -> **409**
`{"message":"SHA does not match HEAD of source branch: <diff_head_sha>"}`; 3) `merge_request.update(squash: ...)` side effect (D4); 4) `mergeable?` else 405; 5) MergeService; 6) not merged -> 422 `"Branch cannot be merged"`.
So: sha of an OLDER diff version (push after create, before merge) -> 409 with the newest sha, even if the MR is also a draft (409 beats 405). `sha` of the current head but MR draft -> 405.
`sha:""` is falsy-checked by `params[:sha] &&` -> "" is truthy in Ruby -> 409. The service-level message "Branch has been updated since the merge was requested..." (MS:60-65) is only reachable in a race
(API pre-check passed, then branch moved). It is saved to `merge_error` and the API returns 422. Confidence: high.

### D2. Merge with unresolved discussions / draft / conflict -> 405 shapes
All pre-merge failures from `mergeable?` -> **405** `{"message":"405 Method Not Allowed"}` with no hint; clients must read `detailed_merge_status` (see E). Example S0: `MR(feature, main, "Draft: x")`, wait for
`detailed_merge_status:"mergeable"`? No: -> `"draft_status"`; `PUT /merge` -> 405; `PUT {title:"x"}` -> 200 `draft:false`, detailed becomes `unchecked`/`checking` then `mergeable`; `PUT /merge` -> 200.
After success: `state:"merged"`, `merge_commit_sha` set, `squash_commit_sha:null`, `should_remove_source_branch:null`, `force_remove_source_branch:false`... (MR doc 5.2). Confidence: high.

### D3. merge_commit_message empty string; squash_commit_message empty string
`merge_params` keeps `""` (`.compact` removes only nil, API:143-153) and `MergeStrategies::FromSourceBranch#merge_commit_message` is `merge_params[:commit_message] || default` (from_source_branch.rb:~126):
`""` is truthy in Ruby -> an empty message is sent to Gitaly `UserMergeBranch`, NOT the default message. Gitaly behaviour for empty message not in tree: expect either a commit with empty message
or a failure surfacing as 422 `"Branch cannot be merged"` with `merge_error:"An error occurred while merging"` (MS:~105). Confidence: low (Gitaly unverified).
By contrast `squash_commit_message:""` falls back to default: `SquashService#message` uses `commit_message.presence || default_squash_commit_message` (squash_service.rb:48-50) -> MR title. Confidence: high.

### D4. squash on the merge call: persisted flag + cannot be turned off
`build_merge_params` does `merge_request.update(squash: params[:squash]) if params[:squash]` (API:146) BEFORE mergeability: `PUT /merge {squash:true}` on a draft MR -> 405 **and** `squash` is now true on the MR (GET shows
`squash:true`). `squash:false` on merge is a no-op (falsy, never written) so it cannot clear a previously set flag. Confidence: high.

### D5. Squash with a single commit whose message equals the MR title
Setup: `BR(f)`, `COMMIT(f, "Add a", a.txt)` (message exactly `Add a`), `MR(f, main, "Add a")`; `PUT /merge {squash:true}`.
`SquashService#execute` (squash_service.rb:16-20): `commits_count == 1 && message.strip == first_commit.safe_message.strip` -> returns `squash_sha: diff_head_sha` without creating a commit
(no Gitaly squash). Result: `squash_commit_sha == sha` (the original commit), `merge_commit_sha` = new merge commit (merge_method merge). `squash:true`. If the commit message has a body
(`"Add a\n\nbody"`) or differs from the title, a squash commit with message = MR title (or `squash_commit_template`, or `squash_commit_message` param) is created with new sha. A given `squash_commit_message:"X"` differing from the lone commit message forces a real squash even for 1 commit.
With merge_method `ff`: `merge_commit_sha:null`, `squash_commit_sha` = head. Confidence: high.

### D6. Squash options: `never` and `always`
1. Project `squash_option=never` (`PUT /projects/1 {squash_option:"never"}`): `POST` MR with `squash:true` -> 201 `squash:true`, `squash_on_merge:false` (`squash_on_merge?` returns false first for never, M:2013-2018).
   `PUT /merge {squash:true}` merges WITHOUT squashing (the "Squashing not allowed" error in `SquashService#squash_forbidden?` is unreachable because `squash_on_merge?` is already false). `squash_commit_sha:null`.
2. `squash_option=always`: MR created with `squash` absent -> default true (`create_service.rb:81-83`); created with explicit `squash:false` -> `squash:false`, `squash_on_merge:true`.
   `PUT /merge` (no squash param) on that MR: `missing_required_squash?` = `!squash && squash_always?` (M:2729) -> validate! raises `"This project requires squashing commits when merge requests are accepted."`
   -> API 422 `{"message":"Branch cannot be merged"}`, MR stays opened, `merge_error` set to that text. `PUT /merge {squash:true}` succeeds (D4 sets flag first).
Confidence: high (1), medium-high (2).

### D7. should_remove_source_branch with protected / default source branch
1. Source = a protected branch (`POST /projects/1/protected_branches {name:"feature"}`) or `main` itself from a fork: `PUT /merge {should_remove_source_branch:true}` -> 200 merged; branch is **not** deleted and no error.
   `delete_source_branch?` ANDs param with `can_remove_source_branch?` (M:1733-1739: not protected, not root_ref, `push_code`, `diff_head_sha == source head`; MS:~145-150).
   Response `should_remove_source_branch:null` (the param is stored in `merge_params` only when removable, MS `merge_request_merge_param`).
2. Normal `feature`: `should_remove_source_branch:true` -> merged; branch removed asynchronously by `DeleteSourceBranchWorker` (GET branch may still return 200 for a moment); response `should_remove_source_branch:true`.
3. `remove_source_branch:true` at create -> `force_remove_source_branch:true`; merging with no param deletes it; `should_remove_source_branch:false` on the merge call overrides (params.fetch key present).
4. Create with `remove_source_branch:true` where source is protected: accepted at create (flag only), merge keeps the branch.
Confidence: high for 1-3.

### D8. FF merge when target already contains the source / source has no new commits
Project `merge_method=ff` (`PUT /projects/1 {merge_method:"ff"}`).
1. `feature` created, MR1 opened, then `git push origin feature:main` (C10) -> MR1 auto-merged (not an API merge).
2. If instead `main` is advanced (via COMMIT) so that it contains `feature`'s head but MR1's diff was computed before: MR remains opened; `should_be_rebased?` false (`ancestor?(target_sha, diff_head)` is about target being an ancestor of source, M:1721-1726).
3. Source behind target (never had own commits, `BR(f)` from main, then `COMMIT(main,...)`): MR creation succeeds; diff `empty`; `PUT /merge` -> 405 (`commits_status`); `rebase` does nothing useful.
4. FF with target moved ahead and diverged: `detailed_merge_status:"need_rebase"`; merge -> 405; `PUT /rebase` -> 202 `{"rebase_in_progress":true}`.
Confidence: medium.

### D9. Merge MR whose source has no new commits (empty at creation)
`BR(e)` from main; `MR(e, main, "t")` -> 201. Wait for async: `detailed_merge_status:"commits_status"`, `merge_status:"cannot_be_merged"`, `has_conflicts:true`, `changes_count:null`.
`PUT /merge` -> **405**. `PUT {state_event:"close"}` -> 200 works (`allow_broken=true`). Confidence: high.

### D10. 'merge_when_pipeline_succeeds'/`auto_merge` in a project without CI **[sharpens MR doc 5.4]**
`PUT /merge {auto_merge:true}` (API:~816-838). `available_strategies` for an open, mergeable MR with no running pipeline is EMPTY
(`MergeWhenChecksPassService#availability_details`: `next default_error if mergeable? && !pipeline_in_progress`, merge_when_checks_pass_service.rb:49-58). Fallback `pipeline_allows_merge?` (API:136-141):
feature flag `merge_immediately_when_no_pipeline` is **off** (default) -> `diff_head_pipeline_success?` = `!!nil&.success?` = false -> `not_allowed!` -> **405**.
So on a CI-less project `auto_merge:true` returns 405 for a perfectly mergeable MR, and 405 for a closed/merged MR. With the flag ON it would merge immediately (200).
Confidence: medium (flag state of the target instance; default off).

### D11. auto_merge on a DRAFT MR is accepted
`MR(f, main, "Draft: x")`, wait until `merge_status:"can_be_merged"`. `PUT /merge {auto_merge:true}`: strategy availability skips the draft check (`skipped_auto_merge_checks`, M:1574-1599 `skip_draft_check`) and the
override only blocks when `mergeable?` is TRUE, so a draft (not mergeable) -> available -> **200**, `merge_when_pipeline_succeeds:true`, `merge_user` = caller, system note "enabled an automatic merge when all merge checks for ... pass".
Nothing merges: `AutoMergeProcessWorker` -> `process` -> `mergeable?` false. Un-drafting later triggers the merge. `DELETE .../cancel_merge_when_pipeline_succeeds` -> 200 (MR doc 1.16 quirks apply).
Confidence: medium-high.

---------------------------------------------------------------------------------------------------

## E. Mergeability and detailed_merge_status

All results below are what a *developer* sees after the async check completed, unless noted. Check order (first failing wins; `detailed_merge_status_service.rb`, M:1596-1660):
`not_open, draft_status, merge_time, locked_lfs_files, discussions_not_resolved, ci_must_pass(skipped here, evaluated last), commits_status, need_rebase, conflict`.
Preface states preempt everything: `preparing` -> `checking` -> `unchecked` first.

### E1. Combination matrix (each row is its own run from S0)
| Setup | detailed_merge_status | merge_status | has_conflicts |
|---|---|---|---|
| Draft + conflicting with main | `draft_status` | `cannot_be_merged` | true |
| Closed + Draft | `not_open` | unchanged stored value | per stored |
| Opened, conflict + `only_allow_merge_if_all_discussions_are_resolved` with an unresolved thread | `discussions_not_resolved` (before conflict) | `cannot_be_merged` | true |
| Unresolved thread + merge_method `ff` and target diverged (need rebase) | `discussions_not_resolved` (discussions precede need_rebase) | `can_be_merged` | false |
| Need rebase + conflict (ff project) | `need_rebase` (rebase check precedes conflict) | `cannot_be_merged` | true |
| Draft + no commits | `draft_status` (draft before commits_status) | `cannot_be_merged` | true |
| Merged MR whose stored merge_status is `can_be_merged` | `not_open` | `can_be_merged` | false |
Closing an MR whose merge_status is still `unchecked`/`preparing` (e.g. closed immediately after create) reports `unchecked`/`preparing` (not `not_open`), see B5. Confidence: high for ordering, medium for rows that need discussions (notes API).

### E2. Draft toggling changes status without waiting
`PUT {title:"Draft: t"}` on mergeable MR1 -> 200 with `draft:true`, `work_in_progress:true`, `detailed_merge_status:"draft_status"` in the same response (check runs against current model; stored
merge_status is untouched, so `merge_status:"can_be_merged"` still). `PUT {title:"t"}` -> `mergeable` again immediately. `Draft:` alone as title is valid (stripped to `Draft:`). `[Draft]`, `(draft)`, `DRAFT:` all match; `Draft -`, `WIP:` do not.
Confidence: high.

### E3. merge_status as seen by anonymous / reporter / developer
Entity (`lib/api/entities/merge_request_basic.rb:66-73`): only users with project-level `update_merge_request` (Developer+; **not** the MR author) trigger `check_mergeability(async: true)`; everyone gets `public_merge_status` (M:390-392):
`preparing` and `cannot_be_merged_rechecking` print as `"checking"`; `cannot_be_merged_recheck`, `unchecked`, `checking`, `can_be_merged`, `cannot_be_merged` print as-is.
Scenario: MR1 has a conflict (stored `cannot_be_merged`). `COMMIT(main, ..., unrelated.txt)` (push to target -> `mark_as_unchecked` -> `cannot_be_merged_recheck`, RS:140-175).
- anonymous (public project) / `rep`: `merge_status:"cannot_be_merged_recheck"`, `has_conflicts:false` (`cannot_be_merged?` is false in that state), `detailed_merge_status:"conflict"` (conflict check returns `checking`, payload identifier `conflict`). Repeated reads do not change it.
- `dev` first GET: `merge_status:"checking"` (state moved to `cannot_be_merged_rechecking` before print), `detailed_merge_status:"checking"`, `has_conflicts:false`; seconds later `cannot_be_merged` / `conflict` / `true`.
- Same-state for `unchecked` after a push: anonymous/reporter see `"unchecked"` + `detailed "unchecked"`; dev sees `"checking"`/`"checking"`.
List endpoints never recheck unless `with_merge_status_recheck=true` (and then still only for Developer+). `GET /merge_requests/:iid` as `rep` author: still no recheck (project-level ability).
Confidence: high (logic), medium (exact timing).

### E4. What triggers rechecks (state -> next state)
- Push to source or target branch of an OPEN MR: `can_be_merged|checking|preparing -> unchecked`; `cannot_be_merged|rechecking -> cannot_be_merged_recheck` (RS:140-175).
- `PUT target_branch` / reopen: `mark_as_unchecked` (US:48-52, reopen_service.rb:15).
- `PUT` title/description/labels/assignee: NO recheck. Closing/merging: no recheck; stored status stays.
- Pushes touching closed/merged MRs: nothing (RS selects `opened`).
- Developer GET/PUT-response entity render, `with_merge_status_recheck=true` list, `MergeRequest#mergeable?` inside `PUT /merge` (sync, with lease retry).
- Create: `unchecked` -> `mark_as_preparing` (`create_service.rb:34`) -> response `merge_status:"checking"` (preparing prints as checking), `detailed_merge_status:"preparing"`.
Confidence: high.

### E5. detailed_merge_status after a failed pipeline vs running (setting on)
With `only_allow_merge_if_pipeline_succeeds=true` and no CI config: `ci_must_pass` check: no pipeline -> `mergeable_ci_state?` ... pipeline required but absent -> `detailed_merge_status:"ci_must_pass"`; merge 405.
With a running pipeline: `ci_still_running` (`diff_head_pipeline_considered_in_progress?`); with Draft too: `draft_status` first (ci is evaluated last). (Pipelines are out of scope for the labrat MVP unless CI exists; included for ordering.) Confidence: medium.

---------------------------------------------------------------------------------------------------

## F. Misc API response quirks in lifecycle responses

### F1. closed_by / closed_at / reopen
Close by `dev`: `closed_by` = dev UserBasic, `closed_at` ISO ms. Reopen: both reset to null (`MergeRequestMetricsService#reopen`). Re-close by another user overwrites. `merged_by`/`merge_user` unaffected. `updated_at` changes on every close/reopen. Confidence: high.

### F2. Closing a locked/auto-merge-armed MR
Close with auto-merge armed: `abort_auto_merge('merge request was closed')` -> system note "aborted the automatic merge because merge request was closed", `merge_when_pipeline_succeeds:false`, `merge_user:null`.
Reopening does not re-arm. Confidence: medium.

### F3. Delete MR then recreate: iid not reused
`DELETE /projects/1/merge_requests/1` (owner) -> 204; `MR(feature, main, t)` -> `iid:2` (internal_ids is a counter; MR doc 9.1). Non-owner `dev` -> 403. Deleting merged MR is allowed. Confidence: high.

### F4. Note: PUT returns the entity even for ignored changes
Any PUT with accepted-but-ignored fields (labels as Reporter author, `squash`/`milestone_id` unknown, `state_event:"close"` on merged) returns **200** and the full entity; `updated_at` is not bumped when nothing changed (no `touch`).
Confidence: medium-high.

---------------------------------------------------------------------------------------------------

## G. List / filter / pagination edge cases (`GET /projects/1/merge_requests`)

Setup G0: MR1 (labels bug,ui; title "alpha"), MR2 (label bug; title "Alpha"), MR3 (no labels; title "beta"; description "needle in text"), MR4 closed (label ui; title "alpha").

### G1. labels=None / Any / mixed / unknown / empty
- `labels=None` -> MR3 only (case-insensitive `none`; `Issuables::LabelFilter` `FILTER_NONE='none'`, `issuable_finder/params.rb:8-9`; filter_by_no_label? uses `downcase`). `labels=any` / `Any` -> MR1, MR2, MR4 (state default `all`).
- `labels=None,bug` -> `None` wins (any element equal to none disables the others).
- `labels=bug,ui` -> AND: MR1 only. `labels=bug,ghost` (unknown name) -> **200 `[]`** (`issuables.none` when fewer labels found than requested, label_filter.rb `issuables_with_selected_labels`).
- `labels=` (empty) -> coerced `[]` -> filter ignored, all MRs returned. `labels=%20bug%20` -> stripped -> matches `bug`.
- A real label literally named `None`/`Any` cannot be filtered by (sentinel wins).
Confidence: high.

### G2. Combined negations `not[...]`
- `not[labels]=bug,ui` -> excludes MRs having ANY of the listed labels (not "has both"): returns MR3 only (label_filter `issuables_without_selected_labels`, `exists.not` over the union).
- `labels=bug&not[labels]=ui` -> MR2 only.
- `not[labels]=ghost` -> unknown label ids empty -> returns everything (`return issuables if label_ids.empty?`).
- Unknown keys inside `not[...]` (e.g. `not[state]`) are not declared params and are ignored: **200**, unfiltered (medium).
- `not[author_id]=<id>&not[assignee_id]=...` AND together. `not[draft]`/`wip=no`: `wip` values must be yes/no else 400 `{"error":"wip does not have a valid value"}`.
Confidence: high for labels, medium for unknown keys.

### G3. search + `in`
- `search=alpha` (default sort -> CTE path, `use_minimum_char_limit=false`): ILIKE `%alpha%` on title OR description, case-insensitive -> MR1, MR2, MR4.
- Query split on whitespace into AND terms (`Gitlab::SQL::Pattern.split_query_to_search_terms`, pattern.rb:85-92): `search=needle%20text` matches MR3 (description) but `search=alpha%20needle` matches nothing (both words must hit the same column; each column evaluated separately and OR'ed, so a title/description split across columns does NOT match). Quoted phrase `"needle in"` kept as one term.
- `in=title` -> description matches excluded (MR3 not found for `needle`); `in=description,title` ok; `in=bogus` -> intersect with SEARCHABLE_FIELDS is empty -> falls back to BOTH columns (no 400). `in=` empty -> both.
- Short terms with a non-simple sort: `search=al&order_by=title` -> not CTE -> `use_minimum_char_limit` true -> term shorter than 3 chars matches only as exact ILIKE without wildcards (so `al` finds nothing), whereas with default order_by=created_at `al` finds MR1/MR2/MR4.
- `%` and `_` in `search` are escaped (`sanitize_sql_like`).
Confidence: medium-high.

### G4. iids[] with pagination
`GET ...?iids[]=1&iids[]=3&per_page=1&page=2` -> filter first, then paginate: page 2 = second element of the filtered, ordered set. Default order is `created_at desc, id desc`, so page 1 = MR3, page 2 = MR1. Headers: `X-Total:2`, `X-Total-Pages:2`, `X-Page:2`, `X-Next-Page:""`, `X-Prev-Page:1`.
`iids[]=999` -> `[]` with `X-Total:0`. `iids[]=1&iids[]=1` dedup naturally. `iids=1` (non-array) -> Grape coerces to [1]. Non-integer `iids[]=a` -> 400 `{"error":"iids[0] is invalid"}`. Confidence: medium-high.

### G5. order_by=title ties
`order_by=title&sort=asc`: sorted by `LOWER(title) ASC` then `id DESC` regardless of requested direction (`order_title_asc.with_order_id_desc`, issuable.rb:359-366; `sortable_title.rb:7`).
Result: the three "alpha"/"Alpha" MRs (MR1, MR2, MR4) tie on lower-case title and come back **MR4, MR2, MR1** (id desc), then MR3. With `sort=desc` the group order is MR3 first, then still MR4, MR2, MR1.
`order_by=updated_at|created_at` ties also break by id desc. Collation of `LOWER()` follows the DB locale (en_US vs C): digits/punctuation/unicode ordering can differ from a naive Rust sort; use the oracle.
Confidence: high (tie rule), medium (locale ordering).

### G6. state / scope defaults
`state` default is `all` for the project endpoint (HLP). `scope` has default `created_by_me` ONLY on the global `/merge_requests` endpoint; project and group lists default to everything. `state=locked` returns `[]` normally.
`GET /merge_requests` (global) without scope as `owner` returns only own MRs. Confidence: high (see HLP `optional_scope_param`).

---------------------------------------------------------------------------------------------------

## H. Implementation checklist for labrat derived from the above

1. Error body typing: model-validation -> hash; branch_conflict/validate_fork/base/project_access -> 422 array; validate_branches -> 409 array; Grape-level -> `{"error":...}`.
2. Order PUT work exactly as ISB: state change commits first, remaining attributes second (B6).
3. Reopen failing validation is silent 200 (C3); reopen under discussion_locked is silent 200 (C4); close/reopen on merged is silent 200 (C2).
4. `PUT /merge`: 401 -> 409(sha) -> squash side effect -> 405 -> 422 (D1/D4).
5. Do not validate branch existence at create (A6) but do validate branch names on change; no existence validation at all on update (B4).
6. `detailed_merge_status` preempt order preparing/checking/unchecked before not_open (B5, E1); `has_conflicts` is false in `*_recheck` states; reporter vs developer reads differ (E3).
7. Labels: trim, uniq, create only with `admin_label`; unknown labels in filters yield empty list for AND, ignored for `not` (A10, G1, G2).
8. Title: strip before validate; length in characters; description in bytes with the odd "1 MB ... 1 MB" message (A3, A4).
