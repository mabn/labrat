# GitLab 18.11 (CE) - MR review edge cases as reproducible API scenarios

Companion to `notes-discussions-reviews.md` (NDR) and `approvals-reviewers-todos-systemnotes.md` (ARTS). Those files are NOT repeated here; this file only lists
scenarios that are hard to hit by casual use. Source root: `/home/mabn/dev/gitlab-run-ce/gitlab-src`, paths relative to it.
Method: source reading only (no HTTP/docker/rails). Confidence: high = straight code path, medium = needs one assumption, low = guess about Gitaly/Banzai output.
Error body shapes: see NDR header. `NOTE400(x)` below means `400 {"message":"400 Bad request - Note {<ruby inspect of x>}"}` (Ruby 3.3 `{:key=>[...]}` form).

## 0. Common fixture (call it F0)

Users: `O` maintainer, `D` developer, `R` reporter, `G` guest member, `X` logged-in non-member (project is public), `A` instance admin who is NOT a member.
Project `P` (id 1), default branch `main` containing: `README.md` (10 lines L1..L10), `big.txt` (100 lines), `old_name.txt` (3 lines), `pure_old.txt` (3 lines), `gone.txt` (3 lines), `run.sh` (mode 100644).
Branch `feature` (one commit on top of main) changes: `README.md` line 5 only; `big.txt` line 50 only; `old_name.txt` -> `new_name.txt` with line 2 changed (rename + edit);
`pure_old.txt` -> `pure_new.txt` (100% rename); `gone.txt` deleted; `new.txt` added (3 lines); `run.sh` mode -> 100755 (no content change); `logo.png` added (binary).
`POST /api/v4/projects/1/merge_requests {source_branch:feature,target_branch:main,title:T}` -> MR iid 1 (author D unless stated).
`GET /api/v4/projects/1/merge_requests/1/versions` -> `[0]` gives `BASE=base_commit_sha`, `START=start_commit_sha`, `HEAD=head_commit_sha` (V1). Shorthand
`POS(path-fields, line-fields)` = `position:{base_sha:BASE,start_sha:START,head_sha:HEAD,position_type:"text", ...}` (the three Grape `requires` + `position_type`; missing -> 400 `{"error":"position[base_sha] is missing"}`).
Create thread = `POST /projects/1/merge_requests/1/discussions` (discussions.rb:127-158). Diff of README: `@@ -2,7 +2,7 @@` => context old/new 2-4, `-` old 5, `+` new 5, context 6-8.

---------------------------------------------------------------------------------------------------
## A. Diff-note positions

### A1. Renamed file, both paths vs new_path only
1. `POST discussions body=x POS(old_path:"old_name.txt",new_path:"new_name.txt",new_line:2)` (added line in the rename diff) -> 201 Discussion, `notes[0].type="DiffNote"`, `position` echoes input (key order `base_sha,start_sha,head_sha,old_path,new_path,position_type,old_line,new_line,line_range`; `old_line:null`, `line_range:null`), `resolvable:true,resolved:false`.
2. Same with context line `old_line:1,new_line:1` -> 201 (context line inside hunk).
3. Same as (1) but only `new_path:"new_name.txt"` (no old_path) -> expected 201 or `NOTE400({:line_code=>["can't be blank","must be a valid line code"]})`; if 201 the stored response has `old_path:null` (position stored exactly as sent, `update_position` returns early when `active?`, diff_note.rb/diff_positionable_note.rb:62-77).
Why subtle: `file_path = new_path.presence || old_path` (position.rb `file_path`), diff lookup uses `paths=[old_path,new_path].compact` (position.rb `paths`/`diff_options`); with a single path Gitaly's pathspec may not pair the rename and show the file as fully added, so context-line positions then fail. Confidence: (1),(2) high; (3) low (depends on Gitaly rename detection under pathspec).

