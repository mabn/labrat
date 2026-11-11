# labrat shortcuts audit (MR / code review)

Read-only audit of labrat against GitLab 18.11 CE. GitLab paths are relative to
`/home/mabn/dev/gitlab-run-ce/gitlab-src`. labrat paths are relative to `/home/mabn/dev/shed/labrat`.

Method: grep for TODO/FIXME/"not implemented"/"approximat"/"simplif"/hard-coded returns, a diff of the
18.11 introspection against the resolver arms, a read of the git engine, services and hook code, and a check
of each candidate against the GitLab source. Almost nothing in the code is labelled as a shortcut. The
code carries very few markers, and the `docs/gitlab-ref/*-audit.md` and `coverage-gaps.md` files are partly
stale (for example `/copy_metadata`, group mentions, `mentioned in merge request`, todos on push and
retargeting aborts are all implemented now). Everything below was re-checked against the current code.
Items already in docs/PLAN.md "Known approximations" or docs/discrepancies.md are left out.

Confidence: V = read in both labrat and GitLab source. U = labrat side verified, GitLab behaviour
inferred from source but not recorded.

## Ranked findings

### 1. Tree merge has no rename detection and uses a single merge base (V)
- labrat: `crates/labrat-git/src/merge.rs:61` (`merge_trees`) flattens both trees and merges path by path. A
  file that was moved on one side and edited on the other is reported as a conflict, and so is a
  delete/modify pair. `walk.rs:78` (`merge_base`) returns one base chosen by committer time, with no
  virtual base for criss-cross histories. `merge_files` (`merge.rs:191`) is a plain line merge.
- GitLab: Gitaly merges with git's merge-ort or libgit2 (rename detection on, recursive base). The result
  feeds `MergeRequests::Mergeability::CheckConflictStatusService`
  (`app/services/merge_requests/mergeability/check_conflict_status_service.rb`), merge, `merge_ref`,
  rebase and squash.
- Observable: `merge_status`, `detailed_merge_status` (`conflict`), `has_conflicts`, `PUT .../merge` (405
  or success) and `/rebase` (`merge_error`) differ for any MR whose target moved or renamed files.
- Probe: base commit has `a.txt` (10 lines). Target branch: `git mv a.txt b.txt`. Source branch (branched
  from base): edit line 5 of `a.txt`. Create the MR and then `GET .../merge_requests/:iid`, `PUT
  .../rebase`, `PUT .../merge`. GitLab: `can_be_merged`, the rebase works, the merge result has `b.txt` with
  the edit. Also a criss-cross case: two branches each merged into the other, then edited.

### 2. Diff engine: no git indent heuristic, and a different rename score (V on code, U on effect)
- labrat: `crates/labrat-git/src/diff.rs` uses the `similar` crate's Myers diff for patches (`patch`,
  `diff_lines`, `line_stats`, lines 267 and 705). Renames are found by exact oid or by `similarity()`
  (`diff.rs:211`): common-line bytes divided by the larger size, at or above 50, with candidates ordered by
  score only. Git's xdiff has `diff.indentHeuristic` on and compaction of sliding hunks, and its rename
  scoring is based on hashed chunks. Git also breaks rename ties by basename.
- GitLab: Gitaly `git diff` / `CommitDiff` with `find_renames` (`lib/gitlab/git/diff.rb`,
  `lib/gitlab/git/diff_collection.rb`).
- Observable: for ambiguous hunks (a repeated block added or removed, for example a function inserted next
  to similar functions), added/removed/context line classification and `@@` headers differ. That changes the
  `diff` text, `old_line`/`new_line`, `line_code`, `diffStats` and whether a position is "on a diff line".
  `renamed_file` can differ for 50 to 70 % similar files, and for several identical files.
- Probe: file with two identical 3-line blocks separated by a blank line. Source branch duplicates the
  block (so insertion can slide). `GET .../merge_requests/:iid/diffs`, compare `diff` and the header.
  Then `POST .../discussions` with `position[new_line]` on a slid line. Second probe: delete `x.txt` (100
  lines), add `y.txt` that keeps 55 of its lines plus 100 new ones, compare `renamed_file`.

