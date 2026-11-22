# labrat

A GitLab simulator for testing API integrations. It is one Rust binary that serves GitLab's REST v4
and GraphQL APIs, webhooks and Git over smart HTTP, with all state in a single SQLite file. Nothing
is built yet; the project is at the planning stage.

- [README.md](README.md): what labrat is and isn't.
- [docs/PLAN.md](docs/PLAN.md): goals, architecture, testing, scope (in, later, out) and roadmap.
  Read it before starting any work.

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
  container at `/opt/gitlab/embedded/service/gitlab-rails`. `docs/MAINTAINING.md` explains how
  to set up an instance, record fixtures and move to a newer GitLab version.
- **Use only GitLab CE as the reference:** its source and a CE instance.
- **GitLab EE only through its API.** EE mode (the default; `--noee` gives CE) is checked against
  an EE instance, set up as `docs/MAINTAINING.md` ("The EE reference") describes.
  - Interact with it only through its API: REST, GraphQL, git and webhooks.
  - Never look at GitLab EE source code: not the `ee/` directory, not EE packages or containers.
  - Don't reference EE code either, even code you recall from training. That rules out EE Ruby
    class and module names (`EE::API::Entities::...`, EE services and workers), `ee/` file
    paths and EE-only method names, in code, comments, docs and commit messages.
  - Base EE behaviour on GitLab's public documentation and on what the EE instance's API
    returns (the `fixtures-ee` recordings). Names the API itself exposes are fine, such as
    GraphQL types and global IDs.
- **Stay in scope.** Check the scope lists in `docs/PLAN.md` before adding a feature, and ask before
  building anything listed as out of scope.
- **Test at the HTTP level.** Most tests compare HTTP responses precisely against recorded GitLab
  responses. Keep tests that use the `gitlab` crate to a few end-to-end flows.
- **All state lives in SQLite, git objects included.** The only file labrat writes is its database.
- **Every action goes through a service.** That keeps side effects identical whether an action comes
  from REST, GraphQL, the UI, a quick action or a git push.
- **Keep the docs true.** Update `docs/PLAN.md` when the design or the scope changes.

## Commands

- `cargo build`, `cargo test`: build labrat and run the conformance and client tests.
- Web UI (`web/`, needs Node.js 20.19+ and pnpm): `cargo build` runs `pnpm install` and
  `pnpm build` in `web/` when its sources change and embeds `web/dist` (set
  `LABRAT_SKIP_WEB_BUILD=1` to embed the existing `web/dist` as is). `pnpm dev` serves the UI on
  :5173 against a labrat on :8080. The UI talks to labrat only through the GitLab APIs; see
  `web/README.md`.