### A2. Deleted file: only old_path/old_line
`POS(old_path:"gone.txt",old_line:1)` (no new_path, no new_line) -> 201, `position.new_path:null,new_line:null`. line_code = `sha1("gone.txt")_1_0`-style from the removed line (`type old`, text_formatter.rb `line_age`).
Sending `new_line:1` too -> `NOTE400({:line_code=>["can't be blank","must be a valid line code"]})` because `line_for_position` requires `line.old_line==pos.old_line && line.new_line==pos.new_line` (gitlab/diff/file.rb:83-95; removed lines have new_line=nil).
Suggestions in this thread: body with ```` ```suggestion ```` yields `notes[0].suggestions:[]` (removed line not suggestible; line.rb:106, diff_note.rb `supports_suggestion?`). Confidence: high.

### A3. Added file: new_line only; both lines sent
`POS(new_path:"new.txt",new_line:2)` -> 201. `POS(new_path:"new.txt",old_line:2,new_line:2)` -> `NOTE400({:line_code=>[...]})` (no unfold for new files: lines_unfolder.rb:58). Confidence: high.

### A4. Unchanged lines outside any hunk (big.txt, change only at line 50)
1. `POS(old_path:"big.txt",new_path:"big.txt",old_line:20,new_line:20)` -> 201 (LinesUnfolder expands the blob; lines_unfolder.rb:55-66). `position.old_line=20,new_line=20`, line_code `<sha1 big.txt>_20_20`. Then `GET .../diffs`-style callers see an unfolded diff only through the note.
2. Same with `new_line:20` only -> 400 line_code error (type `new` = added; not in diff).
3. `old_line:20,new_line:21` -> 400 (offset mismatch; unfold computes `new = old + offset`, offset 0 before the first hunk).
4. After the hunk the offset is the net line delta of earlier hunks (0 here): `old_line:80,new_line:80` -> 201.
5. `old_line:101,new_line:101` (beyond EOF) -> 400 (`old_line > blob.lines.size`, line 62).
6. Applies only to text files with a non-empty diff: same call on `run.sh`/`pure_*` -> 400 (`diff_lines.empty?`, line 63).
Confidence: high for 1,2,5; medium for 3,4.

### A5. Pure rename (100%), mode-only change, binary: text positions impossible, file/image positions work
1. `POS(old_path:"pure_old.txt",new_path:"pure_new.txt",old_line:1,new_line:1)` -> 400 line_code error (empty diff_lines). Same for `run.sh` (mode-only) and `logo.png`.
2. `position:{base_sha,start_sha,head_sha,position_type:"file",old_path:"pure_old.txt",new_path:"pure_new.txt"}` -> 201; response position has ONLY `base_sha,start_sha,head_sha,old_path,new_path,position_type` (file_formatter.rb). `notes[0].type="DiffNote"`, `commit_id:null`.
3. file position with only `new_path` -> `NOTE400({:position=>["is incomplete"]})` (file_formatter.rb `complete?` needs both paths).
4. `position_type:"image"` on `logo.png` with `x:10,y:20,width:100,height:50` -> 201, position additionally has `width,height,x,y`. Missing any -> `{:position=>["is incomplete"]}`. `x:0` is accepted (`0.present?` is true).
Confidence: high.

### A6. file/image positions are not validated against the diff at all
`position_type:"file",old_path:"nope/zzz.txt",new_path:"nope/zzz.txt"` -> 201 (no existence check; `validate_diff_file_and_line` only when importing, diff_note.rb:26,35-50; `create_diff_file` only for text). `position_type:"image"` on `README.md` -> 201 likewise. Thread shows up in GET discussions but never on any file. Confidence: medium-high.

### A7. line_range is stored verbatim and never validated
`POS(new_path:"new.txt",new_line:3, line_range:{start:{line_code:"zzz",type:"new",new_line:99},end:{line_code:"zzz",type:"new",new_line:3}})` -> 201; response `position.line_range` echoes input (bogus start line 99, bogus line_code). Only `old_line/new_line` (the END line) is checked. Valid multi-line = start != end (`multiline?`, position.rb:171-175). `line_range.start.line_code` longer than 100 chars -> `NOTE400({:position=>["must be a valid json schema"...]})` (position.json). Unknown key inside `line_range.start` -> Grape drops it (not declared) . After a later push moves the line, the traced position keeps the OLD `line_range` (line_strategy.rb passes `position.line_range` through unchanged) so start/end go stale. Confidence: high (no validation grep hit in app/models, lib/gitlab/diff).

### A8. Position built from an OLDER diff version (V1) after the MR got V2
Setup: create thread-free MR, `POST /projects/1/repository/commits` on `feature` changing README line 5 again -> after RefreshWorker `GET versions` has 2 entries; `HEAD2` = new head.
1. `POST discussions POS(...)` with V1 shas, README `old_line:2,new_line:2` (context line unchanged by the 2nd commit) -> 201, `position.head_sha` is `HEAD2` (position REPLACED by traced position; update_position diff_positionable_note.rb:77-97), `base/start` of V2.
2. Same with V1 shas but README `old_line:5` (removed line) / `new_line:5` -> 201; if the 2nd commit rewrote line 5 the trace says outdated: `position` stays V1 shas+line (change_position is internal), thread is born "outdated"; NO `changed this line in version` system note (that note comes only from `UpdateDiffPositionService`, which runs for active discussions at push time: merge_request.rb:2472-2479).
3. V1 shas with an abbreviated head (8 chars) -> accepted (Gitaly resolves), `active?` is false (string compare) so position is traced and the response carries FULL V2 shas.
4. Random 40-hex shas -> NDR claims 400 line_code error; code reading says `PositionTracer#compare` -> `CompareService#execute` returns nil for unknown commits (compare_service.rb:15-17) and `.diffs` is called on nil (position_tracer.rb `bd_diffs`) => likely 500 `{"message":"500 Internal Server Error"}`. Confidence: (1)(3) medium-high, (2) medium, (4) low; verify on oracle.

### A9. After a push, which threads get re-traced (and which never do)
Setup: thread T1 on README new_line 5 (active), T2 on README `old_line:3,new_line:3` (context). Push to `feature` that rewrites README line 5 and inserts 2 lines at top.
- T2: position becomes `old_line:3,new_line:5` (traced), no system note. T1: outdated: `position` unchanged (API never shows change_position), a system DiffNote reply is appended in the SAME discussion: body `changed this line in [version 2 of the diff](/<ns>/<proj>/-/merge_requests/1/diffs?diff_id=<id>&start_sha=<sha>#<line_code>)`, `system:true`, `type:"DiffNote"`, `resolvable:false`, action `outdated` (merge_requests_service.rb:58-82; version_index = `merge_request_diffs.viewable.count`).
- If the line vanished entirely so the tracer returns `position:nil`, text becomes `changed this file in version 2 of the diff` (no link) because `position_on_text` is nil-safe false (line 63-64).
- Push #3 that restores the line does NOT re-activate T1: only discussions with `active?(old_diff_refs)` are re-traced (merge_request.rb:2476-2478).
- Project setting `resolve_outdated_diff_discussions=true` (`PUT /projects/1`) -> T1 notes get `resolved:true,resolved_by:<pusher>,resolved_at` and `resolved_by_push` internally; `resolved all threads` note only when all threads end up resolved.
- Closed MR: push does nothing (reload_diff ignores non-open MRs, refresh_service.rb:71-72); versions limit reached -> no new diff (reload_diffs_service.rb:15).
- System note only if there is a current_user (pusher/rebaser); rebase via `PUT .../rebase` counts, `git push` by a deploy key without user does not. Confidence: high/medium (async worker timing).

