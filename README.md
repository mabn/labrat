# labrat

**A fast, lightweight GitLab simulator for testing anything that talks to GitLab.**

labrat answers GitLab's REST API, GraphQL API, webhooks and Git over HTTP the way GitLab 18.11
does, from a single 25 MB binary. Point your bot, CLI, code-review tool or sync job at it instead of
a real GitLab, and your tests start in milliseconds instead of minutes.

<p align="center">
  <img src="docs/assets/benchmark.svg" alt="labrat compared with GitLab: 20 merge requests from creation to merge take 0.68 s instead of 189 s; idle memory is 15 MB instead of 3.0 GB; starting from an empty data directory takes 0.01 s instead of 4 minutes." width="760">
</p>

<p align="center">
  <i>labrat compared with GitLab CE 18.11 in Docker, on the same laptop. <a href="docs/BENCHMARKS.md">How this was measured</a>.</i>
</p>

## Features

- **Fast:** a merge request workload runs [100× faster](docs/BENCHMARKS.md) than on GitLab. labrat
  starts in about 10 ms and wipes all its data in 6 ms, so every test can start from a clean state.
- **Small:** one binary and one SQLite file. No Docker, PostgreSQL, Redis or Sidekiq, and about
  15 MB of memory when idle instead of 3 GB.
- **Accurate:** checked against 16,504 responses and 1,428 webhook deliveries recorded from real
  GitLab CE and EE instances. The check compares status codes, headers, bodies and even key order.
- **Built for tests:** endpoints to reset, snapshot and restore, to wait until background work is
  done, and to move the clock. A request log shows every call your code made.
- **Works with real clients:** it is tested with the Rust `gitlab` crate, `curl` and
  `git clone`/`git push`. Other GitLab clients use the same API.
- **Simple web UI:** a GitLab-style web UI to browse projects, review merge requests and change
  settings while you develop or debug.

## Limitations

This is not a GitLab replacement:

- git storage and database is a single sqlite DB
- Single process, no support for scaling
- Test endpoints which bypass all security
- Only functionality related to git hosting and Merge Requests is supported, there is no CI or Issues

## labrat and GitLab at a glance

| | GitLab CE 18.11 (Docker) | labrat |
|---|---|---|
| Download | 1.83 GB image | 10.1 MB binary (gzipped) |
| Size on disk | 6.25 GB | 25.3 MB |
| Install | 4 minutes: the first start sets GitLab up | `brew install`: one binary, nothing to set up |
| Start | 84 s | 11 ms |
| Reset to an empty state | About 5 minutes: recreate the instance | 6 ms |
| Memory when idle | 3.0 GB | 15 MB |
| Peak memory during the workload | 3.5 GB | 28 MB |
| 20 merge requests, created to merged, one after another | 189 s | 0.68 s |
| Processes | 89 | 1 |

Details, the workload and the machine are in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Quick start

Install it with Homebrew (macOS and Linux):

```sh
brew install mabn/tap/labrat
```

Or with the install script (macOS and Linux; it puts `labrat` in `~/.cargo/bin`):

```sh
curl -LsSf https://github.com/mabn/labrat/releases/latest/download/labrat-installer.sh | sh
```

