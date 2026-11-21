# EE merge trains: investigation notes

This covers how GitLab EE 18.11.14 behaves for merge trains. Everything here was observed through
the API of the EE reference instance. No EE source was read, and no EE container was run. The
conformance scenarios under `tests/conformance/tests/scenarios_ee/merge_train_*.rs` and
`graphql_merge_trains.rs` record the details. Their fixtures are the spec. These notes give the
model behind them.

## CI on the reference instance

Merge trains need merge request pipelines, so the reference instance needs CI.

- **The CI configuration:** each scenario project gets a `.gitlab-ci.yml` with one job that
  runs for `merge_request_event` only (`labrat_conformance::CI_CONFIG`).
- **The fake runner:**
  - An instance runner, `labrat-fake-runner`, is registered with `POST /user/runners`. Its
    token is kept outside the repository, in `~/dev/gitlab-ee-ref/runner.env`, and passed to
    the recorder as `GITLAB_RUNNER_TOKEN`.
  - The harness acts as that runner through the runner API. It takes jobs with
    `POST /jobs/request` and finishes them with `PUT /jobs/:id`.
  - A job request must announce a current runner's feature set. Without it, GitLab drops the
    job as `runner_unsupported`.
- **Jobs from other scenarios:** an instance runner gets jobs from any project. The harness
  finishes jobs left over from other scenarios as soon as it gets them.
- **Scenario steps:**
  - `ci_enable` commits the configuration.
  - `ci_pick(n)` waits until the runner holds `n` of the project's jobs.
  - `ci_finish(ref, status)` finishes the held jobs of one ref.
  - `ci_drain` finishes the rest at the end.

When replaying, labrat doesn't read `.gitlab-ci.yml`. The harness switches labrat's fake CI on
instead, with `PUT /-/labrat/projects/:id/ci`. It picks and finishes jobs through
`/-/labrat/ci/pick` and `/-/labrat/ci/finish`.

## Settings

The settings are `merge_pipelines_enabled`, `merge_trains_enabled` and
`merge_trains_skip_train_allowed`. They are set through REST `PUT /projects/:id` (Maintainer;
a Developer gets 403) or GraphQL `projectCiCdSettingsUpdate`.

- The stored values are kept as given. They read through these rules:
  - `merge_trains_enabled` reads as false while `merge_pipelines_enabled` is false;
  - `merge_trains_skip_train_allowed` reads as false while trains read as off.
- GraphQL `Project.ciCdSettings` is null for anyone below Maintainer.
  `projectCiCdSettingsUpdate` returns `RESOURCE_UNAVAILABLE` for non-maintainers and unknown
  projects.
- 18.11 has no setting for parallel train pipelines (19.0) and no train enforcement (19.2).

## Merge request pipelines

- Merged results pipelines off: a detached pipeline on `refs/merge-requests/:iid/head`, with
  the MR head as its SHA.
- Merged results pipelines on: a merged results pipeline on `refs/merge-requests/:iid/merge`.
  Its SHA is the merge ref commit. The MR head counts as its source SHA for `head_pipeline`.
- Pipelines are created by a Sidekiq job, about a second after the MR is created or its source
  branch gets commits.
- Without "pipelines must succeed", a pending or running pipeline doesn't block merging. The
  MR stays `mergeable`.

## The REST API

| Endpoint | Behaviour |
|---|---|
| `GET /projects/:id/merge_trains` | All cars of the project, `id` descending. `scope` is `active` or `complete` (anything else: 400 `scope does not have a valid value`). `sort` is `asc`/`desc`. Trains off: `[]`. |
| `GET /projects/:id/merge_trains/:target_branch` | The same for one target branch. An unknown branch gives `[]`. |
| `GET /projects/:id/merge_trains/merge_requests/:iid` | The MR's car, merged ones included. With no car: 404 `Merge Train Merge Request Not Found`. An unknown MR: 404 `Not found`. |
| `POST /projects/:id/merge_trains/merge_requests/:iid` | Adds the MR. 201 with the whole train (active cars, ascending). With `auto_merge: true` while checks are pending: 202 `[]`, and the MR joins when its pipeline succeeds. |

`POST` errors:

| Condition | Response |
|---|---|
| Trains off | 400 `Merge trains are not enabled for this project` |
| Unknown MR | 404 `Not found` |
| Not mergeable now (pipeline still running, no `auto_merge`) | 400 `Failed to merge` |
| Already on the train, or auto-merge already set | 409 `Merge request is already set to Auto-Merge` |
| A `sha` that isn't the source head | 409 `SHA does not match HEAD of source branch: <head>` |

## Cars

- **Car JSON:** `id`, `merge_request` (basic), `user`, `pipeline` (basic or null),
  `created_at`, `updated_at`, `target_branch`, `status`, `merged_at` and `duration` (seconds
  on the train, set when merged).
- **Statuses:**
  - `idle` until the car's pipeline is created;
  - `fresh` with a current pipeline;
  - `stale` while being restarted;
  - `merged` after merging.
- **System notes:**
  - the first car: `started a [merge train](<project>/-/merge_trains)`;
  - later cars: `added this merge request to the [merge train](...) at position N`;
  - auto add: `enabled automatic add to merge train when checks pass`.
- **Pipelines:**
  - Each car's pipeline runs on `refs/merge-requests/:iid/train`. It is built on the train
    ref of the car ahead of it, or on the target branch for the first car.
  - All car pipelines are created at once, in the background, a moment after the add, and
    they run in parallel.
- **A car that passes before the cars ahead of it** waits, `fresh` with a successful pipeline.
  It merges when the cars ahead have merged.
- **Merging:** the first car merges when its pipeline passes. The MR gets an ordinary merge
  commit (`Merge branch 'x' into 'main'`), by the user who added it. The car becomes
  `merged`, and the next cars keep their pipelines.
- **A failed car pipeline:**
  - The car is deleted. It no longer shows in any listing.
  - The MR gets `removed this merge request from the merge train because the pipeline did not
    succeed. [Learn more](<help>/ci/pipelines/merge_trains.md#merge-request-dropped-from-the-merge-train).`
  - Auto-merge is off.
  - Every car behind it gets a new pipeline, and their old pipelines are canceled.
- **Removal:**
  - `POST .../cancel_merge_when_pipeline_succeeds` removes the car (201
    `{"status":"success"}`). The note is `removed this merge request from the merge train`.
  - The car's pipeline is canceled, and the cars behind it restart.
  - A second cancel gives `{"message":"Can't cancel the automatic merge","status":"error","http_status":406}`.
- **Merging past the train:** `PUT /merge` without `auto_merge` still merges at once on 18.11
  ("merge immediately").

See the fixtures for drops on push, draft and close; merging past the train; skipping the train;
and GraphQL.
