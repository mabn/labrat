# GitLab 18.11 GraphQL "validationTimeout" on invalid queries

Sources: graphql-ruby 2.6.10 at /home/mabn/dev/gitlab-run/ref-gems/graphql-2.6.10/lib/graphql,
GitLab at /home/mabn/dev/gitlab-run-ce/gitlab-src. No HTTP/ruby was run; this is static analysis.
Confidence: mechanism HIGH; exact cut-off between timeout/normal is a cost model (MEDIUM) and needs the probes below.

## Finding: no rule raises. It is a REAL 0.2 s timeout caused by did_you_mean

1. Conversion to the error: `static_validation/validator.rb:41-52` wraps `context.visitor.visit` in
   `Timeout::timeout(timeout)`; `rescue Timeout::Error` -> `handle_timeout` (l.74-78) appends
   `ValidationTimeoutError("Timeout on validation of query")` (`validation_timeout_error.rb`, code
   "validationTimeout", no locations). Errors already collected in `context.errors` are kept in the
   list, but the visit is aborted. (Only GraphQL::ExecutionError is otherwise rescued, l.62; any other
   exception would be a 500, not this message.) The response only has the timeout error in the
   observed cases, because the abort happened inside the first error's message construction,
   before `add_error` ran (the suggestion string is computed before add_error).
2. Timeout value: `schema.rb:846-856` default is 3 s, but GitLab overrides the getter in
   `app/graphql/gitlab_schema.rb:41-55`: **0.2 s**, or **0.5 s** if feature flag
   `graphql_increased_validate_timeout` (gitlab_com_derisk, instance) is on. Rake tasks set 1 s/5 s
   (lib/tasks/gitlab/graphql*.rake). Also `validate_max_errors 5` (gitlab_schema.rb:33).
3. What is slow: the "did you mean" suggestion, computed only when a "not found" error is added:
   - `rules/fragment_types_exist.rb:26-36`: for unknown fragment type condition (inline or named),
     builds the list of ALL types with fields (objects, interfaces, unions: order of 1-2k+ names in
     GitLab; ~480 *_type.rb files plus generated connection/edge/enum/payload types) and runs
     `DidYouMean::SpellChecker.new(dictionary: names).correct(type_name)` (`validation_context.rb:53-66`).
   - `rules/fields_are_defined_on_type.rb:17-18`: for undefined field, dictionary = all fields of the
     parent type. Cheap for Query (~56 fields) and Subscription; expensive for Mutation (~270) and Project (~170).
   - same pattern in arguments_are_defined.rb:13, directives_are_defined.rb:13,
     variables_are_input_types.rb:10 (small dictionaries; unlikely to hit 0.2 s).
   SpellChecker cost ~ (#dictionary words) x (Jaro-Winkler on name length, plus Levenshtein for
   survivors), so it grows with dictionary size AND input-name length. Slow under a 0.2 s budget
   (Ruby, possibly cold, concurrent load; Timeout also counts parse-independent visitor time).
   `schema.did_you_mean` is DidYouMean when defined (schema.rb:1740-1745); GitLab doesn't disable it
   (grep of app/graphql, lib/gitlab/graphql, config: no did_you_mean override).
4. No rule that dereferences nil: FragmentsAreOnCompositeTypes (nil-checked, l.18-27),
   FragmentTypesExist (l.20-25), FieldsAreDefinedOnType (returns without super on nil field, so
   children of an undefined field are never visited) are all nil-safe. GitLab analyzers (Logger,
   Recursion) are AST analyzers and run after validation succeeds, so are not involved.

## Fit to observations

| query | did_you_mean call | dictionary | result |
|---|---|---|---|
| `...F`, `fragment F on Nope` | FragmentTypesExist, name "Nope" | all types (huge) | timeout |
| `... on Nope` | same | same | timeout |
| `mergeRequestCreatePipeline` on Mutation | FieldsAreDefinedOnType, 26-char name | ~270 mutation fields, long similar-ish names | timeout |
| `nopeMutation` | same, 12-char name | same dictionary | normal (fast; short name, few JW survivors) |
| `nopeQuery` | same | Query fields (small) | normal |
| `__schema` (disabled) | different rule/path, no spell check | - | normal |
| `subscription { nope }` | root missing / tiny dictionary | - | normal |

Caveat: mergeRequestCreatePipeline vs nopeMutation differ in name length and in similarity to
existing names (e.g. mergeRequestCreate*, pipelineCreate, mergeRequestReviewerRereview). The
difference between "timeout" and "normal" for mutation fields is therefore a cost heuristic, not a
hard boundary.  Variables ($p) are irrelevant (they only add variable rules; none raise).

