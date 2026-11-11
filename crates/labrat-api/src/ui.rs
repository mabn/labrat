//! A plain server-rendered web UI at GitLab-like URLs, so `web_url`s from the API open real pages.
//! It covers signing in, groups and projects, the repository, merge requests (activity, diffs
//! with inline comments, approvals, merging) and project settings. Every action goes through
//! the same services as the REST API.

use crate::markdown;
use crate::params::parse_query;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use labrat_core::model::{Namespace, Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::comments::{self, CreateParams};
use labrat_core::services::hooks::Hook;
use labrat_core::services::merge_requests::{self as mrs, MergeRequest, merge, mergeability};
use labrat_core::services::notes::Note;
use labrat_core::services::positions::Position;
use labrat_core::services::{emoji, members, projects, reviews, users};
use labrat_core::{App, repo, time};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

const COOKIE: &str = "_labrat_session";

/// Signed-in sessions: cookie value -> (app, user id). Sessions do not survive a restart.
static SESSIONS: LazyLock<Mutex<HashMap<String, (usize, i64)>>> = LazyLock::new(Default::default);

fn app_key(app: &Arc<App>) -> usize {
    Arc::as_ptr(app) as usize
}

struct Ui {
    app: Arc<App>,
    user: Option<User>,
    form: Map<String, Value>,
    query: Map<String, Value>,
}

impl Ui {
    fn f(&self, k: &str) -> String {
        self.form.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
    }
    fn q(&self, k: &str) -> Option<String> {
        self.query.get(k).and_then(Value::as_str).map(str::to_string)
    }
    fn base(&self) -> &str {
        self.app.config.external_url.trim_end_matches('/')
    }
}

fn redirect(to: &str) -> Response {
    (StatusCode::SEE_OTHER, [(header::LOCATION, to.to_string())]).into_response()
}

fn not_found() -> Response {
    let mut r = html_resp("Not found", None, html! { h1 { "404: Page not found" } });
    *r.status_mut() = StatusCode::NOT_FOUND;
    r
}

const CSS: &str = r#"
body{font-family:system-ui,sans-serif;margin:0;color:#222;background:#fafafa}
header{background:#292961;color:#fff;padding:8px 16px;display:flex;gap:16px;align-items:center}
header a{color:#fff;text-decoration:none}main{max-width:1100px;margin:16px auto;padding:0 16px}
a{color:#1f6fd0}table{border-collapse:collapse;width:100%}td,th{padding:4px 8px;border-bottom:1px solid #ddd;text-align:left}
.box{background:#fff;border:1px solid #ddd;border-radius:4px;padding:12px;margin:12px 0}
.muted{color:#777;font-size:90%}.badge{display:inline-block;padding:1px 6px;border-radius:8px;background:#eee;font-size:85%}
.opened{background:#d8f5dd}.merged{background:#e2dcf7}.closed{background:#f7dcdc}
.diff{font-family:monospace;font-size:13px;white-space:pre}.add{background:#e6ffec}.del{background:#ffebe9}
.ln{color:#999;width:40px;text-align:right;user-select:none}.note{border-left:3px solid #ccc;padding:4px 8px;margin:6px 0;background:#fff}
.system{color:#666;font-size:90%}textarea{width:100%;min-height:60px}input[type=text],input[type=password],select{padding:4px}
form.inline{display:inline}nav.tabs a{margin-right:12px}button{cursor:pointer}
"#;

fn page(title: &str, user: Option<&User>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                title { (title) " · labrat" }
                style { (PreEscaped(CSS)) }
            }
            body {
                header {
                    a href="/" { b { "labrat" } }
                    @if let Some(u) = user {
                        a href="/projects/new" { "New project" }
                        a href="/groups/new" { "New group" }
                        a href="/-/labrat/requests" { "Requests" }
                        @if u.admin { a href="/admin/users" { "Admin" } }
                        span style="flex:1" {}
                        span { "@" (u.username) }
                        form.inline method="post" action="/users/sign_out" { button { "Sign out" } }
                    } @else {
                        span style="flex:1" {}
                        a href="/users/sign_in" { "Sign in" }
                    }
                }
                main { (body) }
            }
        }
    }
}

fn html_resp(title: &str, user: Option<&User>, body: Markup) -> Response {
    let mut r = page(title, user, body).into_string().into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, "text/html; charset=utf-8".parse().unwrap());
    r
}

fn session_user(app: &Arc<App>, headers: &HeaderMap) -> Option<User> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    let sid = cookie.split(';').map(str::trim).find_map(|c| c.strip_prefix(&format!("{COOKIE}=")))?;
    let (key, uid) = *SESSIONS.lock().get(sid)?;
    if key != app_key(app) {
        return None;
    }
    app.db.read(|c| User::find(c, uid)).ok().flatten().filter(|u| u.state == "active")
}

/// Entry point from the router's fallback for non-API paths.
pub async fn handle(app: Arc<App>, method: Method, path: &str, query: &str, headers: HeaderMap, body: &[u8]) -> Response {
    let mut form = Map::new();
    if method == Method::POST {
        parse_query(&String::from_utf8_lossy(body), &mut form);
    }
    let mut q = Map::new();
    parse_query(query, &mut q);
    let user = session_user(&app, &headers);
    let ui = Ui { app, user, form, query: q };
    let post = method == Method::POST;
    match path {
        "/users/sign_in" if post => return sign_in(&ui),
        "/users/sign_in" => return sign_in_page(&ui, None),
        "/users/sign_out" => {
            if let Some(sid) = headers.get(header::COOKIE).and_then(|c| c.to_str().ok()).and_then(|c| {
                c.split(';').map(str::trim).find_map(|x| x.strip_prefix(&format!("{COOKIE}=")).map(str::to_string))
            }) {
                SESSIONS.lock().remove(&sid);
            }
            return redirect("/users/sign_in");
        }
        _ => {}
    }
    let Some(user) = ui.user.clone() else {
        if method == Method::GET || method == Method::HEAD {
            // Public pages are readable anonymously; everything else asks to sign in.
            if path == "/" {
                return redirect("/users/sign_in");
            }
        } else {
            return redirect("/users/sign_in");
        }
        return route(&ui, None, post, path);
    };
    route(&ui, Some(&user), post, path)
}

fn route(ui: &Ui, user: Option<&User>, post: bool, path: &str) -> Response {
    match (path, post, user) {
        ("/", _, Some(u)) => return dashboard(ui, u),
        ("/projects/new", false, Some(_)) => return new_project_page(ui, None),
        ("/projects/new", true, Some(u)) => return create_project(ui, u),
        ("/groups/new", false, Some(_)) => return new_group_page(ui, None),
        ("/groups/new", true, Some(u)) => return create_group(ui, u),
        ("/-/labrat/requests", false, Some(u)) => return requests_page(ui, u),
        ("/admin/users", false, Some(u)) if u.admin => return admin_users(ui, u, None),
        ("/admin/users", true, Some(u)) if u.admin => return admin_create_user(ui, u),
        _ => {}
    }
    if let Some(resp) = upload_file(ui, user, path) {
        return resp;
    }
    let trimmed = path.trim_matches('/');
    let (full, rest) = match trimmed.split_once("/-/") {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (trimmed.to_string(), String::new()),
    };
    let found = ui.app.db.read(|c| {
        let p = Project::find_by_full_path(c, &full)?;
        let ns = if p.is_none() { Namespace::find_by_full_path(c, &full)? } else { None };
        Ok::<_, labrat_core::Error>((p, ns))
    });
    match found {
        Ok((Some(p), _)) => project_route(ui, user, post, p, &rest),
        Ok((None, Some(ns))) if rest.is_empty() => group_page(ui, user, ns),
        _ => not_found(),
    }
}

// ---------------------------------------------------------------------------------------------
// Sign in, dashboard, admin

fn sign_in_page(ui: &Ui, error: Option<&str>) -> Response {
    html_resp(
        "Sign in",
        None,
        html! {
            div.box style="max-width:360px;margin:40px auto" {
                h2 { "Sign in to labrat" }
                @if let Some(e) = error { p style="color:#b00" { (e) } }
                form method="post" action="/users/sign_in" {
                    p { label { "Username or email" br; input type="text" name="login" value=(ui.f("login")); } }
                    p { label { "Password" br; input type="password" name="password"; } }
                    button { "Sign in" }
                }
                p.muted { "The root user's password is set with --root-password." }
            }
        },
    )
}

fn sign_in(ui: &Ui) -> Response {
    let login = ui.f("login");
    let user = ui.app.db.read(|c| {
        let u = match User::find_by_username(c, &login)? {
            Some(u) => Some(u),
            None => {
                let id: Option<i64> = c
                    .query_row("SELECT id FROM users WHERE lower(email) = lower(?1)", [&login], |r| r.get(0))
                    .ok();
                id.map(|i| User::find(c, i)).transpose()?.flatten()
            }
        };
        Ok::<_, labrat_core::Error>(u)
    });
    match user {
        Ok(Some(u)) if u.state == "active" && users::check_password(&u, &ui.f("password")) => {
            let sid = hex::encode(rand::random::<[u8; 20]>());
            SESSIONS.lock().insert(sid.clone(), (app_key(&ui.app), u.id));
            let mut r = redirect("/");
            r.headers_mut().insert(header::SET_COOKIE, format!("{COOKIE}={sid}; Path=/; HttpOnly; SameSite=Lax").parse().unwrap());
            r
        }
        _ => sign_in_page(ui, Some("Invalid login or password.")),
    }
}

fn visible_projects(c: &Connection, user: Option<&User>) -> labrat_core::Result<Vec<Project>> {
    let ids: Vec<i64> = {
        let mut st = c.prepare("SELECT id FROM projects ORDER BY id DESC")?;
        st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let mut out = vec![];
    for id in ids {
        if let Some(p) = Project::find(c, id)?
            && ProjectPerms::load(c, user, &p)?.read_project() {
                out.push(p);
            }
    }
    Ok(out)
}

fn mr_row(ui: &Ui, p: &Project, mr: &MergeRequest) -> Markup {
    html! {
        tr {
            td { a href=(format!("/{}/-/merge_requests/{}", p.full_path, mr.iid)) { (mr.title) } br; span.muted { (p.full_path) " !" (mr.iid) " · " (mr.source_branch) " → " (mr.target_branch) } }
            td { span class=(format!("badge {}", mr.state)) { (mr.state) } }
            td.muted { (time::iso(mr.updated_at)) }
        }
        @if false { (ui.base()) }
    }
}

fn dashboard(ui: &Ui, u: &User) -> Response {
    let data = ui.app.db.read(|c| {
        let projects = visible_projects(c, Some(u))?;
        let mine = MergeRequest::list_where(
            c,
            "state = 'opened' AND (author_id = ?1 OR id IN (SELECT merge_request_id FROM merge_request_assignees WHERE user_id = ?1)
               OR id IN (SELECT merge_request_id FROM merge_request_reviewers WHERE user_id = ?1)) ORDER BY updated_at DESC",
            [u.id],
        )?;
        let mut rows = vec![];
        for m in mine {
            if let Some(p) = Project::find(c, m.target_project_id)? {
                rows.push((p, m));
            }
        }
        Ok::<_, labrat_core::Error>((projects, rows))
    });
    let Ok((projects, mrs)) = data else { return not_found() };
    html_resp(
        "Dashboard",
        Some(u),
        html! {
            h2 { "Your merge requests" }
            div.box { @if mrs.is_empty() { p.muted { "Nothing assigned, authored or waiting for your review." } } @else { table { @for (p, m) in &mrs { (mr_row(ui, p, m)) } } } }
            h2 { "Projects" }
            div.box { table { @for p in &projects { tr { td { a href=(format!("/{}", p.full_path)) { (p.full_path) } } td.muted { (p.description.clone().unwrap_or_default()) } } } } }
        },
    )
}

fn requests_page(ui: &Ui, u: &User) -> Response {
    let list = crate::request_log::entries(&ui.app);
    let filter = ui.q("q").unwrap_or_default();
    html_resp(
        "Requests",
        Some(u),
        html! {
            h2 { "API requests" }
            form method="get" { input type="text" name="q" value=(filter) placeholder="filter by path"; " " button { "Filter" } }
            p.muted { "The newest " (list.len()) " calls to /api/v4 and /api/graphql, newest first. Also available as JSON at GET /-/labrat/requests.json." }
            @for e in list.iter().filter(|e| filter.is_empty() || e.uri.contains(&filter)) {
                details.box {
                    summary {
                        code { (e.method) " " (e.uri) } " → "
                        b style=(if e.status >= 400 { "color:#b00" } else { "" }) { (e.status) }
                        span.muted { " · " (e.user.clone().unwrap_or("anonymous".into())) " · " (format!("{:.1} ms", e.duration_ms)) " · " (time::iso(e.at)) }
                    }
                    p.muted { "Request headers" }
                    div.diff { @for (k, v) in &e.request_headers { (k) ": " (v) "\n" } }
                    @if !e.request_body.is_empty() { p.muted { "Request body" } div.diff { (e.request_body) } }
                    p.muted { "Response body" }
                    div.diff style="white-space:pre-wrap" { (e.response_body) }
                }
            }
        },
    )
}

fn admin_users(ui: &Ui, u: &User, error: Option<String>) -> Response {
    let list = ui.app.db.read(|c| {
        let mut st = c.prepare("SELECT id FROM users ORDER BY id")?;
        let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok::<_, labrat_core::Error>(ids.into_iter().filter_map(|i| User::find(c, i).ok().flatten()).collect::<Vec<_>>())
    });
    let users = list.unwrap_or_default();
    html_resp(
        "Users",
        Some(u),
        html! {
            h2 { "Users" }
            div.box { table { tr { th { "Username" } th { "Name" } th { "Email" } th { "State" } th { "Admin" } }
                @for x in &users { tr { td { (x.username) } td { (x.name) } td { (x.email) } td { (x.state) } td { @if x.admin { "yes" } } } } } }
            h3 { "New user" }
            @if let Some(e) = error { p style="color:#b00" { (e) } }
            form.box method="post" action="/admin/users" {
                p { input type="text" name="username" placeholder="username"; " " input type="text" name="name" placeholder="Name"; " " input type="text" name="email" placeholder="email"; " " input type="password" name="password" placeholder="password"; }
                p { label { input type="checkbox" name="admin" value="1"; " Admin" } }
                button { "Create user" }
            }
        },
    )
}

fn admin_create_user(ui: &Ui, u: &User) -> Response {
    let res = ui.app.db.tx(|c| {
        users::create(
            c,
            Some(u),
            users::NewUser {
                username: ui.f("username"),
                name: ui.f("name"),
                email: ui.f("email"),
                password: Some(ui.f("password")).filter(|p| !p.is_empty()),
                admin: ui.f("admin") == "1",
                skip_confirmation: true,
                ..Default::default()
            },
        )
    });
    match res {
        Ok(_) => redirect("/admin/users"),
        Err(e) => admin_users(ui, u, Some(error_text(&e))),
    }
}

fn error_text(e: &labrat_core::Error) -> String {
    match e {
        labrat_core::Error::Validation(list) => list.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "),
        labrat_core::Error::Api(_, v) => v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()),
        labrat_core::Error::Param(m) => m.clone(),
        other => other.to_string(),
    }
}

fn new_group_page(ui: &Ui, error: Option<String>) -> Response {
    html_resp(
        "New group",
        ui.user.as_ref(),
        html! {
            h2 { "New group" }
            @if let Some(e) = error { p style="color:#b00" { (e) } }
            form.box method="post" action="/groups/new" {
                p { label { "Name " input type="text" name="name" value=(ui.f("name")); } }
                p { label { "Path " input type="text" name="path" value=(ui.f("path")); } }
                p { label { "Visibility " select name="visibility" { option value="0" { "private" } option value="10" { "internal" } option value="20" { "public" } } } }
                button { "Create group" }
            }
        },
    )
}

fn create_group(ui: &Ui, u: &User) -> Response {
    let res = ui.app.db.tx(|c| {
        labrat_core::services::groups::create(
            c,
            u,
            labrat_core::services::groups::NewGroup {
                name: ui.f("name"),
                path: ui.f("path"),
                visibility: ui.f("visibility").parse().ok(),
                ..Default::default()
            },
        )
    });
    match res {
        Ok(g) => redirect(&format!("/{}", g.full_path)),
        Err(e) => new_group_page(ui, Some(error_text(&e))),
    }
}

fn new_project_page(ui: &Ui, error: Option<String>) -> Response {
    let u = ui.user.clone();
    let namespaces = ui.app.db.read(|c| {
        let mut st = c.prepare("SELECT id FROM namespaces ORDER BY full_path")?;
        let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let mut out = vec![];
        for id in ids {
            if let Some(ns) = Namespace::find(c, id)?
                && let Some(user) = &u
                    && projects::can_create_in(c, user, &ns)? {
                        out.push(ns);
                    }
        }
        Ok::<_, labrat_core::Error>(out)
    });
    let namespaces = namespaces.unwrap_or_default();
    html_resp(
        "New project",
        ui.user.as_ref(),
        html! {
            h2 { "New project" }
            @if let Some(e) = error { p style="color:#b00" { (e) } }
            form.box method="post" action="/projects/new" {
                p { label { "Namespace " select name="namespace_id" { @for ns in &namespaces { option value=(ns.id) { (ns.full_path) } } } } }
                p { label { "Name " input type="text" name="name" value=(ui.f("name")); } }
                p { label { "Visibility " select name="visibility" { option value="0" { "private" } option value="10" { "internal" } option value="20" { "public" } } } }
                p { label { input type="checkbox" name="readme" value="1" checked; " Initialize with a README" } }
                button { "Create project" }
            }
        },
    )
}

fn create_project(ui: &Ui, u: &User) -> Response {
    let res = ui.app.db.tx(|c| {
        let ns = Namespace::find(c, ui.f("namespace_id").parse().unwrap_or(0))?.ok_or(labrat_core::Error::not_found("Namespace"))?;
        let name = ui.f("name");
        let path = name.trim().to_lowercase().replace(char::is_whitespace, "-");
        let p = projects::create(
            c,
            u,
            &ns,
            projects::ProjectAttrs { name: Some(name), path: Some(path), visibility: ui.f("visibility").parse().ok(), ..Default::default() },
        )?;
        if ui.f("readme") == "1" {
            crate::routes::repo_init::initialize_with_readme(c, &ui.app.config, &p, u, &repo::default_branch(&p))?;
        }
        Ok::<_, labrat_core::Error>(p)
    });
    match res {
        Ok(p) => redirect(&format!("/{}", p.full_path)),
        Err(e) => new_project_page(ui, Some(error_text(&e))),
    }
}

fn group_page(ui: &Ui, user: Option<&User>, ns: Namespace) -> Response {
    let list = ui.app.db.read(|c| {
        let all = visible_projects(c, user)?;
        Ok::<_, labrat_core::Error>(all.into_iter().filter(|p| p.namespace_id == ns.id).collect::<Vec<_>>())
    });
    let list = list.unwrap_or_default();
    html_resp(
        &ns.full_path,
        user,
        html! {
            h2 { (ns.full_name) }
            p.muted { (ns.description.clone().unwrap_or_default()) }
            div.box { table { @for p in &list { tr { td { a href=(format!("/{}", p.full_path)) { (p.name) } } td.muted { (p.description.clone().unwrap_or_default()) } } } } }
        },
    )
}

// ---------------------------------------------------------------------------------------------
// Projects

fn project_nav(p: &Project, perms: &ProjectPerms) -> Markup {
    let base = format!("/{}", p.full_path);
    html! {
        h2 { a href=(base) { (p.full_name) } }
        nav.tabs {
            a href=(format!("{base}/-/tree/{}", repo::default_branch(p))) { "Files" }
            a href=(format!("{base}/-/commits/{}", repo::default_branch(p))) { "Commits" }
            a href=(format!("{base}/-/branches")) { "Branches" }
            a href=(format!("{base}/-/merge_requests")) { "Merge requests" }
            @if perms.admin_project() {
                a href=(format!("{base}/-/settings")) { "Settings" }
            }
        }
    }
}

fn project_route(ui: &Ui, user: Option<&User>, post: bool, p: Project, rest: &str) -> Response {
    let perms = match ui.app.db.read(|c| ProjectPerms::load(c, user, &p)) {
        Ok(x) if x.read_project() => x,
        _ => return if user.is_none() { redirect("/users/sign_in") } else { not_found() },
    };
    let parts: Vec<&str> = rest.split('/').collect();
    match (parts.as_slice(), post) {
        ([""], false) => project_page(ui, user, &p, &perms),
        (["tree", r @ ..], false) => tree_page(ui, user, &p, &perms, r),
        (["blob", r @ ..], false) => blob_page(ui, user, &p, &perms, r),
        (["commits", r @ ..], false) => commits_page(ui, user, &p, &perms, &r.join("/")),
        (["branches"], false) => branches_page(ui, user, &p, &perms),
        (["merge_requests"], false) => mr_list(ui, user, &p, &perms),
        (["merge_requests", "new"], false) => new_mr_page(ui, user, &p, &perms, None),
        (["merge_requests", "new"], true) => create_mr(ui, user, &p, &perms),
        (["merge_requests", iid], false) => mr_page(ui, user, &p, &perms, iid, false),
        (["merge_requests", iid, "diffs"], false) => mr_page(ui, user, &p, &perms, iid, true),
        (["merge_requests", iid, action], true) => mr_action(ui, user, &p, iid, action),
        (["settings"], false) if perms.admin_project() => settings_page(ui, user, &p, &perms, None),
        (["settings", action], true) if perms.admin_project() => settings_action(ui, user, &p, &perms, action),
        _ => not_found(),
    }
}

fn readme(c: &Connection, p: &Project) -> labrat_core::Result<Option<String>> {
    let r = repo::repo(c, p);
    let Some(head) = r.branch(&repo::default_branch(p))? else { return Ok(None) };
    let tree = r.commit(&head)?.tree;
    for name in ["README.md", "README", "readme.md"] {
        if let Some(e) = r.entry_at(&tree, name)? {
            return Ok(Some(String::from_utf8_lossy(&r.blob(&e.oid)?).into_owned()));
        }
    }
    Ok(None)
}

fn project_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms) -> Response {
    let data = ui.app.db.read(|c| {
        let open: i64 = c.query_row("SELECT count(*) FROM merge_requests WHERE target_project_id = ?1 AND state = 'opened'", [p.id], |r| r.get(0))?;
        Ok::<_, labrat_core::Error>((readme(c, p)?, open))
    });
    let (readme_text, open) = data.unwrap_or((None, 0));
    html_resp(
        &p.full_path,
        user,
        html! {
            (project_nav(p, perms))
            div.box {
                p { (p.description.clone().unwrap_or_default()) }
                p.muted { "Clone: " code { (ui.base()) "/" (p.full_path) ".git" } " · " (open) " open merge requests" }
            }
            @if let Some(t) = readme_text { div.box { (PreEscaped(markdown::render(&t))) } }
        },
    )
}

fn split_ref<'a>(c: &Connection, p: &Project, parts: &'a [&'a str]) -> labrat_core::Result<Option<(String, String)>> {
    let r = repo::repo(c, p);
    for i in (1..=parts.len()).rev() {
        let candidate = parts[..i].join("/");
        if r.resolve(&candidate)?.is_some() {
            return Ok(Some((candidate, parts[i..].join("/"))));
        }
    }
    Ok(None)
}

fn tree_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms, parts: &[&str]) -> Response {
    let data = ui.app.db.read(|c| {
        let Some((rf, path)) = split_ref(c, p, parts)? else { return Ok(None) };
        let r = repo::repo(c, p);
        let commit = r.resolve(&rf)?.expect("resolved");
        let tree = r.commit(&commit)?.tree;
        let dir = if path.is_empty() {
            Some(tree)
        } else {
            r.entry_at(&tree, &path)?.filter(|e| e.is_tree()).map(|e| e.oid)
        };
        let Some(dir) = dir else { return Ok(None) };
        let mut entries: Vec<(bool, String)> = r.tree(&dir)?.into_iter().map(|e| (e.is_tree(), e.name)).collect();
        entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        Ok::<_, labrat_core::Error>(Some((rf, path, entries)))
    });
    let Ok(Some((rf, path, entries))) = data else { return not_found() };
    let base = format!("/{}", p.full_path);
    let join = |n: &str| if path.is_empty() { n.to_string() } else { format!("{path}/{n}") };
    html_resp(
        &p.full_path,
        user,
        html! {
            (project_nav(p, perms))
            p { b { (rf) } " / " (path) }
            div.box { table {
                @for (is_dir, name) in &entries {
                    tr { td {
                        @if *is_dir { "📁 " a href=(format!("{base}/-/tree/{rf}/{}", join(name))) { (name) } }
                        @else { "📄 " a href=(format!("{base}/-/blob/{rf}/{}", join(name))) { (name) } }
                    } }
                }
            } }
        },
    )
}

fn blob_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms, parts: &[&str]) -> Response {
    let data = ui.app.db.read(|c| {
        let Some((rf, path)) = split_ref(c, p, parts)? else { return Ok(None) };
        let r = repo::repo(c, p);
        let commit = r.resolve(&rf)?.expect("resolved");
        let tree = r.commit(&commit)?.tree;
        let Some(e) = r.entry_at(&tree, &path)? else { return Ok(None) };
        Ok::<_, labrat_core::Error>(Some((rf, path, r.blob(&e.oid)?)))
    });
    let Ok(Some((rf, path, data))) = data else { return not_found() };
    let binary = labrat_git::diff::is_binary(&data);
    html_resp(
        &path,
        user,
        html! {
            (project_nav(p, perms))
            p { b { (rf) } " / " (path) }
            div.box.diff { @if binary { "Binary file (" (data.len()) " bytes)" } @else { (String::from_utf8_lossy(&data)) } }
        },
    )
}

fn commits_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms, rf: &str) -> Response {
    let data = ui.app.db.read(|c| {
        let r = repo::repo(c, p);
        let Some(head) = r.resolve(rf)? else { return Ok(None) };
        let mut list = vec![];
        for oid in labrat_git::walk::rev_list(&r, &[head], &[])?.into_iter().take(100) {
            let cm = r.commit(&oid)?;
            list.push((oid.hex(), cm.message.lines().next().unwrap_or("").to_string(), cm.author.name.clone()));
        }
        Ok::<_, labrat_core::Error>(Some(list))
    });
    let Ok(Some(list)) = data else { return not_found() };
    html_resp(
        &p.full_path,
        user,
        html! {
            (project_nav(p, perms))
            h3 { "Commits on " (rf) }
            div.box { table { @for (sha, title, author) in &list { tr { td { code { (&sha[..8]) } } td { (title) } td.muted { (author) } } } } }
        },
    )
}

fn branches_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms) -> Response {
    let data = ui.app.db.read(|c| {
        let r = repo::repo(c, p);
        let mut out = vec![];
        for (name, oid) in r.branches()? {
            out.push((name.clone(), oid.hex(), repo::is_protected(c, p, &name)?));
        }
        Ok::<_, labrat_core::Error>(out)
    });
    let list = data.unwrap_or_default();
    let base = format!("/{}", p.full_path);
    html_resp(
        &p.full_path,
        user,
        html! {
            (project_nav(p, perms))
            div.box { table {
                @for (name, sha, protected) in &list {
                    tr {
                        td { a href=(format!("{base}/-/tree/{name}")) { (name) } @if *protected { " " span.badge { "protected" } } }
                        td { code { (&sha[..8]) } }
                        td { a href=(format!("{base}/-/merge_requests/new?source_branch={name}")) { "Create merge request" } }
                    }
                }
            } }
        },
    )
}

