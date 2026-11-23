# labrat plan

## Goals

1. **Behave exactly like GitLab for everything in scope.** That covers:
   - routes, parameters, defaults and validation errors;
   - status codes and JSON (field names, types, nulls and order);
   - pagination headers, webhook payloads and headers, and the GraphQL schema.

   The reference instance, not the documentation, decides what "exactly" means.
2. **Merge requests first.** That means the full lifecycle and everything a person can do in an MR:
   - comment, start and resolve threads, comment on diff lines;
   - review, approve and request changes;
   - react with emoji, suggest and apply changes;
   - rebase and merge.
3. **Fast and disposable.** One binary and one SQLite file, or memory only. It starts quickly and
   resets or snapshots between tests.
4. **Enough Git to get data in and out with the stock `git` client.**
5. **A web UI that looks and feels like GitLab's** to see state, click through the main flows and
   change settings. It uses only the GitLab APIs labrat serves, so it covers what labrat covers.

Non-goals:
- durability, availability, scaling and upgrades;
- security beyond correct permission checks;
- looking like GitLab.

## Reference version

The target is **GitLab CE 18.11** (Community Edition). The reference is the local instance at
`http://localhost:8929`, managed from `~/dev/gitlab-run-ce`. It
serves as the oracle in two ways:

- **Source.** The exact Rails code ships in the container at
  `/opt/gitlab/embedded/service/gitlab-rails`:

  | Path | What it defines |
  |------|-----------------|
  | `lib/api/` | Routes and parameters |
  | `lib/api/entities/` | JSON shapes |
  | `app/services/` | Behavior |
  | `lib/gitlab/hook_data/` | Webhook payloads |
  | `app/services/system_notes/` | System-note wording |
- **Live behavior.** The conformance suite (see below) runs the same scenarios against GitLab and
  labrat and compares the results.

When the docs and the instance disagree, the instance wins. Each disagreement is recorded in
`docs/discrepancies.md`, which is created with the first entry. An entry records:
- the endpoint or feature;
- what the docs say and what the instance actually does;
- how it was verified: a conformance scenario, or a file and line in the Rails source.

Then, when labrat differs from the docs, anyone can see it's on purpose, and newer GitLab versions can
be rechecked against the list.

Behavior is ported from GitLab CE, which is MIT-licensed and is the only reference.

For now, labrat stays on CE 18.11. Moving to a newer version would mean re-recording
the fixtures and fixing the differences.

**Enterprise Edition.** labrat also imitates GitLab EE 18.11 (Ultimate license), and does so by
default. `--noee` switches to exact CE behaviour.
- **EE mode covers** everything in scope below, with EE's responses: extra JSON fields, EE
  defaults and behaviour, and EE's GraphQL schema. It adds one EE feature, merge request
  approval rules and settings (see "In scope").
- **The EE reference** is an EE 18.11.14 instance reached only through its API. No EE source and
  no EE container are used. `docs/MAINTAINING.md` explains how to record against it.
- **Testing:** the `conformance_ee` test binary checks every shared scenario against EE
  recordings (`tests/conformance/fixtures-ee/`), plus EE-only scenarios
  (`tests/conformance/tests/scenarios_ee/`). The `conformance` binary checks the CE recordings
  with labrat running as CE.

## Architecture

```
 git clone/fetch/push   REST /api/v4   GraphQL /api/graphql   web UI   labrat test endpoints
          │                  │                 │                │              │
          └──────────────────┴────────┬────────┴────────────────┴──────────────┘
                                      │  auth + permissions
                                      ▼
                     services: one per user action, as in GitLab
                     (create MR, merge, add note, approve, push…)
                        │                  │                  │
                        ▼                  ▼                  ▼
                   git engine        SQLite: GitLab       in-process jobs
              (objects, packs,       data + git objects   (mergeability, diffs,
               diff, merge, rebase)  and refs             webhook delivery)
```

- **HTTP:** tokio and axum, with one listener for everything. It uses GitLab's URL layout, for
  example `/group/project/-/merge_requests/1` and `/group/project.git`. As a result, the `web_url`,
  `http_url_to_repo` and similar fields in API responses point to real labrat pages.
- **Services:** each user action is one service, mirroring GitLab's service objects
  (`MergeRequests::CreateService`, `Notes::CreateService`, …). Every entry point goes through them:
  REST, GraphQL, UI, quick actions and git push. That way the side effects are the same however an
  action was triggered. Side effects include:
  - system notes;
  - webhooks and events;
  - to-dos;
  - mergeability rechecks.
- **Serializers:** these mirror GitLab's Grape entities (`MergeRequestBasic`, `MergeRequest`,
  `Note`, `Discussion`, `UserBasic`, …) field for field and in the same order. Webhook payloads
  mirror the `Gitlab::HookData` builders.