### A10. Reply to an outdated thread
`POST discussions/<T1.id>/notes body=r` -> 201 Note, `type:"DiffNote"`, `position` = thread's ORIGINAL position (reply_attributes copies position/original_position/line_code, diff_discussion.rb `reply_attributes`), `resolvable:true`. If T1 was resolved, the reply is created already resolved (build_service.rb:65), `resolved_by` = replier. The system "changed this line" note is part of the same discussion so `notes.length` includes it. Replying with `discussion_id` of that system note's discussion is the same id -> allowed (first note is the user's note). Confidence: high.

### A11. Position with wrong path case / extra whitespace
`new_path:"readme.md"` (case differs) -> 400 line_code error (path is a literal git path). Confidence: high.

### A12. Duplicate threads on the same line are allowed
Posting A2/A3 twice yields two discussions with different `id`s; no uniqueness. Confidence: high.

---------------------------------------------------------------------------------------------------
## B. Discussions and notes

### B1. Reply to a reply == reply to the thread
`POST /notes body=a` -> note N1 (individual, `type:null`, discussion id D1). `POST discussions/D1/notes body=b` -> 201 `type:"DiscussionNote"`, and N1 is silently converted: `GET notes/N1` now `type:"DiscussionNote",resolvable:true,resolved:false` (create_service.rb:104-111 `update_discussions`). Replying again with D1 (the id of the reply is the same D1; there is no per-note reply target) -> 201, third note in same discussion. `POST discussions/<id of reply note>/notes` -> 404 `{"message":"404 Discussion Not Found"}` (note ids are not discussion ids). Confidence: high.

### B2. Individual note awards/todos before and after conversion
Award `:thumbsup:` on N1 (E5). Convert (B1). `GET notes/N1/award_emoji` still returns it (awardable_type "Note" is the STI base class). Side effect difference: awarding later on the converted note resolves the user's todos for the DISCUSSION instead of the MR (`add_service.rb` `todoable`). Confidence: medium-high.

### B3. Resolve via note PUT vs discussion PUT (partial resolution)
Thread D (DiscussionNote N1 + reply N2). `PUT discussions/D/notes/N2 resolved=true` -> 200 Note N2 `resolved:true,resolved_by:<you>`; `GET discussions/D` -> `resolved:false` (N1 still open). `PUT discussions/D resolved=true` -> 200 Discussion `resolved:true`, both notes resolved with same `resolved_at`.
Body+resolved together -> 400 `{"error":"body, resolved are mutually exclusive"}`; neither -> `body, resolved are missing, exactly one parameter must be provided` (discussions.rb:239). On an individual note (`type:null`) `PUT discussions/<id>/notes/N resolved=true` -> 400 `400 Bad request - Note is not resolvable` (notes_helpers.rb:66) but `PUT discussions/<id> resolved=true` -> 403 `403 Forbidden` (line 192). Confidence: high.

### B4. Resolving twice emits "resolved all threads" twice
Single thread D on MR. `PUT discussions/D resolved=true` -> 200; system note `resolved all threads` (action `discussion`) + MR webhook action `update`. Repeat the SAME call -> 200 again and ANOTHER `resolved all threads` note + webhook: `Discussions::ResolveService#resolve_discussion` calls `ResolvedDiscussionNotificationService` for every resolvable discussion without checking whether anything changed (resolve_service.rb:30-38; the service has no dedupe, resolved_discussion_notification_service.rb:5-9). Same for `PUT .../notes/N resolved=true` on an already-resolved note (resolve_service.rb in notes/). Confidence: high.

### B5. Resolve state after deleting notes (no system note, state just flips)
Thread D: N1 (resolved), N2 (open reply). `DELETE discussions/D/notes/N2` -> 204; `GET discussions/D` now `resolved:true` (resolved? = no note to_be_resolved; resolvable_discussion.rb:50-53) WITHOUT a `resolved all threads` note and without hook (destroy_service.rb has no notification). Deleting N1 instead (first note) leaves N2 first: discussion `individual_note:false`, N2 `type:"DiscussionNote"`. Deleting the last unresolved thread flips `merge_request.discussions_resolved?` silently. Confidence: high.

### B6. Reply to a resolved thread via API reopens nothing but is created resolved; draft reply reopens
`POST discussions/D/notes body=more` on a resolved D -> 201 note with `resolved:true,resolved_by:<replier>` and D stays `resolved:true` (build_service.rb:65). Compare C6 (draft reply without resolve_discussion un-resolves D). Confidence: high.

### B7. Internal notes in threads
- `POST /notes body=s internal=true` (R/D/O) -> 201 `internal:true,confidential:true`. `POST discussions/<id>/notes body=r` -> 201 reply `internal:true` (inherits parent; discussions reply route has no `internal` param, so it cannot be set or unset).
- `G` (guest) or `X`: `GET discussions` omits that discussion; `GET discussions/<id>` -> 404 `404 Discussion Not Found`; `POST discussions/<id>/notes` -> 404 `404 Discussion Not Found` (readable_discussion_notes filters, discussions.rb:181-186). Total counts X-Total still include it (pagination before filter, NDR 1.1).
- `POST /discussions` has no `internal` param, so internal threads can only be made as individual note + reply (internal DiffNote is rejected: note.rb:893-897 `{:confidential=>["can not be set for this type of note"]}`).
- Reply by a user who later lost `read_internal_note` (demoted to Guest) -> 404 as above; their earlier internal notes remain but are hidden from them. Confidence: high/medium.

### B8. Quick action inside a reply
`POST discussions/D/notes body="/approve"` -> 202 `{"commands_changes":{},"summary":["Approved the current merge request."]}`, no note persisted, thread unchanged; `body="LGTM\n/approve"` -> 201 Note with `body:"LGTM"` (command stripped, extractor.rb:100-120) and the approval + `approved this merge request` system note (async) happen. Quick actions in replies work like top-level ones; `commands_changes` for `/approve` is `{}` because the effect happens inside the command block. Confidence: high.

### B9. Quick action second time: 202 "Could not apply" (not 4xx)
After B8, `POST /notes body="/approve"` again -> 202 `{"commands_changes":{},"summary":["Could not apply approve command."]}` (condition `eligible_for_approval_by?` false; command_definition.rb:81). Same for `/unapprove` without own approval, `/request_changes` by a non-reviewer, `/close` on a closed MR. No error status, nothing persisted. Confidence: high.

### B10. Editing a note to add / remove quick actions
- `PUT notes/N body="hello\n/lock"` (N = D's own note "hello") -> 200 Note body `hello`, `commands_changes:{"discussion_locked":true}`; MR becomes locked + system note `locked the discussion in this merge request`.
- `PUT notes/N body="/lock"` -> 202 `{"commands_changes":{"discussion_locked":true},"summary":["Locked the discussion."]}` AND the note N is DELETED (update_service.rb:57-60 `delete_note`); `GET notes/N` -> 404. Destructive surprise for integrations that "edit" with a bare command.
- Editing N so the body no longer contains a command that an earlier version had: commands were stripped on the first save, so the stored text never contains them; re-adding the same command text later executes again (only update-params, not side-effect commands like `/approve`, are diffed against the original, interpret_service.rb:60-75).
- Edit body to blank: Grape `allow_blank: false` -> 400 `{"error":"body is empty"}`. Confidence: high.

### B11. Quick action update fails AFTER the note was saved: 400 but note exists
Setup: second MR iid 2 `feature2 -> other` and open MR iid 1 `feature2 -> main` conflicts target. `POST /merge_requests/2/notes body="ping\n/target_branch main"` -> note is saved first (`yield(only_commands)` at create_service.rb:78) then `do_commands` adds `note.errors.add(:validation, ...)` (line 131) => 400 `NOTE400({:validation=>["Validate branches Another open merge request already exists for this source branch: !1"]})` but the note "ping" IS persisted (retrying duplicates it). Confidence: medium (exact validation text from merge_request.rb:1364-1372 full_messages).

### B12. Notes on a locked MR by role (after `PUT /merge_requests/1 discussion_locked=true`)
- `X` (non-member): `POST /notes` -> 403 `{"message":"403 Forbidden"}`; `POST discussions/D/notes` -> 403; `PUT discussions/D resolved=true` -> 403; `DELETE`/`PUT` own old note -> 403 (issuable_policy.rb:48-52, note_policy.rb:62-64); `POST award_emoji` -> 404 `404 Award Emoji Not Found`; `POST draft_notes` -> 500 `{"message":"500 Internal Server Error"}` (`access_denied!` is undefined in Grape, draft_notes.rb:43).
- `G` (guest member): `POST /notes` 201 (is_container_member), resolve own thread: 403 unless author of the note/MR (`resolve_note`), `A` (admin non-member): `POST /notes` expected 403 as well because the IssuablePolicy prevent has no admin escape (low confidence).
- `D`,`R`,`O`: unaffected. System notes are created regardless. `reopen` is prevented while locked (merge_request_policy.rb:6-8). Confidence: high for X, medium for G/A.

### B13. Note by a user who then lost access
`X2` posts a note on a private project MR, then is removed (`DELETE /projects/1/members/:id`). The note stays; `GET notes` shows `author` unchanged. X2's calls now 404 `404 Project Not Found`. Their open drafts remain in DB but are unreachable; their approval row persists and still appears in `GET .../approvals` `approved_by` (no cleanup on member removal). Reviewer row also persists (`reviewers` still lists X2). Confidence: medium.

### B14. Notes API checks that bite integrations
- `GET notes?sort=asc&order_by=created_at` orders by id (NDR 1.1). - `POST /notes` with `internal:true` by guest -> silently normal note. - `created_at` ignored for non-owners without error.
- `PUT discussions/<wrong-discussion-id>/notes/N body=x` succeeds: path discussion id is not checked (NDR 1.2). Confidence: high.

---------------------------------------------------------------------------------------------------
## C. Draft notes and review submission

(Every draft call: `/projects/1/merge_requests/1/draft_notes`; `D2` = a second developer who is a reviewer of MR 1: `PUT /merge_requests/1 reviewer_ids=[D2]`.)

### C1. First draft flips reviewer to review_started, last delete flips to unreviewed, even from requested_changes
As D2: `POST draft_notes note=a` -> 201 DraftNote; `GET .../reviewers` -> `[{"user":{...D2},"state":"review_started",...}]` (draft_notes/create_service.rb:52-58). `DELETE draft_notes/<id>` -> 204, state back to `unreviewed`, even if before the draft D2 had `requested_changes` (state was overwritten at first draft). If D2 is `approved` both transitions error "Reviewer has approved" which is ignored (update_reviewer_state_service.rb:15) and state stays `approved`. A second draft does not touch state (`draft_notes.one?`). Confidence: high.

### C2. Publish ordering with mixed replies and new threads
Existing thread D (note N1). As D2: `POST draft_notes {note:"t1", position:POS(new.txt line 1)}` (id1), `{note:"r1", in_reply_to_discussion_id:D}` (id2), `{note:"t2"}` (no position, id3), `{note:"r2", in_reply_to_discussion_id:D}` -> 400 `{"message":{"author_id":["has already been taken"]}}` (one reply draft per thread per user, draft_note.rb validation; DRAFT errors are JSON objects, NDR 25).
`POST draft_notes/bulk_publish` -> 204, notes created in id order (1,2,3): t1 new DiffNote thread, r1 joins D (created resolved if D resolved, then `unresolve!` because `resolve_discussion` false: publish_service.rb:75-84), t2 becomes a `DiscussionNote` THREAD (not individual note; draft_note.rb `type`). All three share one Review (`review_id`), no per-note emails. Reviewer state stays `review_started` (no REST path changes it, except C8). Note ids ascend with draft ids; `GET discussions` order is by first-note created_at. Confidence: high.

### C3. Draft reply to a discussion that was deleted before publishing
`POST draft_notes {note:"r", in_reply_to_discussion_id:D}` -> 201. Then `DELETE` every note of D (`discussions/D/notes/:id`). `POST draft_notes/bulk_publish` -> 204 and the draft is GONE with no note created: `Notes::BuildService#discussion_not_found` returns an invalid Note (build_service.rb:16,77-81), `CreateService` returns it unsaved, publish_service deletes all drafts anyway (`draft_notes.delete_all`, line ~46); `set_discussion_resolve_status` is a no-op because the unsaved note's discussion is not resolvable. Single `PUT draft_notes/:id/publish` -> 204 and same loss. Creating the draft AFTER deletion -> 400 `{"message":{"base":["Thread to reply to cannot be found"]}}`. A Review row is still created. Confidence: high.

### C4. Draft on a line that disappears before publishing
`POST draft_notes {note:"x", position:POS(README new_line:5)}` (V1 shas) -> 201 (`line_code` computed lazily, draft_note.rb:`find_line_code`). Push a commit that removes README line 5; wait for V2. `PUT .../publish` -> 204 and a DiffNote thread is created at the V1 position that is already outdated (DiffNote `update_position` traces at create; `change_position` set; no `changed this line` system note). Publish_params carry `position/original_position/change_position` but NOT `line_code`, so line_code is recomputed on the old refs and validates (draft_note.rb:126-135).
Drafts whose position never existed at the given refs (e.g. README `new_line:77`) are accepted at creation (positions not validated, NDR 1.3) and silently vanish on publish (invalid Note, draft deleted). Confidence: medium-high.

### C5. Draft with wrong shas
`POST draft_notes {position:{base_sha:"deadbeef..40",start_sha:...,head_sha:...,position_type:"text",new_path:"README.md",new_line:5}}` -> likely 500 at CREATE (DraftNote includes DiffPositionableNote `update_position`, same nil-compare as A8.4) rather than silently accepted. Confidence: low.

### C6. Draft reply with resolve_discussion
`POST draft_notes {note:"fixed", in_reply_to_discussion_id:D, resolve_discussion:true}` -> 201 `resolve_discussion:true,discussion_id:D`. `PUT publish` -> reply created, D resolved by publisher, and `ResolvedDiscussionNotificationService` runs on EVERY publish (publish_service.rb:117-123): if all threads are now resolved a `resolved all threads` note + MR hook is emitted. A reply draft WITHOUT `resolve_discussion` on a resolved D re-opens it. `resolve_discussion:true` for a user who cannot resolve (e.g. `X` on locked MR) -> 400 `{"message":{"base":["User is not allowed to resolve thread"]}}`; with no `in_reply_to_discussion_id` -> same error. Confidence: high.

### C7. Quick actions inside draft bodies
- `note:"/approve"` alone: bulk_publish -> 204; the draft is consumed (command-only note never saved), D2 approved (state `approved`, system note `approved this merge request` async). `review_id` suppresses the inner publish (merge_request_actions.rb:268).
- `note:"LGTM\n/approve"` -> note "LGTM" published inside the review AND approval.
- `note:"/submit_review approve"` inside a draft: no-op (`next if params[:review_id]`, line 195) and the draft disappears.
- Second draft also containing `/approve` -> condition false after the first approval ("Could not apply" ignored); nothing persisted.
- Single publish of a draft containing `/approve` or `/submit_review` (no review_id): the quick action runs `DraftNotes::PublishService` for ALL of the user's drafts first (lines 195-205, 268-270) -> every pending draft is published, not only the one addressed; response still 204. Confidence: high for first 4, medium for the last.

### C8. /submit_review as the only REST way to set reviewer state
`POST /notes body="/submit_review"` (D2) -> 202 `{"commands_changes":{},"summary":["Submitted the current review."]}`; publishes drafts, then `UpdateReviewerStateService('reviewed')`: system note `left review comments` (action `reviewed`), state `reviewed`. `/submit_review requested_changes` -> note `requested changes` + state `requested_changes`. `/submit_review approve` -> summary `["Submitted the current review.","Approved the current merge request."]` joined by one space -> single string `"Submitted the current review. Approved the current merge request."`; failed approval -> `... Failed to approve the current merge request.`. `/submit_review bogus` -> only "Submitted the current review." (merge_request_actions.rb:194-240). Confidence: high.

### C9. /submit_review by a NON-reviewer still writes system notes
`D` (developer, not a reviewer): `POST /notes body="/submit_review requested_changes"` -> 202 `Submitted the current review.`; side effects: system note `requested changes` is created BEFORE the reviewer lookup, then "Reviewer not found" is ignored (update_reviewer_state_service.rb:10-12,28-30). `/submit_review reviewed` -> system note `left review comments` with no state anywhere. Same quirk for an `approved` reviewer sending `/submit_review reviewed`: note `left review comments` is written, then "Reviewer has approved" error, state stays `approved`. Confidence: high.

### C10. Draft limits and shapes
Blank `note=""` draft accepted (201) and skipped at bulk publish but removed (`delete_all`); `PUT draft_notes/:id {position:...}` without `note` -> 400 `400 Bad request - Missing params to modify`; `PUT {note:"y"}` without position wipes the draft's position (`update!(note:, position: nil)`, draft_notes.rb:192) so a diff draft turns into a plain thread on publish. Confidence: high.

---------------------------------------------------------------------------------------------------
## D. Suggestions

Setup: `big.txt` on `feature` has 100 lines `line N\n`. Thread via `POST discussions` with ```` ```suggestion:-0+0\nNEW\n``` ```` at `POS(big.txt old_line:60,new_line:60)` -> `notes[0].suggestions:[{id:S,from_line:60,to_line:60,appliable:true,applied:false,from_content:"line 60\n",to_content:"NEW\n"}]` (entity lib/api/entities/suggestion.rb, exact fields `id,from_line,to_line,appliable,applied,from_content,to_content`). Apply = `PUT /api/v4/suggestions/:id/apply`.

### D1. Ranges clamped at file start / end
- `suggestion:-5` at `new_line:2` -> `from_line:1` (clamped to 1, suggestible.rb `from_line`), `to_line:2`, `lines_above` stored 5. Apply replaces lines 1-2 with the block.
- `suggestion:+9` at `new_line:98` (file has 100 lines) -> `to_line:107`, `from_content` is lines 98-100 only (array slice truncates); apply replaces 98-100.
- `-100` / `-150`: above clamps to 100 (MAX_LINES_CONTEXT) for line math. Syntax `+2-1` or `-1 +2` is not a context spec: treated as single line (suggestions_parser.rb:7). Confidence: high.

### D2. Empty or whitespace-only suggestion DELETES the lines
Block ```` ```suggestion\n\n``` ```` or only spaces: `to_content:""` (`@text.blank?`, suggestion.rb:36). Apply removes the line including its newline (replacement string is empty, no line break appended). A suggestion that should replace a line by an empty line cannot be expressed. Confidence: high.

### D3. Trailing newline, no trailing newline, CRLF
- File `big.txt` without trailing newline, suggestion on LAST line: `from_content:"line 100"` (no break), `to_content` = text only (line break chars come from the last from-line, suggestion.rb:42-43) so the result also has no trailing newline - PROVIDED the Banzai code text has no trailing "\n"; if it does the file gains a newline. [VERIFY on oracle: what `node.text` of the code block returns.]
- CRLF file: `from_content` lines end `\r\n`; `to_content` = text + `\r\n` (regex `/\r\n|\r|\n/` on the last from-line). A multi-line suggestion's inner line breaks come from the Markdown code text (cmark normalises to `\n`), giving mixed endings (inner `\n`, last `\r\n`). Confidence: medium/low.
- `different_content?`: identical replacement -> `appliable:false`, apply -> 400 `{"message":"This suggestion already matches its content."}`. Confidence: high.

### D4. Two suggestions, same file, one batch
`PUT /suggestions/batch_apply {ids:[S1,S2]}` (also `ids=1,2` comma form) with S1 line 10, S2 line 20 -> 200 `[{id:S1,...},{id:S2,...}]`, ONE commit on `feature` `Apply 2 suggestion(s) to 1 file(s)\n\n` (offset-adjusted ranges, file_suggestion.rb:44-52). Multi-line S1 (`-1+1`, 3 lines -> 1 string) followed by S2 below it is placed correctly (offset += count-1).
Overlap: S1 `-1+1` at line 10 (9-11) and S2 at line 11 -> 400 `{"message":"Suggestions are not applicable as their lines cannot overlap."}` (suggestion_set.rb:82); adjacent non-overlapping (line 12) fine. Two suggestions on the SAME line from two notes -> same error. Unknown id in batch -> 404 `{"message":"Suggestions are not applicable as one or more suggestions were not found."}`; duplicate id counts as missing. Different source branches -> 400 `Suggestions must all be on the same branch.` Check order in `_error_message`: per-suggestion errors (file missing, branch, appliable, head) come before overlap. Response `applied` is likely still `false` (records loaded before `update_all`) - [VERIFY]. Confidence: high (behaviour), medium (response flags).

### D5. Apply after a new push
a) Push that touches OTHER lines, MR diff refreshed: position traced to new head; apply -> 200, commit parent is the pushed head.
b) Push done but RefreshWorker has not yet created V2 (or `diff_refs` stale): 400 `{"message":"A file has been changed."}` (`position.head_sha != noteable.source_branch_sha`, suggestion_set.rb:100-103).
c) Push that changes the suggested line, after refresh: `Suggestions::OutdateService` sets `outdated=true` (refresh_service.rb:40) -> 400 `Can't apply as this line was changed in a more recent version.` (multi-line: `these lines were`). Entity `appliable:false`.
d) Pushes on a CLOSED MR: no V2, suggestions not re-evaluated, but apply -> 400 `This merge request is closed. To apply this suggestion, edit this file directly.`; merged -> `This merge request was merged...`; source branch deleted -> `Can't apply as the source branch was deleted.`. Already applied -> `Can't apply this suggestion.`. `.ipynb` -> readability message. Confidence: high.

