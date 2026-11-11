# labrat vs GitLab 18.11: REST endpoints not implemented

Inventory only. GitLab source: the CE 18.11 source copy (paths below relative to it). labrat routes:
`crates/labrat-api/src/routes/*.rs` (all merged under `/api/v4` in `lib.rs`; no REST routes in `git_http.rs`/`ui.rs`).
Method: a throw-away indentation parser over the Grape `resource/namespace/route_param` nesting, then manual
reading for helper-mounted endpoints (notes/discussions/award_emoji/events/subscriptions/time tracking). Not re-verified
by running anything. Paths are relative to `/api/v4`. `:id` = project (or group) id/path.

## Result for the MR core

Every route in lib/api/merge_requests.rb, merge_request_diffs.rb, draft_notes.rb, subscriptions.rb (MR + project label),
suggestions.rb, todos.rb (project MR `/todo`, `/todos*`), time_tracking_endpoints.rb, resource_{label,state,milestone}_events.rb,
notes.rb/discussions.rb/award_emoji.rb (MR, MR notes, commit comments/discussions) and merge_request_approvals.rb has a labrat route. Gaps are therefore in the repository, project, group,
user and utility areas below.

Not-in-labrat but noted differences in already-routed endpoints:
- `GET /groups/:id/-/search` and `GET /projects/:id/-/search` (lib/api/search.rb:279, 316 use `:id/(-/)search`); labrat only routes `/groups/{id}/search`, `/projects/{id}/search`.
- `GET /projects/:id/repository/branches/:branch` also answers `HEAD` (lib/api/branches.rb:97); labrat has no HEAD on it.
- `PUT /projects/:id/labels/promote` (lib/api/labels.rb:113, deprecated, name in body); labrat only has `/labels/{name}/promote`.

---

## 1. Repository bits (high likelihood)

| # | Method + path | Source | Notes |
|---|---|---|---|
| 1.1 | `GET /projects/:id/protected_tags` | lib/api/protected_tags.rb:32 | list protected tags (array of `{name, create_access_levels[]}`)|
| 1.2 | `GET /projects/:id/protected_tags/:name` | lib/api/protected_tags.rb:54 | single protected tag (wildcard names allowed); 404 if none |
| 1.3 | `POST /projects/:id/protected_tags` | lib/api/protected_tags.rb:82 | create; `name`, `create_access_level`,  |
| 1.4 | `DELETE /projects/:id/protected_tags/:name` | lib/api/protected_tags.rb:115 | unprotect, 204 |
| 1.5 | `GET /projects/:id/repository/files/:file_path/blame` | lib/api/files.rb:218 | array of `{commit{...}, lines[]}` blame ranges; params `ref` required, `range[start]`, `range[end]` |
| 1.6 | `HEAD /projects/:id/repository/files/:file_path/blame` | lib/api/files.rb:195 | same headers as GET, no body |
| 1.7 | `GET /projects/:id/repository/archive` (also `archive.:format`) | lib/api/repositories.rb:218 | tar/zip stream of `sha`, `path`, `format`; used by renovate-like tools for download |
| 1.8 | `GET /projects/:id/repository/contributors` | lib/api/repositories.rb:315 | `[{name,email,commits,additions,deletions}]`; `order_by`, `sort`, pagination |
| 1.9 | `GET /projects/:id/repository/tags/:tag_name/signature` | lib/api/tags.rb:172 | tag signature (GPG/SSH/X509); 404 if tag unsigned |
| 1.10 | `GET /projects/:id/repository/changelog` | lib/api/repositories.rb:366 | generated changelog notes for `version` (`{notes: "..."}`) |
| 1.11 | `POST /projects/:id/repository/changelog` | lib/api/repositories.rb:408 | commits changelog to a file on `branch`; 200 empty |
| 1.12 | `GET /projects/:id/repository/health` | lib/api/repositories.rb:283 | repo size/health JSON (admin-ish, rarely used) |
| 1.13 | `PUT /projects/:id/repository/submodules/:submodule` | lib/api/submodules.rb:54 | update submodule pointer via commit (`branch`, `commit_sha`, `commit_message`) |
| 1.14 | `POST /projects/:id/repository/commits/authorize` | lib/api/commits.rb:299 | Workhorse-only upload authorize; not for clients |
| 1.15 | `POST /projects/:id/repository/files/:file_path/authorize`, `PUT .../authorize` | lib/api/files.rb:319, 363 | Workhorse-only; not for clients |
| 1.16 | `HEAD /projects/:id/repository/branches/:branch` | lib/api/branches.rb:97 | existence check (200/404) |

