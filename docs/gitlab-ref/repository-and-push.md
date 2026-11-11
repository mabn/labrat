# GitLab 18.11 reference: repository REST APIs, protected branches, Git-over-HTTP, push effects

Source root: GitLab CE source (`/home/mabn/dev/gitlab-run-ce/gitlab-src`; all paths below are relative to it).
Gitaly source is NOT available in the tree; statements about Gitaly's internal git flags are marked
**[unverified]**.

Conventions used by the Rails API layer (lib/api/helpers.rb):
- `not_found!(res)` -> 404 `{"message":"404 <res> Not Found"}` (no res => `"404 Not Found"`) (helpers.rb:593).
- `forbidden!(reason)` -> 403 `{"message":"403 Forbidden"}`; with reason: `"403 Forbidden - <reason>"` (helpers.rb:581, 670).
- `bad_request!(reason)` -> 400 `"400 Bad request"` / `"400 Bad request - <reason>"` (helpers.rb:585).
- `unauthorized!` -> 401 `"401 Unauthorized"`. `render_api_error!(msg, status)` -> `{"message": msg}` (msg may be an Array for model errors).
- Grape param validation failures (missing/invalid) -> 400 `{"error":"name is missing"}` (different shape: `error`, not `message`).
- Default pagination: `per_page` 20, max 100 (config/initializers/kaminari_config.rb:4-5). `page` default 1.
- Project `:id` accepts integer id or URL-encoded full path. 404 for unknown/invisible project: `{"message":"404 Project Not Found"}`.

---------------------------------------------------------------------------------------------------
## 1. Entities (exact field order)