### 3. Webhook delivery: one attempt, no retries, no auto-disable state (V)
- labrat: `crates/labrat-api/src/hooks.rs:120` (`send`) does one POST with a 10 s timeout and logs it. There
  is no retry. `crates/labrat-api/src/routes/hooks.rs:54-55` hard-codes `alert_status: "executable"` and
  `disabled_until: null`. `recent_failures` and backoff do not exist.
- GitLab: `app/workers/web_hook_worker.rb:11` (`retry: 4`), `app/services/web_hook_service.rb`, and
  `app/models/concerns/web_hooks/auto_disabling.rb` (threshold 3, temporary then permanent disable). Hook
  JSON shows `alert_status` `temporarily_disabled` or `disabled` and a real `disabled_until`.
- Observable: a receiver that is down for a short time, or returns 5xx, is retried by GitLab and not by
  labrat. After repeated failures GitLab stops delivering and `GET /projects/:id/hooks[/:hook_id]` shows the
  status; labrat keeps firing.
- Probe: create a project hook to a receiver that returns 500; trigger 5 MR events; `GET
  /projects/:id/hooks/:id` (`alert_status`, `disabled_until`) and `.../events` (count of log rows per
  event, the retry spacing). Record a receiver that fails once and then succeeds.

### 4. GraphQL `MergeRequest.taskCompletionStatus` is always 0/0 (V)
- labrat: `crates/labrat-api/src/graphql/resolvers.rs:1713` returns `{count: 0, completedCount: 0}`. The REST
  entity does compute it (`entities/merge_request.rs:121`, `task_status`).
- GitLab: `app/graphql/types/merge_request_type.rb:233` through `Taskable#task_completion_status`
  (`app/models/concerns/taskable.rb:119`).
- Observable: any GraphQL client that shows task progress.
- Probe: create an MR with description `- [x] a\n- [ ] b\n1. [ ] c`; query `mergeRequest(...) {
  taskCompletionStatus { count completedCount } }`. GitLab: 3 and 1. Also `GET .../merge_requests/:iid`
  `task_completion_status` for comparison.

### 5. `Note.authorIsContributor` is always false (V)
- labrat: `resolvers.rs:2379`.
- GitLab: `app/graphql/types/notes/note_type.rb:68` calls `Note#contributor?`, which calls
  `ProjectTeam#contributor?` (`app/models/project_team.rb:257-275`): true for a non-member (below Guest)
  with a merged MR on the default branch in the project.
- Observable: review tools that show a "Contributor" badge.
- Probe: public project; user U (not a member) opens an MR that is merged into the default branch; U then
  comments on a second MR; query `notes { nodes { authorIsContributor maxAccessLevelOfAuthor { stringValue }
  } }`. GitLab: `true` for U's notes. A member author gives `false`.

### 6. Draft detection from pushed commits and repeated draft prefixes (V)
- labrat: `crates/labrat-core/src/services/merge_requests/refresh.rs:383` marks the MR as draft only if the
  commit title starts with `fixup! ` or `squash! ` (a single space). `merge_requests/mod.rs:243-262`
  (`is_draft_title`, `draftless_title`) strips every repeated prefix in a loop.
- GitLab: `app/models/commit.rb:549` `DRAFT_REGEX = /\A\s*<draft>|(fixup!|squash!)\s/` (second alternative
  is unanchored and `\s` is any whitespace). `app/models/merge_request.rb:916-924` removes one prefix with
  `sub` (the `\A` inside the `+` group stops a second match). `lib/gitlab/regex/merge_requests.rb:10`.
- Observable: `draft`, `title`, `detailed_merge_status: draft_status`, system note `marked this merge
  request as **draft** from <sha>`, `/ready` and `draft=false` results.
- Probe: push a commit titled `Tidy (squash! later)` to an open MR's source branch (GitLab: becomes
  `Draft: ...`). Then `PUT` an MR titled `Draft: Draft: x` with `draft=false` or `/ready` (GitLab leaves
  `Draft: x` and it is still a draft).