// ---------------------------------------------------------------------------------------------
// Merge requests

fn mr_list(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms) -> Response {
    let state = ui.q("state").unwrap_or("opened".into());
    let list = ui.app.db.read(|c| {
        let all = MergeRequest::list_where(c, "target_project_id = ?1 ORDER BY created_at DESC, id DESC", [p.id])?;
        Ok::<_, labrat_core::Error>(all.into_iter().filter(|m| state == "all" || m.state == state).collect::<Vec<_>>())
    });
    let list = list.unwrap_or_default();
    let base = format!("/{}/-/merge_requests", p.full_path);
    html_resp(
        "Merge requests",
        user,
        html! {
            (project_nav(p, perms))
            p {
                @for s in ["opened", "merged", "closed", "all"] {
                    a href=(format!("{base}?state={s}")) { @if s == state { b { (s) } } @else { (s) } } " "
                }
                @if user.is_some() { " · " a href=(format!("{base}/new")) { "New merge request" } }
            }
            div.box { @if list.is_empty() { p.muted { "No merge requests." } } table { @for m in &list { (mr_row(ui, p, m)) } } }
        },
    )
}

fn new_mr_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms, error: Option<String>) -> Response {
    let branches: Vec<String> = ui.app.db.read(|c| Ok::<_, labrat_core::Error>(repo::repo(c, p).branches()?.into_iter().map(|b| b.0).collect())).unwrap_or_default();
    let src = ui.q("source_branch").unwrap_or(ui.f("source_branch"));
    let default = repo::default_branch(p);
    html_resp(
        "New merge request",
        user,
        html! {
            (project_nav(p, perms))
            h3 { "New merge request" }
            @if let Some(e) = error { p style="color:#b00" { (e) } }
            form.box method="post" action=(format!("/{}/-/merge_requests/new", p.full_path)) {
                p { "From " select name="source_branch" { @for b in &branches { option value=(b) selected[*b == src] { (b) } } }
                    " into " select name="target_branch" { @for b in &branches { option value=(b) selected[*b == default] { (b) } } } }
                p { input type="text" name="title" placeholder="Title" style="width:100%" value=(ui.f("title")); }
                p { textarea name="description" placeholder="Description" { (ui.f("description")) } }
                button { "Create merge request" }
            }
        },
    )
}