### D6. Editing the note re-creates suggestions (ids change, applied lost)
`PUT notes/N body=<same text>` (or any edit) -> `note.suggestions.delete_all` + recreate (update_service.rb:98-106): new ids, `applied:false` even if the old one was applied. Old id then: `PUT /suggestions/<old>/apply` -> 404 `{"message":"Suggestion is not applicable as the suggestion was not found."}`. Edits that only contain commands skip this. Confidence: high.

### D7. Commit message placeholders
`PUT /suggestions/S/apply {commit_message:"Fix %{file_paths} by %{username} on %{branch_name} (%{suggestions_count}/%{files_count}) %{project_path} %{project_name} %{user_full_name}\n\n%{co_authored_by}"}` -> commit message with substitutions; sorted unique paths joined `, `; `%{co_authored_by}` = `Co-authored-by: Name <commit_email>` lines for each DISTINCT suggestion-note author other than the applier, joined `\n`, empty if none (commit_message.rb:24-39). Unknown `%{foo}` stays literal. `commit_message:""` falls back to project `suggestion_commit_message` then default `Apply 1 suggestion(s) to 1 file(s)\n\n` (note the `(s)`). Project setting comes from the TARGET project. Commit author/committer = applier (`author_name=current_user.name`, `commit_email_or_default`). Confidence: high.

