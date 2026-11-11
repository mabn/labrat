# MR-related REST API error responses (GitLab 18.11 CE) vs. conformance fixtures

Source: /home/mabn/dev/gitlab-run-ce/gitlab-src (static reading only; nothing was executed).
Fixtures: /home/mabn/dev/shed/labrat/tests/conformance/fixtures/*.json. Coverage was determined by scanning every
fixture step with status >= 400 (plus 3xx) for the message text and request shape. "Fixture:label" cites file (no .json) and step label.

Conventions: `render_api_error!(msg, st)` -> `{"message": msg}` (lib/api/helpers.rb:676). Grape param validation -> `{"error": "..."}` 400.
`authorize!` -> 403 `{"message":"403 Forbidden"}`. `bad_request!(r)` -> 400 `{"message":"400 Bad request - r"}`.
`not_found!(x)` -> 404 `{"message":"404 x Not Found"}`. `ActiveRecord::RecordNotFound` -> 404 `{"message":"404 Not found"}` (lib/api/api.rb:134).
`render_validation_error!` -> 400 `{"message":{attr:[...]}}`.

## A. Generic / cross-cutting

| # | Status / body | Code path | Trigger | Coverage |
|---|---|---|---|---|
| A1 | 400 `{"error":"<param> is missing"}` / `is empty` / `does not have a valid value` / `is invalid` / `are mutually exclusive` | Grape validators | missing/blank/enum/int-typing/exactly_one_of | COVERED: merge_request_lifecycle:"missing title"; mr_create_update_edges:"empty title on update"; mr_list_filters:"list state=bogus"; review_discussions:"body and resolved","neither"; rest_params:"position line not a number". NOT COVERED for `state_event` bad value, non-integer `merge_request_iid` ("merge_request_iid is invalid"), `name is missing` on award_emoji, `note is missing` on draft_notes, `body is missing` on MR notes |
| A2 | 404 `{"message":"404 Not found"}` (unknown MR iid / note id / draft id / award id) | RecordNotFound; merge_requests.rb `find_project_merge_request`, notes_helpers.rb:44,80,91 | GET/PUT/POST on `/merge_requests/99...` | PARTIAL: todo_mutations:"missing mr todo"; review_discussions:"edited note gone","award unknown note","delete again". NOT COVERED for MR GET/PUT/merge/notes/discussions/approve/draft_notes on a nonexistent iid, and for draft id of another user |
| A3 | 404 `{"message":"404 MergeRequest Not Found"}` | notes_helpers.rb:123 `find_noteable` (noteable unreadable) | notes/discussions on an MR the user cannot read (rare: private project, nonmember gets project 404 first) | NOT COVERED (low value) |
| A4 | 401 `{"message":"401 Unauthorized"}` (no/invalid token) | `authenticate!` | any call without token | COVERED generically (e.g. audit_followups:"anonymous project search"); not specifically on MR notes |
| A5 | 429 `{"message":{"error":"This endpoint has been requested too many times. Try again later."}}` + Retry-After | notes.rb `check_rate_limit! :notes_create` (300/min default), api.rb:188 | >300 note POSTs/min | NOT COVERED (impractical) |
| A6 | 408 `{"message":{"error":"Request timed out"}}` | merge_requests.rb:23 | DB timeout | NOT COVERED (not reproducible) |
| A7 | 412 `{"message":"412 Precondition Failed"}` | helpers.rb:47 `check_unmodified_since!` | DELETE MR/note/award with stale `If-Unmodified-Since` | NOT COVERED |
| A8 | 409 `{"message":"409 Conflict: Resource lock"}` | api.rb:142 | lock contention | NOT COVERED (not reproducible) |

## B. Merge request CRUD / lifecycle (lib/api/merge_requests.rb)

| # | Status / body | Code path | Trigger | Coverage |
|---|---|---|---|---|
| B1 | 400 `{"message":{"title":["can't be blank"]}}` | create: handle_merge_request_errors! (merge_requests_helpers.rb:126) | `title:""` | COVERED mr_create_update_edges:"blank title" |
| B2 | 400 `{"message":{"title":["is too long (maximum is 255 characters)"]}}` | Issuable title length | title 256 chars | COVERED mr_create_update_edges:"title too long" |
| B3 | 400 `{"message":{"source_branch":["is invalid"]}}` / target_branch | merge_request.rb:1383 validate_branch_name | ref-invalid name e.g. `a..b` | COVERED mr_create_update_edges:"invalid branch","both branches invalid","invalid target on update" |
| B4 | 422 `{"message":["You can't use same project/branch for source and target"]}` | merge_request.rb:1358 (:branch_conflict -> unprocessable_entity!) | source == target | COVERED mr_create_update_edges:"same source and target" |
| B5 | 409 `{"message":["Another open merge request already exists for this source branch: !N"]}` | merge_request.rb:1368 (:validate_branches -> conflict!) | duplicate open MR | COVERED merge_request_lifecycle:"duplicate mr" |
| B6 | 422 `{"message":["Source project is not a fork of the target project"]}` | merge_request.rb:1417 (:validate_fork) | `target_project_id` of non-parent project | NOT COVERED |
| B7 | 422 `{"message":["Target project has disabled merge requests"]}` (key :base) | merge_request.rb:1409 | project `merge_requests_access_level=disabled`, POST MR | NOT COVERED |
| B8 | 403 `{"message":"403 Forbidden"}` on create | merge_requests.rb:373, create_service.rb:76 | Guest / archived project / no access to `target_project_id` | COVERED archived_readonly:"new mr"; NOT COVERED for Guest/Reporter-without-MR-feature, or target_project_id inaccessible (404 `Project.find` -> "404 Not found") |
| B9 | 400 `{"message":{"reviewers":["total must be less than or equal to 200"]}}` | merge_request.rb:1423 | >200 reviewer_ids | NOT COVERED (impractical) |
| B10 | 400 `{"message":{"description":["is too long (1.1 MiB). The maximum size is 1 MiB."]}}` | issuable.rb:84 bytesize validator | description > `description_and_note_max_size` | NOT COVERED |
| B11 | 400 `{"error":"assignee_id, assignee_ids, reviewer_ids, ... are missing, at least one parameter must be provided"}` | `at_least_one_of` (merge_requests.rb update) | PUT with no updatable fields | COVERED mr_create_update_edges:"no params" |
| B12 | 400 `{"error":"title is empty"}` | `allow_blank: false` | PUT title:"" | COVERED mr_create_update_edges:"empty title on update" |
| B13 | 400 `{"error":"state_event does not have a valid value"}` | `values: %w[close reopen]` | PUT state_event:"merge" | NOT COVERED |
| B14 | 403 PUT update | merge_requests.rb:756 find_merge_request_with_access(:update_merge_request) | Reporter/Guest | COVERED mr_create_update_edges:"reporter cannot update"; archived_readonly:"bob update" |
| B15 | 400 `{"message":{"assignees":["Cannot assign @x to !N"]}}`-style (update_service.rb:368, attr :assignees/:reviewers) | UpdateService new_user_ids -> render_validation_error | assignee_ids/reviewer_ids of a user who cannot read MR (non-member on private project) | NOT COVERED (fixture "unknown assignee" returned 200 silently) |
| B16 | 403 `{"message":"403 Forbidden - <reason>"}` quick-action not allowed | merge_requests.rb:388,773 | scope_validator (ai_workflows tokens only) | NOT COVERED (out of scope) |
| B17 | 403 DELETE | merge_requests.rb:411 | Developer | COVERED mr_create_update_edges:"developer delete" |
| B18 | 405 `{"message":"405 Method Not Allowed"}` on merge (not mergeable: draft, closed, merged, no changes, CI missing, needs rebase, conflicts, unresolved discussions, ...) | merge_requests.rb:841,850 | PUT /merge while `mergeable?` false | COVERED for draft/closed/merged/empty/conflict/needs-rebase/pipeline: merge_request_lifecycle:"merge draft"; mr_create_update_edges:"merge closed"; mr_merge_edges:"merge merged","merge empty"; merge_request_close_reopen_conflict:"merge conflict"; merge_request_fast_forward:"merge needs rebase"; commit_statuses:"merge blocked". NOT COVERED: blocked by unresolved discussions (mergeability_checks has no merge call) |
| B19 | 401 `{"message":"401 Unauthorized"}` on merge | merge_requests.rb:816 | Reporter/Guest/bob cannot push to target | COVERED mr_create_update_edges:"reporter merge"; archived_readonly:"admin merge" |
| B20 | 409 `{"message":"SHA does not match HEAD of source branch: <sha>"}` | helpers.rb:600 check_sha_param! | merge/approve with stale `sha` | COVERED merge_request_lifecycle:"wrong sha"; mr_merge_edges:"merge wrong sha"; review_drafts_approvals:"approve stale sha" |
| B21 | 422 `{"message":"Branch cannot be merged"}` | merge_requests.rb:833,839,848 | merge service ran but MR not merged (e.g. squash not allowed) | COVERED mr_merge_edges:"merge without squash"; random_walk_merge_2:"69 mr1 op6" |
| B22 | 405 on auto-merge request when strategy unavailable and pipeline not allowing | merge_requests.rb:841 | `auto_merge:true` with no CI | COVERED mr_merge_edges:"auto merge without ci" |
| B23 | cancel_merge_when_pipeline_succeeds: 401 `{"message":"401 Unauthorized"}` (merge_requests.rb:891: neither can-merge nor author); service failure returns ServiceResponse hash (auto_merge_service.rb:55 "Can't cancel the automatic merge", http_status 406 -- the endpoint does not render it, so likely 201 with `{"status":"error","message":"Can't cancel the automatic merge","http_status":406}`; verify on real instance) | merge_requests.rb:888-894 | POST cancel when no auto-merge enabled / by Reporter | NOT COVERED |
| B24 | GET merge_ref: 400 `{"message":"<mergeability message>"}` (e.g. conflicts) | merge_requests.rb:872 | conflicting MR | NOT COVERED (only 200s in rest_params / mergeability_checks) |
| B25 | rebase: 403 `{"message":"403 Forbidden - Cannot push to source branch"}` / `- Source branch does not exist` / `- Source branch is protected from force push`; 409 `{"message":"<RebaseLockTimeout msg>"}` | merge_requests.rb:173,919; rebase_service.rb:10-17 | Reporter / deleted branch / protected branch w/o force push | COVERED first two: random_walk_hooks_4:"00 carol mr1 op18","45 carol mr1 op18". NOT COVERED: protected-branch variant, 409 |
| B26 | POST /pipelines: 400 `{"message":{"base":["The pipeline did not run. Review the workflow:rules configuration for the pipeline."]}}`; 405 (nil payload) | merge_requests.rb:722,727 | no CI config | COVERED mr_context_commits:"create mr pipeline"; 405 NOT COVERED |
| B27 | context_commits: 400 size>20, already created, invalid sha, empty/missing | merge_requests.rb:554,567,597 | | COVERED (audit_followups_2, mr_context_commits) |
| B28 | list filters 400 | Grape | bogus enum | COVERED mr_list_filters |

## C. Notes (lib/api/notes.rb, helpers/notes_helpers.rb)

| # | Status / body | Code path | Trigger | Coverage |
|---|---|---|---|---|
| C1 | 400 `{"message":"400 Bad request - Note {:note=>[\"can't be blank\"]}"}` | notes_helpers.rb:172 | body:"" / whitespace | COVERED review_flow:"empty note" |
| C2 | 400 `{"message":"400 Bad request - Note {:note=>[\"is too long (N MiB). The maximum size is 1 MiB.\"]}"}` | notes/active_record.rb:16 | body > 1 MiB | NOT COVERED |
| C3 | 403 create note | notes_helpers.rb:153 authorize!(:create_note) | locked discussion by non-member; archived; Guest where not allowed | COVERED review_discussions:"outsider note locked"; archived_readonly:"bob comment" |
| C4 | 400 `{"message":"400 Bad request - Failed to apply commands."}` | create_service.rb:~135; notes_helpers.rb:176 | commands-only note where command fails (`/label ~zzz`, `/title`, `/relabel`, bad `/react`, `/copy_metadata`) | COVERED quick_action_audit (08,10,16,22,23,32) |
| C5 | 400 `{"message":"400 Bad request - <quick action update error>"}` / `Note {:validation=>[...]}` | create_service.rb:~126 | `/assign` or `/reviewer` of >200 users, etc. | NOT COVERED (impractical) |
| C6 | 400 `{"message":"400 Bad request - Note {:base=>[\"Maximum number of comments exceeded\"]}"}` | note.rb:861 | >5000 notes | NOT COVERED (impractical) |
| C7 | 400 `{"error":"body is empty"}` | PUT note `allow_blank:false` | | COVERED review_discussions:"edit blank" |
| C8 | 403 PUT/DELETE note | notes_helpers.rb:45,81 authorize!(:admin_note) | non-author Developer editing/deleting another user's note; ANY user editing/deleting a system note (note_policy.rb `~editable`) | NOT COVERED |
| C9 | 404 `{"message":"404 Note Not Found"}` | notes_helpers.rb:97 get_note | Guest GET of an internal (confidential) note | NOT COVERED (awards/discussion variants covered) |
| C10 | 404 `{"message":"404 Not found"}` | RecordNotFound | note id not on this MR | COVERED review_discussions:"edited note gone" |
| C11 | PUT note with `body` identical/unchanged etc. | | | n/a (200) |

## D. Discussions (lib/api/discussions.rb)

| # | Status / body | Code path | Trigger | Coverage |
|---|---|---|---|---|
| D1 | 400 `{"message":"400 Bad request - Note {:line_code=>[\"can't be blank\", \"must be a valid line code\"]}"}` | diff_note.rb:19 | position not on a diff line | COVERED review_positions (many), review_flow:"diff note bad line" |
| D2 | 400 `{"message":"400 Bad request - Note {:position=>[\"is incomplete\"]}"}` | diff_note.rb:212 | file/image position missing coords | COVERED review_positions:"file position one path","image incomplete" |
| D3 | 400 `{"error":"position[head_sha] is missing"}` / `position[new_line] is invalid` / position_type enum | Grape | | COVERED review_positions:"missing head sha"; rest_params:"position line not a number"; NOT COVERED for position_type bad value |
| D4 | 404 `{"message":"404 Discussion Not Found"}` | discussions.rb:77,187 | unknown id, or note id used as discussion id, or internal thread as Guest | COVERED review_discussions:"reply to note id","guest internal thread","guest reply internal" |
| D5 | 404 `{"message":"404 Notes Not Found"}` | discussions.rb:164 | GET .../discussions/<unknown>/notes | NOT COVERED |
| D6 | 400 `{"message":"400 Bad request - Replies to system notes are not allowed."}` | discussions.rb:189 | reply to a discussion whose first note is a system note | NOT COVERED |
| D7 | 400 `{"message":"400 Bad request - Discussion can not be replied to."}` | discussions.rb:192 | reply to a non-convertible individual note (commit notes) | COVERED for commits only (audit_followups:"reply to plain"); MR individual notes ARE convertible, so no MR trigger |
| D8 | 403 reply in locked thread | create_note authorize | | COVERED review_discussions:"outsider reply locked" |
| D9 | 400 `{"error":"body, resolved are mutually exclusive"}` / `... are missing, exactly one parameter must be provided` | exactly_one_of | | COVERED review_discussions |
| D10 | 400 `{"message":"400 Bad request - Note is not resolvable"}` | notes_helpers.rb:66 | PUT discussions/:id/notes/:nid resolved on non-resolvable note | COVERED review_discussions:"resolve individual note" |
| D11 | 403 resolve discussion | notes_helpers.rb:64,192 | Guest/Reporter/outsider; non-resolvable discussion | COVERED review_discussions:"resolve individual discussion"; review_permissions |
| D12 | 404 `{"message":"404 Discussion Not Found"}` on resolve of unknown discussion | notes_helpers.rb:191 | PUT discussions/<deadbeef> resolved=true | NOT COVERED on MR (commit variant only: commit_discussions:"resolve on commit" gives different `{"error":"404 Not Found"}`) |
| D13 | 403 resolve in locked MR by non-member (policy resolve_note) | note_policy.rb | | COVERED-ish review_permissions:"outsider resolve" |

## E. Draft notes (lib/api/draft_notes.rb)

| # | Status / body | Code path | Trigger | Coverage |
|---|---|---|---|---|
| E1 | 400 `{"message":{"base":["Thread to reply to cannot be found"]}}` | draft_notes/create_service.rb:15 | in_reply_to_discussion_id unknown | COVERED review_drafts_approvals:"draft reply after delete" |
| E2 | 400 `{"message":{"base":["Replies to system notes are not allowed"]}}` | create_service.rb:19 | reply to system-note discussion | NOT COVERED |
| E3 | 400 `{"message":{"base":["User is not allowed to resolve thread"]}}` | create_service.rb:26 | resolve_discussion:true w/o thread | COVERED review_drafts_approvals:"draft resolve without thread" |
| E4 | 400 `{"message":{"author_id":["has already been taken"]}}` | draft_note.rb:29 | second draft reply in same thread | COVERED review_drafts_approvals:"second reply same thread" |
| E5 | 400 `{"message":"400 Bad request - Missing params to modify"}` | draft_notes.rb:185 | PUT without note | COVERED review_drafts_approvals:"update without note" |
| E6 | 400 `{"error":"note is missing"}` (create); `{"error":"note is empty"}` (PUT note:"") | Grape | | NOT COVERED |
| E7 | 404 `{"message":"404 Not found"}` | `.find` in get_draft_note (the `not_found!("Draft Note")` branches are dead code) | GET/PUT/DELETE/publish of unknown draft id or other user's draft | NOT COVERED |
| E8 | 400 position errors via render_validation_error (e.g. `{"message":{"base":...}}`) | | draft with malformed position | NOT COVERED |
| E9 | 500 empty body | draft_notes.rb:244,271 | publish fails (cannot create_note e.g. locked MR for non-member / RecordInvalid) | NOT COVERED |
| E10 | 403 on draft create when user cannot create notes | draft_notes.rb:39 `access_denied!` -> `{"message":"403 Forbidden"}` | locked MR non-member, archived | NOT COVERED |

## F. Approvals (lib/api/merge_request_approvals.rb)

| # | Status / body | Code path | Trigger | Coverage |
|---|---|---|---|---|
| F1 | 401 `{"message":"401 Unauthorized"}` | approvals.rb:81 `unauthorized! unless success` (ApprovalService returns nil when not eligible or merged) | approve twice; author/ineligible; **approve a merged MR**; approve a closed MR (eligible_for_approval_by? false) | COVERED for repeat approve (review_flow:"approve again", review_drafts_approvals:"approve again rest"); NOT COVERED explicitly for merged / closed MR |
| F2 | 403 `{"message":"403 Forbidden"}` | find_merge_request_with_access(:approve_merge_request) | Guest/Reporter/outsider | COVERED review_permissions, review_drafts_approvals:"reporter approves" |
| F3 | 404 `{"message":"404 Not Found"}` on unapprove without own approval (also unapprove merged MR) | approvals.rb:100 | | COVERED review_flow:"unapprove again"; NOT COVERED on merged MR |
| F4 | 401 reset_approvals | approvals.rb:121 | Developer/no permission, or MR merged | NOT COVERED (neither the developer nor the merged variant) |

## G. Suggestions (lib/api/suggestions.rb, lib/gitlab/suggestions/suggestion_set.rb, suggestion.rb:52)

All are 400 `{"message":"<text>"}` unless noted.

| # | Message | Trigger | Coverage |
|---|---|---|---|
| G1 | 404 "Suggestion is not applicable as the suggestion was not found." | unknown id | COVERED review_suggestions:"apply old id" |
| G2 | 404 "Suggestions are not applicable as one or more suggestions were not found." | batch with unknown id | COVERED review_suggestions:"batch unknown" |
| G3 | 403 `403 Forbidden` | Guest/Reporter/outsider | COVERED review_permissions, review_suggestions:"apply by reporter" |
| G4 | "Can't apply this suggestion." | already applied | COVERED review_suggestions:"apply applied" |
| G5 | "This suggestion already matches its content." | to_content == from_content | COVERED review_suggestions:"apply identical" |
| G6 | "Suggestions are not applicable as their lines cannot overlap." | batch overlap | COVERED review_suggestions:"batch overlap" |
| G7 | "This merge request was merged. To apply this suggestion, edit this file directly." | MR merged | NOT COVERED |
| G8 | "This merge request is closed. To apply this suggestion, edit this file directly." | MR closed | NOT COVERED |
| G9 | "Can't apply as the source branch was deleted." | source branch deleted | NOT COVERED |
| G10 | "Can't apply as this line was changed in a more recent version." / "...these lines were changed..." | new push altered the lines (note outdated) | NOT COVERED |
| G11 | "A file was not found." / "A file has been changed." / "Suggestions must all be on the same branch." / ipynb message | rare | NOT COVERED (G11 "A file has been changed." needs a push racing; skip) |
| G12 | `{"error":"ids is missing"}` | batch_apply without ids | NOT COVERED |

## H. Award emoji (lib/api/award_emoji.rb; AwardEmojis::AddService)

| # | Status / body | Trigger | Coverage |
|---|---|---|---|
| H1 | 404 `404 Award Emoji Name has already been taken Not Found` | duplicate | COVERED review_flow:"award dup" |
| H2 | 404 `404 Award Emoji Name is not a valid emoji name Not Found` | bad name/upper case | COVERED review_flow:"award invalid"; review_discussions:"upper case" |
| H3 | 404 `404 Award Emoji Awardable cannot add emoji reactions Not Found` | system note | COVERED review_discussions:"award system note" |
| H4 | 404 `404 Award Emoji Not Found` | cannot read / locked / archived | COVERED archived_readonly, review_discussions:"outsider award locked","award internal as guest" |
| H5 | 401 delete other user's award | COVERED review_flow:"delete others award" |
| H6 | 404 `404 Not found` delete/get unknown award | COVERED review_discussions:"delete again" |
| H7 | 400 `{"error":"name is missing"}` | POST without name | NOT COVERED |
| H8 | 404 `404 Award Emoji Name can't be blank Not Found` (name:"") | NOT COVERED (Grape: `requires :name` accepts empty string, model presence fails; verify) |

## I. Todos / events

| # | Status / body | Coverage |
|---|---|---|
| I1 | 304 empty on re-adding todo | COVERED todo_mutations:"bob marks again" |
| I2 | 404 `404 Not found` for todo on unknown MR | COVERED todo_mutations:"missing mr todo" |
| I3 | 400 `{"error":"action does not have a valid value"}` (state/type similarly) | action COVERED todo_lifecycle:"bad action"; state/type NOT COVERED |
| I4 | 404 `404 Not found` POST /todos/:id/mark_as_done for unknown/other user's todo (todos.rb:129 `current_user.todos.find`) | NOT COVERED |
| I5 | resource_label_events/:id unknown: 404 `404 Not found`; resource_state_events/:id unknown (404 `404 Not found` RecordNotFound/finder); resource_milestone_events/:id unknown (same) | label COVERED milestones_and_events:"missing label event"; state & milestone NOT COVERED |

---

# NOT COVERED -- probe list (grouped into scenario-sized batches)

All paths relative to /api/v4; `P` = project id; `N` = MR iid. Expected bodies come from the source reading above and should be treated as hypotheses until recorded.
Users: owner/maintainer `M`, developer `D` (non-author), author `A` (Developer who opened the MR), reporter `R`, guest `G`, outsider `O`.

## Scenario 1: notes & discussions on an MR with a system note (highest value for bots)
Setup: MR N opened by A with a commit; A comments (note n1); D posts a thread (discussion d1, with reply); M approves (creates a system note "approved this merge request"); fetch `GET /projects/P/merge_requests/N/discussions` to find the system-note discussion `sysd` (individual_note, first note system) and the system note id `sysn`.
1. `POST /projects/P/merge_requests/N/discussions/<sysd>/notes` body `{"body":"reply"}` as D -> 400 `{"message":"400 Bad request - Replies to system notes are not allowed."}` (D6)
2. `PUT /projects/P/merge_requests/N/notes/<sysn>` `{"body":"x"}` as M and as D -> 403 `{"message":"403 Forbidden"}` (C8)
3. `DELETE /projects/P/merge_requests/N/notes/<sysn>` as M -> 403 (C8)
4. `PUT /projects/P/merge_requests/N/notes/<n1>` `{"body":"x"}` as D (not author, Developer) -> 403 (C8)
5. `DELETE /projects/P/merge_requests/N/notes/<n1>` as D -> 403 (C8)
6. `GET /projects/P/merge_requests/N/discussions/deadbeef/notes` as D -> 404 `404 Notes Not Found` (D5)
7. `PUT /projects/P/merge_requests/N/discussions/deadbeef` `{"resolved":true}` as D -> 404 `404 Discussion Not Found` (D12)
8. Internal note: M posts `POST .../notes {"body":"secret","internal":true}` -> id `inn`; as G `GET /projects/P/merge_requests/N/notes/<inn>` -> 404 `404 Note Not Found` (C9)
9. `POST /projects/P/merge_requests/N/notes` `{"body":"<1 MiB+1 bytes of 'a'>"}` -> 400 `Note {:note=>["is too long (1 MiB). The maximum size is 1 MiB."]}` (C2) (record exact size text)
10. `POST /projects/P/merge_requests/N/notes` `{}` -> 400 `{"error":"body is missing"}` (A1)
11. `POST /projects/P/merge_requests/N/discussions/<d1>/notes` `{}` -> 400 `{"error":"body is missing"}`
12. Nonexistent MR: `GET /projects/P/merge_requests/9999/notes`, `POST .../9999/notes {"body":"x"}`, `GET .../9999/discussions`, `POST .../9999/approve`, `PUT .../9999/merge`, `GET .../9999`, `PUT .../9999 {"title":"x"}` -> all 404 `{"message":"404 Not found"}` (A2)
13. Non-integer iid: `GET /projects/P/merge_requests/abc/notes` -> 404 (route requirement) or 400 `merge_request_iid is invalid`; record which (A1)

## Scenario 2: draft notes
Setup: D has an MR N with discussion `sysd` (system-note thread, from Scenario 1) and another user's draft id `othd` (created by M).
1. `POST /projects/P/merge_requests/N/draft_notes` `{"note":"x","in_reply_to_discussion_id":"<sysd>"}` as D -> 400 `{"message":{"base":["Replies to system notes are not allowed"]}}` (E2)
2. `POST .../draft_notes` `{}` -> 400 `{"error":"note is missing"}` (E6)
3. `PUT .../draft_notes/<own draft>` `{"note":""}` -> 400 `{"error":"note is empty"}` (E6)
4. `GET`, `PUT {"note":"x"}`, `DELETE`, `PUT .../publish` on `.../draft_notes/<othd>` as D -> 404 `{"message":"404 Not found"}` each (E7)
5. `GET .../draft_notes/999999` -> 404 `404 Not found` (E7)
6. Locked MR (`PUT .../N {"discussion_locked":true}`), as O (non-member, public project): `POST .../draft_notes {"note":"x"}` -> 403 (E10); and publish -> record status (E9)
7. Malformed position: `POST .../draft_notes` `{"note":"x","position":{"base_sha":"<sha>","start_sha":"<sha>","head_sha":"<sha>","position_type":"text","new_path":"nope.txt","new_line":1}}` -> record (E8; may succeed since DraftNote does not validate line_code)

## Scenario 3: approvals on merged/closed MRs
Setup: MR N1 merged (after M approved); MR N2 closed; D approved N2 earlier.
1. `POST /projects/P/merge_requests/N1/approve` as M2 (eligible reviewer) -> 401 `401 Unauthorized` (F1)
2. `POST /projects/P/merge_requests/N1/unapprove` as M (who approved) -> 404 `404 Not Found` (F3)
3. `PUT /projects/P/merge_requests/N1/reset_approvals` as M -> 401 (F4 merged variant)
4. `POST /projects/P/merge_requests/N2/approve` (closed) as eligible user -> record (F1, expected 401)
5. `POST /projects/P/merge_requests/N/cancel_merge_when_pipeline_succeeds` with no auto-merge set, as M -> record status/body (B23; hypothesis 201 + hash with `http_status` 406); as R (not author, can't merge) -> 401 (B23)

## Scenario 4: suggestions on merged/closed/deleted/outdated
Setup: four separate MRs each with a suggestion note (```suggestion fenced block on a diff line):
 S_m: MR then merged; S_c: MR then closed; S_d: source branch deleted via `DELETE /projects/P/repository/branches/<src>` (MR stays open); S_o: push another commit changing the suggested line.
1. `PUT /suggestions/<S_m>/apply` as M -> 400 `This merge request was merged. To apply this suggestion, edit this file directly.` (G7)
2. `PUT /suggestions/<S_c>/apply` -> 400 `This merge request is closed. To apply this suggestion, edit this file directly.` (G8)
3. `PUT /suggestions/<S_d>/apply` -> 400 `Can't apply as the source branch was deleted.` (G9)
4. `PUT /suggestions/<S_o>/apply` -> 400 `Can't apply as this line was changed in a more recent version.` (G10; also try a multi-line suggestion for the "these lines" variant)
5. `PUT /suggestions/batch_apply` `{}` -> 400 `{"error":"ids is missing"}` (G12)
6. `PUT /suggestions/batch_apply` `{"ids":[<S_m>,<S_c>]}` -> record (first-suggestion error wins)

## Scenario 5: MR create/update validations
1. `POST /projects/P/merge_requests` `{"source_branch":"feat","target_branch":"main","title":"t","target_project_id":<unrelated project Q readable by user>}` -> 422 `{"message":["Source project is not a fork of the target project"]}` (B6)
2. Disable MRs: `PUT /projects/P {"merge_requests_access_level":"disabled"}`; `POST /projects/P/merge_requests {...valid...}` -> record (B7; expected 403 from `create_merge_request_from`, or 422 "Target project has disabled merge requests")
3. As G: `POST /projects/P/merge_requests {...valid...}` -> record (B8; expect 403)
4. `POST ... {"target_project_id":999999,...}` -> 404 `{"message":"404 Not found"}` (B8)
5. `POST ... {"description":"<1 MiB+1 'a'>", ...}` -> 400 `{"message":{"description":["is too long (1 MiB). The maximum size is 1 MiB."]}}` (B10)
6. `PUT /projects/P/merge_requests/N {"state_event":"merge"}` -> 400 `{"error":"state_event does not have a valid value"}` (B13)
7. `PUT /projects/P/merge_requests/N {"assignee_ids":[<user without project access>]}` (private project) -> record (B15; expected 400 with `Cannot assign @u to !N` under key assignees)
8. `DELETE /projects/P/merge_requests/N` as owner with header `If-Unmodified-Since: 2000-01-01T00:00:00Z` -> 412 `{"message":"412 Precondition Failed"}` (A7) (and same on DELETE note/award)
9. `PUT /projects/P/merge_requests/N/merge` while a project setting `only_allow_merge_if_all_discussions_are_resolved=true` and an unresolved thread exists -> 405 `405 Method Not Allowed` (B18)
10. `GET /projects/P/merge_requests/N/merge_ref` on a conflicting MR -> 400 `{"message":"<text>"}` (B24)
11. `PUT /projects/P/merge_requests/N/rebase` on MR whose source branch is protected (no force push) as M -> 403 `403 Forbidden - Source branch is protected from force push` (B25)
12. `POST /projects/P/merge_requests/N/pipelines` as R/G -> record (403 expected via authorize)

## Scenario 6: award emoji, todos, events
1. `POST /projects/P/merge_requests/N/award_emoji` `{}` -> 400 `{"error":"name is missing"}` (H7)
2. `POST /projects/P/merge_requests/N/award_emoji` `{"name":""}` -> record (H8)
3. `GET /todos?state=bogus` -> 400 `{"error":"state does not have a valid value"}`; `?type=bogus` likewise (I3)
4. `POST /todos/999999/mark_as_done` -> 404 `{"message":"404 Not found"}` (I4)
5. `GET /projects/P/merge_requests/N/resource_state_events/999999` and `.../resource_milestone_events/999999` -> 404 `404 Not found` (I5)

## Not worth probing
A3, A5 (300 notes/min), A6, A8, B9, B16, C5, C6, G11.