## Genuine timeout on large-but-valid queries?
Possible in principle: 0.2 s budget over the whole visitor (all rules, FieldsWillMerge being
quadratic in same-response-key fields, FragmentsAreFinite, etc.), and GitLab's 10,000-char limit
(`app/controllers/graphql_controller.rb:52` MAX_QUERY_SIZE) bounds the work but does not make it
trivially fast: e.g. thousands of repeated aliases/spreads of the same field, or ~100s of unknown
fields each triggering a spell check (but max_errors 5 stops after 5 errors via
`throw :too_many_validation_errors`, bounding spell checks to <=5-ish). Realistically, in the
test-instance setting, timeouts come from the spell checker, not from query size.

## Rule for a reimplementation (pseudo-code)

```
def validation_outcome(doc, schema):
    # run normal validation in document order (graphql-ruby visitor order, depth-first,
    # fragment definitions visited in document order too), collecting errors, max 5.
    for each error-producing node in visit order:
        if node is fragment definition / inline fragment with unknown type condition T:
            # dictionary = all object/interface/union types (thousands)
            return TIMEOUT_ONLY   # [{"message":"Timeout on validation of query","locations":[],
                                  #   "extensions":{"code":"validationTimeout"}}]
        if node is a field F that does not exist on parent type P (and P is not a union):
            n = |fields(P)| ; L = len(F.name)
            if P == Mutation (and likely Project or any type with >= ~150 fields) and L >= ~20:
                return TIMEOUT_ONLY      # heuristic: cost ~ n * L^2 > 0.2 s
            else:
                emit normal undefinedField error (with did-you-mean suffix if any)
        # other errors are emitted normally
    return errors
```
Notes: TIMEOUT_ONLY drops errors collected earlier is NOT verified; errors produced before the
slow node may still appear alongside the timeout error (context.errors keeps them; only the
timeout error is appended). Whether children of an unknown-type fragment matter: they are not
visited (FragmentTypesExist skips super), so contents of the fragment are irrelevant.
Unknown fragment type => timeout regardless of the rest of the query (on the observed instance).

## Probe queries (expected under this theory)

1. `{ project(fullPath:"a/b") { ...F } } fragment F on Q { id }` -> timeout (1-char type name; still large dictionary) ; if normal, cost depends on name length too.
2. `{ ... on Zzzzzzzzzzzzzzzzzzzzzzzzzz { id } }` -> timeout (inline on root, no variables/project).
3. `query { ... on Nope { id } }` -> timeout (no variables, confirms $p irrelevant).
4. `fragment F on Nope { id } query { __typename }` -> timeout? (unused unknown fragment; FragmentTypesExist still runs) and also fragmentsAreUsed.
5. `mutation { mergeRequestCreatePipeline { errors } }` (no args, no variables) -> timeout (long name only).
6. `mutation { a }` / `mutation { nope }` -> normal (short names).
7. `mutation { zzzzzzzzzzzzzzzzzzzzzzzzzz }` (26 chars, dissimilar) -> if timeout: length drives it; if normal: similarity drives it.
8. `mutation { mergeRequestCreatePipelin }`... and `mutation { mergeRequestCreatex }` -> bracket the length/similarity threshold.
9. `mutation { nopeMutation { errors } }` and `mutation { nopeMutation(input:{a:1}) { errors } }` -> normal (observed analog), shows args/sub-selection irrelevant.
10. `{ project(fullPath:"a/b") { nopeFieldWithAVeryLongNameIndeed } }` -> probably timeout (Project has ~170 fields); `{ project(fullPath:"a/b") { x } }` -> normal.
11. `{ currentUser { zzzzzzzzzzzzzzzzzzzzzzzzzz } }` and `{ zzzzzzzzzzzzzzzzzzzzzzzzzz }` -> normal for Query (small dictionary), contrast with 7.
12. Repeat probe 5 several times and under load; flaky results (sometimes normal) confirm a real wall-clock timeout (0.2 s) rather than an exception. Also compare after toggling `graphql_increased_validate_timeout` (0.5 s) if possible.
13. `{ project(fullPath:"a/b") { ... on Nope { x } } }` plus 6 distinct unknown short root fields -> shows whether earlier errors survive alongside the timeout error.
14. `query ($v: Nope!) { __typename }` -> variablesAreInputTypes (dictionary: input types, medium); timeout if input-type dictionary is slow too.
