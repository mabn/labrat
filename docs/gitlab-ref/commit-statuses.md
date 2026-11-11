# GitLab CE 18.11: commit statuses API, external pipelines, MR effects

Source-reading only (no instance contacted). All paths are relative to the GitLab CE source root (`/home/mabn/dev/gitlab-run-ce/gitlab-src`).
`[unverified]` = could not be confirmed from source alone (gems such as grape / grape-entity /
state_machines are not vendored in the tree, and Gitaly behaviour is not visible).

Conventions used below:
- Error bodies produced by `render_api_error!(msg, code)` are `{"message": msg}` (`lib/api/helpers.rb:676`).
- `not_found!(x)` -> 404 `{"message":"404 x Not Found"}` (`lib/api/helpers.rb:593`); `forbidden!` -> 403 `{"message":"403 Forbidden"}`
  (`:581`); `unauthorized!` -> 401 `{"message":"401 Unauthorized"}` (`:606`); `not_allowed!` -> 405
  `{"message":"405 Method Not Allowed"}` (`:610`); `conflict!` -> 409 (`:622`).
- `ActiveRecord::RecordNotFound` -> 404 `{"message":"404 Not found"}` (`lib/api/api.rb:134`).
- Timestamps in JSON are ISO 8601 UTC with milliseconds (`2016-01-19T09:05:50.355Z`) [format taken from entity doc examples; the
  `+02:00` examples in `PipelineBasic` are only documentation].

---------------------------------------------------------------------------------------------------

## 1. `POST /projects/:id/statuses/:sha`

Defined `lib/api/commit_statuses.rb:102-120`. Service: `app/services/ci/create_commit_status_service.rb`.

### 1.1 Params (`lib/api/commit_statuses.rb:82-100`)

| param | type | required | notes |
|---|---|---|---|
| `sha` (path) | String | yes | resolved with `project.commit(params[:sha])` (`create_commit_status_service.rb:71`), so a branch/tag name or short sha also resolves; stored/returned sha is the full commit sha (`delegate :sha, to: :commit`, `:11`). |
| `state` | String | yes | one of `pending running success failed canceled skipped` (Grape `values:`). |
| `ref` | String | no | branch/tag name. NOT validated to exist. |
| `target_url` | String | no | |
| `description` | String | no | |
| `name` | String | no | label; default `'default'` |
| `context` | String | no | alias for `name` (GitHub compat). Precedence: `params[:name] \|\| params[:context] \|\| 'default'` (`:109-111`). |
| `coverage` | Float | no | |
| `pipeline_id` | Integer | no | existing pipeline for this sha (+ref) |

`attributes_for_keys` (`lib/api/helpers.rb:534`) only forwards `target_url description coverage` when `present?` (or explicit `false`),
so omitting them on an update keeps the old value; an empty string is ignored (does not clear).

### 1.2 Success response

Status: **201** (Grape POST default; the file never calls `status`; the desc block's `code: 200` is documentation only) [Grape default is
unverified in-tree but there is no override].
Body = `Entities::CommitStatus` of the created/updated job.

### 1.3 Errors (exact status + body)

Order of evaluation: Grape param validation -> `authenticate!` (before block, `:20`) -> `authorize! :create_commit_status` -> service.

| condition | status | body | cite |
|---|---|---|---|
| no auth | 401 | `{"message":"401 Unauthorized"}` | `commit_statuses.rb:20` |
| project not visible | 404 | `{"message":"404 Project Not Found"}` | generic project lookup |
| lacks `create_commit_status` (Developer+; archived / builds-disabled-with-no-access / repository-disabled projects are prevented) | 403 | `{"message":"403 Forbidden"}` | `:103`; `config/authz/roles/developer.yml:26`; `app/policies/project_policy.rb:692-702`; archived: `config/authz/permission_groups/internal/project/archived.yml:42` |
| `state` missing | 400 | `{"error":"state is missing, state does not have a valid value"}` | Grape validators [message text unverified, standard Grape+GitLab format] |
| `state` not in list | 400 | `{"error":"state does not have a valid value"}` | same [unverified text] |
| `coverage` not a float | 400 | `{"error":"coverage is invalid"}` | [unverified text] |
| commit not found | 404 | `{"message":"404 Commit Not Found"}` | `create_commit_status_service.rb:52` |
| `state` blank (unreachable via API since Grape requires it) | 400 | `{"message":"State is required"}` | `:53` |
| no `ref` given, no matching pipeline, and no branch contains the sha | 404 | `{"message":"404 References for commit Not Found"}` | `:54` |
| `pipeline_id` given but no non-archived pipeline with that id AND this sha (AND `ref` if given) | 404 | `{"message":"404 Pipeline for pipeline_id, sha and ref Not Found"}` | `:58` |
| pipeline at job-size limit and no running/pending job with this name/ref/user in it | 422 | `{"message":"The number of jobs has exceeded the limit"}` | `:62`, `:84-89` (limit `ci_pipeline_size`; default 0 = unlimited [unverified]) |
| user lacks `update_pipeline` on the (found or just-created) pipeline | 403 | `{"message":"403 Forbidden"}` | `:37`; `app/policies/ci/pipeline_policy.rb:40-47` (prevented when ref is protected and user cannot push/create tag on it, or pipeline archived). NOTE: the pipeline row is already created at this point (`find_or_create_pipeline` runs before the check, `:36-37`), so a 403 can leave an orphan empty pipeline behind. |
| model validation failure (e.g. `target_url` not an http(s) URL or >255, `description`/`ref` >255, name collides with a non-generic job of same name in the pipeline -> `name: ["has already been taken"]`) | 400 | `{"message":{"target_url":["..."]}}` (Hash of attribute -> messages, NOT a string) | `:42`; `app/models/generic_commit_status.rb:65-89`; `app/models/commit_status.rb:48-52`. Exact URL error strings [unverified]. |
| illegal state transition | 400 | `{"message":"Cannot transition status via :run from :running"}` | `app/services/ci/pipelines/add_job_service.rb:28-29` rescues `StateMachines::InvalidTransition`, service returns `bad_request(response.message)` (`:44-46`) |
| other StandardError in add-job | 400 | `{"message":"<exception message>"}` | `add_job_service.rb:30-34` |
| lock not obtained within ~10 s (20 retries x 0.5 s on `api:commit_statuses:project:<pid>:sha:<raw sha param>`, ttl 1 min) | 409 | `{"message":"Another update to this commit status is in progress"}` | `create_commit_status_service.rb:17-26,190-203`; API maps `reason: :conflict` -> `conflict!` (`commit_statuses.rb:110-115`) |
| add-job lock `ci:pipelines:<id>:add-job` not obtained | falls into `rescue StandardError` -> 400 with lock error message [unverified text] | `add_job_service.rb:19,30` |