Prebuilt binaries for macOS (Apple Silicon and Intel), Linux (x86_64 and arm64, static) and Windows
are also on the [releases page](https://github.com/mabn/labrat/releases).

Run it with an in-memory database and a known token for `root`:

```sh
labrat --db :memory: --root-token glpat-test-token
```

Or run it in Docker, for example as a service container in CI:

```sh
docker run --rm -p 8080:8080 ghcr.io/mabn/labrat --db :memory: --root-token glpat-test-token
```

labrat listens on `127.0.0.1:8080`. To look around the web UI, click "Generate demo data" in its
sidebar (or `POST /-/labrat/demo`): it populates labrat with example projects.
`scripts/seed-demo.sh` resets labrat and fills it with a smaller demo. Use it as you would use GitLab:

```sh
curl -H "PRIVATE-TOKEN: glpat-test-token" -X POST localhost:8080/api/v4/projects \
  -H 'Content-Type: application/json' -d '{"name": "demo", "initialize_with_readme": true}'
git clone http://root:glpat-test-token@localhost:8080/root/demo.git
```

The web UI is at <http://localhost:8080/>. Sign in as `root` with the root password
(`labrat-root-password` unless set), or with a personal access token.

<p align="center">
  <img src="docs/assets/web-ui-merge-request.png" alt="labrat's web UI showing an open merge request: the description, an approve and merge widget, a review thread with a suggested change, and a sidebar with reviewers and labels." width="760">
</p>

### Using it in tests

A typical test suite starts one labrat and resets it before each test:

```sh
curl -X POST localhost:8080/-/labrat/reset     # empty again: only root and its token
# ... run the code under test against http://localhost:8080 ...
curl localhost:8080/-/labrat/idle              # returns once background jobs and webhooks are done
```

Background work runs as it does in GitLab: a new merge request is `preparing` for a moment and its
merge status is computed afterwards. Call `/-/labrat/idle` before asserting on anything that depends
on it. To check that your code copes with slow background work, start labrat with
`--job-delay-ms`.

## What is simulated

- **Users, groups, projects, members and tokens**, with GitLab's permission model.
- **Git over smart HTTP**: clone, fetch and push, protected branches, push options
  (`git push -o merge_request.create …`) and GitLab's push messages.
- **Repository APIs**: branches, tags, commits (create from actions, cherry-pick, revert), files,
  trees, compare, commit comments and discussions, commit statuses and the external pipelines
  they create.
- **Merge requests**:
  - the full lifecycle, asynchronous preparation and mergeability, and detailed merge status;
  - every merge method with squash and templates, rebase, auto-merge (waiting for a green pipeline
    when one exists) and scheduled merges (`merge_after`);
  - source branch deletion with retargeting, diffs and versions, context commits and description
    templates;
  - labels (with promotion), project and group milestones, time tracking with time logs;
  - subscriptions, to-dos (including snoozing), resource events, contribution events and markdown
    uploads;
  - the list filters and sorts.
- **Reviews**: notes, threads, diff notes with position tracing, suggestions, award emoji, draft
  notes and review submission, approvals, reviewer states and quick actions.
- **Webhooks**: push, tag push, merge request, comment (MR and commit), emoji and pipeline
  events, with GitLab's payloads and headers, branch filters, delivery logs, test deliveries, and
  system hooks (including `repository_update`).
- **GraphQL**: GitLab's 18.11 schema for introspection and validation, with queries and
  mutations for merge requests (including the full `mergeRequests` filter set and
  `mergeabilityChecks`), notes, discussions, labels, emoji and to-dos.
- **Search**: `/search`, `/groups/:id/search` and `/projects/:id/search` for merge requests,
  milestones, notes, commits, users and projects, as basic search answers them.
- **Web UI**: a GitLab-style single-page app over the REST and GraphQL APIs: projects, groups,
  files (with editing and committing), branches, commits, merge requests (activity, threads, diffs
  with inline comments, suggestions, reviews, approvals, merging and rebasing), project and merge
  request settings, members, labels, milestones, webhooks, access tokens, users and the request
  log.

labrat is not a GitLab replacement. It has no CI runners or jobs, no issues, no forks, no durability
guarantees and no high availability. Its web UI looks and works like GitLab's for what labrat
implements, and talks to labrat only through the GitLab APIs. The full scope
is in [docs/PLAN.md](docs/PLAN.md).

## How faithful is it?

labrat is checked against a real GitLab, not against its documentation. `tests/conformance`
holds 154 scenarios: sequences of API calls, GraphQL queries, git pushes and webhook deliveries,
recorded against GitLab CE 18.11. EE mode is checked against 168 scenarios recorded against GitLab
EE 18.11. `cargo test` runs each scenario against labrat and compares every response
with the recording, after replacing ids, times and tokens with placeholders. It compares status,
headers, body and key order.

```sh
cargo test                                                            # check labrat against the recordings
cargo test -p labrat-conformance --test conformance todo_lifecycle    # one scenario
```

Some details that make this work:

- **Time:** recordings store when GitLab answered each step. The check moves labrat's clock to
  match, so time-dependent behaviour replays the same way, such as GitLab touching a merge request
  at most once a minute.
- **Random walks:** the `random_walk*` scenarios run fixed-seed sequences of operations by two users
  on one or two merge requests, and compare every webhook delivery. They find interactions that
  hand-written scenarios miss.
- **Real clients:** `tests/client` drives labrat with the Rust `gitlab` crate and the real `git`
  client.
- **Documentation gaps:** where GitLab's documentation and its behaviour disagree, labrat follows
  the behaviour. [docs/discrepancies.md](docs/discrepancies.md) lists those cases.

[docs/MAINTAINING.md](docs/MAINTAINING.md) explains how to record new scenarios and how to move to
a newer GitLab version.

## Configuration

| Option | Environment | Default | Meaning |
|---|---|---|---|
| `--listen` | `LABRAT_LISTEN` | `127.0.0.1:8080` | Address to listen on |
| `--db` | `LABRAT_DB` | `labrat.sqlite` | SQLite file, or `:memory:` |
| `--external-url` | `LABRAT_EXTERNAL_URL` | `http://<listen>` | Base URL used in links and payloads |
| `--root-password` | `LABRAT_ROOT_PASSWORD` | `labrat-root-password` | Password of `root`, created on an empty database |
| `--root-token` | `LABRAT_ROOT_TOKEN` | none | A personal access token for `root` with this exact value |
| `--ssh-port` | `LABRAT_SSH_PORT` | `22` | Port shown in `ssh_url_to_repo` (git over SSH isn't served) |
| `--job-delay-ms` | `LABRAT_JOB_DELAY_MS` | `0` | Delay every background job, to expose intermediate states |
| `--job-jitter-ms` | `LABRAT_JOB_JITTER_MS` | `0` | Random extra delay per job |
| `--noee` | `LABRAT_NOEE` | off | Behave like GitLab Community Edition instead of Enterprise Edition |

**Editions.** By default labrat answers like GitLab Enterprise Edition 18.11. EE support covers
the features labrat supports on CE, plus merge request approval rules, code owners and approval
policies (as merge requests see them), merge request dependencies and merge trains. Nothing else
from EE is included.

- **CE features:** these answer as EE answers them. That means EE's extra JSON fields, its GraphQL
  schema, and its defaults where they differ (for example, authors can't approve their own merge
  requests, and approvals reset on push).
- **Approval rules:** project and merge request approval rules, approval settings and approval
  state, over REST and GraphQL.
- **Code owners:** the `code_owner` approval rules merge requests get, and how they block or
  allow merging. labrat doesn't read `CODEOWNERS` files: set a branch's code owners through the
  test endpoint below.
- **Approval policies:** the policy approval rules, violations, warn mode dismissals, bypasses
  and the security bot's comment on merge requests. labrat runs no security scans: set the
  policies (GitLab's policy schema, as JSON) through the test endpoints below.
- **Merge request dependencies:** blocking merge requests over REST and GraphQL, and the merge
  status they cause.
- **Merge trains:** the merge train settings, adding and removing merge requests, the train
  listings, and the train's pipelines, merges and restarts, over REST and GraphQL. labrat runs
  no real CI. Pipelines come from labrat's fake CI, which is set up through the test endpoints
  below.
- **Other EE-only features:** their endpoints and fields don't exist, as on an instance without
  the feature. That includes external status checks, epics, and security and compliance
  features.

With `--noee` labrat answers exactly like the Community Edition, and the EE endpoints and fields
don't exist.

The database schema version is checked on start. After an upgrade that changes it, delete the
database file.

### Test endpoints

These need no authentication. labrat is a test tool: only let it listen where your tests can reach
it.

| Endpoint | What it does |
|---|---|
| `POST /-/labrat/reset` | Wipes all data and recreates `root` (and its token) |
| `POST /-/labrat/snapshot?path=…` | Saves the database to a file |
| `POST /-/labrat/restore?path=…` | Restores a saved database |
| `GET /-/labrat/idle` | Returns once every background job and webhook delivery has finished |
| `POST /-/labrat/time` | Shifts labrat's clock |
| `GET /-/labrat/requests.json` | The last 500 API calls with requests and responses (also in the UI at `/-/labrat/requests`) |
| `POST /-/labrat/demo` | Adds a `demo` group of projects whose merge requests each show one feature, and the users they need (also "Generate demo data" in the UI). Returns 409 if the group exists |
| `PUT /-/labrat/projects/:id/ci` | Fake CI for a project (EE merge trains): `{"enabled": true, "jobs": ["test"], "runner": {"mode": "manual"}}`, or `"mode": "auto"` with `"status"` (`success`, `failed` or `canceled`), `"duration_ms"` and per-ref `"overrides"`. `{"enabled": false}` turns it off. `GET` shows it. |
| `GET /-/labrat/projects/:id/ci/jobs` | The project's fake CI jobs |
| `POST /-/labrat/ci/pick` | `{"project_id": 1}`: the fake runner starts the project's pending jobs |
| `POST /-/labrat/ci/finish` | `{"project_id": 1, "ref": "refs/merge-requests/1/train", "status": "failed"}`: finishes the running jobs on that ref (`"include_pending": true` for pending ones too) |
| `PUT /-/labrat/projects/:id/code_owners` | EE code owners of a branch, as `CODEOWNERS` sections: `{"branch": "main", "sections": [{"name": null, "entries": [{"pattern": "*", "owners": ["@alice"]}]}, {"name": "Backend", "approvals": 2, "optional": false, "entries": [{"pattern": "/app/", "owners": ["@bob", "@group/path"]}]}]}`. Open MRs into the branch get their code owner rules at once. `GET` and `DELETE` take `?branch=` (default: the default branch). |
| `PUT /-/labrat/projects/:id/approval_policies` | EE approval policies: `{"approval_policy": [...]}`, a policy file's list in GitLab's schema (as JSON). Replaces the project's policies and evaluates the open MRs. `GET` shows them, `DELETE` removes them. |
| `PUT /-/labrat/projects/:id/merge_requests/:iid/policy_results` | Approval policies' results on one MR, by name (`null`: as labrat evaluates it): `{"policies": {"Licenses": {"violated": false}, "No critical vulns": {"violated": true, "status": "FAILED", "newScanFinding": [{"name": "SQL injection", "path": "app/db.rb", "reportType": "SAST", "severity": "CRITICAL", "location": {}}]}}}`. The lists take GraphQL's `PolicyViolationDetails` shapes: `newScanFinding`, `previousScanFinding`, `licenseScanning`, `errors`, `comparisonPipelines`. |

## Documentation

- [docs/BENCHMARKS.md](docs/BENCHMARKS.md): the comparison with GitLab and how to run it.
- [docs/MAINTAINING.md](docs/MAINTAINING.md): recording fixtures, writing scenarios and moving to a
  newer GitLab version.
- [docs/PLAN.md](docs/PLAN.md): architecture and scope.
- [docs/discrepancies.md](docs/discrepancies.md): where GitLab's documentation and its behaviour
  disagree.
