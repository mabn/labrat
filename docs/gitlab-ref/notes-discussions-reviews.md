# GitLab 18.11 reference: MR notes, discussions, diff positions, draft notes/reviews, suggestions, award emoji, quick actions

Source tree: `/home/mabn/dev/gitlab-run-ce/gitlab-src` (GitLab CE 18.11). Paths below are relative to it.
Runtime facts verified on the oracle container: Ruby 3.3.10 (so `Hash#inspect` renders as `{:note=>["..."]}`, NOT the 3.4 `{note: [...]}` form); tanuki_emoji 0.13.0.
Items marked **[VERIFY]** are derived from reading code but are subtle enough that they should be confirmed against the live 18.11 oracle (localhost:8929) before being frozen into a fixture.

Conventions: `lib/api/helpers.rb` error helpers produce `{"message": "..."}`:
- `not_found!(x)` -> 404 `"404 x Not Found"` (`404 Not Found` with no arg) (helpers.rb:593)
- `forbidden!(r)` -> 403 `"403 Forbidden"` (+ `" - r"`) (helpers.rb:581, 670)
- `bad_request!(r)` -> 400 `"400 Bad request - r"` (helpers.rb:585, 670). Note lowercase "request".
- `unauthorized!` -> 401 `"401 Unauthorized"`
- `ActiveRecord::RecordNotFound` anywhere -> 404 `{"message":"404 Not found"}` (lowercase f) (api.rb:134)
- Grape param errors -> 400 `{"error":"body is missing"}`, `"body is empty"` (allow_blank false), `"x does not have a valid value"`, `"x is invalid"` (coerce). `exactly_one_of` -> `"body, resolved are missing, exactly one parameter must be provided"`; both given -> `"body, resolved are mutually exclusive"` (Grape en.yml).
- Unhandled exception -> 500 via `rescue_from :all`.
- POST default 201, PUT 200, DELETE 204 (`destroy_conditionally!` sets 204 and honours `If-Unmodified-Since`: 412 `"412 Precondition Failed"` if resource `updated_at` is newer; helpers.rb:40-62).
- Timestamps serialised ISO-8601 with ms (`2024-01-01T00:00:00.000Z`) by the JSON formatter.

---------------------------------------------------------------------------------------------------
## 1. Routes

All project-scoped: prefix `/api/v4/projects/:id`. `:id` = project id or url-encoded path. `:noteable_id` / `:merge_request_iid` is the MR **iid** (notes.rb:44, find_noteable uses `target_iid`, notes_helpers.rb:126-137).

Common lookup failures:
- project not visible -> 404 `404 Project Not Found`
- MR not found/not readable -> 404 `"404 MergeRequest Not Found"` (`not_found!(MergeRequest)` stringifies the class; notes_helpers.rb:123). Requires both `read_merge_request` and `read_note` on the MR (helpers.rb:235-238).
- Unauthenticated -> 401 (`authenticate!`), except routes with job-token policy; GETs allow `ai_workflows` scope and job tokens (`read_merge_requests` policy).

### 1.1 Notes (lib/api/notes.rb)

| Verb | Path | Success |
|---|---|---|
| GET | `/merge_requests/:noteable_id/notes` | 200, array of Note |
| GET | `/merge_requests/:noteable_id/notes/:note_id` | 200 Note |
| POST | `/merge_requests/:noteable_id/notes` | 201 Note, or 202 NoteCommands |
| PUT | `/merge_requests/:noteable_id/notes/:note_id` | 200 Note, or 202 NoteCommands |
| DELETE | `/merge_requests/:noteable_id/notes/:note_id` | 204 empty |

**GET list params** (notes.rb:43-52): `order_by` String in `created_at|updated_at` default `created_at`; `sort` `asc|desc` default **`desc`**; `activity_filter` in `all_notes|only_comments|only_activity` default `all_notes` (keys of `UserPreference::NOTES_FILTERS`, user_preference.rb:12); pagination `page`=1, `per_page`=20 (max 100, offset pagination with `X-*` headers).
- `only_comments` -> `Note.user` (system=false); `only_activity` -> `Note.system` (note.rb:154-163).
- Ordering (notes_helpers / helpers.rb:560-570 `order_options_with_tie_breaker`): if `order_by=created_at` the order column is replaced with **`id`**: `ORDER BY id <sort>`; for `updated_at`: `ORDER BY updated_at <sort>, id <sort>`.
- Pagination happens BEFORE the readability filter: after paginating, notes that fail `readable_by?` (internal notes for non-reporters, system notes whose cross-references the user cannot see) are dropped, so a page can be shorter than `per_page` while headers (`X-Total`, etc.) count unfiltered rows (notes.rb:75-89).
- Includes system notes by default. Includes internal notes only if the user has `read_internal_note` (reporter+ in project).

**GET single**: `noteable.notes.find(note_id)` (404 `404 Not found` if the note does not belong to that MR); if unreadable -> 404 `"404 Note Not Found"` (notes_helpers.rb:90-99).

**POST params** (notes.rb:127-134): `body` String required (empty string passes Grape and fails the model), `confidential` Boolean (deprecated alias of internal), `internal` Boolean, `created_at` String, `merge_request_diff_head_sha` String.
- Flow (notes_helpers.rb:151-178): rate limit `check_rate_limit! :notes_create, scope: current_user` (default 300/min; allowlist setting) -> 429 `{"message":{"error":"This endpoint has been requested too many times. Try again later."}}`; `find_noteable`; `authorize!(:create_note, noteable)` -> 403 `403 Forbidden`; `created_at` is silently **dropped** unless the user has `set_note_created_at` (owner role / admin); when kept, `updated_at = created_at` too; `internal: params[:internal] || params[:confidential]`.
- Result handling `process_note_creation_result` (notes_helpers.rb:165-178), in order:
  1. quick-actions status `commands_only? && success?` -> **202** body `NoteCommands` (see 1.1.1)
  2. `note.errors.present?` -> 400 `"400 Bad request - Note #{note.errors.messages}"`, e.g. `400 Bad request - Note {:note=>["can't be blank"]}`
  3. `note.persisted?` -> 201 `Note` entity
  4. quick action `error?` -> 400 `"400 Bad request - " + error_messages.join(', ')`
- `QuickActions::InterpretService::QuickActionsNotAllowedError` rescued -> 403 with that message.
- Validation messages (all via step 2): empty body `{:note=>["can't be blank"]}`; too long `{:note=>["is too long (1.1 MB). The maximum size is 1 MB."]}` (`BytesizeValidator`, app/validators/bytesize_validator.rb: `number_to_human_size` of byte size vs `description_and_note_max_size`, default **1.megabyte = 1,048,576 bytes**, not 1,000,000; application_setting.rb:786); >=5000 notes on the MR -> `{:base=>["Maximum number of comments exceeded"]}` (note.rb:861, `Noteable::MAX_NOTES_LIMIT`=5000; not applied to system notes); `created_at` before 1970 -> `{:created_at=>["The created date provided is too far in the past."]}`; internal on a reply whose parent has different confidentiality -> `{:confidential=>["reply should have same confidentiality as top-level note"]}`; internal on a DiffNote type -> `{:confidential=>["can not be set for this type of note"]}`.
- Internal note on an MR is allowed (MR is `for_issuable?`, note.rb:890-898) but requires `mark_note_as_internal` (needs `read_internal_note`, i.e. reporter+, and `read_merge_request`). If the user lacks it, `internal`/`confidential` is **silently ignored** (BuildService:64) and a normal note is created.
- `merge_request_diff_head_sha` is only consumed by the `/merge` quick action.