fn create_mr(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms) -> Response {
    let Some(u) = user else { return redirect("/users/sign_in") };
    let res = ui.app.db.tx(|c| {
        mrs::create(
            c,
            &ui.app.config,
            p,
            u,
            mrs::MrParams {
                title: Some(ui.f("title")),
                description: Some(Some(ui.f("description")).filter(|d| !d.is_empty())),
                source_branch: Some(ui.f("source_branch")),
                target_branch: Some(ui.f("target_branch")),
                ..Default::default()
            },
        )
    });
    crate::jobs::kick(&ui.app);
    match res {
        Ok(mr) => redirect(&format!("/{}/-/merge_requests/{}", p.full_path, mr.iid)),
        Err(e) => new_mr_page(ui, user, p, perms, Some(error_text(&e))),
    }
}

struct MrView {
    mr: MergeRequest,
    author: String,
    approvers: Vec<String>,
    can_approve: bool,
    approved_by_me: bool,
    can_merge: bool,
    mergeable: bool,
    detailed: String,
    discussions: Vec<Vec<(Note, String)>>,
    diff: Vec<FileDiff>,
    awards: Vec<(String, usize, bool)>,
    commits: Vec<(String, String)>,
    refs: (String, String, String),
    drafts: Vec<labrat_core::services::reviews::Draft>,
    suggestions: HashMap<i64, Vec<(i64, bool, Option<String>)>>,
}