### D8. Who can apply
`R`/`G`/`X`: 403 `403 Forbidden` (needs push to the SOURCE branch; protected source branch with `push_access_level` Maintainers makes `D` fail). Author of the note may apply their own if they can push. Note: no MR-read check beyond that. Confidence: high.

### D9. Suggestion in reply notes / on context lines
Reply to a diff thread with a suggestion block -> suggestion created on the reply (position copied; `supports_suggestion?` true unless thread line is a removed line). Suggestion block on unchanged context line (A4.1) is allowed; `target_line=position.new_line`. Suggestion body in an ordinary (non-diff) discussion note: no rows, `suggestions` key absent from Note JSON (entity only for DiffNote). Confidence: high.

---------------------------------------------------------------------------------------------------
## E. Award emoji

### E1. Aliases, case, URL encoding
`POST /merge_requests/1/award_emoji name=thumbs_up` / `+1` (JSON) / `:thumbsup:` -> 201 `{"id","name":"thumbsup",...}`: stored name always canonical (add normalisation `TanukiEmoji.find_by_alpha_code(name)&.name`, base_service.rb:18). Second alias by same user -> 404 `{"message":"404 Award Emoji Name has already been taken Not Found"}` (canonicalised first, so `thumbs_up` then `+1` collides). Upper case `THUMBSUP`/`Thumbsup` -> 404 `404 Award Emoji Name is not a valid emoji name Not Found` (lookup is case-sensitive per NDR 6; [VERIFY]). Form-encoded `name=+1` arrives as `" 1"` (plus = space) -> 404 invalid name; use JSON or `%2B1`. `GET .../award_emoji` returns `name:"thumbsup"` and MR `upvotes` counts 1. Confidence: high (alias/dup), medium (case).

