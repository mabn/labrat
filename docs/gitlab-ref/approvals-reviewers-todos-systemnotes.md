# GitLab 18.11 CE - MR approvals, reviewers/assignees, todos, system notes, resource events

Source root: the GitLab CE 18.11 source copy (all paths below are relative to it). Line numbers are 18.11.

Confidence tags: [C] read directly in code; [I] inferred by combining code paths (no runtime verification).

---------------------------------------------------------------------------------------------------

## 1. MR approvals

### 1.0 Model

CE has a single, rule-less approval model: an MR is `approved?` when it has at least one `approvals` row (`app/models/concerns/approvable.rb`: `approved?` = `approvals.present?`). There is no approvals-required count. Pushes do not remove approvals. Approvals also survive MR close/reopen.

### 1.1 Endpoint inventory (all under `/api/v4/projects/:id/merge_requests/:merge_request_iid`)

| Method + path | File:line | Behavior |
|---|---|---|
| GET `/approvals` | `lib/api/merge_request_approvals.rb:34` | entity `MergeRequestApprovals` (1.2) |
| POST `/approve` | `merge_request_approvals.rb:63` | see 1.3 |
| POST `/unapprove` | `merge_request_approvals.rb:93` | see 1.4 |
| PUT `/reset_approvals` | `merge_request_approvals.rb:114` | see 1.5 |

Anonymous: `before { authenticate_non_get! }` => 401 `{"message":"401 Unauthorized"}` for non-GET. GET on public project works anonymously.
MR not found: 404 `{"message":"404 Not found"}` (find_by! => RecordNotFound handler; standard labrat 404).

### 1.2 GET /approvals - response (`MergeRequestApprovals` entity)

`lib/api/entities/merge_request_approvals.rb`. Field order exactly:

1. `user_has_approved` = `merge_request.approved_by?(current_user)` (false for nil user)
2. `user_can_approve` = `merge_request.eligible_for_approval_by?(current_user)` (same predicate as POST /approve, see 1.3)
3. `approved` = `merge_request.approvals.present?`
4. `approved_by` = array of `{user: UserBasic, approved_at}` (entity `Approvals`, `lib/api/entities/approvals.rb`: `user` using UserBasic, `created_at` renamed `approved_at`) from `merge_request.approvals` (order = association order, i.e. approval id asc)

UserBasic field order (`lib/api/entities/user_basic.rb`, `user_safe.rb`): `id, username, public_email, name, state, locked, avatar_url, web_url` (+`avatar_path` only if `only_path` option; `custom_attributes` only with flag). `name` goes through `redacted_name`.

### 1.3 POST /approve

Params: `sha` (String, optional), `publish_review` (Boolean, optional).
Flow (`lib/api/merge_request_approvals.rb:63-83`), in order:

1. `find_merge_request_with_access(iid, :approve_merge_request)`: 404 if no MR; **403 `{"message":"403 Forbidden"}`** if user lacks `:approve_merge_request` (`lib/api/helpers.rb:379-383,427-429`).
2. `check_sha_param!`: if `sha` given and `!= merge_request.diff_head_sha` => **409** `{"message":"SHA does not match HEAD of source branch: <diff_head_sha>"}` (`lib/api/helpers.rb:600-604`).
3. If `publish_review` truthy: `DraftNotes::PublishService`; on failure **500** `{"message":"Failed to publish review"}`.
4. `MergeRequests::ApprovalService#execute` (`app/services/merge_requests/approval_service.rb`). Returns nil/falsy => **401** `{"message":"401 Unauthorized"}` (no reason suffix). Falsy when:
   - `merge_request.eligible_for_approval_by?(user)` false (includes: already approved by this user = "approve twice => 401");
   - MR already merged (`return if merge_request.merged?`) - closed MRs CAN be approved.
5. Success => entity `MergeRequestApprovals` as in 1.2, status **201**.
   NOTE: `Approval.safe_ensure_unique { approval.save }`; if save fails (race/unique), the service still returns `success` and nothing else happens (`return success unless save_approval(approval)`), so the API returns 201 with no new approval.

Eligibility [C]: `Approvable#eligible_for_approval_by?` (`app/models/concerns/approvable.rb:63-65`):
`user.present? && !approved_by?(user) && user.can?(:approve_merge_request, self)`.
=> **No author check and no committer check.** The MR author (if Developer+ member) CAN approve their own MR via the API (201), and `user_can_approve` for that author is true.

Policy `:approve_merge_request` (`app/policies/merge_request_policy.rb:20-22,51-53`): `can?(:update_merge_request) && subject_container.member?(user)` (project member incl. inherited/invited; `update_merge_request` roughly Developer+, or author/assignee that can read; Guests/Reporters/non-members get 403). Prevented when MR `hidden & ~admin` (line 36).
Admin: `can?(:update_merge_request)` true for admin, but `member?` requires actual membership (`Project#member?`); admins who are not members: [I] 403 (member? checks the team, not admin) - verify.

Side effects of a successful approve (`approval_service.rb`):
- `merge_request.approvals.new(user:, patch_id_sha: merge_request.current_patch_id_sha)` row.
- `update_reviewer_state(mr, user, 'approved')`: `UpdateReviewerStateService` - only effective if the approver is already a reviewer of the MR (see 2.4); errors ("Reviewer not found" / "Reviewer has approved") are ignored.
- GraphQL triggers (merge status updated, approval state updated); usage counters.
- Publishes `MergeRequests::ApprovedEvent` {current_user_id, merge_request_id, approved_at} consumed asynchronously (`lib/gitlab/event_store/subscriptions/merge_requests_subscriptions.rb`):
  - `CreateApprovalEventWorker` -> `event_service.approve_mr` => creates a user **Event** (target_type MergeRequest, action `approved` = Event action 10) (activity feed `/events`);
  - `CreateApprovalNoteWorker` -> `SystemNoteService.approve_mr` => system note body **`approved this merge request`**, `system_note_metadata.action = "approved"`, author = approver;
  - `ResolveTodosAfterApprovalWorker` -> `TodoService#resolve_todos_for_target(mr, approver)`: marks the approver's pending todos on that MR as done (resolved_by_action `system_done`);
  - `ExecuteApprovalHooksWorker` -> notification email `approve_mr` + merge_request webhook (see 1.7).
  Note these are async (sidekiq); a labrat implementation may do them inline but order of note/event vs response is not observable synchronously in GitLab.