struct FileDiff {
    old_path: String,
    new_path: String,
    lines: Vec<(Option<usize>, Option<usize>, char, String)>,
}

fn file_diffs(c: &Connection, p: &Project, mr: &MergeRequest) -> labrat_core::Result<Vec<FileDiff>> {
    let Some(d) = mr.latest_diff(c)? else { return Ok(vec![]) };
    let (Some(base), Some(head)) = (d.base_commit_sha.clone(), d.head_commit_sha.clone()) else { return Ok(vec![]) };
    let r = repo::repo(c, p);
    let (b, h) = (labrat_git::Oid::from_hex(&base)?, labrat_git::Oid::from_hex(&head)?);
    let (bt, ht) = (r.commit(&b)?.tree, r.commit(&h)?.tree);
    let mut out = vec![];
    for ch in labrat_git::diff::diff_trees(&r, Some(&bt), Some(&ht))? {
        let old = if ch.new_file { vec![] } else { r.blob(&ch.old_oid)? };
        let new = if ch.deleted_file { vec![] } else { r.blob(&ch.new_oid)? };
        let mut lines = vec![];
        if !labrat_git::diff::is_binary(&old) && !labrat_git::diff::is_binary(&new) {
            let ol: Vec<&str> = std::str::from_utf8(&old).unwrap_or("").lines().collect();
            let nl: Vec<&str> = std::str::from_utf8(&new).unwrap_or("").lines().collect();
            let all = labrat_git::diff::diff_lines(&old, &new);
            let changed: Vec<usize> = all.iter().enumerate().filter(|(_, l)| l.kind != labrat_git::diff::LineKind::Context).map(|(i, _)| i).collect();
            for (i, l) in all.iter().enumerate() {
                if !changed.iter().any(|c| c.abs_diff(i) <= 3) {
                    continue;
                }
                let (mark, text) = match l.kind {
                    labrat_git::diff::LineKind::Added => ('+', nl.get(l.new_pos.wrapping_sub(1)).copied().unwrap_or("")),
                    labrat_git::diff::LineKind::Removed => ('-', ol.get(l.old_pos.wrapping_sub(1)).copied().unwrap_or("")),
                    _ => (' ', nl.get(l.new_pos.wrapping_sub(1)).copied().unwrap_or("")),
                };
                lines.push((l.old_line(), l.new_line(), mark, text.to_string()));
            }
        }
        out.push(FileDiff { old_path: ch.old_path, new_path: ch.new_path, lines });
    }
    Ok(out)
}

