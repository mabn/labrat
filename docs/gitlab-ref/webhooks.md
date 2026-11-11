# GitLab CE 18.11 webhooks reference

Source root: GitLab CE source, `/home/mabn/dev/gitlab-run-ce/gitlab-src` (paths below are relative to it).
Scope: project hooks + system hooks; events push, tag_push, merge_request, note (MR comments), emoji. `gems/` (gitlab-http, labkit) is NOT in the tree, so a few facts (URL-blocker messages, redirect behaviour, JSON encoding of Time) are marked **[inferred]**.

Conventions: "async path" = real event delivered via Sidekiq (`WebHookWorker`); "sync/test path" = `force: true` test deliveries. They differ in timestamp formatting (see section 3.0).

---------------------------------------------------------------------------------------------------

## 1. Hook management API

### 1.1 Authorization
- All project-hook routes: `before { authenticate! }`, then `authorize! (GET ? :read_web_hook : :admin_web_hook), user_project` (`lib/api/project_hooks.rb:8-12`). Roles: `read_web_hook` and `admin_web_hook` are granted to Maintainer and Owner only (`config/authz/roles/maintainer.yml:55,140`, `owner.yml:77,128`). Developer and below get 403 (`{"message":"403 Forbidden"}`); unknown project 404.
- System-hook routes: `authenticate!` + `authorize! ability` with no subject (`lib/api/system_hooks.rb:10-15`); `GlobalPolicy` enables `read_web_hook`/`admin_web_hook` for admins only (`app/policies/global_policy.rb:159-160`). Non-admin => 403.
- Project resource id: `:id` is ID or URL-encoded path (`project_hooks.rb:56-59`).

### 1.2 Project hook routes (`lib/api/project_hooks.rb`)
| Verb + path | Source | Notes |
|---|---|---|
| `GET /projects/:id/hooks` | :75-77 | `paginate(user_project.hooks)`, entity `ProjectHook` with `with_url_variables: false, with_custom_headers: false` => **`url_variables` and `custom_headers` keys omitted entirely** from list items. Default order = association default (no explicit order; effectively id asc). |
| `GET /projects/:id/hooks/:hook_id` | :92-95 | `user_project.hooks.find` (404 `{"message":"404 Not found"}`), full entity incl. `url_variables`/`custom_headers` (masked). |
| `POST /projects/:id/hooks` | :155-165 | `url` required. 201 on success. |
| `PUT /projects/:id/hooks/:hook_id` | :113-115 | 200 with entity. |
| `DELETE /projects/:id/hooks/:hook_id` | :129-135 | `destroy_conditionally!` => 204 empty body (`lib/api/helpers.rb:51-63`); honors `If-Unmodified-Since`. Missing hook 404. |
| `PUT /projects/:id/hooks/:hook_id/url_variables/:key` | `lib/api/hooks/url_variables.rb:23-32` | body `value` (String, required). 204. Merges into existing map. Failure => 422 `Illegal key or value`. |
| `DELETE /projects/:id/hooks/:hook_id/url_variables/:key` | :40-50 | 404 `{"message":"404 URL variable Not Found"}`-style via `not_found!('URL variable')` if key absent; 204; 422 `Could not unset variable`. |
| `PUT/DELETE .../custom_headers/:key` | `lib/api/hooks/custom_headers.rb:77-105` | same as above; `key` limit 255 chars; errors `Illegal key or value`, `Could not unset custom header`, 404 `not_found!('Custom header')`. |
| `GET /projects/:id/hooks/:hook_id/events` | `lib/api/hooks/events.rb:122-128` | see 1.7 |
| `POST /projects/:id/hooks/:hook_id/test/:trigger` | `lib/api/hooks/trigger_test.rb:225-241` | see 1.6 |
| `POST /projects/:id/hooks/:hook_id/events/:hook_log_id/resend` | `lib/api/hooks/resend_hook.rb:150-164` | see 1.8 |

Validation/CRUD happens through `WebHooks::CreateService` (`app/services/web_hooks/create_service.rb:127-144`) and `API::Helpers::WebHooksHelpers#update_hook/save_hook` (`lib/api/helpers/web_hooks_helpers.rb:51-88`).

### 1.3 Params (`project_hooks.rb:24-53`, `web_hooks_helpers.rb:8-28`)
All optional except `url` on POST. Types via Grape; booleans accept true/false/"true"/"1" etc.
`name` String(<=255), `description` String(<=2048), `url` String (required POST, optional PUT), `push_events`, `issues_events`, `confidential_issues_events`, `merge_requests_events`, `tag_push_events`, `note_events`, `confidential_note_events`, `job_events`, `pipeline_events`, `wiki_page_events`, `deployment_events`, `feature_flag_events`, `releases_events`, `milestone_events`, `emoji_events`, `resource_access_token_events`, `resource_deploy_token_events`, `vulnerability_events` (all Boolean); `enable_ssl_verification` Boolean; `token` String (write-only); `push_events_branch_filter` String; `custom_webhook_template` String (<=4096); `branch_filter_strategy` String with `values: wildcard|regex|all_branches` (`WebHook.branch_filter_strategies.keys`; invalid => 400 `{"error":"branch_filter_strategy does not have a valid value"}`); `url_variables` Array of `{key: String req, value: String req}`; `custom_headers` Array of `{key, value}` same shape.
Missing required `url` on POST => 400 `{"error":"url is missing"}`.

**Defaults are DB defaults** (`db/structure.sql` `CREATE TABLE web_hooks`, ~33383): `push_events` TRUE; `tag_push_events` FALSE (nullable); `issues_events`, `merge_requests_events`, `note_events`, `pipeline_events`, `wiki_page_events`, `confidential_issues_events`, `repository_update_events`, `job_events`, `deployment_events`, `releases_events`, `feature_flag_events`, `emoji_events`, `resource_access_token_events`, `resource_deploy_token_events`, `vulnerability_events`, `milestone_events` FALSE; **`confidential_note_events` has NO default => `null` when unset**; `enable_ssl_verification` TRUE (nullable); `branch_filter_strategy` smallint 0 = `wildcard`; `recent_failures` 0; `disabled_until` NULL; `push_events_branch_filter`, `name`, `description`, `custom_webhook_template` NULL; `filter` jsonb `{}` (not exposed by API).
SystemHook overrides defaults: `push_events` false, `repository_update_events` TRUE, `merge_requests_events` false (`app/models/hooks/system_hook.rb:85-87`).
No `PUT` partial semantic surprises: `declared_params(include_missing: false)` => only sent keys are written. An empty update (no known params) => 400 `{"message":"No parameters provided"}` (`web_hooks_helpers.rb:70`).
`url_variables`/`custom_headers` on **PUT are merged** into existing maps (`:60-68`); on POST they replace (create). Only non-empty arrays are applied.

### 1.4 Validations and exact errors (`app/models/concerns/web_hooks/hook.rb`)
API-level mapping (`create_service.rb:135-142`, `web_hooks_helpers.rb:79-87`), checked in this order, HTTP 422, body `{"message": ...}`:
1. any error on `url` => **`Invalid url given`** (this hides the model message; applies to blank url, bad scheme, missing url var, blocked local URL, length >8192).
2. error on `push_events_branch_filter` => **`Invalid branch filter given`**.
3. error on `custom_headers` => **`Custom headers validation failed: <errors joined with ", ">`** (e.g. `key is too long (maximum is 255 characters)`, or JSON-schema messages).
4. otherwise on create: `hook.errors.full_messages.to_sentence`; on update: `render_validation_error!(hook, 422)` => `{"message":{"attr":["msg"]}}` map form (single model => `messages.first`, i.e. the attribute=>[msgs] hash).
   Examples: plan limit `Maximum number of project hooks (100) exceeded` (Limitable on `:project` scope, `project_hook.rb:29-31`, `plan_limits.project_hooks` default 100; `app/models/concerns/limitable.rb:60-66`); `name`/`description` too long; `custom_webhook_template` "is too long (maximum is 4096 characters)", "contains a numeric value that is too large", "exceeds allowed JSON complexity limits" (`hook.rb:251-268`).

