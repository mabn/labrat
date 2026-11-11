# MR endpoint parameter audit (GitLab CE 18.11) vs labrat

Method: source reading only (no HTTP). GitLab: `/home/mabn/dev/gitlab-run-ce/gitlab-src` (`lib/api/merge_requests.rb`, `lib/api/helpers/merge_requests_helpers.rb`, `app/finders/merge_requests_finder.rb`, `app/finders/issuable_finder.rb` + `issuable_finder/params.rb`, `issuables/*_filter.rb`, notes/discussions/draft_notes/approvals/award_emoji/time tracking/subscriptions/todos). labrat: `crates/labrat-api/src/routes/merge_requests.rs` (abbrev. `mr.rs`), `routes/notes.rs`, `routes/milestones.rs`, `routes/pipelines.rs`, `params.rs`.
Scenario column: grep of `tests/conformance/tests/scenarios/*.rs` (scenarios pass query params as JSON maps, so the key appears as a quoted string).
Status: IMPL = implemented, PARTIAL = implemented with a listed gap, MISSING = ignored or wrongly accepted.
Endpoint codes: G = `GET /merge_requests`, Gr = `GET /groups/:id/merge_requests`, P = `GET /projects/:id/merge_requests`.

## 0. Grape behaviours that matter for every row

- Typed params (`Integer`, `DateTime`, `Boolean`): an empty string coerces to nil (param ignored, no error). labrat `params.rs` `int`/`time` turn `""` into 400 `x is invalid`, `bool` turns `""` into `false`. Affects `author_id=`, `created_after=`, `merge_user_id=`, `source_project_id=` etc.
- `values:` mismatch gives 400 `{"error":"x does not have a valid value"}` (labrat `one_of`, same text).
- `integer_none_any` (assignee_id, reviewer_id): non-integer other than none/any (any case) gives 400 `x should be an integer, none, or any, however got abc`. labrat says `x is invalid` and only inside the per-MR loop, so an empty list never errors.
- `mutually_exclusive`: 400 `a, b are mutually exclusive`.
- Finder `.presence`: empty `source_branch=`, `target_branch=`, `milestone=`, `author_username=` are ignored by GitLab. labrat compares against the empty string and returns nothing.

## 1. List params (G, Gr, P): `merge_requests_params` = `merge_requests_base_params` + `merge_requests_negatable_params` + `pagination`

