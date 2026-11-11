# GraphQL MR / review mutation error outcomes (GitLab 18.11 CE) vs. conformance fixtures

Source: /home/mabn/dev/gitlab-run-ce/gitlab-src (static reading only; nothing executed, no HTTP).
Fixtures: /home/mabn/dev/shed/labrat/tests/conformance/fixtures/*.json. Fixtures do not store the query text, so coverage was
decided by (a) listing every `POST /api/graphql` step whose body has top-level `errors`, a null mutation payload, or a non-empty
payload `errors`, and (b) matching the step label to the query in tests/conformance/tests/scenarios/*.rs.
Cited as "fixture:label". "Random-walk" steps pick random users/ops, so only the outcome that was actually recorded counts.

Notation: R = top-level `errors[0].message` =
`The resource that you are attempting to access does not exist or you don't have permission to perform this action`
(authorize_resource.rb:11), `path:[<mutationField>]`, `data.<mutationField>: null`.
[inferred] = derived from source only, expected text not seen in any fixture.

## 0. Cross-cutting behaviour (all mutations)

| # | Outcome | Source | Trigger | Coverage |
|---|---|---|---|---|
| X1 | Top-level R, no payload | base_mutation.rb:52-58 (`authorized?` -> `execute_graphql_mutation` global ability + scope check), authorize_resource.rb:52 | no token / anonymous, or token without `api` scope (read_api PAT) [inferred] | NOT COVERED (only anonymous *queries*: graphql_merge_requests:"users anonymous") |
| X2 | Top-level R | authorize_resource.rb:46-56 `authorized_find!` -> `authorize!(nil)` fails | `find_object` returns nil: unknown project path / unknown iid / unknown GID of a valid type | NOT COVERED for any MR/note/discussion/emoji mutation (nonexistent targets). Recorded R outcomes are all permission cases |
| X3 | Top-level R | same, ability denied | user lacks the mutation's ability (see per-mutation tables) | COVERED: see per-mutation rows |
| X4 | Top-level, variable coercion: `Variable $id of type NoteID! was provided invalid value`, `extensions.problems[0].explanation` = `"gid://gitlab/Project/1" does not represent an instance of Note` | global_id_type.rb:96-101 (suitable? check), :27-35 (`"x" is not a valid Global ID` / `is not a Gitlab Global ID`) | GID of wrong model for any `XxxID` argument (NoteID, DiscussionID, NoteableID, AwardableID, TodoID, UserID, MilestoneID, LabelID, MergeRequestID) | NOT COVERED on mutations. (Queries covered: graphql_merge_requests:"bad gid"; "missing variable"; literal and `ID!`-vs-`NoteID!` variable mismatch: graphql_mutations_3:"update image note"). Doc: docs/gitlab-ref/graphql.md:303-324 |
| X5 | Top-level `... is not a valid ID for X.` | gitlab_schema.rb:153-171 `parse_gid` | only reached by `GitlabSchema.parse_gid(..., expected_type:)` callers: createNote `discussionId` (notes/create/note.rb:31). Because DiscussionID already rejects wrong-model GIDs at coercion (X4), the "not a valid ID for" text is effectively unreachable via mutations; do not expect it | NOT COVERED (unreachable) |
| X6 | Top-level `Argument '<arg>' on InputObject '<X>Input' has an invalid value (...)` / `Field is missing required arguments` / `Expected type 'X'` etc. (graphql-ruby static validation) | graphql-ruby | bad enum literal, missing required input field | COVERED generically: graphql_accept_strategy:"mwps" (bad enum), "nullability mismatch" (variableMismatch), graphql_merge_requests:"missing argument". NOT COVERED for mutation inputs missing a required field (e.g. `sha` omitted) |
| X7 | Top-level `No object found for \`<arg>: "<gid>"\`` (graphql-ruby `LoadApplicationObjectFailedError`) | `loads:` args: set_milestone.rb:8-11, reviewer_rereview.rb:8-11, convert_to_thread.rb:14-19; no schema rescue_from found in gitlab_schema.rb | well-formed GID, row does not exist [unverified text; docs/gitlab-ref/graphql.md:629 also marks it unverified] | NOT COVERED |
| X8 | Read-only instance: R-style `You cannot perform write operations on a read-only instance` | base_mutation.rb:5,26 | DB read-only | NOT COVERED (not reproducible) |

## 1. Merge request mutations (app/graphql/mutations/merge_requests/)

All except Create/SetSubscription: `authorize :update_merge_request` (base.rb:23); Accept overrides with `:accept_merge_request` (accept.rb:7).
Find: `resolve_issuable` by `projectPath`+`iid` (base.rb:25-27; resolves_issuable.rb:11-18).

| # | Mutation | Kind | Message | file:line | Trigger | Coverage |
|---|---|---|---|---|---|---|
| M1 | any of RequestChanges/SetDraft/SetLabels/SetMilestone/SetLocked/SetReviewers/SetAssignees/Update/ReviewerRereview | top | R | base.rb:23 | Guest/Reporter/non-member (no `update_merge_request`; also not author/assignee) | COVERED: RequestChanges review_permissions:"guest/reporter/outsider request changes", random_walk_graphql_2:"38 carol op11"; SetDraft random_walk_hooks_4:"21 carol mr2 op9"; SetLabels "33 carol mr1 op5"; SetReviewers "47 carol mr1 op6"; SetMilestone random_walk_graphql_2:"41 carol op8"; SetLocked "44 carol op14". NOT COVERED: SetAssignees, Update, ReviewerRereview, Accept (accept_merge_request), SetSubscription |
| M2 | all (project/iid based) | top | R | resolves_issuable.rb:12 / authorize_resource.rb:46 | unknown iid or unknown projectPath | NOT COVERED |
| M3 | Accept | payload | `The merge request is already scheduled to be merged` | accept.rb:22,85-86 | auto-merge already enabled | COVERED graphql_accept_strategy:"mwcp again" |
| M4 | Accept | payload | `This branch cannot be merged` | accept.rb:18,87-88,96-102 | `mergeable?` false (blocked, not mergeable) / `auto_merge_eligible?(strategy)` false | COVERED random_walk_mixed_graphql:"54 bob mr2 op9". NOT COVERED on already-merged MR, closed MR |
| M6 | Accept | payload | `The merge-head is not at the anticipated SHA` | accept.rb:20,91 | `sha` != diff_head_sha | COVERED graphql.rs/graphql_merge_requests:"accept wrong sha"; graphql_accept_strategy:"mwcp wrong sha" |
| M7 | Accept | payload | `The merge failed` | accept.rb:21,75 | `merge_async`/AutoMergeService returned :failed | COVERED graphql_accept_strategy:"mwcp mergeable now" |
| M8 | Accept | payload | `e.message` of `MergeBaseService::MergeError` | accept.rb:77-81 | service raises MergeError synchronously | NOT COVERED (no known cheap trigger) |
| M9 | Accept | top | graphql validation | accept.rb:33 | `sha` omitted / `$sha: String` variable | COVERED (nullability) graphql_accept_strategy:"nullability mismatch"; bad enum "mwps" |
| M10 | Accept | top | R | accept.rb:7 | Reporter / non-member | NOT COVERED |
| M11 | RequestChanges | payload | `Reviewer not found` | request_changes.rb:18; update_reviewer_state_service.rb:41 | caller is not a reviewer of the MR | COVERED random_walk_graphql:"03 admin op11" (+ more) |
| M12 | RequestChanges | payload | `Invalid permissions` | update_reviewer_state_service.rb:5 | unreachable after authorize (same ability) | unreachable |
| M13 | RequestChanges | payload | `Reviewer has approved` / `Failed to update reviewer` / `Failed to remove approval` | update_reviewer_state_service.rb:21,22,29 | `Reviewer has approved` is unreachable (state is `requested_changes`); others need DB failure | unreachable |
| M14 | ReviewerRereview | payload | `Reviewer not found` | reviewer_rereview.rb:25; request_review_service.rb:57 | `userId` is not a reviewer | COVERED random_walk_reviewers:"36 cat op5" |
| M15 | ReviewerRereview | top | X4 / X7 | reviewer_rereview.rb:8-11 | `userId` = Project GID / nonexistent User GID | NOT COVERED |
| M16 | SetReviewers | payload | `Reviewers Cannot assign @dave to !1` (attribute `:reviewers`, full_messages) | set_reviewers.rb:30 -> update_service.rb:368 `new_user_ids`; update_reviewers_service.rb:11-12 | username of a user who cannot `read_merge_request` (non-member on private project) | NOT COVERED |
| M17 | SetReviewers / SetAssignees | none | unknown username silently dropped (UsersFinder returns none, no error). SetReviewers also truncates to 1 id (`.first(1)`, update_reviewers_service.rb:29) | set_reviewers.rb:37 | `reviewerUsernames:["nobody"]` | audit_followups_3:"set reviewer","append reviewer" cover reviewers success paths; unknown username NOT COVERED |
| M18 | SetAssignees | payload | `Assignees Cannot assign @dave to !1` | assignable.rb:21-22 -> update_assignees_service.rb:15,... -> update_service.rb:368 | assignee cannot read MR | NOT COVERED |
| M19 | Update | payload | `Title can't be blank` | update.rb:50-55; Issuable validation | `title:""` | COVERED graphql.rs/graphql_merge_requests:"update blank title" |
| M20 | Update | payload | `Title is too long (maximum is 255 characters)` | Issuable length validation | 256-char title | NOT COVERED |
| M21 | Update | payload | `Branch conflict You can't use same project/branch for source and target` | merge_request.rb:1357-1359 (attr `:branch_conflict`, full_messages) | `targetBranch` == source branch on open MR | NOT COVERED |
| M22 | Update | payload | `Target branch is invalid` (or similar full_message of `errors.add(:target_branch)`) | merge_request.rb:1390 `validate_branch_name` | `targetBranch:"a..b"` | NOT COVERED |
| M23 | Update | payload | `Validate branches Another open merge request already exists for this source branch: !N` | merge_request.rb:1368 | reopen (`state: OPEN`) / retarget so it duplicates another open MR | NOT COVERED (only on create: graphql_merge_requests:"create mr") |
| M24 | Update | top | enum literal error `Expected type 'MergeRequestNewState'` | update.rb:24 | `state: MERGE` | NOT COVERED |
| M25 | Update | top | `timeEstimate must be formatted correctly, for example \`1h 30m\`` / `... greater than or equal to zero. ...` | ValidateTimeEstimate (update.rb:58-62) | bad / negative estimate | COVERED graphql_accept_strategy:"time estimate invalid","time estimate negative" |
| M27 | Update | none | `targetBranch` on closed/merged MR silently dropped; `state: CLOSE` on merged = no-op, no error | update_service.rb:104-110 (`general_fallback`) | merged MR | NOT COVERED (record real behavior) |
| M28 | SetDraft / SetLocked / SetLabels / SetMilestone / SetSubscription | payload | `errors_on_object` only; no realistic failing input. Archived project -> R | set_draft.rb:25-28 | | NOT COVERED on archived project |
| M29 | SetLabels | top | X4 | set_labels.rb:9 | `labelIds:["gid://gitlab/Issue/1"]` | NOT COVERED. Unknown label id is silently ignored (no error) |
| M30 | SetMilestone | top | X7 `No object found for \`milestoneId: ...\`` or R if milestone unreadable | set_milestone.rb:8-11 | nonexistent milestone GID | NOT COVERED. `milestoneId:null` clears: covered random_walk_graphql ops |
| M31 | Create | top | R | create.rb:46 (`create_merge_request_from`) | unknown project / Guest / archived | NOT COVERED |
| M32 | Create | payload | `Validate branches Another open merge request already exists for this source branch: !2` | create.rb:60-66 | duplicate open MR | COVERED graphql_merge_requests:"create mr" |
| M33 | Create | payload | `Branch conflict You can't use same project/branch for source and target`; `Title can't be blank`; `Source branch is invalid` | merge_request.rb:1357,1390 | same branches; `title:""`; invalid ref | NOT COVERED (REST analogues covered: mr_create_update_edges) |
| M34 | Create | payload | `Source branch "x" does not exist` is only produced by BuildService (build_service.rb:176), and `validate_merge_request_branch_existence` is default-off (gitlab_com_derisk, 18.11), so a nonexistent branch may not error at all | | | NOT COVERED (record real behavior) |
| M35 | SetSubscription | top | R | set_subscription.rb:24 (`update_subscription`) | cannot read MR | NOT COVERED |

## 2. Notes / discussions / emoji (app/graphql/mutations/{notes,discussions,award_emojis})

| # | Mutation | Kind | Message | file:line | Trigger | Coverage |
|---|---|---|---|---|---|---|
| N1 | createNote / createDiscussion / createDiffNote / createLatestDiffNote | top | R | notes/create/base.rb:9,27 (`create_note`; policies: issuable_policy.rb:48-52 locked & non-member; merge_request_policy.rb:14-16 cannot read) | noteable not readable, or MR locked and caller not a project member (public project), or archived | NOT COVERED (all create-note R cases; X2 nonexistent MR GID too) |
| N2 | createNote etc. | payload | `Note can't be blank` (attr `note`) and `note: null` | notes/create/base.rb:36-39; note.rb validation | `body:""` (whitespace) | NOT COVERED |
| N3 | createNote | payload | `Discussion to reply to cannot be found` | notes/build_service.rb:12,57-61 | reply on discussion in noteable while `create_note` denied on discussion.noteable / confidential parent and caller cannot mark internal | NOT COVERED |
| N4 | createNote | payload | `Replies to system notes are not allowed` | build_service.rb:13,63-67 | `discussionId` of a system note's discussion | NOT COVERED |
| N5 | createNote | top | `The discussion does not exist or you don't have permission to perform this action` | notes/create/note.rb:34,43-50 | `discussionId` nonexistent or belonging to another noteable | NOT COVERED |
| N6 | createNote | top | X4 (`NoteableID` vs Project GID); `DiscussionID` vs Note GID | notes/create/base.rb:12 | | NOT COVERED |
| N7 | createNote | top | `This endpoint has been requested too many times. Try again later.` | create/base.rb:59 | >300 notes/min | NOT COVERED (impractical) |
| N8 | createNote | payload | quick-action failures in `quickActionsStatus.errorMessages`, `errors: [ "Validation ..." ]` | create_service.rb:112-118 | `/assign @nobody`-style | quick_action_* REST/GraphQL scenarios; graphql.rs:"quick action note" covers a success only; NOT COVERED failure |
| N9 | createDiffNote | top | `position oldLine or newLine arguments are required` | notes/create/diff_note.rb:22-25 | position without both lines | NOT COVERED |
| N10 | createLatestDiffNote | top | `oldLine or newLine arguments are required` | latest_diff_note.rb:43-46 | both null | NOT COVERED |
| N11 | createLatestDiffNote | top | `The provided head_sha does not match the current merge request diff. The diff may have been updated since you last viewed it.` | resolve_diff_position_service.rb:17-22 via latest_diff_note.rb:72 | stale head | COVERED graphql_more_mutations:"latest diff note stale head" |
| N12 | createLatestDiffNote | top | `File not found in merge request diff: <path>` | resolve_diff_position_service.rb:25 | `filePath` not in diff | NOT COVERED |
| N13 | createLatestDiffNote | payload | `Line code can't be blank`, `Line code must be a valid line code` | DiffNote validation | line outside diff | COVERED graphql_more_mutations:"latest diff note missing line" |
| N14 | createLatestDiffNote | top | X4 (`MergeRequestID` vs Issue GID) | latest_diff_note.rb:31 | | NOT COVERED |
| N15 | updateNote | top | R | notes/update/base.rb:9,17 (`admin_note`; note_policy.rb:37 `~editable` prevents for system notes even for admin; locked & non-member) | non-author non-maintainer; system note; unknown note | NOT COVERED (graphql.rs:"update note as other" is admin = allowed: success) |
| N16 | updateNote | payload | `Note can't be blank` | update/base.rb:27-31; notes/update_service.rb:11-14 | `body:""` [whether `.reset` clears errors before payload is read is unverified; may return `errors: []`] | NOT COVERED |
| N17 | updateNote | none | quick-action-only body destroys the note: `note:null, errors:[]` | update/base.rb:28-30 | body `/label ~x` only | NOT COVERED |
| N18 | destroyNote | top | R | notes/destroy.rb:8,16 | non-author Developer/Reporter; system note (even admin); unknown note | NOT COVERED (graphql.rs:"destroy note" is author: success) |
| N19 | destroyNote | note | always `errors: []` on success; no payload errors exist | destroy.rb:20-22 | | n/a |
| N20 | noteConvertToThread | top (ArgumentError) | `Note cannot be converted to a resolvable thread` | convert_to_thread.rb:26-28; individual_note_discussion.rb:16-18 | note is already a thread / diff note / system note | COVERED random_walk_graphql:"16 bob op13", "30 bob op13", "31 admin op13" |
| N21 | noteConvertToThread | top | R | convert_to_thread.rb:12,22 (`resolve_note`) | cannot resolve (outsider, locked non-member) | NOT COVERED |
| N22 | discussionToggleResolve | top | R | toggle_resolve.rb:50-54 (`resolvable? && resolve_note`) | unknown discussion; individual (non-resolvable) note's discussion; lacks resolve_note | COVERED (recorded cause unknown, random walk): random_walk_graphql_2:"36 bob op3","47 carol op3". NOT COVERED explicitly: individual note discussion, unknown discussion |
| N23 | discussionToggleResolve | top | X4 (`DiscussionID` vs Note GID) | toggle_resolve.rb:13 | | NOT COVERED |
| N24 | discussionToggleResolve | none | resolving already-resolved / unresolving open thread is a no-op, `errors:[]` (`Discussion failed to be (un)resolved` only on RecordNotSaved) | toggle_resolve.rb:35-37 | | NOT COVERED (record real behavior) |
| N25 | awardEmojiAdd | payload | `Name has already been taken` | add_service.rb:19-24 | duplicate award | COVERED graphql_merge_requests:"award add again" |
| N26 | awardEmojiAdd / Toggle | payload | `Name is not a valid emoji name` | award_emoji validation (emoji_name_validator.rb:15-17); add_service.rb:22-24 | `name:"notanemoji"` | NOT COVERED |
| N27 | awardEmojiRemove | payload | `User has not awarded emoji of type <name> on the awardable` | destroy_service.rb:11-13 | not awarded | COVERED graphql_merge_requests:"award remove missing"; random_walk_graphql many |
| N28 | awardEmoji* | top | `You cannot add emoji reactions to this resource.` | award_emojis/base.rb:6,27-33 | awardable is a system note (`Note#emoji_awardable?` = !system?, note.rb:500), or MR the caller cannot read | NOT COVERED |
| N29 | awardEmoji* | top | R | award_emojis/base.rb:8 (`award_emoji`; issuable_policy.rb:48-52 locked & non-member, archived) | unknown awardable GID; locked MR non-member | NOT COVERED |
| N30 | awardEmoji* | top | X4 (`AwardableID` vs Project GID) | base.rb:10 | | NOT COVERED |
| N31 | awardEmojiToggle | payload+field | `toggledOn:false, awardEmoji:null, errors:[...]` on invalid name | toggle.rb:16-26 | | NOT COVERED (success covered: graphql.rs:"award toggle off") |
| N32 | awardEmoji* | none | alias normalisation: `name:"+1"` -> stored `thumbsup` | award_emojis/base_service.rb:15-17 | | NOT COVERED |

## 3. To-do mutations (app/graphql/mutations/todos/)

| # | Mutation | Kind | Message | file:line | Trigger | Coverage |
|---|---|---|---|---|---|---|
| T1 | todoMarkDone / todoRestore / todoSnooze / todoUnSnooze | top | R | mark_done.rb:6,14 (`update_todo`) | todo of another user, or unknown id | COVERED (other user's) todo_mutations:"admin marks bob's done". NOT COVERED: unknown id, restore/snooze/unsnooze of another user's |
| T2 | todoCreate | top | `Cannot return null for non-nullable field TodoCreatePayload.errors` (GitLab bug: `errors` nil when todo already pending) | create.rb:22-29 | second create on same target | COVERED todo_mutations:"admin todoCreate again" |
| T3 | todoCreate | top | R | create.rb:8 (`create_todo`) | unknown/unreadable MR GID | NOT COVERED |
| T4 | todoSnooze | none | past `snoozeUntil` accepted, no error | snooze.rb:24; snoozing_service.rb:11-16 | | COVERED todo_snooze:"snooze in the past" |
| T5 | todoSnoozeMany / todoResolveMany / todoRestoreMany / todoDeleteMany | top | `Too many to-do items requested.` | base_many.rb:27-31,44-46 | >100 ids (checked before any lookup, fake GIDs suffice) | NOT COVERED |
| T6 | todosMarkAllDone | top | `Resource not available: gid://gitlab/MergeRequest/<id>` | mark_all_done.rb:55-61 | `targetId` of nonexistent target | NOT COVERED |
| T7 | todoDeleteMany | top | rate limit R-text | delete_many.rb:36-40 | bulk delete throttle | NOT COVERED (impractical) |
| T8 | *Many | none | foreign/unknown ids silently skipped, `errors:[]` | base_many.rb:48-52 | | partly: todo_mutations:"bob restores many" |

## 4. Out of scope

- Draft-note GraphQL mutations: none exist in app/graphql (grep `draft_note` = 0 hits). Draft notes are REST-only.

---

# NOT COVERED probes (ready to script)

Two scenarios. Conventions: users `admin` (Owner, token `a`), `bob` (Developer, author of notes), `carol` (Reporter), `dave` (non-member of the private project, valid token). Project private, MR !1 open (author bob, a diff on `a.txt`, bob has one individual note, one diff-note thread and a system note, e.g. produced by a label change), MR !2 merged (accept it first), MR !3 closed. Unless stated, call `s.graphql(label, token, query, vars)`; every row records the real response, then assert labrat equals it. Expected text is [inferred] from source unless marked.

Reusable GIDs: `mr1`, `mr2` (merged), `proj` = `gid://gitlab/Project/<id>`, `indNote` (bob's individual note), `sysNote` (system note GID), `sysDisc` (system note's discussion GID), `thread` (diff-note discussion GID), `NOPE_MR` = `gid://gitlab/MergeRequest/99999999`, `NOPE_NOTE` = `gid://gitlab/Note/99999999`, `NOPE_DISC` = `gid://gitlab/Discussion/0000000000000000000000000000000000000000`.

## Scenario A: `graphql_mr_mutation_errors` (merge request mutations)

Setup: as above; run each as stated.

A1. Unknown iid / unknown project (X2/M2), as admin, expect R at `path:["mergeRequestSetDraft"]` etc.
```graphql
mutation($p: ID!, $i: String!) { mergeRequestSetDraft(input: {projectPath: $p, iid: $i, draft: true}) { errors mergeRequest { iid } } }
```
vars `{"p": "<full>", "i": "999"}`; second call `{"p": "nope/nope", "i": "1"}`. Third: same shape with `mergeRequestUpdate(input:{projectPath,iid,title:"x"})`.

A2. Reporter (carol) on mutations not yet covered (M1/M10), expect R each:
```graphql
mutation($p: ID!) { mergeRequestUpdate(input: {projectPath: $p, iid: "1", title: "carol"}) { errors mergeRequest { title } } }
mutation($p: ID!, $u: [String!]!) { mergeRequestSetAssignees(input: {projectPath: $p, iid: "1", assigneeUsernames: $u}) { errors mergeRequest { iid } } }
mutation($p: ID!, $s: String!) { mergeRequestAccept(input: {projectPath: $p, iid: "1", sha: $s}) { errors mergeRequest { state } } }
mutation($p: ID!, $u: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $p, iid: "1", userId: $u}) { errors } }
mutation($p: ID!) { mergeRequestSetSubscription(input: {projectPath: $p, iid: "1", subscribedState: true}) { errors mergeRequest { subscribed } } }
```
vars `{"p": full, "u": ["bob"] / bob GID, "s": <head sha>}`. (SetSubscription as dave non-member on private project -> R; as carol it succeeds - run both.)

A3. Accept on merged and closed MR (M4), as admin with the correct current `sha`:
```graphql
mutation($p: ID!, $i: String!, $s: String!) { mergeRequestAccept(input: {projectPath: $p, iid: $i, sha: $s}) { errors mergeRequest { state } } }
```
vars `{"i":"2"}` (merged) and `{"i":"3"}` (closed). Expect payload `errors:["This branch cannot be merged"]`. Also with `strategy: MERGE_WHEN_CHECKS_PASS` (use the enum value that graphql_accept_strategy uses for its successful "mwcp" step) on the merged MR.

A4. Accept missing `sha` (M9, X6): inline literal omitting sha:
```graphql
mutation($p: ID!) { mergeRequestAccept(input: {projectPath: $p, iid: "1"}) { errors } }
```
Expect top-level `Argument 'sha' on InputObject 'MergeRequestAcceptInput' is required. Expected type String!` (code `missingRequiredInputObjectAttribute`).

A5. Reviewer/assignee who cannot read the MR (M16/M18), as admin, dave not a member of the private project:
```graphql
mutation($p: ID!, $u: [String!]!) { mergeRequestSetReviewers(input: {projectPath: $p, iid: "1", reviewerUsernames: $u}) { errors mergeRequest { reviewers { nodes { username } } } } }
mutation($p: ID!, $u: [String!]!) { mergeRequestSetAssignees(input: {projectPath: $p, iid: "1", assigneeUsernames: $u}) { errors mergeRequest { assignees { nodes { username } } } } }
```
vars `{"u": ["dave"]}`; expect `Reviewers Cannot assign @dave to !1` / `Assignees Cannot assign @dave to !1`. Then `{"u": ["nobody-xyz"]}` (unknown username; expect `errors:[]`, assignees/reviewers cleared on REPLACE).

A6. RequestChanges / Rereview edge cases:
```graphql
mutation($p: ID!, $i: String!) { mergeRequestRequestChanges(input: {projectPath: $p, iid: $i}) { errors mergeRequest { approved } } }
```
vars `{"i":"2"}` (merged MR) with bob first added as reviewer before the merge: record whatever (success vs error). And:
```graphql
mutation($p: ID!, $u: UserID!) { mergeRequestReviewerRereview(input: {projectPath: $p, iid: "1", userId: $u}) { errors } }
```
vars `{"u": "<proj GID>"}` (X4: `... does not represent an instance of User`) and `{"u": "gid://gitlab/User/99999999"}` (X7; record the exact `No object found for ...` text).

A7. Update validation errors (M20-M24, M27), as admin on open MR !1:
```graphql
mutation($p: ID!, $i: String!, $t: String, $tb: String, $s: MergeRequestNewState) { mergeRequestUpdate(input: {projectPath: $p, iid: $i, title: $t, targetBranch: $tb, state: $s}) { errors mergeRequest { title targetBranch state } } }
```
- `{"i":"1","t":"<256 x 'a'>"}` -> `Title is too long (maximum is 255 characters)`
- `{"i":"1","tb":"<MR source branch>"}` -> `Branch conflict You can't use same project/branch for source and target`
- `{"i":"1","tb":"a..b"}` -> record (invalid target branch text)
- `{"i":"3","s":"OPEN"}` with a second open MR from the same source branch existing -> `Validate branches Another open merge request already exists for this source branch: !N`
- `{"i":"2","tb":"other","s":"CLOSE"}` on merged MR -> record (expected silently ignored, `errors:[]`)
- literal `state: MERGE` inline (`state: MERGE` in the input) -> top-level `Argument 'state' on InputObject 'MergeRequestUpdateInput' has an invalid value (MERGE). Expected type 'MergeRequestNewState'.`

A8. SetMilestone / SetLabels GID problems (M29/M30):
```graphql
mutation($p: ID!, $m: MilestoneID) { mergeRequestSetMilestone(input: {projectPath: $p, iid: "1", milestoneId: $m}) { errors mergeRequest { milestone { title } } } }
mutation($p: ID!, $l: [LabelID!]!) { mergeRequestSetLabels(input: {projectPath: $p, iid: "1", labelIds: $l}) { errors mergeRequest { labels { nodes { title } } } } }
```
vars `{"m":"gid://gitlab/Milestone/99999999"}` (record `No object found ...`), `{"m": "<proj GID>"}` (X4), `{"l":["gid://gitlab/Label/99999999"]}` (expect silently ignored), `{"l":["<proj GID>"]}` (X4).

A9. MergeRequestCreate (M31-M34), as bob:
```graphql
mutation($p: ID!, $t: String!, $s: String!, $b: String!) { mergeRequestCreate(input: {projectPath: $p, title: $t, sourceBranch: $s, targetBranch: $b}) { errors mergeRequest { iid } } }
```
- `{"t":"x","s":"main","b":"main"}` -> `Branch conflict You can't use same project/branch for source and target`
- `{"t":"","s":"second","b":"main"}` where no open MR from `second` exists -> `Title can't be blank`
- `{"t":"x","s":"no-such-branch","b":"main"}` -> record (may succeed given the default-off flag)
- `{"t":"x","s":"a..b","b":"main"}` -> record (`Source branch is invalid`)
- as carol (Reporter, MR enabled) and dave -> R for dave; `{"p":"nope/nope"}` as admin -> R.

A10. Unauthenticated mutation (X1): `s.graphql("anonymous mutation", None, "mutation($p: ID!) { mergeRequestSetDraft(input:{projectPath:$p, iid:\"1\", draft:true}) { errors } }", ...)` -> expect R at `["mergeRequestSetDraft"]`. Optional if labrat has a read_api PAT: same call with it.

## Scenario B: `graphql_note_mutation_errors` (notes, discussions, emoji, todos)

Setup: same project; bob created individual note `indNote`; `thread` is a diff-note discussion by bob; `sysNote`/`sysDisc` from a system note on !1. Locked variants need a public project plus an outsider: `lock` MR !1 via `mergeRequestSetLocked` (locked:true) in a second public project with `eve` (non-member).

B1. createNote variants (N1-N6):
```graphql
mutation($id: NoteableID!, $b: String!, $d: DiscussionID) { createNote(input: {noteableId: $id, body: $b, discussionId: $d}) { errors note { id body } quickActionsStatus { errorMessages } } }
```
- bob, `{"id": mr1, "b": ""}` and `{"b": "   "}` -> payload `Note can't be blank`, `note:null`
- bob, `{"id": NOPE_MR, "b":"x"}` -> R (X2)
- bob, `{"id": "<proj GID>", "b":"x"}` -> variable coercion error `... does not represent an instance of Noteable` (X4)
- bob, `{"id": mr1, "b":"x", "d": NOPE_DISC}` -> top `The discussion does not exist or you don't have permission to perform this action`
- bob, `{"id": mr1, "b":"x", "d": sysDisc}` -> payload `Replies to system notes are not allowed`
- bob, `{"id": mr1, "b":"x", "d": "<Note GID>"}` -> X4 (`DiscussionID`)
- dave (non-member, private project), `{"id": mr1, "b":"x"}` -> R
- eve (non-member), locked MR in public project -> R; same MR after unlock -> success (also try `awardEmojiAdd` for eve on the locked MR -> R)
- bob, `{"id": mr1, "b": "/assign @nobody-xyz"}` -> record `quickActionsStatus.errorMessages` and `errors`
```graphql
mutation($id: NoteableID!, $pos: DiffPositionInput!) { createDiffNote(input: {noteableId: $id, body: "x", position: $pos}) { errors note { id } } }
```
- `pos` with `paths`/shas valid but neither `newLine` nor `oldLine` -> top `position oldLine or newLine arguments are required`
```graphql
mutation($id: MergeRequestID!, $head: String!, $f: String!, $n: Int) { createLatestDiffNote(input: {noteableId: $id, headSha: $head, filePath: $f, newLine: $n, body: "x"}) { errors note { id } } }
```
- `{"f":"a.txt","n":null}` -> top `oldLine or newLine arguments are required`
- `{"f":"missing.txt","n":1}` -> top `File not found in merge request diff: missing.txt`
- `{"id": "<Issue GID>"}` -> X4 (`MergeRequestID`)

B2. updateNote / destroyNote (N15-N18):
```graphql
mutation($id: NoteID!, $b: String) { updateNote(input: {id: $id, body: $b}) { errors note { id body } quickActionsStatus { messages } } }
mutation($id: NoteID!) { destroyNote(input: {id: $id}) { errors } }
```
- carol on bob's `indNote` (update and destroy) -> R each; bob update with `{"b":""}` -> record (`Note can't be blank` or `errors:[]`)
- bob update with `{"b":"/label ~bug"}` (quick action only) -> `note:null, errors:[]`, note gone
- admin on `sysNote` (update and destroy) -> R (system notes are not editable, even for admin)
- bob/admin on `NOPE_NOTE` -> R; `"<proj GID>"` -> X4

B3. Discussions (N21-N24):
```graphql
mutation($id: DiscussionID!, $r: Boolean!) { discussionToggleResolve(input: {id: $id, resolve: $r}) { errors discussion { id resolved } } }
mutation($id: NoteID!) { noteConvertToThread(input: {id: $id}) { errors note { id } } }
```
- bob resolve `indNote`'s discussion (not resolvable) -> R; `NOPE_DISC` -> R; carol on `thread` (Reporter, not author) -> record; resolve `thread` twice -> second is `errors:[]`; unresolve an open thread -> `errors:[]`; `"<Note GID>"` as DiscussionID -> X4
- dave `noteConvertToThread` on `indNote` -> R; `NOPE_NOTE` -> record X7 text; `sysNote` (admin) -> record (R or `Note cannot be converted to a resolvable thread`)

B4. Emoji (N26-N32):
```graphql
mutation($id: AwardableID!, $n: String!) { awardEmojiAdd(input: {awardableId: $id, name: $n}) { errors awardEmoji { name } } }
```
(plus same shape for `awardEmojiToggle ... toggledOn` and `awardEmojiRemove`)
- bob, `{"id": mr1, "n": "notanemoji"}` (Add and Toggle) -> payload `Name is not a valid emoji name` (Toggle: `toggledOn:false`)
- bob, `{"id": sysNote, "n":"thumbsup"}` -> top `You cannot add emoji reactions to this resource.`
- dave, `{"id": mr1}` -> same top-level text or R (record which; dave cannot read the MR so `cannot_read_merge_request?` is true: expected `You cannot add emoji reactions to this resource.` after the `award_emoji` authorize passes, else R)
- bob, `{"id": NOPE_MR}` -> R; `{"id": "<proj GID>"}` -> X4
- bob, Remove `{"id": mr1, "n":"notanemoji"}` -> `User has not awarded emoji of type notanemoji on the awardable`
- bob Add `+1` then Add `thumbsup` -> second is `Name has already been taken`; read back the stored `name`

B5. Todos (T1, T3, T5, T6):
```graphql
mutation($ids: [TodoID!]!) { todoResolveMany(input: {ids: $ids}) { errors todos { id } } }
mutation($id: TodoID!) { todoMarkDone(input: {id: $id}) { errors todo { id state } } }
mutation($id: TodoableID!) { todoCreate(input: {targetId: $id}) { errors todo { id } } }
mutation($id: TodoableID) { todosMarkAllDone(input: {targetId: $id}) { errors todos { id } } }
```
- `todoResolveMany` with 101 ids `gid://gitlab/Todo/1..101` -> top `Too many to-do items requested.` (repeat shape with `todoSnoozeMany` needing `snoozeUntil`, and `todoDeleteMany`)
- `todoMarkDone` / `todoRestore` with `gid://gitlab/Todo/99999999` -> R; `todoSnooze`/`todoUnSnooze` on admin's todo as bob -> R
- `todoCreate` with `NOPE_MR` as bob -> R; with dave on private mr1 -> R; with `"<proj GID>"` -> X4 (`TodoableID`)
- `todosMarkAllDone` with `NOPE_MR` -> top `Resource not available: gid://gitlab/MergeRequest/99999999`