- No todo is created for anyone on approve.

### 1.4 POST /unapprove

`find_merge_request_with_access(iid, :approve_merge_request)` => 403 if no permission. Then `RemoveApprovalService` (`app/services/merge_requests/remove_approval_service.rb`). Returns nil (=> **404** `{"message":"404 Not Found"}` via `not_found!`, line 100) when the user has no approval on the MR, or the MR is merged. Note: 404, NOT 401 (opposite of /approve). Success => `MergeRequestApprovals` entity, status **201**.
Effects: delete the user's approval rows; reviewer state -> `unapproved` (if reviewer, via `UpdateReviewerStateService`); system note **`unapproved this merge request`** (action `unapproved`) created SYNCHRONOUSLY in the service (unlike approve); notification `unapprove_mr` (async); webhook (1.7); GraphQL triggers. No todos.

### 1.5 PUT /reset_approvals

`find_project_merge_request` (read access; 404), then `unauthorized!` (**401**) unless `current_user.can?(:reset_merge_request_approvals, mr) && !mr.merged?`. `:reset_merge_request_approvals` is enabled only for **bot users** that can approve (`merge_request_policy.rb:24-26`) => ordinary users always get **401 `{"message":"401 Unauthorized"}`**. Bot success: `approvals.delete_all`, response **202** with empty body. No notes, no webhooks, no reviewer state change.

### 1.6 Who may approve - summary table

| Actor | POST /approve | `user_can_approve` in GET |
|---|---|---|
| Guest/Reporter/non-member | 403 | false |
| Developer+ (not yet approved) | 201 | true |
| Author with Developer+ | 201 | true |
| Already approved | 401 | false (`user_has_approved` true) |
| Any user, MR merged | 401 | n/a |
| Any user, MR closed | 201 | true |

### 1.7 Webhooks for approvals (merge_request events)

Payload built by `MergeRequests::BaseService#hook_data` (`app/services/merge_requests/base_service.rb:20-36`) via `Gitlab::DataBuilder::Issuable`: `object_kind: "merge_request"`, `event_type: "merge_request"`, `user`, `project`, `object_attributes{... , action, actioned_at, system:false}`, `labels`, `changes`, `repository`, `assignees` (only if any), `reviewers` (only if any; each = user hook_attrs + `state` + `re_requested`).
Action values relevant here:
- approve: `ExecuteApprovalHooksService` (`app/services/merge_requests/execute_approval_hooks_service.rb`): notification `approve_mr` + `execute_hooks(mr, 'approved')`, for every approval (**always `action: "approved"`**).
- unapprove: `RemoveApprovalService` => `execute_hooks(mr, 'unapproved')` + notification `unapprove_mr` (unless `skip_notification`).
Hook delivery: `project.execute_hooks(data, :merge_request_hooks)` + integrations.

### 1.8 Approval-related quick actions (for completeness)

`/approve` (condition: persisted, `eligible_for_approval_by?`, not merged; publishes pending draft notes first), `/unapprove` (`eligible_for_unapproval_by?` = approved_by && can approve; not merged), `/submit_review [state]` (state `approve` -> ApprovalService; any `MergeRequestReviewer.states` key -> `UpdateReviewerStateService`; default `reviewed`), `/request_changes` (requires being a reviewer) (`lib/gitlab/quick_actions/merge_request_actions.rb:194-310`).

---------------------------------------------------------------------------------------------------

## 2. Reviewers and assignees

### 2.1 Data model

- `merge_request_assignees` (user_id, merge_request_id, created_at...) and `merge_request_reviewers` (id, user_id, merge_request_id, project_id, state, created_at) (`app/models/merge_request.rb:139-142`, `app/models/merge_request_reviewer.rb`).
- `MergeRequestReviewer.state` enum (`app/models/concerns/merge_request_reviewer_state.rb`): `unreviewed: 0, reviewed: 1, requested_changes: 2, approved: 3, unapproved: 4, review_started: 5`. Default (new reviewer) = `unreviewed`.
- `MergeRequest#allows_reviewers?` true. `allows_multiple_assignees?` = `project.allows_multiple_merge_request_assignees?` => **false**; `allows_multiple_reviewers?` => **false** (`app/models/merge_request.rb:2638-2644`, `app/models/project.rb:3726-3733`). A merge request has at most one assignee and one reviewer.

### 2.2 State transitions

All through `MergeRequests::UpdateReviewerStateService#execute(mr, state)` (`app/services/merge_requests/update_reviewer_state_service.rb`):
- requires `can?(current_user, :update_merge_request, mr)` else error "Invalid permissions";
- finds reviewer row for current_user; none => error "Reviewer not found" (and for `requested_changes`/`approved`/`reviewed` side effects still run BEFORE this check - see below);
- `state == 'requested_changes'`: system note **`requested changes`** (action `requested_changes`) + merge-status trigger;
- `state == 'reviewed'`: system note **`left review comments`** (action `reviewed`), except for the Duo code review bot;
- if reviewer exists: `return error("Reviewer has approved") if reviewer.approved? && state not in [requested_changes, unapproved]`; `reviewer.update(state:)`; GraphQL triggers; cache invalidation; for `requested_changes` additionally removes the user's approval if they had one (`remove_approval` with `skip_system_note, skip_notification, skip_updating_state`; so NO `unapproved this merge request` note in that case).
Transitions seen in code:
- ApprovalService -> `approved`; RemoveApprovalService -> `unapproved`;
- review submit (GraphQL/quick action `/submit_review`) -> `reviewed` (default) / `requested_changes` / `approved`;
- RequestReviewService (re-request) -> `unreviewed` (2.5);
- adding a reviewer who already has an approval -> `approved` (`set_reviewers_approved`, `base_service.rb:227-232`).
`review_started` is set by the draft-notes flow (`app/services/draft_notes/create_service.rb`, `destroy_service.rb`) - not detailed here.

