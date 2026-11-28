# GitLab 18.11 CE GraphQL query complexity

Source: GitLab CE source, `/home/mabn/dev/gitlab-run-ce/gitlab-src` (paths below relative to it).
graphql-ruby: pinned to **2.6.10** (`config/initializers/graphql_subscriptions_serialize_patch.rb:24`).
The gem source is NOT on this disk, so everything about the gem's own analyzer
(`GraphQL::Analysis::QueryComplexity`) is from memory of graphql-ruby 2.x. Those parts are marked **[GEM/UNVERIFIED]**.
Everything in `app/`, `lib/` is read from source.

Handy calibration tool: `{ queryComplexity { score limit } }` (`app/graphql/types/query_complexity_type.rb`)
returns the analyzer score and the limit for the current request. The `queryComplexity` field itself and
its children are included in the score (1 + 1 + 1 = 3 if you select both subfields). Use it against the
real instance to confirm the **[GEM/UNVERIFIED]** points.

## 1. Limit selection and error

`app/graphql/gitlab_schema.rb:4-9, 194-207` (applied per query in `multiplex`, lines 56-62):

```
max_query_complexity(ctx):
  if ctx[:current_user]&.admin      -> 300   # plain User#admin column; NO admin-mode check
  elsif ctx[:current_user]          -> 250
  elsif ctx[:introspection]         -> 226   # only anonymous
  else                              -> 200
```
- `ctx[:introspection]` is set only by `GraphqlController#execute_introspection_query`
  (`app/controllers/graphql_controller.rb:400-402`), chosen by `introspection_query?` (:426: operationName
  == the introspection op name, or query text contains `__schema`, or `__type` without `__typename`).
  A logged-in user running introspection gets 250/300, not 226. In production the controller serves a
  static file for such requests, so complexity is not evaluated at all (dev/test evaluate it).
- Check is strict: error iff `total > max`. Message (gem):
  `Query has complexity of {total}, which exceeds max complexity of {max}`
  (HTTP 200 with `{"errors":[{"message":...}]}`). **[GEM/UNVERIFIED]** wording, matches the example in the task.
- Related but separate: max depth 15 anonymous / 20 authenticated (:12-13, :210-216); `validate_max_errors 5`;
  `default_max_page_size 100` (:31).
- Schema uses `complexity_cost_calculation_mode(:legacy)` (`gitlab_schema.rb:37`).
- `current_user` here is the Rails `current_user` (token/session user); blocked/none -> anonymous.

## 2. Algorithm

### 2.1 Gem analyzer (legacy mode) **[GEM/UNVERIFIED]**

```
total(query) = sum over root fields F of the operation (query OR mutation) of cost(F)
              (fields excluded by @skip/@include are not visited)

cost(F at node n)  = F.definition.calculate_complexity(child_complexity = children_sum(n))
children_sum(n)    = combine the selections of n (see scoping below)
```
`calculate_complexity` for GitLab fields goes to `BaseField#complexity_for` (2.2), because the field
responds to `complexity_for`. Arguments to the proc are `query.arguments_for(node, field).keyword_arguments`
(snake_case Ruby keys, honouring `as:`; **defaults are included**, e.g. `sort` default
`:created_desc` is present even if the client omits it).

Scoping in `:legacy` mode (the part I am least sure of): each field occurrence's cost is added into a bucket
keyed by the field's parent type definition (the type condition for fragment/inline-fragment selections,
otherwise the enclosing object type). The children sum of a field is the **max over buckets** of the
**sum of costs in the bucket**. Consequences:
- Plain object-typed selections with no fragments: simply the sum of all child costs.
- Same response key selected twice (e.g. two `nodes {...}` blocks, or the same fragment spread twice): in
  legacy each *node* is costed and added, i.e. duplicates are likely **double counted**. (`:future` mode
  would merge by response key and count once.) Uncertain.
- `... on A { x } ... on B { y }` on an interface/union: max(cost(A bucket), cost(B bucket)), not sum.
- A fragment on interface `I` plus direct fields on the concrete type land in two different buckets, so
  only the larger counts (legacy quirk). Uncertain.
- Named fragments are traversed at each spread site.
- `__typename`/introspection fields: ordinary fields with default cost 1 (no GitLab override). graphql-ruby
  has an option to skip them (`count_introspection_fields`-style); GitLab's comment "GraphiQL introspection
  query has complexity of 226" (`gitlab_schema.rb:7`) implies they ARE counted. Uncertain whether `__typename` is 1.
