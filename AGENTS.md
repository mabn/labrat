# labrat

A GitLab simulator for testing API integrations. It is one Rust binary that serves GitLab's REST v4
and GraphQL APIs, webhooks and Git over smart HTTP, with all state in a single SQLite file. Nothing
is built yet; the project is at the planning stage.

- [README.md](README.md): what labrat is and isn't.
- [docs/PLAN.md](docs/PLAN.md): goals, architecture, testing, scope (in, later, out) and roadmap.
  Read it before starting any work.
- [docs/LOG.md](docs/LOG.md): the work log. Its top entry shows the current state and the next task.

## Rules

- **GitLab is the spec.** Match GitLab CE 18.11 exactly. That covers:
  - routes, parameters and defaults;
  - error bodies and status codes;
  - JSON fields and their order;
  - pagination headers;
  - system note wording;
  - webhook payloads and headers.

  When GitLab's docs and the reference instance disagree, the instance wins. Record every such case
  in `docs/discrepancies.md` (the format is in `docs/PLAN.md` under "Reference version").
- **Check against the reference instance.** It is a GitLab CE 18.11 instance, locally at
  `http://localhost:8929` and set up in `~/dev/gitlab-run-ce`. The exact Rails source is in its
  container at `/opt/gitlab/embedded/service/gitlab-rails`.
- **Use only GitLab CE as the reference:** its source and a CE instance.
- **Stay in scope.** Check the scope lists in `docs/PLAN.md` before adding a feature, and ask before
  building anything listed as out of scope.
- **Test at the HTTP level.** Most tests compare HTTP responses precisely against recorded GitLab
  responses. Keep tests that use the `gitlab` crate to a few end-to-end flows.
- **All state lives in SQLite, git objects included.** The only file labrat writes is its database.
- **Every action goes through a service.** That keeps side effects identical whether an action comes
  from REST, GraphQL, the UI, a quick action or a git push.
- **Keep the docs true.** Update `docs/PLAN.md` when the design or the scope changes.
- **Keep `docs/LOG.md` current.** It is newest first, with timestamps in UTC.
  - Add an entry for each significant step.
  - During active work, add one at least every 2 hours. Skip it when idle and nothing has changed.
  - Each entry says what was done, what is in progress and what comes next.

## Commands

There is no code yet. Add build and test commands here once the Cargo workspace exists.