### 2.3 REST exposure

`GET /projects/:id/merge_requests/:merge_request_iid/reviewers` (`lib/api/merge_requests.rb:469-485`): read access; `Kaminari.paginate_array(merge_request.merge_request_reviewers)` (default association order = id asc); entity `MergeRequestReviewer` (`lib/api/entities/merge_request_reviewer.rb`): `{ "user": UserBasic, "state": "unreviewed", "created_at": ... }` in that order. Standard pagination headers.
MR entities: `reviewers` (array of UserBasic), `assignees`, `assignee` (first or null) (`lib/api/entities/merge_request_basic.rb:39-40`). There is no REST endpoint to change reviewer state or re-request review (only GraphQL mutations `mergeRequestReviewerRereview`, `mergeRequestRequestChanges`, and quick actions). Setting reviewers via REST = `reviewer_ids` on POST/PUT merge request.

Webhook reviewers entry (`app/models/concerns/issuable.rb:570-578`): `user.hook_attrs.merge(state:, re_requested: bool)`; `re_requested` true only for the reviewer in a re-request event. `changes.reviewers` = `{previous:[...], current:[...]}` when attrs differ (`issuable.rb:580-589`). Top-level `reviewers` key omitted when none (`lib/gitlab/data_builder/issuable.rb:30-35`). `object_attributes.reviewer_ids`, `assignee_ids`, `assignee_id` (deprecated first id) (`lib/gitlab/hook_data/merge_request_builder.rb`).

### 2.4 One assignee / one reviewer: how extra ids are handled (silent truncation, no error)

Generic path (POST/PUT /merge_requests through `MergeRequests::Create/UpdateService`):
- **assignees** (`app/services/issuable_base_service.rb:104-139`): for each of `assignee_ids`, `add_assignee_ids`, `remove_assignee_ids`: blank => untouched; `unless issuable.allows_multiple_assignees?` => `params[key] = params[key].first(1)` **before** read-permission filtering; then `User.id_in(ids).select { can read MR }`; if input is exactly `['0']` (`IssuableFinder::Params::NONE = '0'`, `app/finders/issuable_finder/params.rb:14`) => `[]` (unassign all); elsif filtered ids non-empty => use them; else the param is **deleted** (no change, no error). So `assignee_ids=[5,6]` -> only 5; `assignee_ids=[999(nonexistent),6]` -> first(1)=[999] -> no readable users -> param dropped -> no change. Note `first(1)` runs on the raw order given.
  Whole filter is skipped (params deleted) if the user cannot set issuable metadata (`filter_params` lines 88-96): non-authorized users' assignee params are silently dropped.
- **reviewers** (`app/services/merge_requests/base_service.rb:199-225`): `reviewer_ids` blank => untouched; if `!can_admin_issuable?` => param silently deleted; `first(1)` unless multiple allowed; read-permission filter; `['0']` => `[]`; none readable => param deleted.
- `assignee_id` (singular, deprecated) is not truncated by `filter_assignees` (it is not in the key list); the model setter `assignee_id=` wraps it as `assignee_ids = Array(id)` (`app/models/concerns/deprecated_assignee.rb:20-22`). `assignee_id=0` handling: [I] results in no matching user => unassigned; verify.
Specialized path: when the UpdateService params contain exactly one key and it is `assignee_ids`/`assignee_id`, `MergeRequests::UpdateAssigneesService` runs (`update_service.rb:330-346`): `filter_sentinel_values(ids).first(1)` where sentinel = reject `0` (`update_service.rb:389-391`). `UpdateReviewersService` same with `reviewer_ids`. In these services the permission check is `can?(:set_merge_request_metadata)`; `new_user_ids` aborts if a user can't read the MR (adds `errors`; [not read in detail]) and `return merge_request if new_ids.size != ids.size`.

### 2.5 Side effects of changing assignees

`HandleAssigneesChangeService` (`app/services/merge_requests/handle_assignees_change_service.rb`), run async via `HandleAssigneesChangeWorker` after the update:
1. system note via `SystemNoteService.change_issuable_assignees` (2.x notes below), action `assignee`;
2. notification `reassigned_merge_request`;
3. `AssignmentEventRecorder` -> `ResourceEvents::MergeRequestAssignmentEvent` rows (`merge_request.assignment_events`) - NOT exposed by any public REST API;
4. todos: `todo_service.reassigned_assignable(mr, current_user, old_assignees)` -> `create_assignment_todo`: for `assignees - old_assignees` create todo action **assigned** (author = the acting user; created even for self-assignment; skipped for users who already have a pending `assigned` todo on the MR);
5. cache invalidation; webhook `update` (when `execute_hooks` option set by the specialized path; in the full UpdateService path the generic update hook is fired with `old_associations: assignees`).
Note: assignment note and todos are created asynchronously.