- Variables/arguments in mutation inputs are not costed. Only selections count.

### 2.2 GitLab per-field cost (read from source; high confidence)

`app/graphql/types/base_field.rb`.

```
DEFAULT_COMPLEXITY = 1
base_complexity    = 1 + (calls_gitaly? ? 1 : 0)                       # :95-99
calls_gitaly?      = field's calls_gitaly: kwarg, else resolver_class.calls_gitaly? (:101-103)
                     # resolver.calls_gitaly! is class-ivar, NOT inherited, and lost on ".single"

# initialize (:15-26)
given = kwargs[:complexity] || resolver_class.complexity   # BaseResolver.complexity == 0 (base_resolver.rb:115)
constant = given.is_a?(Integer) && given > 0
if given is Integer and > 0          : field.complexity = given                    # CONSTANT
elsif resolver_class present         : field.complexity = PROC (below)
else                                 : field.complexity = base_complexity          # 1 or 2

complexity_for(child):                                                             # :63-81
  Numeric -> complexity + child
  Proc    -> complexity.call(ctx, kwargs, child)      # (if kwargs unavailable: child)
```
So:

| field kind | cost |
|---|---|
| plain field (no resolver, no `complexity:`) | `1 + child`; `calls_gitaly: true` -> `2 + child` |
| explicit `complexity: N` (N>0), with or without resolver, connection or not | `N + child` (calls_gitaly adds nothing; no page-size multiplier; no resolver_complexity) |
| field with resolver (`resolver:`), no explicit complexity | PROC below |
| mutation field (`mount_mutation`) | see section 4 |

Resolver PROC (`field_resolver_complexity`, :146-158):
```
c = child + resolver.resolver_complexity(args, child_complexity: child).to_i   # nil.to_i == 0
c += 1 if field.calls_gitaly?
ext = resolver.calculate_ext_conn_complexity           # false for all but WorkItemDiscussionsResolver
if field.connection? or ext:                            # connection? = return type is a *Connection
    page_size = field.max_page_size || schema.default_max_page_size     # field kwarg, else resolver `max_page_size`, else 100
    limit     = min(compact([args[:first], args[:last], page_size]))
    mult      = resolver.complexity_multiplier(args).to_f
    c += c * (limit * mult)                              # float math (IEEE double)
return c.to_i                                            # truncation toward zero, at EVERY field
```
`BaseResolver` defaults (`app/graphql/resolvers/base_resolver.rb:119-148`):
```
resolver_complexity(args) = 1 + (args[:sort] ? 1 : 0) + (args[:search] ? 5 : 0)
complexity_multiplier(args):
   args[:iids] is Array: len 1..100 -> 0.05 ; 101..1000 -> 0.5 ; >1000 -> 5.0 ; (len 0 -> 0.01)
   elsif args[:iid]                  -> 0
   else                              -> 0.01
```
Notes:
- Because `sort` args usually have a `default_value`, `args[:sort]` is truthy even when not passed: +1.
  (`search` has no default: +5 only when supplied. `searchTerm`/`searchTitle` args are NOT `search`.)
- With default page size 100 and default multiplier 0.01 the connection factor is exactly `limit*0.01`
  = 1.0, so `c` doubles; `first: 5` gives factor 0.05; `first: 1` gives 0.01 (so `c + c*0.01`, truncated).
- A **non-resolver** connection field (e.g. `MergeRequest.approvedBy`, `.reviewers`, `.commits`,
  `Discussion.notes`, `.discussions`, `.commenters`, `awardEmoji`) has NO first/last scaling at all:
  cost is just `1 (or 2 / explicit) + child`. Only resolver-backed connection fields scale.