### E2. Award, delete, award again
`POST` -> id 11; `DELETE award_emoji/11` -> 204; `POST` same name -> 201 with NEW id 12 (uniqueness only on live rows); webhooks `award`,`revoke`,`award`. `DELETE award_emoji/11` again -> 404 `{"message":"404 Not found"}`. `DELETE` another user's award -> 401 `401 Unauthorized`; admin may. `/react :thumbsup:` twice in notes: first adds (202 `Toggled :thumbsup: emoji reaction.`), second removes. Confidence: high.

### E3. Emoji on a note in a locked MR
`X` (non-member) after lock: `POST notes/N/award_emoji` -> 404 `404 Award Emoji Not Found`; `GET .../award_emoji` -> 200 (read allowed). Member `G`: 201. Confidence: high.

### E4. DELETE of own award on a locked MR returns 204 but deletes nothing
`X` awarded before the lock. After `discussion_locked=true`: `DELETE award_emoji/<id>` by `X` -> 204; `GET award_emoji` still lists it. The API calls `DestroyService` with the award OWNER and ignores the result; `user_can_award?` is false (award_emoji.rb:103-106, destroy_service.rb:6-8, helpers.rb `destroy_conditionally!` sets 204 first). Same when the award owner lost access/was blocked and an admin deletes it. Confidence: high.