Changing reviewers (`handle_reviewers_change`, `base_service.rb:81-99`): system note `change_issuable_reviewers` (action `reviewer`); notification `changed_reviewer_of_merge_request`; todos `reassigned_reviewable` -> for `reviewers - old_reviewers` todo action **review_requested** (author = acting user; dedupe on pending); `set_reviewers_approved` (state approved if that user already has an approval); first-reviewer-assigned metric. Also `resolve_todos_for(mr)`: ANY change of labels/assignees/reviewers makes the acting user's own pending todos on that MR done (async `ResolveTodosWorker`) (`update_service.rb:195-199,381-387`).
On MR **creation** with assignees/reviewers: no assignee/reviewer system note is created (notes only come from `handle_changes` on update); todos are: `AfterCreateService#prepare_merge_request` -> `todo_service.new_merge_request` => assigned todos for assignees, review_requested todos for reviewers, mention todos (`app/services/todo_service.rb` new_issuable) [C].

### 2.6 Re-request review (`RequestReviewService`, `app/services/merge_requests/request_review_service.rb`)

GraphQL/quick action `/request_review @user`; requires `:update_merge_request`; target must already be a reviewer else error "Reviewer not found". Steps: remove the reviewer's approval silently (`has_unapproved`), reviewer state -> `unreviewed`, notification `review_requested_of_merge_request`, todo **review_requested** (`create_request_review_todo`), system note via `SystemNoteService.request_review` (below), webhook `update` with `re_requested: true` for that reviewer and state change in `changes.reviewers`.

### 2.7 Assignee/reviewer system note texts (`app/services/system_notes/issuables_service.rb`)

All use `Gitlab::I18n.with_default_locale` (English) and `user.to_reference` = `@username`; `to_sentence` joins (1: `@a`; 2: `@a and @b`; 3+: `@a, @b, and @c`).
- assignees (`change_issuable_assignees`, :89-104), action `assignee`:
  - added only: `assigned to @alice`
  - removed only: `unassigned @alice`
  - both: `assigned to @bob and unassigned @alice`
  - (empty diff produces an empty-bodied note attempt; avoided by callers since assignees changed)
  - legacy single `change_assignee` (:66-72): `assigned to @alice` / `removed assignee`.
- reviewers (`change_issuable_reviewers`, :117-130), action `reviewer`:
  - `requested review from @alice`
  - `removed review request for @alice`
  - both: `requested review from @bob and removed review request for @alice`
- re-request (`request_review`, :132-138), action `reviewer`: `requested review from @alice`; with unapproval: `[ "requested review from @alice", "removed approval" ].to_sentence` => `requested review from @alice and removed approval`.

---------------------------------------------------------------------------------------------------

## 3. Todos API (`lib/api/todos.rb`)

All endpoints `authenticate!` => 401 anonymous.

| Route | Notes |
|---|---|
| `POST /projects/:id/merge_requests/:merge_request_iid/todo` (also `issues/:issue_iid/todo`) | lines 27-48. `find_merge_request_with_access` (404/403), `TodoService#mark_todo(mr, user).first`; present Todo entity (status 201) or **`not_modified!` => 304** (empty body) when a pending `marked` todo already exists (mark_todo `excluded_user_ids` dedupe; `marked` not in `ACTIONS_MULTIPLE_ALLOWED`). `create_todo` ability requires `~anonymous & can?(:read_merge_request)` (`merge_request_policy.rb:28-31`) |
| `GET /todos` | lines 109-121 |
| `POST /todos/:id/mark_as_done` | lines 135-143: `current_user.todos.find(id)` (404 otherwise); `resolve_todo(todo, user, resolved_by_action: :api_done)`: no-op if already done; sets `state: done`, `snoozed_until: nil`. Returns the todo entity (state `done`), status 201 |
| `POST /todos/mark_as_done` | lines 145-151: uses the same filter params as the list (`find_todos`), `resolve_todos(..., resolved_by_action: :api_all_done)`, **204** empty |

### 3.1 List filters (`todo_filters`, lines 53-60)
`action` (enum of `Todo.action_names.values` as strings), `author_id` (Integer), `project_id` (Integer), `group_id` (Integer), `state` (`pending` | `done`; default = pending only; `all` is NOT accepted by the API even though the finder supports it), `type` (one of `TodosFinder.todo_types`: `Commit Issue WorkItem MergeRequest DesignManagement::Design AlertManagement::Alert Namespace Project Key WikiPage::Meta`), plus pagination (`page`, `per_page`). Invalid enum value => 400 `{"message":"action does not have a valid value"}` style (Grape validation).
Finder (`app/finders/todos_finder.rb:44-58`): for user; hides todos of banned authors for pending/all-default states; filter by action id; `author_id` (`'0'` string meaning none is only for GraphQL); state (pending also excludes snoozed: `not_snoozed`); target_id; types; `group_id` => todos in that group **and descendants**; `project_id` => `for_undeleted_projects.for_project`. Sort: default `id_desc` (newest id first). After pagination, `Todos::AllowedTargetFilterService` removes todos whose target project/target is unreadable (`can?(:read_todo)`), so a page can contain fewer than `per_page` items while X-Total headers still count the pre-filter set.
`Todo.action_names` order / values (`app/models/todo.rb:12-45`): ids -> names: 1 assigned, 9 review_requested, 2 mentioned, 3 build_failed, 4 marked, 5 approval_required, 6 unmergeable, 7 directly_addressed, 10 member_access_requested, 11 review_submitted, 14 ssh_key_expired, 15 ssh_key_expiring_soon. `ACTIONS_MULTIPLE_ALLOWED` (duplicates allowed while pending) = mentioned, directly_addressed, member_access_requested.