- Field overrides that matter (verified): `ProjectResolver.complexity_multiplier -> 0`
  (`resolvers/project_resolver.rb:17`, irrelevant since non-connection), `Projects::SnippetsResolver` (0 if iid/iids else 0.05),
  `BoardListIssuesResolver`, `BoardListsResolver`, `WorkItemDiscussionsResolver` (ext_conn),
  `BlobsResolver.resolver_complexity = super + paths.size` (`blobs_resolver.rb:36`),
  `ResolvesPipelines.resolver_complexity = super + 2 if sha + 2 if ref` (`concerns/resolves_pipelines.rb:57-63`),
  `Ci::ProjectPipelineResolver.resolver_complexity = (super - 10) if args.present?` else **nil -> 0**
  (`ci/project_pipeline_resolver.rb:29-32`; so `project.pipeline` with no args costs `child + 0 + 1(gitaly)`,
  with `iid/sha/id` costs `child - 9 + 1 = child - 8`; can reduce the total),
  `NamespaceProjectsResolver.resolver_complexity = super + 10` (`namespace_projects_resolver.rb:77`).
- `markdown_field` (`lib/gitlab/graphql/markdown_field.rb:19`) sets `complexity ||= 5` -> constant 5.

### 2.3 Connection plumbing types (no GitLab override)

`Types::CountableConnectionType`/`LimitedCountableConnectionType`/`MergeRequestConnectionType` derive from
`GraphQL::Types::Relay::BaseConnection`; their fields (`edges`, `nodes`, `pageInfo`, `count`, `totalTimeToMerge`...)
use the gem's field class -> cost `1 + child`. `BaseEdge` (`types/base_edge.rb`) uses `BaseField`: `node`, `cursor` = 1.
`PageInfo` subfields 1 each. So `nodes { a b }` = 1 + 1 + 1 = 3; `pageInfo { hasNextPage endCursor }` = 3;
`edges { node { a } }` = 1 + (1 + 1 + 1... ) i.e. edges(1) + node(1) + a(1) = 3; `count` = 1.
These are NOT multiplied by `first`; the multiplication is applied to the whole connection field (nodes+edges+pageInfo+resolver base).
**[GEM/UNVERIFIED]** that gem's `edges`/`nodes` have default complexity 1 (very likely).

### 2.4 Permissions types

No special rule. `userPermissions` is a plain field (`method: :itself`, `lib/gitlab/graphql/expose_permissions.rb`) cost 1 + child, and each
boolean permission is 1, except those declared with `calls_gitaly: true` (cost 2). See tables.

## 3. Non-default fields (cost != 1)

Legend: **G** = `calls_gitaly: true`, plain field => 2 + child. **C=n** constant `n + child`.
**R-conn** = resolver connection => `(child + resolver_complexity [+1 G]) * (1 + min(first,last,pageSize)*mult)`, truncated.
**+sort** = resolver has a sort arg WITH default, so `resolver_complexity` is 2 always (+5 with `search`).
Non-connection resolver fields cost `1 + child` (same as default) unless noted, so they are not listed.
File:line is the `field` statement's first line unless noted.

### Query (`app/graphql/types/query_type.rb`)
| field | cost | ref |
|---|---|---|
| project | `1 + child` (ProjectResolver, non-conn) | :165 |
| mergeRequest | `1 + child` (plain, `id` arg) | :115 |
| currentUser | `1 + child` (plain) | :70 |
| group, namespace, user | `1 + child` | :87, :130, :232 |
| users | R-conn +sort(default created_desc) +search: `(child + 2 [+5 if search]) * (1 + limit*0.01)` | :236, resolvers/users_resolver.rb |
| groups, projects, issues, snippets, runners, todo... | R-conn, resolver-specific (sort default => +1) | :94, :173, :103 |
| ciConfig | C=126 | :31 |
| timelogs | C=5 | :215-219 |
| wikiPage | `1 + child + 1(G)` (resolver-level G) | :240-245 |
| workItemDescriptionTemplateContent, securityConfiguration | +1 G | :252, :309 |
| queryComplexity | 1 + child | :177 |