URL rules (`hook.rb:61-67,85`):
- `url` presence, length <= 8192 (`MAX_PARAM_LENGTH`).
- `public_url: true` (PublicUrlValidator < AddressableUrlValidator, `app/validators/public_url_validator.rb`): schemes `http`/`https` only, `allow_localhost: false`, `allow_local_network: false` — **unless** instance setting `allow_local_requests_from_web_hooks_and_services` (default FALSE, `structure.sql:14508`) is true, in which case both are allowed (`addressable_url_validator.rb:109-117`). Also `outbound_local_requests_allowlist` setting honored, `deny_all_requests_except_allowed`. DNS rebinding check is NOT applied at save time (`dns_rebind_protection: false`, `:55-66`) but is applied at delivery.
- Validation message form (model-level, before API remap): blank => `Url must be a valid URL`; blocked => `Url is blocked: <reason>` with reason from gitlab-http UrlBlocker, e.g. "Requests to localhost are not allowed" / "Requests to the local network are not allowed" **[inferred, gem not in tree]**; the API always returns `Invalid url given` for these.
- If `url_variables?` (hook has any url variables) the static `public_url` check is skipped on `url`; instead `interpolated_url` is validated (`hook.rb:63-67,85`). `url` containing `{name}` placeholders (regex `VARIABLE_REFERENCE_RE = /\{([A-Za-z]+[0-9]*(?:[._-][A-Za-z0-9]+)*)\}/`, `:26`) requires every name be a key in `url_variables`, else url error `Invalid URL template. Missing keys: [...]` (`:234-245`) => API `Invalid url given`.
- SystemHook: `validates :url, system_hook_url: true, unless url_variables?` (`system_hook.rb:91`); `SystemHookUrlValidator` allows local when `allow_local_requests_from_system_hooks` (default **TRUE**, `structure.sql:14509`); system hooks skip the base `public_url` validation (`validate_public_url?` false, `:107-110`) and `allow_local_requests?` follows the system-hooks setting (`:95-97`). Also `organization_id` presence validated (set from `Current.organization` on create, `create_service.rb:130`).
- `token`: length <= 8192, no `\n` (`:69`). **Changing `url` without also sending `token` in the same request nulls the token** (`before_validation :reset_token`, `:74,178-180`). Same for `url_variables` (`reset_url_variables`, on update only, `:182-187`: cleared if interpolated url changed and a variable key/value overlaps) and `custom_headers` (cleared if the *interpolated* URL changed, `:189-196`).
- `url_variables` JSON schema (`app/validators/json_schemas/web_hooks_url_variables.json`): object, <=20 props, key pattern `^[A-Za-z]+[0-9]*(?:[._-][A-Za-z0-9]+)*$`, value string 1..2048.
- `custom_headers` schema (`web_hooks_custom_headers.json`): <=20 props, key regex `\A[A-Za-z]+[0-9]*(?:[._-][A-Za-z0-9]+)*\z` (1..255), value string 1..2048, no CR/LF, `%0a/%0d`, `\n`/`\r` sequences, no leading/trailing whitespace.
- `push_events_branch_filter`: if strategy `all_branches` the filter is forced to nil (`:77,247-249`); if `regex` must be a valid RE2 (`untrusted_regexp` validator); if `wildcard` (default): whitespace is squished (in place), must be a valid git ref name after `*`->`x` replacement, else `is not a valid branch name`; > 4000 chars => `is longer than the allowed length of 4000 characters.` (`app/validators/web_hooks/wildcard_branch_filter_validator.rb`).
- Deleting: `WebHooks::DestroyService` (`web_hooks/destroy_service.rb`) logs are not purged manually (partition drop).

### 1.5 Response entities (field ORDER matters)
`API::Entities::ProjectHook < Hook` (`lib/api/entities/hook.rb`, `project_hook.rb`). JSON key order:
```
id, url, name, description, created_at, push_events, tag_push_events, merge_requests_events,
repository_update_events, enable_ssl_verification,
[organization_id  — SystemHook only],
alert_status, disabled_until,
[url_variables — omitted in list], push_events_branch_filter, branch_filter_strategy,
custom_webhook_template, [custom_headers — omitted in list],
-- ProjectHook additions --
project_id, issues_events, confidential_issues_events, note_events, confidential_note_events,
pipeline_events, wiki_page_events, deployment_events, feature_flag_events, job_events,
releases_events, milestone_events, emoji_events, resource_access_token_events,
resource_deploy_token_events, vulnerability_events
```
System hook entity (`Entities::Hook`) = the first block only, with `organization_id` present (always for SystemHook) and `url_variables`/`custom_headers` always present.
- `token` is never returned. `url` is returned as stored (placeholders `{x}` and userinfo intact).
- `url_variables`: `masked_url_variables` => `[{"key":"name"}, ...]` (array of key objects, values never shown), `[]` if none (`hook.rb:164-166`); `custom_headers` same via `masked_custom_headers` (`:168-170`). (`SECRET_MASK = '************'` is only used by `masked_token`, not by API.)
- `alert_status`: string, `"executable"` | `"temporarily_disabled"` | `"disabled"`. **`auto_disabling_web_hooks` feature flag is `default_enabled: false` (`config/feature_flags/ops/auto_disabling_web_hooks.yml`) and only `ProjectHook` participates (`auto_disabling.rb:8,22-27`)** => on a stock instance `alert_status` is always `"executable"`, `disabled_until` null, `recent_failures` never increments. System hooks always `"executable"`. Implement the full state machine (1.9/2.6) but gate it behind a flag defaulting off.
- `created_at`: API format `2012-05-28T04:42:42.000Z` (millisecond ISO8601, UTC). `disabled_until` same format or null.
- `repository_update_events`, `tag_push_events` etc. booleans. `branch_filter_strategy` string.

### 1.6 Test endpoint — `POST /projects/:id/hooks/:hook_id/test/:trigger`
`lib/api/hooks/trigger_test.rb:217-241`
- `trigger` must be in `ProjectHook.triggers.values.map(&:to_s)` (:219-223), `confidential_issues_events, confidential_note_events, deployment_events, emoji_events, feature_flag_events, issues_events, job_events, merge_requests_events, milestone_events, note_events, pipeline_events, push_events, releases_events, resource_access_token_events, resource_deploy_token_events, tag_push_events, wiki_page_events` (`app/models/hooks/project_hook.rb:9-27`, `triggerable_hooks.rb:21-43`). Anything else => 400 `{"error":"trigger does not have a valid value"}`.
- Rate limit when FF `web_hook_test_api_endpoint_rate_limit` (ops, default TRUE): 5 requests/minute per `[project, current_user]` (`application_rate_limiter.rb:123`); exceeding => 429 `{"message":{"error":"This endpoint has been requested too many times. Try again later."}}` + `Retry-After: 60` (`lib/api/helpers/rate_limiter.rb:19-23`).
- Service: `TestHooks::ProjectService` (`app/services/test_hooks/project_service.rb:47-83`, `base_service.rb:15-25`) builds data then `hook.execute(data, trigger_key, force: true)` — **synchronous**, ignores disabled state (`force`), log row written inline (`web_hook_service.rb:188-190`).
- Success when receiver status in 200..299 => **`201` with body `{"message":"201 Created"}`** (`created!` => `render_api_error!('201 Created', 201)`, `helpers.rb:648`).
- Otherwise `422 {"message": <result.message>}`; `result.message` is: the receiver's response body (full, as received, `ServiceResponse.success(message: response.body)`) when it responded but not 2xx (incl. 3xx), or the exception text for connection errors (e.g. timeout/refused/SSL/`URL is blocked: ...`), or `Error while parsing rendered custom webhook template: ...`, `Silent mode enabled`, `Recursive webhook blocked`. Empty receiver body => `{"message":""}`.
- No-data errors (`Integrations::ProjectTestData`, `app/services/concerns/integrations/project_test_data.rb`): raised as ArgumentError and returned via `error(e.message)` => 422 `{"message": ...}`:
  - `note_events` => `Ensure the project has notes.` (:24)
  - `issues_events`/`confidential_issues_events` => `Ensure the project has issues.`
  - `merge_requests_events` => **`Ensure the project has merge requests.`** (:40)
  - `job_events` `Ensure the project has CI jobs.`; `pipeline_events` `Ensure the project has CI pipelines.`; `wiki_page_events` `Ensure the wiki is enabled and has pages.`; `releases_events` `Ensure the project has releases.`; `emoji_events` `Ensure the project has notes.`
  - `confidential_note_events`, `deployment_events`, `feature_flag_events` have no data builder case => `data` nil => **`Testing not available for this hook`** (`base_service.rb:18`).
  - push/tag_push/milestone/access-token/deploy-token never raise no-data (see section 5).

