# Maintaining labrat

This guide covers three jobs:

1. Day-to-day work: building, running the conformance suite and reading its failures.
2. Recording fixtures: setting up a reference GitLab instance, recording new or changed
   scenarios, and writing new scenarios.
3. Moving labrat to a newer GitLab version, and what supporting several versions at once would
   take.

The commands are written for Linux with Docker. Run them from the repository root unless a step
says otherwise.

## How the pieces fit

- **Reference instance:** a GitLab Community Edition instance running in Docker at
  `http://localhost:8929` (SSH on port 2424). It is the single source of truth for how labrat
  behaves. When GitLab's documentation and the instance disagree, the instance wins; record such
  cases in [discrepancies.md](discrepancies.md).
- **Scenarios** (`tests/conformance/tests/scenarios/*.rs`): sequences of API calls, GraphQL queries,
  git pushes and webhook deliveries.
- **Fixtures** (`tests/conformance/fixtures/<scenario>.json`): one recording per scenario, made by
  running the scenario against the reference instance. The harness normalizes ids, times, tokens,
  SHAs, the base URL and the per-run name suffix into placeholders such as `<project:1>`, `<TIME>`
  and `<RUN>`.
- **Check mode** (the default for `cargo test`): each scenario starts an in-process labrat, runs the
  same steps and compares every response and webhook delivery with the fixture: status, headers,
  body and key order. labrat's clock is moved forward to match the times GitLab answered each step,
  so time-dependent behaviour replays the same way.
- **Record mode** (`LABRAT_RECORD=1`): the scenarios run against the reference instance and
  overwrite their fixtures.
- **Version-specific data** taken from one GitLab version:
  - `GITLAB_VERSION` and `GITLAB_REVISION` in `crates/labrat-core/src/lib.rs`;
  - the GraphQL schema, `crates/labrat-api/data/graphql-introspection.json.gz`;
  - the emoji tables, `crates/labrat-core/src/data/emoji.txt` and `emoji-info.tsv`;
  - the CI favicon digests in `crates/labrat-api/src/routes/pipelines.rs` (`favicon_digest`);
  - the default README template, `crates/labrat-api/src/templates/readme_default.md`.
- **Scripts** (`scripts/reference/`):
  - `gitlab.sh` starts and stops the instance;
  - `setup.sh` prepares a fresh instance;
  - `update-data.sh` copies the version-specific data and the Rails source out of it;
  - `dump-graphql-schema.rb` and `dump-emoji.rb` are run by `update-data.sh` inside the
    container.

The current fixtures were recorded against GitLab 18.11.14 (`gitlab/gitlab-ce:18.11.14-ce.0`).

## Prerequisites

- **Rust:** a stable toolchain from [rustup](https://rustup.rs). If `cargo` isn't on your `PATH`
  after installing, run `. ~/.cargo/env`.
- **Tools:** `git`, `curl` and `jq`.
- **Docker:** needed only for recording. Your user must be able to run `docker`. If you were just
  added to the `docker` group and haven't logged in again, wrap each command in
  `sg docker -c "..."`.
- **Docker bridge:** webhooks during recording go from the GitLab container to a receiver on the host
  at `172.17.0.1`, Docker's default bridge address. If your bridge uses another address (check with
  `ip -4 addr show docker0`), change it in `Scenario::receiver` in `tests/conformance/src/lib.rs`.
- **Resources:** GitLab needs about 4 GB of free memory and 10 GB of disk for the image and data.

## Day-to-day work

```sh
cargo build                     # build everything
cargo test                      # all tests: unit, client and the conformance scenarios (about 3 minutes)
cargo test -p labrat-conformance --test conformance merge_status    # scenarios whose name contains merge_status
cargo test -p labrat-conformance --test conformance scenarios::basics::users_and_tokens -- --exact
```

Running the binary locally:

```sh
cargo run -p labrat -- --db :memory: --root-token glpat-test-token
```

When a scenario fails, the test output lists each difference with its step number, step label and
JSON path, for example:

```
step 41 `14 admin mr2 op15 hooks`: .body[0].body.changes.updated_at: missing (expected {...})
```