### 3.2 Todo entity (`lib/api/entities/todo.rb`), field order
`id`, `project` (ProjectIdentity: `id, description, name, name_with_namespace, path, path_with_namespace, created_at`; only when `project_id`), `group` (NamespaceBasic; only when `group_id`), `author` (UserBasic), `action_name`, `target_type` (e.g. `"MergeRequest"`), `target` (full `API::Entities::MergeRequest` entity, rendered with issuable_metadata from `Gitlab::IssuableMetadata` and `include_subscribed: false`), `target_url`, `body`, `state` (`pending`|`done`), `created_at`, `updated_at`.
- `body`: `note.note` if the todo has a note, else `target.title` (`app/models/todo.rb:345-353`).
- `target_url` for MR (`build_merge_request_target_url`, :469-479): `polymorphic_url([project, mr], anchor: "note_<note_id>" or nil)`, i.e. `http://host/<ns>/<proj>/-/merge_requests/<iid>` (+ `#note_N`); for `build_failed` the path is prefixed `:pipelines` => `.../-/pipelines/...`-style polymorphic route (`pipelines_project_merge_request_url`, i.e. `/<ns>/<proj>/-/merge_requests/<iid>/pipelines`).
- No `snoozed_until` / `resolved_by_action` field in the REST entity.

### 3.3 When MR todos are created / resolved (`app/services/todo_service.rb`)

Created:
- `assigned` - MR created with assignees; assignees added (above). Author = actor.
- `review_requested` - MR created with reviewers; reviewers added; re-request.
- `mentioned` / `directly_addressed` - MR description/title mentions on create (`new_issuable` -> `create_mention_todos`) and on title/description update (`update_merge_request`; skipped when only task checkboxes toggled); in notes (`new_note`/`update_note`, `handle_note`) with `note` attached. A user is only todo'd if they can read the MR; the actor is excluded from mentions (`mentioned_users(author)`). `directly_addressed` = `@user` at the start of a line/ sentence style (first-line mention) - `target.directly_addressed_users`.
- `build_failed` - head pipeline fails: `merge_request_build_failed` -> for each `merge_participants` (author + merge_user when auto-merge) (`AddTodoWhenBuildFailsService`).
- `unmergeable` - auto-merge aborted/MR became unmergeable (`abort_auto_merge_with_todo`, `base_service.rb:324-330`) for `merge_participants`.
- `marked` - API/UI "add a to-do".
- `review_submitted` - `new_review`: when a reviewer submits a review (draft notes publish), todo for `(assignees | author) - reviewer` (`create_review_submitted_todo`).
- `approval_required`: no creator found in the 18.11 CE source.
Resolved (done, `resolved_by_action: system_done`):
- `resolve_todos_for_target(mr, user)` (resolves only `user`'s pending todos on the MR, any action) is called on: MR close (`close_merge_request`), merge (`merge_merge_request`), push to the MR source branch (`merge_request_push`, in `RefreshService#...` line 296 - for the pusher), new note on the MR (the note author's), award emoji (`new_award_emoji`), review submission (`new_review`), approve (`ResolveTodosAfterApprovalWorker`), and label/assignee/reviewer change (async, acting user), task-list toggle (`handle_task_changes`).
- Build retried: `merge_request_build_retried` resolves todos for each merge participant.
- `restore_*` can set back to pending (UI only).

---------------------------------------------------------------------------------------------------

## 4. System notes on merge requests

Plumbing: `SystemNoteService` (`app/services/system_note_service.rb`) -> `SystemNotes::*Service#create_note` (`app/services/system_notes/base_service.rb:19-26`): `Note.create(noteable:, project:, author:, note: body, created_at: noteable.system_note_timestamp, system: true, system_note_metadata: SystemNoteMetadata{action, commit_count?, description_version?})` (`app/services/note_summary.rb`). Returned by the notes API with `system: true`, `type: null`, `body` = stored `note` text (markdown/HTML as stored), `noteable_type: "MergeRequest"`, `noteable_iid`. System notes are `individual_note`s (no discussion thread). `commands_changes: {}`.
`system_note_metadata.action` is NOT in the REST note entity (`lib/api/entities/note.rb`) - only stored.

### 4.1 CRITICAL: what is a ResourceEvent (NOT returned by notes API)

These are stored in dedicated tables, shown as synthetic notes only in the UI discussions feed, **absent from `GET /projects/:id/merge_requests/:iid/notes`** (and from `/discussions`):
- `resource_state_events`: **closed, reopened, merged** (`SystemNoteService.change_status` -> `ResourceEvents::ChangeStateService`; called from `MergeRequests::BaseService#create_note` (close: `close_service.rb:19`, reopen: `reopen_service.rb:10` with state `'reopened'`) and `PostMergeService#create_note` (merge)). No Note row is created for these.
- `resource_label_events`: labels added/removed (`Issuable::CommonSystemNotesService#create_labels_note` -> `ResourceEvents::ChangeLabelsService`). No `added ~label` Note exists.
- `resource_milestone_events`: milestone set/changed/removed (`create_milestone_change_event` -> `ChangeMilestoneService`). No milestone Note.
- Also resource events (not notes, not REST-exposed here): `resource_events_merge_request_assignment_events` (assignment history).
Synthetic note texts used by UI (for reference, also what an exported-feed would show):
- state (`app/models/state_note.rb:28-48`): body = event.state (`closed`, `merged`, `reopened`, `locked`), + ` with <gfm_reference>` when the event has a source MR/commit (`closed with merge request !5`), `merged manually` if merged with commit source.
- labels (`app/models/label_note.rb`): `added ~1 ~2 labels`/`added ~1 label`, `removed ~3 label`, joined `added ~1 label and removed ~2 label`; deleted labels: `added 1 deleted label`, `added ~1 + 1 deleted label` style (labels sorted by name).
- milestone (`app/models/milestone_note.rb`): `changed milestone to %5` (project: `%iid` full/absolute reference) / `removed milestone %5`.

### 4.2 Real Note-based system notes possible on an MR (exact text, action)

(`ref` = project `to_reference`; `@user` = to_reference)

