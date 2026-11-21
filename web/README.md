# labrat web UI

A single-page app (React, Vite, TypeScript) that looks and works like GitLab's UI for the parts
labrat implements. It talks to labrat only through the GitLab APIs (`/api/v4` and `/api/graphql`),
so everything it shows or does is also available to any other client.

## Build

```sh
cargo build      # from the repository root
```

`cargo build` runs `pnpm install --frozen-lockfile` and `pnpm build` (type-check, then write
`dist/`) here whenever the UI's sources change, and embeds `dist/` into the labrat binary. Without
pnpm on `PATH`, or with `LABRAT_SKIP_WEB_BUILD=1`, it embeds whatever `dist/` already holds.
`dist/` is not committed; a labrat built without it serves a page that says how to build the UI.

## Develop

```sh
cargo run -- --db :memory: --root-token glpat-test-token   # labrat on :8080
pnpm dev                                                   # the UI on http://localhost:5173
```

The dev server forwards `/api`, `/uploads`, the sign-in and sign-out form posts and the request log
to labrat (`LABRAT_URL` overrides `http://127.0.0.1:8080`).

## How it fits together

- **Signing in** posts a form to `/users/sign_in`, which starts a cookie session as GitLab's sign-in
  does. The API accepts that session for reads, and for writes when the request carries the
  session's CSRF token (`X-CSRF-Token`, read from the `_labrat_csrf` cookie). The sign-in page can
  also take a personal access token, kept in the browser's local storage.
- **URLs** follow GitLab's: `/<namespace>/<project>/-/merge_requests/1/diffs`, `/-/tree/<ref>/<path>`
  and so on. labrat serves `index.html` for every path outside the API and git, so `web_url`s in API
  responses open the right page.
- **Request log**: the app sends `X-Labrat-UI: 1`, and labrat keeps those calls out of
  `/-/labrat/requests`, which stays a log of the clients under test.
- **Markdown** is rendered in the browser (marked and DOMPurify), close to but not exactly GitLab's.

## Layout

- `src/api/`: the API client (pagination, auth headers, errors) and the entity types used.
- `src/components/`: layout (sidebar and breadcrumbs), shared widgets, members, labels and webhooks.
- `src/pages/`: dashboards, groups, admin, and `project/` and `mr/` for project and merge request
  pages.
