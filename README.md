# labrat

labrat is a GitLab simulator for testing API integrations. It is a single Rust binary that serves
GitLab's REST API (v4), GraphQL API, webhooks and Git over HTTP. It keeps everything in one SQLite
file: users, projects, merge requests and the git objects themselves.

It is for testing anything that talks to GitLab, such as bots, CLIs, code-review tools and sync jobs.
It starts in moments, can be wiped or snapshotted between tests, and answers API calls the way
GitLab 18.11 does. Merge requests are the focus. That means the whole lifecycle and everything people
do in one: comments, threads, diff comments, reviews, approvals, reactions, suggestions and merging.

It is not a GitLab replacement. It has no CI runners, no issues, no durability, no upgrade path and
no high availability. Its web UI looks and works like GitLab's for what labrat implements, and talks
to labrat only through the GitLab APIs.

**Status:** the merge request and review feature set is implemented and checked against a real
GitLab 18.11 instance (see [Fidelity](#fidelity)). [docs/PLAN.md](docs/PLAN.md) covers the
architecture and scope; [docs/LOG.md](docs/LOG.md) is the work log.
[docs/MAINTAINING.md](docs/MAINTAINING.md) explains how to record fixtures, write scenarios and move
to a newer GitLab version.

## Quick start

```sh
npm --prefix web ci && npm --prefix web run build   # the web UI; optional, needs Node.js 20.19+
cargo build --release
./target/release/labrat --db :memory: --root-token glpat-test-token
```

labrat listens on `127.0.0.1:8080`. Use it as you would use GitLab:

```sh
curl -H "PRIVATE-TOKEN: glpat-test-token" -X POST localhost:8080/api/v4/projects \
  -H 'Content-Type: application/json' -d '{"name": "demo", "initialize_with_readme": true}'
git clone http://root:glpat-test-token@localhost:8080/root/demo.git
```

The web UI is at <http://localhost:8080/>. Sign in as `root` with the root password
(`labrat-root-password` unless set), or with a personal access token. The UI is a React app in
[`web/`](web/README.md) that `cargo build` embeds from `web/dist`; a binary built without it serves
a page that says how to build it.

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

The database schema version is checked on start. After an upgrade that changes it, delete the
database file.

## Test endpoints

These need no authentication. labrat is a test tool and should only listen where tests can reach
it.

| Endpoint | What it does |
|---|---|
| `POST /-/labrat/reset` | Wipes all data and recreates `root` (and its token) |
| `POST /-/labrat/snapshot?path=…` | Saves the database to a file |
| `POST /-/labrat/restore?path=…` | Restores a saved database |
| `GET /-/labrat/idle` | Returns once every background job and webhook delivery has finished |
| `POST /-/labrat/time` | Shifts labrat's clock |
| `GET /-/labrat/requests.json` | The last 500 API calls with requests and responses (also in the UI at `/-/labrat/requests`) |

## What is simulated

- **Users, groups, projects, members and tokens**, with GitLab's permission model.
- **Git over smart HTTP**: clone, fetch and push, protected branches, push options
  (`git push -o merge_request.create …`) and GitLab's push messages.
- **Repository APIs**: branches, tags, commits (create from actions, cherry-pick, revert), files,
  trees, compare, commit comments and discussions, commit statuses and the external pipelines
  they create.
- **Merge requests**: the full lifecycle, asynchronous preparation and mergeability, detailed merge
  status, every merge method with squash and templates, rebase, auto-merge (waiting for a green
  pipeline when one exists), scheduled merges (`merge_after`), source branch deletion with
  retargeting, diffs and versions, context commits, description templates, labels (with
  promotion), project and group milestones, time tracking with time logs, subscriptions, to-dos
  (including snoozing), resource events, contribution events, markdown uploads, and the list
  filters and sorts.
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

Out of scope: CI runners and jobs, issues, forks, and the rest listed in the
plan.

## Fidelity

`tests/conformance` holds scenarios recorded against a real GitLab 18.11 instance. Each scenario
is a sequence of API calls (and webhook deliveries). In check mode it runs against an in-process
labrat, and every response is compared with the recording after normalizing ids, times and tokens:
status, headers, body and key order.

```sh
cargo test                       # check labrat against the recordings
cargo test -p labrat-conformance --test conformance todo_lifecycle   # one scenario
LABRAT_RECORD=1 GITLAB_URL=… GITLAB_TOKEN=… \
  cargo test -p labrat-conformance -- --test-threads=1   # re-record against GitLab
```

The scenarios live in `tests/conformance/tests/scenarios/` and build into one test binary.

Recordings also store when each step was sent and when GitLab answered it. When a recording is
checked, the harness moves the clock of that labrat instance forward to GitLab's answer time before
the step, because GitLab writes near the end of a request. Behaviour that depends on time, such as
GitLab touching an MR at most once a minute, then replays the same way. Each instance has its own
clock (`POST /-/labrat/time`), so scenarios can run in parallel. A step within a fraction of a second
of such a boundary can still come out differently; re-record that scenario rather than adjusting
labrat.

The `random_walk*` scenarios run a fixed-seed sequence of operations by two users on one or two
MRs. Most of them also compare every webhook delivery. They find interactions that hand-written
scenarios miss.

`tests/client` drives labrat with the Rust `gitlab` crate and with the real `git` client.
[docs/discrepancies.md](docs/discrepancies.md) lists where GitLab's documentation and its behaviour
disagree. labrat follows the behaviour.
