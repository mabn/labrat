<div align="center">

# labrat

**A fast, lightweight GitLab simulator for testing anything that talks to GitLab.**

[![GitHub Release](https://img.shields.io/github/v/release/mabn/labrat?style=flat-square)](https://github.com/mabn/labrat/releases)
[![GitLab 18.11](https://img.shields.io/badge/GitLab-18.11%20CE%20%26%20EE-FC6D26?style=flat-square&logo=gitlab&logoColor=white)](#-how-faithful-is-it)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)

*REST API • GraphQL • Webhooks • Git hosting • Web UI — in a single 25 MB binary.*

[Quick Start](#-quick-start) •
[Benchmarks](#-labrat-vs-gitlab) •
[Features](#-what-is-simulated) •
[Configuration](#-configuration) •
[Docs](#-documentation)

<br />

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/benchmark-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/benchmark-light.svg">
  <img src="docs/assets/benchmark-light.svg" alt="labrat compared with GitLab: 20 merge requests from creation to merge take 0.68 s instead of 189 s; idle memory is 15 MB instead of 3.0 GB; starting from an empty data directory takes 0.01 s instead of 4 minutes." width="760">
</picture>

*labrat compared with GitLab CE 18.11 in Docker on the same laptop. [See how this was measured →](docs/BENCHMARKS.md)*

</div>

---

**labrat** answers GitLab's REST API, GraphQL API, webhooks, and Git over HTTP the way GitLab 18.11 does. Point your bot, CLI, code-review tool, or sync job at it instead of a real GitLab instance, and your test suite starts in **milliseconds instead of minutes**.

## ✨ Highlights

- **100× faster:** Runs a 20-MR workload in **0.68 s** instead of 189 s. Starts in ~11 ms and resets to a clean state in 6 ms.
- **Zero dependencies:** One 25 MB binary and one SQLite file (or `:memory:`). No Docker, PostgreSQL, Redis, or Sidekiq—just **15 MB** of idle RAM instead of 3 GB.
- **Verified against real GitLab:** Tested against **16,504 API responses** and **1,428 webhook deliveries** recorded from real GitLab CE & EE 18.11 instances—comparing status codes, headers, bodies, and JSON key order.
- **Built for test suites:** Dedicated test endpoints to reset, snapshot/restore, wait for background jobs (`/-/labrat/idle`), shift the clock, and inspect a live request log.
- **Drop-in compatibility:** Tested with the Rust `gitlab` crate, `curl`, and standard `git clone`/`git push`.
- **Built-in Web UI:** A GitLab-style interface to browse projects, review merge requests, and inspect requests while developing or debugging.

> [!IMPORTANT]
> **labrat is a test simulator, not a GitLab replacement.**
> It runs as a single process backed by SQLite, exposes unauthenticated test endpoints, and only implements Git hosting and Merge Request workflows (no real CI runners, Issues, forks, or HA).

---

## 🚀 Quick Start

### 1. Install

**Homebrew (macOS & Linux):**
```sh
brew install mabn/tap/labrat
```

**Install script (macOS & Linux — installs to `~/.cargo/bin`):**
```sh
curl -LsSf https://github.com/mabn/labrat/releases/latest/download/labrat-installer.sh | sh
```

**Docker (great for CI service containers):**
```sh
docker run --rm -p 8080:8080 ghcr.io/mabn/labrat --db :memory: --root-token glpat-test-token
```

Prebuilt binaries for macOS (Apple Silicon & Intel), Linux (x86_64 & arm64, static), and Windows are available on the [Releases page](https://github.com/mabn/labrat/releases).

### 2. Run & Connect

Start labrat with an in-memory database and a known root token:

```sh
labrat --db :memory: --root-token glpat-test-token
```

Use it exactly like a real GitLab instance on `http://127.0.0.1:8080`:

```sh
# Create a project via the REST API
curl -H "PRIVATE-TOKEN: glpat-test-token" -X POST localhost:8080/api/v4/projects \
  -H 'Content-Type: application/json' \
  -d '{"name": "demo", "initialize_with_readme": true}'

# Clone it over HTTP
git clone http://root:glpat-test-token@localhost:8080/root/demo.git
```

Open **<http://localhost:8080/>** in your browser and sign in as `root` (default password: `labrat-root-password`, or use your token). Click **"Generate demo data"** in the sidebar (or `POST /-/labrat/demo`) to populate example projects, or run `scripts/seed-demo.sh` for a smaller dataset.

<p align="center">
  <img src="docs/assets/web-ui-merge-request.png" alt="labrat's web UI showing an open merge request: the description, an approve and merge widget, a review thread with a suggested change, and a sidebar with reviewers and labels." width="760">
  <br />
  <i>labrat's GitLab-style Web UI showing a merge request with approval rules, merge widget, inline suggested changes, and EE dependencies.</i>
</p>

### 3. Use in Automated Tests

Start one `labrat` process for your suite and reset it between tests:

```sh
curl -X POST localhost:8080/-/labrat/reset   # Wipes state in ~6 ms (keeps root & token)
# ... run your code under test against http://localhost:8080 ...
curl localhost:8080/-/labrat/idle            # Blocks until background jobs & webhooks finish
```

> [!TIP]
> Like real GitLab, new merge requests briefly enter a `preparing` state while mergeability is computed asynchronously. Call `/-/labrat/idle` before asserting on MR state—or start labrat with `--job-delay-ms` to test how your code handles slow background jobs.

---

## 📊 labrat vs. GitLab

| Metric | GitLab CE 18.11 (Docker) | **labrat** |
| :--- | :--- | :--- |
| **Download size** | 1.83 GB image | **10.1 MB** (gzipped) |
| **Size on disk** | 6.25 GB | **25.3 MB** |
| **First-time setup** | ~4 minutes | **`brew install` (instant)** |
| **Startup time** | 84 s | **11 ms** |
| **Reset to empty state** | ~5 minutes (recreate container) | **6 ms** |
| **Idle memory** | 3.0 GB | **15 MB** |
| **Peak memory (workload)** | 3.5 GB | **28 MB** |
| **20 MRs (create → merge)** | 189 s | **0.68 s** |
| **Processes** | 89 | **1** |

Full methodology, workload details, and hardware specs are in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

---

## 🧩 What is Simulated

labrat implements GitLab's **Enterprise Edition 18.11** behavior by default (pass `--noee` for Community Edition).

<details>
<summary><b>Click to expand full API & feature coverage</b></summary>

- **Users, groups, projects, members, and tokens** with GitLab's permission model.
- **Git over smart HTTP:** clone, fetch, push, protected branches, push options (`git push -o merge_request.create …`), and GitLab's push messages.
- **Repository APIs:** branches, tags, commits (create from actions, cherry-pick, revert), files, trees, compare, commit comments/discussions, commit statuses, and the external pipelines they create.
- **Merge requests:**
  - Full lifecycle, async preparation and mergeability, and detailed merge status.
  - All merge methods with squash/templates, rebase, auto-merge (waiting for green pipelines), and scheduled merges (`merge_after`).
  - Source branch deletion with retargeting, diffs/versions, context commits, and description templates.
  - Labels (with promotion), project/group milestones, time tracking and time logs.
  - Subscriptions, to-dos (including snoozing), resource/contribution events, markdown uploads, and full list filters/sorts.
- **Code review:** notes, threads, diff notes with position tracing, suggestions, award emoji, draft notes and review submission, approvals, reviewer states, and quick actions.
- **Webhooks:** push, tag push, merge request, comment (MR & commit), emoji, and pipeline events—with GitLab's payloads/headers, branch filters, delivery logs, test deliveries, and system hooks (`repository_update`).
- **GraphQL:** GitLab 18.11 schema for introspection and validation, with queries and mutations for merge requests (including `mergeRequests` filters and `mergeabilityChecks`), notes, discussions, labels, emoji, and to-dos.
- **Search:** `/search`, `/groups/:id/search`, and `/projects/:id/search` for MRs, milestones, notes, commits, users, and projects.
- **Web UI:** Single-page app backed exclusively by labrat's REST and GraphQL APIs—projects, groups, file editing/committing, branches, commits, MRs (diffs, inline threads, suggestions, approvals, merge/rebase), settings, webhooks, tokens, and request log.

**Enterprise Edition (EE) additions** (disabled with `--noee`):
- **Approval rules:** Project and MR approval rules, settings, and state over REST & GraphQL (including EE defaults like resetting approvals on push and disallowing self-approval).
- **Code owners:** `code_owner` approval rules and merge blocking (configured via test endpoint; labrat does not parse `CODEOWNERS` files).
- **Approval policies:** Policy approval rules, violations, warn-mode dismissals, bypasses, and security bot comments (configured via test endpoint; no real security scans).
- **MR dependencies:** Blocking merge requests and resulting merge status over REST & GraphQL.
- **Merge trains:** Settings, train listings, pipelines, merges, and restarts via labrat's fake CI test endpoints.
- *Not included:* External status checks, epics, security/compliance scanners, real CI runners, issues, and forks.

See [docs/PLAN.md](docs/PLAN.md) for the complete architectural scope.

</details>

---

## 🎯 How Faithful Is It?

labrat is verified against **real GitLab instances**, not just documentation.

- **Conformance suite:** `tests/conformance` contains **154 CE scenarios** and **168 EE scenarios** recorded against GitLab 18.11. `cargo test` replays every API call, GraphQL query, git push, and webhook against labrat and diffs status codes, headers, bodies, and JSON key order.
- **Deterministic time:** Recordings capture timestamps and shift labrat's clock to match, accurately reproducing time-gated behaviors (such as GitLab touching an MR at most once per minute).
- **Random walks:** Fixed-seed `random_walk*` scenarios simulate two concurrent users on merge requests and diff every webhook delivery to catch edge-case interactions.
- **Behavior over docs:** Where GitLab's documentation and actual behavior disagree, labrat matches the behavior ([see docs/discrepancies.md](docs/discrepancies.md)).

```sh
cargo test                                                          # Run full conformance suite
cargo test -p labrat-conformance --test conformance todo_lifecycle  # Run a single scenario
```

---

## 🔧 Configuration

| Flag | Environment Variable | Default | Description |
| :--- | :--- | :--- | :--- |
| `--listen` | `LABRAT_LISTEN` | `127.0.0.1:8080` | Address to listen on |
| `--db` | `LABRAT_DB` | `labrat.sqlite` | SQLite file path, or `:memory:` |
| `--external-url` | `LABRAT_EXTERNAL_URL` | `http://<listen>` | Base URL used in links and webhook payloads |
| `--root-password` | `LABRAT_ROOT_PASSWORD` | `labrat-root-password` | Password for `root` on an empty DB |
| `--root-token` | `LABRAT_ROOT_TOKEN` | *none* | Personal access token for `root` |
| `--ssh-port` | `LABRAT_SSH_PORT` | `22` | Port shown in `ssh_url_to_repo` (SSH is not served) |
| `--job-delay-ms` | `LABRAT_JOB_DELAY_MS` | `0` | Delay every background job to expose intermediate states |
| `--job-jitter-ms` | `LABRAT_JOB_JITTER_MS` | `0` | Random extra delay per background job |
| `--noee` | `LABRAT_NOEE` | `off` | Emulate GitLab Community Edition instead of EE |

> [!NOTE]
> The SQLite schema version is checked on startup. If an upgrade changes the schema, delete your existing database file.

### Test & Control Endpoints

> [!WARNING]
> These endpoints require **no authentication**. Only bind labrat to interfaces your tests can reach.

| Endpoint | Description |
| :--- | :--- |
| `POST /-/labrat/reset` | Wipes all data and recreates `root` (and its token) |
| `POST /-/labrat/snapshot?path=…` | Saves the current database to a file |
| `POST /-/labrat/restore?path=…` | Restores a saved database snapshot |
| `GET /-/labrat/idle` | Blocks until all background jobs and webhook deliveries finish |
| `POST /-/labrat/time` | Shifts labrat's internal clock |
| `GET /-/labrat/requests.json` | Last 500 API calls with requests/responses (UI: `/-/labrat/requests`) |
| `POST /-/labrat/demo` | Seeds a `demo` group showcasing features (`409` if it already exists) |

<details>
<summary><b>EE & Fake CI test endpoints (Merge Trains, Code Owners, Approval Policies)</b></summary>

| Endpoint | Description |
| :--- | :--- |
| `PUT /-/labrat/projects/:id/ci` | Configure fake CI for EE merge trains: `{"enabled": true, "jobs": ["test"], "runner": {"mode": "manual"}}` or `"mode": "auto"` with `"status"` (`success`, `failed`, `canceled`), `"duration_ms"`, and per-ref `"overrides"`. `{"enabled": false}` disables it; `GET` reads it. |
| `GET /-/labrat/projects/:id/ci/jobs` | List the project's fake CI jobs. |
| `POST /-/labrat/ci/pick` | `{"project_id": 1}` — fake runner starts the project's pending jobs. |
| `POST /-/labrat/ci/finish` | `{"project_id": 1, "ref": "refs/merge-requests/1/train", "status": "failed"}` — finishes running jobs on that ref (pass `"include_pending": true` for pending jobs too). |
| `PUT /-/labrat/projects/:id/code_owners` | Set EE code owners for a branch: `{"branch": "main", "sections": [{"name": null, "entries": [{"pattern": "*", "owners": ["@alice"]}]}, {"name": "Backend", "approvals": 2, "optional": false, "entries": [{"pattern": "/app/", "owners": ["@bob", "@group/path"]}]}]}`. Applies to open MRs immediately. `GET` / `DELETE` accept `?branch=`. |
| `PUT /-/labrat/projects/:id/approval_policies` | Set EE approval policies: `{"approval_policy": [...]}` (GitLab policy schema JSON). Replaces policies and re-evaluates open MRs. `GET` reads, `DELETE` removes. |
| `PUT /-/labrat/projects/:id/merge_requests/:iid/policy_results` | Override policy results on one MR by name (`null` uses labrat's evaluation): `{"policies": {"Licenses": {"violated": false}, "No critical vulns": {"violated": true, "status": "FAILED", "newScanFinding": [{"name": "SQL injection", "path": "app/db.rb", "reportType": "SAST", "severity": "CRITICAL", "location": {}}]}}}`. Supports GraphQL `PolicyViolationDetails` lists: `newScanFinding`, `previousScanFinding`, `licenseScanning`, `errors`, `comparisonPipelines`. |

</details>

---

## 📚 Documentation

- **[docs/BENCHMARKS.md](docs/BENCHMARKS.md)** — Benchmark methodology, workload, and hardware details.
- **[docs/MAINTAINING.md](docs/MAINTAINING.md)** — Recording fixtures, writing conformance scenarios, and upgrading GitLab versions.
- **[docs/PLAN.md](docs/PLAN.md)** — Architecture and full feature scope.
- **[docs/discrepancies.md](docs/discrepancies.md)** — Where GitLab's documentation and real behavior disagree.