### Project (`app/graphql/types/project_type.rb`)
| field | cost | line |
|---|---|---|
| mergeRequests | R-conn, +sort(created_desc), +5 if `search`; iids tiers: len<=100 -> mult 0.05 (resolvers/merge_requests_resolver.rb:159, project_merge_requests_resolver.rb) | 369 |
| mergeRequest | `child + 1` only (`MergeRequestResolver`, no sort, non-conn) => same as default | 376 |
| issues | R-conn +sort (Issues::SortArguments default created_desc) +search | 382 |
| workItems, workItemStateCounts | R-conn +sort | 388, 397 |
| issueStatusCounts | resolver overrides complexity, G, ext_conn (not analysed in detail) | 404 |
| milestones | R-conn +sort (due_date_asc default) | 410 |
| labels | R-conn, (`searchTerm` is not `search`) no sort | 772 |
| pipelines | R-conn, field G (+1), resolver adds +2 if `sha`, +2 if `ref`; mult 0.01 | 495 |
| pipeline | resolver `resolver_complexity` nil when no args (see 2.2); G; non-conn: `child + 0 + 1` no args, `child - 8` with args | 519 |
| releases, release | R-conn +sort(released_at_desc) | 634, 639 |
| pipelineSchedules | R-conn +sort(id_desc) | 503 |
| packages, containerRepositories, inheritedCiVariables, timelogs, ciVariables | R-conn (+sort where default); timelogs C=5 constant | 456, 667, 538, 729 (C at 733), 532 |
| projectMembers | R-conn, sort arg without default (+1 only if given), search +5 | 415 |
| snippets | R-conn, multiplier 0.05 (0 if iid/iids) | 561 |
| boards | R-conn, max_page_size 2000 (limit = min(first,last,2000)) | 571 |
| branchRules | G + R-conn, max_page_size 100 | 802 |
| forkDetails (G), languages (G), protectableBranches (G) | 2 + child | 790, 810, 900 |
| ciConfigVariables, ciPipelineCreationInputs, sastCiConfiguration | G (2 + child) | 33, 57, 90 |
| forksCount, avatarUrl, statisticsDetailsPaths | G (2 + child) | 159, 250, 359 |
| commitReferences, agentConfigurations | resolver G => `1 + 1 + child` (+conn multiplier for agentConfigurations) | 848, 736 |
| descriptionHtml | C=5 (markdown_field) | 1018 |
Everything else on Project (repository, group, namespace, statistics, userPermissions, ...) is 1. `repository` itself is plain 1.
Note: all plain connection fields on Project (e.g. `jiraImports`, `timelogCategories`, `visibleForks`, `packagesProtectionRules` w/o resolver scaling) are `1 + child`.

### MergeRequest (`app/graphql/types/merge_request_type.rb`)
| field | cost | line |
|---|---|---|
| notes (from NoteableInterface, `notes/noteable_interface.rb:8`) | R-conn (NotesResolver: res_c=1, filter arg only) => `(child+1)*(1+limit*0.01)` | iface :8 |
| discussions, commenters (iface :10, :12) | plain connections `1 + child` | |
| closedAt, mergedAt | C=5 | 23, 47 (C at 23-24) |
| subscribed | C=5 | 229 |
| assignees, reviewers | C=5 (plain connection, no multiplier) | 185, 224 |
| labels | C=5 (BulkLabelsResolver, constant: no multiplier) | 199 |
| participants | C=15 (constant) | 212 |
| committers | C=5 (constant; G ignored) | 255 |
| diffStats, diffStatsSummary, diffHeadSha | G | 33, 85, 29 |
| sourceBranchProtected, targetBranchPath, defaultMergeCommitMessage, defaultSquashCommitMessage, divergedFromTargetBranch, detailedMergeStatus, mergeabilityChecks, mergeableDiscussionsState, rebaseInProgress, shouldBeRebased, sourceBranchExists, targetBranchExists, approved, availableAutoMergeStrategies, mergeable | G (2 + child) | 55, 66, 81, 83, 87, 118, 122, 129, 134, 136, 140, 144, 240, 249, 261 |
| commits, commitsWithoutMergeCommits | G plain connection => `2 + child` (no scaling) | 251, 253 |
| pipelines | R-conn, G +1 (resolver G, ResolvesPipelines), resolver `max_page_size 500` (merge_request_pipelines_resolver.rb:15): without first/last limit=500, factor 500*0.01=5.0, i.e. `c + c*5` | 179 |
| linkedWorkItems | G, resolver, non-conn: `child + 1 + 1` | 313 |
| titleHtml, descriptionHtml | C=5 each (markdown_field) | 329, 330 |
| userPermissions | 1 + child | :19 |
All other MR fields (title, iid, author, headPipeline, milestone, awardEmoji(plain conn), approvedBy(plain conn), timelogs(plain conn), currentUserTodos(plain conn), upvotes/downvotes (non-conn resolvers), userNotesCount, ...) are 1.
Note carefully: MR `pipelines` has `max_page_size 500` so a bare `pipelines { nodes { id } }` costs `(child+1+1) * 6`.