- **Background jobs:** GitLab does a lot of work in Sidekiq, so clients see intermediate states. For
  example, `detailed_merge_status: "checking"` appears right after an MR is created, and
  `rebase_in_progress: true` during a rebase. labrat runs these jobs as in-process tasks to reproduce
  the same states. A labrat-only endpoint blocks until the queue is idle, so tests can wait without
  sleeping.

  In-process jobs finish within milliseconds, so clients would almost never see those intermediate
  states, and their polling and retry code would go untested. A **job delay setting** slows jobs
  down on purpose:
  - one global delay, with overrides per job type (mergeability check, diff generation, rebase,
    merge, webhook delivery);
  - an optional random jitter range, so timing isn't perfectly predictable;
  - set at startup through the CLI or config, and changeable at runtime through the labrat test
    endpoints and the admin UI, so one test can slow jobs down without restarting the server.

  The default is no delay, which keeps tests fast. Wait-until-idle still works with delays on.
- **GraphQL:** async-graphql with a dynamic schema, built at startup from GitLab 18.11's schema
  (3,742 types and 610 mutations). Introspection and code generators therefore see the real schema.
  Resolvers exist only for the in-scope subset; any other field returns a GraphQL error saying it
  isn't implemented in labrat.
- **UI:** a single-page app (React, Vite and TypeScript) in `web/`. It talks to labrat only through
  `/api/v4` and `/api/graphql`, like any other client, so it needs no UI-specific endpoints and
  exercises the same services. Signing in is a form post to `/users/sign_in` that starts a cookie
  session; the API accepts that session the way GitLab's accepts its own frontend's, with a CSRF
  token required on writes (`labrat-api/src/web.rs`). The built app (`web/dist`) is embedded in the
  binary by a build script, so `cargo build` needs no Node.js; without a build the binary serves a
  page explaining how to make one. Every non-API path serves the app, so GitLab's URL paths and
  `web_url`s open the right page. The app's own API calls carry `X-Labrat-UI` and stay out of the
  request log.
- **Webhooks:** delivered with reqwest. Every delivery's request and response is stored for
  inspection.

### Storage

There is one SQLite file, accessed through rusqlite with SQLite bundled, so there is no system
dependency.
- It uses WAL and `synchronous=OFF`, because durability isn't a goal.
- `--db :memory:` runs entirely in RAM.
- Writes go through a single writer, and reads use a small connection pool.

Git data:

| Table         | Holds                                                                   |
|---------------|-------------------------------------------------------------------------|
| `git_objects` | Every commit, tree, blob and tag by SHA-1, stored whole (zlib), shared by all projects |
| `git_refs`    | (project, ref name) → SHA; `HEAD` is a column on the project            |

There are no stored packfiles, no deltas and no garbage collection:
- A push is unpacked into individual objects.
- A fetch builds a pack on the fly without deltas. Such a pack is valid, only larger, which is fine
  for test-sized repositories.

GitLab data tables follow GitLab's own model:
- users and tokens;
- namespaces (groups and personal namespaces), projects and members;
- protected branches;
- merge requests and their diff versions;
- notes, including diff positions, and draft notes;
- approvals, reviewers and award emoji;
- labels and milestones;
- webhooks and their delivery logs;
- events and to-dos.

IDs come from per-table sequences, as in GitLab, and MR `iid`s are numbered per project.

Snapshots use `VACUUM INTO` and restore by swapping the file back in. That makes per-test isolation a
cheap file copy.

### Git

**Protocol: Git smart HTTP.** This is the transport behind `git clone https://gitlab…/group/project.git`:

```
GET  /<namespace>/<project>.git/info/refs?service=git-upload-pack|git-receive-pack
POST /<namespace>/<project>.git/git-upload-pack     clone, fetch, pull
POST /<namespace>/<project>.git/git-receive-pack    push
```

- **Authentication** is HTTP Basic with a token as the password, which is what GitLab accepts.
- **Why smart HTTP:** it is the simplest protocol that supports push.
  - Dumb HTTP is read-only.
  - SSH needs an SSH server and key management.
- **Protocol version:** v0 first. A client that asks for v2 falls back to v0 automatically when the
  server doesn't offer v2, so v2 is optional.
- **Server-side work.** labrat also has to do what GitLab delegates to Gitaly:
  - read trees and blobs;
  - create commits (commits API, merges, squashes, applied suggestions);
  - find merge bases;
  - produce diffs (MR changes, compare);
  - run three-way merges with conflict detection (mergeability, merge commits);
  - rebase (a series of cherry-picks).