### 7. MR `/changes` `overflow` is always false and the diff collection has no max limits (V)
- labrat: `routes/merge_requests.rs:1178` inserts `overflow: false`. `entities/repository.rs:192-199`
  models only the safe limits (100 files, 5000 lines, 512000 bytes, per-file 200 KB). The 1000-file marker
  exists only on the stored version row (`services/merge_requests/diffs.rs:149`).
- GitLab: `lib/api/entities/merge_request_changes.rb` (`overflow?`), `lib/gitlab/git/diff_collection.rb`
  (`overflow?`, `overflow_max_files?`, `overflow_max_lines?`, `overflow_max_bytes?`) with the hard limits
  `max_files`/`max_lines`/`max_bytes`.
- Observable: `GET .../changes` and `/diffs` on a large MR return every file in labrat and a truncated list
  with `overflow: true` in GitLab. `changes_count` is "1000+" in both.
- Probe: branch that adds 1,100 small files (and a variant with one 60,000-line file); `GET
  .../merge_requests/:iid/changes` and `.../diffs?per_page=100&page=11`.

### 8. `commit_id` on MR diff notes is dropped (V)
- labrat: `routes/notes.rs:595` reads `commit_id` only for draft notes and stores it in the draft row.
  `services/reviews.rs:444` (`publish_one`) does not pass it to the published note, and the discussion
  create path has no `commit_id` input (`services/comments.rs` `CreateParams`). `entities/note.rs:42`
  always shows the stored column (null).
- GitLab: `lib/api/discussions.rb:142` (`opts[:commit_id] = params[:commit_id]` for MR `DiffNote`),
  `app/models/draft_note.rb:10` (`DIFF_ATTRS` includes `commit_id`), `lib/api/entities/note.rb:21` exposes
  `commit_id` on MR diff notes.
- Observable: the `commit_id` field of notes and discussions created with a commit scope (GitLab's
  "single commit" review mode, some review bots).
- Probe: `POST .../merge_requests/:iid/discussions` with `body`, a valid `position[...]` and
  `commit_id=<sha of a commit in the MR>`; read `notes[0].commit_id`. Same through `POST .../draft_notes`
  with `commit_id` then `.../draft_notes/bulk_publish`.

### 9. GraphQL mutations and roots that return "not implemented in labrat" (V)
- labrat: `resolvers.rs:3122`, `:3268`, `:3563` and `:438` fall through to `not_implemented(...)` for
  `createBranch`, `branchDelete`, `branchRuleCreate/Update/Delete`, `branchRuleSquashOption*`,
  and root fields such as `Query.timelogs`, `Query.issue`, `Query.snippets`.
- GitLab: `app/graphql/mutations/repositories/branches/create.rb`, `.../delete.rb`,
  `app/graphql/mutations/branch_rules/*.rb`, `app/graphql/resolvers/timelog_resolver.rb`.
- Observable: a GraphQL-only bot that creates a branch, then an MR, gets a field error instead of data.
- Probe: `mutation { createBranch(input:{projectPath:"g/p", name:"b1", ref:"main"}) { branch { name }
  errors } }`; `mutation { branchDelete(input:{projectPath:"g/p", name:"b1"}) { errors } }`; `{ timelogs {
  nodes { id } } }`.

### 10. Rebase replay order and merge commits (U)
- labrat: `services/merge_requests/merge.rs:605-614` takes `rev_list` (committer-date order) and reverses
  it, then `labrat-git/src/merge.rs:264` (`rebase`) replays each commit's diff against its first parent. A
  merge commit is replayed as a single cherry-pick of its first-parent diff. Commits with clock skew
  replay in date order, not topological order.
- GitLab: Gitaly `UserRebaseConfirmable` (git rebase): linear topological order, merge commits dropped.
- Observable: the content and order of commits on the source branch after `PUT .../rebase`.
- Probe: source branch with commits C1, C2 where C2's committer date is earlier than C1's (use
  `GIT_COMMITTER_DATE`), plus one variant containing a merge of the target into the source; `PUT
  .../rebase`, then `GET .../commits` and compare SHAs, order and tree.

