//! Demo data for trying labrat in the web UI: `POST /-/labrat/demo`.
//!
//! It adds a `demo` group with a few projects whose merge requests each show one feature, and
//! the users that take part in them. Nothing else changes. The data is made through labrat's own
//! API, called in-process as the demo users, so it goes through the same services (system notes,
//! to-dos, pipelines, approvals) as requests from outside.

use crate::ctx::ApiError;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request};
use labrat_core::App;
use labrat_core::model::User;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

/// Marks a request labrat makes to itself, acting as this user (see [`crate::ctx::Req`]).
#[derive(Clone, Copy)]
pub struct InternalUser(pub i64);

/// The password of the users the demo data creates.
pub const PASSWORD: &str = "demo-password-123";

const GROUP: &str = "demo";

/// (username, name); bob is a maintainer, the others developers.
const USERS: &[(&str, &str)] = &[
    ("alice", "Alice Smith"),
    ("bob", "Bob Jones"),
    ("carol", "Carol Lee"),
    ("dave", "Dave Kim"),
    ("erin", "Erin Novak"),
    ("frank", "Frank Ortiz"),
];

struct Demo {
    router: Router,
    app: Arc<App>,
    ee: bool,
    root: i64,
    users: HashMap<&'static str, i64>,
}

type R<T> = Result<T, ApiError>;

fn fail(msg: String) -> ApiError {
    labrat_core::Error::message(500, msg).into()
}

impl Demo {
    fn user(&self, name: &str) -> i64 {
        if name == "root" { self.root } else { self.users[name] }
    }

    /// Calls a labrat route as `user` and waits until the background work it started is done.
    async fn call(&self, user: &str, method: Method, path: &str, body: Value) -> R<Value> {
        let write = method != Method::GET;
        let mut req = Request::builder().method(method.clone()).uri(path).extension(InternalUser(self.user(user)));
        let body = if body.is_null() {
            Body::empty()
        } else {
            req = req.header("content-type", "application/json");
            Body::from(body.to_string())
        };
        let resp = self.router.clone().oneshot(req.body(body).map_err(|e| fail(e.to_string()))?).await.map_err(|e| fail(e.to_string()))?;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 30).await.map_err(|e| fail(e.to_string()))?;
        if write {
            crate::jobs::wait_idle(&self.app).await;
        }
        if !status.is_success() {
            return Err(fail(format!(
                "Generating the demo data failed at {method} {path} ({status}): {}. The demo data is incomplete; reset labrat before trying again.",
                String::from_utf8_lossy(&bytes)
            )));
        }
        Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn post(&self, user: &str, path: &str, body: Value) -> R<Value> {
        self.call(user, Method::POST, &format!("/api/v4{path}"), body).await
    }

    async fn put(&self, user: &str, path: &str, body: Value) -> R<Value> {
        self.call(user, Method::PUT, &format!("/api/v4{path}"), body).await
    }

    async fn get(&self, user: &str, path: &str) -> R<Value> {
        self.call(user, Method::GET, &format!("/api/v4{path}"), Value::Null).await
    }

    /// A labrat test endpoint (`/-/labrat/...`).
    async fn labrat(&self, method: Method, path: &str, body: Value) -> R<Value> {
        self.call("root", method, &format!("/-/labrat{path}"), body).await
    }

    async fn project(&self, path: &str, name: &str, description: &str, group: i64, files: &[(&str, &str)]) -> R<i64> {
        let p = self
            .post("root", "/projects", json!({"name": name, "path": path, "namespace_id": group, "description": description, "initialize_with_readme": true}))
            .await?;
        let id = p["id"].as_i64().unwrap_or_default();
        for (u, _) in USERS {
            let level = if *u == "bob" { 40 } else { 30 };
            self.post("root", &format!("/projects/{id}/members"), json!({"user_id": self.user(u), "access_level": level})).await?;
        }
        let actions: Vec<Value> = files.iter().map(|(f, content)| json!({"action": "create", "file_path": f, "content": content})).collect();
        self.post("root", &format!("/projects/{id}/repository/commits"), json!({"branch": "main", "commit_message": "Add the first version", "actions": actions})).await?;
        Ok(id)
    }