fn load_mr(ui: &Ui, user: Option<&User>, p: &Project, iid: &str) -> Option<MrView> {
    let iid: i64 = iid.parse().ok()?;
    ui.app
        .db
        .tx(|c| {
            let Some(mr) = MergeRequest::find_by_iid(c, p.id, iid)? else { return Ok(None) };
            // As in GitLab, a viewer who can update the MR triggers a mergeability recheck.
            if let Some(u) = user
                && mrs::can_update(c, p, &mr, u)? {
                    mergeability::check_async(c, &mr)?;
                }
            let mr = mr.reload(c)?;
            let name = |id: i64| User::find(c, id).ok().flatten().map(|u| u.username).unwrap_or_default();
            let approvers = reviews::approvals(c, &mr)?.into_iter().map(|(u, _)| name(u)).collect();
            let notes = Note::for_noteable(c, "MergeRequest", mr.id)?;
            let mut discussions: Vec<Vec<(Note, String)>> = vec![];
            for n in notes {
                if n.internal && !user.is_some_and(|u| ProjectPerms::load(c, Some(u), p).map(|x| x.reporter()).unwrap_or(false)) {
                    continue;
                }
                let a = name(n.author_id);
                match discussions.iter_mut().find(|d| d[0].0.discussion_id == n.discussion_id) {
                    Some(d) => d.push((n, a)),
                    None => discussions.push(vec![(n, a)]),
                }
            }
            let mut awards: Vec<(String, usize, bool)> = vec![];
            for a in emoji::list(c, "MergeRequest", mr.id)? {
                let mine = user.is_some_and(|u| u.id == a.user_id);
                match awards.iter_mut().find(|x| x.0 == a.name) {
                    Some(x) => {
                        x.1 += 1;
                        x.2 |= mine;
                    }
                    None => awards.push((a.name.clone(), 1, mine)),
                }
            }
            let r = repo::repo(c, p);
            let mut commits = vec![];
            let refs = match mr.latest_diff(c)? {
                Some(d) => (
                    d.base_commit_sha.clone().unwrap_or_default(),
                    d.start_commit_sha.clone().unwrap_or_default(),
                    d.head_commit_sha.clone().unwrap_or_default(),
                ),
                None => Default::default(),
            };
            if let Some(d) = mr.latest_diff(c)? {
                for oid in d.commits(&r)? {
                    let cm = r.commit(&oid)?;
                    commits.push((oid.hex(), cm.message.lines().next().unwrap_or("").to_string()));
                }
            }
            let drafts = match user {
                Some(u) => reviews::drafts(c, &mr, u)?,
                None => vec![],
            };
            let mut suggestions: HashMap<i64, Vec<(i64, bool, Option<String>)>> = HashMap::new();
            for d in &discussions {
                for (n, _) in d {
                    for sg in labrat_core::services::suggestions::for_note(c, n.id)? {
                        let reason = labrat_core::services::suggestions::inapplicable_reason(c, p, &mr, n, &sg)?;
                        suggestions.entry(n.id).or_default().push((sg.id, sg.applied, reason));
                    }
                }
            }
            Ok::<_, labrat_core::Error>(Some(MrView {
                drafts,
                suggestions,
                author: name(mr.author_id),
                approvers,
                can_approve: match user {
                    Some(u) => reviews::can_approve(c, p, &mr, u)?,
                    None => false,
                },
                approved_by_me: match user {
                    Some(u) => reviews::has_approved(c, &mr, u)?,
                    None => false,
                },
                can_merge: match user {
                    Some(u) => mrs::can_be_merged_by(c, p, &mr, u)?,
                    None => false,
                },
                mergeable: mergeability::mergeable(c, &ui.app.config, p, &mr)?,
                detailed: mergeability::detailed_status(c, p, &mr)?,
                discussions,
                diff: file_diffs(c, p, &mr)?,
                awards,
                commits,
                refs,
                mr,
            }))
        })
        .ok()
        .flatten()
}