### 11. Retarget chain after merge skips the merger's permission check and `updated_by` (V)
- labrat: `merge.rs:515` updates dependent MRs with a raw `UPDATE` (target branch, `retargeted`,
  `updated_at`), for up to 4 MRs, with no check of the merger's rights and no `updated_by_id`.
- GitLab: `app/services/merge_requests/retarget_chain_service.rb:22` skips MRs where the merger cannot
  `update_merge_request`, then runs `UpdateService`, which sets `updated_by`, resolves the merger's
  todos and runs the "target branch was changed" auto-merge abort.
- Observable: `updated_by`, todos and hook `changes` of the child MR; the child is untouched in GitLab
  when the merger lacks rights.
- Probe: MR A (feature-a to main) and MR B (feature-b to feature-a) in a project where the merger can
  merge A but is not allowed on B (B's source in a branch the merger cannot update, for example B authored
  by a user and the merger is only Reporter on the child, or B in a different state); merge A with
  `should_remove_source_branch=true`; read B (`target_branch`, `updated_at`, `merge_user`) and its
  system notes.

### 12. 200 assignee/reviewer cap not enforced (V)
- labrat: no such check (no `must be less than or equal to` text anywhere in the crates).
- GitLab: `app/models/concerns/issuable.rb:37,170-173`; also in `notes/create_service.rb` for quick
  actions.
- Observable: only with more than 200 ids; low likelihood. Only one reviewer or assignee is kept, so
  GitLab's check runs before the truncation.
- Probe: `PUT .../merge_requests/:iid` with `reviewer_ids` of 201 ids (or `assignee_ids`).

### 13. Cross-reference notes cover only same-project `!N` and MR URLs (V)
- labrat: `services/cross_refs.rs:13` (`referenced_iids`) matches `!N`, `path!N` and the MR URL for this
  project only. A commit message reference creates `mentioned in commit <sha>` on the MR; text in an MR
  or note creates `mentioned in merge request !N`.
- GitLab: `app/models/concerns/mentionable.rb` and `lib/gitlab/reference_extractor.rb` resolve every
  reference type. Missing here: a commit SHA in an MR description or note (GitLab adds `mentioned in merge
  request !N` to the commit, visible in `GET .../repository/commits/:sha/comments`), `!N+`/`!N+s`
  suffixes, `/-/merge_requests/N` URLs with a trailing path, and the commit-note side. Issue references
  are out of scope (no issues).
- Observable: system notes on MRs and on commits.
- Probe: MR description containing a full or short SHA of a commit in the project; `GET
  .../repository/commits/:sha/comments` and `GET .../discussions` on the MR. Also a note body `see !2+s`.