### Commit (lib/api/entities/commit.rb:5-34)
`id, short_id, created_at, parent_ids, title, message, author_name, author_email, authored_date,
committer_name, committer_email, committed_date, trailers, extended_trailers, web_url`
- `short_id` = first 8 chars of sha (Gitlab::Git::Commit#short_id(7) is `id[0..7]`, lib/gitlab/git/commit.rb:247, MIN_SHA_LENGTH=7 at :18).
- `created_at` == `committed_date` (git/commit.rb:261). Dates are Time objects in the commit's own tz offset (init_date_from_gitaly, git/commit.rb:465), serialized ISO-8601 with milliseconds, e.g. `2012-05-28T04:42:42.000-07:00`.
- `title` = `full_title` = first line of message split on `[\r\n]`; `"No commit message"` if blank (commit.rb:244).
- `message` = full raw message (safe_message).
- `trailers`: hash `{}` unless Gitaly was asked for trailers (list endpoint: `trailers=true`); `extended_trailers`: hash name -> array of values. Single-commit GET always carries them (Gitaly find_commit) **[unverified]**.
- `web_url` = `<base>/<full_path>/-/commit/<sha>`.

### CommitStats (entities/commit_stats.rb): `additions, deletions, total`.
### CommitWithStats (entities/commit_with_stats.rb): Commit fields + `stats` (used by list with `with_stats=true`).
### CommitDetail (entities/commit_detail.rb): Commit fields, then `stats` (only if `include_stats`), `status` (last pipeline status of the commit or null), `project_id`, `last_pipeline` (PipelineBasic or null).
Note: `status`/`last_pipeline` null when no pipeline exists; `last_pipeline` present only if user `:read_pipeline`.

### Branch (entities/branch.rb:5-74) field order
`name, commit (Commit entity of branch tip, dereferenced), merged, protected, developers_can_push, developers_can_merge, can_push, default, web_url`
- `merged`: with list endpoint = name is in `repository.merged_branch_names(names)` (merged into default branch and not identical); single endpoints (`merged_to_root_ref?`, repository.rb:1064): `nil` if repo has no head commit; otherwise `branch.target != root_ref_sha && ancestor?(branch.target, root_ref_sha)` => the default branch itself is `merged:false`.
- `protected`: `ProtectedBranch.protected?(project, name)` (protected_branch.rb): true if (repo empty AND default branch protected) or any protected-branch rule (project or inherited group rule) matches name (exact or wildcard).
- `developers_can_push` / `developers_can_merge`: true iff any push/merge access level of a *matching* protected rule has access_level == 30 (`ProtectedRef.developers_can?`, concerns/protected_ref.rb).
- `can_push`: `Gitlab::UserAccess#can_push_to_branch?(name)` for current_user (false for anonymous). See section 3.5.
- `default`: `project.default_branch == name`.
- `web_url`: `<base>/<path>/-/tree/<branch name>` (name NOT percent-encoded for `/`; other special chars as Rails url helper encodes).

### Tag (entities/tag.rb:5-29) order
`name, message, target, commit, release (only if can read_release: object {tag_name, description} or null), protected, created_at`
- `message`: annotated tag message, else null. `target`: for annotated = tag object sha; lightweight = commit sha. `commit` = dereferenced commit. `created_at` = tag's `date` (tagger date for annotated; commit date for lightweight).
- (`release` key present for users with read_release (Reporter+ / public), key absent otherwise.)

### Diff (entities/diff.rb:5-23) order
`diff, collapsed, too_large, new_path, old_path, a_mode, b_mode, new_file, renamed_file, deleted_file, generated_file`
- `diff` = `json_safe_diff` (binary -> `"Binary files <old> and <new> differ\n"`, see section 5); with `unidiff=true` -> `unidiff`.
- Modes: octal strings from gitaly ints (`old_mode.to_i.to_s(8)`), e.g. `"100644"`; new file => `a_mode "0"`, deleted => `b_mode "0"` (git/diff.rb:301-302).
- `new_file` = `from_id` is all-zero sha; `deleted_file` = `to_id` all-zero; `renamed_file` = `from_path != to_path`.

### TreeObject (entities/tree_object.rb): `id, name, type, path, mode`
- `type` in `tree|blob|commit` (commit = submodule). `mode` zero-padded to 6 chars: `"040000"`, `"100644"`, `"100755"`, `"120000"`, `"160000"`.

### Compare (entities/compare.rb:5-31) order
`commit, commits, diffs, compare_timeout, compare_same_ref, web_url`
- `commit` = `compare.commits.last` (the newest = head side) or null if no commits.
- `commits` order: `Gitlab::Git::Commit.between(base, head)` (rev-list `base..head`, **oldest first** per GitLab UI convention; `commit` is `.last`) **[order unverified; derived from `.last` being the head]**.
- `compare_timeout` = `diffs.overflow?`. `compare_same_ref` = base.id == head.id (then `commits`=[] and diffs=[]).
- `web_url`: `<base>/<path>/-/compare/<from>...<to>` (shas of start/base commit; with `straight=true` uses two-dot route `/-/compare/<from>..<to>`; url_builder.rb:88-96). For non-straight, `from` = merge-base sha (`base_commit_sha || start_commit_sha`) per Compare#to_param (models/compare.rb:30).

### Contributor, BasicRef
Contributor: `name, email, commits, additions, deletions`. BasicRef (commit refs): `type` ("branch"|"tag"), `name`.

---------------------------------------------------------------------------------------------------
## 2. Branches (lib/api/branches.rb)

Common `before`: `require_repository_enabled!` (404 `"404 Repository Not Found"` if repo feature disabled), `authorize_read_code!` (403 `"403 Forbidden"` if cannot `:read_code`). `rescue_from NoRepository -> 404 "404 Not Found"`. Branch routes use `branch: NO_SLASH_URL_PART_REGEX` only for single-branch routes (names with `/` must be URL-encoded as `%2F`).

### GET /projects/:id/repository/branches (branches.rb:56-85)
Params: `page`, `per_page`, `search` (String), `regex` (String), `sort` (`name_asc|updated_asc|updated_desc`; default `name`), `page_token` (keyset).
- Auth: public repo readable anonymously (job token allowed).
- Default order = name ascending (Gitaly NAME). `updated_asc`/`updated_desc` order by tip commit date.
- Search (git_refs_finder.rb:12-60): case-insensitive substring match (RE2-escaped, lowercase). If `search` contains `^`, `$` or `*` it is a regex-ish pattern: `^` and `$` anchors honored, `*` -> `.*?`. Without those operators, an exact (case-insens.) name match is moved to the first position.
- `regex`: RE2 (`Gitlab::UntrustedRegexp`) `match?` on name (unanchored). Invalid -> 400 `{"message":"Regex is invalid"}` (branches.rb:83).
- `search` and `regex` both given: search first, then regex filter.
- Pagination: normal offset pagination with `x-total`, `x-total-pages`, `x-per-page`, `x-page`, `x-next-page`, `x-prev-page`, `Link`. With `pagination=keyset` + feature flag `branch_list_keyset_pagination` the page_token path is used; token = last branch name; `Link: <...page_token=NAME&per_page=N>; rel="next"` only when `records.count == per_page`. First page w/o search/regex under the flag fetches via Gitaly with fabricated offset headers (gitaly_keyset_pager.rb:40-95).
- Response cached 30s per user (irrelevant).
- Empty repo: returns `[]` (200).

### GET /projects/:id/repository/branches/:branch (107-111)
200 Branch entity. Invalid refname -> 400 `{"message":"The branch refname is invalid"}` (helpers.rb:340-346; validation per section 7). Missing -> 404 `{"message":"404 Branch Not Found"}`.
### HEAD same path -> 204 if exists else 404 (no body).

### POST /projects/:id/repository/branches (195-209)
Params: `branch` (required, non-blank), `ref` (required, non-blank: branch name, tag or sha). Permission: `push_code` (Developer+) else 403 `"403 Forbidden"`. Success: **201** Branch entity (Grape default for POST).
Errors, all `400 {"message": ...}` (via `Branches::CreateService`, app/services/branches/create_service.rb + validate_new_service.rb):
- `"Branch name is invalid"` (GitRefValidator fails).
- `"Branch already exists"`.
- `"Ref is missing"` (blank ref - normally pre-empted by param validation `"ref is empty"`).
- `"Failed to create branch '<branch>': invalid reference name '<ref>'"` (ref not resolvable).
- `"Failed to create branch '<branch>': Branch name conflicts with existing branch hierarchy."` (D/F conflict e.g. `a` exists and creating `a/b`).
- PreReceiveError -> hook message.
Empty repository: `create_default_branch` first creates `README.md` (empty content, message `"Add README.md"`) on `default_branch_or_main` as the current user, then creates the branch from `ref` (note: only a blank `ref` is rejected before the README commit; the README commit is therefore made even if the later branch-name validation fails).
Created branch with name == a 40-hex sha: API service does not forbid, but pushes do (section 3.4).

### DELETE /projects/:id/repository/branches/:branch (220-235)
`push_code` required (403 otherwise). 404 `"404 Branch Not Found"`. `destroy_conditionally!` honors `If-Unmodified-Since` header against the tip commit's `authored_date` -> 412 `{"message":"412 Precondition Failed"}`. Success: **204**, empty body.
`Branches::DeleteService` errors -> `render_api_error!(msg, http_status)`: no push access `"You dont have push access to repo"` (405); `"No such branch"` (404); `"Failed to remove branch"` (400); PreReceiveError/CommandError message (400). **Protected branch**: deleting through API uses `rm_branch` which runs the pre-receive hook via Gitaly => internal allowed check => for a protected branch returns 400 with the git-access message (section 3.4: "You can only delete protected branches using the web interface." when the deleting user is Maintainer; "...Only a project maintainer or owner can delete a protected branch." for non-maintainers). The default branch -> `"The default branch of a project cannot be deleted."` (400). **[behaviour of API-originated rm_branch w.r.t. `updated_from_web?` is determined by gitaly_context; API ops are not "web", so expect the non_web message]**.

### DELETE /projects/:id/repository/merged_branches (243-248)
`push_code`. Enqueues `Branches::DeleteMergedService` async; **202** body `{"message":"202 Accepted"}` (`accepted!`, helpers.rb:652). The service skips default branch and protected branches (`ProtectedBranch.protected?`), deleting every branch in `merged_branch_names`.

### Legacy protect/unprotect
- `PUT /projects/:id/repository/branches/:branch/protect` params `developers_can_push`, `developers_can_merge` (Boolean, optional). Needs `admin_project` (Maintainer+) -> else 403. 404 `"404 Branch Not Found"` if branch missing. If a protected branch with the exact name exists: `LegacyApiUpdateService` (push/merge levels replaced only for params that are non-nil: true=>30, false=>40); else `LegacyApiCreateService`: push level 30 if developers_can_push else 40; merge level 30 if developers_can_merge else 40. Returns **200** Branch entity (PUT). Invalid -> 422 `{"message":[...full_messages]}`.
- `PUT .../unprotect` same permission; destroys the matching protected branch rule if one *exact-name* exists; returns 200 Branch entity (even if it was not protected).

---------------------------------------------------------------------------------------------------
## 3. Tags (lib/api/tags.rb)
`before`: `authorize_read_code!`, `not_found! unless user_project.repo_exists?` (404 `"404 Not Found"`).

### GET /projects/:id/repository/tags (tags.rb:44-72)
Params: `sort` (`asc|desc`, default `desc`), `order_by` (`name|updated|version`, default `updated`), `search`, `page_token`, `page`, `per_page`. Sort key sent to Gitaly: `"#{order_by}_#{sort}"`. `search` uses same finder semantics as branches (`^`,`$`,`*`); Gitaly pagination used for page 1 only when `search` blank. Errors: invalid page token 422 `{"message":"Invalid page token: refs/tags/<tok>"}`; Gitaly CommandError 503.
### GET .../tags/:tag_name -> Tag entity; 404 `"404 Tag Not Found"`.
### POST /projects/:id/repository/tags (tags.rb:99-116)
Params: `tag_name` (req), `ref` (req; commit sha or branch name), `message` (opt; **presence => annotated tag**; message is `strip`ped; empty string after strip => lightweight (`message&.strip` then `add_tag` treats blank as lightweight) **[unverified blank handling in Gitaly client]**). Permission `admin_tag` (= push_code, ProjectPolicy:573) else 403.
Success **201** Tag entity. Errors (Tags::CreateService, app/services/tags/create_service.rb): all via `render_api_error!(msg, 400)`:
- `"Target is empty"` (400 - ref blank)
- `"Tag name invalid"`
- `"Tag <name> already exists"` (service returns 409 internally but API always maps to **400**)
- `"Target <ref> is invalid"` (ref not found)
- `"Failed to create a tag: <name>"`
- PreReceiveError message (e.g. protected-tag rejections: `"You are not allowed to create this tag as it is protected."`).
### DELETE .../tags/:tag_name -> 204 (If-Unmodified-Since honored vs commit authored_date). 404 `"404 Tag Not Found"`; 403 if `delete_tag` denied; service error -> `render_api_error!(msg, return_code)` (`"Failed to remove tag"` 400, pre-receive message 400). Deleting a tag also destroys its Release.
### GET .../tags/:tag_name/signature -> 404 `"404 Signature Not Found"` if unsigned (out of scope).

---------------------------------------------------------------------------------------------------
## 4. Commits (lib/api/commits.rb)
`before`: `require_repository_enabled!`, `authorize_read_code!`, `verify_pagination_params!`. Commit routes use `API::COMMIT_ENDPOINT_REQUIREMENTS` (sha may contain `/`-encoded branch names, since `:sha` accepts ref names).

### GET /projects/:id/repository/commits (commits.rb:149-199)
Params: `ref_name` (default: `project.default_branch`), `since`, `until` (DateTime, ISO8601; `since`->Gitaly `after`, `until`->`before`), `path` (leading `/` stripped), `follow` (Boolean, follow renames when path set), `author` (substring, expanded with the matching users' emails), `all` (Boolean; when true `ref_name` ignored => `--all`), `with_stats`, `first_parent`, `order` (`default|topo`, default `default`), `trailers` (Boolean, default false), `page`, `per_page`.
- 404 `{"message":"404 Repository Not Found"}` if repository does not exist on disk.
- Offset/limit: `limit = min(per_page, 100)`, `offset=(page-1)*limit`; **no total headers**: pagination headers produced with `without_count` semantic: `x-next-page` is set iff returned count == per_page (kaminari fake total = offset+size+1). Header set: `x-per-page`, `x-page`, `x-next-page`, `x-prev-page`, `Link` (no `x-total`, `x-total-pages`).
- Unknown/invalid ref => Gitaly NotFound => `Repository#log` returns `[]` => **200 `[]`** (not 404). `ArgumentError` (e.g. invalid ref for Gitaly) -> 400 `{"message":"ref_name is invalid"}`.
- Empty repo (ref = nil/`main` from preferences, no commits) -> `[]`.
- Entities: `Commit`, or `CommitWithStats` when `with_stats=true` (adds `stats {additions,deletions,total}` after web_url).
- Commits sorted per git log default (reverse chronological, by commit date) ; `order=topo` => `--topo-order`; `first_parent` => `--first-parent`.

### GET /projects/:id/repository/commits/:sha (commits.rb:~290)
`sha` = sha, short sha, branch or tag name. Param `stats` (Boolean, default **true**). 404 `"404 Commit Not Found"`. Response CommitDetail (stats present unless `stats=false`).
Resolution: `Project#commit(sha)` -> `repository.commit(sha)` -> `find_commit`; short sha (>=7) allowed; refs that are both branch and tag: git rev-parse picks `refs/tags` before `refs/heads` (ambiguous ref warnings ignored) **[Gitaly behavior; unverified]**.

### GET .../commits/:sha/diff
Params `page`, `per_page`, `unidiff` (default false). `commit.diffs(expanded: true).diffs.to_a` paginated by Kaminari array (full pagination headers incl. `x-total`). Entity Diff. 404 `"404 Commit Not Found"`.
### GET .../commits/:sha/refs
Params `type` (`branch|tag|all`, default all), pagination w/o count. Entity BasicRef (`type`,`name`); order: branches then tags, each sorted by name.
### GET .../commits/:sha/merge_requests
Params `state`. Requires `read_merge_request` (403 otherwise). Entity MergeRequestBasic.
### GET/POST .../commits/:sha/comments (legacy commit notes)
GET -> notes array (CommitNote: `note, path, line, line_type, author, created_at`). POST params `note` (req), `path`, `line`, `line_type` (`new|old`, default new; `line` and `line_type` required when `path` given). Returns **201** CommitNote; line anchors only if line found in diff, else a plain commit note; `path/line` ignored when not matching. Errors via `process_note_creation_result`: 400 `{"message":"Note can't be blank"}` style.
### GET .../commits/:sha/sequence -> `{"count": N}`; `/signature` -> 404 `"404 Signature Not Found"` when unsigned.

### POST /projects/:id/repository/commits/:sha/cherry_pick  and /revert
Params: `branch` (req, non-blank), `dry_run` (default false), cherry_pick also `message`. Requires push access to branch: `authorize_push_to_branch!` => 401 if unauthenticated, else 403 `"403 Forbidden - You are not allowed to push into this branch"`. 404 `"404 Commit Not Found"`, 404 `"404 Branch Not Found"` (target branch must exist).
- Success cherry_pick/revert: **201** Commit entity of the new commit. `dry_run` true: **200** `{"dry_run":"success"}` (note `status :ok`).
- Failure: **400** `{"message": "...", "error_code": "conflict"|"empty"|null}`; with dry_run adds `"dry_run":"error"`. Messages (commits/change_service.rb): `"Commit cherry-pick failed: Can't perform this action automatically. It may have already been done, or a more recent commit may have updated some of this content. Please perform this action locally."` ("Commit revert failed: ..." for revert; "Merge request cherry-pick failed:"/"Merge request revert failed:" when the commit is a merge-request merge commit). Default cherry-pick commit message = `commit.cherry_pick_message` + `"\n\nCo-authored-by: <author name> <author email>"`; revert message = `Revert "<title>"\n\nThis reverts commit <sha>`. Authored by current user (name + `commit_email_or_default`).

### POST /projects/:id/repository/commits (create commit with actions) (commits.rb:~240-290)
**Transport**: in 18.11 the body is uploaded by Workhorse (`requires :file, WorkhorseFile`); `/authorize` pre-flight route `POST .../commits/authorize`. Rails reads the buffered body: JSON (`application/json` or missing type), `application/x-www-form-urlencoded` (nested query), or multipart; other media type -> 400 `"400 Bad request - Unsupported Content-Type: <type>"`. Invalid JSON -> 400 `{"message":"Invalid json","error":"Invalid json"}`. labrat, if it terminates HTTP directly, just parses the JSON body and keeps the validations below. Missing workhorse header -> 403 `"403 Forbidden - Request should be executed via GitLab Workhorse"` (skip).
Validation order (commits.rb `validate_commits_attrs!`), all 400 with prefix `400 Bad request - `:
1. `authenticate!` -> 401 `"401 Unauthorized"`; `branch` type check (`"branch must be a string"`).
2. `authorize_push_to_branch!(branch)` -> 403 `"403 Forbidden - You are not allowed to push into this branch"` (protected-branch aware).
3. String checks: `branch`, `commit_message`, `start_branch`, `start_sha`, `author_email`, `author_name` each `"<name> must be a string"`.
4. `"branch is required"`, `"commit_message is required"`.
5. actions missing/empty: unless `allow_empty` true -> `"Provide at least one action, or set allow_empty to true"`; bad `allow_empty` -> `"allow_empty must be a boolean"`.
6. `"actions must be an array"`.
7. `"start_branch, start_sha are mutually exclusive"` (if both keys present).
8. Per action `i`: `"actions[i][file_path] must be a string"` (also previous_path, last_commit_id), `"actions[i][action] is required"`, `"actions[i][file_path] is required"`, `"actions[i][action] must be one of: create, update, move, delete, chmod"`, `"actions[i][encoding] must be text or base64"` (default `text`), update: `"actions[i][content] is required for update action"`, move: `"actions[i][previous_path] is required for move action"`, chmod: `"actions[i][execute_filemode] is required for chmod action"` / `"... must be a boolean"`.
   (create requires no content key check here; missing content for create commits an empty/nil file? -> content nil.)
9. `start_project` (id or path): must be readable and equal project or a fork-network parent: 403 `"403 Forbidden - Project is not included in the fork network for <full_name>"`; unknown -> 404.
Defaults: `start_branch` defaults to `branch` (unless `start_sha` given); `stats` default **true** (`ActiveModel` cast != false); `force` default false. Unknown top-level keys are dropped.
Service validations (`Commits::CreateService#validate!`, app/services/commits/create_service.rb:64-114), result `400 {"message": ...}`:
- `"You are not allowed to push into this branch"`
- `"You can't pass both start_branch and start_sha"`, `"Invalid start_sha '<sha>'"` (not a 40/64-hex), `"Cannot find start_sha '<sha>'"`
- `"You can only create or edit files when you are on a branch"` (start_branch missing in non-empty repo)
- Branch exists + different start + not force: `"A branch called '<b>' already exists. Switch to that branch in order to make changes"`
- New-branch name validation: `"Something went wrong when we tried to create '<b>' for you: Branch name is invalid"` / `"...: Branch already exists"` (only evaluated for empty repo or when start differs).
- `"You must provide a commit message"`.
- `Files::MultiService#validate_file_status!` (update/move/delete/chmod w/ `last_commit_id`): `"The file has changed since you started editing it: <path>"` (compares last commit touching path on start_branch vs. on `last_commit_id`; path = `previous_path || file_path`).
- Index errors from Gitaly (`Gitlab::Git::Index::IndexError`), exact strings (gitaly_client/operation_service.rb:723-742): `"You must provide a file path"`, `"invalid path: '<path>'"`, `"A directory with this name already exists"`, `"Path cannot include directory traversal"`, `"A file with this name already exists"`, `"A file with this name doesn't exist"`, `"Unknown error performing git operation"`; `"Could not update refs/heads/<b>. Please refresh and try again."` (CommitError).
Semantics per action: `create` (fails if path exists as file => "A file with this name already exists"; if path is a dir => "A directory with this name already exists"), `update` (path must exist; `content` replaces), `delete`, `move` (`previous_path` -> `file_path`; `content` omitted/nil => keep previous content (`infer_content`), `content: ""` => empty file), `chmod` (`execute_filemode` true => 100755 else 100644). LFS pointers transform applies on `create` when project LFS enabled and `.gitattributes` says so.
Commit author: `author_name`/`author_email` params, else user's name / commit email (namespace commit email, else user's `commit_email`). Committer = current user.
Success: **201** CommitDetail (with `stats` when `stats` true). `status` null, `last_pipeline` null at that point.
`force=true`: allows resetting an existing different branch to start point (branch exists check skipped; Gitaly `force`). Branch protection still enforced via Gitaly pre-receive.

---------------------------------------------------------------------------------------------------
## 5. Repository files (lib/api/files.rb)
`FILE_ENDPOINT_REQUIREMENTS`: `file_path` is `NO_SLASH_URL_PART_REGEX` (so URL-encode `/` as `%2F`; param decoded). Every route does `content_type "application/json"` (even `.txt`).
`file_path: true` validator rejects invalid paths (traversal etc.) -> 400 `{"error":"file_path should be a valid file path"}`.

### GET /projects/:id/repository/files/:file_path  (files.rb:~215)
Params `ref` **required** (branch, tag or sha; blank -> 400 `{"error":"ref is empty"}` / missing `"ref is missing"`). Flow (`assign_file_vars!`): `authorize_read_code!`; `user_project.commit(ref)` else 404 `"404 Commit Not Found"`; blob lookup else 404 `"404 File Not Found"`.
Response JSON exact key order (`blob_data` + `content`):
`file_name, file_path, size, encoding ("base64"), content_sha256, ref, blob_id, commit_id, last_commit_id, execute_filemode, content`
- `file_name` = basename, `file_path` = path as in tree, `size` bytes, `content` = `Base64.strict_encode64(data)`, `content_sha256` = lowercase hex sha256 of raw bytes, `ref` echoes the request param verbatim, `blob_id` git blob sha, `commit_id` = sha the ref resolved to, `last_commit_id` = last commit touching the path on that commit (`last_commit_id_for_path(sha, path, literal_pathspec: true)`), `execute_filemode` = blob mode == 100755. LFS pointer files return pointer content (not resolved).
- Same data is sent as response headers (see HEAD). Blob size rate-limit via `check_rate_limit_for_blob` (429 on abuse).
### HEAD same path: headers only, body empty, 200. Headers built by `set_http_headers` (headers_helpers.rb): each key `foo_bar` -> `X-Gitlab-Foo-Bar`:
`X-Gitlab-File-Name, X-Gitlab-File-Path, X-Gitlab-Size, X-Gitlab-Encoding (base64), X-Gitlab-Content-Sha256, X-Gitlab-Ref, X-Gitlab-Blob-Id, X-Gitlab-Commit-Id, X-Gitlab-Last-Commit-Id, X-Gitlab-Execute-Filemode (true/false)`. Missing -> 404 with no body (HEAD).
### GET .../files/:file_path/raw
`ref` optional here (defaults to default branch/HEAD via `user_project.commit(nil)`... param declared optional; if omitted `params[:ref]` nil -> `commit(nil)` -> HEAD of default branch **[assign_file_vars! uses params[:ref]; nil => repository.commit(nil) => root_ref]**), `lfs` (Boolean, default false: with true and LFS pointer, serves LFS object). Response: raw bytes (Workhorse sends via Gitaly), `Content-Type` sniffed (text/plain default), `Content-Disposition: inline; filename="<name>"`, plus the same `X-Gitlab-*` headers and no-cache headers (`Cache-Control: max-age=0, private, must-revalidate`, `Pragma: no-cache`, `Expires: Fri, 01 Jan 1990 00:00:00 GMT`) **[no_cache_headers exact values from Gitlab::NoCacheHeaders; unverified]**.
### GET .../files/:file_path/blame (and HEAD)
Params `ref` (required), `range[start]`,`range[end]` (ints >= 1; `"range[start] must be less than or equal to range[end]"` 400). Response array of ranges `[{commit: Commit-ish {id, parent_ids, message, authored_date, author_name, author_email, committed_date, committer_name, committer_email}, lines: [..]}]` (Entities::BlameRange). Out of scope detail.

### POST /projects/:id/repository/files/:file_path (create) / PUT (update) (files.rb:~262-330)
Body via Workhorse (same transport notes as commits). Authorize routes `POST|PUT .../files/:file_path/authorize`.
Body fields: `branch` (req), `commit_message` (req), `content` (req key; may be ""), `start_branch`, `author_email`, `author_name`, `encoding` (`text` default | `base64`), `last_commit_id`, `execute_filemode` (bool).
Validation order (400, message prefix `400 Bad request - `): `branch/commit_message/start_branch/author_*/last_commit_id must be a string` -> `"branch is required"` -> `"commit_message is required"` -> `"content is required"` (key absent) -> `"encoding must be text or base64"` -> `"execute_filemode must be a boolean"`.
Then `authenticate!` (401) happened first; then `authorize_push_to_branch!` => 403 `"403 Forbidden - You are not allowed to push into this branch"`.
- start_branch defaults to branch (`commit_params`). Creating on a new branch: pass `branch` new + `start_branch` existing.
- **Create success: 201** `{"file_path":"<path>","branch":"<branch>"}`. **Update success: 200** same shape.
- Create errors (`render_api_error!(msg, 400)`): `"A file with this name already exists"`, `"A directory with this name already exists"`, `"You must provide a file path"`, `"invalid path: '<path>'"`, `"Path cannot include directory traversal"`, `"You can only create or edit files when you are on a branch"`, `"A branch called '<b>' already exists. Switch to that branch in order to make changes"`, `"Something went wrong when we tried to create '<b>' for you: Branch name is invalid"`, `"You must provide a commit message"`.
- Update errors: `"A file with this name doesn't exist"` (path missing), and **`"You are attempting to update a file that has changed since you started editing it."`** (400) when `last_commit_id` is supplied and differs from the last commit touching the file on `start_branch` (files/update_service.rb; `file_has_changed?` returns false if either lookup is nil). Update with identical content still creates a commit.
- Create/Update LFS transformer + `execute_filemode` (true => mode 100755; omitted => keep existing / 100644).
- Empty repository: create file with `branch` = default branch name works (start_branch validation skipped when repo empty) and becomes the first commit (initial push semantics section 6.5; default-branch protection hooks run: `after_create_default_branch`).

### DELETE /projects/:id/repository/files/:file_path
Query/form (not Workhorse) params: `branch` (req, non-blank), `commit_message` (req, non-blank), `start_branch`, `author_email`, `author_name`, `last_commit_id`. `authorize! :push_code` (403 `"403 Forbidden"`). Success **204** empty. Errors 400 `render_api_error!`: `"You are attempting to delete a file that has been previously updated."` (last_commit_id mismatch), `"A file with this name doesn't exist"`, `"You are not allowed to push into this branch"` (from service validate_permissions!), etc. (Note: delete path does NOT use `authorize_push_to_branch!`, so protected-branch denial surfaces as 400 `"You are not allowed to push into this branch"`, not 403.)

---------------------------------------------------------------------------------------------------
## 6. Repository (lib/api/repositories.rb)
`before { authorize_read_code! }`.

### GET /projects/:id/repository/tree (repositories.rb:~100-125)
Params: `ref` (default default-branch), `path`, `recursive` (bool, default false), `pagination` (`legacy|keyset|none`, default legacy; `none` only valid with `recursive=true` else 400 `{"error":"pagination cannot be \"none\" unless \"recursive\" is true"}`), `page_token` (with keyset), `page`, `per_page`.
- Commit lookup: `project.commit(ref || default_branch)`; missing -> 404 `{"message":"404 Tree Not Found"}` (this is the empty-repo outcome too).
- Path not in tree: Gitaly returns IndexError for resolve failure -> 404 `{"message":"<gitaly message>"}` (the API passes `rescue_not_found: false`); path-validation errors give `"You must provide a file path"` / `"Path cannot include traversal syntax"` / `"Only relative path is accepted"` / `"Path is too long"` (commit_service.rb:752). **A non-existent but well-formed path under a valid ref yields 200 `[]`** in practice (Gitaly returns empty for missing subtree) **[unverified; the 404-with-message branch is only for IndexError]**.
- Order of entries: trees first, then blobs, then submodule commits (`Tree#sorted_entries`, models/tree.rb:80); within each group Gitaly order (byte-wise by name; for recursive: depth-first by path).
- Non-recursive entry `path` is full path from repo root (`files/html`), `name` basename.
- Legacy pagination: offset on the in-memory array; page 1 uses Gitaly pagination + `x-total` computed from full tree size (gitaly_keyset_pager.rb). Headers: `x-total`, `x-total-pages`, `x-per-page`, `x-page`, `x-next-page`, `x-prev-page`, `Link`.
- Keyset (`pagination=keyset`): `page_token` = tree entry **oid**; response `Link: <...&page_token=<last oid>>; rel="next"` only if `records.count == per_page`; no totals.
- `pagination=none`+`recursive=true`: entire tree, no headers.
- Entity TreeObject.

### GET /projects/:id/repository/blobs/:sha (+ /raw)
`sha` is blob object id (not a ref). Not a blob / missing -> 404 `"404 Blob Not Found"`. JSON: `{"size": N, "encoding":"base64", "content":"<b64>", "sha":"<blob sha>"}` (key order: size, encoding, content, sha). `/raw`: raw bytes with `Content-Disposition: inline; filename="<blob name>"`... (name likely empty for sha-only lookup), no-cache headers.

### GET /projects/:id/repository/compare (repositories.rb:~245-275)
Params: `from` (req), `to` (req) (branch/tag/sha), `from_project_id` (Integer), `straight` (Boolean, default false), `unidiff` (default false).
- `from_project_id` not a project the user can use as MR target of this project: 400 `{"message":"Target project id:<id> is not a fork of project id:<:id param>"}`; project lacks read_code: 403 `"403 Forbidden - You don't have access to this fork's parent project"`.
- Semantics: base = `from`, head = `to`. Non-straight (`from...to`): diff from merge-base(from,to) to `to`; commits = `rev-list from..to` (commits reachable from `to` but not `from`). Straight (`from..to`): diff `from` -> `to`.
- Unknown ref on either side: `Gitlab::Git::Compare` yields null base/head => service returns nil => 404 `{"message":"404 Ref Not Found"}`.
- Same ref (base.id == head.id): 200 with `commit:null, commits:[], diffs:[], compare_timeout:false, compare_same_ref:true`.
- Diffs: collection built with `Compare` defaults: `max_files=1000`, `max_lines=50000`, `expanded:false`, `ignore_whitespace_change:false` => section 7 limits; BUT entity returns `diffs.diffs.to_a` raw Gitlab::Git diffs (collapsed possible since `expanded:false` — see section 7).
- Response cached 1 minute.

### GET /projects/:id/repository/merge_base
Param `refs[]` (array; also comma-separated). `< 2` refs -> 400 `{"message":"Provide at least 2 refs"}`; unknown -> 400 `"Could not find ref: x"` / `"Could not find refs: x, y"`; none -> 404 `"404 Merge Base Not Found"`; success Commit entity (merge base of all refs).
### GET /projects/:id/repository/contributors
Params `ref`, `order_by` (`commits|name|email`, default `commits`), `sort` (`asc|desc`, default `asc`), pagination. Computed from the first 2000 non-merge commits of ref (`Repository#contributors`, repository.rb:829) grouped by author email. Errors -> 404 `"404 Not Found"`.
### GET /projects/:id/repository/archive[.format]
Params `sha`, `format` (`tar.gz` default, `zip`, `tar.bz2`, `tar`, ...), `path`, `ref_type`, `include_lfs_blobs`, `exclude_paths`. Streamed by Workhorse from Gitaly; filename `<project-path>-<ref>-<sha>.<ext>`-ish (`append_sha: true`). Errors -> 404 `"404 File Not Found"`; rate limit 429.
### GET /projects/:id/repository/health, /changelog - out of scope.

---------------------------------------------------------------------------------------------------
## 7. Diff generation details

### Limits (all from ApplicationSetting, db/structure.sql:14486,14636-14637; models/commit.rb:36-41,97-110)
- `diff_max_patch_bytes` = 204800 (200 KiB) hard per-file; upper bound 500 KiB (git/diff.rb:29,33).
- `diff_max_files` = 1000 (upper bound 3000), `diff_max_lines` = 50000 (upper bound 100000).
- Safe limits (collapse thresholds) = max/10: `safe_max_files` = 100, `safe_max_lines` = 5000, `safe_max_bytes` = safe_max_files * 5 KiB = 512000 (git/diff_collection.rb:21-35); `max_bytes` = max_files * 5 KiB = 5,120,000.
- Per-file: `too_large?` when patch bytes >= 204800 (hard) -> patch pruned to `""` and `too_large:true`; `collapsed?` when `!expanded && bytes >= 20480` (hard/10) -> patch pruned to `""`, `collapsed:true` (git/diff.rb:221-250). Generated files (`generated_file`) collapse when not expanded.
- Collection overflow (diff_collection.rb:204-240): stop adding files when `i >= max_files`, or accumulated line count >= `max_lines`, or bytes >= `max_bytes` => `overflow? = true`; remaining files are omitted (Compare entity `compare_timeout: true`). The overflowing file that crosses the limit is discarded (not returned).
- Over *safe* limits (file index >= 100, lines > 5000, bytes >= 512000) and diff not expanded -> subsequent diffs are collapsed (patch `""`, `collapsed:true`).
- `expanded`: `Commit#diffs(expanded: true)` is used by the commit diff API => no collapsing for size, only the hard `too_large`/overflow limits. Compare API passes defaults (`expanded:false`) => collapsing applies at 20 KiB per file and safe limits; a single-file iterator (`@iterator.size == 1`) is auto-expanded (expand_diff?, diff_collection.rb:163-172). Binary files: expand only if `!enforce_limits || expanded`.
- Gitaly performs the actual cut (request carries `enforce_limits`, `collapse_diffs = !expanded`, `max_files`, `max_lines`, `max_bytes`, `safe_max_*`, `max_patch_bytes`; commit_service.rb:661-673). labrat must reproduce Gitaly's behavior.

### Format of `diff`
- String begins at the first hunk header `@@ -a,b +c,d @@ <funcname context>`; **no** `diff --git`, `index`, `---`/`+++` header lines (those are stripped by Gitaly). Lines end with `\n`; "\ No newline at end of file" markers retained.
- Pure rename with no content change: `diff` = `""`, `renamed_file:true`, `old_path != new_path`.
- Mode-only change: `diff` = `""` and `a_mode != b_mode`.
- Empty new file: `diff` = `""`, `new_file:true`.
- `unidiff=true` (git/diff.rb:203-211): non-binary non-empty diff is prefixed with `"--- a/<old_path>\n+++ b/<new_path>\n"` (`/dev/null` for `new_file` old side / `deleted_file` new side).
- Binary: Gitaly yields a patch beginning `Binary files a/x and b/x differ`; GitLab's own `json_safe_diff` (when charlock detects binary content in `@diff`) replaces with `"Binary files <old_path> and <new_path> differ\n"` (paths WITHOUT `a/`/`b/` prefixes; git/diff.rb:129-131). Treat: for binary files emit `"Binary files #{old_path} and #{new_path} differ\n"` (for new files old_path == new_path in GitLab's record? old_path is `to_path`-equal for adds: Gitaly sets both paths to the file path).
- Non-UTF-8 content is transcoded with replacement characters (`encode_utf8_with_replacement_character`).
### Git options used by Gitaly **[Gitaly source not in tree; from knowledge of Gitaly's `git diff` invocation, treat as unverified]**
`--patch --raw --abbrev=40 --full-index --find-renames` (rename detection on, default 50% similarity; no copy detection), context 3 lines (git default), default diff algorithm (myers; no `--patience`/`--histogram`; `diff.indentHeuristic` default true in modern git), no whitespace options unless `ignore_whitespace_change` (`--ignore-space-change`), `-z`-style raw parse. `ignore_whitespace_change` is not exposed by the repository API endpoints in this file.
### MR diff refs (merge_request.rb:1243-1308)
- `head_sha` = source branch tip; `start_sha` = target branch tip at diff creation (`merge_request_diff.start_commit_sha`); `base_sha` = merge base(target tip, source tip) (`target_project.merge_base_commit`). MR diff content = diff `base_sha`..`head_sha` (three-dot). For compare API: `base_sha = straight ? start : merge_base` (compare.rb:98-103).
- Each `MergeRequestDiff` ("version") stores head/base/start commit shas, commits and files; `real_size`, `state` in `collected|overflow|without_files|empty|...`.