Global templates (lib/api/templates.rb): `GET /templates/licenses` (:43), `GET /templates/licenses/:name` (:64),
`GET /templates/:type` (:91, types gitignores/gitlab_ci_ymls/dockerfiles), `GET /templates/:type/:name` (:106). Only the project-scoped
`/projects/:id/templates/:type[/:name]` is routed.

Pipelines used with MR/commit status: all MR/commit-status routes (`POST /statuses/:sha`, commit `statuses`, MR `pipelines` GET/POST,
`/projects/:id/pipelines*`) are routed. Other pipeline/job endpoints are out of scope.

## 2. Users and current user (high likelihood: glab/python-gitlab call these)

| # | Method + path | Source | Notes |
|---|---|---|---|
| 2.1 | `GET /users/:user_id/status` | lib/api/users.rb:286 | `{emoji,message,message_html,availability,clear_status_at}` |
| 2.2 | `GET /user/status` | lib/api/users.rb:1658 | own status, same shape |
| 2.3 | `PUT /user/status`, `PATCH /user/status` | lib/api/users.rb:1630, 1643 | set status (`emoji`,`message`,`availability`,`clear_status_after`) |
| 2.4 | `GET /user/emails`, `GET /user/emails/:email_id`, `POST /user/emails`, `DELETE /user/emails/:email_id` | lib/api/users.rb:1417, 1548, 1564, 1582 | own emails; `[{id,email,confirmed_at}]` |
| 2.5 | `GET /users/:id/emails`, `POST /users/:id/emails`, `DELETE /users/:id/emails/:email_id` | lib/api/users.rb:803, 778, 821 | admin-managed emails |
| 2.6 | `GET /user/keys`, `GET /user/keys/:key_id`, `POST /user/keys`, `DELETE /user/keys/:key_id` | lib/api/users.rb:1256, 1271, 1291, 1310 | own SSH keys |
| 2.7 | `GET /users/:user_id/keys` (public), `GET /users/:id/keys/:key_id`, `POST /users/:user_id/keys`, `DELETE /users/:id/keys/:key_id` | lib/api/users.rb:593, 612, 568, 633 | user SSH keys |
| 2.8 | `GET/POST /user/gpg_keys`, `GET/DELETE /user/gpg_keys/:key_id`, `POST /user/gpg_keys/:key_id/revoke` | lib/api/users.rb:1330, 1361, 1344, 1398, 1380 | own GPG keys |
| 2.9 | `GET/POST /users/:id/gpg_keys`, `GET/DELETE /users/:id/gpg_keys/:key_id`, `POST .../revoke` | lib/api/users.rb:686, 659, 707, 729, 754 | user GPG keys |
| 2.10 | `GET /user/preferences`, `PUT /user/preferences` | lib/api/users.rb:1535, 1512 | user preference flags |
| 2.11 | `GET /user/activities`, `GET /user/support_pin`, `POST /user/support_pin`, `PUT /user/avatar` | lib/api/users.rb:1610, 1484, 1466, 1671 | misc own-user |
| 2.12 | `GET /users/:user_id/memberships` | lib/api/users.rb:1039 | admin; project/namespace memberships; `type` filter |
| 2.13 | `GET /users/:id/associations_count` | lib/api/users.rb:1074 | admin; `{groups_count,projects_count,issues_count,merge_requests_count}` |
| 2.14 | `POST /users/:id/follow`, `POST /users/:id/unfollow`, `GET /users/:id/following`, `GET /users/:id/followers` | lib/api/users.rb:304, 327, 352, 372 | follow graph |
| 2.15 | `POST /users/:id/ban`, `POST /users/:id/unban`, `POST /users/:id/approve`, `POST /users/:id/reject` | lib/api/users.rb:1000, 1018, 887, 906 | admin state transitions (labrat has block/unblock/activate/deactivate only) |
| 2.16 | `GET /users/:user_id/contributed_projects`, `GET /users/:user_id/starred_projects` | lib/api/projects.rb:311, 334 | project lists (labrat has `/users/:user_id/projects`) |
| 2.17 | `GET /users/:user_id/project_deploy_keys` | lib/api/users.rb:538 | |
| 2.18 | `PATCH /users/:id/disable_two_factor`, `DELETE /users/:id/identities/:provider`, `GET/POST /users/:id/support_pin(/revoke)`, `PUT /user/:user_id/credit_card_validation` | lib/api/users.rb:489, 516, 122, 150, 1441 | admin/rare |
| 2.19 | `GET /keys/:id`, `GET /keys?fingerprint=` | lib/api/keys.rb:21, 40 | look up user by SSH key/fingerprint |
| 2.20 | `GET /avatar?email=` | lib/api/avatar.rb:18 | `{avatar_url}` |
| 2.21 | `GET /user_counts` | lib/api/user_counts.rb:15 | `{merge_requests, assigned_issues, assigned_merge_requests, review_requested_merge_requests, todos}` (used by UI/CLI status) |
| 2.22 | `GET /notification_settings`, `PUT /notification_settings` | lib/api/notification_settings.rb:20, 39 | global notification level |
| 2.23 | `GET/PUT /projects/:id/notification_settings`, `GET/PUT /groups/:id/notification_settings` | lib/api/notification_settings.rb:79, 99 | per-source |
| 2.24 | `POST /personal_access_tokens/:id/rotate`, `POST /personal_access_tokens/self/rotate`, `GET /personal_access_tokens/self/associations` | lib/api/personal_access_tokens.rb:73; personal_access_tokens/self_rotation.rb:35; self_information.rb:67 | token rotation (renovate/bots that manage their own PAT) |
| 2.25 | `GET /namespaces`, `GET /namespaces/:id`, `GET /namespaces/:id/exists` | lib/api/namespaces.rb:43, 76, 95 | namespace lookup (glab uses `/namespaces/:id` and `exists`) |

