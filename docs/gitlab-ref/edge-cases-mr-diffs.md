# Edge cases: MR diffs, compare, commit diffs (GitLab 18.11 CE)

Source tree audited (read-only): `/home/mabn/dev/gitlab-run-ce/gitlab-src`. Paths below are relative to it.
No HTTP requests were made; everything here is from Rails source reading.

**Important scope caveat.** The tree contains only the Rails side (`app lib config db`). The actual
`git diff` invocation, rename/copy detection, binary handling, hunk text, the limit-enforcement loop and generated-file
content heuristics live in **Gitaly (Go), which is not in the tree**. Statements about Gitaly are marked
`[gitaly-from-memory]` with a lower confidence. Everything else is read directly from Rails code.

Confidence legend: H = read directly and unambiguous; M = read directly but involves a multi-step inference, or depends
on one Gitaly assumption; L = mostly Gitaly assumption / memory.

---------------------------------------------------------------------------------------------------

## 0. Architecture cheat sheet (read this first)

Four different pipelines produce a `Gitlab::Git::Diff` and they behave differently:

| Endpoint | Source of data | `expanded` | Limits applied at read time | `generated_file` |
|---|---|---|---|---|
| `GET /merge_requests/:iid/diffs` | **DB** (`merge_request_diff_files`), via `PaginatedMergeRequestDiff` | `false` (Base.default_options) | safe-limit collapsing, per-file 20 KB collapse | persisted bool (true/false; NULL for legacy rows) |
| `GET /merge_requests/:iid/changes` | **DB**, `MergeRequestDiff#diffs` -> `FileCollection::MergeRequestDiff` | `false` | same as above, plus safe_max_files | persisted |
| `GET /merge_requests/:iid/changes?access_raw_diffs=true` | **DB**, `raw_diffs(limits: false)` | n/a (`limits:false` => expanded) | none; `overflow` hard-coded `false` | persisted |
| `GET /merge_requests/:iid/versions/:id` | **DB**, `raw_diffs(limits: false)` | effectively true | none | persisted |
| `GET /repository/compare` | **Gitaly** live (`Compare#diffs`) | `false` | Gitaly limits + collapse; Rails per-file rule | **`null`** (no `generated_files` passed) |
| `GET /repository/commits/:sha/diff` | **Gitaly** live (`Commit#diffs(expanded: true)`) | `true` | Gitaly hard limits only (no collapse); silently truncates | **`null`** |
| `GET /merge_requests/:iid/raw_diffs` | Workhorse -> Gitaly `RawDiff` (plain git diff text) | n/a | none | n/a |

References: `lib/api/merge_requests.rb:616-625` (`/changes`), `:643-653` (`/diffs`), `:664-670` (`/raw_diffs`);
`lib/api/merge_request_diffs.rb:35-53` (`/versions`); `lib/api/commits.rb:395-402`; `lib/api/repositories.rb:248-270`;
`lib/api/entities/compare.rb:15-20`; `lib/api/entities/merge_request_changes.rb`.

Default option merge for FileCollections: `lib/gitlab/diff/file_collection/base.rb:12-14`
(`Commit.max_diff_options.merge(ignore_whitespace_change: false, expanded: false, include_stats: true, ...)`).

### JSON shape of each diff entry (`API::Entities::Diff`, `lib/api/entities/diff.rb:5-24`)

Key order: `diff, collapsed, too_large, new_path, old_path, a_mode, b_mode, new_file, renamed_file, deleted_file, generated_file`.
- `diff` = `json_safe_diff` by default, `unidiff` if `?unidiff=true` (`diff.rb:10`).
- `collapsed` = `collapsed?`, `too_large` = `too_large?`, `generated_file` = `generated?` (may be `null`, see section 6).
- `a_mode`/`b_mode` are **strings**, produced as `old_mode.to_i.to_s(8)` (`lib/gitlab/git/diff.rb:301-302`). A missing side is the string
  `"0"` (new file => `a_mode: "0"`; deleted file => `b_mode: "0"`). Never `null`/`""` for Gitaly-sourced diffs. (H for the Rails
  conversion; M that Gitaly sends mode 0 for the absent side - consistent with `blank_ref?(from_id)` usage.)
- `new_file = blank_ref?(from_id)`, `deleted_file = blank_ref?(to_id)`, `renamed_file = (from_path != to_path)`
  (`diff.rb:303-305`). For add/delete Gitaly sets `from_path == to_path` (both = the file path), so `old_path == new_path` for
  added and deleted files and `renamed_file` is `false`. (M)
- `diff` text from Gitaly **starts at the first `@@` hunk header**: no `diff --git`, `index`, `---`, `+++` header lines. (H for
  Rails not adding any; M that Gitaly strips them; consistent with the API docs example `@@ -71,6 +71,8 @@`.)

---------------------------------------------------------------------------------------------------

## 1. Renamed files, similarity threshold, copy detection

Observable output
- Pure rename (100% similarity): `renamed_file: true`, `new_file: false`, `deleted_file: false`, `old_path` != `new_path`,
  `a_mode == b_mode` (e.g. `"100644"`), **`diff: ""`**, `collapsed: false`, `too_large: false`. With `unidiff=true`: still `""`
  (`diff.rb:204` `return diff if diff.blank?`).
- Rename with edits (similarity >= threshold): same flags, `diff` = hunks relative to the old file, no rename header lines.
  `unidiff=true` => `"--- a/<old_path>\n+++ b/<new_path>\n" + diff` (`diff.rb:203-212`).
- Similarity below threshold => *not* a rename: two entries, one `deleted_file: true` (old path) and one `new_file: true` (new
  path), each with a full-file diff. Copy detection: none requested by Rails; a copied file shows as `new_file: true` with
  full content.