### 14. GraphQL arguments accepted and ignored (V)
- labrat: `Project.labels` (`resolvers.rs:905`) uses only `searchTerm`, matching titles only; `title`,
  `searchIn`, `archived` (default false) and `includeAncestorGroups` (default false) are ignored.
  `MergeRequest.pipelines` and `Project.pipelines` ignore `scope` and `refType`. `Project.mergeRequests`
  and `Group.mergeRequests` ignore `releaseTag`, `blobPath` (GitLab raises "targetBranches field must be
  specified..."), `deployedAfter`, `deployedBefore`, `deploymentId`, `environmentName`.
- GitLab: `app/graphql/resolvers/labels_resolver.rb`, `app/graphql/resolvers/merge_requests_resolver.rb`,
  `app/graphql/resolvers/concerns/resolves_merge_requests.rb:69-86`.
- Observable: a label search by description, or an archived label, returns different rows; `blobPath`
  without the required args returns data instead of an error.
- Probe: `labels(searchTerm:"alpha")` where "alpha" appears only in a label description;
  `mergeRequests(blobPath:"README.md")`; `mergeRequests(environmentName:"production")` (GitLab: empty).

### 15. Position tracing across renames and `line_range` (U)
- labrat: `services/positions.rs:253` (`trace`) reads the same `new_path`/`old_path` at the new refs. A file
  renamed between versions is treated as changed (outdated). `line_range` is carried over unchanged.
- GitLab: `lib/gitlab/diff/position_tracer.rb` with `line_strategy.rb`, which diffs with rename
  detection (`ac_diffs`, `bd_diffs`). GitLab also keeps the old `line_range` (see `edge-cases-reviews.md`
  A7), so only the rename part differs.
- Observable: discussion `position`, `resolved` (with `resolve_outdated_diff_discussions`), and the
  `changed this line in version N` system note after a rename push.
- Probe: thread on `a.txt` line 3; push a commit that renames `a.txt` to `b.txt` without edits;
  `GET .../discussions` (position paths, `change_position`), and `GET .../notes` for the system note.

### 16. `Commit` GraphQL fields: `tags`, `signature`, `signatureHtml`, `pipelines` (V)
- labrat: `resolvers.rs:1803` `tags` is always `[]`; `:1832` `signature`, `signatureHtml` and `pipelines` are
  always null, although labrat does create pipelines from commit statuses.
- GitLab: `app/graphql/types/repositories/commit_type.rb` (`tags` from Gitaly, `signature` as `GpgSignature` or
  `SshSignature`, `pipelines` connection).
- Observable: `mergeRequest.commits.nodes { tags signature pipelines }`.
- Probe: tag a commit that is in an MR (`POST /repository/tags`); post a commit status to it; query
  `commits { nodes { tags pipelines { nodes { id } } } }`. For `signature`, push an SSH- or GPG-signed
  commit.

### 17. Applying a suggestion to a non-UTF-8 file blanks the file (V)
- labrat: `services/suggestions.rs:319` does `std::str::from_utf8(&data).unwrap_or("")`, so the
  replacement is applied to an empty line list.
- GitLab: `app/services/suggestions/apply_service.rb` / `Gitlab::Suggestions::FileSuggestion` work on bytes
  and keep the other lines.
- Observable: the committed file content.
- Probe: file with a Latin-1 byte (0xE9) on line 1 and plain text on lines 2-3; a suggestion on line 3;
  `PUT /suggestions/:id/apply`; `GET .../repository/files/:path/raw`.

### 18. Fixed values that GitLab would compute, outside CI (V)
Low likelihood but cheap to record:
- `MergeRequest.relativePosition` (`resolvers.rs:1712`), always null.
- `MergeRequest.hidden` (`:1533`), always false.
- `MergeRequest.inProgressMergeCommitSha` (`:1534`), always null. GitLab fills it while the state is
  `locked`.
- `first_deployed_to_production_at` (`entities/merge_request.rs:393`), always null (no deployments).
- `Project.forksCount`, `starCount`, `openIssuesCount` fixed at 0 (`resolvers.rs:965`); `isForked` false.
- Probe for `inProgressMergeCommitSha`: needs a merge in progress (state `locked`); hard to catch over
  REST. Check `merge_commit_sha` handling during `PUT .../merge` with a pre-receive delay.

## Not counted (one line each)
- CI never runs: `routes/pipelines.rs:269`, `resolvers.rs:2673`, `:3418`. Pipelines exist only as skipped
  or commit-status pipelines. Large parts of GraphQL `Pipeline` (`:2012-2021`) are fixed.
- Issues do not exist: `closes_issues`, `related_issues` always `[]` (`routes/merge_requests.rs:47-48`),
  `Milestone` issue connections, `Query.issue`.
- Forks do not exist: MRs are same-project; `allowCollaboration` fixed false (`resolvers.rs:1707`) is
  correct for same-project MRs (`MergeRequests::BaseService` drops the param, `base_service.rb:192`).
- Markdown references other than `@user` and `!N`: already in PLAN.md approximations.
- `@all` mentions: not a gap, `disable_all_mention` is on by default
  (`config/feature_flags/development/disable_all_mention.yml`).

## Suggested recording order
1, 4, 5, 6, 8, 9, 3, 7, 2, 10, 13, 14, 11, 15, 16, 17, 12.
Items 1 to 3 are the largest in impact and need git history setup, so they are recorded as multi-step
scenarios. Items 4, 5, 8, 9, 14 and 16 are one GraphQL or REST call after a small setup.