| # | Trigger / source | Body (exact) | metadata.action |
|---|---|---|---|
| 1 | Push adds commits (`merge_requests/refresh_service.rb:342`; `commit_service.rb:11-20`) | see 4.3 | `commit` (+`commit_count` = total) |
| 2 | Title change (`issuables_service.rb:178-192`) | see 4.4 (HTML) | `title` |
| 3 | Draft toggle via title (`merge_requests_service.rb:29-35`) | `marked this merge request as **draft**` / `marked this merge request as **ready**` | `title` |
| 4 | Draft from WIP commit on push (`:37-41`) | `marked this merge request as **draft** from <commit.to_reference(project)>` (e.g. `from 1a2b3c4d`) | `title` |
| 5 | Description changed (`:223-229`) | `changed the description` | `description` (+`description_version_id` if saved) |
| 6 | Task toggled (`:298-306`) | `marked the checklist item **<escaped text>** as completed` / `... as incomplete`; table items: `marked the task table item **...** as ...` (`Taskable::COMPLETED/INCOMPLETE`) | `task` |
| 7 | Assignees changed | see 2.7 | `assignee` |
| 8 | Reviewers changed / re-request | see 2.7 | `reviewer` |
| 9 | Approved (async worker) | `approved this merge request` | `approved` |
| 10 | Unapproved | `unapproved this merge request` | `unapproved` |
| 11 | Requested changes | `requested changes` | `requested_changes` |
| 12 | Submitted review | `left review comments` | `reviewed` |
| 13 | Target branch changed (`update_service.rb:279`, `merge_requests_service.rb:97-109`) | `changed target branch from \`old\` to \`new\`` | `branch` |
| 14 | Target branch deleted -> retargeted | `deleted the \`old\` branch. This merge request now targets the \`new\` branch` | `branch` |
| 15 | Source branch deleted/restored via push (`refresh_service.rb:326`, `:122-133`) | `deleted source branch \`feature\`` / `restored source branch \`feature\`` | `branch` |
| 16 | Discussion lock toggled (`issuables_service.rb:444-455`) | `locked the discussion in this merge request` / `unlocked the discussion in this merge request` (`noteable.class.to_s.titleize.downcase`) | `locked` / `unlocked` |
| 17 | Cross reference "mentioned in" (`:246-271`, `Mentionable#create_cross_references!`) | `mentioned in <gfm_reference>`: other MR => `mentioned in merge request !7` (cross-project `mentioned in merge request group/proj!7`), issue => `mentioned in issue #3`, commit => `mentioned in commit abc1234`. Note is created ON the mentioned object, authored by the mentioner; not created if same text exists or MR contains the commit (`cross_reference_disallowed?`) | `cross_reference` |
| 18 | All threads resolved (`resolved_discussion_notification_service.rb:8`) | `resolved all threads` | `discussion` |
| 19 | Thread continued in issue (`:49-56`) | `created #12 to continue this discussion` (a REPLY in the discussion, `Note` with discussion attrs) | `discussion` |
| 20 | Diff discussion outdated (`:58-82`) | `changed this line in [version 2 of the diff](/ns/proj/-/merge_requests/1/diffs?diff_id=..&start_sha=..#<line_code>)` or `changed this file in ...`; plain `changed this line in version 2 of the diff` when no version params. Reply inside the diff discussion | `outdated` |
| 21 | Auto-merge enabled (`merge_requests_service.rb:6-10`, `auto_merge/merge_when_checks_pass_service.rb:62-70`) | `enabled an automatic merge when all merge checks for <sha> pass` (sha = `merge_params['sha']`, full 40-char) | `merge` |
| 22 | Auto-merge cancelled (`:12-17`) | `canceled the automatic merge` | `merge` |
| 23 | Auto-merge aborted (`:19-27`) | `aborted the automatic merge because <reason with first char lowercased>` e.g. `aborted the automatic merge because target branch was changed`, `... merge request was closed` | `merge` |
| 24 | Time estimate (`time_tracking_service.rb`) | `added time estimate of 1h` / `changed time estimate to 2h from 1h` / `removed time estimate of 1h` | `time_tracking` |
| 25 | Time spent (`/spend`, `POST add_spent_time`, Timelogs) | `added 1h 30m of time spent at <spent_at>` / `subtracted 30m of time spent at <spent_at>` / `removed time spent` (reset) / `deleted 1h of spent time from <spent_at>`; `<spent_at>` = `spent_at.in_time_zone(author tz)` interpolated with Ruby `to_s` (`2026-11-07 00:00:00 UTC` or `... +0100`), default `DateTime.current` | `time_tracking` |
| 26 | Cherry-pick note on commit/MR (`merge_requests_service.rb:154-163`) | `picked the changes into the branch [\`branch\`](/ns/proj/-/tree/branch) with commit <sha>` (+ `commit_id` set) | `cherry_pick` |
| 27 | Issue -> MR link (note on the ISSUE, not MR) | `created merge request !3 to address this issue` | `merge` |

Not produced for MRs: due-date notes (MRs have no due date). MR-related relate/unrelate notes exist only for issues. No system note is created for: rebase, squash, merge (state event only), close/reopen (state event), pipeline events, "removed source branch on merge".
There is no "source branch will be deleted" or "set to merge when pipeline succeeds" legacy note in 18.11 (`AutoMerge::STRATEGIES = [merge_when_checks_pass]`, `app/services/auto_merge_service.rb:6-9`); MWPS API param is mapped onto the checks-pass strategy.

### 4.3 add_commits format (`app/services/system_notes/commit_service.rb:11-20,60-133`)