### E5. Award on system note / internal note / non-existent note
System note -> 404 `404 Award Emoji Awardable cannot add emoji reactions Not Found`; internal note as `G` -> 404 `404 Award Emoji Not Found`; unknown note id -> 404 `404 Not found`. Awarding own MR/note is allowed. Confidence: high.

### E6. Todos side effect of awarding
Award on the MR or any of its notes marks the awarder's pending todos on the MR done (add_service.rb todoable); on a `DiscussionNote` only the discussion's todos. MR `updated_at` unchanged, the NOTE `updated_at` bumped. Confidence: medium.

---------------------------------------------------------------------------------------------------
## F. Approvals and reviewer states

### F1. Approve, push, still approved
`POST /approve` (D2) -> 201; push new commit to `feature`; `GET /approvals` -> `approved_by` still lists D2, no `reset approvals` note, reviewer state unchanged (ARTS 1.8). `POST /approve` again -> 401 `401 Unauthorized`. With `sha=<stale head>` -> 409 `SHA does not match HEAD of source branch: <head>` checked before eligibility (ARTS edge 5). Confidence: high.

### F2. Approve on closed MR works; merged does not
`PUT /merge_requests/1 state_event=close`; `POST /approve` (D) -> 201 and async `approved this merge request` note (closed MRs allowed, approval_service.rb:7-8). After merge -> 401. `POST /unapprove` on merged -> 404 `404 Not Found`. Confidence: high.

