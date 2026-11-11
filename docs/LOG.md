# Work log

This is a running log of the work on labrat. The newest entry comes first, so the top entry shows the
current state.

- **When to add an entry:** after each significant step. During active work, add one at least every
  2 hours, even if no step has finished. Skip it when nothing has changed.
- **What an entry contains:** what was done, what is in progress now, and what comes next.
  Mention a decision, a problem or a discrepancy found only if it changes what happens next. Link
  commits or files when that helps.
- **Time:** entries are timestamped in UTC.

## 2026-11-11 UTC

**Done since the last entry:**
- The project is renamed labrat and has its own repository. Crates, the binary, `LABRAT_*`
  environment variables and the `/-/labrat/` test endpoints use the new name.
- [MAINTAINING.md](MAINTAINING.md) explains how to set up a reference instance, record fixtures,
  write scenarios and move to a newer GitLab version. `scripts/reference/` holds the scripts it
  uses; `update-data.sh` reproduces the current schema and emoji files exactly.

## 2026-11-10 UTC

**Done since the last entry:**
- The reference is GitLab CE 18.11 (`~/dev/gitlab-run-ce`). All fixtures are recorded from it.
- The GraphQL schema labrat embeds is CE's, generated from the CE instance.
- Features and fields that CE doesn't have were removed: approval rules and settings, requested
  changes and blocking MRs, branch rule extras, and extra project, group and user fields. Tests,
  `docs/gitlab-ref` and the other docs were changed to match.
- `RemoveApprovalService` leaves merged MRs alone, so re-requesting a review there no longer
  reports a removed approval.
- A `/spend` in a note with text reports the note's touch as an `updated_at` change in the
  MR hook.
- The suite has 154 scenarios, all passing.

## 2026-11-08 15:00 UTC

**Done since the last entry:**
- A port of git's xdiff (`labrat-git/src/xdiff.rs`) replaces the `similar` crate's Myers. 70,000
  random cases match `git diff --no-index`, and `diff_ambiguity` records the ambiguous cases
  against GitLab.
- Merges follow renames (merge-ort behaviour).
- From the shortcuts audit ([labrat-shortcuts-audit.md](gitlab-ref/labrat-shortcuts-audit.md)):
  - `fixup!`/`squash!` anywhere in a commit title marks the MR as a draft;
  - GraphQL `taskCompletionStatus` and `Commit.pipelines` are filled in;
  - diff notes keep their `commit_id`;
  - mentioning a commit in an MR adds "mentioned in merge request !N" to the commit;
  - suggestions apply to non-UTF-8 files.
- GraphQL `createBranch`, `branchDelete` and `branchRuleCreate` are implemented.
- Webhook failures (no retries, no auto-disable in 18.11) and rebase edge cases already
  matched.
- The suite has 156 scenarios, all passing.

**Remaining (not started):**
- the 200-assignee/reviewer cap;
- `/changes` overflow caps for MRs with over 1,000 files;
- GraphQL arguments that are accepted but ignored: some `labels(...)` and
  `pipelines(scope, refType)` arguments;
- `branchRuleUpdate` and `branchRuleDelete`.
The documented approximations are in PLAN.md.

## 2026-11-08 13:35 UTC

**Done:**
- `pipelineCreate` and `POST /pipeline` in projects without CI configuration, and the CI and
  paid-tier settings echo (`pipeline_create_settings`).
- GraphQL "Did you mean" suggestions, ported from Ruby's `DidYouMean::SpellChecker`, on unknown
  fields, arguments, input keys, directives, fragment types and variable types
  (`graphql_did_you_mean`).
- GitLab's occasional "Timeout on validation of query" turned out to be a real 0.2 s timeout
  while it builds those suggestions, and it varies between runs. labrat gives the normal error
  instead (PLAN.md).
- Pushes to the target branch, including a force push that rewrites its history
  (`target_force_push`), already matched GitLab.
- Four more walks match GitLab.
- The suite has 149 scenarios, all passing.

- `random_walk_force_2` matches as well (150 scenarios).

**Next:** keep auditing the remaining review edge cases and record more walks.

## 2026-11-08 12:35 UTC

**Done:**
- Skipped pipelines are complete: pipeline webhooks, GraphQL queries, and `ref` filters that
  also match the MR refs of MRs from that branch.
- A subagent inventoried GitLab's MR and review features against labrat
  ([mr-feature-inventory.md](gitlab-ref/mr-feature-inventory.md)). Coverage is high; the
  first-tier gaps are now recorded and fixed:
  - Commit message templates now follow GitLab's MessageGenerator: merged_by, approved_by,
    reviewed_by, co_authored_by and the rules for empty placeholders.
  - MR descriptions drop CRs and trailing whitespace.
  - `git push` prints the same remote messages, and the push options behave the same: assignees,
    and option updates applied before the MR refresh.
  - `remove_source_branch_after_merge` and locked discussions after a merge behave as in GitLab.
    So do `resolve_outdated_diff_discussions`, which now adds "resolved all threads", and several
    GraphQL arguments.
- Four more random walks match GitLab.
- The suite has 144 scenarios, all passing.