## 3. Projects (medium)

| # | Method + path | Source | Notes |
|---|---|---|---|
| 3.1 | `GET /projects/:id/users` | lib/api/projects.rb:955 | project user list (mention autocomplete), `search`, `skip_users[]` |
| 3.2 | `GET /projects/:id/groups` | lib/api/projects.rb:985 | ancestor groups the project is shared with |
| 3.3 | `GET /projects/:id/invited_groups` | lib/api/projects.rb:1006 | groups invited to project |
| 3.4 | `POST /projects/:id/star`, `POST /projects/:id/unstar`, `GET /projects/:id/starrers` | lib/api/projects.rb:705, 725, 750 | star state; star returns project |
| 3.5 | `GET /projects/:id/languages` | lib/api/projects.rb:765 | `{"Ruby": 65.3, ...}` |
| 3.6 | `POST /projects/:id/share`, `DELETE /projects/:id/share/:group_id` | lib/api/projects.rb:859, 890 | share with group |
| 3.7 | `PUT /projects/:id/transfer`, `GET /projects/:id/transfer_locations` | lib/api/projects.rb:1078, 1105 | move project |
| 3.8 | `POST /projects/:id/restore` | lib/api/projects.rb:353 | undelete (delayed deletion) |
| 3.9 | `POST /projects/:id/import_project_members/:project_id` | lib/api/projects.rb:921 | copy members |
| 3.10 | `GET /projects/:id/share_locations`, `GET /projects/:id/pages_access`, `POST /projects/:id/housekeeping`, `POST /projects/:id/repository_size`, `GET /projects/:id/storage` | lib/api/projects.rb:484, 605, 1028, 1057, 1129 | misc |
| 3.12 | `GET /projects/:id/members` extras: `POST/GET/PUT/DELETE /projects/:id/access_requests`, `/invitations` | lib/api/access_requests.rb:29-105; invitations.rb:43-132 | membership workflows (also for `/groups/:id/...`) |
| 3.13 | `GET/POST/PUT/DELETE /projects/:id/badges[/:badge_id]`, `/badges/render` | lib/api/badges.rb:42-153 | also for groups |

