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
