# Benchmarks

labrat compared with the GitLab instance it imitates: GitLab Community Edition 18.11.14 in its
official Docker image. Both were measured on the same machine: GitLab on 2026-11-11, labrat again
on 2026-11-23.

The comparison is lopsided by design. GitLab is a complete DevOps platform with PostgreSQL, Redis,
Gitaly, Sidekiq, Puma and nginx. labrat imitates the part of GitLab's API that integrations use,
in one process. These numbers show what that costs when all a test needs is the API.

## Results

| | GitLab CE 18.11 (Docker) | labrat | Difference |
|---|---|---|---|
| Download | 1.83 GB (compressed image) | 9.9 MB (binary, gzipped) | 185× smaller |
| Size on disk | 6.25 GB image | 25.0 MB binary (stripped) | 250× smaller |
| Processes | 89 | 1 | |
| Install: first start from an empty data directory | 242 s until healthy (203 s until the sign-in page loads) | 0: nothing to install; the first start takes 12 ms | |
| Start with existing data | 84 s | 12 ms | |
| Reset to an empty state | No built-in way; recreate the instance | 7 ms (`POST /-/labrat/reset`) | |
| Memory when idle | 3.03 GiB | 15 MB | 200× less |
| Peak memory during the workload | 3.55 GiB | 144 MB | 26× less |
| Data on disk after the runs | 475 MB (after 59 merge requests in total) | 4.8 MB SQLite file and its WAL (after 20) | |
| Workload: 20 merge requests, created to merged | 187 s and 191 s (two runs) | 2.3 s (median of five runs, 1.8 s to 2.4 s) | 82× faster |

Median latency of each call in the workload:

| Call | GitLab | labrat |
|---|---|---|
| Create a project with a README | 2,749 ms | 2.8 ms |
| Commit a file to a new branch | 1,413 ms | 2.1 ms |
| Open a merge request | 154 ms | 2.6 ms |
| Get a merge request | 95 ms | 3.2 ms |
| Wait until the merge request is mergeable | 3,097 ms | 106 ms |
| Comment | 104 ms | 2.2 ms |
| Merge | 1,114 ms | 5.8 ms |
| List merged merge requests | 101 ms | 2.2 ms |
| GraphQL query | 97 ms | 1.6 ms |

The GitLab column shows the first of its two runs. The labrat "wait until mergeable" time is mostly
the 100 ms polling interval of the workload script: the merge request is mergeable on the second poll.

One more result: during the first GitLab run, one call failed. Gitaly took 10.9 seconds to create a
repository, past GitLab's 10-second deadline, so `POST /projects` returned
`400 Failed to create repository`. The two runs in the table came after it, on the same instance.

Getting the software isn't compared: neither pulling GitLab's image nor building labrat counts.
For reference, `cargo build --release` takes 3 min 31 s with an empty Cargo cache on this machine,
including downloading 299 MB of crates.

## Why GitLab takes minutes to start

The Docker image contains GitLab's software, but the first start installs it into the data
directory. The Omnibus package runs a Chef configuration run (`gitlab-ctl reconfigure`), which on
this machine updated 403 of 1,097 resources in 3 min 14 s. The run:
- writes the configuration for nine services (PostgreSQL, Redis, Gitaly, Puma, Sidekiq,
  Workhorse, nginx, sshd and logrotate);
- creates the PostgreSQL cluster and loads GitLab's schema of 1,426 tables;
- seeds the database.

Every later start repeats part of that work. The Chef run checks all 644 resources again (16 s, even
with nothing to change), then Puma loads the Rails application, about 11,000 Ruby files plus their
gems (60 s), and the other services start. labrat has none of these steps: its schema is created in
SQLite on an empty database, and it starts serving right after.
| `cargo build --release` with dependencies already downloaded | 3 min 27 s |

## Setup

- **Machine:** Intel Core i7-4980HQ (4 cores, 8 threads, 2.8 GHz), 16 GB RAM, Apple SSD (SATA), Linux 7.0,
  Docker 29.1.
- **GitLab:** `gitlab/gitlab-ce:18.11.14-ce.0`, started with
  [`scripts/reference/gitlab.sh`](../scripts/reference/gitlab.sh). That script trims GitLab for a
  single user: Puma in single mode, 10 Sidekiq threads, Prometheus and the Kubernetes agent server
  off. A default installation uses more memory. The instance was prepared with
  [`scripts/reference/setup.sh`](../scripts/reference/setup.sh) (52 s) and left idle for 5 minutes
  before the idle memory was read.
- **labrat:** a release build (`cargo build --release`), run with `--db :memory:` and with a file
  database. Both gave the same results.
- **Workload:** [`scripts/bench/workload.py`](../scripts/bench/workload.py), 20 iterations, one
  client making one request at a time. Each iteration:
  - creates a project with a README;
  - commits a file to a new branch;
  - opens a merge request;
  - polls it every 100 ms until it is mergeable;
  - comments;
  - merges;
  - lists merged merge requests;
  - reads the merge request over GraphQL.
- **Memory:** `docker stats` for GitLab, sampled every second during the workload. `VmRSS` and
  `VmHWM` from `/proc` for labrat.
- **Start time:** from starting the container or process until the first successful HTTP response.

## Running it yourself

labrat:

```sh
cargo build --release
./target/release/labrat --db :memory: --root-token glpat-bench-token &
python3 scripts/bench/workload.py http://127.0.0.1:8080 glpat-bench-token 20
```

GitLab: set up an instance as described in [MAINTAINING.md](MAINTAINING.md), then run the same
script against it:

```sh
set -a; . "$GITLAB_HOME/labrat.env"; set +a
python3 scripts/bench/workload.py "$GITLAB_URL" "$GITLAB_TOKEN" 20
```