### 1.4 Finding / creating the pipeline (`create_commit_status_service.rb:76-129`)

`first_matching_pipeline` (`:92-100`): `project.ci_pipelines.newest_first(sha:, limit: 100 (nil if pipeline_id))` -> `.not_archived`
-> `.for_ref(params[:ref])` only if `ref` param was given -> `.id_in(pipeline_id)` only if given -> `.first` (highest id).

Key consequences for a simulator:
1. Reuse is by **sha (+ref if supplied)**, NOT by `source`. Any newest pipeline for the sha (push, MR event, API, external...) is appended
   to, even if it is already finished. An external pipeline is only *created* when none matches (or the size limit is exceeded and the job
   is new, `:78-81`).
2. A finished pipeline gets re-opened by new jobs (see 3.3).
3. `ref` used for the new pipeline/job: `params[:ref] || first_matching_pipeline&.ref || repository.branch_names_contains(sha).first`
   (`:65-68`); order of `branch_names_contains` comes from Gitaly [unverified; assume alphabetical]. If `ref` param absent the `ref` is
   NOT "the default branch" despite the param doc.
4. New pipeline attributes (`:113-129`): `source: :external`, `sha` (full), `ref`, `user: current_user`,
   `protected: project.protected_for?(ref)`, `tag: repository.tag_exists?(ref)`, `iid` allocated via `ensure_project_iid!`
   (per-project counter, starts at 1), `status: created`, `before_sha` NULL (API shows the blank sha, see 3.1), `committed_at` NULL,
   `config_source` NULL, no yaml_errors. Then `Ci::PipelineCreatedEvent` is published (`:125-127`) -> MR head pipeline update (4.2).
5. Stage: one stage per pipeline named `external`, `position = 1_000_000` (`:131-136`, `generic_commit_status.rb:61`); jobs get
   `stage_idx = 1_000_000`.

### 1.5 Update-vs-new-job rule (the core of the state machine)

The job to update is looked up with (`:138-162`, `find_or_build_external_commit_status :142-149`):

```
GenericCommitStatus.running_or_pending            # status IN (running, pending)  <-- only live jobs match
  .for_project(project).in_pipelines(pipeline).in_partition
  .for_ref(ref).by_name(name).for_user(current_user)   # same poster
  [.ref_protected if project.protected_for?(ref)]
  .find_or_initialize_by(ci_stage: external stage, stage_idx: 1_000_000)
```
- Match found (a pending/running job with same name, same ref, same user, same pipeline) -> **update that job**.
- Otherwise a **new** `GenericCommitStatus` row is built (name/ref/user/project/pipeline inherited from the scope), even if a
  success/failed/canceled/skipped job with the same name exists. Afterwards `update_older_statuses_retried!`
  (`commit_status.rb:340-347`, called in `add_job_service.rb:23`) sets `retried=true, processed=true` on all other **latest** jobs in the
  pipeline with the same `name` (any status, any user). Retried jobs are hidden from `GET .../statuses` unless `all=true`.
- Posting as a different user always creates a separate job even if same name.

Then `apply_job_state!` (`:170-188`), run inside `Ci::Pipeline.transaction` and the `ci:pipelines:<id>:add-job` lock:

| state | call | new job (initial `created`) | existing `pending` | existing `running` |
|---|---|---|---|---|
| `pending` | `job.enqueue!` | -> pending (`queued_at=now`) | **400** `Cannot transition status via :enqueue from :pending` | **400** `Cannot transition status via :enqueue from :running` |
| `running` | `job.enqueue` (non-bang, result ignored) then `job.run!` | created->pending->running (`queued_at`,`started_at`=now) | -> running (`started_at`=now) | **400** `Cannot transition status via :run from :running` |
| `success` | `job.success!` | created->success (`finished_at`=now; `started_at` stays NULL) | -> success | -> success |
| `failed` | `job.drop!(:api_failure)` | created->failed (`finished_at`, failure_reason api_failure) | -> failed | -> failed |
| `canceled` | `job.cancel!` | created->canceled (`finished_at`) | -> canceled | -> canceled (no `canceling` because `supports_canceling?` is false) |
| `skipped` | `job.skip!` | created->skipped | -> skipped | **400** `Cannot transition status via :skip from :running` |

State machine: `app/models/commit_status.rb:137-241`. Timestamps: `queued_at` on ->pending; `started_at` on
`[created,preparing,pending]=>running`; `finished_at` on `any=>success/failed/canceled` (`:185-202`). Note `skipped` sets neither.
Message format `Cannot transition status via :<event> from :<state>` is the state_machines gem default (matches the example in the task;
gem not vendored) [format unverified in-tree, but this is the standard text].
Because found jobs are only pending/running, a terminal job can never receive a transition error: re-posting `success` after
`success` creates a **new** success job (old one -> retried); posting `running` after `success` creates a new job in `running`.
Failed attribute assignment: if transition raises, `target_url`/`description`/`coverage` changes are not saved (no save happened).
`allow_failure` is always false via this API (DB default false; `Reason#force_allow_failure?` needs an exit code, `reason.rb:18`).

### 1.6 Response entity `API::Entities::CommitStatus` (`lib/api/entities/commit_status.rb`), exposure order

`id, sha, ref, status, name, target_url, description, created_at, started_at, finished_at, allow_failure, coverage, pipeline_id, author`
(`author` = `UserBasic` of job `user` via `alias_method :author, :user`).
`UserBasic` (`lib/api/entities/user_basic.rb` + `user_safe.rb`) order: `id, username, public_email, name, state, locked, avatar_url,
[avatar_path only if options only_path], web_url`. (`custom_attributes` only with `with_custom_attributes`.) `public_email` is a nullable
varchar (`db/structure.sql:661`) -> `null` or `""` [unverified which]. `locked` = `access_locked?`. `avatar_url` is gravatar URL or
uploaded avatar absolute URL (null when gravatar disabled & no avatar) [unverified].

Example (first `pending` post, name omitted, ref omitted, poster user id 1):
```json
{
  "id": 41,
  "sha": "6104942438c14ec7bd21c6cd5bd995272b3faff6",
  "ref": "feature-1",
  "status": "pending",
  "name": "default",
  "target_url": null,
  "description": null,
  "created_at": "2026-11-07T10:00:00.123Z",
  "started_at": null,
  "finished_at": null,
  "allow_failure": false,
  "coverage": null,
  "pipeline_id": 12,
  "author": {
    "id": 1,
    "username": "root",
    "public_email": null,
    "name": "Administrator",
    "state": "active",
    "locked": false,
    "avatar_url": "https://www.gravatar.com/avatar/...?s=80&d=identicon",
    "web_url": "http://gitlab.example.com/root"
  }
}
```
After `running` on the same job: `"status":"running","started_at":"...Z"`. After `success`: `"status":"success","finished_at":"...Z"`.
`created_at` is the row creation time (not changed by transitions). The `sha` is always the full 40-char sha.