**Next:** the inventory's remaining items, such as GraphQL `pipelineCreate` and the `ci_*`
settings echo, and more walks.

## 2026-11-08 10:40 UTC

**Done:**
- Comments on whole files now go out of date as GitLab's FileStrategy decides
  (`diff_position_variants`). A fuzz run of 400 review requests found no crashes.
- `merge_status_matrix` records:
  - blockers removed one at a time over a failing status;
  - every commit-status state;
  - LFS turned off;
  - list rechecks.
  Fixes: the LFS check is inactive when LFS is off; group lists keep "unchecked" while they
  recheck; clearing `merge_after` with null answers with the stale schedule.
- A subagent audited the parameters of the MR and review endpoints
  ([mr-endpoint-params-audit.md](gitlab-ref/mr-endpoint-params-audit.md)). `mr_list_params`
  and `mr_action_params` cover the plausible gaps:
  - wildcards for `my_reaction_emoji` and milestones;
  - validation errors;
  - `merged_at` ordering;
  - archived projects;
  - deployment filters;
  - empty parameters;
  - async pipelines.
- Skipped pipelines (`skip_ci_pipelines`). Commits with `[skip ci]` or pushes with `ci.skip`
  create skipped branch and MR pipelines, as GitLab does even without CI configuration. These
  pipelines feed `head_pipeline` and the merge checks.
- Four more random walks match GitLab.
- The suite has 138 scenarios, all passing.

**Next:** webhooks for skipped pipelines, and more audits of review flows.

## 2026-11-08 08:35 UTC

**Done:**
- The push audit ([push-side-effects-audit.md](gitlab-ref/push-side-effects-audit.md)) is
  recorded as `push_side_effects` and `push_side_effects_2`. Fixes:
  - commit titles in system notes are truncated as GitLab does, with "No commit message" for an
    empty one;
  - the draft note uses the full SHA;
  - an MR that becomes unmergeable through a conflict gives its author a to-do;
  - resolving a to-do clears its snooze;
  - a merge by push records its source MR;
  - the auto-merge abort note links to GitLab's own help page.
- GraphQL now applies the static validation rules labrat lacked (`graphql_static_validation`), and
  GitLab's 10,000-character query limit.
- A local fuzz run against labrat found ways a single request could crash or hang it. GitLab
  never saw these requests. All are fixed and recorded:
  - a fragment cycle overflowed the stack and aborted the process;
  - a fragment bomb took exponential time;
  - 20,000 arguments, directives or variables took quadratic time;
  - extreme `page` values overflowed (`pagination_extremes`);
  - a negated duration overflowed.
- Duration parsing and formatting is now a port of GitLab's ChronicDuration
  (`labrat_core::chronic`), with years and months. It has GitLab's limits: estimates are capped
  at 2^31-1, spent time at 4 years in total and never below zero (`time_tracking_extremes`).
- Request bodies may be up to 250 MB, as with Omnibus nginx.
- One more random walk and a state-only GraphQL update fix.
- The suite has 130 scenarios, all passing.

**Next:** more fuzzing of the review endpoints (drafts, suggestions, positions), and more walks.

## 2026-11-08 07:15 UTC

**Done:**
- `draft_quick_actions`: drafts that contain commands, published one at a time and as a review.
- A second audit, of GraphQL mutation errors
  ([graphql-mr-error-coverage.md](gitlab-ref/graphql-mr-error-coverage.md)), recorded as
  `graphql_mutation_errors`. labrat now validates variable values by type (enums, scalars,
  input objects, lists, GlobalID models). There are about a dozen mutation fixes, including
  "Cannot assign", "No object found", state transitions and system notes.
- Two more random walks match GitLab. One of them found the unvalidated enum variables.
- `closed_mr_updates`: closed and merged MRs can be retargeted and keep
  `remove_source_branch`, as in GitLab. GitLab only drops these for forks. A merged MR whose
  status was reset becomes `cannot_be_merged`.
- The suite has 124 scenarios, all passing.

**Next:** more audits and walks. Candidate areas are MR lists and filters under concurrent
changes, and approval and reviewer state across pushes.

## 2026-11-08 05:55 UTC

**Done:**
- Reverting and cherry-picking an MR's merge commit now follows GitLab: "This reverts merge
  request !N", the MR's commits listed in the cherry-pick message, and the "picked the changes into
  the branch" note.
- A subagent listed the MR-area error responses that no fixture covered
  ([mr-api-error-coverage.md](gitlab-ref/mr-api-error-coverage.md)). `api_error_paths` and
  `api_error_paths_2` record about 50 of them. Fixes that came out of them:
  - `batch_apply` without `ids` crashed labrat;
  - unreadable MRs are "404 Not found";
  - 409 for a duplicate MR wins over other validation errors;
  - the fork-only `target_project_id` errors;
  - the empty award name message;
  - "MiB" in size limits;
  - a denied draft note is a 500, a GitLab bug (`access_denied!` is undefined in the draft notes
    API).
- After a merge, the refresh of the other MRs targeting the branch is a background job, as in
  GitLab.
- The suite has 119 scenarios, all passing.

