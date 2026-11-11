# Merge requests: REST API and lifecycle (GitLab CE 18.11)

Source root: GitLab CE source (`/home/mabn/dev/gitlab-run-ce/gitlab-src`). Paths below are relative to it.
Abbreviations: **API** = `lib/api/merge_requests.rb`; **Basic** = `lib/api/entities/merge_request_basic.rb`;
**MR** = `app/models/merge_request.rb`.
Not covered here (other docs): notes, discussions, award emoji, draft notes, UserBasic / Milestone / Label / Pipeline / Issue entities (only referenced).
Things I could not verify from source are marked **[unverified]**.

---------------------------------------------------------------------------------------------------

## 0. Cross-cutting API behavior

- Error envelope: `{"message": ...}`. Helpers `lib/api/helpers.rb`:
  - `not_allowed!` -> 405 `{"message":"405 Method Not Allowed"}` (l.610). `unauthorized!` -> 401 `"401 Unauthorized"` (l.606). `forbidden!` -> 403 `"403 Forbidden"` (+ ` - reason`). `conflict!(msg)` -> 409 `msg or "409 Conflict"`. `unprocessable_entity!(msg)` -> 422 `msg or "422 Unprocessable Entity"` (l.626). `not_modified!` -> 304 (empty body). `bad_request!(reason)` -> `"400 Bad request - reason"`.
  - `render_validation_error!(model)` (l.656) -> 400, `message` = `model.errors.messages` hash, e.g. `{"message":{"title":["can't be blank"]}}`.
  - `ActiveRecord::RecordNotFound` -> 404 `{"message":"404 Not found"}` (lowercase "found"; `lib/api/api.rb:134`). Missing project -> `"404 Project Not Found"`.
  - `Gitlab::Access::AccessDeniedError` -> 403 `"403 Forbidden"` (api.rb:130). `ActiveRecord::StaleObjectError` / lease failure -> 409 `{"message":"409 Conflict: Resource lock"}` (api.rb:138).
  - Grape param validation errors -> 400 `{"error":"title is missing"}` style (not `message`).
  - `ActiveRecord::QueryCanceled` -> 408 `{"error":"Request timed out"}` (API:22).