| Param (endpoints) | Grape type / values / default | GitLab semantics | labrat status | Scenario |
|---|---|---|---|---|
| `state` (G,Gr,P) | String, `opened closed locked merged all`, default `all` | `by_state`: opened/closed/merged/locked scopes | IMPL `mr.rs:213`, filter `:327`; invalid value 400 | yes: edge_cases (`locked`,`bogus`), random_walk_lists (closed/merged/all) |
| `order_by` (G,Gr,P) | String, `created_at label_priority milestone_due popularity priority title updated_at merged_at`, default `created_at` | `sort = "#{order_by}_#{sort}"` -> `sort_by_attribute`; ties `id desc` | PARTIAL `mr.rs:216,594-650`. Gaps: (a) `merged_at` NULLs are LAST for both asc and desc in GitLab (`order_by_metric_column` `nulls_last`, `merge_request.rb:760`), labrat `Option::cmp` puts NULL first on asc (`:643`); (b) `label_priority`/`priority` ignore the "labels named in `labels=` are excluded from priority" rule (`label_names_excluded_from_priority_sort`, `issuable_finder.rb:sort`) | yes: edge_cases (`title`,`bogus`), mr_filters (popularity, milestone_due, label_priority, priority), random_walk_lists (updated_at, title, merged_at desc, popularity). Not: `merged_at` asc with unmerged MRs, `labels` + `label_priority` |
| `sort` | String, `asc desc`, default `desc` | direction | IMPL `mr.rs:227` | yes |
| `with_labels_details` | Boolean, default false | labels become objects (name,color,description,description_html,text_color) | IMPL `mr.rs:665` | NO |
| `with_merge_status_recheck` | Boolean, default false | async recheck (project: if caller can update; group: batch service); presenter skips individual rechecks | IMPL `mr.rs:666-683` (group batch path modelled) | yes: audit_followups_3, merge_status_matrix, push_side_effects (project) |
| `created_after` `created_before` `updated_after` `updated_before` | DateTime | inclusive bounds | IMPL `mr.rs:291-294,461` (ISO, offsets, date only). Gap: `created_after=` (empty) -> labrat 400, GitLab ignores | NO |
| `view` | String, `simple` | `MergeRequestSimple` entity | PARTIAL `mr.rs:664`: simple works; any other value is silently ignored, GitLab 400 `view does not have a valid value` | simple only (merge_requests.rs:160) |
| `scope` | String, `created-by-me assigned-to-me created_by_me assigned_to_me reviews_for_me all`; G default `created_by_me`, Gr/P none | `current_user_related?` scopes give none for anonymous; underscore-normalised | IMPL `mr.rs:228-241,327-356,785`; invalid 400 | only `scope=all` on G (archived_projects). `created_by_me` default, `assigned_to_me`, `reviews_for_me`, dashed forms: NO |
| `source_branch` / `target_branch` | String | exact match, `.presence` | PARTIAL `mr.rs:282,419`: exact match ok; empty string excludes everything (GitLab ignores) | NO (only used in create bodies) |
| `source_project_id` | Integer | `where(source_project_id:)` | IMPL `mr.rs:472`; `""` gap | yes mr_filters |
| `search` | String | title/description fuzzy (`full_search`, min-char rules, `pg_full_text_search` path) | IMPL `mr.rs:284,429` (term rules modelled) | yes edge_cases, random_walk_lists |
| `in` | String, free text | `title`, `description`, or comma list; unknown tokens fall back to both | IMPL `mr.rs:285` | yes (`in=title`, `in=bogus`, `in=description`) |
| `wip` | String, `yes no` | `to_boolean` -> title prefix `Draft - `/`Draft:`/`[Draft]`/`(Draft)` (ILIKE) | IMPL `mr.rs:290,451` | yes |
| `author_id` | Integer; mutually exclusive with `author_username` | `authored(id)` | IMPL `mr.rs:267`; mutex ok `:242`; `""` gap | NO |
| `author_username` | String | `User.by_username` (case-insensitive; group handle also accepted); unknown user -> empty | IMPL `mr.rs:267-270`; group handle `@group` not supported; empty string gap | yes (random_walk_lists positive; mr_filters only `not[]`) |
| `assignee_id` | `[Integer,String]` `integer_none_any` | `None` -> unassigned, `Any` -> assigned, else `assigned_to(id)` | PARTIAL `mr.rs:272,358`: values ok; invalid-value text/timing differ (see section 0) | `Any` only (random_walk_lists); numeric positive: NO |
| `assignee_username` | `Array[String]`, `check_assignees_count` (more than one value: 400 `assignee_username allows one value, but found 2: a, b`); `mutually_exclusive :assignee_id, :assignee_username` | assigned_to(users by username); String is wrapped, NOT comma-split | PARTIAL `mr.rs:252-264,272`: count check ok for `[]` form. Gaps: no `assignee_id`/`assignee_username` mutex 400; `assignee_username=a,b` is split by labrat (takes `a`) but GitLab looks up the literal user `a,b` (empty) | NO |
| `reviewer_id` | `[Integer,String]` `integer_none_any`; mutex with `reviewer_username` | `None` -> `no_review_requested`, `Any` -> `review_requested`, id -> `review_requested_to`; unknown id -> none | IMPL `mr.rs:277,373`, mutex `:247` | `None` only |
| `reviewer_username` | String | **Only** a real username (the None/Any wildcard is read from `reviewer_id` only, `merge_requests_finder/params.rb`); unknown -> none | IMPL `mr.rs:277-279` (`reviewer_username=None` returns empty, as GitLab) | yes |
| `labels` | `Array[String]` CommaSeparatedToArray | `None`/`Any` (case-insens, anywhere in list) else all must match | IMPL `mr.rs:280,388` + `coerce_list_query` | yes (edge_cases: None, none, Any, mixed, ghost, empty, trimmed) |
| `milestone` | String | `milestone_title`: `none`/`No Milestone`, `any`/`Any Milestone`, `#upcoming`, `#started` (REST uses the `#`-names, see `issuable_finder/params.rb`, `Timebox::Upcoming.name`), else title | PARTIAL `mr.rs:281,402`: title, `none`, `any` only. MISSING `#started`, `#upcoming`, `No Milestone`, `Any Milestone`. Empty string excludes all (GitLab ignores) | yes (`v1`, `None`) |
| `my_reaction_emoji` | String | `None` -> `not_awarded(user)` (MRs the user has not reacted to at all), `Any` -> `awarded(user)`, name -> `awarded(user, name:)`; match is case-insensitive; no-op for anonymous | **PARTIAL, bug** `mr.rs:560-575`: SQL counts `name = 'None'` so `None` returns EVERY MR (even those the user reacted to); `Any` is case-sensitive in SQL (`any`/`ANY` -> matches nothing) | yes (`thumbsup` only; `None`/`Any` NO) |
| `deployed_before` `deployed_after` | untyped | MRs with a deployment in the window (`DeploymentMergeRequest`); unparsable date -> nil -> ignored | **MISSING** (ignored: returns all MRs). labrat has no deployments, so GitLab returns `[]` whenever either is set with a parsable date | NO |
| `environment` | untyped | MRs deployed to env name | **MISSING** (ignored); GitLab returns `[]` (no deployments) | NO |
| `approved` | String, `yes no` | declared and passed to the finder as a scalar param but no `by_approved` exists in 18.11 `merge_requests_finder.rb`: accepted, no effect (verify live) | PARTIAL: ignored like GitLab, but no `values` check: `approved=maybe` -> labrat 200, GitLab 400 | NO |
| `merge_user_id` / `merge_user_username` | Integer / String, mutually exclusive | `merged_by(user)` via metrics; unknown user -> none | IMPL `mr.rs:299-310,475` | yes |
| `not` | Hash with negatable params (author_id, author_username, assignee_id, assignee_username, labels, milestone, my_reaction_emoji, reviewer_id, reviewer_username) | see `not[...]` rows | PARTIAL `mr.rs:490-559` | partial |
| `not[author_id]` | Integer | not_authored | IMPL `:547` | NO |
| `not[author_username]` | String | not_authored(by_username) | IMPL `:492` (username compare case-sensitive) | yes |
| `not[assignee_id]` | `[Integer,String]` | `not_assigned_to` | IMPL `:497` | yes |
| `not[assignee_username]` | Array[String] | `not_assigned_to(users)` | IMPL `:501` | NO |
| `not[reviewer_id]` / `not[reviewer_username]` | Integer / String (mutex) | `no_review_requested_to(user)`; **unknown user -> `items.none`** | PARTIAL `:513-530`: unknown user is a no-op in labrat (returns all MRs), GitLab returns `[]` | username yes (existing user only); id NO |
| `not[milestone]` | String | not-title; `#upcoming`/`#started` negate those sets | PARTIAL `:531` title only | yes (title) |
| `not[labels]` | Array | exclude MRs with any listed label (names are literal, no None/Any) | IMPL `:552` | yes |
| `not[my_reaction_emoji]` | String | `not_awarded(user, name:)` | IMPL `:538` | yes |
| `non_archived` (G default false, Gr default true; not declared on P) | Boolean | `items.non_archived` when truthy | Gr IMPL `mr.rs:764`; **G MISSING**: `non_archived=true` is not applied (only echoed into pagination links `:792`) | Gr false only (archived_projects); G NO |
| `iids` (P only) | `Array[Integer]` CommaSeparatedToIntegerArray | `iid_in` | IMPL `mr.rs:295,468` incl. `iids[]=a`; PARTIAL: also applied on G/Gr where GitLab drops the undeclared param | yes (P) |
| `page` `per_page` | pagination | | IMPL | yes pagination_extremes, edge_cases |