**Next:** more random walks, and draft notes that contain quick actions.

## 2026-11-08 05:15 UTC

- **Done:**
  - Quick actions in MR descriptions, on create and update, through REST and GraphQL (61edaef).
  - A subagent audit of all quick actions, mostly confirmed by quick_action_audit (54e41fb):
    - commands of other types are recognised;
    - argument parsing for users, labels and durations;
    - the remaining messages.
  - `/subscribe` and `/unsubscribe` are evaluated on the state before the note is saved.
  - Found a race inside GitLab: an MR edit's `ResolveTodosService` and the self-mention to-dos
    it creates. Documented in PLAN.md; the second note walk avoids it.
- **Next:** further walk seeds and areas: draft notes with quick actions, MR edits by
  non-members, Reporter-level reviewers.

## 2026-11-08 03:45 UTC

- **Done:**
  - random_walk_mixed_graphql, the mixed walk through GraphQL mutations: `/done` resolves a
    single to-do, and literal input objects with missing required fields fail validation
    (2e86c61, 4401233).
  - Quick actions, in three scenarios:
    - parsing (43d4dff): `>>>` blocks, tabs, repeated `/label`;
    - repeated and conflicting commands in one note (f0891a0);
    - the webhooks each command fires (2852b15): `/estimate` through the MR update, and
      note hooks with the MR as it was before the commands.
- **Next:** quick actions in MR descriptions (create/update), then more walk seeds.

## 2026-11-08 02:40 UTC

- **Done:**
  - random_walk_mixed: unavailable quick actions report "Could not apply" instead of failing
    the note (692d8ea). A second seed matched on the first recording (83e4dd2).
  - The remaining GraphQL fields of MR-related types: commit diffs, tree last commit, user
    settings fields, commit-status pipeline jobs and stages, and the milestone report's
    complexity (5eb747b). Project autocomplete users, root group and languages (b9300e3).
  - random_walk_mixed_hooks, the mixed walk with all webhooks:
    - a quick action that only closes or reopens saves nothing else, so there's no
      `update` hook and no `updated_by`;
    - `/unassign_reviewer` ignores its arguments and removes every reviewer.
- **Next:** more walk seeds and variants (hooks with a Reporter, GraphQL mutations in the mix).

## 2026-11-08 01:25 UTC

- **Done:**
  - push_followups: push-option MR titles from the first multi-line commit, draft MRs for
    branches without commits, `draft=false` ignored on update, push processing before push
    options, and merge commits of manual merges (95b62c3).
  - stacked_mrs: merging a parent drops its children's auto-merge, with a note and an
    unmergeable to-do, and the retargeted child loses its head pipeline (4b8406f).
  - Failed background jobs now fail scenarios (9dd18e4).
  - random_walk_notes: self-mentions make to-dos, MR edits resolve the editor's to-dos,
    `/subscribe` follows implicit subscriptions, and participants follow note `created_at`.
    Replay now runs due jobs when the clock jumps, which made the order of asynchronous
    approval notes reproducible (6bf5717).
  - Measured, in reply to a question: the labrat release binary is 22.5 MB stripped, and uses
    about 14 MB of RAM idle and about 31 MB after 200 MRs. The GitLab container uses about
    2.5 GiB.
- **In progress:** random_walk_mixed, a 60-step walk over all MR operations.

## 2026-11-08 00:40 UTC

- **Done:**
  - random_walk_reviewers: three-reviewer review cycles. It found that `/user_counts` MR counts
    are cached for 24 hours and invalidated only for specific users, and that the to-do count
    has two refresh paths (snoozed to-dos counted or not). Both are modelled (4889c52, cce5463).
  - Recordings now replay at their own date, so a midnight rollover no longer breaks fixtures
    with day-dependent values (508cfe4).
  - A subagent audit of GitLab's MR services against labrat
    ([gitlab-ref/mr-services-audit.md](gitlab-ref/mr-services-audit.md)). audit_followups_3
    confirmed and fixed:
    - "mentioned in merge request !N" cross-reference notes, which don't touch the MR;
    - `@group` mentions (to-dos and participants);
    - pushes resolving the pusher's to-dos on the MR;
    - throttled note touches, and no touch on note deletion;
    - `/copy_metadata`.
- **Next:** the remaining audit items: push options (title from commits, `draft=false`),
  stacked-MR auto-merge aborts, merge commit SHAs of manual merge pushes, and the 200-user cap.

## 2026-11-07 23:40 UTC

- **Done:**
  - Protected tags, blame (following renames), contributors, and group label and milestone
    CRUD (4014e1f, 6e63c9b).
  - random_walk_force: review threads, suggestions and drafts while the source branch is
    amended, force-pushed and rebased with git. Positions, outdated threads and versions
    already matched GitLab. The normalizer now maps short SHAs of intermediate commits in
    "added N commits" notes.
- **Next:** more walks (multi-reviewer review cycles with re-requests), and a review pass
  over the remaining endpoint gaps.

## 2026-11-07 22:55 UTC