---------------------------------------------------------------------------------------------------
## 8. Protected branches API (lib/api/protected_branches.rb)

### Entity
`id, name, push_access_levels[], merge_access_levels[], allow_force_push`
Access level object (entities/protected_ref_access.rb): `id, access_level, access_level_description, deploy_key_id (push levels only, emitted only if the record has that attribute, so merge levels omit it)`.
- `access_level_description`: `30 -> "Developers + Maintainers"`, `40 -> "Maintainers"`, `60 -> "Instance admins"`, `0 -> "No one"` (concerns/protected_ref_access.rb:`human_access_levels`). Deploy key level: description = deploy key title.

### GET /projects/:id/protected_branches
Params `search` (fuzzy `ILIKE` name match; lowercased), pagination. Includes group-inherited rules (`all_protected_branches`), limit 100 (ProtectedBranchesFinder::LIMIT). `authorize_read_code!` -> 403. Order: DB default (by id? finder has no explicit order) **[unspecified; sort by id asc]**.
### GET /projects/:id/protected_branches/:name -> 404 `{"message":"404 Not found"}` (find_by! -> ActiveRecord::RecordNotFound rescued globally as `404 Not Found`). **Only project-level rules** (`user_project.protected_branches`), exact name (wildcard names addressed literally, e.g. `release%2F*`).
### POST /projects/:id/protected_branches
Params: `name` (req), `push_access_level` (Integer in {0,30,40,60}; **default 40**), `merge_access_level` (same; default 40), `allow_force_push` (Boolean, default **false**), `allowed_to_push` (Array of JSON; only `deploy_key_id` entries are declared, lib/api/helpers/protected_branches_helpers.rb).
- Valid level values: `ProtectedRefAccess.allowed_access_levels` = `[30,40,60,0]` (60 excluded only on GitLab.com). Other value -> 400 `{"error":"push_access_level does not have a valid value"}`.
- Permission `:create_protected_branch` (Maintainer+) else 403 `"403 Forbidden"`. Duplicate name (project rule exact) -> **409** `{"message":"Protected branch '<name>' already exists"}`.
- Success **201** ProtectedBranch entity. Validation failure -> 422 `{"message":["Name can't be blank", ...]}` (array of full messages).
- Name may be a wildcard (`*`), see matching rules below.
### PATCH /projects/:id/protected_branches/:name
Params `allow_force_push` (Boolean, allow_blank false), `allowed_to_push` (Array of JSON, `deploy_key_id` entries only); `push_access_level`/`merge_access_level` are NOT accepted. 404 if rule missing; 403 via `update_protected_branch`. Returns **200** entity.
### DELETE /projects/:id/protected_branches/:name -> **204**; 404 if rule missing; 403 `destroy_protected_branch`. Honors If-Unmodified-Since vs `updated_at`.
Wildcards: `RefMatcher` (models/ref_matcher.rb): `*` matches any chars including `/` (non-greedy `.*?`, anchored `\A...\z`), all other chars literal (Regexp.quote). Exact name also matches. Matching is case-sensitive.
Name `HEAD`/invalid names are accepted (only presence validated; uniqueness scope project/namespace).