Declared by finder but NOT declared in REST (GitLab drops them, labrat also ignores them, no gap): `draft`, `merged_after/before`, `review_state(s)`, `or[...]`, `commit_sha`, `blob_path`, `not[target_branch]`, `not[approved_by_usernames]`, `approved_by_ids`, `approved_by_usernames`, `approver_ids`, `approver_usernames`, `include_assigned`.

## 2. Single-MR endpoints

| Endpoint | Param | Grape | GitLab semantics | labrat | Scenario |
|---|---|---|---|---|---|
| POST P | `title` `source_branch` `target_branch` | String, required | 400 `x is missing` (all missing listed) | IMPL `mr.rs:866` | yes |
| POST P | `target_project_id` | Integer | fork target; non-fork other project -> 422/404 | PARTIAL `mr.rs:879` (forks out of scope; error paths only) | yes (api_error_paths) |
| POST/PUT | `assignee_id` Integer, `assignee_ids` Array[Integer] CSV | assignee(s); `0`/empty clears | IMPL `mr.rs:812` | yes |
| POST/PUT | `reviewer_ids` | Array[Integer] CSV; `0`/empty clears | IMPL `mr.rs:822` | yes |
| POST/PUT | `description` | String | quick actions in text executed | IMPL `mr.rs:855 take_quick_actions` | yes (description_quick_actions) |
| POST/PUT | `labels` `add_labels` `remove_labels` | Array[String] CSV | IMPL | yes |
| POST/PUT | `milestone_id` | Integer; 0 clears | IMPL | yes |
| POST/PUT | `remove_source_branch` | Boolean -> `force_remove_source_branch` (needs push_code) | PARTIAL: no `push_code` check (known, edge-cases-rest-params #8) | yes |
| POST/PUT | `allow_collaboration`, `allow_maintainer_to_push` (alias) | Boolean | stored; exposed in JSON only `for_fork?` | PARTIAL `mr.rs:839`: parsed, counts for at_least_one_of, never stored (forks out of scope, invisible for non-fork) | NO |
| POST/PUT | `squash` | Boolean | IMPL | yes |
| POST/PUT | `merge_after` | String | schedule | IMPL `mr.rs:844` | yes |
| PUT | `title` `target_branch` | String `allow_blank: false` | 400 `x is empty` | IMPL `mr.rs:944` | yes |
| PUT | `state_event` | `close reopen` | 400 invalid value | IMPL `one_of` | yes |
| PUT | `discussion_locked` | Boolean | IMPL | yes |
| PUT | at_least_one_of (17 keys) | | 400 `a, b ... are missing, at least one parameter must be provided` | IMPL `mr.rs:925-940` same order | yes |
| DELETE | If-Unmodified-Since | header | 412 | IMPL `mr.rs:1006` | NO |
| GET show | `render_html` | Boolean | rendered title/description | IMPL `mr.rs:905` | NO |
| GET show | `include_diverged_commits_count` | Boolean | `diverged_commits_count` | IMPL `mr.rs:908` | NO |
| GET show | `include_rebase_in_progress` | Boolean | | IMPL | yes |
| GET participants / reviewers / commits / versions / closes_issues / related_issues | `page` `per_page` | | IMPL (paginate) | partly |
| GET closes_issues, related_issues | (none) | issues referenced via closing patterns / mentions | **MISSING** `mr.rs:1152 empty_list`: always `[]` (labrat has no issues) | NO |
| GET changes | `unidiff` (Boolean, default false); `access_raw_diffs` (undeclared, read via `to_boolean`) | | IMPL `mr.rs:1067,1069` | yes (unidiff, access_raw_diffs) |
| GET diffs | `unidiff`, `page`, `per_page` | | IMPL `mr.rs:1103` | yes |
| GET raw_diffs | (none) | text/plain patch | IMPL | yes |
| GET versions/:version_id | `version_id` Integer, `unidiff` | | IMPL `mr.rs:1345` | yes |
| GET pipelines | pagination | | IMPL (default per_page 15 forced `:1141`) | yes |
| POST pipelines | `async` (Boolean, default false) | true -> 202 empty, creation async | **MISSING**: `pipelines.rs:294` ignores `async`, always errors (`NO_PIPELINE`) | partly (pipeline_endpoints) |
| PUT merge | `merge_commit_message` `squash_commit_message` | String | commit messages | IMPL `mr.rs:1166` | NO |
| PUT merge | `should_remove_source_branch` | Boolean | | IMPL | yes |
| PUT merge | `merge_when_pipeline_succeeds` (deprecated), `auto_merge` | Boolean, OR-ed | | IMPL `mr.rs:1171` | yes |
| PUT merge | `sha` | String | 409 `SHA does not match HEAD of source branch: <sha>` | IMPL `:1184` | yes |
| PUT merge | `squash` | Boolean | saved before the mergeability decision | IMPL `:1176,1196` | yes |
| GET merge_ref | (none) | `commit_id` | PARTIAL (known: non-open MR 400 vs 200) | yes |
| POST cancel_merge_when_pipeline_succeeds | (none) | | IMPL | yes |
| PUT rebase | `skip_ci` | Boolean | | IMPL `mr.rs:1304` | NO |
| POST time_estimate | `duration` String required | `3h30m`; `0` allowed; invalid -> 400 reason | IMPL `time_op` `mr.rs:1487` | yes (time_tracking_extremes) |
| POST add_spent_time | `duration` String required, `summary` String (undeclared) | | IMPL | `duration` yes; `summary` NO |
| POST reset_time_estimate / reset_spent_time / GET time_stats | (none) | | IMPL | yes |
| POST subscribe / unsubscribe | (none) | 304 when unchanged; needs update_merge_request | IMPL `mr.rs:1376` (permission check looser: Reporter+) | yes |
| POST todo | (none) | 304 when pending todo exists | IMPL `mr.rs:1411` | yes |

## 3. Notes, discussions, drafts, approvals, award emoji, resource events

| Endpoint | Param | Grape | GitLab semantics | labrat | Scenario |
|---|---|---|---|---|---|
| GET notes | `order_by` | `created_at updated_at`, default `created_at` | `created_at` is really `id` (`helpers.rb:560`) | PARTIAL `notes.rs:171`: sorts `created_at` then id (differs only for notes with owner-set `created_at`) | NO |
| GET notes | `sort` | `asc desc`, default `desc` | | IMPL | yes (`asc`) |
| GET notes | `activity_filter` | `all_notes only_comments only_activity` | `with_notes_filter` | IMPL `:177` | NO |
| POST notes | `body` | String required | | IMPL | yes |
| POST notes | `internal`, `confidential` (deprecated) | Boolean | | IMPL `:246` | yes `internal`; `confidential` NO |
| POST notes | `created_at` | String | dropped without `set_note_created_at`; unparseable -> now | IMPL `:249` | yes (rest_params) |
| POST notes | `merge_request_diff_head_sha` | String | gates `/merge` quick action | IMPL | partly |
| PUT notes/:id | `body` (allow_blank false), `confidential` (deprecated, ignored) | | IMPL `:272` | yes |
| GET discussions(/:id/notes) | pagination | | IMPL | yes |
| POST discussions | `body`, `created_at`, `position{base_sha,start_sha,head_sha,position_type(text image file),new_path,new_line,old_path,old_line,width,height,x,y,line_range{start,end{line_code,type,old_line,new_line}}}` | `internal`/`merge_request_diff_head_sha` NOT declared (ignored) | IMPL `notes.rs:429-445` (ignored correctly), position integer check `:392-420` | yes; `line_range` yes (review_edges) |
| POST discussions/:id/notes (reply) | `body`, `created_at` only | `internal`/`merge_request_diff_head_sha` ignored | PARTIAL `notes.rs:447`: uses the shared `create_params`, still reads `internal` and `merge_request_diff_head_sha` (only `create_discussion` clears them) | NO |
| PUT discussions/:id/notes/:nid | `body`, `resolved` | | IMPL `:471` | yes |
| PUT discussions/:id | `resolved` Boolean required | | IMPL `:520` | yes |
| GET/POST/PUT/DELETE draft_notes | `note`, `in_reply_to_discussion_id`, `commit_id`, `resolve_discussion`, `position{...}` | | IMPL `notes.rs:588` (500 on denied create, as GitLab) | yes |
| PUT draft_notes/bulk_publish, /:id/publish | (none) | | IMPL | yes |
| POST approve | `sha` (409 on mismatch), `publish_review` | | IMPL `:908` | `sha` yes; `publish_review` NO |
| POST unapprove, PUT reset_approvals, GET approvals | (none) | | IMPL | yes |
| POST/GET/DELETE award_emoji (MR and note) | `name` String required; `award_id` Integer; pagination | | IMPL `notes.rs:703-800` | yes |
| GET resource_{label,state,milestone}_events | pagination, `event_id` | | IMPL `milestones.rs` | yes |

## 4. Prioritised gaps (probe = request plus precondition for a conformance scenario)

P1 (a bot or dashboard will hit these)
1. `my_reaction_emoji=None` returns all MRs (bug); `Any`/`any` case handling. Probe: 3 open MRs, admin awards `thumbsup` on MR1 only; `GET /projects/:id/merge_requests?my_reaction_emoji=None` (expect MR2, MR3), `...=Any` (MR1), `...=any` (MR1), `...=ANY`.
2. `milestone=#started`, `#upcoming`, `No Milestone`, `Any Milestone`, and `milestone=` (empty). Probe: milestones A (start_date past, due future), B (start_date future), C (no dates) on one project; MR per milestone plus MR without; list with each value.
3. `approved=maybe` -> 400 `approved does not have a valid value`; `view=bogus` -> 400. Probe: any project (no preconditions needed), include `approved=yes` (200, unfiltered).
4. `merged_at` ordering: `order_by=merged_at&sort=asc&state=all` with one merged and two open MRs (GitLab: merged first, NULLs last in both directions, labrat: NULLs first on asc).
5. `non_archived=true` on global list. Probe: archived project with an MR plus normal project; `GET /merge_requests?scope=all&non_archived=true` (archived MR must disappear).
6. `environment=production` / `deployed_after=2020-01-01T00:00:00Z` / `deployed_before=...` on project list (GitLab `[]` because no deployments; labrat returns everything).
7. Empty typed params from templated URLs: `created_after=`, `author_id=`, `merge_user_id=`, `target_branch=`, `source_branch=`, `milestone=`, `author_username=` (GitLab ignores; labrat 400 or empty list). Probe: 2 MRs, one request per empty param.

P2
8. `assignee_id`/`reviewer_id` invalid value message and eager validation: `GET .../merge_requests?assignee_id=abc` on a project with ZERO MRs (GitLab 400 `assignee_id should be an integer, none, or any, however got abc`; labrat 200 `[]`), plus `assignee_id=1&assignee_username=x` (mutex 400) and `reviewer_id=abc`.
9. `not[reviewer_username]=ghost` and `not[reviewer_id]=999999` (GitLab `[]`, labrat all); `not[milestone]=#started`.
10. `created_after/created_before/updated_after/updated_before` have no scenario at all: probe with MRs created across a `sleep`, bounds exactly at an MR's `created_at` (inclusive), date-only and offset formats, invalid (`created_after=yesterday` 400 `created_after is invalid`).
11. `scope=created-by-me|assigned-to-me|reviews_for_me` on G (default `created_by_me`, anonymous `scope` absent -> 401, `scope=all` anonymous) and on P/Gr for a non-author user; `assignee_username[]=a&assignee_username[]=b` 400; `assignee_username=a,b`.
12. `labels=p1&order_by=label_priority&sort=asc` (labels in the filter are excluded from priority ordering).
13. `skip_ci=true` on rebase, `merge_commit_message`/`squash_commit_message` on merge (check resulting commit message).
14. `POST /merge_requests/:iid/pipelines?async=true` -> 202 empty body in GitLab.
15. Group/global `iids[]=1` (GitLab ignores undeclared `iids` on G and Gr, labrat filters).
16. `GET .../notes?activity_filter=only_activity&order_by=updated_at&sort=asc`, plus `activity_filter=bogus` 400.
17. `POST discussions/:id/notes` with `internal=true` as Reporter+ (GitLab inherits parent confidentiality, ignores the param).

P3 / known out-of-scope
18. `closes_issues` / `related_issues` always `[]` (needs issues, not implemented).
19. `allow_collaboration` (invisible for non-forks), `render_html`, `include_diverged_commits_count`, `with_labels_details=true` (no scenarios: add one each, cheap), `If-Unmodified-Since` on MR DELETE, `summary` on `add_spent_time`.
