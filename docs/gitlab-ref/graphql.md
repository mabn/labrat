# GitLab 18.11 GraphQL API reference (for labrat)

Target: GitLab CE 18.11, graphql-ruby **2.6.10** (from the running
container's Gemfile.lock: `graphql (2.6.10)`).

Evidence conventions:
- `[live]` = observed against http://localhost:8929 (read-only queries only; no mutation was ever sent).
- `[src path:line]` = read from /home/mabn/dev/gitlab-run-ce/gitlab-src (path relative to that dir).
- `[unverified]` = inferred / not exercised (mutations are source-only by rule).

Companion file: `graphql-introspection.json` (full schema of GitLab CE, generated from CE code). Use it as the
authoritative field/arg/enum/input list.

---------------------------------------------------------------------------------------------------

## 1. Endpoint behaviour

Route: `match '/api/graphql', via: [:get, :post], to: 'graphql#execute'` [src config/routes/api.rb:4].
Controller: `app/controllers/graphql_controller.rb`. Any other verb (PUT etc.) -> Rails 404 HTML. [live]

### 1.1 Request shapes

POST, JSON body (normal case):
```
POST /api/graphql
Content-Type: application/json
{"query":"...", "variables":{...}|"<json string>", "operationName":"Name"}
```
- `variables` may be a JSON object, a JSON-encoded string, absent, or null. A non-blank string is parsed
  as JSON (nesting limit 3 levels of string-in-string; parse limits depth 32, arrays/hashes 50000,
  total elements 100000) [src lib/gitlab/graphql/variables.rb:6-50]. [live] `"variables":"{\"p\":\"x\"}"` works.
- Unparseable variables string -> HTTP 422:
  `{"errors":[{"message":"unexpected character (after ) at line 1, column 2 [parse.c:931] in '{bad"}]}` [live]
  (message is the Ruby JSON parser's text; any non-empty message is fine to emulate.)
  Any non-String/Hash/nil `variables` -> 422 `Unexpected parameter: <value>`  [src variables.rb:36-37].
- POST with `application/x-www-form-urlencoded` (`query=...`) also works. [live]
- POST with `Content-Type: text/plain` and a JSON body: params are not parsed, so the query is empty ->
  `200 {"errors":[{"message":"Unexpected end of document","locations":[]}]}` [live]
- Malformed JSON body -> HTTP 400 `{"error":"Invalid JSON format"}`, `Content-Type: application/json`
  (raised by a middleware before the controller). [live]
- Permitted params: `query, operationName, remove_deprecated, variables` (and `_json` for batches)
  [src graphql_controller.rb:178-187].

GET: `GET /api/graphql?query=...&variables=<json string>&operationName=...` [live: works, 200].
- GET with a mutation (or an unparseable/empty query - `mutation?` rescues `GraphQL::ParseError` and
  returns **true**) -> HTTP 422 `{"errors":[{"message":"Mutations are forbidden in GET requests"}]}`
  [src graphql_controller.rb:189-194, 224-235]. [live] `GET /api/graphql` with no query at all returns exactly that 422.
  HEAD behaves like GET.

Batching (Apollo style): body is a JSON **array** (`params[:_json].is_a?(Array)`) [src :344-346].
Each element `{query, variables, operationName}`. Response is an array in the same order, each element a
normal `{"data":..}` or `{"errors":..}` object. HTTP status is 200 even if elements fail. [live]
```
[{"data":{"currentUser":{"id":"gid://gitlab/User/36"}}},
 {"data":{"project":{"id":"gid://gitlab/Project/82"}}},
 {"errors":[{"message":"Field 'bad' doesn't exist on type 'Query'","locations":[{"line":1,"column":3}],"path":["query","bad"],"extensions":{"code":"undefinedField","typeName":"Query","fieldName":"bad"}}]}]
```
Empty array `[]` -> `200 []`. Batch complexity/depth limits are applied per element; query size limit is
applied to the sum of the element query lengths.

### 1.2 Input validation / limits

| Condition | HTTP | Body |
|---|---|---|
| query > 10,000 chars (`MAX_QUERY_SIZE`; 12,000 for a few work-item operationNames) | 422 | `{"errors":[{"message":"Query too large"}]}` [live] |
| missing/empty `query` in POST | 200 | `{"errors":[{"message":"Unexpected end of document","locations":[]}]}` [live] |
| only `{}` JSON body | 200 | same as above [live] |
| parse error | 200 | see 2.1 |
| invalid auth token | 401 | `{"errors":[{"message":"Invalid token"}]}` [live] |
| user cannot access API (blocked, etc.) | 403 | `{"errors":[{"message":"API not accessible for user"}]}` [src :359-367] |
| unhandled exception | 500 | `{"errors":[{"message":"Internal server error"}]}` [src :88-94] |
| `Gitlab::Graphql::Variables::Invalid`, `Errors::ArgumentError` raised in controller (not resolver) | 422 | `{"errors":[{"message":"<msg>"}]}` |
| rate limited / DB timeout | 429 / 503 | `{"errors":[{"message":"..."}]}`; timeout text `Request timed out. Please try a less complex query or a smaller set of records.` [src :6-38] |

`render_error` always emits `{"errors":[{"message":"..."}]}` (no `locations`, no `data`) [src :369-374].
All responses: `Content-Type: application/json; charset=utf-8` (except the 400 above).

`operationName` handling (graphql-ruby) [live]:
- two named operations, no operationName -> 200 `{"errors":[{"message":"An operation name is required"}]}`
- unknown name -> 200 `{"errors":[{"message":"No operation named \"Z\""}]}`
- `operationName: ""` (empty string present) is treated as a name -> `No operation named ""`
  (while an *absent* operationName selects the single operation).
- Paths in validation errors start with the operation label: `["query","project",...]` for anonymous
  operations, `["query Foo","project",...]` for `query Foo {...}`, `["mutation X",...]`, `["fragment F",...]`.

Other quirks: `{ currentUser @skip(if: true) { id } }` -> `{"data":{}}`. A document with an unused fragment ->
`Fragment X was defined, but not used`. [live]

### 1.3 Authentication

Auth is "sessionless" (`authenticate_sessionless_user!(:graphql_api)`) plus session cookie. Accepted token
transports [live, all returned `currentUser` = claude]:
- `Authorization: Bearer <PAT>`
- `PRIVATE-TOKEN: <PAT>`
- query param `?private_token=<PAT>` and `?access_token=<PAT>`
- HTTP Basic with the token as password (`-u claude:<PAT>`).
(OAuth tokens/job tokens also go through the standard `Gitlab::Auth::AuthFinders`; not exercised.)

- Bad token (any transport) -> **401** `{"errors":[{"message":"Invalid token"}]}` [live]. This is checked in
  `authorize_access_api!` when a token is *present* but yields no user [src :359-362].
- No credentials = anonymous: HTTP 200, `currentUser` is `null`; public data readable:
  `{"data":{"currentUser":null,"project":{"id":"gid://gitlab/Project/82","name":"app","visibility":"public"}}}`;
  private project / its MR resolve to `null` **without** an error. [live]
- Anonymous `users` query -> field error (not null-silent) [live]:
  ```json
  {"errors":[{"message":"The resource that you are attempting to access does not exist or you don't have permission to perform this action","locations":[{"line":1,"column":101}],"path":["users"]}],"data":{"project":null,"mergeRequest":null,"users":null}}
  ```
- Anonymous mutations: `execute_graphql_mutation` is prevented for anonymous [src app/policies/global_policy.rb:59,69,86];
  `BaseMutation.authorized?` then raises `ResourceNotAvailable` -> the same top-level error as above with
  `path:["<mutationName>"]` and `"data":{"<mutationName>":null}` [src app/graphql/mutations/base_mutation.rb:58-66]. [unverified, source]
- Token scopes: queries need `api` or `read_api`; mutations need `api` [src base_mutation.rb:70-72, base_field.rb scopes].
- CSRF is only enforced for cookie-session mutations; irrelevant for token clients.

### 1.4 Response envelope

- Success: `{"data":{...}}`. Field-level errors: `{"errors":[...],"data":{...}}` - **`errors` key is serialized
  before `data`** [live], and the failing nullable field is `null` in `data`.
- Document-level (validation/variable/parse) errors: `{"errors":[...]}` with **no `data` key**, HTTP 200. [live]
- `extensions` is only present on some errors (see section 2). `locations` is present on parse/validation/field
  errors, but an empty array `[]` for empty documents, and **absent** for operation-level errors
  (`An operation name is required`, `No operation named`).
- At most **5 validation errors** are reported (`validate_max_errors 5`) [src app/graphql/gitlab_schema.rb:36]. [live: `{ a b c d e f g h }` -> 5 errors a..e]
- Introspection (`__schema`/`__type`) is special-cased - see section 8.
- `?remove_deprecated=true` (query param **or** body key `remove_deprecated: true`) hides deprecated fields from
  the schema for that request: querying `mergeStatus` then gives `Field 'mergeStatus' doesn't exist on type 'MergeRequest' (Did you mean `mergeStatusEnum`?)`. [live]
  Experiment-status fields (e.g. `MergeRequest.mergeabilityChecks`) are also marked deprecated and therefore hidden by this flag.

---------------------------------------------------------------------------------------------------

## 2. Error formats (all [live] unless stated)

Notation: `locations` are 1-based `{line, column}` of the offending token.

### 2.1 (a) Parse / syntax errors
```
{ currentUser { id
```
```json
{"errors":[{"message":"Expected NAME, not end of file at [1, 17]","locations":[{"line":1,"column":17}]}]}
```
Empty/missing query:
```json
{"errors":[{"message":"Unexpected end of document","locations":[]}]}
```
No `extensions`, no `path`.

### 2.2 (b) Unknown field
`{ currentUser { nope } }`
```json
{"errors":[{"message":"Field 'nope' doesn't exist on type 'CurrentUser'","locations":[{"line":1,"column":17}],"path":["query","currentUser","nope"],"extensions":{"code":"undefinedField","typeName":"CurrentUser","fieldName":"nope"}}]}
```
With a suggestion: `Field 'id2' doesn't exist on type 'Project' (Did you mean `id`?)`.
Unknown on a mutation/subscription root: path starts with `"mutation"`/`"subscription"`.
Unknown root field with query named: `"path":["query Foo","__type"]`.

### 2.3 (c) Missing required argument
`{ project { id } }`
```json
{"errors":[{"message":"Field 'project' is missing required arguments: fullPath","locations":[{"line":1,"column":3}],"path":["query","project"],"extensions":{"code":"missingRequiredArguments","className":"Field","name":"project","arguments":"fullPath"}}]}
```
(multiple missing: `arguments` is a comma-joined list.)

### 2.4 (d) Wrong argument type / unknown argument / bad enum (literal in query)
- Literal Int for String!: `mergeRequest(iid: 1)`
```json
{"errors":[{"message":"Argument 'iid' on Field 'mergeRequest' has an invalid value (1). Expected type 'String!'.","locations":[{"line":1,"column":43}],"path":["query","project","mergeRequest","iid"],"extensions":{"code":"argumentLiteralsIncompatible","typeName":"Field","argumentName":"iid"}}]}
```
- Bad enum literal `state: bogus`: `Argument 'state' on Field 'mergeRequests' has an invalid value (bogus). Expected type 'MergeRequestState'.` same shape/code.
- Argument on input object: `Argument 'authorUsername' on InputObject 'MergeRequestsResolverNegatedParams' has an invalid value (5). Expected type 'String'.`, `typeName":"InputObject"`.
- Unknown argument: `project(id: "x")`
```json
{"message":"Field 'project' doesn't accept argument 'id'","locations":[{"line":1,"column":31}],"path":["query","project","id"],"extensions":{"code":"argumentNotAccepted","name":"project","typeName":"Field","argumentName":"id"}}
```
  For input objects: `InputObject 'MergeRequestsResolverNegatedParams' doesn't accept argument 'bogus'` with `typeName":"InputObject"`.
- Invalid GID literal (custom scalar coercion) - see section 3.3: `code":"argumentLiteralsIncompatible","typeName":"CoercionError"`.
- Note: an Int literal where an `ID` is expected is **accepted** (`project(fullPath: 5)` -> `{"data":{"project":null}}`); ID coerces from Int/String.
- Selection mismatch: scalar with selection / object without: `Field must have selections (field 'descriptionVersion' returns DescriptionVersion but has no selections. Did you mean 'descriptionVersion { ... }'?)`, `"extensions":{"code":"selectionMismatch","nodeName":"field 'descriptionVersion'","typeName":"DescriptionVersion"}`.

### 2.5 (e) Variables
Undeclared variable used: `{ project(fullPath:$p) { id } }`
```json
{"errors":[{"message":"Variable $p is used by anonymous query but not declared","locations":[{"line":1,"column":20}],"path":["query","project","fullPath"],"extensions":{"code":"variableNotDefined","variableName":"p"}}]}
```
Declared but unused: `Variable $i is declared by anonymous query but not used`, `"path":["query"]`, `"extensions":{"code":"variableNotUsed","variableName":"i"}`, location = the operation `{line:1,column:1}`.

Declared type does not match the argument type: `query($p: String!){ project(fullPath:$p){id} }`
```json
{"errors":[{"message":"Type mismatch on variable $p and argument fullPath (String! / ID!)","locations":[{"line":1,"column":29}],"path":["query","project","fullPath"],"extensions":{"code":"variableMismatch","variableName":"p","typeName":"String!","argumentName":"fullPath","errorMessage":"Type mismatch"}}]}
```
(Strict: `String` is NOT usable where `ID` is expected; `ID!` vs `MergeRequestID!` also mismatches.)

Variable value wrong / missing / null for non-null (no `path`; `locations` = the variable definition):
```json
{"errors":[{"message":"Variable $p of type ID! was provided invalid value","locations":[{"line":1,"column":7}],"extensions":{"value":null,"problems":[{"path":[],"explanation":"Expected value to not be null"}]}}]}
```
(Same for missing variable: `value: null`.) Other `explanation`s [live]:
- `Could not coerce value {a:1} to ID`, `Could not coerce value "x" to Int`, `Could not coerce value 3000000000 to Int` (Int is 32-bit)
- `Expected "bogus" to be one of: merged, opened, closed, locked, all` (enum)
- `no time information in "not a time"` (Time scalar; `problems[0]` also carries `"message"` with the same text)
- invalid GID: `{"path":[],"explanation":"\"garbage\" is not a valid Global ID","message":"\"garbage\" is not a valid Global ID"}`
- input object fields: message becomes `Variable $n of type MergeRequestsResolverNegatedParams was provided invalid value for authorUsername (Could not coerce value 5 to String), bogus (Field is not defined on MergeRequestsResolverNegatedParams)`, with `problems:[{"path":["authorUsername"],"explanation":"Could not coerce value 5 to String"},{"path":["bogus"],"explanation":"Field is not defined on MergeRequestsResolverNegatedParams"}]` and `value` echoing the whole variable.
  This is the format a mutation `$input` problem will have, e.g. for `noteableId`
  `... was provided invalid value for noteableId (...)` `problems[0].path == ["noteableId"]` [unverified for mutations].

Other validation: `Fragment F was used, but not defined` (`code":"useAndDefineFragment","fragmentName":"F"`, path `["query","project","... F"]`);
`Directive @bogus is not defined` (`code":"undefinedDirective","directiveName":"bogus"`).

### 2.6 (f) Resource not found => null, no error
- `project(fullPath:"nope")` -> `{"data":{"project":null}}` [live]
- `project.mergeRequest(iid:"99")` -> `null` [live]
- `mergeRequest(id:"gid://gitlab/MergeRequest/99999")` -> `{"data":{"mergeRequest":null}}` [live]
- also `null` (silently) when the object exists but the viewer lacks `read_*` permission (type-level `authorize :read_merge_request`
  [src app/graphql/types/merge_request_type.rb:13], unauthorized objects resolve to null; in lists they are filtered). [live: anonymous on private project]
- `mergeRequest(id:"gid://gitlab/MergeRequest/abc")` -> null (non-numeric id finds nothing, no error). [live]
- Connections of an unreachable parent: the parent is null so there is nothing to descend into.

### 2.7 (g) Mutation errors: payload `errors` vs top-level `errors`
Every mutation payload has `errors: [String!]!` (non-null; `[]` on success) and `clientMutationId` [src mutations/base_mutation.rb:16-19].
- Domain/validation failures (model validation, service refusal) -> HTTP 200, `data.<mutation>.errors = ["..."]`, the object field
  (`mergeRequest`/`note`/...) is `null` or the unchanged object depending on mutation (section 7). No top-level `errors`.
  Strings are `record.errors.full_messages` [src base_mutation.rb:29-31].
- Authorization failures / not found / anonymous / read-only instance -> top-level error with the standard message (2.8),
  `path:["<mutationName>"]`, and `data:{"<mutationName>":null}`.
- `Gitlab::Graphql::Errors::ArgumentError` raised inside a resolver (e.g. `timeEstimate must be formatted correctly, for example `1h 30m``,
  `position oldLine or newLine arguments are required`, `Note cannot be converted to a resolvable thread`) -> top-level
  error `{"message":"...","locations":[..],"path":["<mutationName>"]}` with `data:{"<mutationName>":null}`; no `extensions`.

### 2.8 (h) "The resource that you are attempting to access does not exist or you don't have permission to perform this action"
Constant `RESOURCE_ACCESS_ERROR` [src lib/gitlab/graphql/authorize/authorize_resource.rb:9-10], raised as
`Gitlab::Graphql::Errors::ResourceNotAvailable` (a `GraphQL::ExecutionError`). Appears when:
- a mutation's target (MR/Note/Discussion/Awardable/Project) is not found OR the user lacks the required ability
  (`authorized_find!` / `authorize!`) - deliberately indistinguishable;
- anonymous user runs a mutation, or a resolver that requires a user (`users`);
- read-only DB instance (message instead: `You cannot perform write operations on a read-only instance`);
- discussion reply target unreadable: `The discussion does not exist or you don't have permission to perform this action`
  [src mutations/notes/create/note.rb:43-50];
- awardEmoji on non-awardable: `You cannot add emoji reactions to this resource.` [src award_emojis/base.rb:7,189-195];
- notes rate limit: `This endpoint has been requested too many times. Try again later.` [src notes/create/base.rb:56-60].
Shape [live, from `users`]: `{"message":"<text>","locations":[{"line":L,"column":C}],"path":["users"]}` - **no `extensions`**, and the
field in `data` is `null`.

Other runtime-error messages seen [live]:
- `x is not a valid GitLab ID.` (from `GitlabSchema.parse_gid`; for resolvers taking `[ID]`), path `["users"]`, `"data":{"users":null}`
- `Only one of [assigneeUsernames, assigneeUsername, assigneeWildcardId] arguments is allowed at the same time.` path `["project","mergeRequests"]`, `"data":{"project":{"mergeRequests":null}}`
- `Can only provide either `first` or `last`, not both` (path ends at `nodes`, parent fields remain, `"nodes":null`)
- `Please provide a valid cursor` (cursor decodes but is not valid JSON), `Invalid input: "garbage"` (not base64) - keyset connections
- ArgumentError message `"<x> is not a valid ID for <ExpectedTypes>."` from `parse_gid` with `expected_type`.
Note: *runtime* error `path` has no `"query"` prefix (`["users"]`, `["project","mergeRequests"]`) while *validation* error `path` does.

### 2.9 (i) Complexity / depth / recursion
Reported as top-level errors with HTTP 200, **no `locations`, no `extensions`, no `data`** [live]:
```json
{"errors":[{"message":"Query has complexity of 302, which exceeds max complexity of 300"}]}
{"errors":[{"message":"Query has depth of 21, which exceeds max depth of 20"}]}
```
Recursion analyzer (same field *name* with sub-selection nested > 2 times in one branch; ignores `node edges nodes ofType`)
[src lib/gitlab/graphql/query_analyzers/ast/recursion_analyzer.rb]:
```json
{"errors":[{"message":"Recursive query - too many of fields '{\"parent\"=>12}' detected\nin single branch of the query\")\n"}]}
```
(message contains Ruby `Hash#to_s`, `=>` JSON-escaped as `>`; and a literal trailing `")\n`.) Several of these can appear in one `errors` array
(recursion error comes first, then depth). Limits: section 9.

---------------------------------------------------------------------------------------------------

## 3. Global IDs

Format `gid://gitlab/<ModelClassName>/<id>`; `<id>` is the integer primary key (string for Commit sha / Discussion sha).
`Class::Name` namespacing is kept verbatim (no escaping). Output is always the canonical string.

Observed [live] / known from schema:
| Type | Example | Notes |
|---|---|---|
| MergeRequest | `gid://gitlab/MergeRequest/60` | numeric `merge_requests.id` (NOT iid). `MergeRequest.iid` is a separate **String** field (`"1"`) |
| Project | `gid://gitlab/Project/66` | |
| Group | `gid://gitlab/Group/196` | `Project.group` and `Project.namespace` both give `Group/<id>` for group projects |
| User namespace | `gid://gitlab/Namespaces::UserNamespace/37` | `User.namespace` |
| User | `gid://gitlab/User/36` | |
| Note (regular/system) | `gid://gitlab/Note/129` | class name of the STI row |
| DiffNote | `gid://gitlab/DiffNote/138` | **STI class name**, not `Note` |
| DiscussionNote | `gid://gitlab/DiscussionNote/135` | notes belonging to a (non-diff) thread |
| Discussion | `gid://gitlab/Discussion/b7f2ecac94a3bad290eef27fbab24f9ca6ae170d` | `Discussion.id` is always `Discussion/<40-hex sha1>` = notes.discussion_id (random hex) |
| Discussion replyId | `gid://gitlab/IndividualNoteDiscussion/<sha>` / `gid://gitlab/DiffDiscussion/<sha>` / `gid://gitlab/Discussion/<sha>` | class depends on discussion kind [src types/notes/base_discussion_interface.rb:197-199]; same sha as `id` |
| Label | `gid://gitlab/ProjectLabel/13` (project label) / `GroupLabel/<id>` | type `LabelID` |
| Commit | `gid://gitlab/Commit/851d7388399fec3d1608b8cbca0544c727c41fc5` | full sha |
| SystemNoteMetadata | `gid://gitlab/SystemNoteMetadata/111` | |
| ProjectMember | `gid://gitlab/ProjectMember/90` | |
| Milestone | `gid://gitlab/Milestone/<id>` | `Milestone.id` type is plain `ID`; `iid` is `ID` |
| MergeRequestDiff | `gid://gitlab/MergeRequestDiff/<id>` | only via deprecated experiment field `MergeRequest.mergeRequestDiffs` |
| Pipeline | `gid://gitlab/Ci::Pipeline/<id>` | `headPipeline` (null in this env) |
| AwardEmoji | **no `id` field** | `AwardEmoji` exposes name, emoji, unicode, unicodeVersion, description, user only |
| Todo | `gid://gitlab/Todo/<id>` | |

Awardable ids passed to `awardEmoji*` are the awardable's own GID (`gid://gitlab/MergeRequest/60`, `gid://gitlab/Note/130`, `gid://gitlab/DiffNote/138` ...).
There is **no root `node(id:)` field** in GitLab's schema (`Field 'node' doesn't exist on type 'Query' (Did you mean `note`?)`). Root lookups:
`mergeRequest(id: MergeRequestID!)`, `note(id: NoteID!)` (deprecated), `project(fullPath: ID!)`, `group(fullPath: ID!)`, `namespace(fullPath: ID!)`,
`user(id: UserID, username: String)`, `users(...)`, `projects(...)`, `currentUser`, `metadata`.

### 3.1 Typed ID scalars
`MergeRequestID`, `ProjectID`, `NoteID`, `UserID`, `LabelID`, `DiscussionID`, `NoteableID`, `AwardableID`, `MilestoneID`... are
`Types::GlobalIDType[Model]` scalars [src app/graphql/types/global_id_type.rb]. Input coercion:
1. `GlobalID.parse(value)` fails -> `"<inspect value>" is not a valid Global ID`
2. app part != `gitlab` -> `"<value>" is not a Gitlab Global ID`
3. model class (via `safe_constantize`) not a subclass/same as expected model -> `"<value>" does not represent an instance of <Model>`
   (subclasses accepted: a `NoteID` accepts `DiffNote/..`, `DiscussionNote/..`; `DiscussionID` accepts `DiffDiscussion/..`, `IndividualNoteDiscussion/..`; `NoteableID` accepts MergeRequest/Issue/Commit/...).
4. then looked up; a missing record gives `null` (queries) - not an error.
Result coercion of `note.id` for an STI note yields the STI class name.

### 3.2 Invalid GID in a literal argument [live]
```
mergeRequest(id:"garbage")
{"errors":[{"message":"\"garbage\" is not a valid Global ID","locations":[{"line":1,"column":3}],"path":["query","mergeRequest","id"],"extensions":{"code":"argumentLiteralsIncompatible","typeName":"CoercionError"}}]}
mergeRequest(id:"gid://gitlab/Project/66")   -> "\"gid://gitlab/Project/66\" does not represent an instance of MergeRequest"
mergeRequest(id:"gid://gitlab/Nope/1")       -> "\"gid://gitlab/Nope/1\" does not represent an instance of MergeRequest"
mergeRequest(id:"gid://other/MergeRequest/1")-> "\"gid://other/MergeRequest/1\" is not a Gitlab Global ID"
mergeRequest(id:"gid://gitlab/MergeRequest/") -> not a valid Global ID
mergeRequest(id:"60")  -> "\"60\" is not a valid Global ID"      ; mergeRequest(id: 60) -> "60 is not a valid Global ID"
```
Same error shape for every typed-ID argument. Valid-but-missing -> `{"data":{"mergeRequest":null}}`.

### 3.3 Invalid GID via variable [live]
```json
{"errors":[{"message":"Variable $i of type MergeRequestID! was provided invalid value","locations":[{"line":1,"column":7}],"extensions":{"value":"garbage","problems":[{"path":[],"explanation":"\"garbage\" is not a valid Global ID","message":"\"garbage\" is not a valid Global ID"}]}}]}
```
Using `$i: ID!` for a `MergeRequestID!` argument -> `variableMismatch` (`Type mismatch on variable $i and argument id (ID! / MergeRequestID!)`).
In mutation inputs (`$input: CreateNoteInput!`) the problem path is the field: `"path":["noteableId"]` [unverified].

### 3.4 Plain `ID`/`[ID!]` arguments that GitLab parses itself
e.g. `users(ids: ["gid://gitlab/User/36","x"])` -> top-level `x is not a valid GitLab ID.` with `"data":{"users":null}` (see 2.8). [live]

---------------------------------------------------------------------------------------------------

## 4. Connections and pagination

### 4.1 Shape
Every `*Connection` type has `edges { cursor node }`, `nodes`, `pageInfo { hasNextPage hasPreviousPage startCursor endCursor }` (+ `count` on
Countable connections, see 4.5). Arguments on all: `first: Int, after: String, last: Int, before: String`. `PageInfo` fields:
`hasNextPage: Boolean!`, `hasPreviousPage: Boolean!`, `startCursor: String`, `endCursor: String` (null when empty). [live]
Empty connection: `"pageInfo":{"hasNextPage":false,"hasPreviousPage":false,"startCursor":null,"endCursor":null},"count":0`.

### 4.2 Page size
- `default_max_page_size 100` [src app/graphql/gitlab_schema.rb:31]. No `first`/`last` -> returns up to 100. `first: 101` -> silently clamps to 100 (no error). [live (4 rows only; clamping is `[first,last,max].min`)]
  `limit_value = [first, last, max_page_size || 100].compact.min` [src lib/gitlab/graphql/pagination/keyset/connection.rb:136-138].
- Per-field overrides: `Discussion.notes` 200 [src types/notes/discussion_type.rb:21]; `Project.pipelines`-style fields 2000 (project_type.rb:574, group_type.rb:152);
  `MergeRequest.pipelines` 500; commits `min(first, 100)`; repository tree/blobs 100.
- `first: -1` -> returns empty `nodes` (no error) [live]. `first: 0` -> empty [unverified]. `first` + `last` together on keyset connections ->
  `Can only provide either `first` or `last`, not both` at path `[..., "nodes"]` with `nodes: null` [live].
- `last: N` without `before`: last N items, `hasPreviousPage` true if more existed, `hasNextPage` false. [live]
- `hasNextPage` (keyset): with `first`: `fetched(limit+1) > limit`; true whenever `before` is given; `hasPreviousPage`: true whenever `after` is given;
  false when only `first`/`before`; with `last`: whether an extra row existed. [src keyset/connection.rb:34-62]

### 4.3 Cursor formats [live, decoded]
Cursor encoder: url-safe Base64 **without padding** (e.g. `MQ` = `1`).
| Connection kind | Decoded cursor | Examples |
|---|---|---|
| Array connections (`MergeRequest.discussions`, `participants`, `commits`, other array-backed) | decimal string of **1-based position** of the item (`"1"`, `"2"`,...) | `MQ`,`Mg`,`Mw`,`NA`,... `MTU`=15. `after: "Mw"` starts at item 4 |
| Keyset over created_at (default MR sort, `MergeRequest.notes`) | JSON `{"created_at":"2026-11-07 07:05:47.255965000 +0000","id":"74"}` | `eyJjcmVhdGVkX2F0IjoiMjAyNi0xMS0wNyAwNzowNTo0Ny4yNTU5NjUwMDAgKzAwMDAiLCJpZCI6Ijc0In0` (values are strings; time = UTC, 9 fractional digits, ` +0000`) |
| Keyset sort `UPDATED_*` | `{"updated_at":"...","id":"73"}` | |
| Keyset `TITLE_*` | `{"title":"alpha","id":"72"}` | |
| Keyset `MERGED_AT_*` | `{"merge_request_metrics_merged_at":null,"merge_request_metrics_id":"660"}` | |
| Keyset by id only (`reviewers`, `assignees`, `approvedBy`, labels?, `awardEmoji`) | `{"id":"54"}` | `eyJpZCI6IjU0In0` |
A cursor is only meaningful to the connection that issued it; clients treat it as opaque, so labrat may use any scheme but should keep
these shapes if it wants byte-compat. Array connections: a non-numeric `after`/`before` (e.g. `Zm9v`, `abc`) is **ignored without error**
(treated as start) [live]. Keyset connections: non-base64 -> `Invalid input: "garbage"`; base64 of non-JSON -> `Please provide a valid cursor`. [live/src]
Cursors are returned on `edges[].cursor`, `pageInfo.startCursor/endCursor`.

### 4.4 Ordering
- `Project.mergeRequests` default `sort: created_desc` (argument default) [src app/graphql/resolvers/merge_requests_resolver.rb:~170 `default_value: :created_desc`];
  keyset (created_at DESC, id DESC): `iid 4,3,2,1` for 4 MRs. [live] Sort enum `MergeRequestSort` values are UPPERCASE: `CREATED_ASC/DESC, UPDATED_ASC/DESC, MERGED_AT_ASC/DESC, CLOSED_AT_ASC/DESC, TITLE_ASC/DESC, PRIORITY_*, LABEL_PRIORITY_*, MILESTONE_DUE_*, POPULARITY_*`
  (lower-case `created_desc` etc. still exist but are deprecated aliases). `priority/popularity/label_priority/milestone_due` sorts fall back to **offset** pagination (cursor = offset) [src concerns/resolves_merge_requests.rb:8-11,38-43].
- `Project.labels` sorted by title ascending (observed `bug, devlabel, new one`). [live]
- `MergeRequest.discussions`: ordered by first-note creation (array); IndividualNote discussions are one per user note / system note. [live]
- `MergeRequest.notes`: created_at ASC, keyset. [live]
- `MergeRequest.commits`: newest first (array order from git log) [live: head commit first].
- `MergeRequest.participants`: array; observed `bob, claude, root` (author/commenters/mentions) [live].

### 4.5 `count`
- Countable connections (`CountableConnectionType`): `count(limit: Int): Int!`. Without `limit` exact count. With `limit: n` returns `min(count, n+1)` ("Returns limit + 1 for counts greater than the limit").
  [live: `count(limit:2)` over 3 MRs -> 3] [src types/countable_connection_type.rb]. `limit` must be > 0 (numericality validator).
- Which connections have `count`: `MergeRequestConnection` (also `totalTimeToMerge`), `NoteConnection` (`MergeRequest.notes` returns `count:19` all / `9` ONLY_COMMENTS / `10` ONLY_ACTIVITY),
  `LabelConnection`, `UserCoreConnection`, `MergeRequestParticipantConnection`... **Not** on `DiscussionConnection` or `CommitConnection`
  (`Field 'count' doesn't exist on type 'DiscussionConnection'`). Check the introspection per type. `LimitedCountableConnectionType` default limit 1000 (max 1000).
- `MergeRequest.notes(filter: NotesFilterType = ALL_NOTES)` enum `ALL_NOTES | ONLY_COMMENTS | ONLY_ACTIVITY`. [live]

### 4.6 `nodes` vs `edges`
Both always available and consistent; `nodes` is plain list of nodes; `edges[].cursor` per-item. Authorization filtering removes items from both.

---------------------------------------------------------------------------------------------------

## 5. MergeRequest type (live samples; MR `rev-r12b0c0/app !1`, id 60)

Entry points: `project(fullPath:"g/p"){ mergeRequest(iid:"1"){...} mergeRequests(first:..){...} }`, `mergeRequest(id:"gid://gitlab/MergeRequest/60")`.
`MergeRequest implements CurrentUserTodos, NoteableInterface, Todoable`. Full list: introspection json. Fields that **do not exist**: `sha` (use `diffHeadSha`),
`isDraft`, `author.id` is `UserID`... `MergeRequestPermissions.approveMergeRequest` (it is `canApprove`).

### 5.1 Scalars and formats
| Field | Type | Observed value / rule |
|---|---|---|
| `id` | `ID!` | `"gid://gitlab/MergeRequest/60"` |
| `iid` | `String!` | `"1"` (string!) |
| `title` | `String!` | `"Add app"`; `titleHtml: String` |
| `description` | `String` | **`null` when empty/never set** (not `""`) |
| `descriptionHtml` | `String` | `""` when description null; else Markdown-rendered, e.g. `<p data-sourcepos="1:1-1:12" dir="auto">Use logging.</p>` |
| `state` | `MergeRequestState!` | enum names are **lowercase**: `opened`, `closed`, `merged`, `locked` (`all` is only valid as a filter) |
| `draft` | `Boolean!` | |
| `createdAt`/`updatedAt` | `Time!` | ISO8601 UTC with `Z`, whole seconds: `"2026-11-07T06:12:35Z"` (`Time#iso8601`, [src types/time_type.rb:21-23]). Values that are plain Ruby `Time` with offset serialize as `+00:00` (git commit dates: `"2026-11-07T06:13:33+00:00"`) |
| `closedAt`, `mergedAt` | `Time` | null unless closed/merged (from MR metrics) |
| `preparedAt` | `Time` | `"2026-11-07T06:12:37Z"` |
| `sourceBranch`, `targetBranch` | `String!` | |
| `webUrl` | `String` | `http://localhost:8929/<ns>/<proj>/-/merge_requests/1` ; `webPath: String!` = `/ns/proj/-/merge_requests/1` |
| `reference(full:false)` | `String!` | `"!1"`; full: `"rev-r12b0c0/app!1"` |
| `projectId`, `targetProjectId`, `sourceProjectId` | `Int` | numeric project id |
| `project`/`targetProject`/`sourceProject` | `Project` | |
| `diffHeadSha` | `String` | 40-hex; **null if source branch missing** |
| `diffRefs` | `DiffRefs` | `{baseSha, headSha, startSha}` (baseSha nullable; headSha/startSha `String!`) |
| `mergeCommitSha` | `String` | set once merged; `inProgressMergeCommitSha`, `rebaseCommitSha` also |
| `squash` | `Boolean!` | value of the MR flag (`false`) |
| `squashOnMerge` | `Boolean!` | takes project squash policy into account |
| `squashReadOnly` | `Boolean!` | |
| `shouldRemoveSourceBranch` | `Boolean` | **null** unless the MR flag was explicitly set (`null` in all samples) |
| `forceRemoveSourceBranch` | `Boolean` | project-setting driven (`true` observed in this project) |
| `timeEstimate`, `totalTimeSpent` | `Int!` | seconds, `0` default (`timeEstimate:12600` on one MR) |
| `humanTimeEstimate`, `humanTotalTimeSpent` | `String` | null when 0 |
| `subscribed` | `Boolean!` | for the current user |
| `discussionLocked` | `Boolean!` | |
| `commitCount` | `Int` | 0 when source branch gone |
| `userNotesCount` | `Int` | non-system notes (9) ; `userDiscussionsCount` `Int` (6) |
| `upvotes`, `downvotes` | `Int!` | counts of `thumbsup`/`thumbsdown` award emoji (1 / 0) |
| `resolvableDiscussionsCount`, `resolvedDiscussionsCount` | `Int` | (4 / 1) |
| `mergeableDiscussionsState` | `Boolean` | true if no unresolved resolvable threads |
| `headPipeline` | `Pipeline` | **null** (no CI in this env) ; `hasCi: Boolean!` false |
| `milestone` | `Milestone` | null |
| `mergeUser` | `UserCore` | merged-by (metrics) or auto-merge user; null |
| `autoMergeEnabled` | `Boolean!`, `autoMergeStrategy: String` (null), `availableAutoMergeStrategies: [String!]` (`[]` here) | |
| `sourceBranchExists`/`targetBranchExists` | `Boolean!` | |
| `divergedFromTargetBranch`, `shouldBeRebased`, `rebaseInProgress`, `mergeOngoing` | `Boolean!` | |
| `allowCollaboration`, `retargeted`, `hidden` | `Boolean` | `false` |
| `taskCompletionStatus` | `{count, completedCount}` | `0/0` |
| `mergeStatus` | `String` | **deprecated**: lower-case `can_be_merged` |

### 5.2 Merge status
`mergeStatusEnum: MergeStatus` values `UNCHECKED, CHECKING, CAN_BE_MERGED, CANNOT_BE_MERGED, CANNOT_BE_MERGED_RECHECK`.
`detailedMergeStatus: DetailedMergeStatus` values: `UNCHECKED, CHECKING, MERGEABLE, COMMITS_STATUS, CI_MUST_PASS, CI_STILL_RUNNING,
DISCUSSIONS_NOT_RESOLVED, DRAFT_STATUS, NOT_OPEN, NOT_APPROVED, BLOCKED_STATUS, EXTERNAL_STATUS_CHECKS, PREPARING, JIRA_ASSOCIATION, CONFLICT,
NEED_REBASE, APPROVALS_SYNCING, LOCKED_PATHS, LOCKED_LFS_FILES, MERGE_TIME, SECURITY_POLICIES_VIOLATIONS, TITLE_NOT_MATCHING`. Computed by `MergeRequests::Mergeability::DetailedMergeStatusService` [src merge_request_type.rb:394-396].
Observed combinations [live]:
| situation | detailedMergeStatus | mergeStatusEnum | mergeable | conflicts |
|---|---|---|---|---|
| open, clean | `MERGEABLE` | `CAN_BE_MERGED` | true | false |
| open, conflicting | `CONFLICT` | `CANNOT_BE_MERGED` | false | true |
| open, source branch deleted (0 commits) | `COMMITS_STATUS` | `CANNOT_BE_MERGED_RECHECK` | false | false |
| merged | `NOT_OPEN` | `CAN_BE_MERGED` | false | false |
| closed | `NOT_OPEN` | `CAN_BE_MERGED` or `CANNOT_BE_MERGED` (last computed value) | false | (conflicts mirrors `cannot_be_merged?`) |
`conflicts` = `cannot_be_merged?` (merge_status == cannot_be_merged) [src merge_request_type.rb:208]. `mergeable` = `mergeable?`.

`mergeabilityChecks: [MergeRequestMergeabilityCheck!]!` (experiment, hidden by `remove_deprecated`): list of `{identifier, status}` in this fixed
order (open, clean) [live]:
```
NOT_OPEN SUCCESS | DRAFT_STATUS SUCCESS | MERGE_TIME INACTIVE | LOCKED_LFS_FILES SUCCESS | DISCUSSIONS_NOT_RESOLVED INACTIVE |
CI_MUST_PASS INACTIVE | COMMITS_STATUS SUCCESS | NEED_REBASE INACTIVE | CONFLICT SUCCESS
```
Status enum: `SUCCESS, CHECKING, FAILED, INACTIVE, WARNING`.

### 5.3 Approvals [live + src]
- `approved: Boolean!` is **`approvals.present?`** (true iff anybody approved) [src app/models/concerns/approvable.rb:59-61]:
  unapproved open MR `approved:false`; MR approved by bob `approved:true`.
- `approvedBy(first..): UserCoreConnection` -> `{"nodes":[{"id":"gid://gitlab/User/54","username":"bob-r12b0c0"}]}` (cursor `{"id":"54"}`).
- `userPermissions.canApprove: Boolean!`.
- **No approve/unapprove GraphQL mutation exists in 18.11** (no `mergeRequestApprove`/`mergeRequestUnapprove` in `Mutation`).
  Approvals must go through REST `POST /projects/:id/merge_requests/:iid/approve|unapprove`. `mergeRequestRequestChanges` & `mergeRequestReviewerRereview` exist.

### 5.4 People
- `author: MergeRequestAuthor` (implements User): `id gid User/<n>`, `username`, `name`, `state` (`active`), `avatarUrl` (gravatar URL when no avatar: `https://www.gravatar.com/avatar/<64hex>?s=80&d=identicon`), `webUrl` (`http://localhost:8929/claude`), `webPath` (`/claude`), `publicEmail` (null), `mergeRequestInteraction`.
- `assignees(first..): MergeRequestAssigneeConnection` and `reviewers: MergeRequestReviewerConnection`; both have `count`? (assignees: `count` worked -> `0`), `pageInfo`. Nodes are User-like with `mergeRequestInteraction: UserMergeRequestInteraction` [live]:
```json
{"id":"gid://gitlab/User/54","username":"bob-r12b0c0","mergeRequestInteraction":{"reviewState":"APPROVED","approved":true,"canMerge":false,"canUpdate":true,"reviewed":false}}
```
  `reviewState: MergeRequestReviewState` = `UNREVIEWED | REVIEWED | REQUESTED_CHANGES | APPROVED | UNAPPROVED | REVIEW_STARTED` (nullable; `null` for author/non-reviewers);
  `reviewed` is `reviewState != UNREVIEWED`-ish but observed `false` with `APPROVED` (it reflects the `reviewed` flag only [unverified semantics]); `canMerge` = that user can merge; `canUpdate` = that user can update the MR.
  The interaction is about the **node user**, not the current viewer.
- `participants`, `commenters`, `committers`: user connections; `participants` complexity 15.

### 5.5 Diff/commits
- `diffStats: [DiffStats!]` -> `[{"path":"src/app.py","additions":7,"deletions":0}]`; arg `path` filters.
- `diffStatsSummary` -> `{"additions":7,"deletions":0,"changes":7,"fileCount":1}` (`changes = additions + deletions`).
- `commits(first..): CommitConnection` (no `count`): node `{id:"gid://gitlab/Commit/<sha>", sha, shortId:"851d7388" (8 chars), title, fullTitle, description (null when body empty; "\nCo-authored-by: ..." otherwise - note leading newline), message, authorName, authorEmail, authoredDate:"2026-11-07T06:13:33+00:00", committedDate, webUrl, webPath, author{User}}`. Cursor = array position.

### 5.6 Labels / milestone / emoji / permissions
- `labels(first..)`: `{nodes:[{id:"gid://gitlab/ProjectLabel/13", title, color:"#ff0000", textColor:"#FFFFFF", description:null, descriptionHtml:"", createdAt, updatedAt, lockOnMerge, archived}], count}`; `[]`/`count:0` when none.
- `awardEmoji(first..)`: `[{"name":"thumbsup","emoji":"👍","unicode":"1f44d","description":"thumbs up","unicodeVersion":"6.0","user":{...}}, {"name":"heart","emoji":"❤️","unicode":"2764-fe0f","description":"red heart","unicodeVersion":"6.0"}]` (cursor `{"id":"<award id>"}`, order by id).
- `userPermissions: MergeRequestPermissions!` all `Boolean!`: `adminMergeRequest, canApprove, canMerge, cherryPickOnCurrentMergeRequest, createNote, pushToSourceBranch, readMergeRequest, removeSourceBranch, revertOnCurrentMergeRequest, updateMergeRequest`.
  Sample (owner viewer, open MR): all true except `cherryPickOnCurrentMergeRequest:false, revertOnCurrentMergeRequest:false`. Anonymous on a public MR: `createNote:false, canMerge:false`.

### 5.7 Complexity cost of MergeRequest fields
Default cost 1/field; explicit: `closedAt 5, mergedAt 5, assignees 5, reviewers 5, labels 5, participants 15, subscribed 5, committers 5` [src merge_request_type.rb]. See section 9.

---------------------------------------------------------------------------------------------------

## 6. Notes / Discussions

### 6.1 `MergeRequest.discussions(first,after,last,before): DiscussionConnection!`
Array connection (cursor = 1-based position). Discussions are in chronological order; each system note is its own `IndividualNoteDiscussion`.
```
Discussion { id: DiscussionID! ("gid://gitlab/Discussion/<sha>"), replyId: DiscussionID!, createdAt: Time!,
  resolvable: Boolean!, resolved: Boolean!, resolvedAt: Time, resolvedBy: UserCore,
  noteable: NoteableType, notes(first..max 200): NoteConnection!, userPermissions { resolveNote } }
```
[live] kinds in one MR: IndividualNoteDiscussion (`resolvable:false`, `replyId` class `IndividualNoteDiscussion`), plain thread `Discussion` (`DiscussionNote` notes, resolvable true),
`DiffDiscussion` (DiffNote notes with `position`, resolvable true). Resolved thread: `resolved:true, resolvedBy{username}, notes[].resolvedAt:"2026-11-07T06:13:04Z"`.
There is no `count` on `DiscussionConnection`. `Discussion.userPermissions.resolveNote` false for individual notes.

### 6.2 `Note`
Fields (introspection): `id author{UserCore} authorIsContributor awardEmoji body bodyFirstLineHtml bodyHtml createdAt discussion{Discussion} externalAuthor imported internal
lastEditedAt lastEditedBy maxAccessLevelOfAuthor noteableId noteableType position{DiffPosition} project resolvable resolved resolvedAt resolvedBy system systemNoteIconName
systemNoteMetadata{id action descriptionVersion} updatedAt url userPermissions{adminNote awardEmoji createNote readNote repositionNote resolveNote}`.
Live user note (note 130) key values:
```json
{"id":"gid://gitlab/Note/130","body":"Looks good overall, @root?",
 "bodyHtml":"<p data-sourcepos=\"1:1-1:26\" dir=\"auto\">Looks good overall, <a href=\"/root\" title=\"Administrator\" class=\"gfm gfm-project_member js-user-link\" data-user=\"1\" data-original=\"@root\" data-container=\"body\" data-placement=\"top\" data-reference-type=\"user\">@root</a>?</p>",
 "bodyFirstLineHtml":"<p>Looks good overall, <a href=\"/root\" ... data-user=\"1\" data-reference-type=\"user\">@root</a>?</p>",
 "system":false,"systemNoteIconName":null,"internal":false,
 "url":"http://localhost:8929/rev-r12b0c0/app/-/merge_requests/1#note_130",
 "createdAt":"2026-11-07T06:12:44Z","updatedAt":"2026-11-07T06:12:44Z",
 "lastEditedAt":"2026-11-07T06:12:44Z","lastEditedBy":null,
 "resolvable":false,"resolved":false,"resolvedAt":null,"resolvedBy":null,
 "imported":false,"externalAuthor":null,"authorIsContributor":false,"maxAccessLevelOfAuthor":"Developer",
 "noteableId":60,"noteableType":"MergeRequest","position":null,
 "author":{"id":"gid://gitlab/User/54","username":"bob-r12b0c0"},
 "discussion":{"id":"gid://gitlab/Discussion/367650268bc428163ef914e0271024578cb02bdf"},
 "project":{"id":"gid://gitlab/Project/66"},
 "userPermissions":{"adminNote":true,"awardEmoji":true,"createNote":true,"readNote":true,"repositionNote":true,"resolveNote":true},
 "awardEmoji":{"nodes":[]},"systemNoteMetadata":null}
```
- `lastEditedAt` is **never null**: `Note#last_edited_at = last_edited_at column || updated_at` [src app/models/note.rb:482-486]; `lastEditedBy` is only set when the body was edited.
- `internal` = `confidential?` (Boolean, nullable type but always bool). `noteableId` Int = the MR's table id (60), `noteableType` = `"MergeRequest"`.
- `maxAccessLevelOfAuthor`: human access level string (`"Owner"`, `"Developer"`, ...).
- `system` notes: `systemNoteIconName` (e.g. `"user"` for reviewer-request; from `SystemNoteHelper.system_note_icon_name`), `systemNoteMetadata{id:"gid://gitlab/SystemNoteMetadata/111", action:"reviewer"}`; body e.g. `requested review from @bob-r12b0c0`, `locked the discussion in this merge request`, `approved this merge request`, `resolved all threads`, `added 1 commit\n\n<ul><li>851d73...`.
  Non-system notes: `systemNoteIconName:null`, `systemNoteMetadata:null`.
- System-note authors are the acting user.
- `resolvable`: true for DiscussionNote/DiffNote thread starters and replies; `resolved`/`resolvedAt`/`resolvedBy` per note; false for individual/system notes.
- Note `id` class name: `Note`, `DiffNote`, `DiscussionNote` (see section 3). `Discussion` ids are random sha1 hex.
- Authorization: `Note` requires `read_note`; hidden/internal notes are filtered for non-members.

### 6.3 `DiffPosition`
```
DiffPosition { diffRefs: DiffRefs!, filePath: String!, oldPath: String, newPath: String, positionType: DiffPositionType!,
               oldLine: Int, newLine: Int, x,y,width,height: Int }
```
[live] added line (line 2 of new file):
```json
{"filePath":"src/app.py","oldPath":"src/app.py","newPath":"src/app.py","oldLine":null,"newLine":2,"positionType":"text","x":null,"y":null,"width":null,"height":null,
 "diffRefs":{"baseSha":"6387fc0f9d45cf149f5c82d631ee1f8570c24c20","headSha":"851d7388399fec3d1608b8cbca0544c727c41fc5","startSha":"6387fc0f9d45cf149f5c82d631ee1f8570c24c20"}}
```
- `positionType` enum values are **lowercase** (`text`, `image`; also `file`). `oldLine/newLine` only for text, `x/y/width/height` only for image positions (else null) [src types/notes/diff_position_type.rb:306-328].
- `Note.position` is `null` for non-diff notes (`position.is_a?(Gitlab::Diff::Position)` check) [src note_type.rb:99-101].
- Position `diffRefs` are those at comment time (may differ from MR `diffRefs` after new pushes).
- Input `DiffPositionInput { baseSha: String, headSha: String!, startSha: String!, paths: DiffPathsInput!{oldPath,newPath}, newLine: Int, oldLine: Int }`;
  `position_type` forced to `text` by `createDiffNote`.

---------------------------------------------------------------------------------------------------

## 7. Mutations in scope (source only - none executed)

Common rules (all [src]):
- Input is a single `input:` argument of type `<Name>Input!`; payload `<Name>Payload` with `clientMutationId: String` echoed back, `errors: [String!]!`.
- Class `Mutations::BaseMutation` [app/graphql/mutations/base_mutation.rb]: `authorized?` requires `execute_graphql_mutation` + token scope `api`; not found/unauthorized -> `ResourceNotAvailable` (2.8) with `data.<name>=null`.
- MR mutations extending `Mutations::MergeRequests::Base` take `projectPath: ID!` (full path) and `iid: String!`; MR is found with `MergeRequestsFinder(current_user, iids:[iid])` in the project; ability required: `update_merge_request` (unless noted) [app/graphql/mutations/merge_requests/base.rb:8-27, concerns/resolves_issuable.rb].
- `errors` strings: `record.errors.full_messages` (attribute-humanized messages, e.g. `"Title can't be blank"`).

### 7.0 Mutation field list present in 18.11 (relevant subset)
`mergeRequestCreate, mergeRequestUpdate, mergeRequestAccept, mergeRequestSetDraft, mergeRequestSetAssignees, mergeRequestSetReviewers, mergeRequestSetLabels, mergeRequestSetMilestone,
mergeRequestSetLocked, mergeRequestSetSubscription, mergeRequestRequestChanges, mergeRequestReviewerRereview, createNote, createDiffNote, createImageDiffNote, createDiscussion, createLatestDiffNote (deprecated), updateNote, updateImageDiffNote, destroyNote,
discussionToggleResolve, noteConvertToThread, repositionImageDiffNote, awardEmojiAdd, awardEmojiRemove, awardEmojiToggle`.
**Absent**: `mergeRequestApprove`, `mergeRequestUnapprove`, `mergeRequestRebase`, `mergeRequestClose`, `mergeRequestMerge`.

### 7.1 mergeRequestCreate
Input: `projectPath: ID!, title: String!, sourceBranch: String!, targetBranch: String!, description: String, labels: [String!], mergeAfter: Time, removeSourceBranch: Boolean` [src merge_requests/create.rb].
Payload: `mergeRequest: MergeRequest` (null if invalid), `errors`.
Auth: ability `create_merge_request_from` on `Project.find_by_full_path(projectPath)`; project not found/forbidden -> top-level ResourceNotAvailable.
Service: `::MergeRequests::CreateService.new(project:, current_user:, params:).execute`; params = args + `author_id: current_user.id`; `removeSourceBranch` -> `force_remove_source_branch`; nil args compacted. `labels` are label **titles**.
Result: `{mergeRequest: mr.valid? ? mr : nil, errors: mr.errors.full_messages}`. Validation messages from the model [src app/models/merge_request.rb:1354-1426]:
`You can't use same project/branch for source and target` (attr branch_conflict -> full message e.g. `Branch conflict You can't use same project/branch for source and target`, i.e. `errors.add :branch_conflict, msg` humanizes the attribute name - [unverified exact text]),
`Source branch does not exist`, `Target branch does not exist`, `Another open merge request already exists for this source branch: !N`, `Target project has disabled merge requests`, `Source project is not a fork of the target project`, `Title can't be blank`, `Source branch can't be blank`.
Note: after create the MR is "preparing"; `detailedMergeStatus` may read `PREPARING`/`CHECKING` until async prep finishes.

### 7.2 mergeRequestUpdate
Input: `projectPath, iid, title, targetBranch, description, state: MergeRequestNewState (OPEN|CLOSED -> state_event reopen|close), timeEstimate: String, mergeAfter: Time, removeSourceBranch: Boolean` [src update.rb].
Service: `::MergeRequests::UpdateService.new(project:, current_user:, params: args).execute(mr)`. `timeEstimate` parsed by `Gitlab::TimeTrackingFormatter.parse(.., keep_zero: true)`; null -> 0;
invalid -> top-level ArgumentError `timeEstimate must be formatted correctly, for example `1h 30m``; negative -> `timeEstimate must be greater than or equal to zero. Remember that every new timeEstimate overwrites the previous value.`
Payload `mergeRequest: merge_request.reset`, `errors: full_messages` (state not changed on validation failure). Ability `update_merge_request`.

### 7.3 mergeRequestAccept
Input: `projectPath, iid, sha: String! (required - expected diff head sha), strategy: MergeStrategyEnum (`MERGE_WHEN_CHECKS_PASS` only), commitMessage, squashCommitMessage, shouldRemoveSourceBranch: Boolean, squash: Boolean = false`.
Ability `accept_merge_request`. Payload `{mergeRequest, errors}` (mergeRequest always returned, not nulled).
Flow [src accept.rb:48-102]: build `MergeRequests::MergeService`; `validate` order -> payload errors (no throw), first match wins:
1. `auto_merge_enabled?` -> `The merge request is already scheduled to be merged`
2. not mergeable (`merge_request.mergeable?`, or `auto_merge_eligible?(strategy:)` when strategy given) -> `This branch cannot be merged`
3. `sha != merge_request.diff_head_sha` -> `The merge-head is not at the anticipated SHA`
Then `merge_request.update(merge_error: nil, squash: squash)`; with `strategy` -> `AutoMergeService#execute(mr, strategy)`; else `merge_request.merge_async(user_id, params)`
(**asynchronous since 16.5: returned `mergeRequest.state` is still `opened`**; Sidekiq performs the merge). `result == :failed` -> `The merge failed`.
`MergeRequests::MergeBaseService::MergeError` is rescued -> `errors:[e.message]`.

### 7.4 mergeRequestSetDraft
Input `projectPath, iid, draft: Boolean!`. Service `UpdateService` with `{wip_event: draft ? 'draft' : 'ready'}` (title gets/loses `Draft: ` prefix). Payload `{mergeRequest, errors}`.

### 7.5 mergeRequestSetAssignees
Input `projectPath, iid, assigneeUsernames: [String!]!, operationMode: MutationOperationMode = REPLACE` (`REPLACE | APPEND | REMOVE`) [src concerns/assignable.rb].
Users resolved with `UsersFinder(current_user, username: usernames)` (unknown usernames silently ignored). New id list: REPLACE = given; APPEND = current ∪ given; REMOVE = current − given.
Service `::MergeRequests::UpdateAssigneesService` `params:{assignee_ids: [...]}`. Payload `{mergeRequest, errors}`. Ability `update_merge_request`. Creates system notes ("assigned to @x" / "unassigned @x").

### 7.6 mergeRequestSetReviewers
Input `projectPath, iid, reviewerUsernames: [String!]!, operationMode (default REPLACE)`. Same list math with `resource.reviewers`. Service `::MergeRequests::UpdateReviewersService` `params:{reviewer_ids}`.
Payload `{mergeRequest, errors}` (errors may include `Reviewers ...` max-size message). System note `requested review from @x`; reviewer rows start `UNREVIEWED`.

### 7.7 mergeRequestSetLabels
Input `projectPath, iid, labelIds: [LabelID!]! (gid://gitlab/ProjectLabel/<id> or GroupLabel), operationMode: MutationOperationMode` (no schema default; absent == REPLACE).
`UpdateService` params `label_ids` (REPLACE) / `add_label_ids` (APPEND) / `remove_label_ids` (REMOVE); ids are `gid.model_id` integers. Payload `{mergeRequest, errors}`.

### 7.8 mergeRequestSetMilestone
Input `projectPath, iid, milestoneId: MilestoneID` (optional; `null` clears). Argument uses `loads: MilestoneType` - a nonexistent milestone yields a top-level load error [unverified message: ``No object found for `milestoneId: "..."` ``]. `UpdateService params:{milestone_id: milestone&.id}`.

### 7.9 mergeRequestSetLocked
Input `projectPath, iid, locked: Boolean!`. `UpdateService params:{discussion_locked: locked}`. System note `locked/unlocked the discussion in this merge request`.

### 7.10 mergeRequestSetSubscription
Input `projectPath, iid, subscribedState: Boolean!`. Ability `update_subscription` (any reader). Calls `merge_request.set_subscription(current_user, state, project)`. Payload `{mergeRequest, errors}`.

### 7.11 mergeRequestRequestChanges
Input `projectPath, iid`. Service `MergeRequests::UpdateReviewerStateService#execute(mr, 'requested_changes')`; errors `result[:status]==:success ? [] : [result[:message]]`. Ability `update_merge_request`. (reviewer state `REQUESTED_CHANGES`.)

### 7.12 mergeRequestReviewerRereview
Input `projectPath, iid, userId: UserID!` (loaded user). Service `MergeRequests::RequestReviewService#execute(mr, user)`; `errors: Array(result[:message])`. Resets reviewer state to unreviewed and re-requests review.

### 7.13 createNote
Input `noteableId: NoteableID!, body: String!, internal: Boolean, discussionId: DiscussionID, mergeRequestDiffHeadSha: String`.
Payload `{note: Note, errors, quickActionsStatus: QuickActionsStatus}` (`note` null when not persisted, e.g. quick-action-only body; `quickActionsStatus{commandNames,commandsOnly,errorMessages,messages}`).
Ability `create_note` on the noteable. Service `::Notes::CreateService.new(noteable.project, current_user, {noteable:, note: body, internal:, [in_reply_to_discussion_id:, merge_request_diff_head_sha:], scope_validator}).execute`.
Reply: `discussionId` parsed with expected type Discussion -> sha; discussion looked up via `noteable.notes.find_discussion(sha)`; unreadable/missing -> top-level `The discussion does not exist or you don't have permission to perform this action`.
Replying to an individual-note discussion converts it to a thread when allowed (`update_discussions`). `data.errors = note.errors.full_messages` (e.g. `Note can't be blank`; spam/`Your comment could not be created ...`).
Rate limit `notes_create` -> top-level error (2.8).

### 7.14 createDiffNote
Input `noteableId, body, internal, position: DiffPositionInput!`. Pre-check: at least one of `position.oldLine/newLine` else top-level ArgumentError `position oldLine or newLine arguments are required`.
Builds `Gitlab::Diff::Position(position_type:'text', paths merged)`, `type:'DiffNote'`, `merge_request_diff_head_sha: position.headSha`. Same service/payload as createNote. Returned note id is `gid://gitlab/DiffNote/<id>`.
Invalid positions yield payload errors from DiffNote validation (e.g. `Position is invalid`-style) [unverified text].

### 7.15 createDiscussion
Input `noteableId, body, internal`. Same as createNote with `type:'DiscussionNote'` (a resolvable thread on the noteable; for MR start of a general thread). Returned note id `gid://gitlab/DiscussionNote/<id>`; `note.discussion.id` gives the thread id.

### 7.16 updateNote
Input `id: NoteID!, body: String`. Ability `admin_note` (author or maintainer). Service `Notes::UpdateService(project, user, {note: body}).execute(note)`.
Payload `note` = `updated.reset` (null if the update destroyed the note because the body was only quick actions), `errors`, `quickActionsStatus`. Sets `lastEditedAt`/`lastEditedBy`.

### 7.17 destroyNote
Input `id: NoteID!`. Ability `admin_note`. Service `Notes::DestroyService(project,user).execute(note)`. Payload `{errors: []}` (note field omitted -> `note: null`).

### 7.18 discussionToggleResolve
Input `id: DiscussionID!, resolve: Boolean!`. Ability check is `discussion.can_resolve?(current_user)` (else ResourceNotAvailable). Services `Discussions::ResolveService` / `Discussions::UnresolveService`.
Payload `{discussion: Discussion, errors}`; `ActiveRecord::RecordNotSaved` -> `Discussion failed to be resolved` / `Discussion failed to be unresolved`.

### 7.19 noteConvertToThread
Input `id: NoteID!` (loaded). Ability `resolve_note`. Non-convertible -> top-level ArgumentError `Note cannot be converted to a resolvable thread`. Payload `{note: first_note, errors}` (note id class becomes `DiscussionNote`).

### 7.20 awardEmojiAdd / awardEmojiRemove / awardEmojiToggle
Input `awardableId: AwardableID!, name: String!`. Ability `award_emoji` on the awardable; non-awardable or unreadable MR -> top-level `You cannot add emoji reactions to this resource.`
Name normalised via `TanukiEmoji.find_by_alpha_code(name)&.name || name` (`:thumbsup:` ok).
- Add: `AwardEmojis::AddService` -> errors from service (`User cannot add emoji reactions to awardable`, `Awardable cannot add emoji reactions`, validation e.g. `Name has already been taken`), payload `{awardEmoji, errors}`.
- Remove: `AwardEmojis::DestroyService`; payload `awardEmoji` always null; error `User has not awarded emoji of type <name> on the awardable`.
- Toggle: `ToggleService` (destroy if user already awarded else add); payload adds `toggledOn: Boolean!`; `awardEmoji` only when toggled on.

### 7.21 Reusing REST services
Everything above funnels into the same services REST uses (`MergeRequests::CreateService/UpdateService/MergeService`, `Notes::CreateService/UpdateService/DestroyService`,
`Discussions::ResolveService/UnresolveService`, `AwardEmojis::*`, `UpdateAssigneesService`, `UpdateReviewersService`, `UpdateReviewerStateService`), so a simulator REST service layer can be called directly; the GraphQL layer adds only
argument translation (section 7.x), `full_messages` error shaping, and permission checks listed above.

---------------------------------------------------------------------------------------------------

## 8. Introspection

- Introspection **is allowed** (anonymous too). [live]
- Mechanism: the controller treats a request as an introspection query if `operationName == "IntrospectionQuery"` or
  (no `operationName` key and (`query` contains `__schema` or (contains `__type` and not `__typename`))) [src graphql_controller.rb:425-432].
  In production it then returns a pre-generated static file (`public/-/graphql/introspection_result.json`, or `introspection_result_no_deprecated.json` when `remove_deprecated=true`)
  [src :400-415]. labrat does not reproduce this: it answers introspection from its own CE schema.
- Dynamic introspection is disabled in production (`disable_introspection_entry_points if Rails.env.production?` [src gitlab_schema.rb:39]).
  If the query has an `operationName` other than `IntrospectionQuery`, or includes `__typename` together with `__type`, it runs dynamically and fails:
  `{"errors":[{"message":"Field '__schema' doesn't exist on type 'Query'","locations":[{"line":1,"column":13}],"path":["query Foo","__schema"],"extensions":{"code":"undefinedField","typeName":"Query","fieldName":"__schema"}}]}`
  (same for `__type`). `__typename` itself works: `{"data":{"__typename":"Query"}}`.
- Complexity of the introspection query is 226 (`INTROSPECTION_MAX_COMPLEXITY`).
- Type entries use the standard shape (`kind,name,description,fields[{name,description,args[{name,description,type,defaultValue,isDeprecated,deprecationReason}],type,isDeprecated,deprecationReason}],inputFields,interfaces,enumValues,possibleTypes`).
  Deprecation reasons for experiments look like `**Status**: Experiment. Introduced in GitLab 16.5.`
- Query used (sent with `"operationName":"IntrospectionQuery"`):
```graphql
query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types { ...FullType }
    directives {
      name
      description
      locations
      args(includeDeprecated: true) { ...InputValue }
    }
  }
}
fragment FullType on __Type {
  kind
  name
  description
  fields(includeDeprecated: true) {
    name
    description
    args(includeDeprecated: true) { ...InputValue }
    type { ...TypeRef }
    isDeprecated
    deprecationReason
  }
  inputFields(includeDeprecated: true) { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes { ...TypeRef }
}
fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
  isDeprecated
  deprecationReason
}
fragment TypeRef on __Type {
  kind
  name
  ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } } } } }
}
```
  (Request: `curl -X POST http://localhost:8929/api/graphql -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{"query":"<above>","operationName":"IntrospectionQuery"}'`.)
- labrat implication: tools (GraphiQL, codegen, `gh`-like clients) only need the schema served verbatim; simplest is to embed `graphql-introspection.json` (the CE schema) and answer any request matching the rule above with it.

---------------------------------------------------------------------------------------------------

## 9. Complexity and depth limits

[src app/graphql/gitlab_schema.rb:4-15, 190-217]
| Viewer | max complexity | max depth |
|---|---|---|
| anonymous | **200** | **15** |
| authenticated non-admin | **250** | **20** |
| authenticated admin (the `claude` test user is admin) | **300** | **20** |
| introspection (anonymous) | 226 | n/a |
[live confirmed: anon 101x`currentUser{id}` (=202) -> `exceeds max complexity of 200`; admin 151x (=302) -> `of 300`; depth anon 16 -> `of 15`, admin 21 -> `of 20`.]
Multiplex: limits applied per query in the array [src :88-103].

Error text (HTTP 200, only `message`): `Query has complexity of 302, which exceeds max complexity of 300` / `Query has depth of 21, which exceeds max depth of 20`.
Depth counts every field level including the leaf: `group{parent x19 {id}}` = depth 21. Also the recursion rule (2.9).
Other limits: query text 10,000 chars (`Query too large`, 422); at most 5 validation errors; execution timeout `gitlab.graphql_timeout` default 30 s, validation timeout 0.2 s [src gitlab_schema.rb:83-95, config/initializers/1_settings.rb:232]
(timeout error text: [unverified] graphql-ruby's `Timeout on <Type.field>`).

### 9.1 Complexity formula (legacy calculation mode) [src app/graphql/types/base_field.rb:11,121-186, resolvers/base_resolver.rb:115-148]
- Plain field: `1 + child complexity` (+1 if `calls_gitaly`; explicit `complexity:` overrides).
- Resolver-backed field (connections): `c = child + resolver_complexity`, where `resolver_complexity = 1 + (sort arg present ? 1 : 0) + (search arg ? 5 : 0)`
  (note: arguments with defaults count as present, so `sort` always adds 1 on MR lists); `c += 1` if gitaly; then if it is a connection:
  `c += c * (limit * multiplier)` where `limit = min(first,last,max_page_size 100)` and `multiplier = 0.01` (iid-array filters: 0.05 / 0.5 / 5.0 by size; single `iid`: 0). Result truncated to int.
- Observed totals [live, anonymous, per aliased copy incl. the `project` parent=1]:
  | selection | cost |
  |---|---|
  | `currentUser{id}` / `project(..){id}` / `group(..){id}` | 2 |
  | `project{mergeRequest(iid:"1"){iid}}` | 3 |
  | `project{mergeRequest{iid reviewers{nodes{id}}}}` | 10 (reviewers 5 + nodes 1 + id 1) |
  | `project{mergeRequests(first:100){nodes{iid}}}` (also default size) | 9 |
  | `project{mergeRequests(first:10){nodes{iid}}}` | 5 |
  | `project{mergeRequests(first:100, search:"a"){nodes{iid}}}` | 19 |
  | `mergeRequest{notes(first:100){nodes{id}}}` | 8 |
  | `mergeRequest{discussions(first:100){nodes{id notes{nodes{id}}}}}` | 8 |
  | `mergeRequest{mergedAt closedAt diffStats{path}}` | 15 |

---------------------------------------------------------------------------------------------------

## 10. Misc facts useful for implementation

- `metadata { version revision enterprise }` (`enterprise` is `false`); **null for anonymous**.
- `currentUser` type is `CurrentUser` (extends UserCore) -> `{id, username}`.
- `MergeRequestState` filter enum includes `all` (only for args). `Project.mergeRequests(state:, iids:, sourceBranches:, targetBranches:, authorUsername:, assigneeUsernames:, reviewerUsername:, approvedBy:, labelName:, milestoneTitle:, createdAfter/Before, updatedAfter/Before, mergedAfter/Before, closedAfter/Before, draft:, search:, sort:, not:{...}, or:{...})`;
  `Time` args parse with Ruby `Time.parse` (`"2026-01-01"` and RFC3339 both OK). Empty array arg (`assigneeUsernames: []`) -> empty result (`some_argument_is_empty?`). Mutually exclusive:
  `[assigneeUsernames, assigneeUsername, assigneeWildcardId]`, `[reviewerUsername, reviewerWildcardId]`, `[milestoneTitle, milestoneWildcardId]` -> `Only one of [...] arguments is allowed at the same time.`
- `Project`: `id name path fullPath webUrl description visibility("private"|"public"|"internal" lowercase) archived createdAt lastActivityAt group namespace repository{rootRef} labels milestones projectMembers`.
  `projectMembers` node: `{id:"gid://gitlab/ProjectMember/90", user{..}, accessLevel{integerValue:20, stringValue:"REPORTER"}}`.
- `User.state` enum lowercase: `active`.
- Lowercase vs uppercase enums cheat-sheet: lowercase = `MergeRequestState`, `DiffPositionType`, `User.state`, `Project.visibility` (plain string);
  UPPERCASE = `MergeStatus`, `DetailedMergeStatus`, `MergeRequestReviewState`, `MergeRequestSort`, `MergeabilityCheck*`, `MutationOperationMode`, `MergeRequestNewState`, `NotesFilterType`, `MergeStrategyEnum`, `AccessLevel stringValue`.
- Unauthorized/unknown field access never leaks existence: `project(fullPath:)` returns `null` identically for missing and forbidden.