### 1.7 Hook events (logs) — `GET /projects/:id/hooks/:hook_id/events`
`lib/api/hooks/events.rb`. Params: `status` (comma-separated list; each of any HTTP code string 100..599 known to `Rack::Utils::HTTP_STATUS_CODES` or `successful|client_failure|server_failure`), `per_page` (default **20**, allowed 1..20), `page`. Finder `WebHooks::WebHookLogsFinder` (`app/finders/web_hooks/web_hook_logs_finder.rb:14-60`): `successful`=200..299, `client_failure`=400..499, `server_failure`=500..599 plus `response_status = 'internal error'`; limited to **last 7 days** (`recent(7)`, = since beginning of day 7 days ago) ordered by `created_at DESC` (`web_hook_log.rb:33-39,45-53`). Entity `Entities::WebHookLog` (`web_hook_log.rb`) order: `id, url, trigger, request_headers, request_data, response_headers, response_body, execution_duration, response_status, created_at`. `trigger` is the hook_name string (`push_hooks`, `merge_request_hooks`, `note_hooks`, `emoji_hooks`, `tag_push_hooks`, `system_hooks`, ...). `response_status` is a STRING (`"200"` or `"internal error"`). `request_headers['X-Gitlab-Token']` shown as `[REDACTED]` (model getter `web_hook_log.rb:68-72`, and stored redacted too).
Requires `read_web_hook` on hook (finder returns none otherwise).

### 1.8 Resend — `POST /projects/:id/hooks/:hook_id/events/:hook_log_id/resend`
`resend_hook.rb:150-164`, `app/services/web_hooks/events/resend_service.rb`. Rate limit FF `web_hook_event_resend_api_endpoint_rate_limit` default TRUE: 5/min per `[project, user]` (same 429 body). `hook.web_hook_logs.find(id)` (404). Errors 422 `{"message": ...}`: `The current user is not authorized to resend a hook event.`; `The hook URL has changed. This log entry cannot be retried.` (compares sha256 of current `interpolated_url` with `url_hash` stored at log time; if log has no url_hash it is treated current). Otherwise `hook.execute(log.request_data, log.trigger, idempotency_key: log.idempotency_key)` — **sync, not forced** (disabled hooks => error `Hook disabled`), the **stored `request_data`** is resent (which has emails already replaced by `[REDACTED]`, see 2.5; and the original `Idempotency-Key` header value is reused). Success => 201 `{"response_status": <integer http code>}` even if receiver returned 500 (service returns success whenever a response arrived). Connection error => 422 with exception text.

### 1.9 System hooks API (`lib/api/system_hooks.rb`)
- `GET /hooks` (paginated `SystemHook.all`), `GET /hooks/:hook_id`, `POST /hooks`, `PUT /hooks/:hook_id`, `DELETE /hooks/:hook_id` (204), `PUT/DELETE /hooks/:hook_id/url_variables/:key`, `.../custom_headers/:key`, and `POST /hooks/:hook_id` (test).
- Params (`:24-39`): `name, description, token, push_events, tag_push_events, merge_requests_events, repository_update_events, enable_ssl_verification, push_events_branch_filter, branch_filter_strategy, url_variables[], custom_headers[]`, plus `url`. Notably NO `note_events`/`emoji_events`/`issues_events` etc.
- Test `POST /hooks/:hook_id` (`lib/api/hooks/test.rb:184-189`): ignores event kind; sends fixed data `{"event_name":"project_create","name":"Ruby","path":"ruby","project_id":1,"owner_name":"Someone","owner_email":"example@gitlabhq.com"}` via `hook.execute(data, 'system_hooks')` (sync, NOT forced; result ignored) and **returns that data hash as the response body, HTTP 201**, regardless of the receiver's outcome. Delivery logged via the async LogExecutionWorker (non-forced path).
- Create uses `Current.organization`; response includes `organization_id`.

---------------------------------------------------------------------------------------------------

## 2. Delivery (`app/services/web_hook_service.rb`)

### 2.1 Which hooks fire, and how
- Project events: `Project#execute_hooks(data, scope)` (`app/models/project.rb:2295-2300`) inside `run_after_commit_or_now`: `triggered_hooks(scope,data).execute` then `SystemHooksService.new.execute_hooks(data, scope)`.
- `Projects::TriggeredHooks#execute` (`app/models/projects/triggered_hooks.rb:16-26`): for each relation: `hooks.hooks_for(scope)` (= `executable` + `where(<event>_events: true)`, `triggerable_hooks.rb:12-17`) `.select_active(scope, data)` (branch filter, only for `:push_hooks`), then per-hook `filter` jsonb check (`hook.filter[scope.to_s]` via `Gitlab::FilterEvaluator`, not API-settable in 18.11), then `hook.async_execute(data, scope.to_s)`.
- **Only project hooks + system hooks** fire (`Project#triggered_hooks`, `project.rb:2303`). Hooks of parent groups do not exist/fire.
- `Project#has_active_hooks?(scope)` gates payload building: true if project hooks, OR system hooks (`SystemHook.hooks_for(scope)`), OR file hooks exist (`project.rb:2334-2342`). Note `SystemHook.hooks_for(:note_hooks)` is `none` (note/emoji not in `SystemHook::AVAILABLE_HOOKS`), so system hooks never receive note or emoji events.
- System hooks (`app/services/system_hooks_service.rb:12-18`): `SystemHook.executable.hooks_for(scope).find_each { hook.async_execute(data, 'system_hooks') }`. System hook scopes: `repository_update_hooks, push_hooks, tag_push_hooks, merge_request_hooks` (`system_hook.rb:68-73`). So system hooks receive push / tag_push / merge_request project events with `push_events` / `tag_push_events` / `merge_requests_events` = true, **same payload as project hooks** but `X-Gitlab-Event: System Hook`. **Branch filter (`push_events_branch_filter`) is NOT applied to system hooks** (they bypass `select_active`). They also get non-project events (`user_create`, `project_create`, ...) with scope `:all` — out of scope here.
- `WebHooks::Hook#async_execute` (`hook.rb:108-110`): `WebHookService.new(...).async_execute if executable?`.