- Threshold: Rails passes **no** similarity option anywhere (`Gitlab::Git::Diff.filter_diff_options`, `diff.rb:97-117`, only
  allows whitespace/limit/expanded/generated/offset keys). The threshold is entirely Gitaly's. `[gitaly-from-memory]`: Gitaly's
  `CommitDiff` runs `git diff --patch --raw --find-renames=30%` (I recall 30%, not git's default 50%), no `--find-copies`. **L-M, verify
  (recipe R1).**
- `FindChangedPaths` with `find_renames` is only used for `Compare#changed_paths` (`app/models/compare.rb:~115-122`) and not in these endpoints.
  `Compare#generated_files` calls it with `find_renames: false` (`lib/gitlab/git/compare.rb:46-53`), see section 6.
- DB storage: `new_path` column is NULL when equal to `old_path` (`merge_request_diff.rb:835-846`, `merge_request_diff_file.rb:44-46`); the
  reader returns `old_path` for `new_path`, so the API always shows both.
- Sort position of a renamed file is by **new** path (`file_collection_sorter.rb:27-29`).
- GraphQL `diffStats` for a renamed file: `path` = new path (Gitaly `DiffStats`; `old_path` not exposed by `DiffStatsType`,
  `app/graphql/types/diff_stats_type.rb`). Threshold there is whatever Gitaly's `DiffStats` uses (numstat; likely default -M). L.

Confidence: Rails flags H; threshold L.

---------------------------------------------------------------------------------------------------

## 2. Binary files, NUL bytes, LFS pointers

Rails logic
- `json_safe_diff` (`diff.rb:262-267`): if `detect_binary?(@diff)` (CharlockHolmes: type binary and confidence 100; a NUL byte
  within the first 1 MiB `DEFAULT_BINARY_SCAN_LEN` is what triggers it, `lib/gitlab/encoding_helper.rb:43-52`) then the API returns the
  synthetic **`"Binary files #{old_path} and #{new_path} differ\n"`** (`diff.rb:129-131`) - note: **no `a/` `b/` prefixes**,
  plain paths, and always the `old_path`/`new_path` even for add/delete (so an added binary gives
  `"Binary files img.png and img.png differ\n"`).
- Otherwise `@diff` is returned verbatim. Real binary files (NUL in first 8000 bytes) make *git* emit
  `Binary files a/x and b/x differ` as patch text with no NUL, so `detect_binary?` is false and that git text passes through
  unchanged. `[gitaly-from-memory]` Gitaly passes git's notice as the patch (new: `Binary files /dev/null and b/x differ`,
  deleted: `Binary files a/x and /dev/null differ`). Evidence in Rails: `BINARY_NOTICE_PATTERN` (`diff.rb:49`),
  `Gitlab::Diff::Parser` bails on it (`lib/gitlab/diff/parser.rb:9`), and the comment at `lib/gitlab/diff/file.rb:491-493` explicitly
  says UTF-16 files "have `Binary files a/filename and b/filename differ' as their raw diff content". **M-H.** (recipe R2)
- When the patch contains actual NULs (a file git considers text because its NULs are past the first 8000 bytes), the stored
  diff is base64 in the DB (`merge_request_diff.rb:796-801`, `merge_request_diff_file.rb:25-40`) and the API
  returns the synthetic no-prefix message above. (H)
- `unidiff=true`: `return json_safe_diff if detect_binary?(@diff) || has_binary_notice?` (`diff.rb:206`) => no `---`/`+++` headers
  for binary files, same notice text.
- flags: `new_file`/`deleted_file`/`renamed_file` per blob ids; modes normal (`"0"`/`"100644"`). `collapsed` false (tiny text) unless
  `generated`. A binary file is also eligible for `expand_diff?` special handling: for binary, expansion = `!enforce_limits || expanded` only
  (`diff_collection.rb:163-171`), i.e. a single-file MR does **not** auto-expand a binary diff, irrelevant since the text is tiny.
- GraphQL `diffStats`: binary files report `additions: 0, deletions: 0` (numstat `-`). `diffStatsSummary.changes = additions+deletions`
  (`diff_stats_summary_type.rb:20-22`). L-M for Gitaly's 0/0.
- LFS pointer files: nothing special in the API. They are ordinary small text blobs: diff shows
  `+version https://git-lfs.github.com/spec/v1\n+oid sha256:...\n+size N` hunks. No smudge. `Gitlab::Diff::File` LFS logic is for the
  HTML viewer only (not involved in these entities). H.

---------------------------------------------------------------------------------------------------

## 3. Mode-only change, symlink, submodule

- chmod +x only: `a_mode: "100644"`, `b_mode: "100755"`, `new_file/deleted_file/renamed_file: false`, **`diff: ""`**
  (Gitaly raw entry without hunks), `collapsed: false`. Rails `mode_changed?` (`diff.rb:191-193`) is not exposed. `unidiff` => `""`. H/M.
- Mode change plus content change: normal hunks and differing modes.
- Symlink add: `new_file: true`, `a_mode: "0"`, `b_mode: "120000"`, diff = `@@ -0,0 +1 @@\n+<target>\n\\ No newline at end of file\n` (symlink blob
  has no trailing newline). Symlink target change: modified, both modes `"120000"`, hunk with two `\ No newline` markers. M.
- Symlink <-> regular file (type change): git emits delete + add as two diffs with the same path; Rails explicitly anticipates duplicates
  (`file_collection_sorter.rb:41` "If A and B have the same name (e.g. symlink change)"). Expect **two entries with the same
  `new_path`**, one `deleted_file: true`, one `new_file: true`. Their relative order is undefined: Ruby `sort` is unstable and the comparator
  returns 0 for equal paths. L-M. (recipe R6)