labrat's full normalized output is written to `tests/conformance/target-actual/<scenario>.json`
(ignored by git). Compare it with `tests/conformance/fixtures/<scenario>.json` to see the whole
step.

Fixing a difference:

1. Find what GitLab does in its source (see [Reading GitLab's source](#reading-gitlabs-source)) and
   what the fixture shows.
2. Change labrat to match. Comments in labrat name the GitLab class or method they follow, such as
   ``// `MergeRequests::RefreshService#abort_auto_merges`: ...``. Keep doing that.
3. Run the failing scenario, then `cargo test`, and commit only when everything passes.
4. Add an entry to [LOG.md](LOG.md) (newest first) after each significant step.

## Setting up a reference instance

Do this once per GitLab version. Each version gets its own directory and container, so an older
instance stays available. Only one instance can run at a time because they all use ports 8929 and
2424.

1. Pick the image tag. Use an exact patch release from
   [Docker Hub](https://hub.docker.com/r/gitlab/gitlab-ce/tags), for example `18.11.14-ce.0`.

2. Set the variables for this instance. Use them in every later step:

   ```sh
   export GITLAB_IMAGE_TAG=18.11.14-ce.0
   export GITLAB_HOME=~/gitlab-reference/18.11.14
   export GITLAB_CONTAINER=gitlab-ce-18.11.14
   ```

3. Stop any other reference instance, for example
   `GITLAB_CONTAINER=<old container> GITLAB_HOME=<old dir> scripts/reference/gitlab.sh stop`.
   You can also run `docker stop -t 300 <old container>`.

4. Start the new one:

   ```sh
   scripts/reference/gitlab.sh start
   ```

   The first start takes 5–10 minutes. `scripts/reference/gitlab.sh logs` shows progress, and
   `docker ps` shows `(healthy)` when it is ready.

5. Save the root password now. GitLab deletes the file 24 hours after the first start:

   ```sh
   scripts/reference/gitlab.sh password
   ```

   Use root, not `claude`, if you need the web UI.

6. Prepare the instance:

   ```sh
   scripts/reference/setup.sh "$GITLAB_HOME/labrat.env"
   ```

   This mirrors what the harness does for labrat:
   - it creates the admin user `claude` ("Claude", `claude@gitlab.unhood.org`) through the API as
     root;
   - it gives `claude` a token named `claude-code` with the scopes `api`, `read_repository`,
     `write_repository` and `admin_mode`, valid for 364 days;
   - it turns on the `rebase_on_merge_automatic` feature flag;
   - it writes `GITLAB_URL`, `GITLAB_TOKEN` and `claude`'s random password to
     `$GITLAB_HOME/labrat.env`. That file holds secrets: keep it out of the repository.

7. Copy the Rails source and the version-specific data:

   ```sh
   scripts/reference/update-data.sh "$GITLAB_HOME/gitlab-src"
   ```

   On the version labrat already targets, this leaves the repository's data files unchanged (the
   schema file may be recompressed; `git checkout` it if its content didn't change). For a new
   version, see [Moving to a newer GitLab version](#moving-to-a-newer-gitlab-version).

Keep the instance in the state the fixtures expect:

- **Don't sign in to the web UI as `claude`.** `GET /user` shows `last_sign_in_at`, and the fixtures
  expect `null`. If it happened, reset it:

  ```sh
  docker exec "$GITLAB_CONTAINER" gitlab-rails runner "User.find_by_username('claude').update_columns(last_sign_in_at: nil, current_sign_in_at: nil, last_sign_in_ip: nil, current_sign_in_ip: nil, sign_in_count: 0)"
  ```

- **Don't change instance settings or feature flags by hand.** Scenarios that need a setting set it
  themselves; for example, `allow_local_requests_from_web_hooks_and_services` is set by the
  webhook scenarios.
- **Leave the data in place.** Leftover groups, projects and users are harmless, because every run
  uses its own name suffix.
- **Renew the token before it expires** (the expiry date is in the env file). Create a new token for
  `claude` with the same name and scopes, in the UI as root or through the API, and update
  `GITLAB_TOKEN`.

The 18.11.14 instance on the original development machine predates these scripts. It lives in
`~/dev/gitlab-run-ce` (container `gitlab-ce`), is started with `~/dev/gitlab-run-ce/run.sh`, and its
credentials are in `~/dev/gitlab-run-ce/claude-gitlab.env`. That `run.sh` is equivalent to
`gitlab.sh`.

## Reading GitLab's source

`update-data.sh` copies `app`, `lib`, `config`, `db` and `Gemfile.lock` from
`/opt/gitlab/embedded/service/gitlab-rails` into the directory you give it. This is the exact code
the instance runs. Search it when you need to know how something works:

```sh
cd "$GITLAB_HOME/gitlab-src"
grep -rn "def abort_auto_merges" app lib
less app/services/merge_requests/refresh_service.rb
```

Useful starting points:
- `lib/api/` (REST endpoints, one file per resource) and `lib/api/entities/` (response shapes);
- `app/graphql/` (types, resolvers, mutations);
- `app/services/merge_requests/` and `app/services/notes/`;
- `app/models/merge_request.rb`;
- `db/structure.sql` (columns and defaults).

Use only this source and the instance as references.

## Recording fixtures

Load the instance's credentials into the environment first:

```sh
set -a; . "$GITLAB_HOME/labrat.env"; set +a
```

Record one scenario:

```sh
LABRAT_RECORD=1 cargo test -p labrat-conformance --test conformance scenarios::commit_comments::commit_comments -- --exact --test-threads=1
```

Then check labrat against the new recording, without `LABRAT_RECORD`:

```sh
cargo test -p labrat-conformance --test conformance scenarios::commit_comments::commit_comments -- --exact
```

Record every scenario (needed after a GitLab upgrade):

```sh
LABRAT_RECORD=1 cargo test -p labrat-conformance --test conformance -- --test-threads=1
```

This takes several hours: most scenarios take under a minute, but each `random_walk*` scenario takes
10–25 minutes.

Rules:

- **Always record with `--test-threads=1`.** Scenarios running in parallel slow GitLab down, and the
  timings they record become unreliable.
- **Don't run anything else heavy on the machine while recording.** GitLab touches an MR at most
  once a minute and runs its background jobs a moment later, so a slow instance records different
  answers.
- **Record a scenario again whenever you change its steps.** A changed scenario without a new
  fixture fails in check mode. The fixture is written only when the scenario reaches `s.finish()`.
- **Commit the fixture together with the scenario and the labrat change** that makes it pass.
- **Read the fixture diff before committing** (`git diff tests/conformance/fixtures/`). It shows
  what GitLab actually answered.

### Timing noise

Some differences come from timing on GitLab's side, not from labrat. They show up as a value that
changes between two recordings of the same scenario with no code change: typically `updated_at`
appearing or missing in a webhook's `changes`, `subscribed`, or a merge status read while a job was
still running.

To tell timing noise from a real difference:

1. Record the scenario again.
2. If the difference goes away, or a different step differs, it was timing. Keep the new
   recording.
3. If the same difference appears in two recordings in a row, it is real. Find the cause in
   GitLab's source and fix labrat.

Don't change labrat just to match a single recording.

## Writing a new scenario

1. Create `tests/conformance/tests/scenarios/<name>.rs`:

   ```rust
   use labrat_conformance::Scenario;
   use reqwest::Method;
   use serde_json::json;

   const GET: Method = Method::GET;
   const POST: Method = Method::POST;

   #[tokio::test]
   async fn my_scenario() {
       let mut s = Scenario::new("my_scenario").await;   // also the fixture's file name
       let admin = s.admin();
       let g = s.name("ms");                              // "ms-<RUN>": unique for each run
       let group = s.step("group", POST, "/groups", Some(&admin), json!({"name": g, "path": g}), "namespace").await;
       let p = s.step("project", POST, "/projects", Some(&admin),
           json!({"name": "app", "namespace_id": group.id(), "initialize_with_readme": true}), "project").await;
       let pid = p.id();
       s.step("list", GET, &format!("/projects/{pid}/repository/branches"), Some(&admin), json!(null), "branch").await;
       s.finish();
   }
   ```

2. Add `mod <name>;` to `tests/conformance/tests/scenarios/mod.rs`.

3. Record it, then check it (see [Recording fixtures](#recording-fixtures)). Fix labrat until it
   passes, then run the whole suite.

The harness API (`tests/conformance/src/lib.rs`):

- `s.step(label, method, path, token, body, kind)` sends a REST request to `/api/v4<path>` and
  records it.
  - **`body`:** sent as JSON, or as query parameters for `GET`.
  - **`kind`:** names the type of the top-level object, so ids are normalized under the right
    placeholder. Examples: `"user"`, `"project"`, `"namespace"`, `"mr"`, `"note"`, `"commit"`.
  - **Return value:** the response; use `.id()` and `.get("field")` on it.
- `s.graphql(label, token, query, variables)` sends and records a GraphQL request.
- `s.step_web(...)`, `s.step_upload(...)` and `s.git(...)` / `s.git_remote_lines(...)` cover web
  pages, multipart uploads and the real `git` client.
- `s.settle()` waits for background jobs: Sidekiq on GitLab, the job queue in labrat. Call it after
  anything that triggers asynchronous work, such as pushes, MR creation or merges.
- `s.poll_mr(project_id, iid)` waits until an MR is prepared and its merge status has settled.
  `s.poll(path, condition)` is the general form. Polling requests are not recorded.
- `s.receiver()` starts a webhook receiver; use its URL when creating a hook. Then
  `s.deliveries(label, &mut receiver, count)` waits for `count` deliveries and records them as one
  step.
- `s.name(base)` makes a name unique to the run. Use it for every group, project and user you
  create.
- `s.finish()` writes the fixture (record mode) or reports the differences (check mode). It must be
  the last call.

Tips:
- Use the user `claude` (`s.admin()`) for setup, and create other users with `POST /users` plus a
  token when a scenario needs a non-admin.
- Write scenarios that check one behaviour each. For interactions, the `random_walk*` scenarios run
  a fixed-seed sequence of operations; copy one and change the seed or the operations.

## Moving to a newer GitLab version

labrat targets one GitLab version at a time. Moving to a newer one means:
- recording everything again on a fresh instance of that version;
- updating the version-specific data;
- fixing labrat until every scenario passes.

Do it on a branch, and plan for several days on a minor or major version.

1. **Choose the version.** Pick the newest patch release of the target minor version, for example
   `19.2.3-ce.0`. Read GitLab's release posts and the `CHANGELOG.md` entries for the versions in
   between. Look for changes to merge requests, notes, approvals, webhooks, the REST API and
   GraphQL. Note any feature flags that changed their default.

2. **Set up a new instance** for that version in its own directory, following
   [Setting up a reference instance](#setting-up-a-reference-instance). Don't upgrade the existing
   instance in place: in-place upgrades must pass through GitLab's required upgrade stops, and
   GitLab can't be downgraded. Keep the old instance stopped, in case you need to compare.

   Check whether `rebase_on_merge_automatic` still exists. Check also whether other flags that
   labrat's comments mention changed their default (`git grep -n "flag" crates`). Adjust `setup.sh`
   and labrat to match.

3. **Update the version-specific data:**

   ```sh
   git switch -c gitlab-<version>
   scripts/reference/update-data.sh "$GITLAB_HOME/gitlab-src"
   ```

   Then do what it prints:
   - set `GITLAB_VERSION` and `GITLAB_REVISION` in `crates/labrat-core/src/lib.rs` to the printed
     `VERSION` and `REVISION`;
   - update the digests in `favicon_digest` in `crates/labrat-api/src/routes/pipelines.rs` from the
     printed file names (`favicon_<status>-<digest>.png`). The `_` arm uses `status_created`;
   - compare `crates/labrat-api/src/templates/readme_default.md` with
     `app/views/projects/readme_templates/default.md.tt` in the copied source. Carry over text
     changes, keeping labrat's `{{...}}` placeholders where the template has ERB tags.

   Review the changes with `git diff --stat` and `git diff crates/labrat-core/src/data`.

4. **See what changed in GitLab's code.** Use the two source copies, old and new:

   ```sh
   diff -ru ~/gitlab-reference/<old>/gitlab-src/lib/api ~/gitlab-reference/<new>/gitlab-src/lib/api | less
   diff -ru ~/gitlab-reference/<old>/gitlab-src/app/services/merge_requests ~/gitlab-reference/<new>/gitlab-src/app/services/merge_requests | less
   ```

   Do the same for `lib/api/entities`, `app/graphql`, `app/services/notes`,
   `app/models/merge_request.rb` and `db/structure.sql`. Many labrat comments cite GitLab files with
   line numbers; those line numbers drift between versions.

5. **Record every scenario** against the new instance (see
   [Recording fixtures](#recording-fixtures)).

6. **Run the suite and fix labrat:**

   ```sh
   cargo test 2>&1 | tee /tmp/upgrade.log
   grep -E "differences in scenario|step [0-9]+ " /tmp/upgrade.log
   ```

   For each difference:
   - decide whether it is a GitLab change (it shows up in the source diff or the changelog) or
     timing noise (see [Timing noise](#timing-noise));
   - fix GitLab changes in labrat;
   - re-record the scenario when it is timing noise.

   Expect most differences to come from new or renamed fields in responses, changed defaults and
   changed messages. Schema changes are handled by the new introspection file. If a resolver handles
   a field the new schema removed, delete that code. If the new schema adds a field labrat doesn't
   resolve yet, requests for it return an error until it is implemented.

7. **Update the docs.** List the places that name the old version:

   ```sh
   git grep -n "18\.11"
   ```

   Update README.md, AGENTS.md, docs/PLAN.md, this file, and comments that say "recorded on
   18.11". Add a LOG.md entry describing the move.

8. **Commit** when `cargo test` passes completely, and open a pull request.

## Supporting several GitLab versions at once

labrat doesn't support this yet. It has one set of version constants, one schema, one set of
fixtures, and behaviour that follows one version. A workable design, in order:

1. **Fixtures per version.** Move the fixtures to `tests/conformance/fixtures/<version>/` and make
   the harness read the directory from an environment variable, for example
   `LABRAT_GITLAB_VERSION=18.11`, with a default. Record each version into its own directory, from
   its own reference instance.
2. **A version setting in labrat.** Add `gitlab_version` to `labrat_core::Config`, with a
   `--gitlab-version` option and a `LABRAT_GITLAB_VERSION` environment variable, and pass it into
   the harness's `start_labrat`.
3. **Per-version data.** Keep one schema file, one set of emoji tables, favicon digests and README
   template per version. Select them from the configured version instead of using `include_bytes!`
   of a single file.
4. **Version checks in code.** Where behaviour differs, branch on the configured version, for
   example `if config.gitlab_version >= v(19, 0) { ... }`. Keep each branch next to a comment that
   names the GitLab change.
5. **Testing.** Run the conformance suite once per supported version, in CI or by looping over
   `LABRAT_GITLAB_VERSION`.

Keep the number of supported versions small. Each one needs its own reference instance, fixtures
and upkeep.

## Troubleshooting

- **`missing fixture ... record it with LABRAT_RECORD=1`:** the scenario has no recording yet.
  Record it.
- **`GITLAB_URL` or `GITLAB_TOKEN` panics while recording:** the env file isn't loaded in this
  shell. Run `set -a; . "$GITLAB_HOME/labrat.env"; set +a`.
- **401 responses while recording:** the token expired or was revoked. Create a new one (see the
  token renewal step above).
- **Webhook steps record no deliveries** (each waits up to 90 seconds): GitLab can't reach the
  receiver. Check the Docker bridge address (see [Prerequisites](#prerequisites)) and that no firewall blocks the container from
  reaching the host.
- **A webhook scenario fails right after enabling a setting:** GitLab caches application settings
  for about a minute. The scenarios that change settings already wait. Keep that wait in new ones.
- **`merge_request_lifecycle` fails at step `current user` on `last_sign_in_at`:** someone signed in
  to the web UI as `claude` before it was recorded. Reset the sign-in fields (see [Setting up a reference instance](#setting-up-a-reference-instance)).
- **GitLab is slow or restarts:** check memory with `free -g` and the logs with
  `scripts/reference/gitlab.sh logs`. Stop other containers.
- **`setup.sh` says `claude` already exists:** the instance was already set up. Use its existing
  env file, or create a new token for `claude` as described above.