- **Implementation:** gitoxide crates (`gix-object`, `gix-pack`, `gix-packetline`, `gix-diff`,
  `gix-merge`) on top of an object database adapter that reads and writes SQLite. gitoxide has no
  server-side `upload-pack` or `receive-pack`, so labrat implements that part itself:
  - pkt-line framing;
  - ref advertisement;
  - want/have negotiation;
  - status reports.
- **Hidden refs:** like GitLab, labrat writes `refs/merge-requests/<iid>/head` and `…/merge` into the
  target project. Clients can fetch them but not push to them.
- **Diff risk:** MR diff text has to match Git's (Gitaly runs `git diff`), because diff comments
  refer to its line numbers and hunks. gitoxide's `imara-diff` may place hunks differently from
  Git's Myers algorithm with the indent heuristic in rare cases. The conformance suite will catch
  that. Differences like this don't block other work:
  - the affected scenarios are marked as known differences and left in place;
  - fixing them, for example by porting Git's xdiff, is the last phase of the roadmap, after
    everything else is done.

## Matching GitLab's behavior

What has to match exactly:

- Routes, parameter names, types and defaults.
- Validation messages and error bodies, for example `{"message": "404 Project Not Found"}` and
  `{"message": {"title": ["can't be blank"]}}`.
- Status codes, including GitLab's 404 instead of 403 for resources the caller can't see.
- Response JSON: the same fields, types, nulls and field order.
- Timestamp formats, which differ between REST and webhook payloads and between event types.
- Offset pagination headers: `X-Total`, `X-Total-Pages`, `X-Page`, `X-Per-Page`, `X-Next-Page`,
  `X-Prev-Page` and `Link`. Also keyset pagination wherever GitLab supports it.
- Side effects:
  - system notes, word for word;
  - which webhooks fire, in what order and with what payload;
  - mergeability transitions;
  - reviewer state changes;
  - to-dos.
- Asynchronous behavior: the intermediate states clients can observe.

Known approximations:
- Markdown-to-HTML output (`description_html`, `note_html` and GraphQL `*Html` fields). Paragraphs,
  emphasis, block quotes, task lists, tables and code blocks follow GitLab's markup, and `@user`
  and `!N` references are linked as GitLab does when the text is saved. Syntax-highlighting
  tokens inside code blocks and other references (`#1`, `~label`, cross-project) are not
  reproduced.
- No emails are sent.
- `raw_diffs` and other raw patch text show invalid UTF-8 as U+FFFD instead of the original
  bytes. The JSON diff fields match GitLab, which drops those bytes.
- Generated-file detection covers `.gitattributes` and common lock files and Go "Code generated"
  headers, not all of linguist's content heuristics.
- Races inside GitLab are modelled one way. An MR update enqueues `ResolveTodosService`
  while the same request still creates the new mention to-dos. GitLab may resolve the
  editor's own fresh self-mention to-do or leave it, depending on Sidekiq's timing. labrat
  always resolves it.
- When a review is published, `/assign` in one draft starts `HandleAssigneesChangeWorker`,
  which finishes while GitLab is still publishing the later drafts. In the recording its
  "assigned to" note fell after the next draft's note and before that draft's commands, so
  labrat runs the pending assignee job each time a draft's note is saved.
- GitLab sometimes answers invalid GraphQL documents with only "Timeout on validation of
  query". graphql-ruby builds "Did you mean" suggestions over large dictionaries (every type for
  an unknown fragment or variable type), and that can run past GitLab's 0.2 s validation budget.
  The same query gets the normal error when it is fast enough
  (docs/gitlab-ref/graphql-validation-timeout.md). labrat always gives the normal error, with the
  suggestion.
- `root_repository_size` on namespaces is 0. GitLab reports the on-disk size of its Git
  storage, and labrat stores objects in SQLite, shared across projects.

**Conformance suite**
- **Scenarios:** each scenario is a sequence of API calls and git pushes, written once as a Rust
  test.
- **Running:** each scenario runs against both GitLab and labrat. The responses are normalized and
  then compared. Normalization covers IDs, timestamps, URLs and the SHAs of commits that contain
  timestamps.
- **Webhooks:** a built-in receiver captures deliveries from both servers so they can be compared
  too.
- **Recorded fixtures:** GitLab's responses are committed as golden files, so day-to-day test runs
  don't need the reference instance. It's needed only to record new scenarios.

The conformance suite is the main way labrat is tested. Most tests check HTTP responses precisely:
status, headers and the body field by field.