### Note (`notes/note_type.rb`, `notes/base_note_interface.rb`)
| field | cost | line |
|---|---|---|
| bodyHtml | G (2 + child) | base_note_interface.rb:30 |
| authorIsContributor | G | note_type.rb:68 |
| awardEmoji | plain connection 1 | base_note_interface.rb:15 |
Everything else (body, author, createdAt, discussion, position, userPermissions ...) = 1.

### Discussion (`notes/discussion_type.rb`, `notes/base_discussion_interface.rb`)
`notes`: plain connection, max_page_size 200 declared but plain => cost 1 + child (no multiplier since non-resolver). Nothing else non-default.

### User (UserCore `types/user_type.rb`, `UserInterface`, `CurrentUser`)
| field | cost | line |
|---|---|---|
| timelogs | C=5 | user_interface.rb:147 (C at 152) |
| todos | R-conn (+sort only if given; TodosResolver sort has no default) | :120 |
| authoredMergeRequests, assignedMergeRequests, reviewRequestedMergeRequests | R-conn, MergeRequestsResolver base => +sort(created_desc) +search | :126-132 |
| groups (Users::GroupsResolver), starredProjects, contributedProjects, snippets, personalAccessTokens, userAchievements, groupMemberships... | R-conn (see resolvers) | :78-269 |
| (CurrentUser) assigneeOrReviewerMergeRequests | R-conn +sort +search | current_user_type.rb:9 |
| (CurrentUser) runners, recentlyViewed* | resolver/conn | :13-41 |
`username`, `name`, `avatarUrl`, etc. = 1. `Query.users` / `Query.user` see Query table.

### MergeRequestReviewer / MergeRequestAssignee / MergeRequestAuthor / MergeRequestParticipant
Subtypes of `UserType`: same as User plus `mergeRequestInteraction` (1 + child).
`types/merge_requests/interacts_with_merge_request.rb`.

### UserMergeRequestInteraction (`types/user_merge_request_interaction_type.rb`)
`canMerge` G (2): :15. `canUpdate`, `reviewState`, `reviewed`, `approved` = 1.

### Pipeline (`types/ci/pipeline_type.rb`)
| field | cost | line |
|---|---|---|
| beforeSha, commit, latest, refText | G (2 + child) | 42, 153, 213, 215 |
| stages | R-conn (PipelineStagesResolver, no sort) | 87 |
| jobs | R-conn (Ci::JobsResolver) | 109 |
| jobArtifacts, testReportSummary, testSuite | non-conn resolvers 1 + child | 131, 170, 176 |
| downstream, warningMessages, errorMessages, manualVariables | plain conns 1 | |
| userPermissions (`updatePipeline`, `cancelPipeline`) | permission: G 2 each | permission_types/ci/pipeline.rb:10-11 |

### CI stages, groups, jobs and statuses (`types/ci/*.rb`)
Recorded in the `ci_complexity` scenario (CE):
| field | cost |
|---|---|
| `Project.pipeline` | `child + 1` without arguments; `child - 8` with `id`, `iid` or `sha`, **floored at 0** (recorded: `pipeline(iid:) { id status }` adds nothing) |
| `Project.jobs` | R-conn (ProjectJobsResolver, no sort) |
| `Project.job`, `Project.pipelineCounts`, `Pipeline.job`, `Pipeline.jobArtifacts` | 1 + child |
| `CiStage.groups`, `CiStage.jobs`, `CiGroup.jobs`, `CiJob.artifacts` | plain conns: 1 + child |
| `DetailedStatus.action`, `DetailedStatus.label` | G (2 + child) |
| `CiJob.canPlayJob` | G (2) |
| `JobPermissions.updateBuild`, `cancelBuild` | G (2 each) |

### Commit (`types/repositories/commit_type.rb`)
title, fullTitle, signature, signatureHtml, diffs: G (2): lines 26, 29, 50, 55, 79. `pipelines`: R-conn with G via ResolvesPipelines (+1, +2 sha/ref): :87. `titleHtml`, `fullTitleHtml`, `descriptionHtml`: C=5 (:92-94).

