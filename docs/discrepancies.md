# Documentation vs. instance

Where GitLab's API documentation (the published docs, or the `desc` blocks they are generated from
in `lib/api`) and the behaviour of GitLab CE 18.11.14 disagree. labrat follows the instance. Each
entry is backed by a conformance recording.

| Endpoint | Documentation | Instance | Recording |
|---|---|---|---|
| `POST /projects/:id/statuses/:sha` | Success is `200` (`lib/api/commit_statuses.rb`, `desc … success code: 200`) | `201 Created`, Grape's default for POST | `commit_statuses`, step `pending` |
| `GET /projects/:id/merge_requests/:iid/pipelines` | Paginated like other lists (20 per page by default) | 15 per page by default: the endpoint declares no pagination params, so `Ci::Pipeline`'s Kaminari default applies | `commit_statuses`, step `mr pipelines` |
| `POST /projects/:id/repository/commits/:sha/comments` | `line_type` is optional with default `new` | When `path` is given, omitting `line_type` is `400 {"error":"line_type is missing"}`: Grape's `requires … default:` inside `given :path` still demands the param | `commit_comments`, step `context line` |
| `POST /api/graphql` introspection | Standard GraphQL introspection | Any request that looks like introspection (`__schema`, or `__type` without `__typename`, or operation `IntrospectionQuery`) gets a pre-generated file of the whole schema, whatever was asked; `__type(name:)` returns the full schema. Dynamic introspection is disabled, so `__schema` under another operation name is an `undefinedField` error | `graphql_merge_requests`, steps `dynamic introspection` and `introspection digest` |
| `POST /projects/:id/merge_requests` | `source_branch` and `target_branch` must exist | An MR from a branch that doesn't exist is created (`201`); the existence check is behind the `validate_merge_request_branch_existence` flag, off by default. The MR ends up `cannot_be_merged` with `detailed_merge_status: commits_status` | `mr_create_update_edges`, step `missing source branch` |
| `PUT /projects/:id/merge_requests/:iid` with `state_event=reopen` | Reopens the MR | When another open MR exists for the same branches, the response is `200` with the MR still `closed`; no `409` | `mr_create_update_edges`, step `reopen conflicting` |

## GitLab EE

Where GitLab's public documentation and the EE reference (GitLab EE 18.11.14, used through its API
only) disagree. labrat's EE mode follows the instance. Each entry is backed by an EE recording in
`tests/conformance/fixtures-ee`.

| Endpoint | Documentation | Instance | Recording |
|---|---|---|---|
| `PUT /projects/:id/merge_requests/:iid/merge` with `auto_merge` (or `merge_when_pipeline_succeeds`) on a project with merge trains | `auto_merge`: "the merge request merges when checks pass"; the merge trains guide: "You cannot use auto-merge ... to skip the merge train" (the API docs' history says the MR is added to the train instead, from GitLab 19.1 when the `fix_merge_api_train_bypass` flag was removed) | While the MR's pipeline runs: `405 Method Not Allowed`, and auto-merge isn't set. Once the pipeline passed: the MR merges at once, outside the train | `merge_train_hooks`, steps `auto merge while pending`, `mwps while pending` and `auto merge after pass` |
| `/submit_review requested_changes` quick action | "Submit review and request changes", with no condition on who submits | From a note, anyone's request blocks the merge (`requested_changes`). In a review published from drafts by a user who isn't a reviewer it does nothing: no "requested changes" note, the merge isn't blocked, and an earlier approval by that user stays | `random_walk_reviewers`, steps `20 dan op4 publish` to `cp20 graphql` and `22 dan op6` |

### Undocumented EE behaviour

Behaviour the documentation doesn't describe, as the EE reference shows it. labrat matches it.

| Endpoint or feature | Instance | Recording |
|---|---|---|
| Merging an MR | The other open MRs into the same target branch become `unchecked` at once (CE leaves them until their next check) | `api_error_paths` and `graphql_mutation_errors`, step `close 3` |
| `GET /projects/:id/merge_requests/:iid/approvals` | `merge_status` is never `checking`: right after a read started a mergeability check, it already answers `can_be_merged` | `random_walk_mixed_3`, step `cp44 mr1 approvals`; `random_walk_mixed_graphql_3`, step `cp59 mr1 approvals` |
| `/unassign_reviewer @user` | Removes only the users named (EE allows several reviewers). A named user who isn't a reviewer is still listed in "Removed reviewer @user.", and the MR is updated (`updated_by_id` changes, an update hook fires) | `random_walk_mixed_graphql_3`, step `21 bob mr2 op0` and its hooks |
| MR reviewers (REST `reviewers`, `GET .../reviewers`, GraphQL `reviewers`, hooks, quick action `commands_changes`) | No fixed order: it follows the database rows' history, and the same reviewers set by the same request came back in different orders. The replays compare EE reviewer lists as sets | `random_walk_reviewers`, steps `15 dan op6` and `cp41 reviewers`; `review_drafts_approvals`, step `two reviewers keeps first` |
| `GET /user_counts` after a push resets approvals | A reviewer whose approval was reset (now `unapproved`) keeps their cached `review_requested_merge_requests` count | `random_walk_reviewers`, step `cp41 dan counts` |
| Merge request hook on open, with merged results pipelines | The MR's pipeline already exists: `head_pipeline_id` is set and the MR is `mergeable` (checked before the pipeline is created) | `merge_train_hooks`, step `mr 3 open and pipeline pending` |
| Pipeline hook `object_attributes.ref_status_name` | The status of the branch's CI ref (the source branch for MR and train pipelines) from earlier finished pipelines: `unknown` before any finished, so a pending train pipeline can report `success` | `merge_train_hooks`, steps `mr 3 pipeline running` and `train pipeline pending` |
| Pipeline hook `builds[].duration` | A running job reports the seconds it has run so far, not `null` | `merge_train_hooks`, steps `mr 3 pipeline running` and `train pipeline running` |
| Merge request hook of a train merge | `merge_params.train_ref` holds `commit_sha` and `merge_commit_sha`, both the train commit | `merge_train_hooks`, step `train pipeline success and merge` |