- **Done:**
  - Group access tokens (9db8758).
  - GraphQL `commitCreate`, `labelUpdate` and image note updates. Null elements of `[T!]` lists and execution-error columns now match
    graphql-ruby (2e4766d).
  - A subagent audit of REST coverage against GitLab's Grape files
    ([gitlab-ref/missing-rest-endpoints.md](gitlab-ref/missing-rest-endpoints.md)): the MR
    core has no gaps, the gaps are in the surrounding API.
  - From that list: `POST /markdown`, `GET /projects/:id/users`, `/user_counts`, user status,
    `/namespaces`, the `-/search` routes and token rotation (client_utility_endpoints).
  - A to-do walk (random_walk_todos) found that award givers and users mentioned in the title,
    description or any note (system notes too) are participants, and so implicitly subscribed.
    participants_sources recorded GitLab's participant order. REST, GraphQL and `subscribed`
    now share one implementation.
- **Next:** more items from the missing-endpoint list (protected tags, blame, archive,
  contributors, group label and milestone CRUD), and more walks.

## 2026-11-07 22:10 UTC

- **Done:**
  - Scalar-field sweeps of `Project`, users, interactions, pipelines and both permission types,
    for every role (7427e78, 8a8599b).
  - GraphQL query complexity, depth and recursion limits, calibrated against GitLab's own
    `queryComplexity` scores for 23 typical MR client queries (8ce2a95).
  - Review comments on single commits and old versions, which already matched.
  - Project access tokens and bot users (ca0516b).
  - Webhook template validation and URL-variable rules (ac9147e).
  - Shallow clones, fetches and pushes over smart HTTP (121606b).
- **Next:**
  - A walk over MR lists: many MRs in varied states, compared through list filters and sort
    orders.
  - Remaining "Later" items as they matter for MR clients (protocol v2, group access tokens).

## 2026-11-07 21:20 UTC

- **Done:**
  - `mr_diff_git` (git-pushed symlinks, submodules and type changes; version lifecycle) splits
    type changes into delete plus add (89dab21).
  - Merge walks (`random_walk_merge`, `_2`) under changing project merge settings found the
    auto-merge availability rule (f6f6957).
  - Sweeps over every scalar field of GraphQL `MergeRequest`, `Note`, `Discussion` and
    `DiffPosition` replaced stubbed fields:
    - `availableAutoMergeStrategies`, `retargeted` and `divergedFromTargetBranch`;
    - `descriptionHtml`, `bodyHtml` and `bodyFirstLineHtml` with GitLab-flavoured Markdown,
      `@user` and `!N` references, task lists and code blocks;
    - `lastEditedAt`.

    (1037f6a, 1f7f21f, 87938a5)
  - Found the intermittent suite hang: a stale job-worker registry entry when an app reused a
    dropped one's address.
- **Next:** the same field sweep for `Project`, `UserCore`, `Pipeline`, reviewers and
  approvals; more walks.

## 2026-11-07 18:50 UTC