Body = parts joined by `"\n\n"`:
1. `added N commit` / `added N commits` (N = new + existing counts, `pluralize`)
2. an HTML list: `<ul>` + existing-summary `<li>` + new commit `<li>`s `</ul>` (built with `content_tag`, so html as one line, no whitespace):
   - each new commit: `<li>{short_id} - {commit.title}</li>` (short_id = 8 chars; title is plain, content_tag escapes `&<>"'`).
   - more than `NEW_COMMIT_DISPLAY_LIMIT = 10` new commits: first `n-10` collapsed to one line `<li>{ids} - {k} earlier commit(s)</li>` where ids = single `short_id` if k==1, `{trunc(start_rev)}...{last_short_id}` when start_rev present (start_rev = oldrev if no existing commits else last existing commit id; `Commit.truncate_sha` = 8 chars) else `{first_short}..{last_short}` (two dots); then the last 10 as individual `<li>`s.
   - existing commits (from other branches already in MR): `<li>{ids} - {count} commit(s) from branch <code>{target_branch}</code></li>` ; `{ns}:{branch}` prefix for fork MRs (`target_project_namespace:branch`); ids = `short_id` if 1, `{trunc(oldrev)}...{last_short}` if oldrev non-blank else `{first}..{last}`.
3. `[Compare with previous version](/ns/proj/-/merge_requests/<iid>/diffs?diff_id=<current merge_request_diff.id>&start_sha=<oldrev>)` (relative path; `start_sha` is the full 40-char oldrev, query params in that order).
Example: `added 1 commit\n\n<ul><li>a1b2c3d4 - Fix it</li></ul>\n\n[Compare with previous version](/g/p/-/merge_requests/1/diffs?diff_id=5&start_sha=<40-hex>)`.
metadata: action `commit`, `commit_count`=N. Created only when `@commits.present?`; for a branch-deleted push no commit note, instead item 15.

### 4.4 Title change note (`issuables_service.rb:178-192`)