---------------------------------------------------------------------------------------------------

## 2. `GET /projects/:id/repository/commits/:sha/statuses`

`lib/api/commit_statuses.rb:55-69`.

- Auth: `authenticate!` (401); `authorize! :read_commit_status` (Reporter+, `config/authz/roles/reporter.yml:49`) -> 403; commit must exist
  (`user_project.commit(params[:sha])`) else 404 `{"message":"404 Commit Not Found"}`.
- Params: `ref` (filters `statuses.ref`; no default - NOT defaulted to default branch), `stage` (joins `ci_stage`, `ci_stage.name = stage`;
  external statuses have stage `external`), `name` (exact), `pipeline_id` (restricts pipelines), `all` (bool, default false), `order_by`
  (`id`|`pipeline_id`, default `id`), `sort` (`asc`|`desc`, default `asc`), standard `page`/`per_page` (`use :pagination`).
  Invalid `order_by`/`sort` values -> Grape 400 `{"error":"order_by does not have a valid value"}` [text unverified].
- Query: pipelines = `project.ci_pipelines.where(sha: params[:sha])` (the raw `:sha` param - a branch name would find the commit but match
  no pipelines -> `[]`) [+ `id: pipeline_id`]; statuses = `::CommitStatus.where(pipeline: pipelines)` - i.e. ALL job types (Ci::Build,
  bridges, generic) of those pipelines, every ref; `.latest` (retried IS NOT TRUE) unless `all=true`; then filters; then
  `order(order_column => sort)` with no secondary tiebreaker.
- Pagination: offset-based, default 20/page, max 100; headers `x-total`, `x-total-pages`, `x-per-page`, `x-page`, `x-next-page`,
  `x-prev-page`, `link` (standard GitLab `paginate`) [headers not re-verified in this pass].
- Body: JSON array of `CommitStatus` (see 1.6). Empty array when nothing matches (not 404).

---------------------------------------------------------------------------------------------------

## 3. The pipeline record and its status

### 3.1 Columns/behaviour of an external pipeline (`p_ci_pipelines`, `db/structure.sql:6364`)

| field | value for external pipeline |
|---|---|
| `id` | global bigint |
| `iid` | per-project sequence (`has_internal_id`, `ci/pipeline.rb:71`) |
| `source` | `external` (enum value 6, `app/models/concerns/enums/ci/pipeline.rb:45`) |
| `ref` | see 1.4(3) |
| `tag` | `repository.tag_exists?(ref)` |
| `sha` | full commit sha |
| `before_sha` | NULL in DB; reader returns `project.repository.blank_ref` = `"0000000000000000000000000000000000000000"` (`ci/pipeline.rb:868-870`) |
| `user` | poster of the first status |
| `status` | aggregated, see 3.2; starts `created` |
| `started_at` | set once, on transition `[created,waiting_for_resource,preparing,pending] => running` (`ci/pipeline.rb:282-284`) |
| `finished_at` | set on every transition to success/failed/canceled (`:286-289`); never cleared when re-opened [no reset found] |
| `duration` | `update_duration` runs on ->success/failed/canceled (and ->manual) only if `started_at` (`:1151-1155`) and sums periods of **`Ci::Build`** jobs only (`lib/gitlab/ci/pipeline/duration.rb:135-143`; generic statuses are a different STI `type`) => **0** if the pipeline ever started, **NULL** if it never started (e.g. went created->success directly). |
| `queued_duration` | `(started_at \|\| finished_at) - created_at` in whole seconds; `nil` if no timestamps or result is 0 (`:1142-1149`) |
| `committed_at`, `yaml_errors`, `config_source`, `failure_reason` | NULL |
| `protected` | `project.protected_for?(ref)` (not in REST entity) |
| `locked` | DB default 1 (artifacts_locked) - n/a |
| `coverage` (computed) | mean of non-null `coverage` of latest statuses (`:943-946`); REST shows a string `"%.2f"` or null (`ci/pipeline_presenter.rb:68-73`) |

### 3.2 Status aggregation

Pipeline status is NOT computed in the POST request. After each job transition an after-commit hook enqueues
`PipelineProcessWorker` (`commit_status.rb:221-234`; skipped if the job is already `processed`, which is reset to false on every
status change `:124-135`). The worker runs `Ci::ProcessPipelineService` -> `AtomicProcessingService#process!`
(`app/services/ci/pipeline_processing/atomic_processing_service.rb:35-41`): update stage -> `pipeline.set_status(StatusCollection#status_of_all)`
(`:96-98`) -> Composite of the **latest** (`retried` false) jobs' `(status, allow_failure)` (`status_collection.rb:109-125`).
Consequently: pipeline `status` in a GET issued immediately after POST may lag by Sidekiq latency (pipeline still `created`). A simulator
should either apply synchronously or document the lag. `needs_processing?` = any latest job with `processed` false.

Composite rules in priority order (`lib/gitlab/ci/status/composite.rb:45-78`), set of distinct latest-job statuses S
(allow_failure jobs that failed/canceled count as `success_with_warnings`, manual+allow_failure as ignored; irrelevant for API-created
jobs because allow_failure is always false):
1. S subset {skipped} -> `skipped`
2. S subset {success, skipped} -> `success`
3. S subset {created} -> `created`
4. S subset {canceled, success, skipped} (contains canceled) -> `canceled`
5. S subset {pending, created, skipped} (contains pending) -> `pending`
6. any of {running, pending} -> `running`   (so success+pending, failed+pending, failed+running, success+running => `running`)
7. else (failed present with nothing live) -> `failed`  (failed+success, failed+canceled, failed+skipped => `failed`)
No jobs -> `success` fallback (`status_collection.rb:87`); with no jobs at all the pipeline stays `created` (never processed).
Then `Pipeline#set_status` (`ci/pipeline.rb:1093-1114`) maps to events (state machine `:220-275`):
- `pending` -> `enqueue`: `[created,manual,waiting*,preparing,skipped,scheduled]=>pending`, **`[success,failed,canceling,canceled]=>running`**,
  `running=>running`. So a finished pipeline that gets a new pending job becomes `running` (not pending) and does NOT reset `started_at`.
- `running` -> `run` (any except running); `success` -> `succeed`; `failed` -> `drop`; `canceled` -> `cancel`; `skipped` -> `skip`.
Side effects of transitions (`ci/pipeline.rb:282-440`): `PipelineHooksWorker` on every non-loopback transition (webhook, section 6);
`PipelineMetricsWorker` on `[created,pending,...]=>running` and `any=>success` (sets MR `pipeline`, section 4.3); on any completed status
(`success failed canceled skipped`) `AutoMergeProcessWorker({'pipeline_id'})` (section 5); on `success/failed` a `PipelineNotificationWorker`
(email) and Ci::Ref status update; `GraphqlTriggers.merge_request_merge_status_updated` for each open MR on `running` or completed.