fn note_box(n: &Note, author: &str) -> Markup {
    html! {
        @if n.system {
            div.system { "@" (author) " " (PreEscaped(markdown::render_inline(&n.note))) " · " span.muted { (time::iso(n.created_at)) } }
        } @else {
            div.note {
                b { "@" (author) } " " span.muted { (time::iso(n.created_at)) @if n.internal { " · internal" } @if n.is_resolved() { " · resolved" } }
                div { (PreEscaped(markdown::render(&n.note))) }
            }
        }
    }
}

fn thread(base: &str, d: &[(Note, String)], signed_in: bool, sugg: &HashMap<i64, Vec<(i64, bool, Option<String>)>>) -> Markup {
    let first = &d[0].0;
    let resolvable = d.iter().any(|(n, _)| n.resolvable && !n.system);
    let resolved = resolvable && d.iter().filter(|(n, _)| n.resolvable && !n.system).all(|(n, _)| n.is_resolved());
    html! {
        div.box {
            @if let Some(pos) = first.position.as_deref().and_then(Position::parse) {
                p.muted { "On " code { (pos.file_path().unwrap_or_default()) } " line " (pos.new_line.or(pos.old_line).unwrap_or(0)) }
            }
            @for (n, a) in d {
                (note_box(n, a))
                @if let Some(list) = sugg.get(&n.id) {
                    @for (id, applied, reason) in list {
                        div.muted {
                            "Suggestion: "
                            @if *applied { "applied" }
                            @else if let Some(r) = reason { (r) }
                            @else if signed_in {
                                form.inline method="post" action=(format!("{base}/apply_suggestion")) {
                                    input type="hidden" name="id" value=(id);
                                    button { "Apply suggestion" }
                                }
                            }
                        }
                    }
                }
            }
            @if signed_in && !first.system {
                form method="post" action=(format!("{base}/note")) {
                    input type="hidden" name="discussion_id" value=(first.discussion_id);
                    textarea name="body" placeholder="Reply…" {}
                    button { "Reply" } " "
                    button name="review" value="1" { "Add to review" }
                }
                @if resolvable {
                    form.inline method="post" action=(format!("{base}/resolve")) {
                        input type="hidden" name="discussion_id" value=(first.discussion_id);
                        input type="hidden" name="resolved" value=(if resolved { "0" } else { "1" });
                        button { @if resolved { "Unresolve thread" } @else { "Resolve thread" } }
                    }
                }
            }
        }
    }
}