- `find_merge_request_with_access(iid, ability=:read_merge_request)` (helpers.rb:379): `user_project.merge_requests.find_by!(iid:)` then `authorize! ability` (403 if not allowed, 404 if iid unknown). Note it looks in **target project** `merge_requests`.
- `find_project_merge_request(iid)` (helpers.rb:369): `MergeRequestsFinder.new(current_user, project_id:).find_by!(iid:)` (404 when invisible). Used by merge, merge_ref, cancel, rebase, delete, time tracking.
- Pagination: offset pagination (`lib/gitlab/pagination/offset_pagination.rb`). Headers: `X-Per-Page, X-Page, X-Next-Page, X-Prev-Page, Link, X-Total, X-Total-Pages`. X-Total/X-Total-Pages are **omitted** (and `rel="last"` link omitted) when the count exceeds `Kaminari MAX_COUNT_LIMIT` (10_000), and for `without_count` relations. Default order if none: `ORDER BY id`. Keyset pagination available on list endpoints only with `pagination=keyset` (not detailed here).
- Route settings: project list/get allow job tokens and public access (`allow_public_access_for_enabled_project_features: [:repository, :merge_requests]`, API:324-328, 435-438). Everything else needs auth: `before { authenticate_non_get! }` (API:14).
- Permission model (config/authz/roles/*.yml): `read_merge_request` Reporter+ (and Guest/anonymous via project feature visibility); `create_merge_request_from`, `update_merge_request`, `admin_merge_request`, `reopen_merge_request`, `push_code`: Developer+; `destroy_merge_request`: Owner (and admin); `create_merge_request_in` target: `guest_access & download_code` (project_policy.rb:856). Author/assignee of an MR get `update_merge_request` + `reopen_merge_request` if they can read it (issuable_policy.rb `rule { can?(:read_merge_request) & assignee_or_author }`). `rule { locked }.prevent :reopen_merge_request` (merge_request_policy.rb) uses the IssuablePolicy condition `locked` = `discussion_locked?`: a discussion-locked MR cannot be reopened by anyone.
- `accept_merge_request`/merge permission = `MergeRequest#can_be_merged_by?(user)` = `Gitlab::UserAccess#can_update_branch?(target_branch)` (MR:2034): user may push to / merge into the target branch (protected-branch "allowed to merge" aware).

---------------------------------------------------------------------------------------------------

## 1. Routes

All project routes are under `/api/v4/projects/:id/merge_requests...` with `:id` = id or url-encoded path. `:merge_request_iid` Integer.

| # | Verb + path | File:line | Permission | Success | Entity |
|---|---|---|---|---|---|
| 1 | GET `/merge_requests` | API:193-219 | auth required unless `scope=all` | 200 | Basic or Simple |
| 2 | GET `/groups/:id/merge_requests` | API:221-257 | `read_merge_request` on group | 200 | Basic/Simple |
| 3 | GET `/projects/:id/merge_requests` | API:305-345 | read | 200 | Basic/Simple |
| 4 | POST `/projects/:id/merge_requests` | API:347-393 | `create_merge_request_from` (project) + `create_merge_request_in` (target) | 201 | MergeRequest |
| 5 | DELETE `.../:iid` | API:395-416 | `destroy_merge_request` | 204 | - |
| 6 | GET `.../:iid` | API:418-450 | read | 200 | MergeRequest |
| 7 | GET `.../:iid/participants` | API:452-467 | read | 200 | UserBasic[] |
| 8 | GET `.../:iid/reviewers` | API:469-484 | read | 200 | MergeRequestReviewer[] |
| 9 | GET `.../:iid/commits` | API:486-514 | read | 200 | Commit[] |
| 10 | GET `.../:iid/context_commits` | API:516-531 | read | 200 | CommitWithLink[] (type full) |
| 11 | POST `.../:iid/context_commits` | API:533-569 | `update_merge_request` | 201 | Commit[] |
| 12 | DELETE `.../:iid/context_commits` | API:571-602 | `destroy_merge_request` (owner!) | 204 | - |
| 13 | GET `.../:iid/changes` | API:604-625 | read | 200 | MergeRequestChanges |
| 14 | GET `.../:iid/diffs` | API:627-653 | read | 200 | Diff[] |
| 15 | GET `.../:iid/raw_diffs` | API:655-670 | read | 200 | raw git diff text |
| 16 | GET `.../:iid/pipelines` | API:672-688 | read | 200 | PipelineBasic[] |
| 17 | POST `.../:iid/pipelines` | API:690-730 | find w/ read, then pipeline creation rights | 200 / 202 | Pipeline |
| 18 | PUT `.../:iid` | API:732-779 | `update_merge_request` | 200 | MergeRequest |
| 19 | PUT `.../:iid/merge` | API:781-854 | `can_be_merged_by?` | 200 | MergeRequest |
| 20 | GET `.../:iid/merge_ref` | API:856-874 | read (find_project_merge_request) | 200 | `{commit_id}` |
| 21 | POST `.../:iid/cancel_merge_when_pipeline_succeeds` | API:876-894 | `can_cancel_auto_merge?` | 201 (!) | see below |
| 22 | PUT `.../:iid/rebase` | API:896-920 | RebaseService#validate | 202 | `{rebase_in_progress}` |
| 23 | GET `.../:iid/closes_issues` | API:921-945 | read | 200 | IssueBasic/ExternalIssue |
| 24 | GET `.../:iid/related_issues` | API:947-970 | read | 200 | IssueBasic/ExternalIssue |
| 25 | GET `.../:iid/versions` | `lib/api/merge_request_diffs.rb:25-37` | read, **authenticate!** (no anon) | 200 | MergeRequestDiff[] |
| 26 | GET `.../:iid/versions/:version_id` | merge_request_diffs.rb:39-59 | read | 200 | MergeRequestDiffFull |
| 27 | POST `.../:iid/subscribe`, `/unsubscribe` | `lib/api/subscriptions.rb:44-77` | `update_merge_request` | 201 / 304 | MergeRequest |
| 28 | POST `.../:iid/todo` | `lib/api/todos.rb:26-44` | read (create_todo) | 201 / 304 | Todo |
| 29 | POST `.../:iid/time_estimate`, `/reset_time_estimate`, `/add_spent_time`, `/reset_spent_time`; GET `/time_stats` | `lib/api/time_tracking_endpoints.rb` (included at API:264) | `admin_merge_request` (writes), `read_merge_request` (stats) | 200 (POST explicitly `status :ok`) | IssuableTimeStats |

POST routes default to 201 in Grape unless `status` is set.

### 1.1 Common list params (`lib/api/helpers/merge_requests_helpers.rb`)

`merge_requests_base_params` (l.58-125) + pagination (`page`, `per_page` default 20 max 100):

| param | type / values | default | notes |
|---|---|---|---|
| `state` | `opened closed locked merged all` | `all` | `by_state` (issuable_finder): opened->`opened`, closed->`closed`, merged->`merged`, locked->`with_state(:locked)`, else no filter |
| `order_by` | `created_at label_priority milestone_due popularity priority title updated_at merged_at` | `created_at` | |
| `sort` | `asc desc` | `desc` | final sort key = `"#{order_by}_#{sort}"` (API:95); `sort_by_attribute` (issuable.rb:338, MR:811): `label_priority_asc/desc`, `milestone_due_asc/desc`, `popularity_asc/desc` (=upvotes), `priority_asc/desc` (due date + label priority), `title_asc/desc`, `merged_at_asc/desc`; others -> `order_by(method)` (created_at, updated_at). Ties broken with `id DESC` (`with_order_id_desc`). Absent sort -> `id DESC`. |
| `view` | `simple` | - | switches entity to MergeRequestSimple; no `skip_merge_status_recheck` option, no eager-load `with_api_entity_associations` (API:105) |
| `scope` | `created-by-me assigned-to-me created_by_me assigned_to_me reviews_for_me all` | none (project/group); `created_by_me` (global, via `optional_scope_param` l.127-131) | dashes normalised by `underscore` (API:96). created_by_me -> `author_id = current_user`; assigned_to_me -> assignee; reviews_for_me -> `review_requested_to(user)`; `all`/nil -> no filter. `created_by_me/assigned_to_me/reviews_for_me` with anonymous user -> empty result (`items.none`, issuable_finder `by_scope`). |
| `author_id` / `author_username` | Integer / String | - | mutually exclusive (400 `author_id, author_username are mutually exclusive`) |
| `assignee_id` | Integer or `None`/`Any` (case-insens, `integer_none_any`) | - | None -> `unassigned`, Any -> `assigned`; id -> `assigned_to(id)` |
| `assignee_username` | Array[String] | - | **max 1 value** else 400 `assignee_username allows one value, but found N: a, b` (CheckAssigneesCount) |
| `reviewer_id` | Integer or `None`/`Any` | - | `by_reviewer` in merge_requests_finder.rb: None->`no_review_requested`, Any->`review_requested`; unknown user -> empty |
| `reviewer_username` | String | - | exclusive with `reviewer_id`; the doc says None/Any work but Params only checks `reviewer_id` for wildcards (`filter_by_no_reviewer?` reads `reviewer_id`) |
| `labels` | comma list | - | AND semantic (each must be present). `None` (no labels) / `Any` (>=1 label), case-insensitive, checked on `label_names.map(&:downcase)` (label_filter.rb). Unknown label name with root namespace lookup -> empty. |
| `with_labels_details` | Boolean | false | labels become LabelBasic objects sorted by title (see 2.1) |
| `milestone` | String | - | `None`/`Any`(case-insens), `Upcoming` (matches `#upcoming`), `Started` (`#started`), else title match |
| `my_reaction_emoji` | String | - | `None`/`Any` supported; needs current_user |
| `source_branch`, `target_branch` | String | - | exact match |
| `source_project_id` | Integer | - | |
| `search` | String | - | title/description full-text/ILIKE; `in` = `title`, `description` or `title,description` restricts columns. Triggers `validate_search_rate_limit!` (429 when exceeded). |
| `created_after/before`, `updated_after/before` | DateTime ISO8601 | - | `>=`/`<=` semantics; if both updated_* given and before < after -> empty result (updated_at_filter.rb) |
| `wip` | `yes no` | - | draft filter via SQL LIKE on title: `Draft - %`, `Draft:%`, `[Draft]%`, `(Draft)%` (merge_requests_finder.rb `draft_match`; note `Draft - ` is matched here but NOT by the Ruby DRAFT_REGEX). `to_boolean('yes')`. `draft` param exists in finder (scalar) but is **not declared** in the API -> not settable. |
| `deployed_before/after`, `environment` | datetime / string | - | `by_deployments`: EXISTS on deployment_merge_requests joined to deployments (env name, finished_at bounds); unparsable date -> filter ignored |
| `approved` | `yes no` | - | declared, but no finder implementation found (only listed in `scalar_params`) -> **ignored** |
| `merge_user_id` / `merge_user_username` | Integer / String | - | exclusive; filters `merge_request_metrics.merged_by_id`; unknown user -> empty |
| `not[...]` | Hash | - | negatable: `author_id author_username assignee_id assignee_username reviewer_id reviewer_username labels milestone my_reaction_emoji` (`reviewer_id` Integer only in not). Labels not -> exclude MRs with any of them. |
| `non_archived` | Boolean | global: `false`; group: `true`; project: not declared | `non_archived` scope when truthy |
| `with_merge_status_recheck` | Boolean | false | see 3.4 |
| `iids[]` | Array[Integer] | - | **project endpoint only** (API:319) |

Unknown `order_by`/`sort`/`state` values -> 400 `{"error":"state does not have a valid value"}`.

### 1.2 GET /merge_requests (global) (API:193-219)
- `authenticate! unless params[:scope] == 'all'` -> anonymous without scope=all: 401. **Default scope `created_by_me`** (only own MRs).
- Visibility otherwise: projects readable by user with merge_requests feature (`Params#projects`).
- `attempt_project/group_search_optimizations` flags set (affects only SQL). Entity Basic (or Simple). `with_merge_status_recheck` handled only inside the entity.

### 1.3 GET /groups/:id/merge_requests (API:221-257)
- `include_subgroups: true`, group = `user_group` (404 if not visible). Extra option `group: user_group` -> `references.relative` is relative to the **group**.
- Unless `with_merge_status_recheck` is false: calls `MergeabilityCheckBatchService` (enqueues `MergeabilityCheckBatchWorker`) and then forces `skip_merge_status_recheck=true`.

### 1.4 GET /projects/:id/merge_requests (API:305-345)
- `authorize! :read_merge_request` (404/403 for invisible), `find_merge_requests(project_id:)`.
- `recheck_mergeability_of` (API:176): only if user `can?(:update_merge_request, user_project)`: `mr.check_mergeability(async: true)` for each, unless skipped.
- Response cache: `present_cached ... expires_in: 8.hours`, cache context `[user.cache_key, mr.merge_status, labels cache keys, assignees, reviewers]`; **skipped** when `with_labels_details=true`. (Cache is a perf concern only; do not reproduce staleness.)
- Headers: X-Total etc. returned (offset pagination).

### 1.5 POST /projects/:id/merge_requests (API:347-393)
Params: `title` (**required**, String), `source_branch` (required), `target_branch` (required), `target_project_id` Integer, plus `optional_params` (API:266-298): `assignee_id` Integer, `assignee_ids` Array[Integer] (comma-separated coerced; `0`/empty = none), `reviewer_ids` same, `description` (<= `description_and_note_max_size` = 1 MiB bytes), `labels` (comma list), `add_labels`, `remove_labels`, `milestone_id` Integer, `remove_source_branch` Boolean (-> `force_remove_source_branch`), `allow_collaboration` Boolean, `allow_maintainer_to_push` (alias `as: :allow_collaboration`), `squash` Boolean, `merge_after` String.
Flow:
1. `authorize! :create_merge_request_from, user_project` (403).
2. `mr_params = declared_params(include_missing: false)`; `force_remove_source_branch = remove_source_branch`; `assignee_id` -> `assignee_ids=[id]` if present (common_helpers.rb:6); adds `scope_validator`.
3. `MergeRequests::CreateService` (section 4). `QuickActionsNotAllowedError` -> 403 with message.
4. `handle_merge_request_errors!` (helpers.rb:70-82): if `!mr.valid?`: for keys `[:project_access, :branch_conflict, :validate_fork, :base]` present -> **422** `{"message": <errors[key] array>}` (first matching key in that order); else `validate_branches` -> **409** `{"message": ["Another open merge request already exists for this source branch: !N"]}`; else `render_validation_error!` -> 400 `{"message":{field:[msgs]}}`.
5. 201 + `Entities::MergeRequest` with `current_user, project: user_project`. **No `include_first_contribution`**, so `first_contribution` key is absent on POST/PUT/merge responses.

Exact errors (model validations, MR:394-412, 1354-1425, issuable.rb:81-86):
- same project & branch: 422 `["You can't use same project/branch for source and target"]` (key `branch_conflict`).
- `source_branch`/`target_branch` blank: `{"source_branch":["can't be blank"]}` (400). Empty `title`: `{"title":["can't be blank"]}`. Title > 255 chars: `{"title":["is too long (maximum is 255 characters)"]}`. Description > limit: `{"description":["is too long (maximum is 1048576 characters)"]}` [message text from BytesizeValidator; **[unverified]** exact wording].
- invalid branch name (leading `-`, trailing `/` or `.`, forbidden bytes etc., `GitRefValidator.validate_merge_request_branch`): `{"source_branch":["is invalid"]}` / `target_branch`. Only validated when attribute changes.
- nonexistent branch (create only; flag `validate_merge_request_branch_existence`): `{"source_branch":["does not exist"]}`, `{"target_branch":["does not exist"]}` (both can appear in one hash) -> 400.
- Merge requests disabled on target project: 422 `["Target project has disabled merge requests"]` (key `base`).
- Source project not a fork of target (source_project nil / unrelated): 422 `["Source project is not a fork of the target project"]` (`validate_fork`).
- Duplicate open MR (same source project, source branch, target branch, target project, state opened): 409 as above, reference formatted `!<iid>` (`to_reference` relative, no project path).
- `assignees`/`reviewers` > 200: `{"reviewers":["total must be less than or equal to 200"]}`.
- Missing create rights on target (`create_merge_request_in`): `CreateService#set_projects!` raises AccessDeniedError -> 403 `403 Forbidden`.
- Unknown target_project_id: `Project.find` -> 404 `404 Not found`.

### 1.6 PUT /projects/:id/merge_requests/:iid (API:732-779)
Params: `title` (allow_blank false), `target_branch` (allow_blank false), `state_event` (`close`|`reopen`), `discussion_locked` Boolean, plus `optional_params`. `at_least_one_of(assignee_id assignee_ids reviewer_ids description labels add_labels remove_labels milestone_id remove_source_branch allow_collaboration allow_maintainer_to_push squash target_branch title state_event discussion_locked merge_after)` -> 400 `{"error":"assignee_id, assignee_ids, ... are missing, at least one parameter must be provided"}`.
Flow: `find_merge_request_with_access(iid, :update_merge_request)` (403 for non-authors w/o rights); `remove_source_branch` -> `force_remove_source_branch` only if key present; `use_specialized_service = true`; `UpdateService` (section 4.4); errors via `handle_merge_request_errors!`; 200 + MergeRequest entity.
- `assignee_id=0` or `assignee_ids=[0]` clears assignees (ProcessAssignees treats `0`/"0"/NONE as empty) [`IssuableFinder::Params::NONE = '0'`]. Specialized path: if the only param is `assignee_ids` -> `UpdateAssigneesService`; if only `spend_time` -> `AddSpentTimeService` (update_service.rb:~340).

### 1.7 DELETE (API:395-416)
`authorize!(:destroy_merge_request, merge_request)` (Owner/admin; Maintainer -> 403). `destroy_conditionally!` honors `If-Unmodified-Since` (412 `"412 Precondition Failed"`); `Issuable::DestroyService`; 204 empty. Cleans refs/diffs asynchronously.

### 1.8 GET single (API:418-450)
Params `render_html`, `include_diverged_commits_count`, `include_rebase_in_progress` (Boolean). Presents `MergeRequest` with `include_first_contribution: true`. `render_html` adds `title_html`, `description_html`.

### 1.9 participants / reviewers (API:452-484)
- participants: `merge_request.participants(current_user)` (author, assignees, reviewers, note authors, system-note authors, mentioned users visible to current_user) paginated array -> UserBasic.
- reviewers: `merge_request.merge_request_reviewers` -> `{ "user": UserBasic, "state": "unreviewed|reviewed|requested_changes|approved|unapproved|review_started", "created_at": ... }` (`lib/api/entities/merge_request_reviewer.rb`, states: app/models/concerns/merge_request_reviewer_state.rb: unreviewed 0, reviewed 1, requested_changes 2, approved 3, unapproved 4, review_started 5). Pagination via Kaminari array (X-Total present).

### 1.10 commits (API:486-514)
Params `page`, `per_page` (`page>0 ? page : 1`, per_page>0 else default 20; limit = min(per_page, 100)). `merge_request_diff.commits(limit:, page:, load_from_gitaly: true)` of the **latest diff**, ordered newest first (`relative_order` 0 = HEAD). Total = `merge_request_diff.commits_count`. Entity `Commit` (section 2.5).

### 1.11 context commits (API:516-602)
- GET: `merge_request.merge_request_context_commits.map(&:to_commit)` paginated; entity `CommitWithLink` with `type: :full` = Commit fields + `author` (UserPath), `author_gravatar_url`, `commit_url`, `commit_path`, `description_html`, `title_html`, `signature_html`, `prev_commit_id`, `next_commit_id`, `pipeline_status_path` [UserPath/gravatar details elsewhere].
- POST: body `commits` (comma list / array, required, not blank). >20 -> 400 `Context commits array size should not be more than 20`. `authorize! :update_merge_request` (403). `AddContextService` errors (rendered with `result[:http_status]`): 403 `You are not allowed to access the requested resource`; 400 `Context commits: [shas] are already created` (Ruby array inspect of existing full shas); 400 `One or more context commits' sha is not valid.`. Success: 201 `Commit[]`.
- DELETE: `authorize!(:destroy_merge_request)` (owner); if `commits_by(oids).size != ids.size` -> 400 `One or more context commits' sha is not valid.`; 204.

### 1.12 changes / diffs / raw_diffs (API:604-670)
See section 6.

### 1.13 pipelines (API:672-730)
- GET: `Ci::PipelinesForMergeRequestFinder#execute` (pipelines whose merge_request or whose sha/ref belongs to MR: MR pipelines + branch pipelines of source branch whose sha in MR commits), `PipelineBasic[]` (id, iid, project_id, sha, ref, status, source, created_at, updated_at, web_url).
- POST: param `async` Boolean default false. Allowed-ness: `CreatePipelineService#execute(mr)` with `allow_duplicate: true`; payload nil (service returned error, e.g. "Cannot create a pipeline for this merge request: no commits to build.") -> 405 `405 Method Not Allowed`; persisted -> 200 `Entities::Ci::Pipeline`; unpersisted -> 400 validation error; `async=true` -> `execute_async` then 202 with **empty body**. Pipeline is created in the **target project** on `refs/merge-requests/:iid/head` when the user can create pipelines there (fork: needs `ci_allow_fork_pipelines_to_run_in_parent_project`; same-project: needs push to source branch name in target), otherwise in source project on the source branch ref (create_pipeline_service.rb:60-85).

### 1.14 PUT /merge (API:781-854)  -- see section 5 for details
Params: `merge_commit_message`, `squash_commit_message`, `should_remove_source_branch` Boolean, `merge_when_pipeline_succeeds` Boolean (deprecated), `auto_merge` Boolean, `sha`, `squash` Boolean.
Decision tree (exact):
1. `find_project_merge_request` (404).
2. `unauthorized! unless merge_request.can_be_merged_by?(current_user)` -> **401** `{"message":"401 Unauthorized"}` (not 403).
3. `auto_merge = to_boolean(merge_when_pipeline_succeeds) || to_boolean(auto_merge)`.
4. `build_merge_params`: `check_sha_param!` -> **409** `{"message":"SHA does not match HEAD of source branch: <diff_head_sha>"}` when `sha` given and != `diff_head_sha`; `merge_request.update(squash: params[:squash]) if params[:squash]` (only truthy; persists immediately); builds `{commit_message, squash_commit_message, should_remove_source_branch, sha: params[:sha] || diff_head_sha }` with nils compacted.
5. If `auto_merge`:
   - strategy available (`AutoMergeService#available_strategies(mr).include?(mr.default_auto_merge_strategy)` where default = `merge_when_checks_pass`; section 5.4): `AutoMergeService.execute(mr, strategy)` (result ignored) -> falls through to present (200).
   - elsif `pipeline_allows_merge?` (flag `merge_immediately_when_no_pipeline`: `(!pipeline_creating? && !diff_head_pipeline) || diff_head_pipeline_success?`; else `diff_head_pipeline_success?`): `422 "Branch cannot be merged"` unless `mergeable?`; then `MergeService` synchronously; `422 "Branch cannot be merged"` unless `merged?`.
   - else `not_allowed!` 405.
6. elsif `merge_request.mergeable?` -> MergeService synchronous, then `422 "Branch cannot be merged"` unless `merged?`.
7. else `not_allowed!` -> **405** `"405 Method Not Allowed"`. (Covers: not open (incl. already merged), draft, unresolved discussions when required, CI not passed when required, conflicts, need rebase, scheduled `merge_after` in future, no commits / missing branch, LFS locks; section 3.)
8. 200 `Entities::MergeRequest` (no first_contribution).
Therefore: merge is **synchronous** in the API (the Sidekiq `merge_async` path is used by UI/quick actions/auto merge). While merging inside MergeService the MR is `locked` (in_locked_state) but the API response only reflects the final state. A failure inside MergeService (git error, pre-receive, sha changed) leaves MR `opened` with `merge_error` set -> 422 `Branch cannot be merged` (the detailed text is only visible later in `merge_error` field of GET).

### 1.15 GET merge_ref (API:856-874)
`MergeabilityCheckService#execute(recheck: true)`; success -> `{"commit_id": "<merge ref head sha>"}` (200). Failure -> **400** `{"message": <result.message>}`; messages: `Merge request is not mergeable`, `Merge ref cannot be updated`, `Unsupported operation` (read-only DB), `Invalid argument`, lease failure message. Side effect: updates `merge_status` and the `refs/merge-requests/:iid/merge` ref.

### 1.16 cancel_merge_when_pipeline_succeeds (API:876-894)
`unauthorized! unless can_cancel_auto_merge?(user)` (`can_be_merged_by? || author == user`) -> 401. Then returns the raw result of `AutoMergeService#cancel` as body with Grape POST default status **201**: success -> `{"status":"success"}`; not enabled -> `{"message":"Can't cancel the automatic merge","status":"error","http_status":406}` still with HTTP 201 (endpoint never renders the error code). Cancel clears `auto_merge_enabled`, `merge_user`, merge_params `should_remove_source_branch commit_message squash_commit_message auto_merge_strategy`, adds system note.

### 1.17 rebase (API:896-920)
Param `skip_ci` Boolean. `RebaseService#validate`: errors -> **403** `403 Forbidden - <msg>`: `Source branch does not exist`, `Cannot push to source branch`, `Source branch is protected from force push`. Then `rebase_async(user.id, skip_ci:)` under `FOR UPDATE NOWAIT` lock: `raise StaleObjectError if !open? || rebase_in_progress?` -> **409** `409 Conflict: Resource lock` (this is the "already in progress"/"not open" response; there is no "Rebase is already in progress" string in the API); lock timeout -> 409 with message `Failed to enqueue the rebase operation, possibly due to a long-lived transaction. Try again later.`. Success: **202** `{"rebase_in_progress": true|false}` (computed immediately after enqueue; true once `rebase_jid` is set and the Sidekiq job is running; with fast workers can be false). The job (`RebaseWorker`): `RebaseService#execute` -> `repository.rebase` (Gitaly), on success updates `rebase_commit_sha`, `merge_error=nil`; on failure `merge_error = "Rebase failed: Rebase locally, resolve all conflicts, then push the branch."` or `"The rebase pre-receive hook failed: <msg>."`; always `rebase_jid = nil`. Rebase pushes a new commit to the source branch -> push processing -> RefreshService (new diff version, merge_status unchecked). Poll via `GET ...?include_rebase_in_progress=true` (fields `rebase_in_progress`, `merge_error`).

### 1.18 closes_issues / related_issues (API:921-970)
- closes: `visible_closing_issues_for(current_user)` (cached closing-issue rows from title/description/commit messages; excludes issues the user cannot read, and issues with autoclose disabled) paginated; internal issues as `IssueBasic`, external as `ExternalIssue`. Computed as `Array`, response JSON array.
- related: `related_issues(user)` = references parsed from title, description, all (non-internal unless user can read internal) note texts, commit messages (when diff persisted); `Gitlab::ReferenceExtractor#issues`.

### 1.19 versions (merge_request_diffs.rb)
- list: `merge_request.merge_request_diffs.order_id_desc` paginated -> MergeRequestDiff (no files). Includes `merge_head` type diffs? `merge_request_diffs` association is the regular ones [association scoped `diff_type: regular` **[unverified]**].
- show: `merge_request.merge_request_diffs.find(version_id)` (404 if not belonging) -> `MergeRequestDiffFull`; param `unidiff` Boolean default false.

### 1.20 subscribe / todo
- subscribe: if `subscribed?(user, project)` already -> **304**; else subscribe and 201 with MergeRequest entity (`current_user, project, parent`). `subscribed?` includes implicit subscription for participants (author, assignees, reviewers, commenters) unless an explicit unsubscribe record exists (issuable.rb:532). unsubscribe mirrors: 304 if not subscribed. Needs `:update_merge_request`.
- todo: `TodoService.mark_todo(mr, user).first` -> 201 Todo; none created (pending todo exists) -> 304.

### 1.21 Time tracking (time_tracking_endpoints.rb)
- `time_estimate` (POST, `duration` required String like `3h30m`): requires `admin_merge_request`; `Gitlab::TimeTrackingFormatter.parse(duration, keep_zero: true)`; nil/negative -> 400 `{"message":"400 Bad request - Time estimate must have a valid format and be greater than or equal to zero."}` via `bad_request!(reason: '...')` (time_tracking_endpoints.rb:69). **[unverified]** `reason:` is passed as a Hash, so the message is probably `400 Bad request - {reason: "Time estimate must have ..."}` (Ruby Hash#inspect); confirm on the oracle.
- `reset_time_estimate` sets 0. `add_spent_time` (`duration` required, optional undeclared `summary`) -> `UpdateService` with `spend_time`. `reset_spent_time`. `time_stats` GET.
- All respond 200 `IssuableTimeStats`: `{"time_estimate":int,"total_time_spent":int,"human_time_estimate":str|null,"human_total_time_spent":str|null}` (`lib/api/entities/issuable_time_stats.rb`; human values via `TimeTrackingFormatter.output`, null when 0). On validation failure -> 400 validation hash.

---------------------------------------------------------------------------------------------------

## 2. Entities (exact field order)

Grape::Entity emits fields in declaration order, parent first. JSON timestamps are ISO 8601 UTC with ms: `2026-01-01T12:00:00.000Z`.

### 2.1 MergeRequestSimple (`view=simple`) (merge_request_simple.rb)
`id, iid, project_id, title, description, state, created_at, updated_at` (IssuableEntity), then `title` (re-exposed, same position), `web_url`.
Result key order: id, iid, project_id, title, description, state, created_at, updated_at, web_url.
- `project_id` = `entity.project.id` = **target** project id.
- `web_url` = `http(s)://host/<target project path>/-/merge_requests/<iid>`.
- `state` strings: `opened`, `closed`, `merged`, `locked`.

### 2.2 MergeRequestBasic (list entity) -- order
(`lib/api/entities/issuable_entity.rb` first)
1. `id, iid, project_id, title, description, state, created_at, updated_at`
2. `merged_by` (UserBasic or null) = `metrics.merged_by` (deprecated)
3. `merge_user` = `metrics.merged_by || merge_user` (the user who set auto-merge, until merged)
4. `merged_at` = `metrics.merged_at`
5. `closed_by` = `metrics.latest_closed_by`; `closed_at` = `metrics.latest_closed_at` (cleared on reopen)
6. `title_html`, `description_html` only if `render_html` option (never in lists)
7. `target_branch, source_branch`
8. `user_notes_count` (non-system notes on MR, `Note.user` scope), `upvotes`, `downvotes` (award emojis named `thumbsup`/`thumbsdown` on the MR; `Gitlab::IssuableMetadata`)
9. `author`, `assignees` (array), `assignee` (first assignee or null), all UserBasic
10. `reviewers` (UserBasic array)
11. `source_project_id` (null if source project deleted), `target_project_id`
12. `labels`: array of title strings sorted by title; with `with_labels_details`: LabelBasic objects `{id,name,description,description_html,text_color,color,archived}` sorted by title
13. `draft` (bool, `MergeRequest.draft?(title)`), `imported` (`imported_from != none`), `imported_from` (enum string: `none gitlab_migration gitlab_project gitlab_group github bitbucket bitbucket_server fogbugz gitea git manifest custom_template jira`)
14. `work_in_progress` (= draft, deprecated)
15. `milestone` (Milestone entity or null)
16. `merge_when_pipeline_succeeds` (= column / alias `auto_merge_enabled`)
17. `merge_status` (see 3.4), `detailed_merge_status` (see 3.3)
18. `merge_after` = `merge_schedule&.merge_after` (null if none)
19. `sha` = `diff_head_sha` (latest diff `head_commit_sha`, falling back to source branch head when no persisted diff; null if neither)
20. `merge_commit_sha`, `squash_commit_sha` (columns; null until merged; squash_commit_sha set only when squash happened; for ff merge merge_commit_sha is null)
21. `discussion_locked` (nullable boolean: can be `null` for MRs never touched; default column NULL)
22. `should_remove_source_branch` (`merge_params['should_remove_source_branch']` boolean-cast; null when absent: `to_boolean(nil)` -> nil), `force_remove_source_branch` (same for `merge_params['force_remove_source_branch']`; on API create this is the `remove_source_branch` value, or `project.remove_source_branch_after_merge?` default)
23. `prepared_at` (null until AfterCreateService finishes)
24. only if `for_fork?` (source != target project): `allow_collaboration`, `allow_maintainer_to_push` (both = `collaborative_push_possible? && allow_maintainer_to_push`; `collaborative_push_possible?` needs both projects non-private and source branch not protected)
25. `reference` (deprecated; `to_reference(options[:project])`), `references` `{short, relative, full}`: short = `!iid`; relative = `to_reference(options[:group] || options[:project])` (`!1` in own project; `group/proj!1` in a group listing or a different project; global list passes neither -> relative to nil = full-ish path `ns/proj!1`); full = `ns/proj!iid`
26. `web_url`
27. `time_stats` `{time_estimate,total_time_spent,human_time_estimate,human_total_time_spent}`
28. `squash` (column bool), `squash_on_merge` (`squash_always? ? true : squash_never? ? false : squash`)
29. `task_completion_status` `{count, completed_count}` (checkbox items in description)
30. `has_conflicts` = `merge_status == 'cannot_be_merged'` exactly (not recheck variants; also true when `broken?` marked unmergeable: no commits / missing branch)
31. `blocking_discussions_resolved` = `true` unless project/group `only_allow_merge_if_all_discussions_are_resolved`, else `unresolved_notes.none?(&:to_be_resolved?)`

Nested `Milestone` entity order: `id, iid, project_id|group_id (conditional), title, description, state, created_at, updated_at, due_date, start_date, expired, web_url`.

### 2.3 MergeRequest (single entity: GET one, POST, PUT, merge, subscribe) (merge_request.rb)
Basic fields, then:
1. `subscribed` (bool; `subscribed?(current_user, project)`; false for anon; omitted when `include_subscribed: false`)
2. `changes_count`: **string**. `merge_request.merge_request_diff.real_size` (column `merge_request_diffs.real_size varchar`). `"N"` normally; `"1000+"` when the stored diff overflowed (`Gitlab::Git::DiffCollection#real_size` -> `"#{size}+"` if overflow; limits `diff_max_files=1000`, `diff_max_lines=50000` application settings). `null` when the MR has no persisted diff yet (just created, state `preparing`) or diff state `empty`.
3. `latest_build_started_at`, `latest_build_finished_at`, `first_deployed_to_production_at`, `pipeline` (PipelineBasic from `metrics.pipeline`): present only if `project.feature_available?(:builds, user)` (true for normal users with builds enabled); values from `merge_request_metrics`.
4. `head_pipeline` (full `Ci::Pipeline` entity; includes `detailed_status`, `user` ...) if `can?(user, :read_pipeline, project)`; value null when no head pipeline. `head_pipeline` = `diff_head_pipeline`-matching pipeline only through `head_pipeline` association (MR:841 `diff_head_pipeline` filters by sha).
5. `diff_refs` `{base_sha, head_sha, start_sha}` or null (null before diff exists)
6. `merge_error` (string/null; set by failed merge/rebase, cleared when MR closes/merges/opens or push)
7. `rebase_in_progress` only when `include_rebase_in_progress`
8. `diverged_commits_count` only when `include_diverged_commits_count` (commits in target not in source: `count_commits_between(source_sha, target_sha)`, cached by shas)
9. `first_contribution` only when `include_first_contribution` (GET single only): merged -> `metrics.first_contribution`; else `!project.merge_requests.merged.exists?(author_id:)`
10. `user` => `{"can_merge": bool}` (`can_be_merged_by?(current_user)`; false for anonymous)

### 2.4 MergeRequestChanges (< MergeRequest) (merge_request_changes.rb)
MergeRequest fields, then `changes` (Diff[] via `diffs as: :changes`), `overflow` (bool).
- default: `changes = merge_request.diffs.diffs` (persisted diff files, limited), `overflow = diff_collection.overflow?`.
- `access_raw_diffs=true` (query param read raw via `params.fetch(:access_raw_diffs,false)` through `to_boolean`; not declared): `raw_diffs(limits: false)` -> all files with full patches, `overflow=false`.
- param `unidiff` (declared via `with_unidiff`, default false) -> diff text in unified form.
- `changes_count` still string/"1000+".

### 2.5 Diff entity (lib/api/entities/diff.rb)
Order: `diff, collapsed, too_large, new_path, old_path, a_mode, b_mode, new_file, renamed_file, deleted_file, generated_file`.
- `diff`: `json_safe_diff` (hunks only, starting `@@ ...`, no `diff --git` header; `"Binary files a and b differ\n"` for binary) or `unidiff` when `enable_unidiff`.
- `collapsed` / `too_large`: file exceeds auto-collapse/size limits (`too_large` ~ > 200KB patch (`DEFAULT_MAX_PATCH_BYTES`), diff blanked ""); `a_mode`/`b_mode` strings like `"100644"` (`"0"` for new/deleted).
- `generated_file`: true when path matches gitattributes `gitlab-generated` / known generated patterns.

### 2.6 MergeRequestDiff (versions list) / MergeRequestDiffFull
List: `id, head_commit_sha, base_commit_sha, start_commit_sha, created_at, merge_request_id, state, real_size, patch_id_sha`. `state` in `collected|overflow|empty|without_files|timeout|overflow_*`; `real_size` string (e.g. `"3"`, `"1000+"`, null when empty); `patch_id_sha` hex or null.
Full adds `commits` (Commit[] newest first) and `diffs` (Diff[] with `raw_diffs(limits: false)`), cached with `unidiff` as cache context.

### 2.7 Commit entity (lib/api/entities/commit.rb)
`id, short_id (8 chars), created_at (= committed_date), parent_ids, title (full_title), message (safe_message), author_name, author_email, authored_date, committer_name, committer_email, committed_date, trailers ({}), extended_trailers ({}), web_url`.
Dates ISO8601 with the commit's own timezone offset (e.g. `2012-05-28T04:42:42-07:00`) -- authored/committed dates keep original TZ, `created_at` is UTC `...Z` [check].

### 2.8 MergeRequestReviewer: see 1.9. IssuableTimeStats: see 1.21.

---------------------------------------------------------------------------------------------------

## 3. State machines and mergeability

### 3.1 `state` (MR:211-255)
Stored as `state_id` smallint: opened=1, closed=2, merged=3, locked=4 (issuable.rb:39). API strings `opened|closed|merged|locked`.
Events: `close` opened->closed; `mark_as_merged` opened|locked->merged; `reopen` closed->opened; `lock_mr` opened->locked; `unlock_mr` locked->opened.
Callbacks: `->opened|merged` sets `merge_jid=nil`; `->closed` clears `merge_error`; `->merged` clears `merge_error`, sets `metrics.first_contribution`; `->opened` enqueues `UpdateHeadPipelineForMergeRequestWorker`.
`locked` is transient during MergeService (`in_locked_state`: add to Redis locked set, `lock_mr`, ensure `unlock_mr` if still locked); stuck MRs are unlocked by cron (`UnstickLockedMergeRequestsService`, MR `merge_ongoing?` returns true while locked).
Merge from locked to merged: PostMergeService `mark_as_merged`. There is no locked->closed.

### 3.2 `merge_status` (MR:263-335; column varchar default 'unchecked')
States: `unchecked, preparing, checking, can_be_merged, cannot_be_merged, cannot_be_merged_recheck, cannot_be_merged_rechecking`.
Events:
- `mark_as_preparing`: unchecked|can_be_merged -> preparing
- `mark_as_unchecked`: preparing|can_be_merged|checking -> unchecked; cannot_be_merged|cannot_be_merged_rechecking -> cannot_be_merged_recheck
- `mark_as_checking`: unchecked -> checking; cannot_be_merged_recheck -> cannot_be_merged_rechecking
- `mark_as_mergeable`: unchecked|cannot_be_merged_recheck|checking|cannot_be_merged_rechecking -> can_be_merged (then if `auto_merge_enabled` enqueue `AutoMergeProcessWorker`)
- `mark_as_unmergeable`: same sources -> cannot_be_merged (from unchecked/checking: notify conflict by email/todo if `notify_conflict?`)
Transitions are timeless (no `updated_at` change). `recheck_merge_status?` = status in `unchecked, cannot_be_merged_recheck, checking, cannot_be_merged_rechecking`.
**API value** `merge_status` = `public_merge_status`: `cannot_be_merged_rechecking` and `preparing` are shown as **`checking`**; all others verbatim (including `cannot_be_merged_recheck`, `unchecked`).

### 3.3 `detailed_merge_status` (Mergeability::DetailedMergeStatusService, app/services/merge_requests/mergeability/detailed_merge_status_service.rb:13-37)
Algorithm (first match wins):
1. `merge_status == preparing` -> `preparing`
2. `checking` or `cannot_be_merged_rechecking` -> `checking`
3. `unchecked` -> `unchecked`
4. Run **all** checks (`MergeRequest.all_mergeability_checks`, `execute_all: false`, param `skip_ci_check: true`): stop at first check whose result is `failed` or `checking`.
   - all OK -> run the CI check on its own: if its result is `checking` or `failed`: `ci_still_running` if `diff_head_pipeline_considered_in_progress?` (pipeline creating or running/pending/created...), else `ci_must_pass`; otherwise `mergeable`.
   - not OK -> the identifier of the first unsuccessful check.
Consequences: `cannot_be_merged_recheck` status falls to step 4, where the conflict check returns `checking` -> value `conflict`. A stale diff (`source_branch_sha != merge_request_diff.head_commit_sha`) also yields `conflict` (checking result). A merged/closed MR returns `not_open` unless its merge_status is one of the first three (which win). CI failure is lowest priority (after conflict/rebase).

Order of checks (MR:1596-1620; tiers concatenated trivial, light, heavy, then git checks `mergeable_git_state_checks`):
1. `not_open` (CheckOpenStatus): fails unless `open?` (state opened; `locked` -> not open)
2. `draft_status` (CheckDraftStatus): fails if `draft?`; skipped param `skip_draft_check`
3. `merge_time` (CheckMergeTime): inactive if no `merge_schedule`; fails if `merge_after.future?`
4. `locked_lfs_files` (CheckLfsFileLocks): inactive unless `project.lfs_enabled?`; fails if any LFS file lock owned by a different user than MR author touches `changed_paths`. Cacheable in `Gitlab::MergeRequests::Mergeability::ResultsStore`.
5. `discussions_not_resolved`: inactive unless `only_allow_merge_if_all_discussions_are_resolved?`; fails if unresolved resolvable notes
6. `ci_must_pass` (CheckCiStatus): inactive unless `auto_merge_enabled?` or `only_allow_merge_if_pipeline_succeeds?`. When active: `mergeable_ci_state?` -> success; else if `pipeline_creating? || head_pipeline.active?` -> checking; else failed. `mergeable_ci_state?`: true unless pipeline must succeed (setting, or strategy `merge_when_checks_pass` with `has_ci_enabled?`); false if no `diff_head_pipeline`; false if creating; true if head pipeline skipped and `allow_merge_on_skipped_pipeline`; else `head_pipeline.success?`
7. `commits_status`: fails if `has_no_commits?` (no persisted diff or commits_count 0) or `branch_missing?` (source or target branch absent)
8. git checks: `need_rebase` (CheckRebaseStatus): inactive unless project merge method is ff or rebase_merge (`ff_merge_must_be_possible?`) and also inactive when `automatic_rebase_available?` (flag `rebase_on_merge_automatic` && project setting); fails if `should_be_rebased?` (= ff required && target head not ancestor of diff head)
9. `conflict` (CheckConflictStatus): `checking` if `source_branch_sha != merge_request_diff.head_commit_sha` or merge_status is not can_be_merged/cannot_be_merged; success if `can_be_merged?`; failed if `cannot_be_merged?`

Check result semantic: `success`, `inactive`, `warning` do not block; `failed` and `checking` block (`unsuccessful?`). All possible `detailed_merge_status` strings: `preparing, checking, unchecked, mergeable, not_open, draft_status, merge_time, locked_lfs_files, discussions_not_resolved, ci_must_pass, ci_still_running, commits_status, need_rebase, conflict`.

`mergeable?` (MR:1625): `mergeable_state?` (trivial+light+heavy checks, `use_cache`), then `check_mergeability(sync)` (runs MergeabilityCheckService synchronously if recheck state, with lease retry), then `mergeable_git_state?` (rebase, conflict). Note: here CI check is **not** skipped.

### 3.4 When status changes (async mergeability)
- Create: model default `unchecked`; `CreateService#after_create` -> `mark_as_preparing` (so create response: `merge_status:"checking"`, `detailed_merge_status:"preparing"`, `prepared_at:null`, `diff_refs:null`, `changes_count:null`, `sha` from source branch head). Then `NewMergeRequestWorker` -> `AfterCreateService`: create diff (+ `refs/merge-requests/:iid/head`), create pipeline, `mark_as_unchecked` + `check_mergeability(async)` -> `mark_as_checking` -> `MergeRequestMergeabilityCheckWorker` -> `MergeabilityCheckService#execute` -> `can_be_merged` or `cannot_be_merged`; finally `prepared_at = now`. Events/notifications/todos/hooks run there.
- `GET` single MR as a user with `update_merge_request` (Developer+): the entity's `merge_status` field calls `check_mergeability(async: true)` -> if state is unchecked/cannot_be_merged_recheck it transitions to checking/rechecking and enqueues the worker **before** reading `public_merge_status` -> so the same response already shows `"checking"`. Non-authorized or anonymous viewers never trigger and see stored value (may remain `unchecked`).
- List endpoints skip this unless `with_merge_status_recheck=true` (project list also runs `recheck_mergeability_of` for each; group list uses batch worker). Group batch: `MergeabilityCheckBatchService` -> `MergeabilityCheckBatchWorker`.
- Push to source or target branch (RefreshService): `mark_as_unchecked` for open MRs touching the branch (can_be_merged/preparing/checking -> unchecked; cannot_be_merged(-ing) -> cannot_be_merged_recheck); flag `batch_merge_status_updates` uses SQL batch updates.
- Target/source branch change via PUT, reopen: `mark_as_unchecked`.
- `MergeabilityCheckService#update_merge_status`: `broken?` (no commits/missing branch/already cannot_be_merged) -> `mark_as_unmergeable`; else `MergeToRefService` (author as committer) writes `refs/merge-requests/:iid/merge` and Gitaly `can_be_merged?` -> `mark_as_mergeable` + `ReloadMergeHeadDiffService`; else `mark_as_unmergeable`.
- Locking: key `mergeability_check:<id>`, ttl 1 min.

### 3.5 Draft detection
`Gitlab::Regex.merge_request_draft` = `/\A(?i)(\[draft\]|\(draft\)|draft:)/` (lib/gitlab/regex/merge_requests.rb:10); `MergeRequest::DRAFT_REGEX = /\A*#{...}+\s*/i` (MR:916). `draft?(title)`:
- Matches only at the **very start** (no leading whitespace): `Draft: x`, `draft: x`, `DRAFT:x`, `[Draft] x`, `(draft) x`.
- Legacy `WIP:` / `[WIP]` / `Draft - x` are NOT drafts (since 16.x). The finder `wip=yes` filter still matches `Draft - %` via SQL.
- `draftless_title` strips the repeated prefix(es) and following whitespace; `draft_title(title)` = title if already draft else `"Draft: #{title}"`.
- `draft` column (`before_validation :set_draft_status`) mirrors `draft?`; entity uses `draft?` (not column).
- Push with a commit whose title starts with draft/fixup!/squash! (`Commit::DRAFT_REGEX = /\A\s*#{merge_request_draft}|(fixup!|squash!)\s/`) auto-marks open MR as draft with system note (RefreshService `mark_mr_as_draft_from_commits`).
- API param `draft` is not accepted (the title prefix is the only way via REST); quick actions `/draft` `/ready` use `wip_event`.

---------------------------------------------------------------------------------------------------

## 4. Create / update services

### 4.1 CreateService (app/services/merge_requests/create_service.rb)
1. `set_projects!`: `@source_project = @project` (the `:id` project), `@project = Project.find(target_project_id)` if given; requires `create_merge_request_from(source)` and `create_merge_request_in(target)` else AccessDeniedError (403).
2. `set_default_attributes!`: `squash` default = `target_project.squash_enabled_by_default?` (project_setting `squash_option` in `always|default_on`) when key absent; `force_remove_source_branch` default = `source_project.remove_source_branch_after_merge?` when nil.
3. New MR with `target_project`, `source_project`, `source_branch`; `merge_after` removed for `UpdateMergeScheduleService` after creation.
4. IssuableBaseService#create (issuable_base_service.rb:211): author = current_user; quick actions are interpreted from `description` (`/assign`, `/label` etc. executed; description stripped of them; `QuickActionsNotAllowedError` -> 403); `filter_params` (below); `state_event=close` in params -> closed immediately after creation (`immediately_close`); callbacks Description / Milestone / Labels / TimeTracking applied in `after_initialize`.
5. `before_create`: warm branch-existence caches; `skip_ensure_merge_request_diff = true` (diff created async), spam check.
6. Save -> system notes, `handle_changes` (assignee/reviewer notifications), `after_create`: `NewMergeRequestWorker`, `mark_as_preparing`. Webhook `merge_request: open` is fired from AfterCreateService after the diff exists.
Defaults NOT applied via REST (only `BuildService`, used by UI/`/new` page and GraphQL?): default title from branch/commit, description template, `Closes #N` from branch name, draft for empty branches. REST needs `title`; blank string -> 400 `title can't be blank`.

### 4.2 filter_params details (issuable_base_service.rb:62-120, merge_requests/base_service.rb:~225-260)
- Without `set_merge_request_metadata` (= `admin_merge_request`, Developer+): `assignee_ids`, `assignee_id`, `discussion_locked`, `due_date` params silently dropped (no error). Labels/milestone: callbacks require `set_merge_request_metadata`; else ignored silently.
- Assignees (`allows_multiple_assignees?` false): list truncated to **first 1**; users who cannot read the MR are dropped; if none left param removed (no change). `[0]`/`"0"` -> clear. Reviewers (`allows_multiple_reviewers?` false): truncated to first 1 likewise; requires `admin_merge_request`; non-readable users dropped; empty after filtering -> param removed; `[0]` -> cleared.
- `allow_collaboration` dropped unless `merge_request.can_allow_collaboration?(user)`.
- `force_remove_source_branch` dropped unless user can `push_code` on source project (assign_allowed_merge_params).
- Labels (labels.rb): `labels` (names) -> `find_or_create_by_titles`: existing labels (project, ancestor groups) matched by exact title; unknown names are **created** as project labels only if user has `admin_label` (needs `admin_label` on the project), else silently skipped (compact). `add_labels` creates too; `remove_labels` only finds. `labels=""` clears all. Set then add then remove: `new = (labels || current) | add - remove`. Locked labels removed are restored (lock-on-merge). Touches MR `updated_at` if label set changed.
- Milestone: `milestone_id` blank/`0` => nil; not found among project + ancestor-group milestones => silently ignored (keeps old).
- Description callback: needs `update_merge_request`; sets `last_edited_at/by` on update.

### 4.3 CloseService / ReopenService
- PUT `state_event=close`: `CloseService`; no-op (silent) unless `can?(:update_merge_request)`. `allow_broken=true` (closing broken MR ok). `close` event only from opened (closing a merged MR does nothing: `merge_request.close` returns false -> no events; PUT still 200 with unchanged state). Side effects: Event `closed`, metrics `latest_closed_by/at`, state event/note "closed" (ResourceStateEvent), notification, todos, webhook `close`, cleanup review-app environments, deactivate pages MR deployments, abort auto-merge ("merge request was closed"), schedule ref cleanup in 14 days (`CleanupRefsService`), merge_status unchanged.
- `state_event=reopen`: needs `reopen_merge_request` (Developer or author/assignee; prevented by `discussion_locked`). `reopen` only from closed (a `merged`/`locked` MR cannot reopen). On success: event, `reload_diff` (new diff version if branches resolve; `fetch_ref!`), `mark_as_unchecked`, clear cleanup schedule, nil `merge_ref_sha`. Validation on reopen runs `validate_branches` (opened) -> if another opened MR exists for the same source/target branches: save fails, MR stays closed; response then goes through `handle_merge_request_errors!` -> 409 `Another open merge request already exists for this source branch: !N` **[inferred, unverified]**. Source branch missing does NOT block reopen at the model level (existence validation is `on: :create`); MR becomes opened, `commits_status` fails afterwards.
- A non-permitted user's `state_event` is silently ignored (200).

### 4.4 UpdateService (merge_requests/update_service.rb)
`execute`: `params[:draft]` true -> `title = draft_title` (not exposed by REST); `merge_after` -> `UpdateMergeScheduleService` (set or destroy schedule; blank clears); specialized services (assignee-only, spend_time-only) else `general_fallback`: removes `source_project_id`, `target_project_id`, `source_branch` params (immutable); if `closed_or_merged_without_fork?` drops `target_branch` and `force_remove_source_branch`; then `update`.
`IssuableBaseService#update` (l.~316): order: quick actions (from description), filter_params, `change_additional_attributes` -> **state change first** (close/reopen), then callbacks, then `assign_attributes`, save, system notes (title/description changes, labels, milestone, assignees, reviewers, target branch change `changed target branch from `a` to `b``, draft status), `handle_changes`, hooks (`update`), counters.
`handle_changes` extras:
- title/description changed: todos update, mention notifications; if auto-merge on and title regex present -> publish event.
- `target_branch` changed: system note, if not `target_branch_was_deleted`: create MR pipeline (`allow_duplicate: true`), `abort_auto_merge('target branch was changed')`; `reload_diff_if_branch_changed` (after_update callback) makes a new diff version (new `MergeRequestDiff` + `refs/merge-requests/:iid/head` refresh); `mark_as_unchecked`.
- title draft toggle: notifications + `DraftStateChangeEvent` (aborts/ re-evaluates auto-merge) + `trigger merge_status updated`.
- Target branch must exist? Not validated on update (only `validate_branch_name`: `{"target_branch":["is invalid"]}`); a nonexistent target -> `commits_status` failing later. Duplicate check applies (409) when changing target to a branch that already has an open MR from the same source.
- Updating a merged MR: allowed for title/description/labels/etc. (target_branch/force_remove only blocked for fork-less closed/merged).
- `discussion_locked` requires `set_merge_request_metadata` else silently dropped.

---------------------------------------------------------------------------------------------------

## 5. Merge flow

### 5.1 MergeService (app/services/merge_requests/merge_service.rb)
`execute(mr, options)`: returns nil if already merged or the exclusive lease `merge_requests_merge_service:<id>` (MERGE_LEASE_TIMEOUT) cannot be obtained. Strategy `FromSourceBranch`. `validate!` in order, each raising `MergeError` (message stored in `merge_error`, MR stays opened):
1. `You are not allowed to merge this merge request`
2. `The target branch is scheduled for deletion. Please wait for the branch to be retargeted.` (flag `prevent_merge_race_condition`)
3. strategy.validate!: `No source for merge`; `Only fast-forward merge is allowed for your project. Please update your source branch` (ff and need rebase and no automatic rebase); `Merge request is not mergeable` (`mergeable?(use_cache: false)`); `This project requires squashing commits when merge requests are accepted.` (`squash_always? && !squash`)
4. `Branch has been updated since the merge was requested. Please review the changes.` (`params[:sha] != diff_head_sha`)
Then inside `in_locked_state` (state -> locked):
- Git work: `squash_on_merge?` -> `SquashService` first (see 5.3); then `merge_requests_ff_only_enabled` -> `ff_merge` (no merge commit; `merge_commit_sha` stays nil; `squash_commit_sha = squashed sha`), else `repository.merge(user, source_sha, mr, message)` with merge commit (`merge_commit_sha` = commit). With `automatic_rebase_available? && ff && should_be_rebased?` -> `CreateRefService` builds squash+rebase on `refs/merge-requests/:iid/rebase_on_merge` then ff.
- Errors: pre-receive -> `Something went wrong during merge pre-receive hook. <msg>`; any other exception -> `An error occurred while merging` (logged).
- Attributes saved: `merged_commit_sha`, `merge_commit_sha`, `squash_commit_sha`; clears `in_progress_merge_commit_sha`.
- `after_merge`: `PostMergeService`; if `delete_source_branch?` (param `should_remove_source_branch` (default `force_remove_source_branch?`) AND `can_remove_source_branch?(user)`: source branch not protected, not default branch, user can push to source project, branch head == `diff_head_sha`) -> `DeleteSourceBranchWorker(mr.id, source_branch_sha, user.id)` async; persists `should_remove_source_branch` into merge_params when changed.
- Unlock; `success`.
`merge_error` is cleared when MR transitions to merged/closed/opened and on pushes.

### 5.2 PostMergeService (post_merge_service.rb)
`mark_as_merged` (state=3 `merged`; `merge_jid=nil`); Event `merged` + `metrics.merged_by_id/merged_at`; todos; state note/ResourceStateEvent `merged` ("merged" system note, with `mentionable_source` for manual merges); close issues (only when `target_branch == project.default_branch`): cache closing issues from description + merge/squash commit messages (`cache_closing_issues_after_merge!`), then `CloseIssueWorker` per issue (batched 5 per minute delay; external trackers closed inline; issues must have `autoclose_by_merged_closing_merge_request?`, and for issues not referenced in the MR description the user's `update_issue` permission is checked); notifications; `DeleteNonLatestDiffsService` (delete files of older diff versions in background); cancel review-app jobs, stop environments; `cleanup_refs` schedule (14 days); deactivate pages; `cancel_auto_merges_targeting_source_branch` (if source branch deleted: abort auto-merge on MRs targeting it); webhook `merge`.
Then `DeleteSourceBranchWorker` -> `RetargetChainService`: only for same-project merged MR: open MRs (max 4) in the source project whose **target branch == merged MR's source branch** are retargeted to the merged MR's target branch via `UpdateService` with `target_branch_was_deleted: true` (system note says "changed target branch from X to Y because Z was merged"/ branch-deleted variant; no new pipeline; head_pipeline_id nil; `retargeted=true`); then `Projects::DeleteBranchWorker` deletes the branch.
Response from API merge: MR entity right after: `state:"merged"`, `merged_at`, `merge_user`/`merged_by`, `merge_commit_sha`, `squash_commit_sha`, `sha` unchanged. (`closed_by` null.) `detailed_merge_status` -> `not_open` unless merge_status (can_be_merged remains) etc.

### 5.3 Messages
Merge commit (MR:1968, `default_merge_commit_message`): if `project.merge_commit_template` present -> expand template (placeholders below), else
```
Merge branch '<source_branch>' into '<target_branch>'

<title>

[Closes #1, #2 and #3]   (only if visible closing issues exist; `to_sentence`; relative references)

See merge request <group/project!iid>
```
joined by blank lines (`\n\n`). `include_description` variant (UI) replaces the Closes line with description. Param `merge_commit_message` overrides entirely (empty string -> `params[:commit_message] ||` treats "" as present since `||` only on nil... "" is truthy in Ruby, so `""` yields an empty message **[verify Gitaly default]**). Author/committer of merge commit = merging user (Gitaly UserMergeBranch with user's name/commit_email); message trailing newline handling by Gitaly.
Squash commit (`default_squash_commit_message`): `squash_commit_template` expansion if present and non-blank, else just the MR `title`. If `merge_request.commits_count == 1` and message equals the single commit's message -> no squash commit is created (reuses `diff_head_sha`). Squash blocked by `squash_never?`: `Squashing not allowed: This project doesn't allow you to squash commits when merging.`; failure: `Squashing failed: Squash the commits locally, resolve any conflicts, then push the branch.`.
Template placeholders (lib/gitlab/merge_requests/message_generator.rb, `%{name}`): `source_branch, target_branch, title, issues ("Closes #1, #2 and #3" or empty), description, reference (full `ns/proj!1`), local_reference (`!1`), source_project_id, first_commit, first_multiline_commit (falls back to title), first_multiline_commit_description, url (MR web_url), reviewed_by (`Reviewed-by: Name <email>` lines), approved_by, merged_by (`Name <commit_email>` of merge user or acting user), co_authored_by (`Co-authored-by: Name <email>` lines for other commit authors), merge_request_author (`Name <email>`), title_from_branch, all_commits (`* msg` blocks, commit bodies >100KiB replaced by `-- Skipped commit body exceeding 100KiB in size.`). Empty placeholders: lines/blank paragraph removal (`\n\n%{x}(\n\n|\Z)` -> `\1`; `^%{x}\n` removed). `\r` stripped. Unknown `%{foo}` left literal. New-MR title/description templates use a reduced placeholder set (UI path only).

### 5.4 Auto merge
- Only strategy: `merge_when_checks_pass` (constant `STRATEGY_MERGE_WHEN_CHECKS_PASS`). `AutoMerge::MergeWhenChecksPassService`.
- `available_for?(mr)` (auto_merge/base_service.rb:~81): user `can_be_merged_by?` (reason `forbidden`), then all mergeability checks with many skips (`skipped_auto_merge_checks`: ci, conflict-if-rechecking, draft, discussions, LFS locks, merge_time) must pass (so remaining: `not_open`, `commits_status`, `need_rebase`, and `conflict` unless still being rechecked), then MWCP-specific: unavailable if `merge_request.mergeable? && !diff_head_pipeline_considered_in_progress?`, i.e. **available only when the MR cannot merge right now** (pipeline pending/running, discussions unresolved, draft, etc.). With no CI and an otherwise-mergeable MR, `auto_merge=true` merges immediately via step 5.2 of 1.14 (`pipeline_allows_merge?` true since no pipeline).
- `execute`: transaction: merge_params `auto_merge_strategy`, `commit_message`, `squash_commit_message`, `should_remove_source_branch`, `sha`, `skip_ci`; `auto_merge_enabled=true` (column `merge_when_pipeline_succeeds`), `merge_user = current_user`; system note `merge_when_checks_pass` ("enabled an automatic merge when all merge checks for <sha> pass"); notification; `AutoMergeProcessWorker`. API responds 200 MR with `merge_when_pipeline_succeeds:true`, `merge_user` set.
- `process`: when `mergeable?` -> `merge_async(merge_user_id, merge_params)` (Sidekiq `MergeWorker`). Triggered when pipeline succeeds, merge_status becomes can_be_merged, draft cleared, discussions resolved etc.
- Abort (with note "aborted the automatic merge because <reason>"): source branch updated (`the source branch was updated.` + learn-more link), target branch changed, MR closed (`merge request was closed`), FF target updated (`target branch was updated`), unmergeable, etc. Reasons: AvailabilityCheck::ABORT_REASONS.
- Cancel: section 1.16.

### 5.5 Rebase flag/setting recap
Project `merge_method` (`merge`, `rebase_merge` = rebase_enabled only, `ff` = ff_only+rebase). Rebase is performed by Gitaly `UserRebaseConfirmable`; `rebase_commit_sha` stores result; `rebase_jid`; skip_ci adds push option `ci.skip`. Beta auto-rebase on merge (`rebase_on_merge_automatic` FF + project `automatic_rebase_enabled`) disables the `need_rebase` check.

---------------------------------------------------------------------------------------------------

## 6. Diff-related APIs

- `GET /changes` (API:604-625): entity 2.4. Permission `read_merge_request`. `changes_count` string. `overflow` true when diff collection hit limits (`max_files` 1000 / `max_lines` 50000 / file size). No deprecation signal in code (documented as deprecated in docs only; keep behavior).
- `GET /diffs` (API:627-653): `paginated_diffs(page, per_page)` -> `Gitlab::Diff::FileCollection::PaginatedMergeRequestDiff`; default per_page 20, max 100; `paginate(..., skip_pagination_check: true)` so headers `X-Total` = files count, `X-Total-Pages`, `X-Page`, `X-Per-Page`, `X-Next-Page`, `Link`. Out-of-range page -> `[]`. Items: Diff entity (2.5) with `unidiff` param. Order: file path sorted (diffs `sorted`).
- `GET /raw_diffs`: `no_cache_headers`; empty body with Workhorse header `Gitlab-Workhorse-Send-Data: git-diff:<base64 json>` (RawDiffRequest left_commit_id=base_sha, right_commit_id=head_sha) and `Content-Disposition: inline`. Real output = `git diff base..head` full patch text (`diff --git a/x b/x\nindex ...`), content-type `text/plain`. labrat must serve the text directly.
- `GET /versions`, `/versions/:id`: 1.19.
- `merge_ref`: 1.15. `GET /commits`: 1.10.

Diff versions: a new `merge_request_diffs` row is created (`create_merge_request_diff`) on: MR creation (async, AfterCreateService), pushes to source branch (any new commits or force push), pushes to target branch **only when** the diff's commits are among the pushed commits (`includes_any_commits?`) or branch/force push matches source, reopen, source/target branch change, rebase (via push). `ReloadDiffsService` skipped when `merge_requests_diffs_limit` (DIFF_VERSION_LIMIT) / commits limit reached (flags default off). A diff has `base_commit_sha` (merge-base), `start_commit_sha` (target head at creation), `head_commit_sha`; `state`: `empty` when no commits (`compare.commits.empty?`), `collected`, `overflow`; `real_size`; `commits_count`; `patch_id_sha` set after commit. `diff_refs.start_sha` = target branch sha at diff time (NOT merge-base unless same).

---------------------------------------------------------------------------------------------------

## 7. Refs written by GitLab (in the **target** project repo)

- `refs/merge-requests/<iid>/head`: = source branch head; written by `MergeRequest#fetch_ref!` (MR:2064: `fetch_source_branch!(source_repo, "refs/heads/<source>", ref_path)`) at the start of every `create_merge_request_diff`, i.e. at MR creation (AfterCreate, async), each push that creates a new diff version, reopen, branch change. Used as the ref for MR pipelines and CI_MERGE_REQUEST_REF_PATH. Force-updated.
- `refs/merge-requests/<iid>/merge`: test-merge commit (parents: target head, source head), message = default merge commit message, author/committer = MR author; written by `MergeToRefService` during mergeability checks (when result mergeable) and by `GET merge_ref`. Not written when conflicts. `merge_ref_sha` column holds frozen value after cleanup.
- `refs/merge-requests/<iid>/rebase_on_merge`: only for automatic rebase on merge (beta).
- Cleanup: on close/merge a `merge_request_cleanup_schedules` row is created (`scheduled_at = now + 14 days`); `CleanupRefsService` deletes `head`, `merge`, `rebase_on_merge` refs after the threshold (stores `merge_ref_sha` first). Reopen deletes schedule. MR delete removes refs.
- Keep-around refs (`refs/keep-around/<sha>`): `MergeRequest#keep_around_commit` after_save keeps `merge_commit_sha`; `MergeRequestDiff#keep_around_commits` (after_create) keeps base/start/head shas (and commit shas via `KeepAroundRefsWorker`); written in the repo that owns the sha (target project for MR diffs, `source: 'MergeRequest'|'MergeRequestDiff'`).

---------------------------------------------------------------------------------------------------

## 8. RefreshService (pushes) (app/services/merge_requests/refresh_service.rb)
Entry on every branch push (post-receive): `refresh_merge_requests!`:
1. Compute new commits (`commits_between(oldrev,newrev)`; branch added: commits since merge-base with an existing MR; branch removed: `[]`).
2. **`close_upon_missing_source_branch_ref`**: open MRs whose source branch no longer exists (source branch deleted, here MRs where `source_project == pushed project`) are closed via CloseService (as the pusher), log "Closing merge request due to missing source branch".
3. **`post_merge_manually_merged`**: open MRs targeting the pushed branch whose `diff_head_sha` is among the pushed commits and whose diff isn't `empty` are marked merged (`merge_commit_sha`/`merged_commit_sha` = merge commit found by `BranchPushMergeCommitAnalyzer`, else the head), via `PostMergeService` (events, issue closing, note "merged" with source commit/MR).
4. LFS link for forks.
5. `reload_merge_requests`: for open MRs where (source project+branch matches push) or force push -> `reload_diff` (new diff version), clear `merge_error`; other MRs whose diff contains pushed commits also reload; then `mark_as_unchecked` (batched when flag `batch_merge_status_updates`).
6. Per MR: outdate suggestions, abort auto-merge when `merge_params['sha'] != newrev` (reason "the source branch was updated. [Learn more](...)"), mark pending todos done, abort auto-merge MWCP for FF projects when target updated and rebase needed (`target branch was updated`), cache closing issues.
7. Per source MR: system note when branch added/removed (`change_branch_presence`: "deleted source branch `x`" / "restored source branch `x`"), "added N commit(s)" note (`add_commits`) with new/existing partition and force-push rewrite text, notify `NotifyAboutPushWorker`, auto-draft from draft/fixup/squash commit titles.
8. Async: `WebHooksWorker` (merge_request update webhook with oldrev), `PipelineWorker` (MR pipelines).
A restored branch does not auto-reopen the MR.

---------------------------------------------------------------------------------------------------

## 9. Edge cases and gotchas

1. **iid allocation**: `has_internal_id :iid, scope: :target_project` (MR:62) via the `internal_ids` row (initialised from `max(iid)` of the target project's MRs). Monotonic per **target** project, never reused after delete; concurrent creates are serialised by the row update; gaps are possible if a save fails after the id was reserved.
2. **Empty MR (no commits)**: creation succeeds (source != target branch required, but identical SHAs OK). Diff state `empty`, `changes_count` null, `diff_refs` still set after async job, `commits_status` check fails -> `detailed_merge_status:"commits_status"`, `merge_status` -> `cannot_be_merged`, `has_conflicts:true`; merge -> 405. The UI/Build path would auto-prefix `Draft:`; REST does not.
3. **source == target**: same project same branch -> 422 `You can't use same project/branch for source and target`. Same branch name across fork and upstream is fine.
4. **Branch names with slashes** are plain strings; `/` allowed, `..`, `@{`, trailing `.lock`, spaces, `~^:?*[\` and control chars invalid -> `is invalid`. Branch lookup uses `refs/heads/<name>`.
5. **Title** max 255 chars; **description** max 1 MiB bytes (setting `description_and_note_max_size`); only validated when changed. Title/description quick actions in `description` on create/update are executed and stripped (e.g. `/assign @u`, `/label ~x`, `/draft`, `/merge`) -> MR may differ from the posted description.
6. `PUT` with identical values still returns 200 and refreshed entity; an update that is rejected by permissions drops fields silently (labels, assignee, milestone, discussion_locked for Reporter-level authors, etc.).
7. **Merge a draft** -> 405 (check `draft_status`). **Merge with unresolved discussions** when the setting is on -> 405 (`discussions_not_resolved`). Merge of already merged/closed MR -> 405. Merge with `sha` mismatch -> 409 (checked before mergeability). Merge by non-pusher (e.g. Reporter, or protected target branch without permission) -> 401. Merge fails inside Git (e.g. conflict not detected earlier) -> 422 `Branch cannot be merged`.
8. **Locked MR**: while merging (state `locked`) `detailed_merge_status` = `not_open`, further merge calls -> 405; the merge lease (`merge_requests_merge_service:<id>`) makes concurrent MergeService.execute return nil -> API 422 `Branch cannot be merged`.
9. **Closing an MR whose source branch is gone**: allowed (`allow_broken`). Merge of such MR -> 405 (`commits_status`). Closing is also automatic on branch deletion (section 8).
10. **Reopen**: only from `closed`; merged cannot reopen; discussion-locked blocks; duplicate open MR blocks (see 4.3).
11. **Unauthenticated users** see `merge_status` as stored (no recheck is triggered; may be `unchecked`/`checking`), `user.can_merge:false`, `subscribed:false`, `head_pipeline` omitted (needs `read_pipeline`), `pipeline`/`latest_build_*` keys present if builds feature available to nil user. Private projects 404.
12. `changes_count` is a string everywhere (`"3"`, `"1000+"`), including `PUT`/`merge` responses.
13. `merge_user` differs from `merged_by` before merge: `merge_user` = auto-merge requester (non-null while auto-merge armed), `merged_by` stays null until merged.
14. `squash` in response is the MR flag; `squash_on_merge` accounts for project policy (`always`/`never`). Setting `squash=true` on a `never` project is stored but ignored on merge (squash_on_merge false).
15. `should_remove_source_branch` vs `force_remove_source_branch`: API create/update `remove_source_branch` writes `force_remove_source_branch`; merge-time `should_remove_source_branch` param is the per-merge choice and is stored in merge_params only when the source branch is removable by the user (`can_remove_source_branch?`).
16. `GET` list returns `merge_status` possibly `checking` without any recheck: values come straight from DB (mapped by `public_merge_status`).
17. `prepared_at` null + `detailed_merge_status:"preparing"` for a short time after create. labrat must either run the after-create job inline or keep this transient state consistent (diff_refs/changes_count null until done).
18. `merge_commit_sha` is null for fast-forward merges and for squash-only; `squash_commit_sha` is the squashed commit only when squashing; `sha` after merge remains the last diff head sha.
19. Cross-project: MR from fork -> `target_project_id` in body or the `:id` path project being the **source**; the MR lives in the target project; subsequent API calls use the target project id. The create path `:id` is the source project (API:259, 373).
20. `labels` unknown names: auto-created project labels with default color; the label list in response is title-sorted.
21. Group list `references.relative` and project list differ (group/ project relative, 2.2 item 25).
22. `merge_when_pipeline_succeeds` param still accepted (deprecated) and has the same effect as `auto_merge`.
23. `With 304`: subscribe/todo/unsubscribe no-ops return empty 304.
24. Cancel auto merge returns HTTP 201 even for the "can't cancel" error (see 1.16).
25. `PUT /rebase` returns 202 even if the job later fails; the failure appears in `merge_error` ("Rebase failed: Rebase locally, resolve all conflicts, then push the branch.").
26. Time zone / formats: `merge_after` stored as timestamptz; entity prints ISO 8601 ms UTC.
27. Merge request web URL: `<base>/<namespace>/<project>/-/merge_requests/<iid>`.
28. **Test oracle**: the Docker 18.11 instance (`localhost:8929`) is the conformance oracle; at the time of writing the `claude` account returned `403 Your account has been blocked`, so no live captures were used for this document.