Created only if the *draftless* title changed (`Issuable::CommonSystemNotesService#create_title_change_note`, :84-91; draft toggle creates note #3 first, then the title note if the rest of the title also changed).
Body stored (and returned) is literal HTML, **not** markdown `**{-old-}**`:
`<p>changed title from <code class="idiff">{old_marked}</code> to <code class="idiff">{new_marked}</code></p>`
where each side is the HTML-escaped (`ERB::Util.html_escape`) title with changed character ranges wrapped by `Gitlab::Diff::InlineDiffMarker`: `<span class="idiff left right deletion">..</span>` on the old side, `<span class="idiff left right addition">..</span>` on the new side. Class list = `idiff` + `left` (first marked range) + `right` (last marked range) + mode (`deletion`/`addition`); so with several ranges the first has `idiff left <mode>`, the middle `idiff <mode>`, the last `idiff right <mode>`; a single range has `idiff left right <mode>`.
Ranges come from `Gitlab::Diff::CharDiff` = diff-match-patch `diff_main` + `diff_cleanupSemantic` (`lib/gitlab/diff/char_diff.rb`), char (not byte) indices. If old/new identical text the ranges are empty and the text is returned unmarked. (The markdown `{-..-}{+..+}` marker class `InlineDiffMarkdownMarker` exists but is NOT used for this note.)

### 4.5 system_note_metadata.action value set (valid)
CE `ICON_TYPES` (`app/models/system_note_metadata.rb:46-57`): `commit description merge confidential visible label assignee cross_reference designs_added designs_modified designs_removed designs_discussion_added title time_tracking branch milestone discussion task moved cloned opened closed merged duplicate locked unlocked outdated reviewer tag due_date start_date_or_due_date pinned_embed cherry_pick health_status approved unapproved status alert_issue_added relate unrelate new_alert_added severity contact timeline_event issue_type relate_to_child unrelate_from_child relate_to_parent unrelate_from_parent override issue_email_participants requested_changes reviewed custom_field duo_agent_started duo_agent_completed duo_agent_failed`. Validation `inclusion in icon_types`, nil allowed. MR-relevant subset used by code: `commit title description task assignee reviewer approved unapproved requested_changes reviewed branch locked unlocked cross_reference discussion outdated merge time_tracking cherry_pick`.

---------------------------------------------------------------------------------------------------

## 5. Resource events REST for MRs

Defined for `Issue` and `MergeRequest` in a loop (`lib/api/helpers/resource_events_helpers.rb:7-12`). All: `authenticate!` (401 anon; but public project read of events by anonymous is not allowed - always requires auth).

### 5.1 State events (`lib/api/resource_state_events.rb`)
- `GET /projects/:id/merge_requests/:eventable_id/resource_state_events` (`eventable_id` is the **IID**; line 31-37), paginated; `ResourceStateEventFinder#execute` (`app/finders/resource_state_event_finder.rb`): `.none` unless `read_merge_request`, else `eventable.resource_state_events.includes(:user)` (default order: id asc via association; [I]).
- `GET .../resource_state_events/:event_id` (404 if absent).
Entity `ResourceStateEvent` (`lib/api/entities/resource_state_event.rb`) order: `id, user (UserBasic), created_at, resource_type ("MergeRequest"), resource_id (MR global id, NOT iid), source_commit (40-char sha or null), source_merge_request_id (int or null), state`.
`state` values: `opened`(1) `closed`(2) `merged`(3) `locked`(4) `reopened`(5) (`app/models/resource_state_event.rb:15`). MR close => `closed`; reopen => `reopened`; merge => `merged` (+`source_merge_request_id`/`source_commit` only when merged via `source`; MR merged by GitLab has both null).
Note: `source_merge_request_id` is returned as the raw global id column (an Integer), the entity key is `source_merge_request_id` (not an object).

### 5.2 Label events (`lib/api/resource_label_events.rb`)
- `GET .../resource_label_events` : `eventable.resource_label_events.inc_relations` then `ResourceLabelEvent.visible_to_user?(current_user, paginate(events))` - **pagination happens before the visibility filter** so pages can come back short.
- `GET .../resource_label_events/:event_id`: 404 `{"message":"404 ResourceLabelEvent Not Found"}` if the user cannot `read_resource_label_event` (message from `not_found!('ResourceLabelEvent')`, format `404 ResourceLabelEvent Not Found`).
Entity (`lib/api/entities/resource_label_event.rb`): `id, user (UserBasic), created_at, resource_type, resource_id, label (LabelBasic), action`; `action` = `add` | `remove` (enum 1/2). `label` is `null` if the label was deleted [I] (events keep `label_id` null / `label` nil -> `LabelBasic` of nil => null).

### 5.3 Milestone events (`lib/api/resource_milestone_events.rb`)
- list: `ResourceMilestoneEventFinder` filters events to milestones whose parent the user can read; `GET .../resource_milestone_events/:event_id` 404 `404 ResourceMilestoneEvent Not Found` if cannot `read_milestone`.
Entity (`lib/api/entities/resource_milestone_event.rb`): `id, user, created_at, resource_type, resource_id, milestone (Entities::Milestone), action ("add"|"remove"), state` where `state` = the **MR's state at event time** (`opened|closed|merged|locked`; enum from Issue+MR available_states, no `reopened`).
Event semantics (`app/services/resource_events/change_milestone_service.rb`): changing milestone A -> B creates ONE `add` event pointing to B (no `remove` for A); removing creates one `remove` event referencing the old milestone.

Event creation semantics for labels: one `resource_label_events` row per added or removed label per update (bulk inserted, `created_at = noteable.system_note_timestamp`), so a single PUT adding 2 labels = 2 rows with identical created_at.

---------------------------------------------------------------------------------------------------

## 6. Edge cases (easy to get wrong)

1. **GET /approvals shape**: `approved` is false and `approved_by` is `[]` until someone approves; there are no `approvals_required`/`approvals_left` fields.
2. `/approvals` returns exactly `{user_has_approved,user_can_approve,approved,approved_by}` (entity `MergeRequestApprovals`).
3. Self-approval: POST /approve by the MR author (Developer+) succeeds with 201 and `user_can_approve` for that author reads `true` (no author check).
4. Status codes: approve-not-eligible => **401**, unapprove-nothing-to-remove => **404**, no permission => **403**, sha mismatch => **409**, reset_approvals by non-bot => **401**, success approve/unapprove => **201**, reset => **202**. 401 body is `{"message":"401 Unauthorized"}` (no `- reason`).
5. The sha check (409) happens AFTER the permission check and BEFORE eligibility; with an already-approved MR and a wrong sha you get 409, not 401.
6. Approving a closed MR works; approving a merged MR returns 401.
7. The approval system note and the approved webhook are produced **asynchronously** after the response (event store workers); the unapproval note and webhook are produced synchronously inside `RemoveApprovalService`. Approver todos are resolved by the approve worker, not the endpoint.
8. Webhook action naming: every approval => `approved`; every unapproval => `unapproved`.
9. Approvals persist across pushes (no reset, no state reset). `patch_id_sha` is still stored on approval rows but unused.
10. Reviewer state `approved` is sticky: `UpdateReviewerStateService` refuses (error "Reviewer has approved") any transition except to `requested_changes`/`unapproved`; e.g. `reviewed` after approving is a no-op. Re-request resets to `unreviewed` AND silently deletes the approval (note says `... and removed approval`).
11. A non-reviewer approving: reviewer-state update errors are ignored; no reviewer row is created.
12. Extra assignee/reviewer ids are silently truncated to the FIRST id as sent (before validity filtering); an invalid first id means no change at all and no error. `[0]` clears. Users who cannot read the MR are silently dropped. Non-privileged users' assignee/reviewer params are silently ignored. Response is the normal MR with unchanged fields.
13. Setting reviewers/assignees on create produces **no** system note, only todos/notifications; on update it produces notes+todos. Self-assignment still creates an `assigned` todo (and mentions never self-todo).
14. Any assignee/reviewer/label change by user X marks X's own pending todos on the MR as done (async).
15. Todo API: `POST .../todo` returns **304** (empty) when a pending marked todo exists; `state=all` is rejected by the API (400); default list is pending-only; `mark_as_done` on all uses the current list filters; `GET /todos` default sort id desc; todos for unreadable targets are dropped after pagination.
16. Resource events are not notes: closed/reopened/merged, label add/remove and milestone changes never appear in `/notes`; `/notes` for an MR contains only the Note-based ones from 4.2 plus user notes. `resource_id` in events is the MR global id; the URL uses iid. Milestone A->B is a single `add` event.
17. Title-change note body is HTML (`<p>changed title from <code class="idiff">…`) and only appears when the title changes beyond the Draft prefix; toggling Draft yields only `marked this merge request as **draft**|**ready**` (action `title`). `Draft:`/`WIP:` prefix detection is `MergeRequest.draft?` / `draftless_title`.
18. add_commits note: `<ul>` HTML is single-line, parts separated by exactly `\n\n`; compare link is a relative path starting `/`, `diff_id` = id of the MR's *current* diff (post-push, after the new diff is created) and `start_sha` = full oldrev. Short ids are 8 chars.
19. `discussion_lock` text uses `titleize.downcase` => `merge request` (lowercase, space). Cross-reference text for MRs is `merge request !N`.
20. `abort_auto_merge` reason first character is lowercased by `format_reason`; reasons come from callers (`'target branch was changed'`, `'merge request was closed'`, ...).
21. `reset_approvals` is bot-only; the doc claim "maintainers/owners" is wrong in code.
22. Webhook `reviewers` entries include `re_requested` only computed during a re-request update; otherwise false.
23. `ResourceStateEvent.state` includes `reopened`, but MR synthetic UI note for reopened is `reopened` text with icon action `opened`.
24. Hidden MRs (`hidden & ~admin`) prevent read/update/approve.