    /// Commits files to `branch` as `user`; a new branch starts from main.
    async fn commit(&self, user: &str, project: i64, branch: &str, message: &str, files: &[(&str, &str, &str)]) -> R<()> {
        let actions: Vec<Value> = files.iter().map(|(a, f, content)| json!({"action": a, "file_path": f, "content": content})).collect();
        let exists = self.get("root", &format!("/projects/{project}/repository/branches/{}", branch.replace('/', "%2F"))).await.is_ok();
        let mut body = json!({"branch": branch, "commit_message": message, "actions": actions});
        if !exists {
            body["start_branch"] = json!("main");
        }
        self.post(user, &format!("/projects/{project}/repository/commits"), body).await?;
        Ok(())
    }

    /// A branch with one commit and a merge request from it; returns the MR's iid.
    #[allow(clippy::too_many_arguments)]
    async fn mr(&self, author: &str, project: i64, branch: &str, files: &[(&str, &str, &str)], title: &str, description: &str, extra: Value) -> R<i64> {
        self.commit(author, project, branch, title.trim_start_matches("Draft: "), files).await?;
        let mut body = json!({"source_branch": branch, "target_branch": "main", "title": title, "description": description});
        if let Value::Object(m) = extra {
            for (k, v) in m {
                body[k] = v;
            }
        }
        let mr = self.post(author, &format!("/projects/{project}/merge_requests"), body).await?;
        Ok(mr["iid"].as_i64().unwrap_or_default())
    }

    async fn note(&self, user: &str, project: i64, iid: i64, body: &str) -> R<Value> {
        self.post(user, &format!("/projects/{project}/merge_requests/{iid}/notes"), json!({"body": body})).await
    }

    async fn approve(&self, users: &[&str], project: i64, iid: i64) -> R<()> {
        for u in users {
            self.post(u, &format!("/projects/{project}/merge_requests/{iid}/approve"), json!({})).await?;
        }
        Ok(())
    }