### 3.3 Typical sequences

- `pending` -> pipeline created(`created`) -> `pending`. -> `running` job -> `running` (+`started_at`). -> `success` -> `success`
  (+`finished_at`, `duration` 0).
- Directly `success` first: created -> `success`; `started_at` NULL, `duration` NULL, `finished_at` set.
- `failed` first: created -> `failed`.
- Two names, one `success` one `pending`: `running`.
- Post `success` for `ci/a` then `pending` for `ci/b` (same sha/ref): same pipeline reopened: success -> `running`.

### 3.4 Entities

`API::Entities::Ci::PipelineBasic` (`lib/api/entities/ci/pipeline_basic.rb`), order:
`id, iid, project_id, sha, ref, status, source, created_at, updated_at, web_url`.
`web_url` = `Gitlab::Routing...project_pipeline_url` = `<base>/<namespace>/<project>/-/pipelines/<id>`.

`API::Entities::Ci::Pipeline` (`lib/api/entities/ci/pipeline.rb`) = Basic fields then, in order:
`before_sha, tag, yaml_errors, user (UserBasic), created_at*, updated_at*, started_at, finished_at, committed_at, duration, queued_duration,
coverage, detailed_status, archived`.
(* `created_at`/`updated_at` are exposed again in the subclass; in a Ruby hash a re-assigned key keeps its first position, so effective
order is `id, iid, project_id, sha, ref, status, source, created_at, updated_at, web_url, before_sha, tag, yaml_errors, user, started_at,
finished_at, committed_at, duration, queued_duration, coverage, detailed_status, archived` [grape-entity duplicate handling unverified].)

`PipelineBasicWithMetadata` = Basic + `name` (last). `PipelineWithMetadata` = Pipeline + `name` (last, after `archived`) - used by
`GET /pipelines/:id` and `/pipelines/latest`. `name` is `pipeline_metadata.name` -> `null` for external pipelines.

`detailed_status` (`DetailedStatusEntity`, `app/serializers/detailed_status_entity.rb`), order:
`icon, text, label, group, tooltip, has_details, details_path, illustration, favicon, [action only if has_action?]`.
For a pipeline `Status::Pipeline::Factory` is used (`lib/gitlab/ci/status/pipeline/*`): `has_details` = `can?(user, :read_pipeline)` (true for
readers), `details_path` = `/<ns>/<proj>/-/pipelines/<id>` (module `Common` overrides each status's nil), `has_action` false so no `action`
key, `illustration` is `null` (core raises NotImplementedError, rescued -> nil), `favicon` = `/assets/ci_favicons/favicon_status_<x>.png`
(production URLs carry an asset digest) [digest unverified].

| pipeline status | icon | text | label | group | tooltip |
|---|---|---|---|---|---|
| created | status_created | Created | created | created | created |
| pending | status_pending | Pending | pending | pending | pending |
| running | status_running | Running | running | running | running |
| success | status_success | Passed | passed | success | passed |
| failed | status_failed | Failed | failed | failed | failed |
| canceled | status_canceled | Canceled | canceled | canceled | canceled |
| skipped | status_skipped | Skipped | skipped | skipped | skipped |
| manual / blocked | status_manual | Blocked | waiting for manual action | manual | same as label |
(Not reachable via statuses API: manual, scheduled, preparing, waiting_*; success-with-warnings needs allow_failure jobs.)
Source files: `lib/gitlab/ci/status/{created,pending,running,success,failed,canceled,skipped}.rb`.

Example `GET /projects/5/pipelines/12` (success, started via running):
```json
{
  "id": 12, "iid": 3, "project_id": 5,
  "sha": "6104942438c14ec7bd21c6cd5bd995272b3faff6",
  "ref": "feature-1", "status": "success", "source": "external",
  "created_at": "2026-11-07T10:00:00.100Z", "updated_at": "2026-11-07T10:00:09.500Z",
  "web_url": "http://gitlab.example.com/root/proj/-/pipelines/12",
  "before_sha": "0000000000000000000000000000000000000000",
  "tag": false, "yaml_errors": null,
  "user": { "id": 1, "username": "root", "public_email": null, "name": "Administrator", "state": "active", "locked": false,
            "avatar_url": "...", "web_url": "http://gitlab.example.com/root" },
  "started_at": "2026-11-07T10:00:05.000Z", "finished_at": "2026-11-07T10:00:09.400Z",
  "committed_at": null, "duration": 0, "queued_duration": 5, "coverage": null,
  "detailed_status": {
    "icon": "status_success", "text": "Passed", "label": "passed", "group": "success", "tooltip": "passed",
    "has_details": true, "details_path": "/root/proj/-/pipelines/12", "illustration": null,
    "favicon": "/assets/ci_favicons/favicon_status_success.png"
  },
  "archived": false,
  "name": null
}
```

### 3.5 Endpoints

- `GET /projects/:id/pipelines` (`lib/api/ci/pipelines.rb:63-70`): needs `read_pipeline` + `read_build` (Reporter+ ; both
  `authorize!` -> 403). `Ci::PipelinesFinder` (`app/finders/ci/pipelines_finder.rb`): excludes child pipelines (`source != parent_pipeline`);
  filters `scope` (running|pending|finished|branches|tags), `status`, `ref`, `sha`, `username`, `yaml_errors`, `updated_*`, `created_*`,
  `source` (`external` valid), `name`; `order_by` in `id status ref updated_at user_id` (default `id`), `sort` default `desc`. Items rendered
  with `PipelineBasicWithMetadata` (Basic + `name`). Offset pagination. Example item:
  `{"id":12,"iid":3,"project_id":5,"sha":"...","ref":"feature-1","status":"success","source":"external","created_at":"...","updated_at":"...","web_url":"...","name":null}`.
- `GET /projects/:id/pipelines/:pipeline_id` (`:150-166`): `user_project.all_pipelines.find` (404 `{"message":"404 Not found"}`),
  `authorize! :read_pipeline` (403), entity `PipelineWithMetadata`. External pipelines are readable (policy treats them as external; `read_build`
  is enabled for reporters even if project CI is disabled, `pipeline_policy.rb:28-37`).
- `GET /projects/:id/merge_requests/:merge_request_iid/pipelines` (`lib/api/merge_requests.rb:685-688`, finder
  `app/finders/ci/pipelines_for_merge_request_finder.rb`): union of (a) `merge_request_event` pipelines of the MR and (b) branch pipelines
  `source_project.all_pipelines.ci_branch_sources.for_branch(source_branch)` with `sha IN recent MR diff head shas (<=100)`.
  `ci_branch_sources` = all CI sources except `merge_request_event`, and **includes `external`** (`enums/ci/pipeline.rb:ci_sources`).
  So an external pipeline appears iff `pipeline.ref == MR.source_branch`, `tag=false`, and `sha` is a head sha of any of the MR's
  diff versions. A status posted with a different `ref` (e.g. the target branch, or a branch picked by `branch_names_contains`)
  will NOT show here. Ordering: `merge_request_event` first, then `id DESC`. Entity `PipelineBasic` (no `name`). Needs `read_pipeline`
  on source/target project, otherwise `[]`.

---------------------------------------------------------------------------------------------------

## 4. Merge request effects

### 4.1 `head_pipeline` selection

`MergeRequest#find_diff_head_pipeline` (`app/models/merge_request.rb:2614`) =
`all_pipelines.for_sha_or_source_sha(diff_head_sha).first`, where `all_pipelines` (`:2179`) = the finder's `all` (3.5) sorted
`merge_request_event` first then `id DESC`. So: the newest pipeline whose `sha` (or `source_sha`) equals the MR's current
`diff_head_sha` and whose ref is the MR source branch (for external ones). An MR-event pipeline always wins over an external one
(none exist without a .gitlab-ci.yml).

`update_head_pipeline` (`:2185-2196`): if a pipeline is found, sets `head_pipeline` and, if changed, `update_columns(head_pipeline_id:,
retargeted: false)` (no callbacks, no `updated_at` bump, **no MR webhook**). If none found: nothing changes (an old id stays).

When it runs (the only 4 call sites):
1. `Ci::PipelineCreatedEvent` (published by CreateCommitStatusService right after the pipeline is created, `:125`) ->
   `MergeRequests::UpdateHeadPipelineWorker` (`lib/gitlab/event_store/subscriptions/merge_requests_subscriptions.rb:8`,
   `app/workers/merge_requests/update_head_pipeline_worker.rb`): for each `pipeline.all_merge_requests.opened` calls `update_head_pipeline`.
   `Pipeline#all_merge_requests` (`ci/pipeline.rb:1158-1167`) for non-MR pipelines = MRs with `source_project_id = project`, `source_branch = pipeline.ref`
   and the pipeline sha contained in the MR's latest diff commits (`MergeRequest.by_commit_sha`, `merge_request.rb:437`). Async (Sidekiq,
   event store) => `head_pipeline_id` lags the POST by a small delay.