- Submodule: `submodule?` = mode `"160000"` on either side (`diff.rb:199-201`). Added: `new_file: true`, `a_mode: "0"`, `b_mode: "160000"`,
  `diff: "@@ -0,0 +1 @@\n+Subproject commit <40-hex>\n"`. Changed: both modes `"160000"`, hunk
  `@@ -1 +1 @@\n-Subproject commit <old>\n+Subproject commit <new>\n`. No `\ No newline`. `[gitaly-from-memory]` for the exact hunk text (this
  is git's standard output). Generated-file check skips submodule paths (`gitaly_client/analysis_service.rb:20-24`). M.
- Mode-only/submodule entries have no content so never collapse (collapse needs bytes >= 20480 or `line_count > 0` during safe-limit
  collapsing, `diff_collection.rb:218`).

---------------------------------------------------------------------------------------------------

## 4. Empty file added/deleted, missing trailing newline

- Empty file added: `new_file: true`, `a_mode: "0"`, `b_mode: "100644"`, **`diff: ""`** (stored as `''`), `old_path == new_path`,
  `collapsed: false`, `too_large: false`. Empty file deleted: `deleted_file: true`, `a_mode: "100644"`, `b_mode: "0"`, `diff: ""`.
  It counts as a file in `changes_count`/`real_size` (size counts every yielded diff, `diff_collection.rb:118-126`) but adds 0 lines/bytes.
  M-H.
- File with no trailing newline: Gitaly keeps git's marker line. Example new file `abc` (no EOL):
  `"@@ -0,0 +1 @@\n+abc\n\\ No newline at end of file\n"`. Changing to add a trailing newline gives
  `-abc\n\\ No newline at end of file\n+abc\n` in the hunk. Rails does not touch it (`count_lines` etc. irrelevant to API). M.
- Diff text normally ends with `\n` (every line, including the last, terminated).

---------------------------------------------------------------------------------------------------

## 5. Diff limits (the intricate part)

### 5.1 Settings and defaults (18.11)
- `ApplicationSetting`: `diff_max_patch_bytes` default **204800** (200 KiB; `Gitlab::Git::Diff::DEFAULT_MAX_PATCH_BYTES`, `diff.rb:29`; DB
  `db/structure.sql:14486`), `diff_max_files` default **1000** (`commit.rb:38`), `diff_max_lines` default **50000** (`commit.rb:37`).
  Validation allows only values >= default and <= upper bounds: patch bytes <= 512000 (`diff.rb:34`, 500 KiB), files <= 3000, lines <=
  100000 (`application_setting.rb:312-333`, `commit.rb:39-40`). So the instance can never be configured *below* the defaults.
- Derived (`Commit.diff_safe_max_files = max_files/10` = **100**; `diff_safe_max_lines = max_lines/10` = **5000**, `commit.rb:104-110`;
  `DIFF_SAFE_LIMIT_FACTOR = 10`):
  - `DiffCollection.limits` (`diff_collection.rb:21-35`): `max_files`=1000, `max_lines`=50000, `max_bytes = max_files*5120 = 5,120,000`,
    `safe_max_files`=100, `safe_max_lines`=5000, `safe_max_bytes = 100*5120 = 512,000`, `max_patch_bytes`=204800.
  - Per-file collapse threshold `patch_safe_limit_bytes = hard/10` = **20480** (`diff.rb:141-143`).
- Rails sends these to Gitaly in `CommitDiffRequest` (`gitaly_client/commit_service.rb:661-673`): `enforce_limits = options[:limits] (default true)`,
  `collapse_diffs = !expanded`, plus all limits above.

### 5.2 Per-file flags and what `diff` becomes

| Condition | `collapsed` | `too_large` | `diff` |
|---|---|---|---|
| patch < 20480 B, within cumulative safe limits | false | false | full hunks |
| 20480 <= patch < 204800 and `expanded=false` (MR `/diffs`, `/changes`, compare) | **true** | false | `""` |
| same but `expanded=true` (commit diff, versions, `access_raw_diffs`, single-file MR) | false | false | full hunks |
| patch over hard limit (`diff_max_patch_bytes`) | false | **true** | `""` |
| after cumulative safe limit tripped, `expanded=false`, `line_count>0` | **true** | false | `""` |
| `generated_file` true and `expanded=false` | **true** | false | `""` (even if tiny) |

References: `prune_diff_if_eligible` (`diff.rb:318-329`): `if too_large? -> too_large! elsif collapsed? || collapse_generated_file? -> collapse!`;
`collapsed?` = `!expanded && diff_bytesize >= patch_safe_limit_bytes` (`diff.rb:242-246`; `>=`, bytes of the patch text). `too_large!`
and `collapse!` both `prune!` (diff=`''`, line_count 0). `too_large?` for DB rows returns the **persisted** boolean without recomputing
(`@too_large` non-nil, `diff.rb:221-227`), so the threshold in force at MR-diff creation time applies forever. For Gitaly-sourced
diffs `@too_large = gitaly_diff.too_large` (`diff.rb:306`). A too-large diff reports `collapsed: false` (the `collapsed?` computation sees
the pruned empty diff). H.

What is *stored* at MR-diff creation: `save_diffs` -> `compare_diffs_preloaded` uses `Commit.max_diff_options` ({max_files, max_lines}) with no
`expanded` key => `expanded=true`, `collapse_diffs=false` (`merge_request_diff.rb:977-984`). So everything up to 200 KiB per file is stored
in full; only `too_large` (>= hard limit; boundary `>=` in Rails' own check `diff.rb:224`, `>` vs `>=` in Gitaly unverified) is stored pruned.
Collapsing is therefore a **read-time** decision on MR endpoints; `collapsed` is *not* a persisted column (`SERIALIZE_KEYS`, `diff.rb:33-45`).

### 5.3 Cumulative ("safe") collapsing at read time (`DiffCollection#each_serialized_patch`, `diff_collection.rb:204-248`)
Applies to DB-backed reads (MR `/diffs`, `/changes`) with `expanded=false`:
- Files are processed in stored (sorted) order. Before each file: `over_safe_limits?(i)` is true if `i >= 100` (files) OR
  `@line_count > 5000` (strict) OR `@byte_count >= 512000` (`:151-161`); the flags are sticky.
- If over safe limits and the file's `line_count > 0`, it is collapsed (`collapsed: true`, `diff: ""`). The *crossing* file itself is not
  collapsed (the check precedes the counter update). Empty-patch files (renames, chmod, empty files) are never collapsed this way.
- Counters add the **post-prune** sizes (`:221-222`), so files already collapsed by the 20 KiB rule add 0 lines/bytes.
- `expand_diff?` (`:163-171`): for non-binary diffs `@iterator.size == 1 || allow_expansion`. For the DB path the iterator is the in-memory
  array, so a MR/page with exactly **one** file is fully expanded (not collapsed, not even generated-collapsed; only too_large stays pruned).
  **Quirk:** `/diffs?page=1&per_page=1` makes the page array size 1 and expands file #1; `page=2&per_page=1` prepends the offset
  (`paginated_diffs.rb:21-32`) so size is 2 and it is not expanded.
- **Quirk (`/diffs` vs `/changes`):** `i` counts only yielded items (`:244-245`). In `/changes` (all files yielded) files with sorted index
  >= 100 that have content are collapsed. In `/diffs` the `i` restarts at 0 on each page (offset files are loaded but not yielded) and
  per_page is capped at 100, so the *file-count* safe limit never trips; only cumulative lines/bytes (which do include the offset files)
  can. => the same file can be `collapsed: true` in `/changes` and fully present in `/diffs?page=N`. M (pure code reading).
- Hard overflow on reload (`:208-236`): `i >= 1000` (checked at start of an item, so exactly 1000 stored files never triggers),
  `@line_count >= 50000` or `@byte_count >= 5,120,000` (after adding; that file is discarded). Because stored files were already truncated
  and counters count post-collapse sizes, reload essentially never overflows.

### 5.4 `overflow`, `changes_count`, `real_size`
- Creation (`MergeRequestDiff#save_diffs`, `merge_request_diff.rb:942-974`): `real_size = diff_collection.real_size`
  (`diff_collection.rb:118-126`): `"#{size}+"` if overflowed, else `size.to_s`, where `size` = number of diffs retained. `state=overflow` if
  `overflow?` (set last so it beats `collected`).
  - Overflow detection is by Gitaly's `overflow_marker` (`diff_collection.rb:187-197`): the marked diff is **dropped**, loop breaks. So 1001+ files
    => 1000 stored, `real_size: "1000+"`. Line overflow (>= 50000 total lines, at Gitaly) can yield e.g. `"5+"` (fewer than 1000 files). Byte
    overflow (>= 5,120,000) similarly. Exact comparison operators (`>` vs `>=`) are Gitaly's `[gitaly-from-memory]`.
  - `collect_all_diff_paths` feature flag is `default_enabled: false` (`config/feature_flags/development/collect_all_diff_paths.yml`) => the
    "keep going after overflow, paths only" mode is OFF; overflow breaks the loop.
- `changes_count` in the MR JSON = `merge_request.merge_request_diff.real_size` (`lib/api/entities/merge_request.rb:10-12`) - the **string column**:
  `"3"`, `"1000+"`, `"5+"`, or **`null`** when (a) the async diff has not been created yet (see section 8), or (b) state `empty` due to no commits
  (real_size never set).
- **`overflow` in `/changes` is recomputed, not read from `state`**: `MergeRequestChanges#overflow?` (`merge_request_changes.rb:14-16`) calls
  `DiffCollection#overflow?` on a fresh DB read. A MR that is `state: overflow` with `changes_count: "1000+"` therefore normally returns
  **`overflow: false`** (only 1000 files stored; see 5.3). With `access_raw_diffs=true` it is always `false` (`:15`). (M-H; high-value verification, recipe R4.)
- Compare API: `compare_timeout` = `compare.diffs.diffs.overflow?` (`entities/compare.rb:17-19`), computed from Gitaly's marker (`each_gitaly_patch`). The
  overflow-marked diff is excluded; the list is silently truncated (compare has no `changes_count`).
- Commit diff API (`commits.rb:400-402`): `commit.diffs(expanded: true).diffs.to_a` -> truncated at Gitaly limits **with no indicator at all**;
  pagination `X-Total` counts only the retained diffs.
- Gitaly collapse semantics for compare (`collapse_diffs=true`): Gitaly sets `collapsed` and empties the patch once safe limits are
  exceeded; Rails also applies its own 20 KiB-per-file rule (`diff.rb:242`) to every file **except the first**: `expand_diff?`
  uses `DiffStitcher#size` (`gitaly_client/diff_stitcher.rb:15-17,27-30`), which is already incremented to 1 when the first diff is yielded, so
  `@iterator.size == 1` is true for file #1 only => file #1 of a compare is never collapsed by the Rails rule (only if Gitaly itself collapsed it
  via `raw.collapsed`, `diff.rb:312`). M.

### 5.5 GraphQL stats are not capped like REST
`diffStats` = `repository.diff_stats(base_sha, head_sha)` (all files, no 1000 cap; `merge_request.rb:1180-1186`). `diffStatsSummary` uses
`merge_request.metrics.added_lines/removed_lines` if both non-NULL (`merge_request_diff_summary_batch_loader.rb:14-20`, `file_count` = persisted
`files_count`, which is `min(32767, count)`, and equals 1000 max for overflowed diffs) otherwise sums `diff_stats` (all files) with
`file_count` = number of paths. Nothing in the app code populates `added_lines/removed_lines` outside importers, so expect the fallback.
If the diff is not created yet or `diff_refs` is nil => `NIL_STATS` `{additions:0, deletions:0, file_count:0}` and `diffStats` => `[]`
(`merge_request_type.rb:332-340`). `Repository#diff_stats` returns an empty collection if either SHA is blank/zero (`repository.rb:539-549`).
`DiffStatsCollection#real_size` ("1000+") is used by `MergeRequest#diff_size` only when `real_size` is nil (`merge_request.rb:1188-1192`).

---------------------------------------------------------------------------------------------------

## 6. `generated_file`

Flow (MR creation only): `Compare#generated_files` (`lib/gitlab/git/compare.rb:46-53`) -> `Repository#detect_generated_files`
(`lib/gitlab/git/repository.rb:1255-1291`):
1. `changed_paths = find_changed_paths([DiffTree(base.id, head.id)])` (no rename detection => a rename shows as D+A) where for MR `base` is
   `@base` = the **start/target-tip commit** (Compare is built `Compare.new(repo, safe_start_commit_sha, head_commit_sha)`, `merge_request_diff.rb:~395`),
   **not** the merge base. So paths changed on the target since the fork are included in the list.
2. `changed_paths.take(diff_max_files)` (`:1260`) - only the first 1000 of that list are examined (target-only changes can crowd out real ones).
3. `.gitattributes` check: `get_file_attributes(base, paths, %w[gitlab-generated linguist-generated])` (`lib/gitlab/git.rb:20-23`). Attributes
   are read at **`base` = target tip**, so a `.gitattributes` introduced by the MR itself is not honored for that MR (M-H). A path is generated
   iff `value == "set"`; `unset`/`false`-style values mark it *not generated* and skip auto-detection for that path (`:1264-1272`).
4. Remaining paths -> Gitaly `CheckBlobsGenerated` (`gitaly_client/analysis_service.rb:17-64`) in batches of 100; submodule paths skipped; for
   `new_file?` the blob is `head:path`, otherwise (modified/deleted) **`base:path`, i.e. the OLD content at the target tip**. So content-based
   detection of a *modified* file judges the old blob. The patterns themselves (go-enry/linguist-style: lockfiles such as package-lock.json /
   yarn.lock, minified js/css, protobuf outputs, `Do not edit` headers, etc.) are Gitaly-side `[gitaly-from-memory]`, L. (recipe R5)
5. Any `CommandError`/`ResourceExhaustedError` => empty set, silently (every file `generated_file: false`).
Application: `DiffCollection#each_gitaly_patch` sets `generated = generated_files.include?(raw.from_path)` (`diff_collection.rb:179`) - looked up by the
**old path**. For a rename the new path was an "ADDED" entry, but lookup uses `from_path` so only the old path's flag matters.
Persisted in `merge_request_diff_files.generated` (nullable boolean, `db/structure.sql:23521`).

Reported:
- MR `/diffs`, `/changes`, `/versions/:id`: `generated_file: true|false` from DB (can be `null` for rows created before the column existed).
- `generated` + `expanded=false` => `collapsed: true`, `diff: ""` (`diff.rb:275-277,318-329`), regardless of size. In `/versions/:id` and
  `access_raw_diffs=true` (expanded) the full diff is returned. A single-file MR is expanded, so a generated lone file shows its diff.
- **Compare and commit-diff endpoints: `generated_file: null`** - no `generated_files` option is ever passed (`grep generated_files` shows only
  `merge_request_diff.rb` and `compare.rb` callers), so `Diff#generated` stays at its constructor default `nil` (`diff.rb:62`) and Grape renders `null`. (H)
- Only `.gitattributes` values `gitlab-generated` / `linguist-generated` are consulted by Rails (not `linguist-vendored`, `-diff`, `binary`).

---------------------------------------------------------------------------------------------------

## 7. CRLF, very long lines, non-UTF-8, whitespace-only changes

- CRLF: carried through untouched; JSON shows `\r\n` inside lines. Lines changed only by EOL conversion appear as `-foo\r\n`/`+foo\n`. Rails does nothing. H/M.
- Very long lines: no truncation in Rails; only the per-file `diff_max_patch_bytes` -> `too_large`, and 20 KiB collapse rule. (Gitaly may
  have its own line-reader limit; L.)
- Encoding pipeline:
  - Gitaly path (`init_from_gitaly`, `diff.rb:298`): `encode!(patch)` (`encoding_helper.rb:19-37`): force UTF-8; if valid, keep. If invalid: if
    CharlockHolmes says binary (NUL, confidence 100) keep as BINARY; else if detected encoding confidence > 50 (`ENCODING_CONFIDENCE_THRESHOLD`)
    re-interpret as that encoding; then `clean` (UTF-16BE round trip, undefined/invalid chars **removed** (replace `""`), and **all `\0` removed**
    via `gsub("\0")`) => valid UTF-8. So Latin-1 content is *transcoded* when confidence > 50, else bytes dropped.
  - Then `Diff#initialize` `encode_diff_to_utf8(true)` (`diff.rb:279-283`) re-runs `encode_utf8_with_replacement_character` only if not binary or not valid
    encoding; U+FFFD (`"�"`) appears only if something still invalid reaches that stage (DB-hash path, or charlock detection fails).
  - Paths: `encode_utf8_safe_path` (invalid bytes replaced/dropped, `encoding_helper.rb:62-75`); `encoded_file_path` not exposed.
  - DB: diffs that are BINARY-encoded with non-ASCII or contain `\0` are base64'd with `binary=true` (`merge_request_diff.rb:796-801,841-844`) and decoded on read;
    otherwise stored as text.
  - Output `diff` is always JSON string valid UTF-8; real NUL-bearing patches are replaced by the synthetic "Binary files X and Y differ\n" (section 2).
  M for exact bytes (needs a recording, recipe R3).
- Whitespace-only change: ordinary hunk (`-  foo` / `+\tfoo`). `ignore_whitespace_change` exists internally (`Gitlab::Git::DiffCollection` /
  `compare.diffs(options)`, `fetching_repository_diffs` forces live Gitaly when set, `merge_request_diff.rb:860-873`) but **no public API parameter**
  in the endpoints under audit exposes it. H.

---------------------------------------------------------------------------------------------------

## 8. MR diff versions

### 8.1 What is stored
- `merge_request_diffs` row per version (`app/models/merge_request_diff.rb`). Only `diff_type: regular` rows are listed (`merge_request.rb:71-72`
  `has_many :merge_request_diffs, -> { regular }`); the internal `merge_head` diff is excluded from `/versions` and never `latest`.
- Columns exposed (`lib/api/entities/merge_request_diff.rb`): `id, head_commit_sha, base_commit_sha, start_commit_sha, created_at,
  merge_request_id, state, real_size, patch_id_sha`. List order: **id DESC** (`merge_request_diffs.order_id_desc`, `merge_request_diffs.rb:42`), standard
  pagination (default per_page 20 from `PaginationParams`). `real_size` is the string; `patch_id_sha` is a hex string (bytea via `sha_attribute`)
  set in an `after_create_commit` (`:set_patch_id_sha`, skipped when base == head, so `null` then) - can be `null` briefly after creation.
- States in 18.11 (`merge_request_diff.rb:55-68`): live = `empty` (initial), `collected`, `overflow`, `without_files` (via `clean` event, system
  cleanup of old merged/closed MRs). Deprecated, still valid values in old data: `timeout`, `overflow_commits_safe_size`,
  `overflow_diff_files_limit`, `overflow_diff_lines_limit`. A fresh 18.11 instance never emits the deprecated ones. (H)
- State logic in `save_diffs` (`:942-974`):
  - `compare.commits.empty?` (head already contained in start, or same sha) => `state: empty`, `real_size: nil`, no diff files, `files_count 0`.
  - commits exist but zero file changes (e.g. an empty commit) => `real_size: "0"`, state stays the initial **`empty`** (the `collected` assignment is
    guarded by `diff_collection.any?`), `commits_count > 0`.
  - at least one file => `collected` with `real_size "N"`; overflow => `overflow` with `"N+"`.
  - `files_count` column = `min(32767, rows)` (`FILES_COUNT_SENTINEL`, `:26`); `commits_count` set in `set_count_columns` (`:996-1001`).
- `/versions/:id` (`merge_request_diffs.rb:35-53`; 404 if the id is not a *regular* diff of that MR): same fields plus `commits` (`diff.commits(load_from_gitaly: true)`,
  ordered by `relative_order`, i.e. **newest first** since stored from `compare.commits.reverse`) and `diffs` = `raw_diffs(limits: false).to_a` straight from DB (no repository
  fallback even for `without_files` => `diffs: []`). `limits: false` => `expanded` (so no 20 KiB collapse, no generated collapse; `too_large: true` entries still pruned).
  Response is cached per `unidiff` (`present_cached`).

### 8.2 When a new version is created
- MR creation: **asynchronously** - `MergeRequests::CreateService#before_create` sets `skip_ensure_merge_request_diff` (`create_service.rb:56`) and
  `NewMergeRequestWorker` -> `AfterCreateService#execute` -> `ensure_merge_request_diff` (`after_create_service.rb:6-8`). Until the worker runs
  (typically sub-second to seconds): `merge_request_diff` is an unsaved `MergeRequestDiff.new` (`merge_request.rb:115-119`) so `diff_refs: null`, `changes_count: null`,
  `/versions: []`, `/diffs: []`, `diffStats: []`, GraphQL `diffRefs: null`. (M-H.)
- Push to the **source** branch (matching project) or **any force push** touching the MR's branches => `reload_diff` => new row (`refresh_service.rb:156-216`),
  even if the resulting content is identical (no dedup). Skipped for non-open MRs (`merge_request.rb:1521`), when 1000 versions reached or 1,000,000 cumulative commits
  (`reached_versions_limit?` `:2828-2843`, gated by feature flags).
- Push to the **target** branch: a new version is created **only if** the push's commit ids intersect the MR's current diff commits
  (`includes_any_commits?(push_commit_ids)`, `refresh_service.rb:195,215`). A plain advance of the target with unrelated commits creates **no** new version, so
  `start_commit_sha` becomes stale relative to the actual target tip while `diff_refs.start_sha` stays at the old value.
- Changing `target_branch`/`source_branch` via update => `reload_diff_if_branch_changed` (`merge_request.rb:1513-1518`) => new version.
- Closed MRs whose source branch is deleted keep the last valid diff (comment `refresh_service.rb:71-76`).

### 8.3 `start_commit_sha` / `base_commit_sha` / `head_commit_sha`
- `start_commit_sha` = `merge_request.target_branch_sha` at the time the version is created (`merge_request_diff.rb:326-328`); `head_commit_sha` = source branch tip;
  `base_commit_sha` = `merge_base(head, start)` (`find_base_sha`, `:1011-1014`), `null` if no common ancestor. These three are the MR `diff_refs`
  (`Entities::DiffRefs`: `base_sha, head_sha, start_sha`) and GraphQL `DiffRefs` (`base_sha` nullable; `head_sha`/`start_sha` non-null).
- After merging the target into the source branch (new push to source), the new version has `start_commit_sha` = new target tip and
  `base_commit_sha` = usually the same tip (so `start == base`, "straight" compare in `DiffRefs#compare_in`, `diff_refs.rb:62-67`), and the diff
  excludes everything already on the target. While the target merely moves ahead, `base` stays at the old fork point.
- The diff content is always `base..head` (`Gitlab::Git::Compare` non-straight => merge_base(head,start) -> head); `/raw_diffs` uses `base_sha..head_sha`
  of the latest version (`lib/gitlab/workhorse.rb:415-421`) and is a streamed Workhorse response (`Content-Disposition: inline`, plain git diff text with
  `diff --git`, `index`, `similarity index`/`rename from/to`, `old mode/new mode`, `Binary files ... differ`); `diff_refs` nil => error (500-ish). Rename detection
  for RawDiff is Gitaly's `git diff` default (-M50%?) `[gitaly-from-memory]` L.
- MR JSON `diff_refs` is `null` when the latest diff has neither start nor base (`merge_request_diff.rb:~438-447`).

---------------------------------------------------------------------------------------------------

## 9. Ordering and pagination

### 9.1 Ordering
- Stored order is **not git order**: `build_merge_request_diff_files` sorts via `FileCollectionSorter` (`file_collection_sorter.rb`) and sets `relative_order`
  (`merge_request_diff.rb:835-846`), `sorted=true`. The sort runs *after* truncation to the first 1000 files in git (bytewise path) order.
- Comparator (`file_collection_sorter.rb:12-45`), on `new_path.presence || old_path` split by `/`:
  1. at each level, if one path has no remaining components and the other has some, the one **with remaining components (a directory) comes first**
     => **directories before files at every level**, regardless of name;
  2. otherwise component-wise `String#<=>` (bytewise, case-sensitive: `B` < `a`);
  3. equal => 0 (unstable sort).
  Example: `["b.txt", "a/z.txt", "B/x", "a/b/c.txt", "A.txt"]` -> `B/x`, `a/b/c.txt`, `a/z.txt`, `A.txt`, `b.txt`.
  Note this differs from `git diff` order (and from compare/commit-diff order, which is Gitaly's raw order, not re-sorted here).
- Renames sort by new path; D+A pairs sort independently.
- `/changes` and `/diffs` use stored order; `/versions/:id` too. Compare and `/commits/:sha/diff` use Gitaly order (git's tree order; renames positioned by
  destination path `[gitaly-from-memory]`).

### 9.2 `/diffs` pagination (`merge_requests.rb:643-653`, `paginated_merge_request_diff.rb`, `paginated_diffs.rb`)
- Grape `use :pagination`: `page` default 1, `per_page` default **20** (`pagination_params.rb`), `except_values: [0]`; Kaminari `max_per_page = 100` caps larger values
  (`config/initializers/kaminari_config.rb`). (The `DEFAULT_PER_PAGE = 30` in the collection is never reached because Grape always supplies 20.)
- Pages are slices of `merge_request_diff_files` ordered by `relative_order`; headers `X-Per-Page, X-Page, X-Next-Page, X-Prev-Page, X-Total, X-Total-Pages, Link`
  where total = number of stored files (<= 1000), `X-Total-Pages = max(total_pages, 1)` (`offset_pagination.rb:~66-71`).
- Page beyond the end => `[]` (200). Because the collection is a Kaminari-like object with `current_page`/`limit_value`, `paginate` does not re-slice
  (`offset_pagination.rb:~35-41`, `skip_pagination_check: true`).
- For page N the offset files are loaded too so cumulative counters see them (5.3). The `i >= max_files` check is page-relative.
- `/commits/:sha/diff` and `repository/compare` are *not* DB-paginated: compare returns everything (no pagination) in `diffs`; commit diff uses
  `Kaminari.paginate_array` over the already-truncated list (default per_page 20).
- `/changes` is unpaginated (all <= 1000 stored files, with the safe-limit collapsing of 5.3).

---------------------------------------------------------------------------------------------------

## 10. Other things the simulator can trip on

- Entity key order and `null` semantics: `generated_file` null (compare/commit), `changes_count` null (before diff exists / no commits), `diff_refs` null,
  `patch_id_sha` null.
- `/commits/:sha/diff` of a merge commit diffs against the **first parent**; root commit diffs against the empty tree (`gitaly_client/commit_service.rb:675-679`).
- `ignore_whitespace_change` forces repository (Gitaly) fetch even for DB-backed diffs (`fetching_repository_diffs`), not exposed publicly.
- `without_files` versions: `/diffs` and `/changes` fall back to live Gitaly with `diff_refs.compare_in` (`merge_request_diff.rb:860-873`); `/versions/:id` returns `diffs: []`.
  Unlikely on a fresh instance.

---------------------------------------------------------------------------------------------------

## Most valuable cases to record against a real instance

All via `POST /projects/:id/repository/commits` (actions with `encoding: "base64"` for binary, `execute_filemode` for chmod; `action: move` with `previous_path`
for renames, `action: chmod`), then open an MR and wait for the async diff (poll `/versions` until non-empty). Capture `/diffs`, `/changes`, `/changes?unidiff=true`,
`/versions/:id`, `/raw_diffs`, `/repository/compare?from=<target>&to=<source>`, `/repository/commits/<sha>/diff`, and the MR JSON.

R1 (rename threshold, copy). Base commit on `main`: `r/a.txt` = 20 numbered lines, `r/b.txt` = 20 lines, `r/c.txt` = 20 lines, `r/copy-src.txt` = 20 lines.
Feature commit: `move r/a.txt -> r/a2.txt` unchanged (100%); `move r/b.txt -> r/b2.txt` with ~35% of lines changed; `move r/c.txt -> r/c2.txt` with ~65% of lines changed;
`create r/copy-dst.txt` identical to `r/copy-src.txt` (copy). Record which are `renamed_file`; tests the 30%-vs-50% question and copy handling. Also check `diff: ""`
for the pure rename and `unidiff` output.

R2 (binary). Commit with `create bin/new.png` (PNG bytes, base64), `create bin/nul.dat` (`"a\0b\n"`, base64), `create bin/utf16.txt` (UTF-16LE with BOM text, base64),
`create bin/late-nul.txt` = 9000 ASCII bytes then `\0` then newline (git thinks text). Second MR commit: `update bin/new.png` with different bytes, `delete bin/nul.dat`.
Record exact `diff` strings (`Binary files a/.. and b/.. differ` vs `/dev/null` vs prefix-less `Binary files x and x differ\n`), `a_mode/b_mode`, `unidiff`, and `late-nul.txt` JSON/DB behaviour.
Also add an LFS-pointer text file (`version https://git-lfs.github.com/spec/v1\noid sha256:<64hex>\nsize 123\n`) to confirm it is plain text.

R3 (encoding/CRLF/long lines). Files: `enc/latin1.txt` (ISO-8859-1 bytes `caf\xe9\n`, base64), `enc/bad.txt` (`\xff\xfe\xfa` invalid sequence mixed with ASCII lines), `enc/crlf.txt`
(`a\r\nb\r\n` then modified to `a\r\nc\r\n`), `enc/long.txt` (one 150 KB line, and a second 250 KB-line file to cross `too_large`), `enc/ws.txt` (tab<->space change only),
`enc/noeol.txt` (no trailing newline, then add newline). Record raw `diff` strings.

R4 (limits - the highest value). Use few-file lines-based recipes rather than 1001 files where possible.
 a. `limits/big20k.txt` with a ~21,000-byte patch (e.g. 700 lines x 30 bytes) in an MR with 2+ files => expect `collapsed: true`, `diff: ""`; the same file as the only file in the MR => full
    diff; and `/diffs?per_page=1&page=1` vs `per_page=20` (expanded-quirk). Boundary: patches of exactly 20479/20480/20481 bytes.
 b. `limits/huge.txt` with patch >= 204,800 bytes (and one at exactly 204,799/204,800) => `too_large: true`, `collapsed: false`, `diff: ""`; check `changes_count` unchanged.
 c. Cumulative: one commit adding 12 files x 600 lines (each patch < 20 KiB) => total 7,200 lines > 5000: record which files after the crossing one come back `collapsed: true` in
    `/diffs` vs `/changes` vs `/versions/:id` (expanded, nothing collapsed).
 d. File-count quirk: 130 small files (single commit, plus each in a directory to vary sort), compare `/changes` (files with index >= 100 collapsed) vs `/diffs?per_page=100&page=2`.
 e. Line overflow: one commit with 6 files x 10,000 lines (each ~30 KB, below 200 KiB) => total >= 50,000. Record `changes_count` (expect `"N+"` with N<1000), `state` in `/versions`, and
    `/changes` `overflow` (expect `false` per analysis) and compare API `compare_timeout`.
 f. File overflow: single commit with 1,001 one-line files => `changes_count: "1000+"`, `/versions` state `overflow`, `/changes` `overflow` (expect false), `X-Total: 1000`; and
    `/repository/commits/<sha>/diff?per_page=100` total (truncation with no indicator); `compare` `compare_timeout`.

R5 (generated). Target branch has `.gitattributes`: `gen/attr.txt gitlab-generated`, `gen/lin.txt linguist-generated`, `gen/off.js -linguist-generated`. Feature branch adds/changes those files plus
`package-lock.json`, `yarn.lock`, `app.min.js`, `x.pb.go`, a file starting with `// Code generated by ... DO NOT EDIT.`, and ALSO adds its own `.gitattributes` line `own/f.txt gitlab-generated` plus
`own/f.txt` (to test base-vs-head attribute source). Also modify (not add) a file whose old version is generated-looking vs new version not (tests base blob use), and rename a generated file.
Record `generated_file` in `/diffs` (expect collapsed + empty diff), `/versions/:id` (full), compare and commit diff (`null`).

R6 (modes/symlinks/submodules/empties). Commit: `chmod mode/x.sh` (`execute_filemode: true`), `create sym/link` as symlink (commits API cannot create symlinks or submodules directly - use git push of a
branch containing a symlink, a gitlink (`git submodule add` or `git update-index --add --cacheinfo 160000,<sha>,sub`), and a symlink->file type change), `create empty/e.txt` (content `""`), delete a
pre-existing empty file. Record `a_mode/b_mode`, empty `diff`, duplicate-path entries and their order for the type change, and submodule hunk text.

R7 (sorting). Single commit creating `b.txt`, `B/x`, `a/z.txt`, `a/b/c.txt`, `A.txt`, `_u.txt`, `a.d/f` , `a/ä.txt`; compare MR `/diffs` order with `/repository/commits/<sha>/diff` order and compare order.

R8 (versions/diff_refs). Create MR; (1) immediately GET MR/`/versions` to see the pre-diff state; (2) push to target an unrelated commit => `/versions` count unchanged; (3) push source commit => new version with
new `start_commit_sha`, same-or-new `base_commit_sha`; (4) merge target into source via commits API branch ops/git push => `start == base` for that version; (5) push an empty commit (`allow_empty`)
to the source => `real_size: "0"`/state `empty` with commits; (6) make source already merged into target => state `empty`, `real_size: null`. Also record list order and `created_at` format.