    /// The fake runner picks up (and with `status`, finishes) the jobs of the MR's pipeline on
    /// `kind` (`head`, `merge` or `train`).
    async fn run_jobs(&self, project: i64, iid: i64, kind: &str, status: Option<&str>) -> R<()> {
        let r#ref = format!("refs/merge-requests/{iid}/{kind}");
        self.labrat(Method::POST, "/ci/pick", json!({"project_id": project, "ref": r#ref})).await?;
        if let Some(status) = status {
            self.labrat(Method::POST, "/ci/finish", json!({"project_id": project, "ref": r#ref, "status": status})).await?;
        }
        Ok(())
    }

    fn pipeline_ref(&self) -> &'static str {
        // With merged results pipelines on (EE), MR pipelines run on the merge ref.
        if self.ee { "merge" } else { "head" }
    }
}

/// Adds the demo data. Fails with 409 when the `demo` group exists already.
pub async fn generate(app: Arc<App>) -> R<Value> {
    let (root, exists, existing) = app.db.read(|c| {
        let root = User::find_by_username(c, "root")?.map(|u| u.id).unwrap_or(1);
        let exists = labrat_core::services::paths::route_taken(c, GROUP)?;
        let mut existing = HashMap::new();
        for (u, _) in USERS {
            if let Some(user) = User::find_by_username(c, u)? {
                existing.insert(*u, user.id);
            }
        }
        Ok::<_, labrat_core::Error>((root, exists, existing))
    })?;
    if exists {
        return Err(labrat_core::Error::message(409, "The demo group exists already. Reset labrat to generate the demo data again.").into());
    }
    let mut d = Demo { router: crate::routes(app.clone()), ee: app.config.is_ee(), app, root, users: existing };
    for (u, name) in USERS {
        if d.users.contains_key(u) {
            continue;
        }
        let user = d
            .post("root", "/users", json!({"username": u, "name": name, "email": format!("{u}@example.com"), "password": PASSWORD, "skip_confirmation": true}))
            .await?;
        d.users.insert(u, user["id"].as_i64().unwrap_or_default());
    }
    let group = d.post("root", "/groups", json!({"name": "Demo", "path": GROUP, "visibility": "internal", "description": "Demo projects: each merge request shows one feature."})).await?;
    let group = group["id"].as_i64().unwrap_or_default();

    let mut projects = vec![basics(&d, group).await?, pipelines(&d, group).await?];
    if d.ee {
        projects.push(approvals(&d, group).await?);
        projects.push(policies(&d, group).await?);
    }
    let users: Vec<&str> = USERS.iter().map(|(u, _)| *u).collect();
    Ok(json!({"group": GROUP, "projects": projects, "users": users, "password": PASSWORD}))
}

const ROUTES: &str = r#"def index(request):
    return "Hello!"


def health(request):
    return {"status": "ok"}


def routes():
    return [
        ("/", index),
        ("/health", health),
    ]
"#;

const CONFIG: &str = "# Server settings\nPORT = 8000\nHOST = \"0.0.0.0\"\n\n# Logging\nLOG_LEVEL = \"info\"\n";

const SERVER: &str = r#"from app.config import HOST, PORT
from app.routes import routes


def serve(app, timeout=30):
    for path, handler in routes():
        app.add(path, handler)
    app.run(HOST, PORT, timeout=timeout)
"#;

/// Merge request basics: review, merge states and metadata. CE and EE.
async fn basics(d: &Demo, group: i64) -> R<Value> {
    let p = d
        .project("web-app", "Web App", "Merge request basics: reviews, threads, merge states and metadata.", group, &[
            ("app/routes.py", ROUTES),
            ("app/config.py", CONFIG),
            ("app/server.py", SERVER),
        ])
        .await?;
    d.post("root", &format!("/projects/{p}/labels"), json!({"name": "bug", "color": "#dc143c"})).await?;
    d.post("root", &format!("/projects/{p}/labels"), json!({"name": "frontend", "color": "#6699cc"})).await?;
    let milestone = d.post("root", &format!("/projects/{p}/milestones"), json!({"title": "v1.0"})).await?;
    let (bob, dave, root) = (d.user("bob"), d.user("dave"), d.user("root"));

    // 1. Approved and ready.
    let iid = d
        .mr("alice", p, "feature/about-page", &[("update", "app/routes.py", &ROUTES.replace("        (\"/health\", health),\n", "        (\"/health\", health),\n        (\"/about\", about),\n").replace("def routes", "def about(request):\n    return \"About us\"\n\n\ndef routes"))],
            "Ready to merge: approved, no open threads",
            "Shows a merge request that can be merged: a reviewer approved it and every thread is resolved.",
            json!({"reviewer_ids": [bob]}))
        .await?;
    d.note("carol", p, iid, "Looks good to me.").await?;
    d.approve(&["bob"], p, iid).await?;

    // 2. Review threads: a suggestion, a resolved thread, an open one and a mention.
    let greeting = ROUTES.replace("    return \"Hello!\"", "    name = request.args.get(\"name\", \"world\")\n    return f\"Hello, {name}!\"");
    let iid = d
        .mr("alice", p, "feature/greeting", &[("update", "app/routes.py", &greeting)],
            "Review threads: a suggestion, a resolved thread and an open one",
            "Shows a review in progress: a diff comment with a suggested change, a resolved thread, an open thread, an emoji reaction and a mention of root, which gives root a to-do.",
            json!({"reviewer_ids": [bob, root], "assignee_ids": [d.user("alice")]}))
        .await?;
    let v = d.get("root", &format!("/projects/{p}/merge_requests/{iid}/versions")).await?;
    let v = &v[0];
    let position = json!({"base_sha": v["base_commit_sha"], "start_sha": v["start_commit_sha"], "head_sha": v["head_commit_sha"], "position_type": "text", "old_path": "app/routes.py", "new_path": "app/routes.py", "new_line": 2});
    d.post("bob", &format!("/projects/{p}/merge_requests/{iid}/discussions"), json!({"body": "An empty name gives \"Hello, !\". How about this?\n\n```suggestion:-0+0\n    name = request.args.get(\"name\") or \"world\"\n```", "position": position})).await?;
    let t = d.post("carol", &format!("/projects/{p}/merge_requests/{iid}/discussions"), json!({"body": "Do we need to escape the name?"})).await?;
    let t = t["id"].as_str().unwrap_or_default().to_string();
    d.post("alice", &format!("/projects/{p}/merge_requests/{iid}/discussions/{t}/notes"), json!({"body": "No, the response is plain text."})).await?;
    d.put("alice", &format!("/projects/{p}/merge_requests/{iid}/discussions/{t}"), json!({"resolved": true})).await?;
    d.post("bob", &format!("/projects/{p}/merge_requests/{iid}/discussions"), json!({"body": "@root could you check this against the style guide too?"})).await?;
    d.post("carol", &format!("/projects/{p}/merge_requests/{iid}/award_emoji"), json!({"name": "thumbsup"})).await?;

    // 3. Draft.
    d.mr("erin", p, "wip/dark-mode", &[("create", "app/theme.py", "COLORS = {\n    \"background\": \"#1f1e24\",\n    \"text\": \"#ececef\",\n}\n")],
        "Draft: a draft can't be merged until it's marked ready",
        "Shows a draft merge request. Mark it as ready to allow merging.",
        json!({})).await?;

    // 4. Conflict: main changes the same line after the branch.
    d.mr("dave", p, "fix/port", &[("update", "app/config.py", &CONFIG.replace("PORT = 8000", "PORT = 8080"))],
        "Merge conflict: main changed the same line",
        "Shows a merge request that conflicts with its target branch: main changed the port too.",
        json!({})).await?;
    d.commit("bob", p, "main", "Use port 9000", &[("update", "app/config.py", &CONFIG.replace("PORT = 8000", "PORT = 9000"))]).await?;

    // 5. Changes requested.
    let iid = d
        .mr("alice", p, "feature/short-timeout", &[("update", "app/server.py", &SERVER.replace("timeout=30", "timeout=5"))],
            "Changes requested: a reviewer asked for changes",
            "Shows a reviewer who requested changes. In EE that blocks merging until they approve or are removed.",
            json!({"reviewer_ids": [dave]}))
        .await?;
    d.note("dave", p, iid, "Five seconds is too short for slow clients.\n\n/request_changes").await?;

    // 6. Labels, milestone, assignee and time tracking.
    let iid = d
        .mr("carol", p, "fix/health-version", &[("update", "app/routes.py", &ROUTES.replace("{\"status\": \"ok\"}", "{\"status\": \"ok\", \"version\": \"1.0\"}"))],
            "Metadata: labels, milestone, assignee and time tracking",
            "Shows the sidebar filled in: labels, a milestone, an assignee, a reviewer and time tracking.",
            json!({"labels": "bug,frontend", "milestone_id": milestone["id"], "assignee_ids": [d.user("carol")], "reviewer_ids": [bob]}))
        .await?;
    d.post("carol", &format!("/projects/{p}/merge_requests/{iid}/time_estimate"), json!({"duration": "3h"})).await?;
    d.post("carol", &format!("/projects/{p}/merge_requests/{iid}/add_spent_time"), json!({"duration": "1h30m"})).await?;

    // 7. Merged with squash.
    let iid = d
        .mr("erin", p, "docs/routes", &[("create", "docs/routes.md", "# Routes\n\n- `/`: the greeting\n- `/health`: the health check\n")],
            "Merged: squashed into one commit",
            "Shows a merged merge request: squashed, with the source branch deleted.",
            json!({"squash": true, "remove_source_branch": true}))
        .await?;
    d.commit("erin", p, "docs/routes", "Fix a typo", &[("update", "docs/routes.md", "# Routes\n\n- `/`: the greeting\n- `/health`: the health check, for load balancers\n")]).await?;
    d.put("bob", &format!("/projects/{p}/merge_requests/{iid}/merge"), json!({"squash": true, "should_remove_source_branch": true})).await?;

    // 8. Closed.
    let iid = d
        .mr("frank", p, "experiment/cache", &[("create", "app/cache.py", "CACHE = {}\n")],
            "Closed: closed without merging",
            "Shows a merge request that was closed instead of merged.",
            json!({}))
        .await?;
    d.note("frank", p, iid, "Closing: the framework caches responses already.").await?;
    d.put("frank", &format!("/projects/{p}/merge_requests/{iid}"), json!({"state_event": "close"})).await?;
    Ok(json!({"path": "demo/web-app", "merge_requests": 8}))
}

/// Fake CI pipelines in each state; in EE also merge trains.
async fn pipelines(d: &Demo, group: i64) -> R<Value> {
    let description = if d.ee { "Pipelines from labrat's fake CI, and merge trains. Merging needs a passed pipeline." } else { "Pipelines from labrat's fake CI. Merging needs a passed pipeline." };
    let main_go = "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"deploying\")\n}\n";
    let p = d.project("deploy-service", "Deploy Service", description, group, &[("main.go", main_go)]).await?;
    d.labrat(Method::PUT, &format!("/projects/{p}/ci"), json!({"enabled": true, "jobs": ["build", "test"], "runner": {"mode": "manual"}})).await?;
    let mut settings = json!({"only_allow_merge_if_pipeline_succeeds": true});
    if d.ee {
        settings = json!({"only_allow_merge_if_pipeline_succeeds": true, "merge_pipelines_enabled": true, "merge_trains_enabled": true, "merge_trains_skip_train_allowed": true});
    }
    d.put("root", &format!("/projects/{p}"), settings).await?;
    let kind = d.pipeline_ref();
    let step = |name: &str| format!("steps/{name}.sh");
    let script = |name: &str| format!("#!/bin/sh\necho \"{name}\"\n");

    let iid = d.mr("alice", p, "feature/health-check", &[("create", &step("health-check"), &script("health check"))],
        "Pipeline passed: build and test succeeded",
        "Shows a merge request whose pipeline passed.",
        json!({})).await?;
    d.run_jobs(p, iid, kind, Some("success")).await?;

    let iid = d.mr("carol", p, "feature/rollback", &[("create", &step("rollback"), &script("rollback"))],
        "Pipeline failed: it can't merge until a pipeline passes",
        "Shows a failed pipeline. This project only allows merging after a pipeline succeeds.",
        json!({})).await?;
    d.run_jobs(p, iid, kind, Some("failed")).await?;

    let iid = d.mr("dave", p, "feature/notify", &[("create", &step("notify"), &script("notify"))],
        "Pipeline running: the runner is working on it",
        "Shows a merge request whose pipeline is running. Finish its jobs in Settings › labrat test setup.",
        json!({})).await?;
    d.run_jobs(p, iid, kind, None).await?;

    d.mr("erin", p, "feature/metrics", &[("create", &step("metrics"), &script("metrics"))],
        "Pipeline pending: no runner picked it up yet",
        "Shows a merge request whose pipeline waits for a runner. Start its jobs in Settings › labrat test setup.",
        json!({})).await?;

    let mut count = 4;
    if d.ee {
        let iid = d.mr("alice", p, "feature/canary", &[("create", &step("canary"), &script("canary"))],
            "Merge train: merged by the train",
            "Shows a merge request that a merge train merged after its train pipeline passed.",
            json!({})).await?;
        d.run_jobs(p, iid, kind, Some("success")).await?;
        d.post("bob", &format!("/projects/{p}/merge_trains/merge_requests/{iid}"), json!({})).await?;
        d.run_jobs(p, iid, "train", Some("success")).await?;

        let first = d.mr("carol", p, "feature/blue-green", &[("create", &step("blue-green"), &script("blue-green"))],
            "Merge train: first in line, its train pipeline is running",
            "Shows a merge request at the front of a merge train. It merges when its train pipeline passes.",
            json!({})).await?;
        d.run_jobs(p, first, kind, Some("success")).await?;
        let second = d.mr("dave", p, "feature/smoke-tests", &[("create", &step("smoke-tests"), &script("smoke tests"))],
            "Merge train: second in line, waiting behind the first",
            "Shows a merge request behind another one on a merge train. Its train pipeline includes the changes ahead of it.",
            json!({})).await?;
        d.run_jobs(p, second, kind, Some("success")).await?;
        d.post("bob", &format!("/projects/{p}/merge_trains/merge_requests/{first}"), json!({})).await?;
        d.post("bob", &format!("/projects/{p}/merge_trains/merge_requests/{second}"), json!({})).await?;
        d.run_jobs(p, first, "train", None).await?;
        count += 3;
    }
    Ok(json!({"path": "demo/deploy-service", "merge_requests": count}))
}

const HANDLERS: &str = "def list_users(db):\n    return db.query(\"SELECT id, name FROM users\")\n\n\ndef get_user(db, user_id):\n    return db.query(\"SELECT id, name FROM users WHERE id = ?\", user_id)\n";

/// EE approvals: rules, code owners, approval resets and dependencies.
async fn approvals(d: &Demo, group: i64) -> R<Value> {
    let p = d
        .project("api-service", "API Service", "Approval rules, code owners and merge request dependencies.", group, &[
            ("api/handlers.py", HANDLERS),
            ("db/schema.sql", "CREATE TABLE users (\n    id INTEGER PRIMARY KEY,\n    name TEXT NOT NULL\n);\n"),
            ("docs/api.md", "# API\n\n- `GET /users`\n- `GET /users/:id`\n"),
        ])
        .await?;
    let ids = |names: &[&str]| names.iter().map(|n| d.user(n)).collect::<Vec<_>>();
    d.post("root", &format!("/projects/{p}/approval_rules"), json!({"name": "Backend", "approvals_required": 2, "user_ids": ids(&["carol", "dave", "erin"])})).await?;
    d.post("root", &format!("/projects/{p}/approval_rules"), json!({"name": "Security", "approvals_required": 1, "user_ids": ids(&["frank"])})).await?;
    d.labrat(Method::PUT, &format!("/projects/{p}/code_owners"), json!({"branch": "main", "sections": [{"name": null, "entries": [{"pattern": "/db/", "owners": ["@dave"]}]}]})).await?;
    d.call("root", Method::PATCH, &format!("/api/v4/projects/{p}/protected_branches/main"), json!({"code_owner_approval_required": true})).await?;
    let handlers = |extra: &str| format!("{HANDLERS}\n\n{extra}");

    let iid = d.mr("alice", p, "feature/delete-user", &[("update", "api/handlers.py", &handlers("def delete_user(db, user_id):\n    db.execute(\"DELETE FROM users WHERE id = ?\", user_id)\n"))],
        "Fully approved: every approval rule is satisfied",
        "Shows approvals from several people: Backend needs 2 of Carol, Dave and Erin, and Security needs Frank. Open \"Show approval rules\" in the merge widget.",
        json!({})).await?;
    d.approve(&["carol", "erin", "frank"], p, iid).await?;

    let iid = d.mr("alice", p, "feature/rename-user", &[("update", "api/handlers.py", &handlers("def rename_user(db, user_id, name):\n    db.execute(\"UPDATE users SET name = ? WHERE id = ?\", name, user_id)\n"))],
        "Needs approvals: Backend has 1 of 2, Security has none",
        "Shows a merge request that still needs approvals: one more from Backend and one from Security.",
        json!({})).await?;
    d.approve(&["carol"], p, iid).await?;

    let iid = d.mr("alice", p, "db/add-email", &[("update", "db/schema.sql", "CREATE TABLE users (\n    id INTEGER PRIMARY KEY,\n    name TEXT NOT NULL,\n    email TEXT\n);\n")],
        "Code owners: db/ changes need Dave's approval",
        "Shows a code owner rule: Dave owns db/, and main requires code owner approval. The other rules are satisfied.",
        json!({})).await?;
    d.approve(&["carol", "erin", "frank"], p, iid).await?;

    let iid = d.mr("frank", p, "feature/count-users", &[("update", "api/handlers.py", &handlers("def count_users(db):\n    return db.query(\"SELECT COUNT(*) FROM users\")\n"))],
        "Approvals reset: a new commit removed the approvals",
        "Shows approvals removed by a push: Carol and Dave approved, then a new commit reset their approvals.",
        json!({})).await?;
    d.approve(&["carol", "dave"], p, iid).await?;
    d.commit("frank", p, "feature/count-users", "Name the count column", &[("update", "api/handlers.py", &handlers("def count_users(db):\n    return db.query(\"SELECT COUNT(*) AS count FROM users\")\n"))]).await?;

    let blocker = d.mr("dave", p, "docs/delete-user", &[("update", "docs/api.md", "# API\n\n- `GET /users`\n- `GET /users/:id`\n- `DELETE /users/:id`\n")],
        "Dependency: another merge request waits for this one",
        "Shows a merge request that blocks another one: the next merge request can't merge before this one.",
        json!({})).await?;
    let blocked = d.mr("alice", p, "feature/delete-endpoint", &[("update", "api/handlers.py", &handlers("def delete_endpoint(db, request):\n    return delete_user(db, request.args[\"id\"])\n"))],
        &format!("Blocked: waits for !{blocker} to merge first"),
        &format!("Shows a merge request dependency: it's blocked until !{blocker} merges. See Dependencies in the sidebar."),
        json!({})).await?;
    d.post("alice", &format!("/projects/{p}/merge_requests/{blocked}/blocks"), json!({"blocking_merge_request_iid": blocker})).await?;
    Ok(json!({"path": "demo/api-service", "merge_requests": 6}))
}

/// EE approval policies, evaluated by labrat without scans (see `services::policies`).
async fn policies(d: &Demo, group: i64) -> R<Value> {
    let refunds = "def refund(db, payment_id):\n    return db.query(\"SELECT * FROM payments WHERE id = ?\", payment_id)\n";
    let p = d.project("payments", "Payments", "Approval policies: a scan finding that needs approval, and a warn-mode policy.", group, &[("payments/refunds.py", refunds)]).await?;
    let policies = json!([
        {
            "name": "Security review",
            "description": "Every merge request should get a maintainer's approval. Warn mode: the violation can be dismissed.",
            "enabled": true,
            "enforcement_type": "warn",
            "rules": [{"type": "any_merge_request", "branch_type": "protected", "commits": "any"}],
            "actions": [{"type": "require_approval", "approvals_required": 1, "role_approvers": ["maintainer"]}]
        },
        {
            "name": "No critical vulnerabilities",
            "description": "New critical findings need a maintainer's approval.",
            "enabled": true,
            "rules": [{"type": "scan_finding", "branch_type": "protected", "scanners": ["sast"], "vulnerabilities_allowed": 0, "severity_levels": ["critical"], "vulnerability_states": ["new_needs_triage"]}],
            "actions": [{"type": "require_approval", "approvals_required": 1, "role_approvers": ["maintainer"]}],
            "fallback_behavior": {"fail": "open"}
        }
    ]);
    d.labrat(Method::PUT, &format!("/projects/{p}/approval_policies"), json!({"approval_policy": policies})).await?;
    let finding = |path: &str| {
        json!({"policies": {"No critical vulnerabilities": {"violated": true, "status": "FAILED", "newScanFinding": [
            {"name": "SQL injection in the refund lookup", "path": path, "reportType": "SAST", "severity": "CRITICAL", "location": {}}
        ]}}})
    };

    let iid = d.mr("alice", p, "feature/refund-by-reference", &[("update", "payments/refunds.py", &format!("{refunds}\n\ndef refund_by_reference(db, reference):\n    return db.query(\"SELECT * FROM payments WHERE reference = '\" + reference + \"'\")\n"))],
        "Policy violation: a new critical vulnerability",
        "Shows an approval policy violated by a scan finding: a maintainer has to approve. The findings are set through labrat's test setup.",
        json!({})).await?;
    d.labrat(Method::PUT, &format!("/projects/{p}/merge_requests/{iid}/policy_results"), finding("payments/refunds.py")).await?;

    d.mr("carol", p, "feature/refund-log", &[("update", "payments/refunds.py", &format!("{refunds}\n\ndef log_refund(log, payment_id):\n    log.info(\"refund %s\", payment_id)\n"))],
        "Warn-mode policy: no findings, only a warning",
        "Shows a warn-mode policy: it asks for a maintainer's approval but can be dismissed instead.",
        json!({})).await?;

    let iid = d.mr("dave", p, "feature/refund-limit", &[("update", "payments/refunds.py", &format!("{refunds}\n\ndef refund_limit(amount):\n    return eval(\"min(\" + amount + \", 1000)\")\n"))],
        "Policy approved: a maintainer approved the finding",
        "Shows a policy violation that Bob, a maintainer, approved.",
        json!({})).await?;
    d.labrat(Method::PUT, &format!("/projects/{p}/merge_requests/{iid}/policy_results"), finding("payments/refunds.py")).await?;
    d.approve(&["bob"], p, iid).await?;
    Ok(json!({"path": "demo/payments", "merge_requests": 3}))
}