Hooks: `/projects/:id/hooks*` (incl. test/events/resend/url_variables/custom_headers) and `/hooks*` are fully routed.
Project members: all 7 shared routes (list/all/show/show all/add/update/delete) routed.

## 4. Groups (medium)

| # | Method + path | Source | Notes |
|---|---|---|---|
| 4.1 | `POST /groups/:id/labels`, `GET /groups/:id/labels/:name`, `PUT /groups/:id/labels/:name`, `PUT /groups/:id/labels`, `DELETE /groups/:id/labels/:name`, `DELETE /groups/:id/labels` | lib/api/group_labels.rb:85, 72, 128, 101, 141, 114 | group label CRUD (labrat only lists) |
| 4.2 | `POST /groups/:id/labels/:subscribable_id/subscribe`, `.../unsubscribe` | lib/api/subscriptions.rb:55, 71 (source Group labels) | |
| 4.3 | `PUT /groups/:id/milestones/:milestone_id`, `DELETE /groups/:id/milestones/:milestone_id` | lib/api/group_milestones.rb:68, 79 | |
| 4.4 | `GET /groups/:id/milestones/:milestone_id/issues`, `.../merge_requests` | lib/api/group_milestones.rb:97, 111 | |
| 4.5 | `GET /groups/:id/projects/shared` | lib/api/groups.rb:573 | |
| 4.6 | `GET /groups/:id/groups/shared`, `GET /groups/:id/invited_groups` | lib/api/groups.rb:471, 494 | |
| 4.7 | `POST /groups/:id/projects/:project_id` | lib/api/groups.rb:618 | transfer project into group |
| 4.8 | `POST /groups/:id/archive`, `/unarchive`, `/restore`, `/transfer`, `/transfer_to_organization`, `GET /groups/:id/transfer_locations`, `POST /groups/:id/share`, `DELETE /groups/:id/share/:group_id` | lib/api/groups.rb:366, 390, 441, 662, 694, 642, 730, 757 | |
| 4.9 | `GET /groups/:id/access_requests`, `/invitations`, `/badges` | see 3.12/3.13 | |

Group members (7 shared routes), `/groups`, `/groups/:id`, subgroups, descendant_groups, group projects are routed.

## 5. Utility / instance (high for bots that render text or probe the server)

| # | Method + path | Source | Notes |
|---|---|---|---|
| 5.1 | `POST /markdown` | lib/api/markdown.rb:29 | body `text`, `gfm`, `project`; returns `{"html": "..."}`; 201. Used by preview/editor integrations |
| 5.2 | `GET /application/statistics` | lib/api/statistics.rb:16 | admin; counts |
| 5.3 | `GET /features`, `/features/definitions`, `POST /features/:name`, `DELETE /features/:name` | lib/api/features.rb:18, 30, 81, 99 | admin feature flags |
| 5.4 | `GET /projects/:id/ci/lint`, `POST /projects/:id/ci/lint` | lib/api/lint.rb:37, 74 | CI config lint (renovate no, tooling yes) |

`/version`, `/metadata`, `/application/settings` GET/PUT, `/search*`, `/todos*`, `/events*` are routed.