### 2.2 Async execution (`web_hook_service.rb:125-138`, `app/workers/web_hook_worker.rb`)
`async_execute` (inside hook's application context): drops silently (logs to AuthLogger) when silent mode is on, when rate-limited (`Gitlab::WebHooks::RateLimiter`, plan limit `web_hook_calls` default 0 = unlimited; system hooks excluded, `rate_limiter.rb:147,180`), or when recursion is blocked (2.4). Otherwise `WebHookWorker.perform_async(hook.id, data.deep_stringify_keys, hook_name, {"recursion_detection_request_uuid", "idempotency_key"})`. Sidekiq `retry: 4` but `WebHookService#execute` rescues HTTP errors and returns an error response instead of raising, so **non-2xx or transport failures are NOT retried**; the Sidekiq retry only matters for unrelated exceptions (e.g. DB). Delivery order is not guaranteed (one Sidekiq job per hook per event; hooks iterated in DB order; project hooks before system hooks).
The worker re-reads the hook (`WebHook.find_by_id`; skipped if deleted), `Gitlab::WebHooks.prepare_data` (wiki only), restores the recursion UUID, and calls `WebHookService.new(hook, data, hook_name, jid, idempotency_key:).execute`.

### 2.3 Request (`web_hook_service.rb:154-162,219-241`)
- Method: **POST** via `Gitlab::HTTP.post`. URL = `hook.interpolated_url` (url with `{var}` substituted; on interpolation failure falls back to raw `hook.url`, `:142-148`). If URL has userinfo (`https://user:pass@host`), it is stripped from the URL and sent as HTTP Basic auth (CGI-unescaped user/password) (`:164-171`).
- Body: compact JSON of `request_payload` = `data`, or the rendered custom template (2.7). Size limit 25 MB (`REQUEST_BODY_SIZE_LIMIT`); exceeding => `Gitlab::Json::LimitedEncoder::LimitExceeded` handled as transport error.
- `verify: hook.enable_ssl_verification`; `timeout: Gitlab.config.gitlab.webhook_timeout` (default **10** s, `config/initializers/1_settings.rb:231`); `allow_local_requests: hook.allow_local_requests?` (setting web_hooks_and_services default false / system hooks default true); response read limited by `max_http_response_size_limit` MB (default 100).
- Redirects: response `code` 3xx is treated as category `:ok` (`:211-217`), suggesting redirects are not followed; actual follow behaviour lives in gitlab-http **[inferred: not followed]**.
- Headers. Final hash = `custom_headers.merge(built headers)` (built headers win on name clash). Built headers in this order (`:219-233`):
  1. `Content-Type: application/json`
  2. `User-Agent: GitLab/18.11.14` (`"GitLab/#{Gitlab::VERSION}"`, VERSION file content)
  3. `Idempotency-Key: <uuid v4>` (generated once per event delivery; preserved by resend; the same value is used across the Sidekiq job)
  4. `X-Gitlab-Event: <event>` (`Gitlab::WebHooks::GITLAB_EVENT_HEADER`)
  5. `X-Gitlab-Webhook-UUID: <new uuid v4 per HTTP request>`
  6. `X-Gitlab-Instance: <Gitlab.config.gitlab.base_url>` (e.g. `https://gitlab.example.com`, no trailing slash)
  7. `X-Gitlab-Token: <token with line breaks removed>` only if token present
  8. `X-Gitlab-Event-UUID: <uuid>` (recursion detection: `"#{GITLAB_EVENT_HEADER}-UUID"`, `recursion_detection/uuid.rb:5`): the request-scoped UUID if this delivery happens inside a request that itself came from a webhook (the header was received), else a per-hook random UUID created within the process.
- `X-Gitlab-Event` values: `hook_name.singularize.titleize` (`:45-49`): `push_hooks`=>**`Push Hook`**, `tag_push_hooks`=>**`Tag Push Hook`**, `merge_request_hooks`=>**`Merge Request Hook`**, `note_hooks`=>**`Note Hook`**, `confidential_note_hooks`=>**`Confidential Note Hook`**, `emoji_hooks`=>**`Emoji Hook`**, `issue_hooks`=>`Issue Hook`, `confidential_issue_hooks`=>`Confidential Issue Hook`, `job_hooks`=>`Job Hook`, `pipeline_hooks`=>`Pipeline Hook`, `wiki_page_hooks`=>`Wiki Page Hook`, `release_hooks`=>`Release Hook`, `deployment_hooks`=>`Deployment Hook`, `feature_flag_hooks`=>`Feature Flag Hook`, `milestone_hooks`=>`Milestone Hook`, `resource_access_token_hooks`=>`Resource Access Token Hook`, `resource_deploy_token_hooks`=>`Resource Deploy Token Hook`. For `SystemHook` instances: `SystemHook`.titleize => **`System Hook`** for every event (`:46`).
  (`singularize` of `push_hooks` -> `push_hook`; note `hook_name` for test deliveries is the trigger key e.g. `merge_request_hooks`, so test deliveries carry the same event header.)
- There is no HMAC signature header; `X-Gitlab-Token` is the shared secret.

### 2.4 Recursion protection (`lib/gitlab/web_hooks/recursion_detection.rb`)
Redis set per UUID: `webhook_recursion_detection:<uuid>`, members = hook ids, TTL 30 min (`TOUCH_CACHE_TTL`), `COUNT_LIMIT = 100`. `block?(hook)` is true only if a request UUID is known (the incoming API request had `X-Gitlab-Event-UUID`, i.e. a receiver calls GitLab back in response to a hook) AND (hook id already in set OR set size >= 100). `execute` calls `register!(hook)` before sending. On block: `ServiceResponse.error(message: 'Recursive webhook blocked')` (sync) or silently dropped in `async_execute` (AuthLogger `Recursive webhook blocked from executing`). labrat implementation: read `X-Gitlab-Event-UUID` from incoming API requests, carry it into hooks fired by that request.

### 2.5 Logging (`web_hook_service.rb:173-209`, `app/services/web_hooks/log_execution_service.rb`, `app/models/hooks/web_hook_log.rb`)
Every executed request (success or failure) produces a `WebHookLog` row (table `web_hook_logs_daily`, daily partitions, retained 14 days; API shows 7 days). Fields:
- `trigger` = hook_name string; `url` = `hook.url` (RAW, with `{vars}` placeholders; userinfo masked by `Gitlab::UrlSanitizer.sanitize_masked_url` on save, `web_hook_log.rb:90-92`); `interpolated_url` only to compute `url_hash` (sha256 of interpolated url).
- `execution_duration` float seconds (monotonic); 0 for template errors.
- `request_headers` = `custom_headers` with every value replaced by `[REDACTED]` merged with the built headers; then `X-Gitlab-Token` set to `[REDACTED]` when hook has a token (`log_execution_service.rb:29`).
- `request_data` = payload (post-template); emails redacted: any leaf string that is entirely an email (`URI::MailTo::EMAIL_REGEXP`) becomes `[REDACTED]` (`web_hook_log.rb:94-98`). If the Sidekiq arg size limit is hit the data becomes `{"oversize": true}` (`:195-209`).
- `response_headers`: capitalized names (`Content-Type`), first 50 headers, each value truncated to 1 KB (bytes), UTF-8 enforced; values equal to a url-variable value are replaced by `{name}` (`log_execution_service.rb:34-44`).
- `response_body`: first 8 KB (`truncate_bytes`), UTF-8 enforced, `''` if none.
- `response_status`: `response.code` — a string like `"200"`; for transport/internal failures the literal **`internal error`** (`InternalErrorResponse::ERROR_MESSAGE`) with `response_headers {}` and `response_body ''`; `internal_error_message` = exception text.
- Sync (forced/test) logs are written inline; async logs go through `WebHooks::LogExecutionWorker` (retry 3, idempotent) which also updates failure state. Template errors log `request_data: {}`.
- Errors that become "internal error" logs: `Gitlab::HTTP::HTTP_ERRORS` (timeouts, connection refused/reset, SSL errors, `BlockedUrlError` e.g. local URL while not allowed, redirect/size errors), `Zlib::DataError`, `LimitedEncoder::LimitExceeded`, `URI::InvalidURIError` (`:107-123`). Also `AppLogger.error("WebHook Error after Ns => msg")`.

### 2.6 Failure state / auto-disable (`app/models/concerns/web_hooks/auto_disabling.rb`, only when FF `auto_disabling_web_hooks` on AND hook is ProjectHook)
Category: `:ok` if 2xx or 3xx else `:error` (4xx, 5xx, internal error). After each log (under an exclusive lease, `log_execution_service.rb:48-74`):
- `:ok` => `enable!`: if `recent_failures != 0 || disabled_until` set both back to `0`/nil.
- `:error` => `backoff!` (only if currently `executable?`): `recent_failures = (recent_failures+1).clamp(1, 40)`; if new count > 3: `disabled_until = now + clamp(1min * 2.0**(recent_failures-3), 1min, 1day)` (computed using the already-incremented count).
- Thresholds: `TEMPORARILY_DISABLED_FAILURE_THRESHOLD = 3`, `PERMANENTLY_DISABLED_FAILURE_THRESHOLD = 39`. `temporarily_disabled?` = (`disabled_until` nil or >= now) and 4 <= failures <= 39; `permanently_disabled?` = failures > 39 (i.e. 40). `executable?` = neither. `alert_status`: `temporarily_disabled` / `disabled` / `executable`. Forced (test) executions bypass the executable check but still update state.
- Hooks not executable are skipped at enqueue (`executable` scope in `hooks_for`, plus `executable?` re-check in the service; sync non-forced returns error `Hook disabled`). With the FF off (stock default) none of this applies. Silent mode => hooks not executable (`Silent mode enabled`).

### 2.7 Custom template (`web_hook_service.rb:301-331`)
If `custom_webhook_template` present, body = template with each `{{a.b.c}}` replaced by `data.deep_stringify_keys.dig('a','b','c')` serialized as JSON (strings inserted WITHOUT surrounding quotes but JSON-escaped; non-strings as raw JSON; missing path => `null`), then parsed with `Gitlab::Json.safe_parse` (limits depth 32, arrays/hashes 100, 500 elements, 4096 bytes). Invalid result => `CustomWebHookTemplateError` "Error while parsing rendered custom webhook template: <msg>" (`Invalid JSON`, parser message, or `You may be trying to access an array value, which is not supported.`) => response/log `response_status: internal error`; `request_data {}`.

---------------------------------------------------------------------------------------------------

## 3. Payloads

### 3.0 Timestamp formats (read carefully)
Three formats occur:
1. **Raw ActiveRecord timestamps** (`created_at`, `updated_at`, `last_edited_at`, `resolved_at`, `prepared_at`, label timestamps, `actioned_at`, and values in `changes`) are `TimeWithZone` objects inside the payload hash. Async delivery serializes the hash through Sidekiq (`JSON.generate`), which renders them via `to_s`: **`2015-05-17 18:21:36 UTC`** (`YYYY-MM-DD HH:MM:SS UTC`, no fractional secs). This is the documented real-world format **[inferred from Sidekiq/ActiveSupport JSON semantics + GitLab docs]**.
2. **Explicit `.xmlschema`**: commit `timestamp` (`app/models/commit.rb:274-276`, `committed_date.xmlschema`, committer's own offset e.g. `2013-05-13T18:18:08+00:00`, `2012-07-12T05:26:17-04:00`); `HookData::BaseBuilder#timestamps_data` (group/project/user builders, not used by our events).
3. **Test deliveries (sync, forced)** are not round-tripped through Sidekiq; the data is encoded directly by `Gitlab::Json::LimitedEncoder` so raw `TimeWithZone` fields come out as ISO8601 with milliseconds **`2015-05-17T18:21:36.000Z`** **[inferred]**. Everything else is identical. Fixed `SAMPLE_DATA` push payloads contain literal strings.
Per payload below, "AR time" means format 1/3 depending on path.

Key order is the Ruby insertion order and is preserved in JSON. All keys below are lowercase snake_case. `nil` => JSON `null`.

### 3.1 Common sub-objects
- `user` = `User#hook_attrs` (`app/models/user.rb:2086-2094`): `{id, name, username, avatar_url (absolute URL), email}`; `email` = `public_email` if present else the literal string **`[REDACTED]`** (`webhook_email`, `:2870-2872`).
- `project` = `Project#hook_attrs` (`app/models/project.rb:2452-2479`), key order: `id, name, description, web_url, avatar_url (absolute, null if none), git_ssh_url, git_http_url, namespace (namespace NAME, not path), visibility_level (0 private,10 internal,20 public), path_with_namespace, default_branch (null when empty repo/no default), ci_config_path, homepage (=web_url), url (=url_to_repo, i.e. the SSH url git@host:path.git), ssh_url, http_url`.
- Deprecated `repository`: MR/note use `project.hook_attrs.slice(:name,:url,:description,:homepage)` => `{name, url, description, homepage}`; push uses `slice(:name,:url,:description,:homepage,:git_http_url,:git_ssh_url,:visibility_level)` (slice follows argument order).
- Label (`app/models/label.rb:271-284`): `{id, title, color, project_id, created_at (AR time), updated_at (AR time), template, description, type ("ProjectLabel"|"GroupLabel"), group_id}`.
- Commit (`app/models/commit.rb:278-294`): `{id, message, title, timestamp, url, author:{name,email}}` (+ `added, modified, removed` arrays when `with_changed_files`, from `repo_changes`: deleted->removed(old_path), renamed or new->added(new_path), else modified(new_path)). `url` = `Gitlab::UrlBuilder.build(commit)` i.e. `<base>/<ns>/<proj>/-/commit/<sha>`. `title` = first line, truncated to 81 chars+`...` if full title >= 100 chars. `message` = `safe_message` (full). **Author email is the raw commit author email**, not redacted in the payload (it is redacted only in the stored log).

### 3.2 Push and tag_push (`lib/gitlab/data_builder/push.rb:98-146`)
Built by `Git::BranchHooksService` / `Git::TagHooksService` (`app/services/git/base_hooks_service.rb`), hook_name `:push_hooks` / `:tag_push_hooks`. Key order:
```
object_kind        "push" | "tag_push"   (tag_push iff ref starts with refs/tags/)
event_name         same value
before             oldrev (40 zeros for creation)
after              newrev (40 zeros for deletion)
ref                "refs/heads/x" | "refs/tags/x"
ref_protected      bool (project.protected_for?(ref))
checkout_sha       nil on deletion; for tags: dereferenced commit sha (nil if tag missing); for branches: newrev
message            nil for branches; for tags: tag.message (nil for lightweight tags)
user_id            pusher id
user_name
user_username
user_email         user.public_email  (raw column: null/"" if unset — NOT "[REDACTED]" here)
user_avatar        absolute avatar URL
project_id
project            Project#hook_attrs
commits            array, oldest -> newest, at most 20 (the LAST 20), each commit hook_attrs WITH added/modified/removed
total_commits_count  integer
push_options       hash (e.g. {} or {"ci":{"skip":true}} ; keys symbolized/JSON strings)
repository         {name,url,description,homepage,git_http_url,git_ssh_url,visibility_level}
```
Commit selection (`branch_hooks_service.rb:222-263`): `PROCESS_COMMIT_LIMIT = 100`.
- Creating the default branch (or any branch when project has no default branch, `default_branch?` is true if `project.default_branch` is nil): newest 101 commits reversed (oldest first); `total_commits_count` = number of commits if <=100 else `repository.commit_count_for_ref(ref)`.
- Creating a non-default branch: `commits_between(default_branch, newrev, limit 101)`; total = count or `count_commits_between(default, newrev)`.
- Updating: `commits_between(oldrev, newrev)`; total likewise.
- Removing: commits `[]`, `total_commits_count` 0, `after` zeros, `checkout_sha` null.
- Then `Push.build` keeps `commits.last(20)`. `total_commits_count` is the true count (can be 150 while `commits` has 20). A force-push/no-new-commits update yields `commits: []`, `total_commits_count: 0`, but the hook still fires.
- Tag: `commits` = `[tag_commit]` (the dereferenced commit) or `[]`, `total_commits_count` = commits.count (0 or 1); `message` = annotated tag message; tag `removing` => `[]`, 0. For a tag whose target != newrev (`tag.target == newrev` check) commits empty.
- Commit `timestamp`: xmlschema in the committer's timezone (`+00:00` style; if Ruby treats an offset-0 time as UTC it could print `Z` [inferred]; sample data uses `+00:00`).
Sample payloads for empty repos (`SAMPLE_DATA`, `push.rb:8-51`): `object_kind "push"`, `event_name "push"`, before `95790bf891e76fee5e1747ab589903a6a1f80f22`, after/checkout_sha `da1560886d4f094c3e6c9ef40349f7d38b5d27d7`, `ref "refs/heads/master"`, `ref_protected true`, `message "Hello World"`, `user_id 4, user_name "John Smith", user_email "john@example.com"` (NO `user_username`), `user_avatar` the odd gravatar URL (`...?s=8://s.gravatar.com/avatar/d4c74594d841139328695756648b6bd6?s=80`), `project_id 15`, project `{id 15,name "gitlab",description "",web_url "http://test.example.com/gitlab/gitlab",avatar_url (same gravatar),git_ssh_url "git@test.example.com:gitlab/gitlab.git",git_http_url "http://test.example.com/gitlab/gitlab.git",namespace "gitlab",visibility_level 0,path_with_namespace "gitlab/gitlab",default_branch "master"}`, one commit `{id c5feabde2d8cd023215af4d2ceeb7a64839fc428, message "Add simple search to projects in public area\n\ncommit message body", title "Add simple search to projects in public area", timestamp "2013-05-13T18:18:08+00:00", url "https://test.example.com/gitlab/gitlab/-/commit/c5feabde…", author{name "Test User",email "test@example.com"}}`, `total_commits_count 1`, `push_options {"ci":{"skip":true}}` (no `repository` key). Tag sample = same merged with `object_kind/event_name "tag_push"`, `ref "refs/tags/v1.0.0"`.

### 3.3 merge_request (`Gitlab::DataBuilder::Issuable#build`, `lib/gitlab/data_builder/issuable.rb:173-197`; `MergeRequest#to_hook_data`, `app/models/concerns/issuable.rb:~590`; `Gitlab::HookData::MergeRequestBuilder`)
Top-level key order:
```
object_kind        "merge_request"
event_type         "merge_request"
user               actor (hook_attrs)   [the user performing the action; current_user of the service]
project            target project hook_attrs
object_attributes  (below)
labels             array of label hook_attrs (current labels)
changes            {attr: {"previous":..., "current":...}}   ({} if none)
repository         {name,url,description,homepage}
assignees          array of user hook_attrs     — key ONLY present if the MR has assignees
reviewers          array of {user hook_attrs..., state, re_requested} — key ONLY present if reviewers exist
```
`object_attributes` key order = columns present in `safe_hook_attributes` order (merge_request_builder.rb:72-105, `attributes.slice(*list)` keeps list order), then computed keys appended in `additional_attributes` order (existing keys keep their original position):
```
assignee_id            (first assignee id or null — OVERWRITTEN value, deprecated)
author_id
created_at             AR time
description            (markdown with relative /uploads image links rewritten to absolute URL by absolute_image_urls)
draft                  bool
head_pipeline_id
id
iid
last_edited_at         AR time|null
last_edited_by_id
merge_commit_sha
merge_error
merge_params           object (e.g. {"force_remove_source_branch":"1"}) or {}
merge_status           "unchecked"|"checking"|"can_be_merged"|"cannot_be_merged"|"cannot_be_merged_recheck"|"cannot_be_merged_rechecking"
merge_user_id
merge_when_pipeline_succeeds  bool
milestone_id
source_branch
source_project_id
state_id               1 opened, 2 closed, 3 merged, 4 locked
target_branch
target_project_id
time_estimate          int seconds
title
updated_at             AR time
updated_by_id
prepared_at            AR time|null
assignee_ids           array
blocking_discussions_resolved  bool (true unless project requires resolved discussions and some unresolved)
detailed_merge_status  string, e.g. "mergeable","checking","unchecked","preparing","ci_must_pass","ci_still_running","not_open","draft_status","discussions_not_resolved","conflict","need_rebase",... (DetailedMergeStatusService)
first_contribution     bool (no merged MR by this author in project; for merged MRs uses metrics)
human_time_change      string|null (TimeTrackingFormatter, e.g. "1h")
human_time_estimate    string|null
human_total_time_spent string|null
labels                 array of label hook_attrs (same as top-level labels)
last_commit            diff_head_commit.hook_attrs (WITHOUT file arrays) | null
reviewer_ids           array
source                 source project hook_attrs (null if fork deleted)
state                  "opened"|"closed"|"merged"|"locked"
system                 bool  (false by default; see below)
target                 target project hook_attrs
time_change            int
total_time_spent       int
url                    MR web URL  <base>/<ns>/<proj>/-/merge_requests/<iid>
work_in_progress       bool (= draft)
-- then, added by service hook_data (app/services/merge_requests/base_service.rb:20-36) / builder:
action                 see section 4
actioned_at            AR time ( Time.current at fire time)  [Issuable#build:186-187]
oldrev                 only when old_rev given and not blank (push-triggered update)
system                 (already present: replaced by true when system-initiated; position unchanged)
```
(`merge_params`/`merge_error`/others are raw column values; columns not in the list — `squash`, `lock_version`, `squash_commit_sha`, `merge_ref_sha`, `rebase_*`, `discussion_locked`, `title_html`, ... — are NOT included.) The `assignee_id` slot exists because column `assignee_id` still exists in `merge_requests`.
Order note: `blocking_discussions_resolved`, `reviewer_ids`, `system` are not columns, so they are appended in `additional_attributes` order (as in the list above); keys that are both a column and in `additional_attributes` (`assignee_id`, `description`, `draft`, `target_branch`) keep their column-slice position.

`changes` (`issuable.rb` `to_hook_data`, `issuable.rb:~520-585`, `Issuable#final_changes`): union of AR `reportable_changes` (saved attribute changes accumulated on the object during this request: `[previous, current]` pairs, including `updated_at`, `updated_by_id`, `title`, `description`, `draft`, `state_id`, `target_branch`, `milestone_id`, `last_edited_at`, `last_edited_by_id`, `merge_status`, `merge_params`...) and association diffs (only when `old_associations` supplied, i.e. update-service flows): `assignees` (arrays of user hook_attrs), `labels` (arrays of label hook_attrs), `target_branch`, `total_time_spent` + `time_change`, and `reviewers` (arrays of `{id,name,username,avatar_url,email,state,re_requested}` — only if reviewers differ incl. state/re_requested). Then filtered to `safe_hook_attributes + [assignees, labels, reviewers, total_time_spent, time_change]` and **emitted in that safe-list order** (not change order), each as `{"previous": old, "current": new}`. Timestamps inside use AR time format. Close/merge/reopen hooks carry whatever changes the in-memory object has recorded in that request (e.g. `state_id`: `{"previous":1,"current":2}`, `updated_at`, `updated_by_id`); `merge` hooks raised from `PostMergeService` commonly include `state_id`, `merge_commit_sha`... only if the same object instance recorded them. For `open` (created from AfterCreateService, a fresh object) `changes` is `{}`.
`reviewers` entries (`issuable.rb:570-578`): `user.hook_attrs` + `state` (`unreviewed|reviewed|requested_changes|approved|unapproved|review_started`) + `re_requested` (true only for the reviewer whose review was just re-requested via `RequestReviewService`).
`assignees` key position: after `repository` (assignees first, then reviewers).

Test payload (`merge_requests_events_data`): latest MR by created_desc via `MergeRequestsFinder`, `to_hook_data(current_user, action: 'open')` => no `oldrev`; `actioned_at` is only added when passed, so it is **absent in test payloads**; `changes {}`; `system` stays `false`.

### 3.4 note on merge request (`lib/gitlab/data_builder/note.rb:41-76`, `lib/gitlab/hook_data/note_builder.rb`)
Key order:
```
object_kind    "note"
event_type     "note" | "confidential_note"   (confidential if note.confidential?(include_noteable: true) i.e. internal note or confidential noteable)
user           note.author.hook_attrs  (create) ; note.author also on update (NOT the editor)
project_id
project        Project#hook_attrs
object_attributes   (below)
repository     {name,url,description,homepage}
merge_request  MergeRequestBuilder#build (the object_attributes block of 3.3 without action/actioned_at/oldrev/changes, `system` false)
```
(`commit`/`issue`/`snippet` replace `merge_request` for other noteables.) Note `object_attributes` order: slice list order (columns present): `author_id, change_position, commit_id, created_at, discussion_id, id, internal, line_code, note, noteable_id, noteable_type, original_position, position, project_id, resolved_at, resolved_by_id, resolved_by_push, st_diff, system, type, updated_at, updated_by_id` — (`attachment` is in the slice list but no such column exists so it is absent) — then `description` (note text with absolute image URLs), `url` (`<mr url>#note_<id>`), `action` ("create" | "update"). Notes:
- `note` (raw body), `type` null for plain notes, `"DiffNote"` or `"DiscussionNote"`. `noteable_type "MergeRequest"`. `system` false (system notes never reach here). `internal` bool. `st_diff` null. `commit_id` null for MR notes. `line_code` null unless diff note.
- `position`, `original_position`, `change_position`: `null` for non-diff notes; for DiffNote they are `Gitlab::Diff::Position#as_json` = `{base_sha, start_sha, head_sha, old_path, new_path, position_type ("text"|"image"|"file"), old_line, new_line, line_range}` (`ignore_whitespace_change` excluded; `lib/gitlab/diff/position.rb:76-78`, formatters `base_formatter.rb:34-43`, `text_formatter.rb:25-28`). `line_range` null or `{"start":{"line_code","type","old_line","new_line"},"end":{...}}`. Image positions add `width,height,x,y` (replacing line fields); file positions have just the base set + `position_type "file"`.
- Timestamps `created_at/updated_at/resolved_at`: AR time.
- Events: create => NewNoteWorker (async, after commit) -> `Notes::PostProcessService` (`execute_note_hooks`): scope `:confidential_note_hooks` if confidential else `:note_hooks` (hook's `note_events` vs `confidential_note_events` flag — **confidential notes do NOT go to hooks that only have `note_events`**); update => `Notes::UpdateService#execute_note_webhook` only if the `note` text changed, action `update`, sync build, no integrations.

### 3.5 emoji (`lib/gitlab/data_builder/emoji.rb`, `lib/gitlab/hook_data/emoji_builder.rb`)
Fired by `AwardEmojis::AddService` (`'award'`) and `DestroyService` (`'revoke'`) through `BaseService#execute_hooks` (`app/services/award_emojis/base_service.rb:14-19`): only if `project.has_active_hooks?(:emoji_hooks)`; scope `:emoji_hooks` (hook flag `emoji_events`, default false). Payload built synchronously in the request, then delivered async. Key order:
```
object_kind   "emoji"
event_type    "award" | "revoke"
user          current_user hook_attrs (the user awarding/revoking)
project_id
project       Project#hook_attrs
object_attributes  {user_id, created_at, id, name, awardable_type, awardable_id, updated_at, awarded_on_url}
note          only when awardable is a Note: Note hook_attrs (same shape/order as 3.4 object_attributes, with description+url but NO action)
merge_request MergeRequestBuilder#build  (key named after noteable class underscore: merge_request | issue | commit | snippet ...)
```
`awardable_type` "MergeRequest" or "Note"; `awarded_on_url` = URL of awardable (MR URL, or note URL `...#note_<id>`). `name` e.g. `thumbsup` (alpha codes normalized via `TanukiEmoji`). For awards on a note, `merge_request` is the note's noteable. For a revoke the destroyed record's data (original id/created_at) is used. Timestamps AR time. Test payload (`emoji_events_data`): `AwardEmoji.new(id: 1, name: 'thumbsup', user: current_user, awardable: project.notes.last, created_at/updated_at: now)` with action `award` — so uses the project's LAST NOTE (possibly a system note); `Ensure the project has notes.` if none.

---------------------------------------------------------------------------------------------------

## 4. MR `action` values and when they fire
All via `MergeRequests::BaseService#execute_hooks(mr, action, old_rev:, old_associations:, system:, system_action:)` (`base_service.rb:38-51`): skipped entirely if `merge_request.skip_ensure_merge_request_diff`; payload built lazily (`Gitlab::Lazy`), so no build cost when no hook matches; delivered via `project.execute_hooks(.., :merge_request_hooks)` (project hooks with `merge_requests_events` + system hooks with `merge_requests_events`), then integrations, then group-mention hooks on `open`. Hooks run on the **target project** (`merge_request.project`).
| `action` | Fired from | When |
|---|---|---|
| `open` | `MergeRequests::AfterCreateService#execute` (`after_create_service.rb:13`) | after MR creation completes (async job after diff generation, NOT inside the create API response); default action arg. Test payloads also use `open`. |
| `close` | `CloseService` (`close_service.rb:22`) | MR closed successfully (API `state_event=close`, UI, or auto-close when source branch deleted by push: `RefreshService#close_upon_missing_source_branch_ref`). Only if user authorized and `merge_request.close` succeeded. |
| `reopen` | `ReopenService` (`reopen_service.rb:13`) | `state_event=reopen`. |
| `merge` | `PostMergeService#execute` (`post_merge_service.rb:49`) | after MR marked merged (via merge API/auto-merge); also when a push to target manually merges the MR (`RefreshService#post_merge_manually_merged`). |
| `update` | `IssuableBaseService#update` (`issuable_base_service.rb:362-366`) via `UpdateService` | after ANY successful save in an update call where `issuable.changed? || params.present? || callbacks` (`:325`) — i.e. **a PUT with identical values still fires `update`**; carries `changes`. A pure `state_event` change (params become empty after the close/reopen service ran) does not additionally fire `update`; if other params accompany it, both `close`/`reopen` and `update` fire. |
| `update` | `Refresh::WebHooksService` (`app/services/merge_requests/refresh/web_hooks_service.rb`), via `MergeRequests::Refresh::WebHooksWorker` from `RefreshService#execute_async_workers` (`refresh_service.rb:418-427`) | every push to a branch (create/update, any branch push incl. force-push) for each OPEN MR whose source branch is the pushed branch and source project is the pushed project; passes `old_rev` => `object_attributes.oldrev` = oldrev unless blank (new-branch push => no `oldrev` key). `changes` is `{}`. NOT fired for tag pushes, branch deletion (MR closed first => `close`), or pushes to target branch. Fork MRs: MRs whose source is the fork project get it when the fork is pushed. |
| `update` | `UpdateReviewersService` (:36), `RequestReviewService` (:28; sets `re_requested`), `HandleAssigneesChangeService` (:37, when `options['execute_hooks']`), `AddSpentTimeService` (:20, `changes.total_time_spent/time_change`), `ResolvedDiscussionNotificationService` (:9; all discussions resolved) | respective operations; reviewers/assignees changes appear in `changes.reviewers` / `changes.assignees` (+ `reviewer_ids` in object_attributes). Labels via UpdateService => `changes.labels`. Title `Draft:` toggles => `changes.title` + `changes.draft`. |
| `approved` | `ExecuteApprovalHooksService#execute` (`execute_approval_hooks_service.rb:8`) | MR approved. Triggered from `MergeRequests::ApprovedEvent` after `ApprovalService`. |
| `unapproved` | `RemoveApprovalService#trigger_approval_hooks` (`remove_approval_service.rb:46-52`) | approval removed. Not sent when `skip_notification`. |
No hook for: draft/ready alone (it is an `update`), discussion lock without UpdateService, rebase, reviewer state changes made by submitting a review (no `execute_hooks`), pipeline status changes, MR note creation (that's a Note Hook). Merge-status/`detailed_merge_status` changes do not fire hooks.
`object_attributes.system` is `false` (NOT the note `system`).

### System notes and note hooks
`Notes::PostProcessService#execute` only runs hooks `unless note.system?` (`post_process_service.rb:11-22`): **system notes (status changes, label changes, assignee notes, "approved this merge request", etc.) never fire Note Hooks**. Update hooks only for edited text. Confidential: `confidential_note_hooks` for internal notes or notes on confidential issues; MR notes can be internal (`internal: true`) => `Confidential Note Hook`, `event_type "confidential_note"`. Notes on commits, issues, snippets use the same event; hooks fire for all noteable types (the MR sub-object only when noteable is an MR). Notes without a project (personal snippets) are skipped.

---------------------------------------------------------------------------------------------------

## 5. Test hook payloads (`TestHooks::ProjectService`, `Integrations::ProjectTestData`)
`hook.execute(data, trigger_key, force: true)`; `trigger_key` is the scope name (`push_hooks` etc.) so `X-Gitlab-Event` and log `trigger` match real events.
| trigger | Data | Empty case |
|---|---|---|
| `push_events` | `Push.build_sample(project, user)` (`push.rb:158-177`): if `project.empty_repo?` => `SAMPLE_DATA` (3.2); else real build: `ref refs/heads/<default_branch>`, `commits = repository.commits(default_branch, limit: 3)`, `oldrev` = oldest of the 3 (`commits.last`), `newrev` = newest, `commits` passed reversed (oldest first) and include `added/modified/removed` (`with_changed_files` defaults true), `message` nil, `total_commits_count` = number passed (<=3), `user = current_user` | none |
| `tag_push_events` | same with `is_tag` truthy: `ref refs/tags/<name-desc-latest tag or v1.0.0>`; for empty repos `SAMPLE_DATA.merge(SAMPLE_TAG_DATA)`. NOTE: the service passes `is_tag: true` as a keyword to a positional param (hash is truthy), `object_kind` becomes `tag_push` from the ref. `oldrev/newrev` are still the default-branch commits. | none |
| `merge_requests_events` | newest MR (`created_desc`, visible to current_user) `.to_hook_data(current_user, action: 'open')` | `Ensure the project has merge requests.` |
| `note_events` | newest note of project (`NotesFinder` `id_desc`, target=project) built with `Note.build(note, current_user, :create)` (user = current user, may be a system note, any noteable) | `Ensure the project has notes.` |
| `emoji_events` | 3.5 test construction (action `award`) | `Ensure the project has notes.` |
| `issues_events`/`confidential_issues_events` | newest issue `.to_hook_data(user, action: 'open')` | `Ensure the project has issues.` |
| `job_events`, `pipeline_events`, `wiki_page_events`, `releases_events`, `milestone_events` (`Milestone.build_sample`), `resource_access_token_events`, `resource_deploy_token_events` | see `project_test_data.rb` | messages in 1.6 |
| `confidential_note_events`, `deployment_events`, `feature_flag_events` | no data | `Testing not available for this hook` |
System hooks have a separate fixed test (1.9). `TestHooks::SystemService` (`system_service.rb`) exists but is not reachable via the System hooks API (for push/tag: `Push.sample_data`; MR: `current_user.projects` last MR, `Ensure one of your projects has merge requests.`).

---------------------------------------------------------------------------------------------------

## 6. Edge cases and gotchas
1. **Hooks fire regardless of the actor's state**: there is no blocked-user / archived-project / project-feature check in `Project#execute_hooks` or `WebHookService` (a blocked user cannot push or comment in the first place; `user` in note payload is the note author). `ProjectHook` only needs `executable` (auto-disabling) and the event flag.
2. **Push hooks and branch filters**: `ActiveHookFilter` applies the filter ONLY to `:push_hooks` (`app/models/hooks/active_hook_filter.rb:9-12`); `tag_push_hooks` always ignore `push_events_branch_filter` and `branch_filter_strategy`. Blank filter => match all (any strategy). `wildcard`: exact string equality or `*` => `.*?` anchored regex (`RefMatcher`, `ref_matcher.rb`) on the branch name (`Gitlab::Git.branch_name(data[:ref])` strips `refs/heads/`); `regex`: RE2 unanchored `===` match (invalid regex => false); `all_branches`: true. Branch name is computed even for deletions (ref still `refs/heads/x`).
3. **System hooks skip branch filters and per-hook `filter`** and do not receive note/emoji events; they get `X-Gitlab-Event: System Hook` for push and MR as well.
4. **Many refs in one push**: `Git::ProcessRefChangesService` groups changes by (ref_type, action created/removed/pushed) and runs project hooks for that group only if `group.size <= push_event_hooks_limit` (app setting, default **3**) (`process_ref_changes_service.rb:38-40`); a bulk push of more than 3 new branches (or >3 tag creations, etc.) fires **no** push/tag_push webhooks at all for that group (events instead collapse into a bulk activity event if > `push_event_activities_limit`, default 3). Each ref in an allowed group fires its own webhook. Pipelines are limited separately (`git_push_pipeline_limit`).
5. **Large pushes**: `commits` capped at the newest 20, ordered oldest->newest; `total_commits_count` true count (uses git rev-list counting beyond 100). Webhook for 0-commit updates still sent. Branch creation from an existing commit with no new commits (branch pointing at default-branch commit) => `commits: []`, `total_commits_count: 0`, `before` zeros.
6. **No default branch** (`project.default_branch` nil, empty repo): first pushed branch is treated as default (`default_branch?` true) => last 100/101 commits of the branch; `project.default_branch` in payload is null for the very first event [value read at build time; the default branch may already be set by then]. Sample/test for an empty repo uses `SAMPLE_DATA` regardless.
7. **Deleted ref**: `after` = 40 zeros, `checkout_sha` null, `commits` [], `total_commits_count` 0; `ref_protected` computed by name even though ref is gone. Branch/tag creation: `before` = 40 zeros. (SHA256 repos use 64 zeros: `Gitlab::Git.blank_ref?` accepts both.)
8. **Token reset on URL change** (1.4) and url_variables/custom_headers reset when the interpolated URL changes — a PUT that changes `url` alone silently drops the token unless `token` is re-sent in the same request.
9. **URL variables never leave the server**: API shows only keys; delivery log `url` keeps `{var}` placeholders; response headers containing a variable's value are rewritten to `{name}` in logs (`log_execution_service.rb:34-44`); custom header values are `[REDACTED]` in logs and API.
10. **Test endpoint 201 body** is literally `{"message":"201 Created"}`; failure body is the RECEIVER's raw body string inside `message`. Test deliveries create log rows synchronously and are not subject to disabled state. Sample timestamps in test payloads use ISO8601 `.000Z` rather than `... UTC` **[inferred]**.
11. **3xx counts as OK** for auto-disable accounting, but the test endpoint requires 2xx (3xx => 422 with receiver body).
12. **Time formats differ within one payload**: `created_at: "2015-05-17 18:21:36 UTC"` (AR) vs `last_commit.timestamp: "2012-07-12T05:26:17-04:00"` (xmlschema). Sidekiq round trip also turns all hash keys into strings and symbols in values (e.g. none present) into strings; `push_options` keys are strings.
13. **Payload built lazily at delivery time for notes** (NewNoteWorker -> PostProcessService) and for MR events (Gitlab::Lazy evaluated on first matching hook, within the request/job that fired the event), so payload reflects state at that moment, e.g. `detailed_merge_status` may be `checking`/`unchecked` right after creation.
14. **`open` hook ordering**: sent from AfterCreateService after pipeline creation attempt and diff preparation, so `head_pipeline_id` and `last_commit` are usually already populated; `merge_status`/`detailed_merge_status` may still be `checking`.
15. **`assignees`/`reviewers` keys absent (not empty arrays)** when none; top-level `labels` always present (may be `[]`); `changes` always present (`{}`).
16. **Idempotency**: `Idempotency-Key` constant across resends of the same log; `X-Gitlab-Webhook-UUID` differs per attempt.
17. **Redaction in stored logs**: any payload leaf that is wholly an email address (commit author email, `user.email`, committer) is stored as `[REDACTED]`; resend therefore delivers redacted emails; `X-Gitlab-Token` stored as `[REDACTED]`.
18. **Hook limit**: 100 project hooks per project (`Maximum number of project hooks (100) exceeded`).
19. **Pagination of hook list**: standard `page`/`per_page` (default 20, max 100) with `X-Total`, `X-Page`... headers; `/events` caps `per_page` at 20 (values outside 1..20 => 400 `{"error":"per_page does not have a valid value"}`).
20. **Recursion**: only when delivering from inside a request that carried `X-Gitlab-Event-UUID`; counted per UUID across hooks (>=100 distinct hooks blocks).
21. **Silent mode** (`Gitlab::SilentMode`): all webhook execution suppressed (async drop; sync error `Silent mode enabled`); `executable` scope returns none.