### F3. Admin who is not a member
`A` (admin, not in project): `POST /approve` -> 403 `403 Forbidden` (`approve_merge_request` needs `is_container_member`; `ProjectTeam#member?` uses project_authorizations, not the admin owner shortcut, project_team.rb:189-193 and 209-220; merge_request_policy.rb:51-53). `A` can still `POST /notes`, resolve threads, merge. `/approve` as quick action by A -> 202 `Could not apply approve command.` Confidence: medium-high (admin mode irrelevant to this check).

### F4. Re-request review of a reviewer who approved
D2 approved and is reviewer (state `approved`). Owner `POST /notes body="/request_review @D2"` -> 202 `{"commands_changes":{"reviewer_ids":[D2.id]},"summary":["Requested a review from @D2."]}`. Effects executed DURING parsing (merge_request_actions.rb:355-372): D2's approval deleted silently, state `unreviewed`, system note `requested review from @D2 and removed approval` (action `reviewer`), review_requested todo for D2, MR webhook `update` with reviewer `re_requested:true`. No `unapproved this merge request` note. `approved_by` empties. The unapproved-then-approved flow works again (`POST /approve` by D2 -> 201). By a user without `update_merge_request` (R): 202 `Could not apply request_review command.` Confidence: high.

### F5. /request_review of a second user is silently ignored (single reviewer)
MR has reviewer alice. `POST /notes body="/request_review @bob"` -> 202 summary `["Requested a review from @bob."]` and `commands_changes.reviewer_ids=[alice,bob]` but `GET .../reviewers` still only alice: update params are cut with `first(1)` keeping the EXISTING first id (base_service.rb:209, update_reviewers_service.rb `reviewer_ids`). `PUT /merge_requests/1 reviewer_ids=[bob]` replaces alice (system note `requested review from @bob and removed review request for @alice`, order: added first). `reviewer_ids=[bob,carol]` -> only bob. `reviewer_ids=[0]` clears. Confidence: high.

### F6. Reviewer removed while having approved; re-added
D2 approved. `PUT /merge_requests/1 reviewer_ids=[0]` -> system note `removed review request for @D2`; D2's approval is KEPT (`GET /approvals` still lists D2, `POST /approve` by D2 -> 401). Re-add `reviewer_ids=[D2]` -> note `requested review from @D2`, state set straight to `approved` (`set_reviewers_approved`, base_service.rb:227-232). Confidence: high.

### F7. /request_changes then /approve ordering, and the reverse
Reviewer D2 (any state): `body="/request_changes"` -> 202 `["Changes requested to the current merge request."]`; system note `requested changes` (action `requested_changes`), state `requested_changes`. If D2 had approved: approval removed WITHOUT note/webhook/notification (remove_approval skips all, base_service.rb:313-316) and state stays `requested_changes`. Then `/approve` or `POST /approve` -> 201, state `approved`. Reverse: approved then `/request_changes` as above. `/request_changes` twice: second also writes another `requested changes` system note (note created before the state check, line 10). Non-reviewer: 202 `Could not apply request_changes command.`. Confidence: high.

### F8. Approve/unapprove quick actions vs REST status codes
`/unapprove` quick action needs own approval: else 202 `Could not apply unapprove command.`; REST `POST /unapprove` -> 404. Quick action `/unapprove` by reviewer sets state `unapproved` and writes `unapproved this merge request` synchronously. Author of MR with Developer role: `POST /approve` -> 201 (no author check) while `GET /approvals` `user_can_approve:false` (ARTS 1.6). Confidence: high.

### F9. publish_review on approve
`POST /approve publish_review=true` publishes the user's drafts first (bulk), then approves; both in one call -> 201; failure to publish -> 500 `{"message":"Failed to publish review"}`. Order of system notes: draft notes first, then `approved this merge request` (async, may sort after). Confidence: medium.

### F10. Reviewer state vs mixed flows summary (state machine used by cases above)
`unreviewed -(first draft)-> review_started -(last draft deleted)-> unreviewed`; `* -(approve)-> approved`; `approved -(unapprove)-> unapproved`; `approved -(re-request)-> unreviewed (+approval removed)`; `approved -/-> reviewed|review_started|unreviewed` (error "Reviewer has approved", but the `left review comments` note IS written for `reviewed`); `requested_changes` allowed from any state; `reviewer removed` deletes the row (state lost), re-add restores `approved` from the approval rows. Confidence: high.