- **Done:**
  - Recordings also store when GitLab answered each step, and replay syncs labrat's clock to that
    time (2142932).
  - GraphQL walks with two and three users, the third a Reporter, found the request-changes
    rules ("Reviewer not found", approval kept) and the convert-to-thread rule (6e22763,
    ec10c0a).
  - A REST hooks walk with a Reporter found `/todo` and `/done` conditions evaluated before the
    note is saved.
  - A diff audit (`docs/gitlab-ref/edge-cases-mr-diffs.md`) and its recording `mr_diff_edges`
    brought GitLab's collapse rules for `/diffs`, `/changes` and compare, `generated_file`
    (`.gitattributes` with git's wildmatch), Rails' binary notice for patches with NUL bytes,
    dropped invalid UTF-8, git path quoting, `diffStats` order, and the pre-receive LFS
    pointer check (ae147d0).
  - Each new walk or audit still finds one to three real differences.
- **Next:**
  - Diff recipes not recorded yet: symlinks, submodules and type changes through git push,
    and the version lifecycle (target pushes, empty commits).
  - More walks over areas the walks rarely reach: auto-merge with pipelines and branch
    deletion.

## 2026-11-07 16:55 UTC

- **Done:**
  - Step-time replay in the conformance harness. Recordings store when each step was sent, and
    the check run moves that labrat instance's own clock forward to match, so GitLab's
    once-a-minute MR touch (notes, labels, assignees, unapprove) replays faithfully.
  - From the timestamp audit (`docs/gitlab-ref/edge-cases-mr-timestamps.md`) and the
    `mr_touch_rules` recording: association changes don't touch the MR except through the
    throttle.
  - From `random_walk_hooks_3`: `/spend` goes through the update (timelog, note, hook,
    `spent_at`/`note_id`); re-requesting a review via `/assign_reviewer` happens immediately and
    is followed by an update; rebasing an up-to-date branch keeps its SHA; squash messages end
    with a newline.
  - `docs/overview.html` explains the project (db99e0a).
- **In progress:** re-recording `random_walk_hooks`. A step landed 60.1 s after a merge, which
  is ambiguous at the one-minute boundary; the previous attempt hit GitLab 502s (Puma restart).
- **Next:** commit this batch once the suite is green, then more walks and audits.

## 2026-11-07 15:05 UTC

- **Done:** five random walks are now recorded and pass: `random_walk`, `random_walk_review`,
  `random_walk_hooks` and `random_walk_hooks_2`, where the hooks walks compare every webhook
  delivery. Fixes from them (8539950, 6fb7b97, e2b16b2):
  - `/ready` no-op;
  - diff-match-patch title diffs;
  - inline quick actions run before the note is saved;
  - `updated_by`/`updated_at` touch rules;
  - no-op set-reviewers/assignees;
  - commits to missing branches;
  - pipeline hook fields;
  - `/unlabel` availability;
  - the "from branch main" commit summary.
- **Done:** the rest of the pipeline API for commit-status pipelines (jobs, bridges, variables,
  test reports, latest, metadata, cancel, retry, delete).
- **In progress:** recording `random_walk_hooks_3` and `random_walk_review_2`. A source audit of
  which operations touch the MR's `updated_at`/`updated_by_id`/`last_edited_*` is running.
- **Next:** fix what those find; keep adding walks while they keep finding differences.

## 2026-11-07 13:50 UTC

- **Done:** GraphQL `Project.repository` (tree, blobs, commit, lastCommit, branchNames),
  `projectMembers`, `pipelines`/`pipeline`, `branchRules`; root `echo`, `groups`, `milestone`,
  `todo`; more fields on users, commits, pipelines, milestones and groups (73011d3, d056478,
  40aefc0).
- **New: a random-walk scenario** (`random_walk`): 40 seeded MR/review operations by two users,
  checking full state along the way. It found real differences, now fixed:
  - `/ready` on a non-draft MR leaves `wip_event` out of `commands_changes` (`/draft` doesn't).
  - Title-change notes use diff-match-patch with semantic cleanup; labrat now uses `dissimilar`
    (a port) instead of an approximation.
- **In progress:** re-recording the walk with the MR's mergeability settled before each step
  (GitLab occasionally leaves it unchecked from late background work; not reproducible alone).
- **Next:** more walks with other seeds and operations (drafts/reviews, suggestions, merge),
  then the remaining low-impact audit items.

## 2026-11-07 12:40 UTC: audits of REST parameters and GraphQL fields

**Done** (each checked against a new recording unless noted)
- REST parameters:
  - quick actions in MR descriptions (REST and GraphQL);
  - discussion parameters;
  - position type checks;
  - lenient `created_at`;
  - stable `merge_ref`;
  - a single auto-merge note;
  - `If-Unmodified-Since` on deletes (from source).
- GraphQL fields:
  - participants order and contents;
  - `approvedBy` order;
  - `mergeUser`, `divergedFromTargetBranch`, `hasCi`;
  - pipeline `type`/`latest`;
  - the squash template;
  - anonymous permissions;
  - `removeSourceBranch`;
  - `commitEmail`;
  - namespaces and groups;
  - system note icons (from source);
  - note permissions per policy;
  - page caps;
  - commit title truncation and favicon digests (from source).
- Archived projects are read-only for MRs (REST approvals still pass, as on GitLab).
- To-do snoozing and bulk mutations; `unmergeable` to-dos when auto-merge aborts.
- Search refinements: group milestones, full paths, sort fallbacks, users.
- Endpoints:
  - group milestones and labels;
  - label and milestone promotion;
  - commit sequence/signature;
  - `approval_settings`;
  - description templates.
- 50 conformance scenarios pass.

**Now:** the GraphQL audit's remaining low-impact items:
- pagination edge cases;
- cursor shapes per sort;
- `lastActivityOn`.

**Next:** keep alternating source audits and recordings on whatever is still unverified.

## 2026-11-07 11:50 UTC: webhooks, uploads, more GraphQL mutations

**Done**
- Webhook audit (`docs/gitlab-ref/edge-cases-webhooks.md`), with fixes checked against new
  recordings:
  - time tracking and re-requested reviews send MR `update` hooks with GitLab's `changes`
    (including the add-spent-time quirk);
  - reviewer states in `changes`;
  - emoji hooks from GraphQL and `/react`;
  - real regex branch filters (invalid ones refused);
  - the push hook limit (3 refs);
  - empty `change_position` on diff notes and `resolved_by_push` null until resolved;
  - the `repository_update` system hook.
- Markdown uploads (`/projects/:id/uploads`, multipart requests), also served in the UI.
- GraphQL `createLatestDiffNote`, image diff notes, `labelCreate`, time logs
  (`timelogCreate`/`timelogDelete`/`timelogs`), and `mergeRequestAccept(strategy:)`.
- Archived projects in search, group MR lists and GraphQL.
- Note search ordered by `updated_at`, with internal notes for Reporters.
- Schema version 7. 42 conformance scenarios and 3 client tests pass.

**Now:** remaining low-impact items:
- to-do snooze mutations;
- `unmergeable` to-dos;
- webhook recursion detection;
- group milestone search.

**Next:** another source exploration round, this time on REST MR endpoints' parameter edge
cases (approvals, merge, rebase), then keep going down the gap lists.

## 2026-11-07 11:10 UTC: source-audit fixes

**Done**
- Two cheap source audits (`docs/gitlab-ref/edge-cases-*.md`) listed differences in to-dos,
  search, GraphQL filters and commit notes. Each fix was checked against a new recording first;
  a few audit claims turned out wrong (for example, REST `mark_as_done` really does ignore
  `project_id`).
- To-dos:
  - mentions in descriptions and edited notes;
  - thread-scoped resolution;
  - one pending to-do per action;
  - close and merge resolve only the actor's to-dos;
  - REST note bodies and URLs;
  - GraphQL `targetUrl`/`target`/`note`.
- GraphQL:
  - `mergeAfter` / `mergeRequestAccept(strategy:)` / `timeEstimate`;
  - `mergeabilityChecks` returns all 18 checks;
  - variable nullability/type checks;
  - nested input literal checks;
  - reviewer and `or` filter semantics;
  - `ignoredReviewerUsername`.
- Commit comments:
  - no replies to plain comments;
  - line comments are `LegacyDiffDiscussion`s;
  - note hooks, "commented on" events and mention to-dos targeting the commit (schema 5).
- Search:
  - authentication first;
  - blank queries;
  - `#N` iid lookups;
  - GitLab's short-term rule.
- The conformance scenarios now build into one test binary. 23 binaries had filled the disk
  again (10 GB); `target/` is now under 1 GB. The disk has about 8 GB free.
- 37 conformance scenarios pass.

**Now:** remaining audit items (archived projects in search, group milestone search, to-do
snooze mutations, `build_failed`/`unmergeable` to-dos).

**Next:** README/PLAN refresh for today's additions, then another round of source exploration
on webhook payloads for the newer events.

## 2026-11-07 10:15 UTC: coverage-gap items

**Done** (each against new recordings from the 18.11 instance)
- Scheduled merges: `merge_after` (REST and GraphQL), the `merge_time` check, and a scheduler
  like `ProcessScheduledMergeWorker` that merges auto-merge MRs once the time has passed.
- GraphQL `Project.mergeRequests` and new `Group.mergeRequests`: the full argument set, via the
  REST finder, as GitLab does.
- `MergeRequest` GraphQL fields: commenters, committers, currentUserTodos and branch fields.
- To-dos: GraphQL mutations, `currentUser.todos`, full MR entity in to-do targets.
- Commit discussions; MR context commits; pipeline creation answers (no CI); the search API for
  MR-related scopes.
- Fixes found on the way:
  - notes touch their MR (throttled to once a minute);
  - non-null errors name the parent type;
  - method mismatch is 404, not 405;
  - removing a key from a response keeps the key order.
- Schema version 4. 32 conformance scenarios pass.
- **Mistake:** commit `1253323` registered a route twice, which made the router panic. My test
  filter didn't catch panics; `d0928e8` fixes it. Test output is now also checked for `panicked`
  and `FAILED`.

**Now:** working down the rest of `docs/gitlab-ref/coverage-gaps.md`.

**Next:**
- GraphQL `createImageDiffNote` and the remaining mutation inputs (`timeEstimate`,
  `overrideRequestedChanges`, the accept `strategy`).
- `MergeRequest.pipelines` arguments.
- A source exploration of edge cases in the features added today.

## 2026-11-07 09:25 UTC: events, docs, UI reviews, coverage audit

**Done**
- Contribution events API (`/events`, `/users/:id/events`, `/projects/:id/events`), recorded for
  pushes, project creation, members, MR state changes, approvals and comments.
- README rewritten with usage, test endpoints and the fidelity approach;
  `docs/discrepancies.md` created with six docs-vs-instance entries.
- Web UI reviews: draft comments, a pending-review box with submit (comment / approve / request
  changes), and "Apply suggestion".
- Clippy's mechanical fixes applied.
- A coverage audit against GitLab's source (`docs/gitlab-ref/coverage-gaps.md`) lists the missing
  MR-related endpoints, params and GraphQL members, with a top-15 priority list.
- From that list: MR list `not[...]` for all keys, `source_project_id`, `merge_user_*`,
  `approved_by_*`, real `label_priority`/`priority`/`milestone_due`/`popularity` orders;
  `GET /repository/commits/:sha/merge_requests`; `unidiff` on `/changes` and `versions/:id`.
- The normalizer now maps user/project ids in query strings. 25 conformance scenarios pass.
- Release build timings: a 100-MR batch takes 0.4 s; GET MR takes 2 ms.

**Now:** working through the coverage priority list.

**Next:** `Project.mergeRequests` GraphQL arguments,
`Group.mergeRequests`, `merge_after`, MR GraphQL fields (`committers`, …), and
commit discussions.

## 2026-11-07 08:52 UTC: diffs, push options, commit statuses

**Done**
- Diff fidelity:
  - MR diffs use GitLab's directories-first order;
  - `collapsed`/`too_large` follow each view's limits;
  - binary notices and empty files match `git`;
  - `unidiff=true` is supported.
- Request log: the last 500 API calls per instance, viewable at `/-/labrat/requests` in the UI or as
  JSON.
- `git push -o merge_request.*` creates and updates MRs. The output matches GitLab's for the same
  pushes, including the centered warning banner. Tested with the real git client.
- GraphQL breadth:
  - MR commits, diff stats and milestones;
  - emoji details (from tanuki_emoji's data, dumped with GitLab's own Ruby);
  - note metadata;
  - root `users`, `projects`, `note` and `metadata`.
- Commit comments, including GitLab's `LegacyDiffNote` handling.
- Commit statuses and external pipelines:
  - the state machine and composite pipeline status;
  - the pipeline endpoints;
  - MR effects: head pipeline, `pipeline`/`head_pipeline`, `ci_must_pass`/`ci_still_running`, and
    auto-merge that waits for green;
  - pipeline webhooks and GraphQL `headPipeline`.
  - The database schema is now at version 2.
- 23 conformance scenarios and 2 client tests pass.

**Now:** nothing in progress.

**Next:**
1. Remaining "Later" items that touch MRs: the events API, MR description templates, MR
   discussions on commits.
2. A pass over the docs (README usage, PLAN status).

## 2026-11-07 08:15 UTC: GraphQL, client tests and web UI

**Done**
- GraphQL at `/api/graphql`:
  - GitLab 18.11's schema, embedded;
  - a small executor with graphql-ruby's validation and error shapes and GitLab's cursor formats;
  - resolvers for users, projects, merge requests, notes, discussions, labels and emoji;
  - MR, note, discussion and emoji mutations on the REST services. `mergeRequestAccept` merges
    asynchronously, like `merge_async`.
  - A subagent documented the details in `docs/gitlab-ref/graphql.md`. The `graphql_merge_requests`
    scenario matches GitLab.
- Milestones API and the MR resource label/state/milestone event endpoints, with a scenario.
- `tests/client`: an end-to-end MR flow through the `gitlab` crate, including webhook payloads
  checked against its hook types.
- Web UI (maud, no JavaScript) at GitLab-like URLs, so API `web_url`s open real pages:
  - sign-in, dashboard, groups and projects;
  - files, commits and branches;
  - MR list, creation and page: activity, threads, replies, resolve, emoji, approve, merge,
    draft, close and reopen;
  - diffs with inline comments;
  - settings: merge options, members, protected branches, webhooks with recent deliveries;
  - an admin user list.
- 19 conformance scenarios and the client test pass.

**Now:** committing.

**Next:**
1. Diff fidelity (the last roadmap phase): compare labrat's diffs with GitLab's on tricky inputs.
2. More MR edge cases where useful.

## 2026-11-07 07:37 UTC: edge cases from the GitLab source

**Done**
- Added seven conformance scenarios from the subagents' edge-case notes. All 17 scenarios pass.
  - MR lifecycle: create/update validation, state transitions, merge refusals, list filters.
  - Reviews: diff positions, discussions and emoji, drafts and approvals, suggestions.
- Added the project labels API.
- Behaviour fixes:
  - titles are trimmed;
  - a close/reopen survives a failed update;
  - the merge call keeps the squash flag and `merge_error` when refused;
  - `auto_merge` returns 405 without CI;
  - marking an MR ready runs an armed auto-merge;
  - list filters coerce params like Grape, and the coerced params show in pagination links;
  - text positions on files with no text diff are rejected;
  - blank suggestions delete lines;
  - note edits recreate suggestions;
  - `/request_review` unions reviewers, so with one reviewer allowed the first stays;
  - re-added reviewers come back `approved`;
  - system-note authors count as participants.
- The harness now delays some jobs by 300 ms (after-create, mergeability, approval notes) to
  match the transient states Sidekiq leaves visible.
- Several first recordings were GitLab racing its own Sidekiq jobs (push refresh vs. MR
  creation, async approval notes, unordered draft lists). The scenarios now settle at those
  points, so the recordings are deterministic.

**Now:** committing.

**Next:**
1. GraphQL for merge requests (schema from 18.11 introspection, MR queries and mutations).
2. The rudimentary web UI.
3. A few tests through the `gitlab` crate.

## 2026-11-07 07:05 UTC: webhooks match GitLab

**Done**
- Reviews layer (committed earlier as `21e8df1`): notes, discussions, diff notes with position
  tracing, suggestions, award emoji, draft notes and reviews, approvals, quick actions.
- Webhooks:
  - project hook API (CRUD, URL variables, custom headers, test, events log, resend) and system
    hooks;
  - delivery with GitLab's headers, redaction in the stored log, custom templates and branch
    filters;
  - payloads for push, tag push, merge request (open/update/close/reopen/merge/approved/
    unapproved, with GitLab's `changes`), note, confidential note and emoji events;
  - the `web_hook_test` rate limit (5 per minute).
- "mentioned in commit <sha>" system notes when a pushed commit references an MR.
- Conformance: new `webhooks_flow` scenario with a receiver that both GitLab (in Docker, via
  172.17.0.1) and in-process labrat deliver to. All 10 scenarios pass.
- Findings that shaped the code:
  - GitLab caches application settings for about a minute, so the scenario waits after enabling
    local webhook URLs.
  - A `PUT` with `state_event` fires both `close`/`reopen` and `update`.
  - Every `PUT` fires `update`, even when nothing changed.
  - The `open` hook is sent with `merge_status: checking`.
- The disk filled up (100%). Cut debug info in the dev profile, which shrank `target/` from 12 GB
  to 1.2 GB.
- Two research subagents wrote `docs/gitlab-ref/edge-cases-mr-lifecycle.md` and
  `edge-cases-reviews.md`.

**Now:** committing.

**Next:**
1. Turn the edge-case notes into conformance scenarios and fix what differs.
2. GraphQL for MRs.
3. The rudimentary web UI.

## 2026-11-07 06:00 UTC: merge request core matches GitLab

**Done**
- Git layer (`labrat-git`): SQLite object store, pack parsing with deltas, undeltified packs, rev
  walks, merge bases, tree diffs with renames, git-style patches, `git patch-id --stable`,
  three-way merges, rebase. Smart HTTP clone/fetch/push with GitLab's auth, pre-receive checks,
  protected branches and push messages; verified with the stock `git` client.
- Repository APIs: branches (incl. GitLab's 30s list cache), tags, commits (incl. create from
  actions, cherry-pick, revert), files, tree, blobs, compare, merge base, protected branches.
- Merge requests: create/update/close/reopen/delete, list filters, diff versions and
  `refs/merge-requests/:iid/head|merge`, async mergeability (`preparing`/`checking`), detailed
  merge status, merge (merge commit, semi-linear, fast-forward, squash, templates), rebase,
  auto-merge, source branch deletion with stacked-MR retargeting, merged-by-push, push refresh
  with "added N commits" notes, system notes, label/milestone/state events, subscriptions,
  todos, time tracking.
- Background jobs are rows in SQLite, run by a per-app worker (configurable delay).
- Conformance: 7 scenarios (users, groups/projects/members, repository, MR lifecycle,
  conflict/close/reopen, fast-forward + rebase, stacked retarget) all match GitLab exactly.
  Recording waits for GitLab's Sidekiq to go quiet and polls MRs until prepared.

**Now:** committing; next the review features.

**Next:** notes and discussions (incl. diff notes and positions), draft notes/reviews,
suggestions, award emoji, approvals, quick actions; then webhooks, GraphQL, UI.

## 2026-11-07 05:10 UTC: Phase 0 skeleton running, first scenarios match GitLab

**Done**
- Installed Rust (rustup, minimal profile) on the dev machine; copied GitLab 18.11's Rails source out of
  the reference container to `~/dev/gitlab-run-ce/gitlab-src` for reading (not in the repo).
- Six background agents wrote source-derived reference specs to `docs/gitlab-ref/` (users/groups/
  projects, merge requests, notes/discussions/reviews, approvals/reviewers/system notes, webhooks,
  repository/push).
- Incident: one research agent deleted the `claude` admin on the reference instance by mistake. It was
  recreated (new token in `~/dev/gitlab-run/claude-gitlab.env`) and the demo MRs reassigned to it.
  Research agents are now told to make no write requests.
- Cargo workspace: `labrat-core` (SQLite schema, models, permissions, services), `labrat-api` (axum REST
  layer, Grape-style params, auth with scopes/sudo, offset pagination with GitLab's Link header, entities),
  `labrat` (CLI binary), `tests/conformance` (harness).
- Endpoints: version/metadata, users (CRUD, block/unblock, PATs, impersonation tokens), groups, projects
  (incl. delayed deletion), members (direct and inherited), labrat test endpoints (reset, snapshot,
  restore, idle, time), background job runner with configurable delay.
- Conformance harness records scenarios against 18.11 and compares normalized responses (ids, times,
  tokens, run-unique names, key order). `users_and_tokens` and `groups_projects_members` pass.

**Now:** committing; then Phase 1 (git storage and smart HTTP).

**Next:** git object store in SQLite, smart HTTP upload/receive-pack, repository APIs, then MRs.

## 2026-11-07 04:35 UTC: planning done

**Done**
- Wrote the project description (`README.md`), the plan (`docs/PLAN.md`) and the agent rules
  (`AGENTS.md`), and added labrat to the shed project table.
- Settled the scope:
  - target GitLab CE 18.11;
  - no forks or cross-project MRs;
  - most tests check HTTP responses precisely, with a few end-to-end flows through the `gitlab`
    crate;
  - fixes for diff output that differs from Git's are postponed to the last roadmap phase.
- Added two items to the plan:
  - a configurable delay for background jobs;
  - a log of docs-vs-instance discrepancies (`docs/discrepancies.md`, created with its first entry).
- Everything is on the `add-labrat` branch (commits `9e05e60` to `6720c73`). It hasn't been merged
  yet.

**Now:** nothing in progress; waiting for the go-ahead to start building.

**Next:** Phase 0, Foundation:
1. Cargo workspace, axum server, SQLite schema, CLI and config.
2. Users, tokens and authentication.
3. Groups, projects, members and permissions.
4. Error and pagination helpers; reset and wait-until-idle endpoints.
5. Conformance harness, and the first fixtures recorded from the 18.11 reference instance.