2. MR reopen (`after_transition any => :opened`, `merge_request.rb:246-248` -> `UpdateHeadPipelineForMergeRequestWorker`).
3. MR creation: `NewMergeRequestWorker` -> `MergeRequests::AfterCreateService` (`after_create_service.rb:16-24`): `merge_request.update_head_pipeline`
   (sync path; with FF `async_mr_pipeline_creation` (default **false**, 18.8, `config/feature_flags/gitlab_com_derisk/async_mr_pipeline_creation.yml`)
   it goes through `MergeRequests::CreatePipelineWorker:67`). Between `POST /merge_requests` and this worker the MR is `preparing`
   (`detailed_merge_status = "preparing"`, `prepared_at` null).
4. `MergeRequests::CreatePipelineWorker` (async MR pipeline creation; not used by default).
Not updated on: pipeline status changes (the association is a live row so status is always current), later pushes (the stale id stays but
`diff_head_pipeline` returns nil once `sha` no longer matches), closed/merged MRs, a status posted for a non-head sha (no pipeline matches
`diff_head_sha`, nothing changes; but see below - the older sha's pipeline IS the one in `/pipelines`).
Posting a status to a sha that already has a pipeline appends to it and does NOT emit `PipelineCreatedEvent` => if the MR was created
*before* the pipeline existed this fires once; if the pipeline pre-existed but MR creation ran earlier than head-pipeline linking you rely on
paths 2-3.

`MergeRequest#diff_head_pipeline` (`:841-843`) = `head_pipeline` if `head_pipeline.sha == diff_head_sha || source_sha == diff_head_sha`, else nil.
This (not raw `head_pipeline`) drives mergeability, GraphQL `headPipeline`, auto-merge.

### 4.2 MR REST entities

List (`GET /merge_requests`, `/projects/:id/merge_requests`): `MergeRequestBasic` - **no `pipeline`/`head_pipeline` keys at all**.
Single (`GET /projects/:id/merge_requests/:iid`, also the PUT merge / create / update responses): `API::Entities::MergeRequest`
(`lib/api/entities/merge_request.rb`). Order = IssuableEntity (`id, iid, project_id, title, description, state, created_at, updated_at`)
then MergeRequestBasic (`merged_by, merge_user, merged_at, closed_by, closed_at, [title_html, description_html], target_branch,
source_branch, user_notes_count, upvotes, downvotes, author, assignees, assignee, reviewers, source_project_id, target_project_id, labels,
draft, imported, imported_from, work_in_progress, milestone, merge_when_pipeline_succeeds, merge_status, detailed_merge_status,
merge_after, sha, merge_commit_sha, squash_commit_sha, discussion_locked, should_remove_source_branch, force_remove_source_branch,
prepared_at, [allow_collaboration, allow_maintainer_to_push only for fork MRs], reference, references, web_url, time_stats, squash,
squash_on_merge, task_completion_status, has_conflicts, blocking_discussions_resolved`, then MergeRequest:
`subscribed, changes_count, latest_build_started_at, latest_build_finished_at, first_deployed_to_production_at, pipeline, head_pipeline,
diff_refs, merge_error, [rebase_in_progress], [diverged_commits_count], [first_contribution], user{can_merge}`.

- `pipeline` (`:26-28`): `PipelineBasic` of **`merge_request.metrics.pipeline`** (NOT head_pipeline), present only if project `builds`
  feature available; `null` when metrics has no pipeline. `merge_request_metrics.pipeline_id` is written only by `PipelineMetricsWorker`
  (`app/workers/pipeline_metrics_worker.rb`), enqueued on pipeline `[created|pending|...]=>running` and `any=>success`; for MRs whose
  `head_pipeline_id == pipeline.id` it sets `pipeline_id`, `latest_build_started_at = pipeline.started_at`, `latest_build_finished_at`
  (nil while active, `finished_at` on success). Consequences: a pipeline that only reached `pending`, or went straight to `failed`/`canceled`,
  never sets `pipeline` (stays `null` / stale older pipeline) while `head_pipeline` is set. `latest_build_started_at/finished_at` mirror that
  worker (they are also exposed under the same `build_available?` condition).
- `head_pipeline` (`:30-32`): `API::Entities::Ci::Pipeline` (detailed, section 3.4) of the raw `merge_request.head_pipeline`
  (NOT sha-checked), exposed when `read_pipeline` on project; key present with `null` when `head_pipeline_id` is null.

Example fragment after `running` status on head sha:
```json
"merge_when_pipeline_succeeds": false,
"merge_status": "can_be_merged",
"detailed_merge_status": "mergeable",
...
"latest_build_started_at": "2026-11-07T10:00:05.000Z",
"latest_build_finished_at": null,
"first_deployed_to_production_at": null,
"pipeline": {"id":12,"iid":3,"project_id":5,"sha":"...","ref":"feature-1","status":"running","source":"external","created_at":"...","updated_at":"...","web_url":"..."},
"head_pipeline": { ...full Pipeline entity as in 3.4 with "status":"running" and detailed_status running... },
```
(`pipeline`/`head_pipeline` statuses are read live from the pipeline row.)

### 4.3 `detailed_merge_status` and CI (`app/services/merge_requests/mergeability/detailed_merge_status_service.rb`)

Algorithm: `preparing` -> `checking` -> `unchecked` early returns; else run all mergeability checks with `skip_ci_check: true`
(`:30-37, 105-107`); if they all pass, then evaluate `CheckCiStatusService` separately: if it is `checking` or `failed` ->
`ci_still_running` when `merge_request.diff_head_pipeline_considered_in_progress?` else `ci_must_pass`; otherwise `mergeable`. If any other
check fails, that check's identifier wins (e.g. `draft_status`, `discussions_not_resolved`, `need_rebase`, `conflict`...)
and CI is not reported. Check order (`merge_request.rb:1647-1676`):
trivial [not_open, draft_status], light [merge_time], heavy [locked_lfs_files, discussions_not_resolved, ci_must_pass,
commits_status], git [need_rebase, conflict].

`CheckCiStatusService` (`check_ci_status_service.rb`):
- `inactive` (counts as pass) unless `merge_request.auto_merge_enabled?` **or** project/group `only_allow_merge_if_pipeline_succeeds`.
  => with the setting OFF and no auto-merge, a running / failed / missing pipeline never affects `detailed_merge_status` (stays `mergeable`).
- `pipeline_must_succeed?` = setting on, OR (auto-merge strategy is `merge_when_checks_pass` AND `merge_request.has_ci_enabled?`
  [= head_pipeline_id set, or any pipeline for the MR, or a CI integration, or pipeline being created]) (`:46-60`).
- `mergeable_ci_state?`: true if not must-succeed; false if no `diff_head_pipeline`; false if pipeline_creating; true if
  `allow_merge_on_skipped_pipeline` (project setting, cascading from group, default false) and pipeline `skipped`; else `pipeline.success?`.
- not mergeable -> `checking` if `pipeline_creating? || diff_head_pipeline.active?` (active = pending/running/preparing/waiting_*) else `failure`.
Resulting `detailed_merge_status` (all else passing, setting ON or auto-merge on):

| diff_head_pipeline | status |
|---|---|
| none (no pipeline / sha mismatch) | `ci_must_pass` (considered_in_progress is false unless `pipeline_creating?`) |
| `created`, `pending`, `running` | `ci_still_running` (`considered_in_progress?`: setting ON -> `!complete?`; setting OFF (auto-merge only) -> `active? \|\| created?`) |
| `success` | `mergeable` |
| `failed`, `canceled` | `ci_must_pass` |
| `skipped` | `ci_must_pass`, or `mergeable` if `allow_merge_on_skipped_pipeline` |
| `manual` (n/a for statuses) | setting ON: `ci_still_running` (not complete); auto-merge only: `ci_must_pass` |

GraphQL enum values are the upper-cased `CI_MUST_PASS`, `CI_STILL_RUNNING`, ... (`app/graphql/types/merge_requests/detailed_merge_status_enum.rb:21-26`).
`merge_status` (`checking|unchecked|can_be_merged|cannot_be_merged|cannot_be_merged_recheck`) is git/conflict based only
(`mergeability_check_service.rb`); CI never changes it. The branch-protection "pipeline must succeed" does not alter `merge_status`.
Project setting names (API/project fields): `only_allow_merge_if_pipeline_succeeds`, `allow_merge_on_skipped_pipeline` (`app/models/project.rb:616,1183`).

### 4.4 `PUT /projects/:id/merge_requests/:iid/merge` (`lib/api/merge_requests.rb:808-855`)

1. `unauthorized!` (401 `{"message":"401 Unauthorized"}`) if the user cannot update the target branch.
2. `check_sha_param!`: if `sha` param differs from `diff_head_sha` -> 409 `{"message":"SHA does not match HEAD of source branch: <diff_head_sha>"}`
   (`helpers.rb:600-604`).
3. No `auto_merge`/`merge_when_pipeline_succeeds` (or false): `merge_request.mergeable?` (`merge_request.rb:1625`: all state checks incl.
   CI check with `skip_ci_check` unset, then git checks). If false -> **405** `{"message":"405 Method Not Allowed"}`. This is what a client sees
   when setting ON and pipeline is running/failed/skipped/missing, or draft, unresolved threads, etc. If true -> `MergeService` runs
   **synchronously**; if after it `merge_request.merged?` is false -> **422** `{"message":"Branch cannot be merged"}` (e.g.
   `merge_error` "Branch has been updated since the merge was requested. Please review the changes." from `merge_service.rb`'s `updated_check!`).
   With setting OFF, no auto-merge and mergeable otherwise, the MR merges even though the external pipeline is `running`/`failed`.
4. Success -> 200 with the `API::Entities::MergeRequest` (state `merged`, `merge_commit_sha`...).
So 422 does not occur for CI reasons; CI blocks yield 405 (or auto-merge, section 5).

---------------------------------------------------------------------------------------------------

## 5. Auto-merge (`auto_merge: true` / deprecated `merge_when_pipeline_succeeds: true`)

Only strategy: `merge_when_checks_pass` (`AutoMergeService::STRATEGY_MERGE_WHEN_CHECKS_PASS`, `app/services/auto_merge_service.rb:6`).
API branch (`lib/api/merge_requests.rb:818-846`):

```
strategy_available = AutoMergeService.available_strategies(mr).include?(mr.default_auto_merge_strategy)   # = merge_when_checks_pass
if strategy_available:   AutoMergeService#execute(mr, strategy)   -> result symbol ignored; response is the MR entity, 200
elsif pipeline_allows_merge?(mr):                       # lib/api/merge_requests.rb:136-141
    422 "Branch cannot be merged" unless mr.mergeable?; MergeService synchronously; 422 unless merged?
else: 405 "405 Method Not Allowed"
```
`pipeline_allows_merge?` = `mr.diff_head_pipeline_success?` (FF `merge_immediately_when_no_pipeline`, 18.11, default **false**; if enabled:
`(!pipeline_creating? && !diff_head_pipeline) || diff_head_pipeline_success?`).

`MergeWhenChecksPassService#availability_details` (`app/services/auto_merge/merge_when_checks_pass_service.rb:49-57`, base `base_service.rb:98-116`):
available iff user may merge AND all mergeability checks pass with `skip_ci_check` (+ approved/ rebase skips) (`merge_request.rb:1574-1590`)
AND NOT (`mr.mergeable?` [full checks incl. CI] `&& !diff_head_pipeline_considered_in_progress?`). In words:
- If the CI check currently fails the MR counts as "not mergeable" => auto-merge available (pipeline running, failed, skipped, no pipeline when setting ON).
- Setting OFF and no merge-blocking issues: available only while the head pipeline is `created/pending/running` (`active? || created?`) or being created;
  with a finished (success/failed/...) or missing pipeline `mergeable?` is true and not in progress => NOT available => falls to
  `pipeline_allows_merge?` -> success pipeline: merges immediately (sync, 200 merged); failed/missing: **405**.
- Any non-CI check failing (draft, discussions, conflict, not open, ...) => unavailable => `pipeline_allows_merge?` -> 405 when pipeline not success;
  if the pipeline is success and `mergeable?` false => 422 `"Branch cannot be merged"`.

Running external pipeline, setting ON or OFF: `PUT /merge {"auto_merge":true}` -> **200** MR entity with `"merge_when_pipeline_succeeds": true`,
`detailed_merge_status` `"ci_still_running"` (CI check now active due to auto-merge), state still `opened`. Side effects of
`AutoMerge::BaseService#execute` (`base_service.rb:10-23`): in a transaction sets merge params (`sha` = `params[:sha] || diff_head_sha`,
`commit_message`, `squash_commit_message`, `should_remove_source_branch`, `auto_merge_strategy: "merge_when_checks_pass"`),
`auto_merge_enabled=true` (column `merge_when_pipeline_succeeds`), `merge_user = current_user`; system note (below); `notify` -> notification email
`merge_when_pipeline_succeeds`; `AutoMergeProcessWorker({'merge_request_id'})` (immediate processing attempt: no-op while CI not green).

System note (author = acting user, `action: 'merge'`, `app/services/system_notes/merge_requests_service.rb:6-10`):
`enabled an automatic merge when all merge checks for <sha> pass` where `<sha>` = full `merge_params['sha']` (the head sha).
Cancel (`POST .../cancel_merge_when_pipeline_succeeds`, 406 `Can't cancel the automatic merge` if not enabled; 200 MR): `canceled the automatic merge`.
Abort (system-initiated): `aborted the automatic merge because <reason>` (`:19-27`; reason strings used elsewhere: `the source branch was updated. [Learn more](...)`
(refresh_service.rb:271), `target branch was changed` (update_service.rb:236), `merge request was closed` (close_service.rb:27)).
**No abort happens on pipeline failure** in 18.11 (see below).

Processing - `MWCP#process` (`merge_when_checks_pass_service.rb:16-31`), invoked by `AutoMergeService#process` from `AutoMergeProcessWorker`
(`app/workers/auto_merge_process_worker.rb`, queue `auto_merge`, dedup until_executed, retry 3):
- Triggers: (a) at enable time; (b) every pipeline entering a completed status (`success failed canceled skipped`, `ci/pipeline.rb:364-367`
  -> `{'pipeline_id'}` -> `pipeline.all_merge_requests.with_auto_merge_enabled`); (c) `merge_status` -> `can_be_merged` (`merge_request.rb:308-313`); (d) event store events
  (draft state change, discussions resolved, title/description update, `MergeableEvent`); (e) project setting changes
  (`project_setting.rb:141`); (f) MR refresh service after push (delayed).
- Feature flag `mwcp_skip_ci_guard` (18.11, `gitlab_com_derisk`, default **false**): with it OFF (default) `process` first does
  `return if mr.has_ci_enabled? && !mr.diff_head_pipeline_success?` (`:19-24`); so with any pipeline linked the MR keeps waiting until
  `diff_head_pipeline` is `success` (a `skipped` pipeline would wait forever - the bug the flag fixes; with flag ON the CI check inside
  `mergeable?` decides, honouring `allow_merge_on_skipped_pipeline`).
- Then `return unless mr.mergeable?` (all checks, including CI which is now enforced because auto_merge is enabled and CI-enabled), then
  `mr.merge_async(merge_user_id, merge_params)` -> `MergeWorker` (Sidekiq, `app/workers/merge_worker.rb`) -> `MergeRequests::MergeService`
  -> git merge, state `merged`, post-merge (source branch removal if requested), system note "merged" etc.
- Timing: statuses POST success -> job success after-commit -> `PipelineProcessWorker` (sets pipeline success) -> after-commit
  `AutoMergeProcessWorker` -> `MergeWorker`: three chained Sidekiq jobs, i.e. the MR merges a moment (sub-second to seconds) after the final POST returns.
- Pipeline **fails**: `AutoMergeProcessWorker` runs, guard returns early (flag OFF), nothing is changed: `auto_merge_enabled` stays true,
  no abort note, no cancel; `detailed_merge_status` becomes `ci_must_pass`; MR stays waiting. If a new status turns the same pipeline
  `running` and then `success` (same pipeline reopened - section 3.2), the completion event fires again and it merges. A push to the source
  branch aborts auto-merge with the "source branch was updated" note (refresh_service.rb:260-272).
- Pipeline `canceled`: same as failed (waiting).
Other flags: `merge_immediately_when_no_pipeline` (above); `async_mr_pipeline_creation` (4.1); `ci_expire_pipeline_cache_workers` (18.11, default false, only cache expiry);

---------------------------------------------------------------------------------------------------

## 6. Webhooks

- Pipeline events: every non-loopback pipeline status transition enqueues `PipelineHooksWorker` (`ci/pipeline.rb:327-341`, skipped if
  `pipeline.user` blocked) -> `Ci::Pipelines::HookService` -> `project.execute_hooks(data, :pipeline_hooks)` if the hook has `pipeline_events: true`
  (also integrations). Worker uses `data_consistency :delayed` and builds data at execution time (so status in payload is the current one).
  Transitions for an external pipeline: e.g. `created->pending`, `pending->running`, `running->success`; never a hook for "created" itself (no
  transition on creation) and none for repeat posts that keep the same pipeline status (loopback). Reopen produces `success->running`.
- Payload (`lib/gitlab/data_builder/pipeline.rb`), key order: `object_kind: "pipeline"`, `object_attributes {id, iid, name (null), ref,
  ref_status_name, tag, sha, before_sha, source ("external"), status, detailed_status (the label e.g. "passed"), stages (["external"]),
  created_at, finished_at, duration, queued_duration, protected_ref, variables ([]), url}`, `merge_request` (**null** for non-MR-event
  pipelines), `user` (hook_attrs of poster), `project` (hook_attrs), `commit` (hook_attrs of commit or null), `builds` (**[]**: built from
  `pipeline.latest_builds`, which are `Ci::Build` only, so generic commit statuses do not appear). `source_pipeline` only for downstream pipelines.
  `created_at` format in hooks is `"2026-11-07 10:00:00 UTC"` (hook serializer) [unverified]. Header `X-Gitlab-Event: Pipeline Hook`.
- Job events (`job_events`/`build` hooks): fired from `Ci::Build` only (`app/models/ci/build.rb:267,390,422` -> `Ci::ExecuteBuildHooksWorker`).
  `GenericCommitStatus` never triggers job hooks. So no job events for the statuses API.
- MR (`merge_request_events`) hooks: nothing fires when `head_pipeline_id` is updated (`update_columns`) or when pipeline status changes.
  `head_pipeline_id` is merely a field inside the MR hook `object_attributes` when the hook fires for other reasons
  (`lib/gitlab/hook_data/merge_request_builder.rb:14`). The auto-merge enable/merge produce normal MR update/merge hooks. The pipeline payload's
  `merge_request` block (when present for MR-event pipelines) contains `detailed_merge_status`, but external pipelines have none.

---------------------------------------------------------------------------------------------------

## 7. System notes on the MR caused by statuses/pipelines

None. grep of `app/services/system_notes/*` and `system_note_service.rb` finds no pipeline/status note in the code. Only the
auto-merge notes in section 5 (`enabled an automatic merge when all merge checks for <sha> pass`, `canceled the automatic merge`,
`aborted the automatic merge because ...`) and the normal merge notes. Posting statuses or pipelines changing state creates no note.
(Emails: `PipelineNotificationWorker` on success/failed; not notes.)

---------------------------------------------------------------------------------------------------

## 8. GraphQL

`MergeRequest.headPipeline` (`app/graphql/types/merge_request_type.rb:174`) is `Types::Ci::PipelineType`, nullable, resolved with
`method: :diff_head_pipeline` (sha-checked, unlike REST `head_pipeline`). Authorization `read_pipeline` on the pipeline.
`MergeRequest.pipelines` (resolver `Resolvers::MergeRequestPipelinesResolver`, max 500) and `pipelineCreationRequests` exist.

Commonly used `Pipeline` fields (`app/graphql/types/ci/pipeline_type.rb`, `pipeline_interface.rb`):
- `id: ID!` -> `gid://gitlab/Ci::Pipeline/12`
- `iid: String!` -> `"3"` (String, not Int)
- `status: PipelineStatusEnum!` -> upper-case enum name: `CREATED, WAITING_FOR_RESOURCE, PREPARING, WAITING_FOR_CALLBACK, PENDING, RUNNING,
  FAILED, SUCCESS, CANCELING, CANCELED, SKIPPED, MANUAL, SCHEDULED` (`pipeline_status_enum.rb`)
- `sha: String` (full sha; arg `format: SHORT|LONG`)
- `beforeSha`, `path: String` (`/<ns>/<proj>/-/pipelines/12`), `ref: String`, `refPath`, `source: String` -> `"external"`,
  `type: String!`, `complete: Boolean!` (completed statuses only), `active: Boolean!`, `warnings`, `duration: Int`, `queuedDuration`,
  `coverage: Float`, `createdAt/updatedAt/startedAt/finishedAt/committedAt`, `user`, `commitPath`, `latest`, `totalJobs`, `failedJobsCount`,
  `yamlErrors: Boolean!`, `mergeRequest`, `name`.
- `detailedStatus: DetailedStatus!` (`detailed_status_type.rb`): `id` (`"<statusId>-<parentId>"`), `name` (upper-case machine name e.g.
  `SUCCESS`, `RUNNING`, `PENDING`, `FAILED`, `CANCELED`, `SKIPPED`, `CREATED`; from the status class name, `status/core.rb` `name`), `label`
  (e.g. `passed`), `text` (deprecated, `Passed`), `group` (deprecated, `success`), `icon` (deprecated), `tooltip`, `hasDetails`,
  `detailsPath`, `favicon`, `action` (null), `deploymentDetailsPath` (null).
- `MergeRequest.detailedMergeStatus` enum: `CI_MUST_PASS`, `CI_STILL_RUNNING`, `MERGEABLE`, ... (values section 4.3).
Example:
```json
{"data":{"project":{"mergeRequest":{"headPipeline":{
  "id":"gid://gitlab/Ci::Pipeline/12","iid":"3","status":"RUNNING",
  "sha":"6104942438c14ec7bd21c6cd5bd995272b3faff6","path":"/root/proj/-/pipelines/12","source":"external",
  "detailedStatus":{"name":"RUNNING","label":"running","text":"Running","group":"running","icon":"status_running","hasDetails":true,
                    "detailsPath":"/root/proj/-/pipelines/12","favicon":"/assets/ci_favicons/favicon_status_running.png","tooltip":"running"}}}}}}
```
`headPipeline` is `null` when no pipeline matches the MR's current diff head sha. Updated via MR subscription
`GraphqlTriggers.merge_request_merge_status_updated` on pipeline running/completion (`ci/pipeline.rb:432-446`).

---------------------------------------------------------------------------------------------------

## 9. Simulator checklist (summary of exact rules)

1. POST: validate commit -> state -> ref -> pipeline_id; find pipeline by newest (sha[,ref][,id]); else create external pipeline
   (`status created`, `iid` next, `before_sha` null, poster as `user`); publish "pipeline created" => update `head_pipeline_id` for open MRs
   with `source_branch == pipeline.ref` whose latest diff contains the sha, if `find_diff_head_pipeline` returns one.
2. Find live job (pending|running) by name+ref+user+pipeline; update it, otherwise insert new; mark other latest same-name jobs `retried`.
3. Apply transition table of 1.5; 400 `{"message":"Cannot transition status via :<event> from :<state>"}` on illegal ones; respond **201**
   with CommitStatus entity.
4. Recompute pipeline status (Composite 3.2) async/sync; set `started_at` on first `->running` from created/pending, `finished_at` on any
   terminal, `duration` 0 when started else null; `queued_duration`; emit pipeline webhook per transition; write MR metrics.pipeline on
   `->running` and `->success`; on completed status run auto-merge processing.
5. MR: `ci_must_pass`/`ci_still_running` only when `only_allow_merge_if_pipeline_succeeds` OR auto-merge enabled; PUT merge => 405 when not
   mergeable (CI), 200+auto-merge note when auto_merge requested and CI-not-green / in progress; waits (no abort) on failure.