fn mr_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms, iid: &str, diffs: bool) -> Response {
    let Some(v) = load_mr(ui, user, p, iid) else { return not_found() };
    let mr = &v.mr;
    let base = format!("/{}/-/merge_requests/{}", p.full_path, mr.iid);
    let signed_in = user.is_some();
    let general: Vec<&Vec<(Note, String)>> = v.discussions.iter().filter(|d| d[0].0.position.is_none()).collect();
    html_resp(
        &mr.title,
        user,
        html! {
            (project_nav(p, perms))
            h2 { (mr.title) " " span.muted { "!" (mr.iid) } }
            p {
                span class=(format!("badge {}", mr.state)) { (mr.state) } " "
                "@" (v.author) " wants to merge " code { (mr.source_branch) } " into " code { (mr.target_branch) }
                " · " span.badge { (v.detailed) }
            }
            nav.tabs { a href=(base) { "Overview" } a href=(format!("{base}/diffs")) { "Changes (" (v.diff.len()) ")" } }
            @if !diffs {
                div.box { (PreEscaped(markdown::render(mr.description.as_deref().unwrap_or("")))) }
                div.box {
                    @for (name, count, mine) in &v.awards { span.badge { (name) " " (count) @if *mine { " (you)" } } " " }
                    @if signed_in {
                        @for e in ["thumbsup", "thumbsdown", "tada", "rocket"] {
                            form.inline method="post" action=(format!("{base}/award")) { input type="hidden" name="name" value=(e); button { (e) } }
                        }
                    }
                }
                div.box {
                    p { "Approved by: " @if v.approvers.is_empty() { span.muted { "nobody" } } @else { (v.approvers.join(", ")) } }
                    @if signed_in && mr.is_open() {
                        @if v.approved_by_me { form.inline method="post" action=(format!("{base}/unapprove")) { button { "Revoke approval" } } }
                        @else if v.can_approve { form.inline method="post" action=(format!("{base}/approve")) { button { "Approve" } } }
                        " "
                        @if v.can_merge && v.mergeable { form.inline method="post" action=(format!("{base}/merge")) { button { "Merge" } } " " }
                        form.inline method="post" action=(format!("{base}/draft")) { button { @if mr.is_draft() { "Mark as ready" } @else { "Mark as draft" } } } " "
                        form.inline method="post" action=(format!("{base}/close")) { button { "Close" } }
                    }
                    @if signed_in && mr.state == "closed" { form.inline method="post" action=(format!("{base}/reopen")) { button { "Reopen" } } }
                    @if let Some(e) = &mr.merge_error { p style="color:#b00" { (e) } }
                }
                h3 { "Commits" }
                div.box { table { @for (sha, title) in &v.commits { tr { td { code { (&sha[..8]) } } td { (title) } } } } }
                h3 { "Activity" }
                @for d in &general { (thread(&base, d, signed_in, &v.suggestions)) }
                @for d in v.discussions.iter().filter(|d| d[0].0.position.is_some()) { (thread(&base, d, signed_in, &v.suggestions)) }
                @if signed_in {
                    form.box method="post" action=(format!("{base}/note")) {
                        textarea name="body" placeholder="Write a comment (quick actions such as /approve work)" {}
                        label { input type="checkbox" name="thread" value="1"; " Start a thread" } " "
                        button { "Comment" } " "
                        button name="review" value="1" { "Add to review" }
                    }
                }
                @if !v.drafts.is_empty() {
                    div.box {
                        h3 { "Your pending review (" (v.drafts.len()) ")" }
                        @for d in &v.drafts {
                            div.note {
                                @if let Some(pos) = d.position.as_deref().and_then(Position::parse) {
                                    span.muted { code { (pos.file_path().unwrap_or_default()) } " line " (pos.new_line.or(pos.old_line).unwrap_or(0)) " · " }
                                }
                                (d.note)
                                form.inline method="post" action=(format!("{base}/delete_draft")) { input type="hidden" name="id" value=(d.id); " " button { "Discard" } }
                            }
                        }
                        form method="post" action=(format!("{base}/submit_review")) {
                            textarea name="summary" placeholder="Summary (optional)" {}
                            label { input type="radio" name="verdict" value="comment" checked; " Comment" } " "
                            label { input type="radio" name="verdict" value="approve"; " Approve" } " "
                            label { input type="radio" name="verdict" value="requested_changes"; " Request changes" } " "
                            button { "Submit review" }
                        }
                    }
                }
            } @else {
                @let refs = &v.refs;
                @for f in &v.diff {
                    div.box {
                        b { @if f.old_path != f.new_path { (f.old_path) " → " } (f.new_path) }
                        table.diff {
                            @for (ol, nl, mark, text) in &f.lines {
                                tr class=(match mark { '+' => "add", '-' => "del", _ => "" }) {
                                    td.ln { (ol.map(|x| x.to_string()).unwrap_or_default()) }
                                    td.ln { (nl.map(|x| x.to_string()).unwrap_or_default()) }
                                    td { (mark) (text)
                                        @for d in v.discussions.iter().filter(|d| d[0].0.position.as_deref().and_then(Position::parse).is_some_and(|pp| pp.new_path.as_deref() == Some(f.new_path.as_str()) && pp.new_line.map(|x| x as usize) == *nl && pp.old_line.map(|x| x as usize) == *ol)) {
                                            div style="white-space:normal" { (thread(&base, d, signed_in, &v.suggestions)) }
                                        }
                                        @if signed_in && mr.is_open() {
                                            details style="white-space:normal" { summary.muted { "comment" }
                                                form method="post" action=(format!("{base}/diff_note")) {
                                                    input type="hidden" name="old_path" value=(f.old_path);
                                                    input type="hidden" name="new_path" value=(f.new_path);
                                                    @if let Some(x) = ol { input type="hidden" name="old_line" value=(x); }
                                                    @if let Some(x) = nl { input type="hidden" name="new_line" value=(x); }
                                                    input type="hidden" name="base_sha" value=(refs.0);
                                                    input type="hidden" name="start_sha" value=(refs.1);
                                                    input type="hidden" name="head_sha" value=(refs.2);
                                                    textarea name="body" {}
                                                    button { "Add comment" } " "
                                                    button name="review" value="1" { "Add to review" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

fn mr_action(ui: &Ui, user: Option<&User>, p: &Project, iid: &str, action: &str) -> Response {
    let Some(u) = user else { return redirect("/users/sign_in") };
    let iid: i64 = iid.parse().unwrap_or(-1);
    let base = format!("/{}/-/merge_requests/{iid}", p.full_path);
    let config = &ui.app.config;
    let mut back = base.clone();
    let res = ui.app.db.tx(|c| {
        let Some(mr) = MergeRequest::find_by_iid(c, p.id, iid)? else { return Ok(()) };
        match action {
            "note" if ui.f("review") == "1" => {
                let body = ui.f("body");
                if !body.trim().is_empty() {
                    let reply = Some(ui.f("discussion_id")).filter(|d| !d.is_empty());
                    let _ = reviews::create_draft(c, p, &mr, u, &body, reply.as_deref(), false, None, None)?;
                }
            }
            "delete_draft" => {
                let id: i64 = ui.f("id").parse().unwrap_or(0);
                if let Some(d) = reviews::drafts(c, &mr, u)?.into_iter().find(|d| d.id == id) {
                    reviews::delete_draft(c, &mr, u, &d)?;
                }
            }
            "submit_review" => {
                reviews::publish_drafts(c, config, p, &mr, u)?;
                let summary = ui.f("summary");
                if !summary.trim().is_empty() {
                    comments::create(c, config, p, &mr, u, CreateParams { body: summary, ..Default::default() })?;
                }
                match ui.f("verdict").as_str() {
                    "approve" => {
                        reviews::approve(c, config, p, &mr, u)?;
                    }
                    "requested_changes" => {
                        reviews::update_reviewer_state(c, config, p, &mr, u, "requested_changes")?;
                    }
                    _ => {
                        reviews::update_reviewer_state(c, config, p, &mr, u, "reviewed")?;
                    }
                }
            }
            "apply_suggestion" => {
                let id: i64 = ui.f("id").parse().unwrap_or(0);
                let _ = labrat_core::services::suggestions::apply(c, config, u, &[id], None)?;
            }
            "note" => {
                let body = ui.f("body");
                if !body.trim().is_empty() {
                    let reply = Some(ui.f("discussion_id")).filter(|d| !d.is_empty());
                    let params = CreateParams { body, thread: ui.f("thread") == "1", reply_to: reply, ..Default::default() };
                    comments::create(c, config, p, &mr, u, params)?;
                }
            }
            "diff_note" => {
                let mut pos = json!({
                    "base_sha": ui.f("base_sha"), "start_sha": ui.f("start_sha"), "head_sha": ui.f("head_sha"),
                    "old_path": ui.f("old_path"), "new_path": ui.f("new_path"), "position_type": "text",
                });
                for k in ["old_line", "new_line"] {
                    if let Ok(n) = ui.f(k).parse::<i64>() {
                        pos[k] = json!(n);
                    }
                }
                if ui.f("review") == "1" {
                    let _ = reviews::create_draft(c, p, &mr, u, &ui.f("body"), None, false, Position::parse(&pos.to_string()), None)?;
                } else {
                    let params = CreateParams { body: ui.f("body"), thread: true, position: Position::parse(&pos.to_string()), ..Default::default() };
                    comments::create(c, config, p, &mr, u, params)?;
                }
                back = format!("{base}/diffs");
            }
            "resolve" => comments::resolve_discussion(c, config, p, &mr, u, &ui.f("discussion_id"), ui.f("resolved") == "1")?,
            "award" => {
                if let Some((a, added)) = emoji::toggle(c, u, "MergeRequest", mr.id, &ui.f("name"))? {
                    labrat_core::services::hooks::emoji_event(c, config, p, &mr, None, u, &a, if added { "award" } else { "revoke" })?;
                }
            }
            "approve" => {
                reviews::approve(c, config, p, &mr, u)?;
            }
            "unapprove" => {
                reviews::unapprove(c, config, p, &mr, u)?;
            }
            "merge" => {
                if mrs::can_be_merged_by(c, p, &mr, u)? && mergeability::mergeable(c, config, p, &mr)? {
                    merge::merge(c, config, p, &mr, u, &merge::MergeParams { commit_message: None, squash_commit_message: None, should_remove_source_branch: None, sha: None })?;
                }
            }
            "close" | "reopen" if mrs::can_update(c, p, &mr, u)? => {
                mrs::apply_state_event(c, config, p, &mr, u, Some(action))?;
            }
            "draft" if mrs::can_update(c, p, &mr, u)? => {
                let title = if mr.is_draft() { mrs::draftless_title(&mr.title) } else { mrs::draft_title(&mr.title) };
                mrs::update(c, config, p, &mr, u, mrs::MrParams { title: Some(title), ..Default::default() })?;
            }
            _ => {}
        }
        Ok::<_, labrat_core::Error>(())
    });
    crate::jobs::kick(&ui.app);
    if let Err(e) = res {
        tracing::warn!("ui action {action} failed: {e}");
    }
    redirect(&back)
}

// ---------------------------------------------------------------------------------------------
// Settings

fn settings_page(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms, error: Option<String>) -> Response {
    let data = ui.app.db.read(|c| {
        let ms = members::list(c, "Project", p.id)?;
        let mut member_rows = vec![];
        for m in ms {
            if let Some(u) = User::find(c, m.user_id)? {
                member_rows.push((u.username, m.access_level));
            }
        }
        let protected = repo::protected_branches(c, p.id)?;
        let hooks = Hook::list(c, "ProjectHook", Some(p.id))?;
        let mut logs = vec![];
        for h in &hooks {
            let mut st = c.prepare("SELECT trigger, response_status, created_at FROM web_hook_logs WHERE web_hook_id = ?1 ORDER BY id DESC LIMIT 5")?;
            let rows: Vec<(String, String, i64)> = st.query_map([h.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
            logs.push((h.id, rows));
        }
        Ok::<_, labrat_core::Error>((member_rows, protected, hooks, logs))
    });
    let Ok((member_rows, protected, hooks, logs)) = data else { return not_found() };
    let base = format!("/{}/-/settings", p.full_path);
    let level = |l: i64| match l {
        10 => "Guest",
        15 => "Planner",
        20 => "Reporter",
        30 => "Developer",
        40 => "Maintainer",
        50 => "Owner",
        _ => "?",
    };
    html_resp(
        "Settings",
        user,
        html! {
            (project_nav(p, perms))
            @if let Some(e) = error { p style="color:#b00" { (e) } }
            h3 { "Merge requests" }
            form.box method="post" action=(format!("{base}/merge_requests")) {
                p { "Merge method " select name="merge_method" {
                    @for (v, l) in [("merge", "Merge commit"), ("rebase_merge", "Merge commit with semi-linear history"), ("ff", "Fast-forward merge")] {
                        option value=(v) selected[p.merge_method == v] { (l) } } } }
                p { "Squash commits " select name="squash_option" {
                    @for v in ["never", "always", "default_on", "default_off"] { option value=(v) selected[p.squash_option == v] { (v) } } } }
                p { label { input type="checkbox" name="remove_source_branch_after_merge" value="1" checked[p.remove_source_branch_after_merge]; " Delete source branch by default" } }
                p { label { input type="checkbox" name="only_allow_merge_if_all_discussions_are_resolved" value="1" checked[p.only_allow_merge_if_all_discussions_are_resolved]; " All threads must be resolved" } }
                p { label { input type="checkbox" name="resolve_outdated_diff_discussions" value="1" checked[p.resolve_outdated_diff_discussions]; " Resolve outdated diff threads automatically" } }
                button { "Save" }
            }
            h3 { "Members" }
            div.box {
                table { @for (name, l) in &member_rows { tr { td { "@" (name) } td { (level(*l)) } } } }
                form method="post" action=(format!("{base}/members")) {
                    input type="text" name="username" placeholder="username"; " "
                    select name="access_level" { @for l in [10, 20, 30, 40, 50] { option value=(l) { (level(l)) } } } " "
                    button { "Add member" }
                }
            }
            h3 { "Protected branches" }
            div.box {
                table { @for b in &protected { tr { td { code { (b.name) } } td {
                    form.inline method="post" action=(format!("{base}/unprotect")) { input type="hidden" name="name" value=(b.name); button { "Unprotect" } } } } } }
                form method="post" action=(format!("{base}/protect")) {
                    input type="text" name="name" placeholder="branch or wildcard"; " "
                    button { "Protect (Maintainers push and merge)" }
                }
            }
            h3 { "Webhooks" }
            div.box {
                @for h in &hooks {
                    div.note {
                        code { (h.url) } " "
                        span.muted { @for e in ["push_events", "merge_requests_events", "note_events", "emoji_events"] { @if h.enabled(e) { (e) " " } } }
                        form.inline method="post" action=(format!("{base}/delete_hook")) { input type="hidden" name="id" value=(h.id); button { "Delete" } }
                        @if let Some((_, rows)) = logs.iter().find(|(id, _)| *id == h.id) {
                            table { @for (t, s, at) in rows { tr { td.muted { (t) } td { (s) } td.muted { (time::iso(*at)) } } } }
                        }
                    }
                }
                form method="post" action=(format!("{base}/add_hook")) {
                    input type="text" name="url" placeholder="https://example.com/hook" style="width:50%"; " "
                    input type="text" name="token" placeholder="secret token"; br;
                    @for e in ["push_events", "tag_push_events", "merge_requests_events", "note_events", "emoji_events"] {
                        label { input type="checkbox" name=(e) value="1" checked[e != "tag_push_events"]; " " (e) } " "
                    }
                    br; button { "Add webhook" }
                }
            }
        },
    )
}

fn settings_action(ui: &Ui, user: Option<&User>, p: &Project, perms: &ProjectPerms, action: &str) -> Response {
    let Some(u) = user else { return redirect("/users/sign_in") };
    let res = ui.app.db.tx(|c| {
        match action {
            "merge_requests" => {
                let flag = |k: &str| Some(ui.f(k) == "1");
                projects::update(
                    c,
                    p,
                    projects::ProjectAttrs {
                        merge_method: Some(ui.f("merge_method")),
                        squash_option: Some(ui.f("squash_option")),
                        remove_source_branch_after_merge: flag("remove_source_branch_after_merge"),
                        only_allow_merge_if_all_discussions_are_resolved: flag("only_allow_merge_if_all_discussions_are_resolved"),
                        resolve_outdated_diff_discussions: flag("resolve_outdated_diff_discussions"),
                        ..Default::default()
                    },
                )?;
            }
            "members" => {
                let target = User::find_by_username(c, &ui.f("username"))?.ok_or(labrat_core::Error::not_found("User"))?;
                members::add(c, Some(u), "Project", p.id, target.id, ui.f("access_level").parse().unwrap_or(30), None)?;
            }
            "protect" => {
                repo::create_protected_branch(c, p, &ui.f("name"), 40, 40, false)?;
            }
            "unprotect" => {
                c.execute("DELETE FROM protected_branches WHERE project_id = ?1 AND name = ?2", rusqlite::params![p.id, ui.f("name")])?;
            }
            "add_hook" => {
                let mut events = Map::new();
                for e in ["push_events", "tag_push_events", "merge_requests_events", "note_events", "emoji_events"] {
                    events.insert(e.to_string(), json!(ui.f(e) == "1"));
                }
                c.execute(
                    "INSERT INTO web_hooks (project_id, type, url, token, events, branch_filter_strategy, enable_ssl_verification, custom_headers, created_at, updated_at)
                     VALUES (?1, 'ProjectHook', ?2, ?3, ?4, 'wildcard', 1, '{}', ?5, ?5)",
                    rusqlite::params![p.id, ui.f("url"), Some(ui.f("token")).filter(|t| !t.is_empty()), Value::Object(events).to_string(), time::now()],
                )?;
            }
            "delete_hook" => {
                c.execute("DELETE FROM web_hooks WHERE id = ?1 AND project_id = ?2", rusqlite::params![ui.f("id").parse::<i64>().unwrap_or(0), p.id])?;
            }
            _ => {}
        }
        Ok::<_, labrat_core::Error>(())
    });
    match res {
        Ok(()) => redirect(&format!("/{}/-/settings", p.full_path)),
        Err(e) => settings_page(ui, user, p, perms, Some(error_text(&e))),
    }
}

/// Uploaded files: `/<project path>/uploads/<secret>/<name>` and
/// `/-/project/<id>/uploads/<secret>/<name>`, for users who can read the project.
///
/// labrat also answers `/uploads/<secret>/<name>` (the link stored in Markdown), so images in
/// rendered notes work without rewriting links per project as GitLab's renderer does.
fn upload_file(ui: &Ui, user: Option<&User>, path: &str) -> Option<Response> {
    let (head, tail) = match path.strip_prefix("/uploads/") {
        Some(t) => ("", t),
        None => path.trim_start_matches('/').split_once("/uploads/")?,
    };
    let (secret, name) = tail.split_once('/')?;
    if secret.len() != 32 || name.contains('/') {
        return None;
    }
    let found = ui.app.db.read(|c| {
        let p = if head.is_empty() {
            let pid: Option<i64> = c.query_row("SELECT project_id FROM uploads WHERE secret = ?1", [secret], |r| r.get(0)).ok();
            match pid {
                Some(id) => Project::find(c, id)?,
                None => None,
            }
        } else {
            match head.strip_prefix("-/project/") {
                Some(id) => Project::find(c, id.parse().unwrap_or(-1))?,
                None => Project::find_by_full_path(c, head)?,
            }
        };
        let Some(p) = p else { return Ok::<_, labrat_core::Error>(None) };
        if !labrat_core::perms::ProjectPerms::load(c, user, &p)?.read_project() {
            return Ok(None);
        }
        let data: Option<Vec<u8>> = c
            .query_row("SELECT data FROM uploads WHERE project_id = ?1 AND secret = ?2 AND filename = ?3", rusqlite::params![p.id, secret, name], |r| r.get(0))
            .ok();
        Ok(data)
    });
    let Ok(Some(data)) = found else { return Some(not_found()) };
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let ctype = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain; charset=utf-8",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    };
    Some(([(header::CONTENT_TYPE, ctype.to_string())], data).into_response())
}