**Client library tests.** A small set of end-to-end tests use the Rust
[`gitlab`](https://lib.rs/crates/gitlab) crate as a real client. Each runs one flow, such as
creating a project, pushing, opening an MR, commenting, reviewing, approving and merging. They show
that a real client works against labrat. There are deliberately few of them. A client library
deserializes responses into its own types, so it would hide extra, missing or wrongly typed fields
that the conformance suite catches.

## Scope

### In scope

**Users and authentication**
- Users:
  - admin create, update, block, unblock and delete;
  - list and search;
  - `GET /user` and `GET /users/:id`;
  - admins.
- Personal access tokens:
  - scopes (`api`, `read_api`, `read_user`, `read_repository`, `write_repository`, `sudo`,
    `admin_mode`) and expiry;
  - `GET /personal_access_tokens/self`.
- Ways to pass a token: the `PRIVATE-TOKEN` header, `Authorization: Bearer` and `?private_token=`.
- Impersonation tokens and the `Sudo` header, which make it easy to act as many users in one test.
- Password login for the web UI.
- `/api/v4/version` and `/api/v4/metadata`.

**Groups and projects**
- Namespaces:
  - groups, subgroups and personal namespaces;
  - private, internal and public visibility;
  - paths and full paths;
  - `:id` given as a number or a URL-encoded path.
- Projects: create, read, update, delete, list with filters, and the `permissions` block on
  `GET /projects/:id`.
- Project settings that affect MRs:
  - merge method: merge commit, merge commit with semi-linear history, or fast-forward;
  - squash option;
  - "all threads must be resolved";
  - delete source branch by default;
  - merge and squash commit templates;
  - suggestion commit message;
  - printing the MR link on push;
  - resolving outdated diff threads;
  - default branch;
  - MR access level.
- Project and group labels and milestones, as far as MRs use them.

**Members and permissions**
- Project and group members:
  - inherited membership;
  - Guest, Planner, Reporter, Developer, Maintainer and Owner roles;
  - expiry dates.
- GitLab's permission rules for every in-scope action: who can see, comment, approve, merge, push
  and change settings.
- Protected branches:
  - allowed-to-push and allowed-to-merge levels;
  - force push;
  - default branch protection for new projects.

**Repository**
- Clone, fetch, pull and push over smart HTTP, including GitLab's push messages ("To create a merge
  request for …, visit:").
- Branches, tags (lightweight and annotated), files (get, raw, create, update, delete), tree, blobs,
  compare and merge base.
- Commits:
  - list, get, diff, refs, and the MRs a commit belongs to;
  - **create commits from file actions** (`POST /repository/commits`), so tests can build repos
    without `git`.

**Merge requests**
- Basic operations:
  - create, get, update, close, reopen and delete;
  - list per project, per group and instance-wide, with GitLab's filters, scopes and ordering.
- MR attributes:
  - draft and ready, both by title prefix and explicitly;
  - one assignee and one reviewer;
  - labels and milestones;
  - locking discussions;
  - subscribing;
  - participants.
- Diffs:
  - `/changes`, `/diffs`, `/raw_diffs` and `/commits`;
  - `/versions`, with a new version on every push;
  - `diff_refs`, the `refs/merge-requests/:iid/*` refs and `/merge_ref`.
- Mergeability: `merge_status` and `detailed_merge_status` with the CE checks, computed
  asynchronously the way GitLab does it. The checks are:
  - the MR is open;
  - it isn't a draft;
  - all threads are resolved;
  - there are no conflicts;
  - it doesn't need a rebase;
  - the source branch is in a valid state.
- Merging:
  - all three merge methods, squash and commit message templates;
  - the `sha` guard;
  - deleting the source branch;
  - retargeting dependent (stacked) MRs when their target branch is deleted on merge;
  - auto-merge, minus CI.
- Rebasing: `PUT /rebase` and `rebase_in_progress`.
- Pipeline endpoints return empty lists.

**Reviews and discussions**
- Comments and threads:
  - notes: create, read, update, delete and sorting;
  - discussions: list, get, start, reply, resolve and unresolve, with standalone comments and threads
    told apart as GitLab does.
- Diff comments:
  - positions for text lines, multi-line ranges and whole files;
  - comments becoming outdated after new pushes;
  - the setting that resolves outdated threads.
- System notes with GitLab's exact wording.
- Reviews:
  - pending review comments (draft notes): create, read, update and delete, publish one, and bulk
    publish;
  - submitting a review with a summary, approving or requesting changes;
  - reviewer states and re-requesting a review.
- Approvals: approve, unapprove and `/approvals` (optional approvals, no rules).
  In 18.11, approving is REST-only; GraphQL has no approve mutation.
- Suggestions:
  - parsed from ` ```suggestion ` blocks;
  - apply and batch apply, both of which create commits.
- Award emoji:
  - on MRs and on notes;
  - the `upvotes` and `downvotes` counts;
  - GitLab's list of valid emoji names.
- Quick actions in descriptions and comments: `/approve`, `/merge`, `/draft`, `/ready`, `/assign`,
  `/assign_reviewer`, `/label`, `/milestone`, `/close`, `/reopen`, `/rebase`, `/title`,
  `/submit_review` and so on.
- MR resource events (label, state and milestone changes).
- The to-dos API: to-dos for assignment, review requests and mentions.

**Webhooks**
- Project webhooks:
  - create, read, update and delete;
  - secret token;
  - SSL verification flag;
  - branch filter.
- Events: push, tag push, merge request, comment and emoji.
- Payloads built like GitLab's, including `changes` and the MR `action` values: `open`, `close`,
  `reopen`, `update`, `approved`, `unapproved` and `merge`.
- Delivery headers: `X-Gitlab-Event`, `X-Gitlab-Token`, `X-Gitlab-Instance`,
  `X-Gitlab-Webhook-UUID`, `Idempotency-Key` and `User-Agent`.
- Delivery history, and `POST /hooks/:id/test/:trigger`.
- System hooks for push and MR events.

**GraphQL** (`/api/graphql`)
- The CE 18.11 schema, generated from the CE code, for introspection and validation, with working
  resolvers for:
  - `currentUser`, `user` and `users`;
  - `project(fullPath)` with `mergeRequest(iid)` and `mergeRequests(...)`;
  - `group`, `namespace` and `mergeRequest(id)`.
- MR fields:
  - notes and discussions;
  - diff stats and commits;
  - assignees, reviewers and approvedBy;
  - labels and award emoji;
  - mergeability checks, `detailedMergeStatus` and `userPermissions`.
- Mutations:
  - MR: `mergeRequestCreate`, `mergeRequestUpdate`, `mergeRequestAccept`, `mergeRequestSetDraft`,
    `mergeRequestSetAssignees`, `mergeRequestSetReviewers`, `mergeRequestSetLabels`,
    `mergeRequestSetMilestone`, `mergeRequestSetLocked`, `mergeRequestSetSubscription`;
  - review: `mergeRequestRequestChanges`, `mergeRequestReviewerRereview`;
  - comments: `createNote`, `createDiffNote`, `createDiscussion`, `updateNote`, `destroyNote`,
    `discussionToggleResolve`, `noteConvertToThread`;
  - emoji: `awardEmojiAdd`, `awardEmojiRemove`, `awardEmojiToggle`.
- Global IDs (`gid://gitlab/MergeRequest/42`), cursor-paginated connections and GitLab's error
  format.

**Web UI** (GitLab's look and layout, limited to what labrat implements)
- Sign-in with a password or a personal access token; dashboards for projects, groups, merge
  requests and to-dos.
- Groups and projects: create, overview with README, members, labels, milestones.
- Repository: file browser, file view, editing and creating files (committing to a new branch can
  open a merge request), branches, tags and commits.
- MR list (state tabs, search, author, reviewer and label filters, sorting) and a new-MR flow that
  compares branches first.
- MR page:
  - description, reactions and the activity timeline with threads, replies and resolving;
  - the merge widget: approvals, mergeability, merge options, auto-merge, rebase, mark as ready;
    in EE mode also approval rules, approving with a password, policy violations (dismissing and
    bypassing them) and merge trains (adding, skipping and leaving a train);
  - the sidebar: assignees, reviewers with their review state, labels, milestone, time tracking,
    lock, notifications and to-dos; in EE mode also merge request dependencies;
  - commits, pipelines (from commit statuses) and changes with versions, inline comments and
    suggestions;
  - draft comments and review submission (with an optional summary and approval).
- Merge trains (EE): the project's active and merged cars, per target branch.
- Settings:
  - project general, repository (default branch, protected branches) and merge request settings;
  - EE: approval rules and approval settings, merge train settings, and code owner approval on
    protected branches;
  - labrat test setup, through the `/-/labrat/` test endpoints: fake CI (with finishing jobs by
    hand) and, for EE, code owners, approval policies and policy results;
  - webhooks with delivery history, test deliveries and resends;
  - project access tokens and personal access tokens;
  - group settings;
  - admin area: users, system hooks and network settings.
- The same URL paths as GitLab, so `web_url` in an API response opens the right page.
- EE features show in CE mode (`--noee`) too, disabled, with a note.

**labrat-only test tooling** (under `/-/labrat/`, which can't collide with GitLab routes)
- A seed file (YAML) that declares users, tokens, groups, projects, files and MRs at startup.
- Reset, snapshot and restore, plus memory-only mode.
- Wait until idle, meaning every background job and webhook delivery has finished.
- A configurable delay for background jobs, so clients can see intermediate states (see
  Architecture).
- A request log in the UI with every API call, its request and its response.
- Demo data (`POST /-/labrat/demo`, "Generate demo data" in the UI): a `demo` group whose
  projects each have merge requests showing one feature, made through the API as demo users.
  EE-only projects are left out in CE mode.
- A fixed root token from a CLI flag or environment variable, so tests can start immediately.

**EE: merge request approvals** (EE mode only)
- Project approval settings, through both `/projects/:id/approvals` and
  `/projects/:id/merge_request_approval_setting`, and top-level group settings
  (`/groups/:id/merge_request_approval_setting`) that lock project settings.
- Project approval rules (`/projects/:id/approval_rules`): users, groups, protected branch
  scoping and any-approver rules.
- MR approval rules (`/merge_requests/:iid/approval_rules`): copied from the project on creation,
  with per-MR overrides unless the project prevents them.
- Approval state: `/approvals` (EE body), `/approval_state`, `/approval_settings` and
  `POST /approvals`.
- Approval behaviour:
  - eligibility, including author approval;
  - the `not_approved` and `approvals_syncing` merge statuses;
  - the `approval`/`approved`/`unapproved`/`unapproval` hook actions;
  - approval resets on push and on reopen.
- GraphQL: `approvalState`, `approvalsRequired`, `approvalsLeft`, branch-rule approval rules,
  and the `mergeRequestUpdateApprovalRule`, `branchRuleApprovalProjectRuleCreate`,
  `approvalProjectRuleUpdate` and `approvalProjectRuleDelete` mutations.
- Not covered:
  - group approval rules, which stay behind a feature flag that is off in 18.11;
  - re-authentication to approve.

**EE: code owners** (EE mode only; the merge request side)
- labrat doesn't read `CODEOWNERS` files. The test endpoint `/-/labrat/projects/:id/code_owners`
  sets a branch's code owners as the file's sections: a name, an approval count (`[N]`),
  whether the section is optional (`^[Section]`), and entries of a pattern and owners
  (`@user`, `@group/path`). Patterns match like CODEOWNERS patterns: `*`, `/` anchoring,
  directories, `*` and `**`.
- MRs get `code_owner` approval rules from the target branch's code owners, as EE derives them:
  - one rule per pattern that owns a changed file (the last match in each section), named
    after the pattern, with its section (`codeowners` for the unnamed one) and count;
  - synced after the open hook, after a push, and when the code owners change, keeping ids;
  - they count only when the target branch requires code owner approval
    (`code_owner_approval_required`), then for at least one approval unless the section is
    optional;
  - they can't be changed through the MR rule endpoints (`403 Prohibited`);
  - with `selective_code_owner_removals`, a push takes back only the approvals of the changed
    files' owners (an `unapproved`/`unapproval` hook, `code_owner_approvals_reset_on_push`).
- They show wherever approval rules do: `/approval_rules`, `/approval_state`,
  `/approval_settings`, `/approvals` (counts, rules left, suggested approvers), the merge
  status, GraphQL `approvalState` and `mergeabilityChecks`, and the hooks' `approval_rules`.

**EE: approval policies** (EE mode only; the merge request side)
- labrat runs no security scans and links no security policy project. The test endpoint
  `/-/labrat/projects/:id/approval_policies` takes a policy file's `approval_policy` list, in
  GitLab's schema as JSON. labrat uses:
  - rules: `any_merge_request` (`commits: any` or `unsigned`), `scan_finding` and
    `license_finding`, with `branches` or `branch_type`;
  - `require_approval` actions with `user_approvers(_ids)`, `group_approvers(_ids)` and
    `role_approvers` (members with exactly that role, the MR's author excepted);
  - `enforcement_type` (`warn`), `fallback_behavior` (`fail: open`) and `bypass_settings`
    (users, groups and roles);
  - `approval_settings`, while violated on an MR: `prevent_approval_by_author`,
    `require_password_to_approve` (`approval_password`) and `remove_approvals_with_new_commit`;
    while in force, on the protected branches the policy covers: `block_branch_modification`
    (no changing or removing their protection, deleting them or moving the default branch
    away) and `prevent_pushing_and_force_pushing` (API commits and git pushes refused). Like
    the EE reference, `prevent_approval_by_commit_author` is reported but doesn't stop
    committers (the project's own committer setting does). `block_group_branch_modification`
    is reported only: labrat has no group-level protected branches.
- Evaluation is EE's without CI reports: `any_merge_request` rules are violated (every labrat
  commit is unsigned); scan and license rules can't be evaluated (`ARTIFACTS_MISSING`) and fail
  closed or open. `/-/labrat/projects/:id/merge_requests/:iid/policy_results` sets a policy as
  violated or not on one MR, to stand for scan results, with finding details in GraphQL's
  shapes (`newScanFinding`, `previousScanFinding`, `licenseScanning`, `errors`,
  `comparisonPipelines`) and a `status`. These details aren't compared with the EE
  reference, and the security bot's comment doesn't list them.
- The MR side:
  - `report_approver` rules with their policy's project rule as `source_rule`, asking for
    approvals once evaluated (after the open hook), and not editable;
  - the `security_policy_violations` status and `SECURITY_POLICY_VIOLATIONS` check (`FAILED`,
    or `WARNING` when the MR is approved anyway);
  - GraphQL `policyViolations`, `policiesOverridingApprovalSettings` and `policyBypassStatuses`;
  - warn mode dismissals (`dismissPolicyViolations`) and bypasses
    (`mergeRequestBypassSecurityPolicy`): the policy's rule then asks for no approvals;
  - the GitLab Security Bot's comment, created and updated as EE words it;
  - the policy fields of the hooks' `approval_rules`.
- Not covered: `send_bot_message`, policy scopes, and the security policy project APIs.

**EE: merge request dependencies** (EE mode only)
- REST `/merge_requests/:iid/blocks` (list, create by `blocking_merge_request_iid` with
  `blocking_project_id`, or by `blocking_merge_request_id`; delete) and `/blockees`, with
  EE's errors and permissions; blockers the user can't see are left out.
- System notes on both MRs ("marked this merge request as blocked by", "as blocking",
  "removed the relation with").
- The `merge_request_blocked` status and `MERGE_REQUEST_BLOCKED` check: any blocker that isn't
  merged blocks, closed ones included. Auto-merge waits for the blockers to merge.
- GraphQL `blockingMergeRequests` and `mergeRequestSetBlockingMergeRequests`.

**EE, later:** external status checks (`STATUS_CHECKS_MUST_PASS`).

**EE: merge trains** (EE mode only)
- Settings: `merge_pipelines_enabled`, `merge_trains_enabled` and
  `merge_trains_skip_train_allowed` (REST `PUT /projects/:id`, GraphQL `ciCdSettings` and
  `projectCiCdSettingsUpdate`).
- REST: `/projects/:id/merge_trains` (with `scope` and `sort`), `/merge_trains/:target_branch`,
  and `/merge_trains/merge_requests/:iid` (show and add, also when checks pass). Removal goes
  through `cancel_merge_when_pipeline_succeeds`.
- Train behaviour:
  - car pipelines on `refs/merge-requests/:iid/train`;
  - the front car merges when its pipeline passes;
  - a failed car is dropped and the cars behind it restart;
  - a push, a draft or closing the MR drops its car;
  - merging past the train restarts it, and an allowed skip records a `skip_merged` car;
  - `PUT /merge` with `auto_merge` (or `merge_when_pipeline_succeeds`) is refused while the
    MR's pipeline runs and merges at once after it passed.
- Webhooks: pipeline hooks (pending, running, finished) for MR and train pipelines, with their
  MR, fake CI jobs and runner, and the merge hook's `merge_params.train_ref`.
- GraphQL: `Project.mergeTrains` with cars; the MR's `mergeTrainCar`, `mergeTrainIndex`,
  `mergeTrainsCount` and train auto-merge strategies; `mergeRequestAccept` with
  `MERGE_TRAIN` and `ADD_TO_MERGE_TRAIN_WHEN_CHECKS_PASS`; `mergeTrainsDeleteCar`.
- **Fake CI**, the one exception to "CI/CD is out of scope", added for merge trains:
  - labrat reads no `.gitlab-ci.yml`;
  - a project's fake CI is set up through `/-/labrat/projects/:id/ci`;
  - with it on, MRs get detached or merged results pipelines, and trains get car pipelines;
  - jobs are finished by a fake runner, by hand (`/-/labrat/ci/pick`, `/-/labrat/ci/finish`)
    or automatically, with a given result after a given duration.

### Later (candidates, by demand)

Items marked **done** are implemented and covered by conformance scenarios.

- **Done:** Push options (`git push -o merge_request.create -o merge_request.target=main`).
- **Done:** Project and group access tokens and their bot users. Not done: deploy keys
  and deploy tokens.
- OAuth2 applications with the authorization-code flow.
- **Done:** The commit statuses API (`POST /statuses/:sha`) and the external pipelines it
  creates. This is for bots that post statuses on MRs.
- **Done:** Shallow clones and fetches (`--depth`, `--deepen`, `--unshallow`) and pushes from them.
  Not done: protocol v2 (git falls back to v0), `--shallow-since` and `--shallow-exclude`.
- **Done:** MR context commits. Not done: `closes_issues` and `related_issues` backed by minimal
  issues.
- **Done:** Time tracking on MRs and MR description templates.
- Automatic rebase before merge. In 18.11 it's a beta behind the `rebase_on_merge_automatic` flag.
- **Done:** Custom webhook headers, URL variables and templates. Auto-disabling failing hooks is
  off by default on self-managed GitLab, so hooks always report `executable`.
- **Done:** The events API. Not done: notification settings, and archive downloads (tar.gz and
  zip).
- **Done (added later):**
  - scheduled merges (`merge_after`);
  - commit discussions;
  - the search API for MR-related scopes;
  - GraphQL to-dos and the full `mergeRequests` filter set.
- Fault injection: errors, latency and rate-limit responses.

### Out of scope

- **Forks:** forks and cross-project MRs, where the source and target projects differ.
- **Group webhooks.**
- **CI/CD:** pipelines, jobs, runners, artifacts, CI variables, environments and deployments.
  The only exception is the fake CI that merge trains need (see "EE: merge trains").
- **Other GitLab products:** container and package registries, Pages, Releases, feature flags,
  Kubernetes and Terraform state.
- **Planning:** issues, epics, boards, work items, incidents, Service Desk, requirements and
  iterations.
- **Content:** wiki, snippets, and search scopes other than merge requests, milestones, notes,
  commits, users and projects (blobs, wiki blobs, issues, work items, snippets).
- **Security and compliance:** scanners, vulnerability reports, audit events, compliance frameworks
  and security policies.
- **Analytics:** analytics, insights and contribution graphs.
- **Git extras:**
  - Git over SSH and SSH key management;
  - Git LFS and partial clone;
  - SHA-256 repositories;
  - mirroring, and project import and export.
- **Identity:**
  - LDAP, SAML, OIDC and other external sign-in, and SCIM;
  - two-factor authentication;
  - email confirmation, emails and notifications.
- **Operations:**
  - durability, backups, HA and Geo;
  - upgrades: a new labrat version may need a fresh database;
  - rate limiting, monitoring and housekeeping.
- **AI:** GitLab Duo features.
- **Exact rendering:** byte-identical Markdown rendering, and a pixel-for-pixel copy of GitLab's web
  UI. labrat's UI follows GitLab's layout and look but renders Markdown in the browser.

## Roadmap

The UI grows with each phase rather than being a phase of its own.

**Status (2026-11-07):** phases 0 to 6 are delivered. Phase 5 continues as demand appears.

| Phase | Delivers | Done when |
|-------|----------|-----------|
| 0. Foundation | <ul><li>Cargo workspace, axum server, SQLite schema, CLI and config</li><li>Users, tokens and auth; groups, projects, members and permissions</li><li>Error and pagination helpers</li><li>Reset and wait-until-idle</li><li>Conformance harness and the first recorded fixtures</li></ul> | User, group and project scenarios match GitLab |
| 1. Git | <ul><li>Smart HTTP clone, fetch and push on the SQLite object store</li><li>Branch, tag, commit, file, tree and compare APIs</li><li>Commits from actions and protected branches</li><li>Push and tag webhooks</li><li>Seed file</li></ul> | A stock `git` round-trips a repository; repository scenarios match |
| 2. MR core | <ul><li>MR create, update, list and filters</li><li>Versions and diffs; MR refs</li><li>Mergeability, all three merge methods, squash and rebase; close and reopen</li><li>System notes and MR webhooks</li><li>MR pages in the UI</li></ul> | The stacked-MR flow from `gitlab-run` behaves identically, including retargeting |
| 3. Reviews | <ul><li>Notes, threads, diff comments and resolving</li><li>Draft notes and review submission; approvals and request changes</li><li>Emoji, suggestions and quick actions</li><li>Comment and emoji webhooks</li><li>Review flows in the UI</li></ul> | Review scenarios match, including system notes and webhook payloads |
| 4. GraphQL | <ul><li>Dynamic schema from the 18.11 SDL</li><li>MR queries and mutations</li></ul> | GraphQL scenarios match |
| 5. Breadth | <ul><li>To-dos and resource events</li><li>Remaining settings UI</li><li>Then "Later" items as needed</li></ul> | Driven by the integrations being tested |
| 6. Diff fidelity | <ul><li>Fixes for diff output that differs from Git's, postponed from earlier phases</li><li>For example, porting Git's xdiff</li></ul> | The scenarios marked as known diff differences pass |

## Planned layout

The project is a Cargo workspace that builds a single `labrat` binary:

```
crates/
  labrat/        binary: CLI, config, wiring
  labrat-git/    SQLite object store, pack reading and writing, smart HTTP, diff, merge, rebase
  labrat-core/   domain model, permissions, services, background jobs, webhook payloads
  labrat-api/    REST v4 routes and entities, GraphQL, the UI's sign-in and static files
web/             the web UI (React + Vite + TypeScript), embedded from web/dist
tests/conformance/   scenarios and recorded GitLab responses
tests/client/        a few end-to-end flows through the `gitlab` crate
```