**1.1.1 NoteCommands (202)**: `{"commands_changes": {...}, "summary": ["<message>"]}`. `commands_changes` is `note.commands_changes || {}`; `summary` is `quick_action_status.messages` = array of at most one string: all per-command execution messages joined with a single space (`execution_messages_for`, interpret_service.rb:166-168; `QuickActionsStatus#add_message` skips blanks). 202 is used for both create and update when the final note text (after removing commands) is empty.
- `commands_changes` is whitelisted+ordered by `Note#commands_changes` (note.rb:711-740): `due_date, label_ids, remove_label_ids, add_label_ids, canonical_issue_id, clone_with_notes, confidential, create_merge_request, add_contacts, remove_contacts, assignee_ids, milestone_id, time_estimate, spend_time, discussion_locked, merge, rebase, wip_event, target_branch, reviewer_ids, health_status, promote_to_epic, weight, emoji_award, todo_event, subscription_event, state_event, title, tag_message, tag_name` (Hash#slice order = this list order). Commands with no `@updates` (approve, unapprove, submit_review, request_changes, run_pipeline) yield `{}`. `commands_changes` is only set when `update_params.present?` and no validation error occurred.
- Command-only note with no message at all -> status error `"Failed to apply commands."` -> 400 `400 Bad request - Failed to apply commands.` (create_service.rb:224). Invalid reviewers/assignees count: `{:validation=>["Reviewers ..."]}` style via `note.errors.add(:validation, ...)` -> step 2 (400 `Note {:validation=>[...]}`). If the downstream MR update fails the same path is used with the service's messages joined by `, `.
- Command-only notes are never persisted and fire no note webhook, no todo from the (empty) note.

**PUT params** (notes.rb:171-177): `body` String `allow_blank: false`, `confidential` Boolean. No `internal`.
- `authorize! :admin_note, note` -> 403 (see section 10). Then `Notes::UpdateService`; result via `process_note_creation_result` (400 `Note {...}`, 200 Note, 202 NoteCommands).
- With no params (`params.present?` false) or system note (`editable?` false) the service returns the note untouched -> 200 with the unchanged note (but system notes already failed `admin_note` -> 403).
- Setting `confidential: true` on an existing note -> `{:confidential=>["can not be changed for existing notes"]}`.

**DELETE**: `authorize! :admin_note` -> 403; `destroy_conditionally!` -> 204 empty. (`Notes::DestroyService`: destroys note, deletes its todos, `suggestions` (dependent delete_all), events.)

### 1.2 Discussions (lib/api/discussions.rb)

| Verb | Path | Success |
|---|---|---|
| GET | `/merge_requests/:noteable_id/discussions` | 200 `[Discussion]` |
| GET | `.../discussions/:discussion_id` | 200 Discussion |
| POST | `.../discussions` | 201 Discussion |
| POST | `.../discussions/:discussion_id/notes` | 201 Note |
| GET | `.../discussions/:discussion_id/notes` | 200 `[Note]` |
| GET | `.../discussions/:discussion_id/notes/:note_id` | 200 Note |
| PUT | `.../discussions/:discussion_id/notes/:note_id` | 200 Note |
| DELETE | `.../discussions/:discussion_id/notes/:note_id` | 204 |
| PUT | `.../discussions/:discussion_id` | 200 Discussion (resolve/unresolve) |

- **List** (l.54-61): paginates `noteable.discussion_ids_relation` = `SELECT discussion_id FROM notes WHERE noteable GROUP BY discussion_id ORDER BY MIN(created_at), MIN(id)` (always ascending; no sort params), `page`=1/`per_page`=20; then loads all notes of those discussion_ids ordered `created_at, id` asc, drops unreadable notes, groups by `discussion_id`. System notes are included (each system note is its own individual-note discussion). A discussion whose notes are all unreadable disappears while counted in headers.
- **Get one**: no notes after readability filter -> 404 `"404 Discussion Not Found"`. **Get notes of discussion**: empty -> 404 `"404 Notes Not Found"`.
- **POST create discussion** params: `body` (required), `created_at`, `position` Hash (MR only): required `base_sha`, `start_sha`, `head_sha`, `position_type` in `text|image|file` (else 400 `position[position_type] does not have a valid value`); optional `new_path`, `new_line`, `old_path`, `old_line` (Integer), `width`,`height`,`x`,`y` (Integer), `line_range` Hash {`start`,`end`: Hash with optional `line_code` String, `type` String, `old_line`, `new_line` Integer}. If `position` given, note `type: 'DiffNote'`, otherwise `'DiscussionNote'`. `commit_id` is **not** a param (always nil on MR diff notes). Goes through `create_note` + `process_note_creation_result` (so same 400/403/429 behaviour; quick actions in the body apply: a commands-only body returns 202 NoteCommands instead of Discussion). Response `note.discussion` (Entity Discussion, 201).
- **POST reply** params: `body`, `created_at`, path `discussion_id`. Order of checks (l.181-211): `readable_discussion_notes` -> empty -> 404 `404 Discussion Not Found`; first note `system?` -> 400 `400 Bad request - Replies to system notes are not allowed.`; `unless first_note.part_of_discussion? || first_note.to_discussion.can_convert_to_discussion?` -> 400 `400 Bad request - Discussion can not be replied to.`; creates with `type: 'DiscussionNote'`, `in_reply_to_discussion_id`. (BuildService may still fail with 400 `Note {:base=>["Discussion to reply to cannot be found"]}` e.g. internal parent and user cannot mark internal; or `{:base=>["Replies to system notes are not allowed"]}`.) A DiffNote thread's reply is rewritten to DiffNote with the parent's position by `reply_attributes` (section 3). Response is Note (201), not Discussion. `created_at` dropped unless `set_note_created_at`.
- **PUT note in discussion** (l.233-250): `body` String (blank allowed by Grape, model rejects blank) and/or `resolved` Boolean; `exactly_one_of :body, :resolved`. `resolved` present -> `resolve_note` (below); else `update_note` as in 1.1. Note: `discussion_id` in the path is NOT verified against the note (the note is found via `noteable.notes.find(note_id)`).
  - `resolve_note` (notes_helpers.rb:61-76): `authorize! :resolve_note, note` (403); `bad_request!("Note is not resolvable")` -> 400 `400 Bad request - Note is not resolvable` if `!note.resolvable?` (plain Note, system note); `resolved=true` -> `Notes::ResolveService#execute` -> resolves **only that one note** (`note.resolve!`; returns false and no error if already resolved) and then `ResolvedDiscussionNotificationService`; `resolved=false` -> `note.unresolve!`. Responds 200 Note entity. The thread is "resolved" only when all its resolvable notes are resolved (section 3).
- **DELETE note**: same as 1.1.
- **PUT discussion resolve** (only for resolvable noteable types; MR yes) params `resolved` Boolean required: `find_discussion` nil -> 404 `404 Discussion Not Found`; `forbidden! unless discussion.can_resolve?(current_user)` -> 403 `403 Forbidden` (also returned for a non-resolvable discussion such as an individual note, because `can_resolve?` requires `resolvable?`); resolved -> `Discussions::ResolveService` (all notes), else `Discussions::UnresolveService`. 200 Discussion entity. Resolving an already-resolved thread is a no-op 200.

### 1.3 Draft notes (lib/api/draft_notes.rb)

All under `/projects/:id/merge_requests/:merge_request_iid/draft_notes` (`:merge_request_iid` via `find_project_merge_request`, 404 `404 Not found` if missing). Drafts are always scoped to the current user (`merge_request.draft_notes.authored_by(current_user)`), so another user's draft id is a 404.

| Verb | Path | Success |
|---|---|---|
| GET | `/draft_notes` | 200 `[DraftNote]` (no pagination, ordered by default scope - treat as id asc) |
| GET | `/draft_notes/:draft_note_id` | 200 DraftNote |
| POST | `/draft_notes` | 201 DraftNote |
| PUT | `/draft_notes/:draft_note_id` | 200 DraftNote |
| DELETE | `/draft_notes/:draft_note_id` | 204 |
| PUT | `/draft_notes/:draft_note_id/publish` | 204 |
| POST | `/draft_notes/bulk_publish` | 204 |

- **POST params** (l.143-151, 50-80): `note` String required; `in_reply_to_discussion_id` String; `commit_id` String (`"undefined"` is mapped to nil); `resolve_discussion` Boolean (default false); `position` Hash with the same shape as 1.2 (here `line_range` allowed regardless of type). No `internal`, no `note_type`.
- Flow: `access_denied! unless can?(:create_note, mr)`. NOTE: `access_denied!` is only defined on `ApplicationController` in this tree; in the Grape API it would raise NoMethodError -> 500 `{"message":"500 Internal Server Error"}` (practically unreachable since an MR you can't comment on is usually a 404 first). Then `DraftNotes::CreateService`; `persisted?` -> 201; else `render_validation_error!` -> 400 `{"message": <errors.messages hash>}` (hash, not string), e.g. `{"message":{"base":["Thread to reply to cannot be found"]}}`.
- Errors from `DraftNotes::CreateService` (create_service.rb:205-241): `in_reply_to_discussion_id` unknown -> `base: "Thread to reply to cannot be found"`; target thread's first note is system -> `base: "Replies to system notes are not allowed"`; `resolve_discussion: true` but no discussion given or user cannot resolve -> `base: "User is not allowed to resolve thread"`; second draft reply by the same author to the same discussion -> `author_id: ["has already been taken"]` (uniqueness scope merge_request_id+discussion_id); note over size limit -> `note: ["is too long (...). The maximum size is 1 MB."]`; `position` JSON too large / schema invalid (app/validators/json_schemas/position.json, `additionalProperties:false`, string maxLength 64 for shas, 1000 for paths) -> `position: [...]`.
- **A blank `note` is accepted** (DraftNote has no presence validation, only DB `NOT NULL`); bulk publish skips blank drafts (`next unless draft_note.note.present?`).
- **Positions are not validated at draft creation** (no line_code validation on DraftNote): invalid lines are accepted and fail silently at publish (section 4).
- **PUT** params: `note` (allow_blank false -> Grape 400 `note is empty`), position. `bad_request!('Missing params to modify') unless params[:note].present?` -> 400 `400 Bad request - Missing params to modify`; missing draft -> 404 `404 Draft Note Not Found`; `authorize_admin_draft!` (`admin_note`). Executes `draft_note.update!(note: params[:note], position: params[:position])`. **Edge:** `position` is always assigned, so a PUT without `position` sets the draft's `position` to nil/blank (DiffPositionableNote setter turns nil into nil); `original_position` is not touched. **[VERIFY]**
- **DELETE**: 204; `DraftNotes::DestroyService` -> if the user has no more drafts, reviewer state -> `unreviewed` (section 4.4).
- **Publish** (`PUT .../:id/publish`): see section 4. 204 on success, `status 500` (no JSON body defined) on service error.
- **Bulk publish** (`POST .../bulk_publish`): 204 on success (also when there are no drafts), else 500.
- 404 for missing draft: `404 Draft Note Not Found` on GET/DELETE (`not_found!("Draft Note")`), but `get_draft_note` uses `.find` so an unknown id actually raises RecordNotFound -> `404 Not found` first. Treat as `{"message":"404 Not found"}`.

### 1.4 Suggestions (lib/api/suggestions.rb)

Top-level, not project-scoped, requires auth only (`before { authenticate! }`).

- `PUT /api/v4/suggestions/:id/apply` params: `id` Integer, `commit_message` String optional. Unknown id -> 404 `{"message":"Suggestion is not applicable as the suggestion was not found."}` (plain message, no "404 ... Not Found" wrapping).
- `PUT /api/v4/suggestions/batch_apply` params: `ids` Array[Integer] (comma-separated string also accepted via `CommaSeparatedToIntegerArray`), `commit_message`. If `Suggestion.id_in(ids).size != ids.length` (also on duplicate ids) -> 404 `"Suggestions are not applicable as one or more suggestions were not found."`.
- Authorization: `authorize! :apply_suggestion, suggestion` for each (SuggestionPolicy: user can push to the MR **source branch** per `Gitlab::UserAccess#can_push_to_branch?`) -> 403 `403 Forbidden`. Note there is no check that the user can read the MR beyond this.
- Apply error: `Suggestions::ApplyService` result `{status: :error, message:}` -> HTTP `result[:http_status] || 400` with `{"message": "<msg>"}`.
- Success: 200; response = `present suggestions, with: Entities::Suggestion` (single record -> object; batch -> array) built from the **in-memory records loaded before the update** (`update_all` is not reflected), so `applied` in the response is likely still `false` and `appliable` still `true`. **[VERIFY]**

### 1.5 Award emoji (lib/api/award_emoji.rb, lib/api/helpers/award_emoji.rb)

MR scope (also issues, snippets). `awardable` for MR: `user_project.merge_requests.find_by!(iid:)` (RecordNotFound -> `404 Not found`); for notes: `awardable.notes.find(note_id)`.

| Verb | Path |
|---|---|
| GET | `/merge_requests/:merge_request_iid/award_emoji` |
| GET | `/merge_requests/:merge_request_iid/award_emoji/:award_id` |
| POST | `/merge_requests/:merge_request_iid/award_emoji` (param `name`) |
| DELETE | `/merge_requests/:merge_request_iid/award_emoji/:award_id` |
| same four | `/merge_requests/:merge_request_iid/notes/:note_id/award_emoji[/:award_id]` |

- List: `can_read_awardable?` else 404 `404 Award Emoji Not Found`; `awardable.award_emoji` (default order `id` asc, `includes(:user)`), paginated (page/per_page=20) -> `[AwardEmoji]`. For notes, read ability is `Note#issuable_ability_name` = `:read_internal_note` if internal else `:read_note`; for MR `:read_merge_request`.
- Get one: `awardable.award_emoji.find(award_id)` (404 `404 Not found`).
- **POST** `name` String required (award_emoji.rb:425-439): `not_found!('Award Emoji') unless can_read_awardable? && can_award_awardable?` -> 404 `404 Award Emoji Not Found` (note: 404, not 403, when the user can't award - e.g. locked MR for a non-member, or cannot see the note). Then `AwardEmojis::AddService`; on failure `not_found!("Award Emoji #{message}")`, where message = `errors.to_sentence`:
  - invalid name: `404 Award Emoji Name is not a valid emoji name Not Found` (validator message `is not a valid emoji name`, emoji_name_validator.rb:17)
  - duplicate by same user: `404 Award Emoji Name has already been taken Not Found` (uniqueness on [user, awardable_type, awardable_id], award_emoji.rb:23)
  - system note: `404 Award Emoji Awardable cannot add emoji reactions Not Found` (`Note#emoji_awardable?` = `!system?`)
  - (`User cannot add emoji reactions to awardable` is unreachable via API because of the pre-check.)
  - success: 201 AwardEmoji entity.
- **DELETE**: `award = awardable.award_emoji.find(award_id)` (404 `404 Not found`); `unauthorized! unless award.user == current_user || current_user&.can_admin_all_resources?` -> **401** `{"message":"401 Unauthorized"}` (not 403) for another user's award; then `destroy_conditionally!` (412 support) -> `AwardEmojis::DestroyService.new(awardable, award.name, award.user)` -> 204. Webhook `revoke`.

---------------------------------------------------------------------------------------------------
## 2. Entities

All entity JSON key order = declaration order. `UserBasic` is documented elsewhere (lib/api/entities/user_basic.rb).

### 2.1 Note (lib/api/entities/note.rb) - used by notes, discussion notes, resolve responses
Order and conditions:
1. `id` Integer
2. `type` String|null - DB column `notes.type`: `null` (plain Note, incl. system notes and notes created via `POST /notes`), `"DiscussionNote"`, `"DiffNote"`. (`LegacyDiffNote` only in old data.)
3. `body` - `NotePresenter#note` = raw markdown text (system notes: email-obfuscated only for issue_email_participants)
4. `author` UserBasic
5. `created_at`, `updated_at`
6. `system` Boolean
7. `noteable_id` (MR **id**), `noteable_type` (`"MergeRequest"`)
8. `project_id`
9. `commit_id` - only when `noteable_type=="MergeRequest" && DiffNote`; null for API-created MR diff notes
10. `position` - only for DiffNote: `note.position.to_h.except(:ignore_whitespace_change)` (section 2.3)
11. `resolvable` Boolean (`potentially_resolvable? && !system?` i.e. type in DiffNote/DiscussionNote on MR and not system)
12. `resolved` Boolean, 13. `resolved_by` UserBasic|null, 14. `resolved_at` - **only emitted when `resolvable`** (keys absent otherwise)
15. `suggestions` - only for MR DiffNote; array of Suggestion (2.5), ordered by `relative_order`
16. `confidential` Boolean (= `internal`), 17. `internal` Boolean (same value)
18. `imported` Boolean, 19. `imported_from` String enum name: `none|gitlab_migration|gitlab_project|gitlab_group|github|bitbucket|bitbucket_server|fogbugz|gitea|git|manifest|custom_template|jira` (has_import_source.rb)
20. `noteable_iid` (MR iid; Issue/MR only)
21. `commands_changes` Hash (always present; `{}` when none; filtered as in 1.1.1)

`resolved_at`/`resolved_by` come from the note's own columns (`resolved_at`, `resolved_by_id`). `resolved` is `false` (not null) for unresolved resolvable notes. System notes have `resolvable:false`, no resolved keys, `type:null`, `system:true`.

### 2.2 Discussion (lib/api/entities/discussion.rb)
`id` (the `discussion_id` of the first note), `individual_note` Boolean, `resolvable` Boolean, `resolved` Boolean (only when resolvable), `notes` [Note]. `resolvable?` = first note potentially resolvable && any note resolvable; `resolved?` = resolvable && no note is `to_be_resolved?` (a note is to-be-resolved when resolvable and `resolved_at` blank).

### 2.3 Position (JSON inside `position` of Note / DraftNote)
`Gitlab::Diff::Formatters::*#to_h` (lib/gitlab/diff/formatters/):
- text: `base_sha, start_sha, head_sha, old_path, new_path, position_type:"text", old_line, new_line, line_range` (`line_range` is `null` when not supplied; otherwise the hash as submitted: `{"start":{"line_code","type","old_line","new_line"},"end":{...}}`)
- image: `base_sha, start_sha, head_sha, old_path, new_path, position_type:"image", width, height, x, y`
- file: `base_sha, start_sha, head_sha, old_path, new_path, position_type:"file"`
`ignore_whitespace_change` is always removed. Missing numeric/path values are `null`. A `position` with no `position_type` defaults to the text formatter. `lib/api/entities/diff_position.rb` (`DiffPosition`: only the 6 base keys) is not used by these endpoints.

### 2.4 DraftNote (lib/api/entities/draft_note.rb)
`id, author_id, merge_request_id, resolve_discussion, discussion_id, note, commit_id, line_code, position`.
- `discussion_id` null unless a reply draft; `commit_id` hex sha string or null (bytea column via `sha_attribute`).
- `line_code` is `DraftNote#line_code` = stored value or computed lazily from the diff (`find_line_code` hits git; null for non-diff drafts).
- `position`: `note.position.to_h.except(:ignore_whitespace_change)`. For a non-diff draft the column is NULL and the `serialize ..., type: Position` coder loads a blank `Position` -> a text-formatter hash with every value null and `position_type:"text"`: `{"base_sha":null,"start_sha":null,"head_sha":null,"old_path":null,"new_path":null,"position_type":"text","old_line":null,"new_line":null,"line_range":null}`. **[VERIFY]**
- No `type`, `internal`, `author` object, timestamps.

### 2.5 Suggestion (lib/api/entities/suggestion.rb)
`id, from_line, to_line, appliable, applied, from_content, to_content`.
- `from_line = max(target_line - min(lines_above,100), 1)`, `to_line = target_line + min(lines_below,100)` where `target_line = note.position.new_line` (app/services/concerns/suggestible.rb:7-25).
- `appliable` = `appliable?(cached: true)` = `inapplicable_reason.nil?`, using the stored `outdated` column. `applied` DB boolean. `outdated` and `lines_above/below` are DB columns but not exposed.

### 2.6 AwardEmoji (lib/api/entities/award_emoji.rb)
`id, name, user (UserBasic), created_at, updated_at, awardable_id, awardable_type, url`. `awardable_type` is `"MergeRequest"` or `"Note"`; `awardable_id` is the MR **id** (not iid) / note id. `url` is null for standard emoji; for group custom emoji it is the image URL.

### 2.7 NoteCommands (202): `commands_changes`, `summary` (array of strings).

### 2.8 System notes in these payloads
Created by `SystemNotes::*` via `Note.create(system: true, ...)` (system_notes/base_service.rb:19-24) with a `system_note_metadata.action`. They appear as Note with `system:true`, `type:null`, `noteable_type:"MergeRequest"`, `resolvable:false`, author = acting user, `body` e.g. (MR-review related; services/system_notes/merge_requests_service.rb, issuables_service.rb):
- `approved this merge request` (action approved); `unapproved this merge request`; `requested changes` (requested_changes); `left review comments` (reviewed); `resolved all threads` (discussion); `requested review from @user` / `... and removed approval` (reviewer); `changed this line in [version 2 of the diff](url)` / `changed this file in version N of the diff` (outdated; created as a *reply* in the diff discussion: `discussion.reply_attributes` => `type` DiffNote, same `discussion_id`, position copied, `system:true`); `created #123 to continue this discussion` (discussion); `marked this merge request as **draft**`; `enabled an automatic merge when all merge checks for <sha> pass`.
System notes live in the same discussions list (own discussion unless an `outdated` note attached to a diff discussion).

---------------------------------------------------------------------------------------------------
## 3. Discussion semantics, positions

**discussion_id generation** (app/models/discussion.rb:71-84, notes/discussion.rb:64-77): `Digest::SHA1.hexdigest([:discussion, noteable_type.underscore, noteable_id_or_commit_id, SecureRandom.hex].join("-"))` -> 40 hex chars. For MR: `"discussion-merge_request-<mr id>-<32 random hex>"` (symbol `:discussion` joins as `discussion`). Generated lazily on first `discussion_id` read (`ensure_discussion_id`); validation requires `/\A\h{40}\z/`. Every note gets its own random id when created standalone; replies copy the parent's `discussion_id` (`reply_attributes`).

**Individual note vs thread** (`Note#discussion_class`, `IndividualNoteDiscussion`):
- `POST /notes` creates a `Note` (type NULL) = *individual note*: `Discussion` entity with `individual_note:true`, `resolvable:false`, no `resolved` key.
- `POST /discussions` w/o position creates `DiscussionNote` (thread, resolvable). With position creates `DiffNote`.
- Replying to an individual note (`POST discussions/:id/notes` with the note's discussion_id, allowed because `can_convert_to_discussion?` = first note not system && noteable `supports_replying_to_individual_notes?` (Issue, MR, AbuseReport, WikiPage::Meta)): `BuildService` calls `convert_to_discussion!` (`first_note.becomes!(DiscussionNote)`), reply gets `type` DiscussionNote and the same `discussion_id`; `CreateService#update_discussions` then saves the first note with `type: "DiscussionNote"` in the same transaction. The thread becomes resolvable; reply attributes drop `discussion_id` for individual notes and re-add via `in_reply_to_discussion_id` (BuildService:6-26).
- Replying to a resolved thread: `note.resolve_without_save(current_user) if discussion&.resolved?` (BuildService:65) - the reply is created already resolved by the replier.
- Commit notes shown in an MR use `OutOfContextDiscussion` (out of scope).

**Resolvable**: `RESOLVABLE_TYPES = DiffNote, DiscussionNote`; `Noteable.resolvable_types` includes MergeRequest; system notes never. A thread is resolvable if any note is. `resolved_by_push` column set when auto-resolved. `Note.resolve!` bulk via `update_all(updated_at, resolved_at, resolved_by_id)` for whole-discussion resolve (no callbacks => no MR touch).
- Permissions `resolve_note`: note author, MR author (`is_noteable_author`), developer+ role, admin; denied for non-members when MR `discussion_locked`; denied for internal notes the user can't read; denied when user cannot read the MR.

**Resolve side effects**: `Discussions::ResolveService` -> per discussion `resolve!`; unique counter; `ResolvedDiscussionNotificationService`: if `merge_request.discussions_resolved?` creates system note `resolved all threads` and fires the **merge_request hook with action `update`**, then async notification. If auto-merge enabled and discussions OK -> `DiscussionsResolvedEvent`. Single-note REST resolve (`Notes::ResolveService`) also calls `ResolvedDiscussionNotificationService`.
- Project setting `only_allow_merge_if_all_discussions_are_resolved` (via `MergeRequest#mergeable_discussions_state?`) blocks merge; unrelated to the notes API.

**Positions and diff notes (DiffNote)**:
- Valid noteable types: MergeRequest, Commit, DesignManagement::Design. `type='DiffNote'` requires non-nil `position`.
- Callbacks on create (`DiffPositionableNote`, diff_note.rb): `set_original_position` (copy of supplied position), `update_position` (if the supplied `position.diff_refs` != MR current `diff_refs`: `PositionTracer.trace` onto the current diff; if the line still exists -> `position` is replaced with the traced position; if outdated -> `change_position` is set and `position` stays original), then `set_line_code` (`position.line_code(repository)`).
- `active?` = `position.diff_refs == noteable.diff_refs` (compares base/start/head). A note is "outdated" when `position` is older than the MR's current `diff_refs`.
- **line_code format**: `"<sha1 hex of file_path>_<old_pos>_<new_pos>"`, `file_path = new_path.presence || old_path` (lib/gitlab/git.rb:113 `diff_line_code(file_path, new_pos, old_pos)` -> `"#{sha}_#{old_pos}_#{new_pos}"`, called from `Diff::Line#legacy_id`). `old_pos`/`new_pos` are the diff parser's *running counters* for both sides on every line (lib/gitlab/diff/parser.rb: both advance on context lines, only new on `+`, only old on `-`), so an added line has the old-side counter at that point (not 0/empty) and a removed line has the current new-side counter; the code therefore always matches `LineCodeValidator::PATTERN = /\A[a-z0-9]+_\d+_\d+\z/` (applies only for `on_text?`). Lookup for a position: `diff_file.line_for_position(pos)` finds the line whose `old_line == pos.old_line && new_line == pos.new_line` (`Line#old_line/new_line` are nil for the non-existent side), so API positions must give `new_line` only for added lines, `old_line` only for removed lines, both for unchanged lines.
- **Invalid position handling at REST create** (nothing explicit; errors come from validations, message `400 Bad request - Note {...}`):
  - `position` hash fails JSON schema -> `{:position=>[...]}`
  - old/new path or line missing -> `{:position=>["is incomplete"]}` (`positions_complete`: `file_path` present, text: `old_line` or `new_line` present, image: `x,y,width,height` present, file: both `new_path` and `old_path` present, `start_sha` and `head_sha` present). Note `base_sha` is NOT required by `DiffRefs#complete?`.
  - MR has no complete diff refs -> `{:noteable=>["doesn't support new-style diff notes"]}`
  - Line not found in the diff for the given shas/path (text position) -> `line_code` computes to nil -> `{:line_code=>["can't be blank", "must be a valid line code"]}`. SHAs that do not resolve to a comparable diff behave the same (diff file nil). (`validate_diff_file_and_line`, with messages `Failed to find diff line for: ...`, only runs during imports.)
  - Positions with `base/start/head` of an earlier MR version are accepted and traced to the latest version.
  - `old_line` on unchanged context lines: `LinesUnfolder` unfolds the diff to find lines outside the hunks, so comments on unchanged lines outside the diff hunk are valid if they exist in the file (requires both old_line and new_line for context lines).
- Multi-line: `position.line_range = {start: {line_code,type,old_line,new_line}, end: {...}}`; `multiline?` iff start != end. `position.old_line/new_line` is the end line. `type` is `"old"`/`"new"`/null.
- File-level (`position_type:"file"`): only `old_path,new_path`; `should_update_position?` is true, tracer uses FileStrategy; no line_code (`on_text?` false). Image: x,y,width,height; tracer ImageStrategy; no line_code; `grouped_diff_discussions` keyed by new_path.
- `supports_suggestion?` for a DiffNote: MR + on_text + latest diff file has the position line and that line is not a removed line.
- `DiffNote#created_at_diff?(diff_refs)` = `original_position.diff_refs == diff_refs`.
- After creating a first-in-thread diff note on an MR, `Discussions::CaptureDiffNotePositionService` (create_service.rb:197-205) records a `diff_note_positions` row for the merge-ref head (internal only; not in REST output).
- A `NoteDiffFile` (hunk) is created `after_commit` (`create_diff_file`) for the first text DiffNote of a discussion; failure raises `NoteDiffFileCreationError` (500).

**Outdating on push** (merge_request.rb:2472-2509, merge_requests/reload_diffs_service.rb, `Discussions::UpdateDiffPositionService`): when a new MR diff version is created, for every *active* new-style diff discussion the position is traced from old to new diff refs. Found -> all notes get new `position`, `change_position=nil`. Not found (outdated) -> notes get `change_position = traced position`, `position` unchanged (so API `position` stays the original); if `project.resolve_outdated_diff_discussions?` (default false) the notes are resolved with `resolved_by_push=true` (resolved_by = pushing user); if a `current_user` exists a system note is added to the discussion: `changed this line in [version N of the diff](/<project>/-/merge_requests/<iid>/diffs?diff_id=...&start_sha=...#<line_code>)` / `changed this file in version N of the diff`, action `outdated`. Also `Suggestions::OutdateService` sets `suggestions.outdated=true` for suggestions whose lines changed, and `MergeRequests::ResolvedDiscussionNotificationService` when the flag auto-resolves every thread.
- API never exposes `original_position`/`change_position`.

---------------------------------------------------------------------------------------------------
## 4. Draft notes and review submission

### 4.1 DraftNote model (app/models/draft_note.rb, table draft_notes)
Columns: id, merge_request_id, author_id, resolve_discussion (default false), discussion_id, note (NOT NULL), position, original_position, change_position (YAML-serialised Position), commit_id (bytea), line_code, internal (default false; not settable via REST), note_type smallint enum `Note:0, DiffNote:1, DiscussionNote:2` (REST never sets it), project_id (= target project).
- `DraftNote#type`: `note_type` if set; `'DiffNote'` if `position&.complete?`; else `'DiscussionNote'` (also for reply drafts). So **non-diff top-level drafts publish as `DiscussionNote` threads (resolvable)**, never plain individual notes.
- Reply drafts: `discussion_id` set from `discussion.reply_id` (the real DB discussion id).

### 4.2 Create (`DraftNotes::CreateService`)
Order: reply checks -> resolve check -> `DraftNote.save` -> clear diff cache if unfolded -> unique counter -> `keep_around_commits` for diff drafts -> `after_execute`: **if the user now has exactly one draft** (`draft_notes.one?`) -> `MergeRequests::UpdateReviewerStateService#execute(mr, 'review_started')`. That service (update_reviewer_state_service.rb): requires `can?(:update_merge_request, mr)` (else error "Invalid permissions", ignored by the draft service); `reviewer = mr.find_reviewer(current_user)` (non-reviewers: error "Reviewer not found", ignored); if reviewer already `approved` and new state not in `requested_changes|unapproved` -> error "Reviewer has approved"; else `reviewer.update(state:)` and triggers (GraphQL subscriptions, MR cache counts). So for a user who is a reviewer of the MR, the first draft sets their state `review_started`; non-reviewers see no change.

### 4.3 Publish (`DraftNotes::PublishService`, draft_notes/publish_service.rb)
- `can?(:create_note, mr)` else `error('Not allowed to create notes')` (-> REST 500).
- **Single** (`PUT /:id/publish`): `create_note_from_draft(draft)` then `draft.delete`. No `Review` row is created. Each note triggers its own notifications (`NewNoteWorker`) and note webhook. After success: `DraftNotePublishedEvent` published (handle_notifications).
- **Bulk** (`POST /bulk_publish`, no draft arg): if the user has no drafts nothing happens (204). Else: `Review.create!(author, merge_request, project)`; for each draft in `order_id_asc` (blank notes skipped): `draft.review = review`; create the note through `Notes::CreateService` with `skip_capture_diff_note_position`, `skip_keep_around_commits`, `skip_merge_status_trigger` true; afterwards `capture_diff_note_positions` for diff discussions, `keep_around_commits`, `draft_notes.delete_all` (also removes blank drafts), `GraphqlTriggers.merge_request_merge_status_updated`, invalidate assignees' MR cache counts; then the event. Notes created in a review have `review_id` set, so `Note#skip_notification?` is true: **no per-note notification emails**; a single review notification is sent by the worker instead.
- `create_note_from_draft`: unfolds diff lines for the draft's `original_position`, then `Notes::CreateService.new(project, current_user, draft.publish_params)`. `publish_params` (draft_note.rb:126-135): `noteable` (the MR), `type` (4.1), `note`, `internal`; for diff drafts also `position, original_position, change_position, commit_id`; `in_reply_to_discussion_id` (reply drafts); `review_id`. Quick actions in the note body run exactly as for normal notes (and `review_id` suppresses `/submit_review` & `/approve` re-publishing).
- **Replies**: `BuildService` finds the discussion, converts an individual note to a discussion if needed, copies `type/noteable/discussion_id` (and position for diff discussions). Afterwards `set_discussion_resolve_status`: for reply drafts only, if `draft.resolve_discussion && discussion.can_resolve?(user)` -> `discussion.resolve!`, **else `discussion.unresolve!`** (publishing a reply without `resolve_discussion` re-opens a resolved thread, undoing BuildService's pre-resolve of the reply).
- **Top-level drafts become new discussions** (DiffNote/DiscussionNote with fresh discussion_id) in draft id order. No error is raised if a created note is invalid (e.g. bad position or stale line): `Notes::CreateService` returns the unsaved note and the draft is deleted anyway, i.e. **invalid drafts are silently dropped on bulk publish** (single publish deletes the draft likewise). `ActiveRecord::RecordInvalid` (from `Review.create!`) is the only path to the `error("Unable to save ...")` result. **[VERIFY]**
- After publish (`MergeRequests::ProcessDraftNotePublishedWorker`, async via event store): `TodoService#new_review` (marks the publisher's pending todos on the MR done via `resolve_todos_for_target`, and creates a todo of action `review_submitted` for assignees and author), `NotificationService#new_review(review)` if a bulk review, and if `discussions_resolved?` -> resolve-all notification. `ResolvedDiscussionNotificationService(send_notifications: false)`: if all discussions are resolved: system note `resolved all threads` + MR webhook (action `update`).
- Reviewer state is **not** changed by publishing. In REST-only flow the reviewer stays `review_started` (set at first draft) until something else sets it. The web "Submit review" (Projects::MergeRequests::DraftsController#publish) additionally: creates a standalone summary note via `Notes::CreateService` with the optional `note` param (plain `Note`, noteable MR; this is the "review summary note"), then `update_reviewer_state`: `reviewer_state=approved` -> `MergeRequests::ApprovalService`, otherwise `UpdateReviewerStateService(state)` - with `'reviewed'` it adds system note `left review comments` (skipped for the Duo code review bot), `'requested_changes'` adds `requested changes`. There is no REST equivalent; the quick action `/submit_review [state]` is the REST-reachable path (below).

### 4.4 Reviewer states (app/models/concerns/merge_request_reviewer_state.rb)
Enum `merge_request_reviewers.state`: `unreviewed:0, reviewed:1, requested_changes:2, approved:3, unapproved:4, review_started:5`. Transitions:
- reviewer added -> `unreviewed` (default)
- first draft created -> `review_started`; last draft deleted/discarded (`DestroyService#after_execute` when `draft_notes.empty?`) -> `unreviewed` (this also fires on single-draft delete; also errors "Reviewer has approved" if approved)
- web submit review `reviewed` / `requested_changes` / `approved`; `/submit_review` quick action (state default `"reviewed"`; `approve` string -> ApprovalService; any key of `MergeRequestReviewer.states` -> UpdateReviewerStateService)
- approve (`MergeRequests::ApprovalService`) -> `approved`; unapprove -> `unapproved` (sets via RemoveApprovalService)
- re-request review (`MergeRequests::RequestReviewService`): reviewer state -> `unreviewed`, removes that user's approval (system note suffix ` and removed approval`), creates a `review_requested` todo + email, system note `requested review from @user`, triggers MR hook `update` (with `re_requested_reviewer_id` in the old-associations payload), GraphQL triggers. Errors: `Invalid permissions` (needs `update_merge_request`), `Reviewer not found`, `Failed to update reviewer`. Reachable in REST via quick action `/request_review @user` when the user is already a reviewer (or GraphQL `mergeRequestReviewerRereview`).
- `requested_changes` via GraphQL `mergeRequestRequestChanges` / `/request_changes` quick action (user must be a reviewer): `UpdateReviewerStateService#execute(mr,'requested_changes')`: system note `requested changes`, merge-status trigger; if the user had approved, the approval is removed. Failure messages: `Invalid permissions`, `Reviewer not found`, `Reviewer has approved` (only when state not in requested_changes/unapproved), `Failed to update reviewer`, `Failed to remove approval`.
- `GET .../reviewers` (outside this doc) reflects `state`.

---------------------------------------------------------------------------------------------------
## 5. Suggestions

**Syntax**: a fenced code block with language `suggestion` plus optional parameters appended with a colon, e.g. ```` ```suggestion:-3+2 ````. The Markdown code-block parser puts the text after `:` into `data-lang-params`; the regex is `^(?:\-(?<above>\d+))?(?:\+(?<below>\d+))?$` (suggestions_parser.rb:7) so valid forms are `-N`, `+M`, `-N+M` (order above-then-below only); anything else (`+M-N`, spaces) fails the match and is treated as a single-line suggestion (`lines_above=lines_below=0`). Values clamp to 100 (`MAX_LINES_CONTEXT`) when computing lines but are stored unclamped in `lines_above/lines_below`.
- Parsing (`Gitlab::Diff::SuggestionsParser.parse`): `Banzai.render(text, suggestions_filter_enabled: true)`, take `pre.language-suggestion` nodes in order; skipped (returns `[]`) if the position isn't complete. Each node -> `Gitlab::Diff::Suggestion(node.text, line: position.new_line, above:, below:, diff_file: position.diff_file(repo))`.
- `from_content` = lines `from_line..to_line` (1-based, inclusive) of the **new blob at the position's head sha** joined (`new_blob_lines_between`). `to_content` = suggestion text (`""` if blank) + the line-break chars (`\r\n|\r|\n`) of the last line of `from_content` (nil if it has none). The exact trailing-newline handling of `node.text` should be checked on the oracle. **[VERIFY]**
- Created by `Suggestions::CreateService` in `Notes::CreateService#when_saved` only if `note.supports_suggestion?` (MR DiffNote on a text position at a non-removed line, including replies in a diff thread), inserted in groups of 100 with `relative_order` = index, `namespace_id = project.project_namespace_id`. Also re-created on note update (`Notes::UpdateService#update_suggestions`: delete all, recreate) unless the update was commands-only. Table: `suggestions(id, relative_order, applied, commit_id, from_content, to_content, lines_above, lines_below, outdated, note_id, namespace_id)`.

**inapplicable_reason** (suggestion.rb:51-66), in order, `appliable` iff nil:
1. `Can't apply this suggestion.` (already applied)
2. `This merge request was merged. To apply this suggestion, edit this file directly.`
3. `This merge request is closed. To apply this suggestion, edit this file directly.`
4. `Can't apply as the source branch was deleted.`
5. outdated (column `outdated`, or `!note.active?` i.e. the note's position head differs from MR diff refs): single line -> `Can't apply as this line was changed in a more recent version.`; multi-line -> `Can't apply as these lines were changed in a more recent version.`
6. `This suggestion already matches its content.` (from_content == to_content)
7. `This file was modified for readability, and can't accept suggestions. Edit it directly.` (path ends with `ipynb`)

**Apply** (`Suggestions::ApplyService` + `Gitlab::Suggestions::SuggestionSet`): validation `error_message` returns the first of: per suggestion - `A file was not found.` (no diff file); `Suggestions must all be on the same branch.`; the live `inapplicable_reason(cached: false)` (recomputes `outdated?` by comparing `from_content` against the current blob); `A file has been changed.` (position head_sha != `noteable.source_branch_sha`); then overlapping line ranges in the same file -> `Suggestions are not applicable as their lines cannot overlap.`. Errors surface as 400 `{"message": ...}`.
- Files are rewritten per file from the source-branch blob: suggestions sorted by `from_line_index`, ranges replaced with `to_content` while tracking an offset for multi-line ranges. Commit via `Files::MultiService` on `source_project`, branch = start_branch = `mr.source_branch`, `author_name = current_user.name`, `author_email = current_user.commit_email_or_default`, actions all `update`.
- On success: `Suggestion.update_all(commit_id: <new commit sha>, applied: true)`, unique counters, `merge_request.mark_as_preparing`.
- **Commit message** (lib/gitlab/suggestions/commit_message.rb): `custom_message.presence || project.suggestion_commit_message.presence || DEFAULT` where DEFAULT = `"Apply %{suggestions_count} suggestion(s) to %{files_count} file(s)\n\n%{co_authored_by}"` (always this form, even for 1 suggestion: `Apply 1 suggestion(s) to 1 file(s)`). The project setting is taken from `suggestion_set.target_project` (target project, 255 chars). Placeholders (plain substring replace, `StringPlaceholderReplacer`): `%{branch_name}`, `%{files_count}`, `%{file_paths}` (sorted unique paths joined `, `), `%{project_name}`, `%{project_path}`, `%{user_full_name}`, `%{username}`, `%{suggestions_count}`, `%{co_authored_by}` (`Co-authored-by: Name <commit_email>` for each distinct suggestion author != current user, joined `\n`). With no co-authors the default message ends with `\n\n` + empty string (trailing blank lines). Applies to custom `commit_message` too (placeholders are substituted there as well).
- Commit author: `author_name/author_email` of the applier; the commit's committer is the same user.

---------------------------------------------------------------------------------------------------
## 6. Award emoji

- Name validity (app/validators/gitlab/emoji_name_validator.rb): `TanukiEmoji.find_by_alpha_code(name)` (tanuki_emoji 0.13.0 index of 3782 emoji incl. aliases; lookup is case-sensitive; colons optional) else, if the awardable's `resource_parent` is a Group or the project's namespace is a Group, a group custom emoji with that name (including ancestors). Otherwise error `Name is not a valid emoji name`.
- Normalisation (`AwardEmojis::BaseService#normalize_name`): `TanukiEmoji.find_by_alpha_code(name)&.name || name` - aliases collapse to the canonical name before saving: `+1`, `:+1:`, `thumbs_up`, `thumbsup` -> `thumbsup`; `-1`, `thumbs_down` -> `thumbsdown`; `heart` stays; `red_heart` -> `heart`; `party_popper` -> `tada`. Stored `award_emoji.name` is always canonical. Custom emoji names pass through as given.
- `AwardEmoji::THUMBS_UP='thumbsup'` / `THUMBS_DOWN='thumbsdown'`: `upvotes`/`downvotes` counts (MR list/entity fields `upvotes`, `downvotes`) count rows with exactly these names; `votes_for_collection` groups by name. They are computed live.
- Who can award (`:award_emoji`): project role guest and up (config/authz/roles/guest.yml, inherited), including logged-in non-members on public projects as with `create_note`; but for an MR the user also needs `read_merge_request` effectively via `can_read_awardable?`. Denied when: MR `discussion_locked` and user not a project member (`IssuablePolicy:49-52`); for Note awardables: cannot read the MR, system note not visible, internal note without `read_internal_note` (note_policy.rb:40-88). **Authors may award their own MR/note** (no restriction). Duplicates by the same user: error above. Different users may award the same emoji.
- Awarding **a system note**: rejected (`emoji_awardable?` false).
- Awarding via REST note body: `POST /notes` with body `:thumbsup:` just creates a normal text note; `Note#award_emoji?` (note.rb:496) is not used in the API/services (legacy web controller behaviour) - **no conversion happens**. The only body-driven award is the `/react :emoji:` (alias `/award`) quick action, which toggles via `AwardEmojis::ToggleService` (adds if absent else destroys).
- Side effects on add (`AddService#after_create`): `TodoService#new_award_emoji(todoable, user)` marks the awarder's pending todos done: target is the MR; for a `DiscussionNote` the discussion; for a plain Note its noteable (MR); then emoji webhook (`emoji_hooks`, payload `object_kind:"emoji"`, `event_type:"award"`, `user`, `project_id`, `project`, `object_attributes{user_id,created_at,id,name,awardable_type,awardable_id,updated_at,awarded_on_url}`, plus `note` hash and `merge_request` hash for note awards, or just `merge_request`) only if the project has active emoji hooks. `revoke` event on destroy.
- Cache side effects (AwardEmoji model callbacks): `after_save/destroy :expire_cache` -> `awardable.try(:bump_updated_at)`: a **Note** gets `updated_at = now` (also copies `updated_at` to `last_edited_at` if previously edited via `updated_by`); the **MergeRequest has no `bump_updated_at`**, so the MR `updated_at` is unchanged by awards. Note awards also broadcast a note update.
- Destroy via REST: only own award or admin (`can_admin_all_resources?`) else 401. `DestroyService` is invoked with the award owner's user, so `user_can_award?` is evaluated on the owner (an admin deleting another user's award on a locked MR can hit `User cannot destroy emoji on the awardable`; the REST result is ignored - 204 regardless).
- `award_emoji.namespace_id` = project namespace id of the MR (sharding key); `organization_id` null unless note/personal-snippet.

---------------------------------------------------------------------------------------------------
## 7. Quick actions in MR notes/descriptions

Pipeline: `Notes::CreateService#execute_quick_actions` -> `Notes::QuickActionsService` (supported noteables: WorkItem, Issue, MergeRequest, Commit) -> `QuickActions::InterpretService#execute` (requires global `use_quick_actions` ability, denied for some bots/blocked users; otherwise content is returned unchanged). Extraction (lib/gitlab/quick_actions/extractor.rb): only lines starting with `/` at the start of a paragraph-level line (not inside inline code, fenced code blocks, or HTML blocks), `\r` removed, command name matched case-insensitively against command names/aliases, optional ` <args to end of line>`; the command line (including its newline) is removed; result `rstrip`ped. Unknown commands, or commands whose `condition` is false, are NOT executed (a command whose condition fails yields message `Could not apply <name> command.` in `execution_message`, counted as executed for `commands_executed_count`). Substitutions (`/shrug`, `/tableflip`) replace the line with text (`¯\＿(ツ)＿/¯`, `(╯°□°)╯︵ ┻━┻`) and keep the note.
- Commands run **before** the note is saved; the remaining text is saved. Update params are applied after save via `MergeRequests::UpdateService(project, current_user, params)` (`apply_updates`); update errors -> `note.errors.add(:validation, ...)`.
- When original text is considered (note edit), commands already present in the old text are ignored.

MR-relevant commands (`desc`/condition/behaviour; messages are the *execution* messages used in `summary`). `upd` = key placed in `commands_changes`.

Issuable (lib/gitlab/quick_actions/issuable_actions.rb):
- `/close` (open && can update) upd `state_event:"close"`; msg `Closed this merge request.`
- `/reopen` (closed && can update; blocked by lock) upd `state_event:"reopen"`; `Reopened this merge request.`
- `/title <t>` upd `title`; `Changed the title to "<t>".`
- `/label`,`/labels ~a ~"b c"` (can `set_merge_request_metadata`) upd `add_label_ids`; `/unlabel`,`/remove_label [labels]` upd `remove_label_ids` or `label_ids: []`, msg `Removed ~a label.`/`Removed all labels.`; `/relabel` upd `label_ids`, msg `Replaced all labels with ...`
- `/todo` upd `todo_event:"add"` `Added a to-do item.`; `/done` `todo_event:"done"` `Marked to-do item as done.`
- `/subscribe`/`/unsubscribe` upd `subscription_event` `Subscribed to notifications.`/`Unsubscribed from notifications.`
- `/react :emoji:`, `/award :emoji:` upd `emoji_award` (name), `Toggled :name: emoji reaction.` (only if `user_can_award?`)
- `/shrug`, `/tableflip` substitutions.
Issue+MR (issue_and_merge_request_actions.rb):
- `/assign @u...` (single assignee) upd `assignee_ids`, `Assigned @u.`; `/unassign [@u]` `Removed assignee @u.`; `/milestone %m` upd `milestone_id`, `Set the milestone to %m.`; `/remove_milestone`; `/copy_metadata <ref>` (labels+milestone from same-project issue/MR) upd `add_label_ids`,`milestone_id`; `/estimate`,`/estimate_time <1w 3d 2h 14m>` upd `time_estimate`, `Set time estimate to 1w 3d.`; `/remove_estimate`; `/spend`,`/spent`,`/spend_time <1h30m> [YYYY-MM-DD]` upd `spend_time {duration,user_id,spent_at,category,note_id}`, `Added 1h 30m spent time.`; `/remove_time_spent`; `/lock` upd `discussion_locked:true` `Locked the discussion.`; `/unlock` `Unlocked the discussion.`
- `/unlink <ref>` (Issue, MR) - `Removed linked item #1.` / `No linked item matches the provided parameter.`
MR only (merge_request_actions.rb):
- `/merge` (condition: persisted && `MergeOrchestrationService#can_merge?`): requires request param `merge_request_diff_head_sha` equal to `mr.diff_head_sha`, otherwise messages `The /merge quick action requires the SHA of the head of the branch.` / `Branch has been updated since the merge was requested.`; success msg `Merged this merge request.` (or `Set to auto-merge.`); upd `merge: <sha>`.
- `/rebase` (open, source branch exists, can push to it) upd `rebase:true`, msg `Scheduled a rebase of branch <branch>.`; failure msgs `This merge request branch is protected from force push.`, `This merge request is currently in an unmergeable state, and cannot be rebased.`, `A rebase is already in progress.`
- `/draft` upd `wip_event:"draft"` msg `Marked this merge request as a draft.` (or `No change to this merge request's draft status.`); `/ready` upd `wip_event:"ready"` `Marked this merge request as ready.`
- `/target_branch <branch>` upd `target_branch` if branch exists; `Set target branch to <b>.`
- `/submit_review [state]` (persisted): publishes the user's drafts via `DraftNotes::PublishService` (skipped if `review_id` param present i.e. during publish) msg `Submitted the current review.`; then `approve` -> approval (`Approved the current merge request.` / `Failed to approve the current merge request.`) or a reviewer state key (`reviewed` default, `requested_changes`, `approved`, `unreviewed`, ...) via UpdateReviewerStateService. No upd key.
- `/request_changes` (persisted && current user is a reviewer) -> `Changes requested to the current merge request.` (or the service error message).
- `/approve` (eligible_for_approval_by? && !merged): publishes pending drafts first (if any and no review_id) then `ApprovalService`; msgs `Submitted the current review.` (only if drafts were published) then `Approved the current merge request.`; `@execution_message[:approve]` is an Array that `map_commands` flat-maps, so the summary is the two sentences joined by one space. No message when approval fails (`next unless success`).
- `/unapprove` (eligible_for_unapproval_by? && !merged) `Unapproved the current merge request.`
- `/request_review @u`, aliases `/assign_reviewer`, `/reviewer` (needs `admin_merge_request`) upd `reviewer_ids`; for an already assigned reviewer performs a re-request (`RequestReviewService`) immediately; msg `Requested a review from @u.`; `/unassign_reviewer [@u]`, alias `/remove_reviewer` `Removed reviewer @u.`
- `/run_pipeline` (if `CreatePipelineService#allowed?`) `New pipeline has been triggered and will appear shortly.`
Only `Issue` has `/relate`, `/confidential`, `/severity`, etc. (not MR).

**REST response for commands**: see 1.1.1. Examples: body `/close` -> 202 `{"commands_changes":{"state_event":"close"},"summary":["Closed this merge request."]}`; body `thanks\n/close` -> 201 Note with `commands_changes:{"state_event":"close"}` and `body:"thanks"`; body `/approve` -> 202 `{"commands_changes":{},"summary":["Approved the current merge request."]}`. A command whose condition fails yields `summary:["Could not apply close command."]` in a 202.

---------------------------------------------------------------------------------------------------
## 8. Ordering, pagination, filters, internal

- Notes list: `order_by` default `created_at` (implemented as `id`), `sort` default **desc**, `per_page` 20 (max 100), `activity_filter` as in 1.1.
- Discussions list: ascending by first note `(created_at, id)`, no params except pagination; paging is by discussion, not note.
- Discussion notes: `created_at, id` asc.
- Draft notes: all drafts of the current user, no pagination (relation default order; ids ascending in practice).
- Award emoji: id asc, paginated.
- Internal notes in MRs: allowed. Visible to reporter+ (`read_internal_note`), hidden from guests/non-members in lists (silently dropped after pagination), counted in `X-Total`. Internal notes' replies must also be internal. Internal DiffNotes are rejected. Webhooks for internal notes use `confidential_note_hooks` with `event_type:"confidential_note"`. `internal` and `confidential` both present in the entity.
- `with_metadata` loads `system_note_metadata`; system notes referencing inaccessible items are hidden (`system_note_visible_for?`).

---------------------------------------------------------------------------------------------------
## 9. Side effects when creating/modifying notes

Synchronous (inside `Notes::CreateService`): validation -> quick actions -> `note.save` -> `update_discussions` (individual->discussion conversion) -> `after_save :touch_noteable` (MR `updated_at` = now for every note save incl. system notes unless `skip_touch_noteable`, and on resolve/edit; **not** for `Note.resolve!` bulk update_all) -> `keep_around_commit` (note commit refs) -> `TodoService#new_note` -> `clear_noteable_diffs_cache` -> `Suggestions::CreateService` -> usage counters -> for first note of an MR thread: capture diff note positions; merge-status GraphQL trigger if the note needs resolving.
- Todos (`TodoService#handle_note`, todo_service.rb:392): skipped for system notes; resolves (marks done) the author's pending todos on the **MR** (individual note) or on the **discussion** (thread note); then `create_mention_todos`: `directly_addressed` todos for users addressed at the start of a line/body (`@user text`) and `mentioned` todos for other `@user`/group mentions, excluding the author, users who can't read the MR, and (for internal notes) users who can't read internal notes. Assignment/review-request todos are separate.
- Async (`NewNoteWorker` after commit): `NotificationService#new_note` (unless `skip_notification?`), then `Notes::PostProcessService`: `EventCreateService#leave_note` (user `commented` event), `create_cross_references!`, **note webhook**: `Gitlab::DataBuilder::Note.build(note, author, :create)` -> `note_hooks` or `confidential_note_hooks` -> `execute_hooks` and `execute_integrations`, group-mention integrations. Payload: `object_kind:"note"`, `event_type:"note"|"confidential_note"`, `user`, `project_id`, `project`, `object_attributes` (id, note, noteable_type, noteable_id, author_id, created_at, updated_at, project_id, attachment, line_code, commit_id, system, st_diff, internal, type, position, original_position, change_position, resolved_at, resolved_by_id, resolved_by_push, discussion_id, updated_by_id, description, url, action:"create"), `repository`, and `merge_request` (MR hook attrs).
- System notes: no webhooks/todos/events (PostProcess skips).
- Update (`Notes::UpdateService`): note hook with `action:"update"` only if the text changed and not commands-only; `update_todos` re-runs mention todos for newly added mentions; `last_edited_at` and `updated_by` set when the text changed (`edited?`); suggestions re-parsed.
- Delete: todos destroyed (`TodoService#destroy_target`), no note hook.
- `user_notes_count` of the MR is computed live (`notes.user.count`, non-system, includes internal notes) and so changes on create/delete.
- Resolving/unresolving: `noteable.broadcast_notes_changed`, MR merge-status GraphQL triggers, `ResolvedDiscussionNotificationService` (system note `resolved all threads` + MR hook `update`).
- Awards: see section 6 (emoji hook, todos done, note `updated_at` bump).
- Merge request hooks fired by this area: only `action:"update"` from all-threads-resolved and from re-request review (reviewer state `unreviewed`, with `re_requested_reviewer_id`). Note creation itself fires only the note hook.

---------------------------------------------------------------------------------------------------
## 10. Edge cases checklist

1. **Edit permissions** (`admin_note`): granted to note author, project maintainer+ (role file), admins; denied if `!editable?` (system note), if the user can't read the MR, if internal and user can't read internal notes. A developer editing another user's note -> 403 `403 Forbidden`. Locked MR: `admin_note` prevented for non-members (IssuablePolicy:49-52).
2. **Editing/deleting system notes**: 403 (PUT and DELETE) because `admin_note` is prevented via `~editable`; resolve of a system note: 400 `Note is not resolvable` only for users that pass `resolve_note` (system notes: author is the actor so `is_author` may pass -> then `resolvable?` false -> 400).
3. **Deleting the first note of a thread**: allowed; remaining replies keep the same `discussion_id` and the first remaining note becomes `first_note` (so `resolvable` etc. recomputed from it; if the first remaining note is a reply DiscussionNote the thread stays a thread). Deleting a DiffNote's first note leaves replies carrying their own copy of `position` (reply_attributes copy), so the thread keeps its position. Deleting all notes removes the discussion.
4. **PUT discussion note with `resolved`**: only resolves that note, not the thread; the thread flips to `resolved:true` only when every resolvable note is resolved. `exactly_one_of` forbids sending `body` and `resolved` together.
5. **Resolving non-resolvable**: note endpoint -> 400 `Note is not resolvable`; discussion endpoint -> 403 `403 Forbidden`.
6. **Locked MR** (`discussion_locked`): non-members: `create_note` -> 403 (POST notes/discussions/replies/drafts publish `Not allowed to create notes`), award -> 404 `Award Emoji Not Found`, resolve denied. Maintainers/members unaffected. Locking affects system notes? No, system notes are created regardless.
7. **Closed/merged MR**: notes, awards, drafts are all still allowed (no state check).
8. **Note length**: byte size > `description_and_note_max_size` (default 1,048,576) -> 400 `Note {:note=>["is too long (X). The maximum size is 1 MB."]}` where X = `number_to_human_size(bytes)` (e.g. `1.1 MB`). Applies to updates and drafts (draft message key is `note` without the `Note ` prefix).
9. **Emoji on system notes**: 404 `Award Emoji Awardable cannot add emoji reactions`; reading awards of system notes works if visible.
10. **Replies to outdated diff notes**: allowed; the reply copies `original_position`, `position`, `line_code` of the thread (DiffDiscussion#reply_attributes) and so is a DiffNote with identical position (even if the discussion is outdated; `change_position` is not copied). Replies to a resolved thread are created resolved (then un-resolve semantics differ for draft replies, 4.3).
11. **Individual note -> thread conversion** changes the first note's `type` from null to `DiscussionNote`: subsequent GETs show `type:"DiscussionNote"`, `resolvable:true`, `resolved:false` and discussion `individual_note:false`.
12. **System-note discussions**: system notes can't be replied to (400 `Replies to system notes are not allowed.`), but the `outdated` system note shares the diff discussion id and appears inside that discussion's `notes` (type DiffNote, system true, resolvable false).
13. **`created_at` parameter** ignored for non-owners without error. `updated_at` is forced equal to `created_at` when kept.
14. **`confidential`/`internal` without permission** ignored silently; `internal:true` on reply to non-internal thread ignored (reply takes the parent's value); reply to internal thread by user without `mark_note_as_internal` -> 400 `Note {:base=>["Discussion to reply to cannot be found"]}` (via BuildService) - when invoked through the discussions route the first_note check (readable) already returns 404 `Discussion Not Found` for users who can't read internal notes.
15. **Position without `base_sha`**: allowed (type String required in Grape; nil not possible) - but shas must be strings; diff refs completeness only needs start/head.
16. **Duplicate emoji POST**: 404 (not 409/400) with `Name has already been taken`.
17. **Another user's award delete**: 401 `401 Unauthorized`.
18. **Draft reply uniqueness**: second reply draft by same user on same thread: 400 `{"message":{"author_id":["has already been taken"]}}` (key order: `errors.messages`).
19. **Publishing drafts as non-reviewer**: works, no reviewer state change; `review_started` only for actual reviewers.
20. **Bulk publish ordering & grouping**: notes created in draft id order, all sharing one `Review`; top-level drafts become separate threads (no merging); reply drafts join their threads.
21. **Suggestions on removed lines** are not created; multi-line suggestions anchored on the end line (`new_line`) of the comment range.
22. **`GET /notes` readability filter after pagination** produces short pages; `X-Total-Pages` unaffected.
23. **Quick action in a 202**: `Note` is not persisted, no note hook, no `user_notes_count` change, MR may still be updated (and `updated_at` touched via MR update).
24. **Webhooks on update-to-commands-only**: the edited note is deleted (`Notes::DestroyService`) and 202 returned.
25. **Error body shapes differ**: note-model errors are STRING-formatted Ruby hashes inside the message (`"400 Bad request - Note {:note=>[\"can't be blank\"]}"`, with Ruby 3.3 inspect: `{:note=>["can't be blank"]}`, double quotes inside JSON are escaped); draft-note errors are real JSON objects (`{"message":{"note":[...]}}`).