### Repository (`types/repository_type.rb`)
| field | cost | line |
|---|---|---|
| blobs | G + R-conn, resolver adds `+paths.size` (BlobsResolver:36) | 9 |
| branchNames | C=170 | 11-12 |
| commit, lastCommit, tree | non-conn resolvers + G (`1+1+child`) | 14, 34, 42 |
| commits | R-conn + G, max_page_size 100 | 18 |
| paginatedTree | R-conn + G, max_page_size 100 | 36 |
| empty, exists, rootRef | G | 30, 32, 40 |

### Blob (`types/repository/blob_type.rb`) / Tree (`types/tree/*.rb`)
Repository::Blob: rawBlob C=2 (:54), base64EncodedBlob C=3 (:59), rawTextBlob C=2 (:64), plainData C=5 (:128);
G (2): lfsOid :29, permalinkPath :105, environmentFormattedExternalUrl :109, environmentExternalUrlForRouteMap :113,
canModifyBlob :134, canCurrentUserPushToBranch :141, language :148, codeNavigationPath :154; blame: R non-conn (:99) 1+child.
Tree::Blob: lfsOid G (tree/blob_type.rb:13). Tree::Tree: lastCommit C=10 (tree_type.rb:13, constant, G ignored), permalinkPath G (:28),
submodules G plain conn (:20), blobs G plain conn (:24), trees plain conn 1 (:17).

### Label / Milestone / AwardEmoji / DiffStats / DiffStatsSummary
- Label: descriptionHtml C=5 (`label_type.rb:28`). Everything else 1.
- Milestone: `releases` plain conn 1. Nothing non-default. (Project.milestones is R-conn with sort default, above.)
- AwardEmoji: none. DiffStats: none. DiffStatsSummary: none (the `MergeRequest.diffStats`/`diffStatsSummary` fields are G).
- Todo (`todo_type.rb`): target :34, targetEntity :45, body :58 are G.

### *Permissions types (`app/graphql/types/permission_types/*`)
- No special rule. `MergeRequestPermissions` (`merge_request.rb`): `pushToSourceBranch`, `removeSourceBranch`,
  `cherryPickOnCurrentMergeRequest`, `revertOnCurrentMergeRequest` (:20), `canMerge` (:23), `canApprove` (:24) are G = 2;
  `readMergeRequest`, `adminMergeRequest`, `updateMergeRequest`, `createNote` = 1.
- `NotePermissions`, `DiscussionPermissions`, `ProjectPermissions`, etc.: all 1.
- Other G permission fields: pipeline `updatePipeline`/`cancelPipeline` (`ci/pipeline.rb:10-11`), `ci/pipeline_schedules.rb:12`
  `playPipelineSchedule`, `ci/job.rb:10-11` `updateBuild`/`cancelBuild`, `deployment.rb:9` `updateDeployment`.

### Group / Namespace (for completeness)
Group: timelogs C=5 (`group_type.rb:254`, C line 257), descendantGroups C=5 (:260/263, constant), boards max_page_size 2000 (:148-152).
Namespace: `projects` R-conn +sort +search, resolver_complexity `super + 10` (`namespace_type.rb:62`, `namespace_projects_resolver.rb:77`);
`workItemDescriptionTemplates` G + R-conn (:164); `descriptionHtml` C=5 (:249).
Release: `commit` C=10 (`release_type.rb:52`), `descriptionHtml` C=5.
Other notable fixed costs: Issue `participants`, `subscribed` C=5 (`issue_type.rb:109,112`); MilestoneStats none.

Caveat on this table: the per-resolver "+sort" notes were derived by reading `argument :sort ... default_value` in the resolver or
its parent/concern; for resolvers I did not open, treat as "R-conn, check resolver". The fixed rules (explicit C, G, markdown_field) are exact.

## 4. Mutations

Mutations ARE analysed: `max_complexity` is applied to the whole operation regardless of type, the root is `Mutation` (`types/mutation_type.rb`,
fields via `mount_mutation`, `lib/gitlab/graphql/mount_mutation.rb`). Cost = sum of root mutation field costs.
- Mount creates `field :name, mutation: Klass`. Mutation classes derive from `GraphQL::Schema::RelayClassicMutation`
  (`mutations/base_mutation.rb:4`), NOT `Resolvers::BaseResolver`, so `Klass.complexity` is the gem default (1, **[GEM/UNVERIFIED]**)
  rather than GitLab's 0. `BaseField#initialize` sees an Integer > 0 -> constant complexity 1.
  => mutation field cost = **1 + child** (payload selection), regardless of `calls_gitaly: true` on the mount
  (e.g. notes create `mutation_type.rb:122-126`; G is ignored since the cost is constant). If instead the complexity were 0 the proc path
  would apply: `child + 1(G)`; no `resolver_complexity` (try returns nil -> 0) so cost = child. I rate "1 + child" ~75% likely.