### Default branch protection (applies when the default branch is first created)
Settings source: user-namespace projects -> `Gitlab::CurrentSettings.default_branch_protection_defaults`; group projects -> `group.default_branch_protection_defaults` if non-blank else instance setting (namespace.rb:494-502). Instance default from config/initializers/1_settings.rb:189-190 = `protected_fully`:
```
{allowed_to_push:[{access_level:40}], allowed_to_merge:[{access_level:40}], allow_force_push:false, developer_can_initial_push:false}
```
(legacy int levels in lib/gitlab/access.rb:24-28: 0 none, 1 dev-can-push, 2 full, 3 dev-can-merge, 4 dev-can-initial-push; Access::BranchProtection#to_hash gives): none = push[30],merge[30],force true; partial(1) push[30],merge[40]; full(2) push[40],merge[40]; dev_can_merge(3) push[40],merge[30]; initial_push(4) push[40],merge[40],developer_can_initial_push true.
`Projects::ProtectDefaultBranchService` (called by `Project#after_create_default_branch` upon first push / first branch creation / file create on empty repo, via BranchHooksService#branch_create_hooks when `default_branch?`):
1. `project.change_head(default_branch)` (HEAD symref -> pushed default branch).
2. If `DefaultBranchProtection#any?` (true unless settings are fully permissive: `return true unless allow_force_push`; i.e. force-push allowed AND all levels developer => false) and branch not already protected => create ProtectedBranch(name=default_branch, push level, merge level, allow_force_push) with creator `project.creator` skipping auth.
   - push level: any NO_ACCESS entry => 0; else any DEVELOPER => 30; else any MAINTAINER => 40; else 60. merge level likewise.
- `Project#default_branch_protected?` = `!developer_can_push?` (project.rb:3384) - used by `ProtectedBranch.protected?` while the repo is empty (a branch is "protected" before it exists if developers cannot push).
- **developer_can_initial_push** (lib/gitlab/access/default_branch_protection.rb:67-79, protected_branch.rb `protected_ref_accessible_to?`): while repo is empty, a Developer (>=30) may push the very first branch when `developer_can_push?` OR `developer_can_initial_push?`; Maintainer+ always; admins/`admin_project` always. After the first branch exists the normal protected-branch rule (push level 40) applies.

### How protection affects access (`Gitlab::UserAccess`, lib/gitlab/user_access.rb)
Role numbers: Guest 10, Reporter 20, Developer 30, Maintainer 40, Owner 50, Admin 60 (instance admin), 0 No access.
- `can_push_to_branch?(ref)` = user can access git AND (has `push_code` OR branch allows collaboration (fork MR "allow collaboration")) AND (branch not protected OR a *matching* rule's push access level accepts user). Role level check: `user.can?(:push_code, project) && project.team.max_member_access(user.id) >= access_level`; level 60 => `user.admin?`; level 0 => nobody; deploy keys with push level for keys.
- `can_merge_to_branch?(ref)` = protected ? merge level accepted : `push_code`.
- `can_delete_branch?(ref)` = protected ? `can?(:destroy_protected_branch)` (Maintainer+) : push_code.
- Matching uses **any** matching rule (several wildcard rules may match; a level passes if *any* access level of *any* matching rule accepts user: `access_levels_for_ref(...).any?`).
- Protected-branch rules also inherited from the root group (`all_protected_branches`).
- Policy: `ProtectedBranchPolicy` delegates to project/group; CE abilities `create/update/destroy_protected_branch` = `admin_project` (Maintainer).

---------------------------------------------------------------------------------------------------
## 9. Git smart HTTP

### Routes (config/routes/git_http.rb)
Matched by regex `repository_git_route_regex` = `<full_namespace_route>\.*\.git` mounted at `*repository_path`, `format: false`:
- `GET  /<ns>/<proj>.git/info/refs?service=git-upload-pack|git-receive-pack`
- `POST /<ns>/<proj>.git/git-upload-pack`
- `POST /<ns>/<proj>.git/git-receive-pack`
- `POST .../ssh-upload-pack|ssh-receive-pack` -> 404 `"Not found"` plain.
- LFS: `POST .../info/lfs/objects/batch`, `info/lfs/locks...`, `gitlab-lfs/objects/:oid[/:size[/authorize]]` (out of scope).
- **Without `.git`**: only `GET /<ns>/<proj>/info/refs` with query blank, `service=git-upload-pack` or `service=git-receive-pack` and a path that is a valid project/wiki/snippet path => **302 redirect** to `/<ns>/<proj>.git/info/refs[?query]` (git_http.rb:58-68, Repositories::RepositoryRedirectUrlConstraint). Other endpoints without `.git` are NOT served by git http (they hit the web routes).
- `/<ns>/<proj>.wiki.git/` (GET `/`) redirects to `/<ns>/<proj>/-/wikis`.
- `HEAD info/refs` -> 403 empty (deny_head_requests).
- Moved/renamed projects: `Gitlab::RepoPath.parse` follows redirects (`follow_redirects: true`); access succeeds and `redirected_path` yields the post-push console message "Project '<old>' was moved to '<new>'.\nPlease update your Git remote:\n  git remote set-url origin <url>" (container_moved.rb).
- Case-insensitive path lookup (`Project.find_by_full_path`).
`Content-Type` of info/refs: handled by Workhorse: `application/x-git-upload-pack-advertisement`/`...receive-pack-advertisement`, pack POSTs `application/x-git-*-result`.

### Authentication (app/controllers/repositories/git_http_client_controller.rb:33-70)
Order: `authenticate_user` -> `parse_repo_path`.
1. If `Authorization: Basic` header present -> `Gitlab::Auth.find_for_git_client(login, password, project:, request:)`. Result success => proceed as that actor.
2. else (no credentials) if downloading (`upload_pack?`) and project readable by anonymous (`::Users::Anonymous.can?(:download_code, container)` i.e. public project with repo feature public) and HTTP protocol allowed => anonymous download, abilities `[:download_code]`.
3. Otherwise: respond **401** with header `WWW-Authenticate: Basic realm="GitLab"` and body (plain text):
`HTTP Basic: Access denied. If a password was provided for Git authentication, the password was incorrect or you're required to use a token instead of a password. If a token was provided, it was either incorrect, expired, or improperly scoped. See https://<host>/help/topics/git/troubleshooting_git#error-on-git-fetch-http-basic-access-denied`
(Same 401 body is sent when basic creds supplied but invalid, or on `MissingPersonalAccessTokenError`. Help URL = `help_page_url('topics/git/troubleshooting_git.md', anchor: 'error-on-git-fetch-http-basic-access-denied')` -> `<gitlab_url>/help/topics/git/troubleshooting_git#error-on-git-fetch-http-basic-access-denied`.)
**Important**: nonexistent/private project WITHOUT credentials => 401 (challenge), never 404 — prevents enumeration. With *valid* credentials but no access => **404** from access check (`NotFoundError`), plain text body `The project you were looking for could not be found or you don't have permission to view it.`

`Gitlab::Auth.find_for_git_client` checks in order (lib/gitlab/auth.rb:121-149), first non-nil wins:
1. `service_request_check` (login `<service>-ci-token` for CI integrations);
2. `build_access_token_check`: login must equal `gitlab-ci-token`, password = CI job token;
3. `lfs_token_check` (only for LFS requests; login = username or `lfs+deploy-key-<id>`);
4. `oauth_access_token_check`: password is an OAuth access token (Doorkeeper) — **login ignored** (so `oauth2:<token>` works; any login works);
5. `personal_access_token_check`: password is an active PAT (state active, not revoked/expired, scope within all available scopes, user not blocked, user can log in with non-expired password) — **username is not checked at all: any non-empty username (e.g. `oauth2`, `git`, the real username) works**; blank username with basic auth: `has_basic_credentials?` requires the header parse; a PAT with empty login still works. Project/service-account bots additionally need read access to project. Abilities from scopes: `api` -> full (download_code, push_code, ...), `read_api` -> read-only, `read_repository` -> `[download_code]`, `write_repository` -> `[download_code, push_code]`, `ai_workflows` -> push+download.
6. `deploy_token_check`: login MUST equal the deploy token username (`gitlab+deploy-token-N` or custom), password = token; must have access to project and repo feature not disabled; scopes `read_repository`/`write_repository`.
7. `user_with_password_for_git`: username/email + password. Skipped if password looks like a GitLab token (`Authn::AgnosticTokenIdentifier.token?`). Fails (raises MissingPersonalAccessTokenError => 401) when the user has 2FA enabled or 2FA enforced. Disabled if `password_authentication_enabled_for_git?` false.
Rate-limit: failed attempts per IP -> `Gitlab::Auth::IpBlocked` => 403 ("Your IP has been banned" ) **[skip]**.
Blocked/inactive user: `check_active_user!` -> 403 plain text via `UserAccessDeniedReason#rejection_message` (e.g. `Your account has been blocked.`).
Token scope lacking: `check_legacy_authentication_abilities!` -> 403 `You are not allowed to download code.` (`auth_download`) / `You are not allowed to upload code.` (`auth_upload`).

### Access check (Gitlab::GitAccess#check, lib/gitlab/git_access.rb:81-104)
Sequence and failure responses (HTTP status of `GitHttpController`: ForbiddenError -> 403, NotFoundError -> 404, CreationError -> 422, TimeoutError -> 503, always `text/plain` body = message):
1. protocol allowed (`Git access over HTTP is not allowed` 403)
2. valid actor, active user, token abilities (above)
3. `check_command_disabled!`: `Pulling over HTTP is not allowed.` / `Pushing over HTTP is not allowed.` (gitlab.yml `gitlab_shell.upload_pack|receive_pack` false)
4. `check_command_existence!`: `The command you're trying to execute is not allowed.`
5. `check_db_accessibility!` (push): `You can't push code to a read-only GitLab instance.`
6. `check_container!`: container nil => 404 `The project you were looking for could not be found or you don't have permission to view it.`; `check_project_accessibility!` => 404 same msg (also for private projects the user can't see); for job tokens a 403 `Insufficient permissions to pull from the repository of project #<id>.` or `Authentication by CI/CD job token not allowed from <path> to project #<id>.`
   - GitAccessProject: namespace missing => 404 `The namespace you were looking for could not be found.`; push-to-create projects (`ensure_project_on_push!`): if receive-pack, changes `_any`, abilities include push_code and user can create projects in the namespace -> project is auto-created **private** with path from URL; failure => 422 `Could not create project: <errors>`; message `The private project <path> was successfully created.\nTo configure the remote, run:\n  git remote add origin <url>\nTo view the project, visit:\n  <project_url>` printed after the push.
7. `check_repository_existence!`: 404 `A repository for this project does not exist yet.`
8. download: `check_download_access!` => 403 `You are not allowed to download code from this project.` ; push: `check_push_access!`:
   - repo read-only (`repository_read_only?`): 403 `The repository is temporarily read-only. Please try again later.`
   - archived (self or ancestor): 403 `You can't push code to an archived project.`
   - no user / non-pushing deploy key: 403 `You are not allowed to upload code for this project.` / `This deploy key does not have write access to this project.`
   - `changes == '_any'` (info/refs stage): user must have `push_code` ability (Developer+ via `push_ability :push_code`), CI build push ability, or any_branch_allows_collaboration; else 403 **`You are not allowed to push code to this project.`**
   - (Reporter/Guest: for info/refs?service=git-receive-pack they get this 403 at the *start* of `git push`: git prints `remote: You are not allowed to push code to this project.` + `fatal: unable to access '...': The requested URL returned error: 403`.)
Per-ref checks happen later, when Gitaly's pre-receive hook calls `POST /api/v4/internal/allowed` (workhorse/Gitaly -> Rails) with `changes="<old> <new> <ref>\n..."`. Failure -> JSON `{"status":false,"message":"<msg>"}` with HTTP 401 (internal_helpers.rb:33-49); git client sees `remote: GitLab: <msg>` + `! [remote rejected] <branch> -> <branch> (pre-receive hook declined)`.
   (labrat implementing pre-receive natively: emit the identical `<msg>` text; the `GitLab: <msg>`; git shows it as `remote: GitLab: <msg>`; each line of a multi-line message gets the `remote: ` prefix) **[hook output format is in gitlab-shell/Gitaly, unverified]**.

### Per-ref checks (Gitlab::Checks::SingleChangeAccess, order: PushCheck, BranchCheck, TagCheck, then CommitsCheck/DiffCheck; bulk: LfsCheck, FileSizeLimitCheck, IntegrationsCheck)
Exact messages:
- PushCheck (checks/push_check.rb): user lacks `push_code` (and no collaboration branch) => `You are not allowed to push code to this project.`
- BranchCheck (checks/branch_check.rb:6-18; only for `refs/heads/*`):
  - deleting default branch: `The default branch of a project cannot be deleted.`
  - (not deletion) branch name matches `\A<40 hex>` (SHA-1; also 64-hex sha256) : `You cannot create a branch with a SHA-1 or SHA-256 branch name.`
  - (not deletion) invalid ref name per GitRefValidator: `You cannot create a branch with an invalid name.`
  - if protected (`ProtectedBranch.protected?`):
    - force push (old and new non-blank and `!ancestor?(old,new)`) and rule's `allow_force_push` false for all matching rules: `You are not allowed to force push code to a protected branch on this project.`
    - empty repo OR updating: `protected_branch_push_checks`: if a *locked* MR's `in_progress_merge_commit_sha == newrev` for that target branch (merge via UI in progress) then needs merge OR push access else `You are not allowed to merge code into protected branches on this project.`; otherwise needs `can_push_to_branch?`, else `You are not allowed to push code to protected branches on this project.` -- except on an empty repo it is the multi-line:
      ```
      A default branch (e.g. main) does not yet exist for <project full_path>
      Ask a project Owner or Maintainer to create a default branch:
        <project members url>
      ```
    - creation (old = zeros) of protected branch: allowed if `can_push_to_branch?`; else if not `can_merge_to_branch?`: `You are not allowed to create protected branches on this project.`; if merge-able but base commit not on an existing protected branch: `You can only use an existing protected branch ref as the basis of a new protected branch.`; if not updated from web: `You can only create protected branches using the web interface and API.`
    - deletion: not `can_delete_branch?` (non-Maintainer): `You are not allowed to delete protected branches from this project. Only a project maintainer or owner can delete a protected branch.`; Maintainer but not via web UI: `You can only delete protected branches using the web interface.`
  - Unprotected branch: no further check (Developer+ can push, create, delete, force-push).
- TagCheck (checks/tag_check.rb:6-17) for `refs/tags/*`: existing tag changed by non-`admin_tag`: `You are not allowed to change existing tags on this project.`; creating tag with default-branch name: `You cannot use default branch name to create a tag`; prohibited name (invalid ref or starts with `refs/tags/`): `You cannot create a tag with a prohibited pattern.`; sha-like: `You cannot create a tag with a SHA-1 or SHA-256 tag name.`; protected tag rules: update `Protected tags cannot be updated.`, delete `You are not allowed to delete protected tags from this project. Only a project maintainer or owner can delete a protected tag.` / `You can only delete protected tags using the web interface.`, create `You are not allowed to create this tag as it is protected.`
- LfsCheck: `LFS objects are missing. Ensure LFS is properly set up or try a manual "git lfs push --all".`
- Size limit / push size messages from RepositorySizeChecker (skip), file-lock message `'<path>' is locked in Git LFS by @<user>`.
- Signed web commits author check: `For signed Web commits, the commit must be equal to the author`.
- Refs other than heads/tags (e.g. `refs/merge-requests/*`, `refs/notes/*`): only PushCheck applies (no branch/tag check). Pushing to `refs/merge-requests/*` is rejected by Gitaly's reference-transaction/pre-receive (internal refs) **[unverified]**.
- Quirk: BranchCheck "protected" evaluation for an *empty repo* returns true when `default_branch_protected?` (even with no rule).

### Post-receive output (client side messages)
`POST /api/v4/internal/post_receive` (lib/api/internal/base.rb:346) -> `PostReceiveService`: JSON `{"messages":[{"message":"...","type":"basic"|"alert"}], "reference_counter_decreased": bool}`. The hook prints messages to the client: `basic` as plain lines (git prefixes `remote: `), `alert` bracketed by `========` lines **[exact decoration is in gitlab-shell/gitaly hook, not Rails; unverified]**. Message order: alert messages are `unshift`ed (first), then basic in append order: push-option MR warning/alert, broadcast message (alert), then MR URL messages, then redirect/project-created messages.
MR link messages (lib/gitlab/internal_post_receive/response.rb:28-38; app/services/merge_requests/get_urls_service.rb):
```
To create a merge request for <branch>, visit:
  <base>/<ns>/<proj>/-/merge_requests/new?merge_request%5Bsource_branch%5D=<branch URL-encoded>
```
or, when an open MR with that source branch exists in the project (`MergeRequest.from_project(project).opened.from_source_branches`):
```
View merge request for <branch>:
  <base>/<ns>/<proj>/-/merge_requests/<iid>
```
Rules: only if `project.printing_merge_request_link_enabled` (default true) and `merge_requests_enabled?`; repo not empty; one entry per pushed `refs/heads/*` change that is not a deletion and whose branch != default branch; for tags/other refs nothing; for first push to empty repo nothing (repo `empty_repo?` evaluated at post-receive time → after push it's no longer empty, but default branch is excluded; pushing the first *non-default* branch to an empty repo counts as ... (project.empty_repo? false after push) so link IS printed for non-default branch).
Push options (Gitlab::PushOptions::VALID_OPTIONS, lib/gitlab/push_options.rb:5-42): `merge_request.{assign,create,description,draft,label,merge_when_pipeline_succeeds,auto_merge,milestone,remove_source_branch,squash,target,target_project,title,unassign,unlabel}` (alias `mr.`), `ci.{skip,variable,input,no_pipeline}`, `integrations.skip_ci`, `secret_detection.skip_all`, `secret_push_protection.skip_all`, `security_policy.bypass_reason`. Parse: `namespace.key[=value]`; multi-value: ci.input, ci.variable, merge_request.label/unlabel/assign/unassign. Errors are shown as `WARNINGS:\nError encountered with push options '<opt>' ...: <errors>`.

---------------------------------------------------------------------------------------------------
## 10. Effects of a push

Pipeline: Gitaly post-receive -> `POST /internal/post_receive` -> `PostReceiveService#execute` -> enqueues `Repositories::PostReceiveWorker` (`PostReceive` is a shim) -> `process_project_changes`:
1. `expire_caches` (branch / tag name caches; status cache if repo empty).
2. `ProjectCacheWorker` (repository_size; commit_count when default branch touched).
3. `Git::ProcessRefChangesService` (git/process_ref_changes_service.rb): changes grouped by ref type (branch/tag) and action (`created` if old blank, `removed` if new blank, else `pushed`). Per change a `Git::BranchPushService`/`Git::TagPushService` is run. >`push_event_activities_limit` changes in one group => single bulk push event; >`push_event_hooks_limit` => project webhooks skipped.
4. `Repositories::HousekeepingService` increment.

### Git::BranchPushService (git/branch_push_service.rb:20-30)
1. `UpdateMergeRequestsWorker.perform_async(project_id, user_id, oldrev, newrev, ref, {push_options, gitaly_context})` unless `merge_request_branches` excludes this branch (MergeRequests::PushedBranchesService prefilters to branches that are source/target of some open MR).
2. language detection (default branch), `record_project_target_platforms`.
3. `BranchHooksService#execute`: `after_push_commit`; on creation `after_create_branch`, and **`project.after_create_default_branch` if branch is default** (`default_branch?` = `[nil, branch_name].include?(project.default_branch)` -> first branch pushed to a repo with no default becomes default). Branch change hooks: queue `ProcessCommitWorker` for commits matching issue cross-reference/closing patterns (`Closes #1`, etc.; closing happens when pushed to default branch), Jira sync. On removal: `after_remove_branch`.
4. Push event (`EventCreateService#push` — event row `action: pushed`, with `push_event_payload`: `commit_count`, `ref_type`, `action created|pushed|removed`, `commit_from`, `commit_to`, `ref`), push webhooks (`push_hooks`) and integrations, pipeline creation async (`Ci::CreatePipelineService.execute_async(:push)`, unless `ci.skip` / removal), env stop (`Environments::StopService.execute_for_branch` on deletion), `Ci::RefDeleteUnlockArtifactsWorker` on deletion.
- Commit list for event/hook: new branch (default) = last 100 commits of branch; new non-default branch = `commits_between(default_branch, newrev)` (limit 101); update = `commits_between(oldrev, newrev)`; `PROCESS_COMMIT_LIMIT=100`; `commits_count` is the exact count when above limit.
- **First push / empty repo**: no default branch exists yet (`project.default_branch` = preference name e.g. `main`). Any first branch push becomes the repo default: HEAD is repointed by `ProtectDefaultBranchService#protect_default_branch` -> `project.change_head(default_branch)` where `default_branch` is read from the repo (`root_ref`, which Gitaly sets to the first branch pushed to an empty repo if HEAD pointed to a non-existent ref **[Gitaly behavior; Rails then calls change_head to re-assert]**). Then the default-branch protection rule is created per section 8. The initial push permission is governed by `ProtectedBranch.protected_ref_accessible_to?` (empty repo branch: admins & `admin_project` always; `can_initial_push?`; else protected-branch rules as `default_branch_protected?`).
  Note: while empty, `ProtectedBranch.protected?` is true for *any* branch name if `default_branch_protected?` (so pushing branch `foo` first by a Developer under the default "fully protected" setting gets the `A default branch (e.g. main) does not yet exist ...` message).

### MergeRequests::RefreshService#execute(oldrev, newrev, ref) (app/services/merge_requests/refresh_service.rb:19-24)
Only for branch pushes. Sequence in `refresh_merge_requests!`:
1. `find_new_commits`: for added branch: if an MR with this source branch exists, `merge_base(mr.diff_head_sha, newrev)` then `commits_between(common_ref, newrev)`; removed: `[]`; updated: `commits_between(oldrev, newrev)` (@commits, oldest first).
2. `close_upon_missing_source_branch_ref`: every MR (open, any target) whose **source branch no longer exists** (this repo, source_project repo) is closed via `MergeRequests::CloseService` (state `closed`, `closed` event, `ResourceStateEvent` -> system note "closed", todo/notification, webhook `close`). This is how deleting a source branch closes its MRs (unless it was merged). MRs *targeting* a deleted branch are NOT auto-closed or retargeted by the push (retargeting only happens through `MergeRequests::RetargetChainService` after an MR is merged with "remove source branch": open MRs whose target == merged MR's source branch get `target_branch` := merged MR's target branch, `target_branch_was_deleted: true`, system note "changed target branch from `x` to `y`").
3. `post_merge_manually_merged` (**merged by push**): open MRs whose `target_branch == pushed branch` and whose `diff_head_sha` (latest diff head, diff state != empty) is among the pushed commit ids (i.e. the MR's head commit is now in target) are marked merged: `merge_commit_sha`/`merged_commit_sha` = merge commit found by `BranchPushMergeCommitAnalyzer` (the commit that merged head; for ff pushes the head sha itself), `MergeRequests::PostMergeService#execute(mr, source)` — `mark_as_merged`, `merged` event, ResourceStateEvent state `merged` with source commit/MR (system note "merged"/"merged in <commit>"), todos, `execute_hooks 'merge'`, notifications, close issues (if target is default branch), delete non-latest diffs, cleanup/ cancel review apps / environments, remove `refs/merge-requests/:iid/*` as part of cleanup_refs(?) (cleanup deletes `merge` & `train` refs; `head` kept).
4. `link_forks_lfs_objects` (fork MRs with LFS).
5. `reload_merge_requests`: open MRs where `source_branch == pushed branch` OR `target_branch == pushed branch` (plus fork MRs sourced from this project's branch):
   - if `source_project == project && source_branch == pushed` OR force push: `mr.reload_diff(current_user)` -> `MergeRequests::ReloadDiffsService` (unless version limit 1000 / commit limit): old `diff_refs` captured, **new `MergeRequestDiff` created** (`create_merge_request_diff`: first `fetch_ref!` copies source branch tip into `refs/merge-requests/<iid>/head` of the target repo — force-updated), stores head_commit_sha, base_commit_sha (merge-base with target tip), start_commit_sha (target tip), commits and file diffs (state `collected`, or `overflow`/`without_files`/`empty`); older diffs' caches cleared; `update_diff_discussion_positions`; `merge_error` cleared.
   - elsif target pushed and `mr.merge_request_diff.includes_any_commits?(push commit ids)` -> reload diff as well (target moved onto MR commits).
   - every MR → `mark_as_unchecked` (merge status recalculated async).
   - Merge-request diff discussions (inline threads): `Discussions::UpdateDiffPositionService` traces each *active* diff discussion to new diff refs via `Gitlab::Diff::PositionTracer`; if the line disappeared (outdated): note `change_position` set; if project setting `resolve_outdated_diff_discussions?` the discussion is resolved with `resolved_by_push: true`; system note `changed this line in [version N of the diff](url)` (or `... changed this file in version N of the diff`) created by pusher; otherwise position updated to new coordinates.
   - `Suggestions::OutdateService` marks suggestions on changed lines outdated.
   - `abort_auto_merges` when `merge_params.sha != newrev` (auto-merge canceled, system note "canceled the automatic merge"... reason `the source branch was updated`).
   - todos for the MR marked done (`merge_request_push`).
6. For each MR with source branch == pushed: system notes: on branch added `restored source branch `<b>``; on removed `deleted source branch `<b>`` (`SystemNoteService.change_branch_presence`). `notify_about_push` -> **system note** created via `SystemNoteService.add_commits`: body
   ```
   added <N> commit[s]

   <ul><li>sha8 - title</li>...</ul>

   [Compare with previous version](/<ns>/<proj>/-/merge_requests/<iid>/diffs?diff_id=<new diff id>&start_sha=<oldrev>)
   ```
   (body text `added 1 commit` / `added 3 commits`; `<ul>` lists `existing` commits summary line first: `<short>...<short> - <n> commits from branch `<target>`` for commits already in MR (fork: `ns:branch`), then new commits `<li>short_id - title</li>`; more than 10 new commits => first ones collapsed into one `<li>a...b - <n> earlier commits</li>`; `NEW_COMMIT_DISPLAY_LIMIT = 10`). `new_commits`=pushed commits not already in `mr.commit_shas`; `existing` = pushed commits already in MR (e.g. merge of target into source). Skipped if no commits (deleted branch, ff to same).
   Also `mark_mr_as_draft_from_commits` (commit titled `draft:`/`wip:`... => MR title prefixed "Draft: ", system note "marked this merge request as **draft** from <commit>").
7. Async: `MergeRequests::Refresh::WebHooksWorker` (MR `update` webhook, `oldrev` attribute), `PipelineWorker` (merge-request pipelines for the MR), notifications (NotifyAboutPushWorker: "push_to_merge_request" emails).

### Tag push (`Git::TagPushService`/`TagHooksService`)
Event + `tag_push_hooks` webhook + tag pipeline; no MR effects. Release/tag cache invalidation.

### Misc
- `refs/merge-requests/<iid>/head` is created at MR creation (`fetch_ref!`) and re-pointed on each diff reload; `.../merge` refs created by mergeability check (`merge_ref`).
- Push to the MR source branch of a **merged/closed** MR: no diff reload (`reload_diff` returns unless `open?`).
- ProcessCommitWorker: `Closes #N` in commit messages closes issues only when pushed to default branch (via `Issues::CloseWorker`), cross-reference system notes "mentioned in commit <sha>" on referenced issues/MRs.

---------------------------------------------------------------------------------------------------
## 11. Edge cases checklist
1. **Empty repository** (no commits):
   - `GET .../repository/branches` -> `200 []`; `GET .../branches/:b` -> 404 `"404 Branch Not Found"`; `HEAD` -> 404.
   - `GET .../tree` -> 404 `{"message":"404 Tree Not Found"}`; `GET .../commits` -> `200 []`; `GET .../commits/:sha` -> 404 `"404 Commit Not Found"`; `GET .../files/:path?ref=main` -> 404 `"404 Commit Not Found"`; `GET .../tags` -> `200 []`; `GET .../compare` -> 404 `"404 Ref Not Found"`; contributors -> `200 []` or 404.
   - `repository.exists?` false (repo not on disk) differs: commits -> 404 `"404 Repository Not Found"`, tags 404 `"404 Not Found"`.
   - `project.default_branch` for an empty repo = group `default_branch_name` setting else instance `default_branch_name` setting (empty string => nil => later `Gitlab::DefaultBranch.value` -> `main`); `Project#default_branch_or_main`. For non-empty repos = `root_ref` (HEAD symref target; if HEAD points to missing branch, `root_ref` is Gitaly's default-branch guess: `main`, `master`, or first branch alphabetically **[unverified]**).
   - Creating a branch via API on an empty repo first commits an empty `README.md` ("Add README.md") on `default_branch_or_main` authored by the caller.
   - Creating first file via Files API on empty repo works for Developers only if the default-branch protection settings allow the initial push.
2. **Branch names / URL encoding**: `/` in single-branch routes must be `%2F` (`NO_SLASH_URL_PART_REGEX`); the Grape router decodes `%2F` before param extraction; other chars (`#`, `?`, `%`, `+`, space is invalid) percent-encoded. Branch create body `branch` is raw. Names validated by `GitRefValidator.validate` (git_ref_validator.rb): non-empty; must not start with `refs/heads/` or `refs/remotes/`; must not be `HEAD`; must not start with `-`; must not end with `/` or `.`; no control chars (0x00-0x1f, 0x7f), space, `~ ^ : ? * [ \`; no `..`, `//`, `@{`; no path component starting with `.` or ending with `.lock`. (single `@` and names without `/` allowed.) Failure messages: GET/DELETE/protect: 400 `"The branch refname is invalid"`; create: `"Branch name is invalid"`.
3. **Ref both branch and tag**: Branches API only reads `refs/heads`, Tags API only `refs/tags`. Commit/tree/files/compare ref resolution with an ambiguous short name resolves via git `rev-parse` precedence (tag before head) **[Gitaly; unverified]**; passing `refs/heads/x` explicitly works for commits/compare; tree/files accept `ref_type` only in web UI.
4. **404 text conventions**: unknown commit/branch/tag/file in commit family: `404 Commit Not Found`; branch: `404 Branch Not Found`; tag: `404 Tag Not Found`; file: `404 File Not Found`; tree: `404 Tree Not Found`; compare ref: `404 Ref Not Found`; blob: `404 Blob Not Found`; merge base: `404 Merge Base Not Found`; project: `404 Project Not Found`; repo feature disabled `404 Repository Not Found`; protected branch/missing rule `404 Not found`. Commits list with unknown ref is **200 []** (not 404); compare with unknown ref **404 Ref Not Found**.
5. **Deleting a protected branch via API** -> goes through Gitaly `UserDeleteBranch` pre-receive => `400 {"message":"You can only delete protected branches using the web interface."}` (Maintainer) or `... Only a project maintainer or owner can delete a protected branch.` (Developer); for default branch `The default branch of a project cannot be deleted.`. **[Message selection per BranchCheck; API requests carry `gitaly_context` without web marker]**. To unprotect then delete: `DELETE /protected_branches/:name` then `DELETE /branches/:b`.
6. **If-Unmodified-Since** on DELETE branch/tag/protected branch -> 412 `"412 Precondition Failed"` when resource newer than header.
7. **Authorization nuances**: branch create requires `push_code` only (not branch-protection aware at the endpoint; denial arrives from the pre-receive hook: `Gitlab::Git::PreReceiveError` is rescued in `Branches::CreateService` and its message is returned verbatim as 400, e.g. `You are not allowed to create protected branches on this project.`); commits/files POST/PUT check `can_push_to_branch?` -> 403 `"403 Forbidden - You are not allowed to push into this branch"`; unauthenticated -> 401 `"401 Unauthorized"` for these (they call `authenticate!`); for public projects anonymous GET works for branches/tags/commits/tree/files/blobs/compare when repository feature public.
8. `GET commits` pagination has no `x-total`; `GET commits/:sha/diff` has totals; tree legacy page 1 computes totals; keyset never.
9. **Merged flag**: `merged` is relative to the default branch only; default branch itself false; branches identical to default branch false.
10. `Branch.commit` for a branch pointing to an annotated tag object? (n/a; branches always commits). `Tag.commit` is the peeled commit; `target` is tag object sha for annotated.
11. `can_push` on Branch entity is false for anonymous and for Reporters; for Developers on protected branches reflects the rule levels.
12. **Same sha vs short**: `GET commits/:sha` accepts 7+ hex chars; shorter hex strings are treated as ref names (likely 404).
13. **Update file with stale `last_commit_id`**: compares last commit for path on `start_branch` vs commit given; unknown/garbage `last_commit_id` -> lookup nil -> treated as not changed (update proceeds).
14. **Content encoding**: `encoding=base64` content is decoded with lenient `Base64.decode64`; `text` content stored as given bytes (UTF-8). CRLF preserved.
15. Created commit via API has `author` = params or user, `committer` = user; `created_at` = server time; GPG-signed web commits not applicable on self-hosted without config.
16. `Branches::CreateService` creating a branch from a ref that is a *tag name* works (any rev resolvable); from a sha works; `ref` invalid -> `invalid reference name '<ref>'` message.
17. **Push of > 1 refs**: access check evaluates every change; first failing change aborts whole push (atomic from client view).
18. **Branch deletion effects**: open MRs with that *source* → closed by push refresh (section 10.2, note `deleted source branch `x``); MRs targeting it stay open (cannot be merged; "target branch does not exist"). Environments with `on_stop` stopped; artifacts unlocked.
19. `merge_requests` created via push options (`merge_request.create`) run synchronously inside post-receive (`PushOptionsHandlerService`) before the URL messages are computed, so the "View merge request for <branch>:" form appears on the same push.