- Input arguments (`input: {...}`) never contribute.
- Payload fields: `errors` 1, `clientMutationId` 1, and the returned object's fields are costed as usual (e.g. `createNote { note { body author { username } } errors }`).
- Same limit selection as queries (admin 300 / user 250 / anon 200).

## 5. Worked examples (authenticated user; limit 250)

Notation: `c(x)` = cost.

### Example 1 — small project query
```graphql
query { project(fullPath: "g/p") { id name } }
```
- id = 1, name = 1 -> child = 2.
- project (ProjectResolver, non-conn, no sort/search): `2 + resolver_complexity(1) = 3`; not a connection -> multiplier skipped. `3`.
- **Total = 3.**

### Example 2 — merge request with notes
```graphql
query { project(fullPath: "g/p") { mergeRequest(iid: "1") {
  notes { nodes { body author { username } } } } } }
```
- `author { username }` = 1 + 1 = 2; `body` = 1 -> nodes children = 3; `nodes` = 1 + 3 = 4.
- `notes` (NotesResolver conn; args = {filter: ALL_NOTES default}; no sort/search): `4 + 1 = 5`; limit = min(100) = 100, mult = 0.01 -> 1.0;
  `5 + 5*1.0 = 10.0` -> `10`.
- `mergeRequest` (MergeRequestResolver single, no sort, non-conn): `10 + 1 = 11`.
- `project`: `11 + 1 = 12`.
- **Total = 12.** Same query with `notes(first: 5)`: `5 + 5*(5*0.01) = 5.25 -> 5`; mergeRequest 6; project 7; **total = 7**.
- Add `title` (1): +1. Add `titleHtml`: +5 (constant). Add `diffStatsSummary { additions deletions }`: 2 + 2 = +4. Add `userPermissions { canMerge }`: 1 + 2 = +3.

### Example 3 — connection with `first: 5` (vs default)
```graphql
query { project(fullPath: "g/p") { mergeRequests(first: 5) {
  nodes { iid title } pageInfo { hasNextPage endCursor } } } }
```
- `nodes { iid title }` = 1 + 2 = 3; `pageInfo { hasNextPage endCursor }` = 1 + 2 = 3 -> child = 6.
- `mergeRequests` (ProjectMergeRequestsResolver): resolver_complexity = 1 + 1 (sort default `created_desc`) = 2 (+5 if `search:` given);
  c = 6 + 2 = 8; connection: limit = min(5, 100) = 5, mult = 0.01 (no iids/iid) -> factor 0.05; `8 + 8*0.05 = 8.4` -> `8`.
- project: `8 + 1 = 9`.
- **Total = 9.** Without `first` (limit 100, factor 1.0): c = 8 + 8 = 16, project 17, **total = 17**.
  With `mergeRequests(first: 5, search: "x")`: resolver_complexity 7, c = 13, `13 + 0.65 = 13.65 -> 13`, project 14.
  With `mergeRequests(iids: ["1"])`: mult 0.05 -> factor `100*0.05 = 5.0` (no `first`): `8 + 40 = 48`.

## 6. Open points to calibrate against a real instance
1. Legacy-mode duplicate response keys and abstract-type fragments (sum vs max vs dedupe). Test: `{ a: queryComplexity{score} b: queryComplexity{score} }` and `{ currentUser { id } currentUser { id } }`, then a fragment-on-interface case.
2. Whether `__typename` and introspection fields count (and cost 1).
3. Mutation field constant 1 vs 0 (query `mutation { createNote(input:{...}) { errors } }` and read the error number when over limit, or use a dry-run via `queryComplexity` in a mutation).
4. Whether keyword_arguments passed to the proc include argument defaults (assumed yes; affects every `sort` resolver by +1).
5. `.single`/inherited resolvers: `calls_gitaly!` is lost on `.single`.
6. Admin: only `User#admin` boolean is checked; PATs/session identical. No admin-mode check exists in `max_query_complexity`.
