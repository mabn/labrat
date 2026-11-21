//! Resolvers for the merge-request part of GitLab's GraphQL schema. Fields outside it return
//! an error saying they are not implemented in labrat.

use super::exec::{Args, Ctx, FResult, FieldError, Object, Out};
use base64::Engine;
use labrat_core::model::{Namespace, Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::emoji::{self, Award};
use labrat_core::services::labels::Label;
use labrat_core::services::merge_requests::{self as mrs, MergeRequest, mergeability};
use labrat_core::services::notes::Note;
use labrat_core::services::positions::Position;
use labrat_core::services::{reviews, subscriptions};
use labrat_core::{repo, time};
use serde_json::{Value, json};
use std::rc::Rc;

pub fn not_implemented(ty: &str, field: &str) -> FieldError {
    FieldError::new(format!("Field '{field}' on type '{ty}' is not implemented in labrat"))
}

pub fn gid(kind: &str, id: i64) -> Value {
    json!(format!("gid://gitlab/{kind}/{id}"))
}

/// Parses `gid://gitlab/<Kind>/<id>`.
pub fn parse_gid(s: &str, kinds: &[&str]) -> FResult<i64> {
    let invalid = || FieldError::new(format!("\"{s}\" is not a valid Global ID"));
    let rest = s.strip_prefix("gid://gitlab/").ok_or_else(invalid)?;
    let (kind, id) = rest.split_once('/').ok_or_else(invalid)?;
    if !kinds.is_empty() && !kinds.contains(&kind) {
        return Err(FieldError::new(format!("\"{s}\" does not represent an instance of {}", kinds[0])));
    }
    id.parse().map_err(|_| invalid())
}

/// GraphQL `Time`: ISO 8601 in UTC without fractions.
pub fn gtime(ms: i64) -> Value {
    json!(time::iso_secs(ms))
}

fn opt_time(ms: Option<i64>) -> Value {
    ms.map(gtime).unwrap_or(Value::Null)
}

pub const RESOURCE_UNAVAILABLE: &str =
    "The resource that you are attempting to access does not exist or you don't have permission to perform this action";

pub fn unavailable() -> FieldError {
    FieldError::new(RESOURCE_UNAVAILABLE)
}

/// `loads:` arguments whose object doesn't exist: graphql-ruby inspects the GlobalID.
pub fn no_object(arg: &str, gid: &str) -> FieldError {
    FieldError::new(format!("No object found for `{arg}: #<GlobalID:0x00007f0000000000 @uri=#<URI::GID {gid}>>`"))
}

// ---------------------------------------------------------------------------------------------
// Connections

/// A shared object so that `nodes` and `edges { node }` can both render it.
#[derive(Clone)]
pub struct Shared(pub Rc<dyn Object>);

impl Object for Shared {
    fn typename(&self) -> String {
        self.0.typename()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        self.0.resolve(cx, field, args)
    }
}

/// A cursor-paginated connection (`first`/`after`, `last`/`before`).
pub struct Conn {
    pub ty: String,
    pub items: Vec<(String, Shared)>,
    pub has_next: bool,
    pub has_prev: bool,
    pub count: usize,
    /// Raised by the connection's fields (`nodes`, `edges`, `pageInfo`), as GitLab's lazy
    /// connections do.
    pub error: Option<String>,
}

thread_local! {
    /// Set when a resolver hit what would be an exception in GitLab (answered with a 500).
    pub static INTERNAL_ERROR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Decodes a cursor; `None` when it isn't url-safe base64 of UTF-8 JSON or a position.
fn decode_cursor(c: &str) -> Option<Value> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(c.trim_end_matches('=')).ok()?;
    let s = String::from_utf8(bytes).ok()?;
    match serde_json::from_str::<Value>(&s) {
        Ok(v @ Value::Object(_)) => Some(v),
        _ if !s.is_empty() && s.chars().all(|ch| ch.is_ascii_digit()) => Some(Value::String(s)),
        _ => None,
    }
}

/// GitLab's cursor encoding: url-safe base64 without padding. Strings are encoded as is
/// (array connections use the 1-based position), objects as JSON.
pub fn cursor(v: Value) -> String {
    let raw = match v {
        Value::String(s) => s,
        other => other.to_string(),
    };
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

/// Keyset cursor over `created_at`: `{"created_at":"2026-11-07 07:05:47.255965000 +0000","id":"74"}`.
fn created_cursor(created_ms: i64, id: i64) -> Value {
    let t = chrono::DateTime::from_timestamp_millis(created_ms).unwrap_or_default();
    json!({"created_at": t.format("%Y-%m-%d %H:%M:%S.%f +0000").to_string(), "id": id.to_string()})
}

/// Array-connection cursors: the 1-based position.
fn positions<T>(items: Vec<T>) -> Vec<(Value, T)> {
    items.into_iter().enumerate().map(|(i, x)| (json!((i + 1).to_string()), x)).collect()
}

impl Conn {
    /// Paginates `all` (already ordered); `key` gives each item's cursor payload.
    /// `all` is already ordered; each item comes with its cursor payload.
    pub fn new(ty: &str, all: Vec<(Value, Shared)>, args: &Args) -> FResult<Conn> {
        Conn::with_max(ty, all, args, 100)
    }

    /// Like `new`, for connections whose field sets its own `max_page_size`.
    pub fn with_max(ty: &str, all: Vec<(Value, Shared)>, args: &Args, max: usize) -> FResult<Conn> {
        let count = all.len();
        let mut error = None;
        if args.int("first").is_some() && args.int("last").is_some() {
            error = Some("Can only provide either `first` or `last`, not both".to_string());
        }
        for key in ["after", "before"] {
            if let Some(c) = args.str(key) {
                match decode_cursor(&c) {
                    None => error = Some("Please provide a valid cursor".to_string()),
                    // A keyset cursor missing this connection's keys makes GitLab raise.
                    Some(Value::Object(o)) => {
                        let wanted: Vec<String> = all.first().and_then(|(k, _)| k.as_object().map(|m| m.keys().cloned().collect())).unwrap_or_default();
                        if wanted.iter().any(|w| !o.contains_key(w)) {
                            INTERNAL_ERROR.with(|f| f.set(true));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut items: Vec<(String, Shared)> = all.into_iter().map(|(k, o)| (cursor(k), o)).collect();
        let mut has_prev = false;
        let mut has_next = false;
        if let Some(a) = args.str("after")
            && let Some(i) = items.iter().position(|(c, _)| *c == a) {
                items.drain(..=i);
                has_prev = true;
            }
        if let Some(b) = args.str("before")
            && let Some(i) = items.iter().position(|(c, _)| *c == b) {
                items.truncate(i);
                has_next = true;
            }
        if let Some(f) = args.int("first") {
            // A negative size behaves like zero.
            let f = f.max(0);
            if items.len() > f as usize {
                items.truncate(f as usize);
                has_next = true;
            }
        }
        if let Some(l) = args.int("last") {
            if l < 0 {
                return Err(FieldError::new("last must be greater than or equal to 0"));
            }
            if items.len() > l as usize {
                let drop = items.len() - l as usize;
                items.drain(..drop);
                has_prev = true;
            }
        }
        if items.len() > max {
            items.truncate(max);
            has_next = true;
        }
        Ok(Conn { ty: ty.to_string(), items, has_next, has_prev, count, error })
    }
}

impl Object for Conn {
    fn typename(&self) -> String {
        self.ty.clone()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        if let Some(e) = &self.error
            && matches!(field, "nodes" | "edges")
        {
            return Err(FieldError::new(e.clone()));
        }
        Ok(match field {
            "nodes" => Out::List(self.items.iter().map(|(_, o)| Out::obj(o.clone())).collect()),
            "edges" => Out::List(
                self.items
                    .iter()
                    .map(|(c, o)| Out::obj(Edge { ty: self.ty.trim_end_matches("Connection").to_string() + "Edge", cursor: c.clone(), node: o.clone() }))
                    .collect(),
            ),
            "pageInfo" => Out::obj(PageInfo {
                start: self.items.first().map(|(c, _)| c.clone()),
                end: self.items.last().map(|(c, _)| c.clone()),
                has_next: self.has_next,
                has_prev: self.has_prev,
                error: self.error.clone(),
            }),
            "count" => Out::val(self.count),
            _ => return Err(not_implemented(&self.ty, field)),
        })
    }
}

struct Edge {
    ty: String,
    cursor: String,
    node: Shared,
}

impl Object for Edge {
    fn typename(&self) -> String {
        self.ty.clone()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "cursor" => Out::val(self.cursor.clone()),
            "node" => Out::obj(self.node.clone()),
            _ => return Err(not_implemented(&self.ty, field)),
        })
    }
}

struct PageInfo {
    start: Option<String>,
    end: Option<String>,
    has_next: bool,
    has_prev: bool,
    error: Option<String>,
}

impl Object for PageInfo {
    fn typename(&self) -> String {
        "PageInfo".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        if let Some(e) = &self.error {
            return Err(FieldError::new(e.clone()));
        }
        Ok(match field {
            "startCursor" => Out::val(self.start.clone()),
            "endCursor" => Out::val(self.end.clone()),
            "hasNextPage" => Out::val(self.has_next),
            "hasPreviousPage" => Out::val(self.has_prev),
            _ => return Err(not_implemented("PageInfo", field)),
        })
    }
}

fn id_cursor(id: i64) -> Value {
    json!({"id": id.to_string()})
}

// ---------------------------------------------------------------------------------------------
// Query

pub struct Query;

fn readable_project(cx: &Ctx, p: Project) -> FResult<Option<(Project, ProjectPerms)>> {
    let perms = ProjectPerms::load(cx.c, cx.user(), &p)?;
    Ok(if perms.read_project() { Some((p, perms)) } else { None })
}

impl Object for Query {
    fn typename(&self) -> String {
        "Query".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        match field {
            "currentUser" => Ok(match cx.user() {
                Some(u) => Out::obj(UserObj::new("CurrentUser", u.clone())),
                None => Out::Null,
            }),
            "user" => {
                let u = match (args.str("id"), args.str("username")) {
                    (Some(id), _) => User::find(cx.c, parse_gid(&id, &["User"])?)?,
                    (None, Some(n)) => User::find_by_username(cx.c, &n)?,
                    _ => return Err(FieldError::new("Provide either a single username or id")),
                };
                Ok(Out::opt(u.map(|u| UserObj::new("UserCore", u))))
            }
            "project" => {
                let path = args.str("fullPath").unwrap_or_default();
                let p = Project::find_by_full_path(cx.c, &path)?;
                Ok(match p.map(|p| readable_project(cx, p)).transpose()?.flatten() {
                    Some((p, perms)) => Out::obj(ProjectObj { p, perms }),
                    None => Out::Null,
                })
            }
            "mergeRequest" => {
                let id = parse_gid(&args.str("id").unwrap_or_default(), &["MergeRequest"])?;
                let Some(mr) = MergeRequest::find(cx.c, id)? else { return Ok(Out::Null) };
                let p = Project::find(cx.c, mr.target_project_id)?.expect("project exists");
                Ok(match readable_project(cx, p)? {
                    Some((p, perms)) if perms.read_merge_request(&p) => Out::obj(MrObj::new(p, mr)),
                    _ => Out::Null,
                })
            }
            "note" => {
                let id = parse_gid(&args.str("id").unwrap_or_default(), &["Note", "DiffNote", "DiscussionNote"])?;
                let Some(n) = Note::find(cx.c, id)? else { return Ok(Out::Null) };
                let Some(mr) = MergeRequest::find(cx.c, n.noteable_id)? else { return Ok(Out::Null) };
                let p = Project::find(cx.c, mr.target_project_id)?.expect("project exists");
                Ok(match readable_project(cx, p)? {
                    Some((p, _)) if note_visible(cx, &p, &n) => Out::obj(NoteObj { p, mr, n }),
                    _ => Out::Null,
                })
            }
            "users" => {
                if cx.user().is_none() {
                    return Err(unavailable());
                }
                let mut list: Vec<User> = vec![];
                if let Some(names) = args.get("usernames").and_then(Value::as_array) {
                    for n in names.iter().filter_map(Value::as_str) {
                        if let Some(u) = User::find_by_username(cx.c, n)? {
                            list.push(u);
                        }
                    }
                } else if let Some(ids) = args.get("ids").and_then(Value::as_array) {
                    for i in ids.iter().filter_map(Value::as_str) {
                        let id = i.rsplit('/').next().and_then(|x| x.parse::<i64>().ok()).ok_or_else(|| FieldError::new(format!("{i} is not a valid GitLab ID.")))?;
                        if let Some(u) = User::find(cx.c, id)? {
                            list.push(u);
                        }
                    }
                } else {
                    let mut st = cx.c.prepare("SELECT id FROM users ORDER BY id DESC")?;
                    let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                    for id in ids {
                        if let Some(u) = User::find(cx.c, id)? {
                            list.push(u);
                        }
                    }
                }
                let items = list.into_iter().map(|u| (id_cursor(u.id), Shared(Rc::new(UserObj::new("UserCore", u))))).collect();
                Ok(Out::obj(Conn::new("UserCoreConnection", items, args)?))
            }
            "projects" => {
                let mut st = cx.c.prepare("SELECT id FROM projects ORDER BY id DESC")?;
                let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                let search = args.str("search").map(|s| s.to_lowercase());
                let membership = args.bool("membership").unwrap_or(false);
                let mut items = vec![];
                for id in ids {
                    let Some(p) = Project::find(cx.c, id)? else { continue };
                    let Some((p, perms)) = readable_project(cx, p)? else { continue };
                    if membership && perms.level == 0 {
                        continue;
                    }
                    // `Project.search`: name, path and description (not the namespace).
                    if search.as_ref().is_some_and(|q| {
                        !p.name.to_lowercase().contains(q)
                            && !p.path.to_lowercase().contains(q)
                            && !p.description.as_deref().unwrap_or("").to_lowercase().contains(q)
                    }) {
                        continue;
                    }
                    items.push((id_cursor(p.id), Shared(Rc::new(ProjectObj { p, perms }))));
                }
                Ok(Out::obj(Conn::new("ProjectConnection", items, args)?))
            }
            // `EchoResolver`: Ruby's `inspect` of the username (`nil` when anonymous).
            "echo" => Ok(Out::val(match cx.user() {
                Some(u) => format!("\"{}\" says: {}", u.username, args.str("text").unwrap_or_default()),
                None => format!("nil says: {}", args.str("text").unwrap_or_default()),
            })),
            "todo" => {
                let Some(u) = cx.user() else { return Ok(Out::Null) };
                let id = parse_gid(&args.str("id").unwrap_or_default(), &["Todo"])?;
                let owner: Option<i64> = cx.c.query_row("SELECT user_id FROM todos WHERE id = ?1", [id], |r| r.get(0)).ok();
                Ok(if owner == Some(u.id) { Out::obj(TodoObj::load(cx.c, id)?) } else { Out::Null })
            }
            "milestone" => {
                let id = parse_gid(&args.str("id").unwrap_or_default(), &["Milestone"])?;
                milestone_obj(cx, Some(id))
            }
            "groups" => {
                let search = args.str("search").map(|s| s.to_lowercase());
                let mut items = vec![];
                let mut st = cx.c.prepare("SELECT id FROM namespaces WHERE type = 'Group' ORDER BY id")?;
                let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                for id in ids {
                    let Some(n) = Namespace::find(cx.c, id)? else { continue };
                    let gp = labrat_core::perms::GroupPerms::load(cx.c, cx.user(), &n)?;
                    let readable = gp.admin || gp.level > 0 || n.visibility_level == labrat_core::model::visibility::PUBLIC
                        || (n.visibility_level == labrat_core::model::visibility::INTERNAL && cx.user().is_some());
                    if !readable || search.as_ref().is_some_and(|q| !n.full_path.to_lowercase().contains(q) && !n.name.to_lowercase().contains(q)) {
                        continue;
                    }
                    items.push((id_cursor(id), Shared(Rc::new(NamespaceObj { ty: "Group", n }))));
                }
                Ok(Out::obj(Conn::new("GroupConnection", items, args)?))
            }
            "queryComplexity" => {
                let (score, limit) = super::analysis::SCORE.with(|s| s.get());
                Ok(Out::obj(JsonObj::new("QueryComplexity", json!({"score": score, "limit": limit}))))
            }
            "metadata" => Ok(match cx.user() {
                Some(_) => Out::obj(JsonObj::new("Metadata", json!({"version": cx.req.app.config.gitlab_version(), "revision": cx.req.app.config.gitlab_revision(), "enterprise": cx.req.app.config.is_ee()}))),
                None => Out::Null,
            }),
            "group" | "namespace" => {
                let path = args.str("fullPath").unwrap_or_default();
                let ns = Namespace::find_by_full_path(cx.c, &path)?;
                // `group` resolves groups only; both need the caller to be able to read it.
                let ns = match ns {
                    Some(n) if field == "group" && n.kind != "Group" => None,
                    Some(n) if n.kind == "Group" => {
                        let gp = labrat_core::perms::GroupPerms::load(cx.c, cx.user(), &n)?;
                        let readable = gp.admin || gp.level > 0 || n.visibility_level == labrat_core::model::visibility::PUBLIC
                            || (n.visibility_level == labrat_core::model::visibility::INTERNAL && cx.user().is_some())
                            || gp.has_visible_projects;
                        readable.then_some(n)
                    }
                    Some(n) => cx.user().filter(|u| u.admin || u.username == n.path).map(|_| n),
                    None => None,
                };
                Ok(Out::opt(ns.map(|n| NamespaceObj { ty: if field == "group" || n.kind == "Group" { "Group" } else { "Namespace" }, n })))
            }
            _ => Err(not_implemented("Query", field)),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Users

pub struct UserObj {
    ty: &'static str,
    u: User,
    mr: Option<(Project, MergeRequest)>,
}

impl UserObj {
    pub fn new(ty: &'static str, u: User) -> UserObj {
        UserObj { ty, u, mr: None }
    }
}

fn avatar(u: &User) -> String {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(u.email.trim().to_lowercase().as_bytes()));
    format!("https://www.gravatar.com/avatar/{hash}?s=80&d=identicon")
}

impl Object for UserObj {
    fn typename(&self) -> String {
        self.ty.into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let u = &self.u;
        let base = cx.req.app.config.external_url.trim_end_matches('/');
        Ok(match field {
            "recentlyViewedItems" => Out::List(vec![]),
            // Fields of the user's own settings and features: empty.
            "customAttributes" => {
                // Only administrators may read custom attributes.
                if cx.user().is_some_and(|me| me.admin) { Out::List(vec![]) } else { Out::Null }
            }
            "ide" => Out::obj(JsonObj::new("Ide", json!({"codeSuggestionsEnabled": false}))),
            "userPreferences" => Out::obj(JsonObj::new(
                "UserPreferences",
                json!({"issuesSort": null, "mergeRequestDashboardListType": "ACTION_BASED", "mergeRequestDashboardShowDrafts": true,
                       "projectsSort": null, "timezone": null, "useWorkItemsView": false, "visibilityPipelineIdType": "ID"}),
            )),
            "callouts" | "groupCallouts" | "namespaceCommitEmails" | "savedReplies" | "starredProjects" => {
                let conn = match field {
                    "callouts" => "UserCalloutConnection",
                    "groupCallouts" => "UserGroupCalloutConnection",
                    "namespaceCommitEmails" => "NamespaceCommitEmailConnection",
                    "savedReplies" => "SavedReplyConnection",
                    _ => "ProjectConnection",
                };
                Out::obj(Conn::new(conn, positions(Vec::<Shared>::new()), args)?)
            }
            "savedReply" => Out::Null,
            // `ContributedProjectsFinder`: projects with the user's contribution events of the last
            // year (pushes, comments; MRs created, closed, merged, approved, updated, deleted)
            // that the viewer can read; without `includePersonal` only those the user belongs to.
            // Newest activity first.
            "contributedProjects" => {
                let since = time::now() - 365 * 86_400_000;
                let mut st = cx.c.prepare(
                    "SELECT DISTINCT project_id FROM events WHERE author_id = ?1 AND created_at > ?2 AND project_id IS NOT NULL
                       AND (action IN ('pushed', 'commented')
                            OR (target_type IN ('MergeRequest', 'Issue', 'WorkItem') AND action IN ('created', 'closed', 'merged', 'approved', 'updated', 'destroyed')))",
                )?;
                let ids: Vec<i64> = st.query_map(rusqlite::params![u.id, since], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                let personal = args.bool("includePersonal").unwrap_or(false);
                let mut found: Vec<(Project, labrat_core::perms::ProjectPerms)> = vec![];
                for id in ids {
                    let Some(p) = Project::find(cx.c, id)? else { continue };
                    if !personal && ProjectPerms::load(cx.c, Some(u), &p)?.level == 0 {
                        continue;
                    }
                    if let Some(rp) = readable_project(cx, p)? {
                        found.push(rp);
                    }
                }
                found.sort_by(|a, b| b.0.last_activity_at.cmp(&a.0.last_activity_at).then(b.0.id.cmp(&a.0.id)));
                let items = found.into_iter().map(|(p, perms)| (id_cursor(p.id), Shared(Rc::new(ProjectObj { p, perms })))).collect();
                Out::obj(Conn::new("ProjectConnection", items, args)?)
            }
            // `TodosResolver`: only the current user's own to-dos.
            "todos" => {
                let mut items = vec![];
                if cx.user().is_some_and(|me| me.id == u.id) {
                    let list = |k: &str| -> Option<Vec<String>> {
                        match args.get(k)? {
                            Value::Array(a) => Some(a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()),
                            Value::String(s) => Some(vec![s.clone()]),
                            _ => None,
                        }
                    };
                    let states = list("state").unwrap_or_else(|| vec!["pending".into()]);
                    let types = list("type");
                    let actions = list("action");
                    // `by_snoozed_status`: `isSnoozed` lists only snoozed to-dos; a pending-only
                    // list leaves them out.
                    let snoozed_filter = args.bool("isSnoozed").unwrap_or(false);
                    let pending_only = states.iter().all(|s| s == "pending");
                    let now = time::now();
                    let mut st = cx.c.prepare("SELECT id, target_type, action, state, snoozed_until FROM todos WHERE user_id = ?1 ORDER BY id DESC")?;
                    let rows: Vec<(i64, String, i64, String, Option<i64>)> =
                        st.query_map([u.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
                    for (id, tt, action, state, snoozed_until) in rows {
                        let snoozed = snoozed_until.is_some_and(|t| t > now);
                        if (snoozed_filter && !snoozed) || (!snoozed_filter && pending_only && snoozed) {
                            continue;
                        }
                        if !states.contains(&state)
                            || types.as_ref().is_some_and(|t| !t.iter().any(|x| x.eq_ignore_ascii_case(&tt)))
                            || actions.as_ref().is_some_and(|a| !a.iter().any(|x| x == labrat_core::services::todos::action::name(action)))
                        {
                            continue;
                        }
                        items.push((id_cursor(id), Shared(Rc::new(TodoObj::load(cx.c, id)?))));
                    }
                }
                Out::obj(Conn::new("TodoConnection", items, args)?)
            }
            // Groups the user belongs to directly; the count includes groups of member projects.
            "groups" | "groupCount" | "projectCount" => {
                let mut st = cx.c.prepare("SELECT source_id FROM members WHERE source_type = 'Namespace' AND user_id = ?1 ORDER BY id")?;
                let direct: Vec<i64> = st.query_map([u.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                let mut st = cx.c.prepare("SELECT source_id FROM members WHERE source_type = 'Project' AND user_id = ?1")?;
                let projects: Vec<i64> = st.query_map([u.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                match field {
                    "projectCount" => Out::val(projects.len()),
                    "groupCount" => {
                        let mut all = direct.clone();
                        for pid in projects {
                            if let Some(p) = Project::find(cx.c, pid)?
                                && let Some(n) = Namespace::find(cx.c, p.namespace_id)?
                                && n.kind == "Group"
                                && !all.contains(&n.id)
                            {
                                all.push(n.id);
                            }
                        }
                        Out::val(all.len())
                    }
                    _ => {
                        let mut items = vec![];
                        for id in direct {
                            if let Some(n) = Namespace::find(cx.c, id)? {
                                items.push((id_cursor(id), Shared(Rc::new(NamespaceObj { ty: "Group", n }))));
                            }
                        }
                        Out::obj(Conn::new("GroupConnection", items, args)?)
                    }
                }
            }
            "id" => Out::val(gid("User", u.id)),
            "username" => Out::val(u.username.clone()),
            "name" => Out::val(u.name.clone()),
            "state" => Out::val(u.state.clone()),
            "avatarUrl" => Out::val(avatar(u)),
            "webUrl" => Out::val(format!("{base}/{}", u.username)),
            "webPath" => Out::val(format!("/{}", u.username)),
            "publicEmail" => Out::val(u.public_email.clone().filter(|e| !e.is_empty())),
            "bot" => Out::val(u.user_type != 0),
            "active" => Out::val(u.state == "active"),
            "locked" => Out::val(false),
            "location" => Out::val(u.location.clone()),
            "bio" => Out::val(u.bio.clone()),
            "discord" => Out::val(u.discord.clone()),
            "github" => Out::val(u.github.clone()),
            "linkedin" => Out::val(u.linkedin.clone()),
            "twitter" => Out::val(u.twitter.clone()),
            "human" => Out::val(u.user_type == 0),
            // `CurrentUser.admin`.
            "admin" => Out::val(u.admin),
            // `User.user_types`, upper-cased by `Types::Users::TypeEnum`.
            "type" => Out::val(match u.user_type {
                0 => "HUMAN",
                1 => "SUPPORT_BOT",
                2 => "ALERT_BOT",
                3 => "VISUAL_REVIEW_BOT",
                4 => "SERVICE_USER",
                5 => "GHOST",
                6 => "PROJECT_BOT",
                7 => "MIGRATION_BOT",
                8 => "SECURITY_BOT",
                9 => "AUTOMATION_BOT",
                11 => "ADMIN_BOT",
                12 => "SERVICE_ACCOUNT",
                _ => "HUMAN",
            }),
            "compositeIdentityEnforced" | "gitpodEnabled" => Out::val(false),
            "preferencesGitpodPath" | "profileEnableGitpodPath" => Out::Null,
            "jobTitle" => Out::val(u.job_title.clone()),
            "organization" => Out::val(u.organization.clone()),
            "pronouns" => Out::val(u.pronouns.clone()),
            "lastActivityOn" => Out::val(u.last_activity_on.clone()),
            "createdAt" => Out::val(gtime(u.created_at)),
            // `read_user_email_address`: the user themselves or an admin.
            // The raw column: unset (null) until a commit email other than the primary is chosen.
            "commitEmail" => Out::val(if cx.user().is_some_and(|me| me.id == u.id || me.admin) {
                u.commit_email.clone().filter(|e| !e.is_empty() && *e != u.email)
            } else {
                None
            }),
            "namespace" => match Namespace::find_by_full_path(cx.c, &u.username)? {
                Some(n) if cx.user().is_some_and(|me| me.id == u.id || me.admin) => Out::obj(NamespaceObj { ty: "Namespace", n }),
                _ => Out::Null,
            },
            "mergeRequestInteraction" => match &self.mr {
                Some((p, mr)) => Out::obj(Interaction { p: p.clone(), mr: mr.clone(), u: u.clone() }),
                None => Out::Null,
            },
            "status" => Out::Null,
            "emails" | "groupMemberships" | "projectMemberships" => return Err(not_implemented(self.ty, field)),
            "authoredMergeRequests" | "assignedMergeRequests" | "reviewRequestedMergeRequests" | "assigneeOrReviewerMergeRequests" => {
                let col = match field {
                    "authoredMergeRequests" => "author_id = ?1",
                    "assignedMergeRequests" => "id IN (SELECT merge_request_id FROM merge_request_assignees WHERE user_id = ?1)",
                    "assigneeOrReviewerMergeRequests" => {
                        "(id IN (SELECT merge_request_id FROM merge_request_assignees WHERE user_id = ?1) OR id IN (SELECT merge_request_id FROM merge_request_reviewers WHERE user_id = ?1))"
                    }
                    _ => "id IN (SELECT merge_request_id FROM merge_request_reviewers WHERE user_id = ?1)",
                };
                // The `ResolvesMergeRequests` arguments apply here too.
                let list = MergeRequest::list_where(cx.c, col, [u.id])?;
                let mut rows = vec![];
                for m in list {
                    if let Some(p) = Project::find(cx.c, m.target_project_id)? {
                        rows.push((p, m));
                    }
                }
                let rows = super::mr_filters::apply(cx, rows, args)?;
                mr_connection(cx, rows.into_iter().map(|(_, m)| m).collect(), args)?
            }
            _ => return Err(not_implemented(self.ty, field)),
        })
    }
}

/// `MergeRequestInteraction` of a reviewer or assignee.
struct Interaction {
    p: Project,
    mr: MergeRequest,
    u: User,
}

impl Object for Interaction {
    fn typename(&self) -> String {
        "UserMergeRequestInteraction".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let (p, mr, u) = (&self.p, &self.mr, &self.u);
        Ok(match field {
            "canMerge" => Out::val(mrs::can_be_merged_by(cx.c, p, mr, u)?),
            "canUpdate" => Out::val(mrs::can_update(cx.c, p, mr, u)?),
            "approved" => Out::val(reviews::has_approved(cx.c, mr, u)?),
            "reviewed" => {
                let st = mr.reviewers(cx.c)?.into_iter().find(|r| r.1 == u.id).map(|r| r.2);
                Out::val(st == Some(1))
            }
            "reviewState" => {
                let st = mr.reviewers(cx.c)?.into_iter().find(|r| r.1 == u.id).map(|r| r.2);
                Out::val(st.map(|s| {
                    match s {
                        1 => "REVIEWED",
                        2 => "REQUESTED_CHANGES",
                        3 => "APPROVED",
                        4 => "UNAPPROVED",
                        5 => "REVIEW_STARTED",
                        _ => "UNREVIEWED",
                    }
                }))
            }
            _ => return Err(not_implemented("UserMergeRequestInteraction", field)),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Namespaces and projects

pub struct NamespaceObj {
    ty: &'static str,
    n: Namespace,
}

impl NamespaceObj {
    pub fn group(n: Namespace) -> NamespaceObj {
        NamespaceObj { ty: "Group", n }
    }
}

impl Object for NamespaceObj {
    fn typename(&self) -> String {
        self.ty.into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let n = &self.n;
        let base = cx.req.app.config.external_url.trim_end_matches('/');
        if field == "mergeRequests" && n.kind == "Group" {
            // `GroupMergeRequestsResolver`: subgroups only with `includeSubgroups`.
            let ns = if args.bool("includeSubgroups").unwrap_or(false) { Namespace::self_and_descendant_ids(cx.c, n.id)? } else { vec![n.id] };
            let mut rows = crate::routes::merge_requests::visible_mrs(cx.c, cx.req, Some(&ns))?;
            if !args.bool("includeArchived").unwrap_or(false) {
                rows.retain(|(p, _)| !p.archived);
            }
            let rows = super::mr_filters::apply(cx, rows, args)?;
            return mr_connection(cx, rows.into_iter().map(|(_, m)| m).collect(), args);
        }
        Ok(match field {
            "id" => Out::val(gid(if n.kind == "Group" { "Group" } else { "Namespaces::UserNamespace" }, n.id)),
            "name" => Out::val(n.name.clone()),
            "path" => Out::val(n.path.clone()),
            "fullPath" => Out::val(n.full_path.clone()),
            "fullName" => Out::val(n.full_name.clone()),
            "description" => Out::val(n.description.clone().unwrap_or_default()),
            // Personal namespaces live at the user's path, groups under /groups/.
            "webUrl" => Out::val(if n.kind == "Group" { format!("{base}/groups/{}", n.full_path) } else { format!("{base}/{}", n.full_path) }),
            "visibility" => Out::val(labrat_core::model::visibility::name(n.visibility_level)),
            "avatarUrl" => Out::Null,
            "archived" => Out::val(false),
            "editPath" => Out::val(if n.kind == "Group" { format!("/groups/{}/-/edit", n.full_path) } else { format!("/{}/edit", n.full_path) }),
            "descendantGroups" | "descendantGroupsCount" => {
                let ids: Vec<i64> = Namespace::self_and_descendant_ids(cx.c, n.id)?.into_iter().filter(|i| *i != n.id).collect();
                if field == "descendantGroupsCount" {
                    Out::val(ids.len())
                } else {
                    let mut items = vec![];
                    for id in ids {
                        if let Some(d) = Namespace::find(cx.c, id)? {
                            items.push((id_cursor(id), Shared(Rc::new(NamespaceObj { ty: "Group", n: d }))));
                        }
                    }
                    Out::obj(Conn::new("GroupConnection", items, args)?)
                }
            }
            "groupMembers" => {
                let mut st = cx.c.prepare("SELECT id, user_id, access_level, expires_at, created_at FROM members WHERE source_type = 'Namespace' AND source_id = ?1 ORDER BY id DESC")?;
                let rows: Vec<(i64, i64, i64, Option<String>, i64)> = st.query_map([n.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
                let mut items = vec![];
                for (mid, uid, level, exp, created) in rows {
                    let Some(u) = User::find(cx.c, uid)? else { continue };
                    let name = match level {
                        50 => "OWNER",
                        40 => "MAINTAINER",
                        30 => "DEVELOPER",
                        20 => "REPORTER",
                        15 => "PLANNER",
                        10 => "GUEST",
                        _ => "MINIMAL_ACCESS",
                    };
                    items.push((id_cursor(mid), Shared(Rc::new(MemberObj { kind: "GroupMember", id: mid, user: u, level, level_name: name, expires_at: exp, created_at: created }))));
                }
                Out::obj(Conn::new("GroupMemberConnection", items, args)?)
            }
            "projects" => {
                let ns_ids = if args.bool("includeSubgroups").unwrap_or(false) { Namespace::self_and_descendant_ids(cx.c, n.id)? } else { vec![n.id] };
                let list = ns_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
                let mut items = vec![];
                for p in Project::list_where(cx.c, &format!("p.namespace_id IN ({list})"), [])? {
                    if let Some((p, perms)) = readable_project(cx, p)? {
                        items.push((id_cursor(p.id), Shared(Rc::new(ProjectObj { p, perms }))));
                    }
                }
                Out::obj(Conn::new("ProjectConnection", items, args)?)
            }
            _ => return Err(not_implemented(self.ty, field)),
        })
    }
}

pub struct ProjectObj {
    p: Project,
    perms: ProjectPerms,
}

impl Object for ProjectObj {
    fn typename(&self) -> String {
        "Project".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let p = &self.p;
        let base = cx.req.app.config.external_url.trim_end_matches('/');
        Ok(match field {
            // `AutocompleteUsersResolver`: direct and inherited members, by user id, matching
            // `search` in the username or name.
            "autocompleteUsers" => {
                let mut ids: Vec<i64> = labrat_core::services::members::list(cx.c, "Project", p.id)?.into_iter().map(|m| m.user_id).collect();
                for g in Namespace::self_and_ancestor_ids(cx.c, p.namespace_id)? {
                    ids.extend(labrat_core::services::members::list(cx.c, "Namespace", g)?.into_iter().map(|m| m.user_id));
                }
                ids.sort();
                ids.dedup();
                let q = args.str("search").map(|x| x.to_lowercase());
                let mut out = vec![];
                for id in ids {
                    let Some(u) = User::find(cx.c, id)? else { continue };
                    if !u.active() || q.as_ref().is_some_and(|q| !u.username.to_lowercase().contains(q) && !u.name.to_lowercase().contains(q)) {
                        continue;
                    }
                    out.push(Out::obj(UserObj::new("AutocompletedUser", u)));
                }
                Out::List(out)
            }
            "forkedFrom" => Out::Null,
            "rootGroup" => {
                let root = Namespace::self_and_ancestor_ids(cx.c, p.namespace_id)?.last().copied();
                match root.map(|id| Namespace::find(cx.c, id)).transpose()?.flatten().filter(|n| n.is_group()) {
                    Some(n) => Out::obj(NamespaceObj { ty: "Group", n }),
                    None => Out::Null,
                }
            }
            // `RepositoryLanguagesService`: byte shares of the default branch's programming
            // languages, largest first, rounded to two decimals.
            "languages" => {
                let r = repo::repo(cx.c, p);
                let mut sizes: Vec<(&'static str, &'static str, u64)> = vec![];
                if let Some(head) = p.default_branch.as_deref().and_then(|b| r.branch(b).ok().flatten()) {
                    let tree = r.commit(&head)?.tree;
                    for (path, (mode, oid)) in labrat_git::diff::flatten(&r, &tree)? {
                        if mode == 0o160000 || mode == 0o120000 {
                            continue;
                        }
                        let Some((name, color)) = language_of(&path) else { continue };
                        let len = r.blob(&oid)?.len() as u64;
                        match sizes.iter_mut().find(|x| x.0 == name) {
                            Some(x) => x.2 += len,
                            None => sizes.push((name, color, len)),
                        }
                    }
                }
                let total: u64 = sizes.iter().map(|x| x.2).sum();
                sizes.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(b.0)));
                Out::List(
                    sizes
                        .into_iter()
                        .map(|(name, color, len)| {
                            let share = (len as f64 * 100.0 / total as f64 * 100.0).round() / 100.0;
                            Out::obj(JsonObj::new("RepositoryLanguage", json!({"name": name, "share": share, "color": color})))
                        })
                        .collect(),
                )
            }
            "id" => Out::val(gid("Project", p.id)),
            "name" => Out::val(p.name.clone()),
            "path" => Out::val(p.path.clone()),
            "fullPath" => Out::val(p.full_path.clone()),
            "nameWithNamespace" => Out::val(p.full_name.clone()),
            "description" => Out::val(p.description.clone()),
            "webUrl" => Out::val(format!("{base}/{}", p.full_path)),
            "httpUrlToRepo" => Out::val(format!("{base}/{}.git", p.full_path)),
            "visibility" => Out::val(labrat_core::model::visibility::name(p.visibility_level)),
            "archived" => Out::val(p.archived),
            "createdAt" => Out::val(gtime(p.created_at)),
            "mergeRequest" => {
                if !self.perms.read_merge_request(p) {
                    return Ok(Out::Null);
                }
                let iid: i64 = args.str("iid").unwrap_or_default().parse().unwrap_or(-1);
                Out::opt(MergeRequest::find_by_iid(cx.c, p.id, iid)?.map(|m| MrObj::new(p.clone(), m)))
            }
            "mergeRequests" => {
                if !self.perms.read_merge_request(p) {
                    return Ok(Out::Null);
                }
                let rows = MergeRequest::list_where(cx.c, "target_project_id = ?1", [p.id])?.into_iter().map(|m| (p.clone(), m)).collect();
                let rows = super::mr_filters::apply(cx, rows, args)?;
                mr_connection(cx, rows.into_iter().map(|(_, m)| m).collect(), args)?
            }
            "repository" => Out::obj(RepositoryObj { p: p.clone() }),
            "labels" => {
                let all = labrat_core::services::labels::available(cx.c, p)?;
                let q = args.str("searchTerm").map(|s| s.to_lowercase());
                let items = all
                    .into_iter()
                    .filter(|l| q.as_ref().is_none_or(|q| l.title.to_lowercase().contains(q)))
                    .map(|l| (id_cursor(l.id), Shared(Rc::new(LabelObj { l }))))
                    .collect();
                Out::obj(Conn::new("LabelConnection", items, args)?)
            }
            "milestones" => {
                let mut st = cx.c.prepare("SELECT id FROM milestones WHERE project_id = ?1 ORDER BY due_date IS NULL, due_date, id")?;
                let ids: Vec<i64> = st.query_map([p.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                let mut items = vec![];
                for id in ids {
                    if let Out::Obj(o) = milestone_obj(cx, Some(id))? {
                        items.push((id_cursor(id), Shared(Rc::from(o))));
                    }
                }
                Out::obj(Conn::new("MilestoneConnection", items, args)?)
            }
            "group" | "namespace" => {
                let ns = Namespace::find(cx.c, p.namespace_id)?;
                Out::opt(ns.filter(|n| field == "namespace" || n.kind == "Group").map(|n| NamespaceObj { ty: if n.kind == "Group" { "Group" } else { "Namespace" }, n }))
            }
            "mergeRequestsEnabled" => Out::val(self.perms.read_merge_request(p)),
            "removeSourceBranchAfterMerge" => Out::val(p.remove_source_branch_after_merge),
            "onlyAllowMergeIfAllDiscussionsAreResolved" => Out::val(p.only_allow_merge_if_all_discussions_are_resolved),
            "mergeRequestsDisableCommittersApproval" if cx.req.app.config.is_ee() => {
                Out::val(!labrat_core::services::approvals::effective(cx.c, p)?.allow_committer_approval.value)
            }
            "onlyAllowMergeIfPipelineSucceeds" => Out::val(p.only_allow_merge_if_pipeline_succeeds),
            "mergeRequestsFfOnlyEnabled" => Out::val(p.merge_method == "ff"),
            "allowMergeOnSkippedPipeline" => Out::val(p.allow_merge_on_skipped_pipeline),
            // One assignee and one reviewer.
            "allowsMultipleMergeRequestAssignees" | "allowsMultipleMergeRequestReviewers" | "serviceDeskEnabled" => Out::val(false),
            "avatarUrl" => Out::Null,
            "autocloseReferencedIssues" | "lfsEnabled" | "sharedRunnersEnabled" => Out::val(true),
            "ciConfigPathOrDefault" => Out::val(p.setting("ci_config_path").and_then(|v| v.as_str().map(str::to_string)).filter(|v| !v.is_empty()).unwrap_or(".gitlab-ci.yml".into())),
            "containerRepositoriesCount" => Out::val(0),
            "isForked" | "pagesForceHttps" | "pagesUseUniqueDomain" => Out::val(false),
            "importStatus" => Out::val("none"),
            "jiraImportStatus" => Out::val("initial"),
            "publicJobs" => Out::val(true),
            "serviceDeskAddress" => Out::Null,
            "descriptionHtml" => Out::val(crate::markdown::render_gfm(cx.c, p, p.description.as_deref().unwrap_or(""), p.updated_at)),
            // Admin-area links, for administrators only.
            "adminEditPath" | "adminShowPath" => Out::val(cx.user().filter(|u| u.admin).map(|_| {
                if field == "adminEditPath" { format!("/admin/projects/{}/edit", p.full_path) } else { format!("/admin/projects/{}", p.full_path) }
            })),
            "editPath" => Out::val(format!("/{}/edit", p.full_path)),
            "webPath" => Out::val(format!("/{}", p.full_path)),
            "sshUrlToRepo" => Out::val(cx.req.app.config.ssh_url(&p.full_path)),
            "forksCount" | "starCount" | "openIssuesCount" => Out::val(0),
            "openMergeRequestsCount" => {
                let n: i64 = cx.c.query_row("SELECT count(*) FROM merge_requests WHERE target_project_id = ?1 AND state = 'opened'", [p.id], |r| r.get(0))?;
                Out::val(n)
            }
            "issuesEnabled" => Out::val(p.feature_level("issues") != labrat_core::model::feature::DISABLED),
            "jobsEnabled" => Out::val(p.feature_level("builds") != labrat_core::model::feature::DISABLED),
            "wikiEnabled" => Out::val(p.feature_level("wiki") != labrat_core::model::feature::DISABLED),
            "snippetsEnabled" => Out::val(p.feature_level("snippets") != labrat_core::model::feature::DISABLED),
            "containerRegistryEnabled" => Out::val(p.feature_level("container_registry") != labrat_core::model::feature::DISABLED),
            "mergeRequestsAccessLevel" => {
                let level = p.feature_level("merge_requests");
                Out::obj(JsonObj::new("ProjectFeatureAccess", json!({"stringValue": labrat_core::model::feature::name(level).to_uppercase(), "integerValue": level})))
            }
            "printingMergeRequestLinkEnabled" => Out::val(p.printing_merge_request_link_enabled),
            "requestAccessEnabled" => Out::val(p.request_access_enabled),
            "mergeCommitTemplate" => Out::val(p.merge_commit_template.clone()),
            "squashCommitTemplate" => Out::val(p.squash_commit_template.clone()),
            "suggestionCommitMessage" => Out::val(p.suggestion_commit_message.clone()),
            "squashReadOnly" => Out::val(matches!(p.squash_option.as_str(), "always" | "never")),
            "topics" => Out::val(p.topics(cx.c)?),
            "lastActivityAt" => Out::val(gtime(p.last_activity_at)),
            "updatedAt" => Out::val(gtime(p.updated_at)),
            "maxAccessLevel" => {
                let level = self.perms.level;
                let name = match level {
                    50 => "OWNER",
                    40 => "MAINTAINER",
                    30 => "DEVELOPER",
                    20 => "REPORTER",
                    15 => "PLANNER",
                    10 => "GUEST",
                    5 => "MINIMAL_ACCESS",
                    _ => "NO_ACCESS",
                };
                Out::obj(JsonObj::new("AccessLevel", json!({"integerValue": level, "stringValue": name})))
            }
            // Direct members and those inherited from the groups above, newest membership first.
            "projectMembers" => {
                let mut sources: Vec<(String, i64)> = vec![("Project".into(), p.id)];
                let mut ns = Namespace::find(cx.c, p.namespace_id)?;
                while let Some(n) = ns {
                    if n.kind == "Group" {
                        sources.push(("Namespace".into(), n.id));
                    }
                    ns = match n.parent_id {
                        Some(pid) => Namespace::find(cx.c, pid)?,
                        None => None,
                    };
                }
                let mut rows: Vec<(i64, String, i64, i64, Option<String>, i64)> = vec![];
                for (ty, id) in &sources {
                    let mut st = cx.c.prepare("SELECT id, user_id, access_level, expires_at, created_at FROM members WHERE source_type = ?1 AND source_id = ?2")?;
                    let found: Vec<(i64, i64, i64, Option<String>, i64)> =
                        st.query_map(rusqlite::params![ty, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
                    for (mid, uid, level, exp, created) in found {
                        if !rows.iter().any(|r| r.2 == uid) {
                            rows.push((mid, ty.clone(), uid, level, exp, created));
                        }
                    }
                }
                rows.sort_by(|a, b| b.0.cmp(&a.0));
                let mut items = vec![];
                for (mid, ty, uid, level, exp, created) in rows {
                    let Some(u) = User::find(cx.c, uid)? else { continue };
                    let name = match level {
                        50 => "OWNER",
                        40 => "MAINTAINER",
                        30 => "DEVELOPER",
                        20 => "REPORTER",
                        15 => "PLANNER",
                        10 => "GUEST",
                        _ => "MINIMAL_ACCESS",
                    };
                    let kind = if ty == "Project" { "ProjectMember" } else { "GroupMember" };
                    items.push((
                        id_cursor(mid),
                        Shared(Rc::new(MemberObj { kind, id: mid, user: u, level, level_name: name, expires_at: exp, created_at: created })),
                    ));
                }
                Out::obj(Conn::new("MemberInterfaceConnection", items, args)?)
            }
            "pipelines" => {
                let mut list = labrat_core::services::ci::Pipeline::list(cx.c, p.id)?;
                list.sort_by(|a, b| b.id.cmp(&a.id));
                let refs = match args.str("ref") {
                    Some(r) => Some(labrat_core::services::ci::refs_for_filter(cx.c, p, &r, args.str("source").as_deref())?),
                    None => None,
                };
                let items = list
                    .into_iter()
                    .filter(|pl| args.str("status").is_none_or(|st| st.eq_ignore_ascii_case(&pl.status)))
                    .filter(|pl| refs.as_ref().is_none_or(|rs| pl.ref_.as_ref().is_some_and(|r| rs.contains(r))))
                    .filter(|pl| args.str("sha").is_none_or(|h| pl.sha == h))
                    .filter(|pl| args.str("source").is_none_or(|src| src.eq_ignore_ascii_case(&pl.source)))
                    .map(|pl| (id_cursor(pl.id), Shared(Rc::new(PipelineObj { p: p.clone(), pl }))))
                    .collect();
                Out::obj(Conn::with_max("PipelineConnection", items, args, 100)?)
            }
            "pipeline" => {
                let list = labrat_core::services::ci::Pipeline::list(cx.c, p.id)?;
                let id = args.str("id").and_then(|g| g.rsplit('/').next().and_then(|x| x.parse::<i64>().ok()));
                let iid = args.str("iid").and_then(|x| x.parse::<i64>().ok());
                let sha = args.str("sha");
                let found = list.into_iter().filter(|pl| {
                    id.is_none_or(|i| pl.id == i) && iid.is_none_or(|i| pl.iid == i) && sha.as_ref().is_none_or(|h| pl.sha == *h)
                }).max_by_key(|pl| pl.id);
                Out::opt(found.map(|pl| PipelineObj { p: p.clone(), pl }))
            }
            // "All branches" first, then each protection rule.
            "branchRules" => {
                let r = repo::repo(cx.c, p);
                let branches: Vec<String> = r.branches()?.into_iter().map(|(n, _)| n).collect();
                let mut items = vec![Shared(Rc::new(AllBranchesObj { p: p.clone(), count: branches.len() }))];
                for pb in labrat_core::repo::protected_branches(cx.c, p.id)? {
                    let count = branches.iter().filter(|b| labrat_core::repo::ref_matches(&pb.name, b)).count();
                    items.push(Shared(Rc::new(BranchRuleObj { pb, count, default: p.default_branch.clone(), p: p.clone() })));
                }
                Out::obj(Conn::new("BranchRuleConnection", positions(items), args)?)
            }
            // Recorded per role (graphql_permissions_all): each field needs one of these tiers.
            "userPermissions" => {
                let perms = &self.perms;
                let signed_in = cx.user().is_some();
                let admin = cx.user().is_some_and(|u| u.admin);
                let owner = perms.level >= 50 || admin;
                let maint = signed_in && perms.admin_project();
                let dev = signed_in && perms.developer();
                let rep = signed_in && perms.reporter();
                let member = signed_in && perms.read_project();
                // A member who isn't the owner can leave.
                let can_leave = signed_in && perms.level > 0 && perms.level < 50;
                let mut m = serde_json::Map::new();
                    m.insert("createLabel".into(), json!(false));
                    m.insert("createPages".into(), json!(false));
                    m.insert("destroyPages".into(), json!(false));
                    m.insert("destroyWiki".into(), json!(false));
                    m.insert("readCrmContact".into(), json!(false));
                    m.insert("readCrmOrganization".into(), json!(false));
                    m.insert("requestAccess".into(), json!(false));
                    m.insert("updateWiki".into(), json!(false));
                    m.insert("adminAllResources".into(), json!(owner));
                    m.insert("archiveProject".into(), json!(owner));
                    m.insert("changeNamespace".into(), json!(owner));
                    m.insert("changeVisibilityLevel".into(), json!(owner));
                    m.insert("removeForkProject".into(), json!(owner));
                    m.insert("removeProject".into(), json!(owner));
                    m.insert("adminOperations".into(), json!(maint));
                    m.insert("adminProject".into(), json!(maint));
                    m.insert("adminRemoteMirror".into(), json!(maint));
                    m.insert("adminWiki".into(), json!(maint));
                    m.insert("pushToDeleteProtectedBranch".into(), json!(maint));
                    m.insert("removePages".into(), json!(maint));
                    m.insert("renameProject".into(), json!(maint));
                    m.insert("updatePages".into(), json!(maint));
                    m.insert("viewEditPage".into(), json!(maint));
                    m.insert("createDeployment".into(), json!(dev && !p.archived));
                    m.insert("createMergeRequestFrom".into(), json!(dev && !p.archived));
                    m.insert("createPipeline".into(), json!(dev && !p.archived));
                    m.insert("createPipelineSchedule".into(), json!(dev && !p.archived));
                    m.insert("createWiki".into(), json!(dev && !p.archived));
                    m.insert("importIssues".into(), json!(dev));
                    m.insert("pushCode".into(), json!(dev && !p.archived));
                    m.insert("adminIssue".into(), json!(rep));
                    m.insert("createDesign".into(), json!(rep));
                    m.insert("createSnippet".into(), json!(rep));
                    m.insert("destroyDesign".into(), json!(rep));
                    m.insert("moveDesign".into(), json!(rep));
                    m.insert("updateDesign".into(), json!(rep));
                    m.insert("createIssue".into(), json!(member && !p.archived));
                    m.insert("createMergeRequestIn".into(), json!(member && !p.archived));
                    m.insert("createWorkItem".into(), json!(member && !p.archived));
                    m.insert("forkProject".into(), json!(member));
                    m.insert("readPagesContent".into(), json!(member));
                    m.insert("uploadFile".into(), json!(member && !p.archived));
                    m.insert("downloadCode".into(), json!(perms.read_code(p)));
                    m.insert("downloadWikiCode".into(), json!(perms.read_code(p)));
                    m.insert("readCommitStatus".into(), json!(perms.read_code(p)));
                    m.insert("readCycleAnalytics".into(), json!(perms.read_project()));
                    m.insert("readDesign".into(), json!(perms.read_project()));
                    m.insert("readEnvironment".into(), json!(perms.read_project()));
                    m.insert("readMergeRequest".into(), json!(perms.read_project()));
                    m.insert("readProject".into(), json!(perms.read_project()));
                    m.insert("readProjectMember".into(), json!(perms.read_project()));
                    m.insert("readWiki".into(), json!(perms.read_project()));
                    m.insert("canLeave".into(), json!(can_leave));
                Out::obj(JsonObj::new("ProjectPermissions", Value::Object(m)))
            }
            _ => return Err(not_implemented("Project", field)),
        })
    }
}

fn mr_connection(cx: &Ctx, list: Vec<MergeRequest>, args: &Args) -> FResult<Out> {
    let mut objs = vec![];
    for m in list {
        let p = Project::find(cx.c, m.target_project_id)?.expect("project exists");
        if let Some((p, perms)) = readable_project(cx, p)?
            && perms.read_merge_request(&p) {
                objs.push((created_cursor(m.created_at, m.id), Shared(Rc::new(MrObj::new(p, m)))));
            }
    }
    Ok(Out::obj(Conn::new("MergeRequestConnection", objs, args)?))
}

struct RepositoryObj {
    p: Project,
}

impl Object for RepositoryObj {
    fn typename(&self) -> String {
        "Repository".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let r = repo::repo(cx.c, &self.p);
        Ok(match field {
            "empty" => Out::val(r.is_empty()?),
            "exists" => Out::val(true),
            "rootRef" => Out::val(self.p.default_branch.clone()),
            "tree" => {
                let rf = args.str("ref").or(self.p.default_branch.clone()).unwrap_or_default();
                let path = args.str("path").unwrap_or_default();
                let Some(head) = r.resolve(&rf)? else { return Ok(Out::Null) };
                let root = r.commit(&head)?.tree;
                let tree = if path.is_empty() || path == "/" {
                    Some(root)
                } else {
                    r.entry_at(&root, path.trim_matches('/'))?.filter(|e| e.mode == 0o40000).map(|e| e.oid)
                };
                match tree {
                    Some(t) => Out::obj(TreeObj { p: self.p.clone(), rf, dir: path.trim_matches('/').to_string(), entries: r.tree(&t)? }),
                    None => Out::Null,
                }
            }
            "blobs" => {
                let rf = args.str("ref").or(self.p.default_branch.clone()).unwrap_or_default();
                let mut items = vec![];
                if let Some(head) = r.resolve(&rf)? {
                    let root = r.commit(&head)?.tree;
                    let paths: Vec<String> = args.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                    for path in paths {
                        if let Some(e) = r.entry_at(&root, &path)?.filter(|e| e.mode != 0o40000) {
                            let data = r.blob(&e.oid)?;
                            items.push(Shared(Rc::new(BlobObj { p: self.p.clone(), rf: rf.clone(), path, mode: e.mode, oid: e.oid, data })));
                        }
                    }
                }
                Out::obj(Conn::new("RepositoryBlobConnection", positions(items), args)?)
            }
            "commit" => {
                let rf = args.str("ref").or(self.p.default_branch.clone()).unwrap_or_default();
                match r.resolve(&rf)? {
                    Some(o) => Out::obj(CommitObj { p: self.p.clone(), oid: o, c: r.commit(&o)? }),
                    None => Out::Null,
                }
            }
            // The newest commit (first-parent history) that changed `path`.
            "lastCommit" => {
                let rf = args.str("ref").or(self.p.default_branch.clone()).unwrap_or_default();
                let path = args.str("path").unwrap_or_default();
                match last_commit(&r, &rf, &path)? {
                    Some((oid, cm)) => Out::obj(CommitObj { p: self.p.clone(), oid, c: cm }),
                    None => Out::Null,
                }
            }
            "branchNames" => {
                let pattern = args.str("searchPattern").unwrap_or("*".into());
                let offset = args.int("offset").unwrap_or(0).max(0) as usize;
                let limit = args.int("limit").unwrap_or(20).max(0) as usize;
                let names: Vec<String> = r
                    .branches()?
                    .into_iter()
                    .map(|(n, _)| n)
                    .filter(|n| labrat_core::repo::ref_matches(&pattern, n))
                    .skip(offset)
                    .take(limit)
                    .collect();
                Out::val(names)
            }
            _ => return Err(not_implemented("Repository", field)),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Merge requests

pub struct MrObj {
    p: Project,
    mr: MergeRequest,
}

impl MrObj {
    pub fn new(p: Project, mr: MergeRequest) -> MrObj {
        MrObj { p, mr }
    }
}

fn user_list(cx: &Ctx, ids: &[i64], ty: &'static str, ctx_mr: Option<(&Project, &MergeRequest)>) -> FResult<Vec<Shared>> {
    let mut out = vec![];
    for id in ids {
        if let Some(u) = User::find(cx.c, *id)? {
            let mut o = UserObj::new(ty, u);
            o.mr = ctx_mr.map(|(p, m)| (p.clone(), m.clone()));
            out.push(Shared(Rc::new(o)));
        }
    }
    Ok(out)
}

pub(crate) fn user_conn(cx: &Ctx, ids: &[i64], conn_ty: &str, node_ty: &'static str, ctx_mr: Option<(&Project, &MergeRequest)>, args: &Args) -> FResult<Out> {
    let list = user_list(cx, ids, node_ty, ctx_mr)?;
    let items = ids.iter().map(|i| id_cursor(*i)).zip(list).collect();
    Ok(Out::obj(Conn::new(conn_ty, items, args)?))
}

impl Object for MrObj {
    fn typename(&self) -> String {
        "MergeRequest".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let (p, mr) = (&self.p, &self.mr);
        let c = cx.c;
        let base = cx.req.app.config.external_url.trim_end_matches('/');
        let diff = mr.latest_diff(c)?;
        Ok(match field {
            "id" => Out::val(gid("MergeRequest", mr.id)),
            "iid" => Out::val(mr.iid.to_string()),
            "title" => Out::val(mr.title.clone()),
            "titleHtml" => Out::val(crate::markdown::render_inline(&mr.title)),
            "description" => Out::val(mr.description.clone()),
            "descriptionHtml" => Out::val(
                mr.description
                    .as_deref()
                    .map(|d| crate::markdown::render_gfm(c, p, d, mr.last_edited_at.unwrap_or(mr.created_at)))
                    .unwrap_or_default(),
            ),
            "state" => Out::val(mr.state.clone()),
            "draft" => Out::val(mr.is_draft()),
            "createdAt" => Out::val(gtime(mr.created_at)),
            "updatedAt" => Out::val(gtime(mr.updated_at)),
            "mergedAt" => Out::val(opt_time(mr.merged_at)),
            "closedAt" => Out::val(opt_time(mr.latest_closed_at)),
            "preparedAt" => Out::val(opt_time(mr.prepared_at)),
            "sourceBranch" => Out::val(mr.source_branch.clone()),
            "targetBranch" => Out::val(mr.target_branch.clone()),
            "sourceBranchExists" => Out::val(repo::repo(c, p).branch(&mr.source_branch)?.is_some()),
            "targetBranchExists" => Out::val(repo::repo(c, p).branch(&mr.target_branch)?.is_some()),
            "sourceProjectId" => Out::val(mr.source_project_id),
            "targetProjectId" => Out::val(mr.target_project_id),
            "sourceProject" | "targetProject" | "project" => {
                let perms = ProjectPerms::load(c, cx.user(), p)?;
                Out::obj(ProjectObj { p: p.clone(), perms })
            }
            "webUrl" => Out::val(mr.web_url(&cx.req.app.config, p)),
            "webPath" => Out::val(format!("/{}/-/merge_requests/{}", p.full_path, mr.iid)),
            "reference" => Out::val(if args.bool("full").unwrap_or(false) { format!("{}!{}", p.full_path, mr.iid) } else { format!("!{}", mr.iid) }),
            "name" => Out::val(mr.title.clone()),
            "sourceBranchProtected" => Out::val(repo::is_protected(c, p, &mr.source_branch)?),
            "targetBranchPath" => Out::val(format!("/{}/-/commits/{}", p.full_path, mr.target_branch)),
            "allowsMultipleAssignees" | "allowsMultipleReviewers" | "supportsLockOnMerge" => Out::val(false),
            "pipelineCreationRequests" => Out::List(vec![]),
            // Relations without an order are paginated by id, newest first.
            "commenters" => {
                let mut ids: Vec<i64> = vec![];
                for n in Note::for_noteable(c, "MergeRequest", mr.id)? {
                    if !n.system && note_visible(cx, p, &n) && !ids.contains(&n.author_id) {
                        ids.push(n.author_id);
                    }
                }
                ids.sort_by(|a, b| b.cmp(a));
                user_conn(cx, &ids, "UserCoreConnection", "UserCore", None, args)?
            }
            "committers" => {
                let r = repo::repo(c, p);
                let mut ids: Vec<i64> = vec![];
                if let Some(d) = &diff {
                    for oid in d.commits(&r)? {
                        let email = r.commit(&oid)?.committer.email.clone();
                        let id: Option<i64> = c
                            .query_row("SELECT id FROM users WHERE lower(email) = lower(?1)", [&email], |r| r.get(0))
                            .ok();
                        if let Some(id) = id.filter(|i| !ids.contains(i)) {
                            ids.push(id);
                        }
                    }
                }
                ids.sort_by(|a, b| b.cmp(a));
                user_conn(cx, &ids, "UserCoreConnection", "UserCore", None, args)?
            }
            "currentUserTodos" => {
                let mut items = vec![];
                if let Some(u) = cx.user() {
                    let state = args.str("state");
                    let mut st = c.prepare("SELECT id FROM todos WHERE user_id = ?1 AND target_type = 'MergeRequest' AND target_id = ?2 ORDER BY id DESC")?;
                    let ids: Vec<i64> = st.query_map(rusqlite::params![u.id, mr.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                    for id in ids {
                        let t = TodoObj::load(c, id)?;
                        if state.as_ref().is_some_and(|s| *s != t.state) {
                            continue;
                        }
                        items.push((id_cursor(id), Shared(Rc::new(t))));
                    }
                }
                Out::obj(Conn::new("TodoConnection", items, args)?)
            }
            "sha" | "diffHeadSha" => Out::val(mr.diff_head_sha(c, p)?),
            "mergeCommitSha" => Out::val(mr.merge_commit_sha.clone()),
            "squash" => Out::val(mr.squash),
            "squashOnMerge" => Out::val(labrat_core::services::merge_requests::merge::squash_on_merge(p, mr)),
            "shouldRemoveSourceBranch" => Out::val(mr.should_remove_source_branch()),
            "forceRemoveSourceBranch" => Out::val(mr.force_remove_source_branch()),
            "discussionLocked" => Out::val(mr.discussion_locked == Some(true)),
            "mergeWhenPipelineSucceeds" | "autoMergeEnabled" => Out::val(mr.merge_when_pipeline_succeeds),
            "timeEstimate" => Out::val(mr.time_estimate),
            "totalTimeSpent" => Out::val(mr.total_time_spent),
            "humanTimeEstimate" | "humanTotalTimeSpent" => {
                let v = if field == "humanTimeEstimate" { mr.time_estimate } else { mr.total_time_spent };
                Out::val(crate::routes::merge_requests::human_duration(v))
            }
            "mergeError" => Out::val(mr.merge_error.clone()),
            // `public_merge_status`: preparing and rechecking show as checking.
            "mergeStatus" => Out::val(mergeability::public_status(&mr.merge_status)),
            "mergeStatusEnum" => Out::val(mergeability::public_status(&mr.merge_status).to_uppercase()),
            "detailedMergeStatus" => Out::val(mergeability::detailed_status(c, &cx.req.app.config, p, mr)?.to_uppercase()),
            "mergeable" => Out::val(mergeability::mergeable(c, &cx.req.app.config, p, mr)?),
            "conflicts" => Out::val(mr.merge_status == "cannot_be_merged"),
            "mergeableDiscussionsState" => Out::val(mergeability::blocking_discussions_resolved(c, p, mr)?),
            // EE: the approval rules are satisfied (true with no rules); CE: someone approved.
            "approved" => Out::val(if cx.req.app.config.is_ee() {
                labrat_core::services::approvals::state(c, p, mr)?.approved()
            } else {
                !reviews::approvals(c, mr)?.is_empty()
            }),
            // `approved_by_users`: a users relation, paginated by id, newest first.
            "approvedBy" => {
                let mut ids: Vec<i64> = reviews::approvals(c, mr)?.into_iter().map(|(u, _)| u).collect();
                ids.sort_by(|a, b| b.cmp(a));
                user_conn(cx, &ids, "UserCoreConnection", "UserCore", None, args)?
            }
            "author" => Out::opt(User::find(c, mr.author_id)?.map(|u| UserObj { ty: "MergeRequestAuthor", u, mr: Some((p.clone(), mr.clone())) })),
            // `metrics.merged_by || merge_user`.
            "mergeUser" => Out::opt(mr.merged_by_id.or(mr.merge_user_id).map(|id| User::find(c, id)).transpose()?.flatten().map(|u| UserObj::new("UserCore", u))),
            "assignees" => user_conn(cx, &mr.assignee_ids(c)?, "MergeRequestAssigneeConnection", "MergeRequestAssignee", Some((p, mr)), args)?,
            "reviewers" => user_conn(cx, &mr.reviewer_ids(c)?, "MergeRequestReviewerConnection", "MergeRequestReviewer", Some((p, mr)), args)?,
            "participants" => {
                let visible = labrat_core::services::subscriptions::participant_ids(c, p, mr, cx.user())?;
                user_conn(cx, &visible, "MergeRequestParticipantConnection", "MergeRequestParticipant", Some((p, mr)), args)?
            }
            "labels" => {
                let ls = mr.labels(c)?.into_iter().map(|l| (id_cursor(l.id), Shared(Rc::new(LabelObj { l })))).collect();
                Out::obj(Conn::new("LabelConnection", ls, args)?)
            }
            "milestone" => milestone_obj(cx, mr.milestone_id)?,
            "commits" | "commitsWithoutMergeCommits" => {
                let r = repo::repo(c, p);
                let mut list = vec![];
                if let Some(d) = &diff {
                    for oid in d.commits(&r)? {
                        let cm = r.commit(&oid)?;
                        let merge = cm.parents.len() > 1;
                        if field == "commitsWithoutMergeCommits" && merge {
                            continue;
                        }
                        list.push(Shared(Rc::new(CommitObj { p: p.clone(), oid, c: cm })));
                    }
                }
                Out::obj(Conn::new("CommitConnection", positions(list), args)?)
            }
            "diffStats" => match &diff {
                Some(d) => {
                    let r = repo::repo(c, p);
                    let (Some(b), Some(h)) = (d.base_commit_sha.clone(), d.head_commit_sha.clone()) else { return Ok(Out::List(vec![])) };
                    let bt = r.commit(&labrat_git::Oid::from_hex(&b)?)?.tree;
                    let ht = r.commit(&labrat_git::Oid::from_hex(&h)?)?.tree;
                    let mut changes = labrat_git::diff::diff_trees(&r, Some(&bt), Some(&ht))?;
                    // Gitaly's `DiffStats` keeps git's order: bytewise by path, not directories first.
                    changes.sort_by(|a, b| a.new_path.as_bytes().cmp(b.new_path.as_bytes()));
                    let want = args.str("path");
                    let mut out = vec![];
                    for ch in &changes {
                        if want.as_ref().is_some_and(|w| *w != ch.new_path) {
                            continue;
                        }
                        let (a, dl) = labrat_git::diff::line_stats(&r, ch)?;
                        out.push(Out::obj(JsonObj::new("DiffStats", json!({"path": ch.new_path, "additions": a, "deletions": dl}))));
                    }
                    Out::List(out)
                }
                None => Out::Null,
            },
            "resolvableDiscussionsCount" | "resolvedDiscussionsCount" => {
                let notes = Note::for_noteable(c, "MergeRequest", mr.id)?;
                let mut ids: Vec<(String, bool)> = vec![];
                for n in notes.iter().filter(|n| n.resolvable && !n.system) {
                    match ids.iter_mut().find(|(d, _)| *d == n.discussion_id) {
                        Some((_, r)) => *r &= n.is_resolved(),
                        None => ids.push((n.discussion_id.clone(), n.is_resolved())),
                    }
                }
                Out::val(if field == "resolvableDiscussionsCount" { ids.len() } else { ids.iter().filter(|x| x.1).count() })
            }
            "projectId" => Out::val(mr.target_project_id),
            // `diverged_commits_count > 0`: the target has commits the source doesn't.
            "divergedFromTargetBranch" => {
                let r = repo::repo(c, p);
                let target = r.branch(&mr.target_branch)?;
                // `source_branch_sha`: the live branch head (nil once the branch is gone).
                let source = r.branch(&mr.source_branch)?;
                Out::val(match (target, source) {
                    // `diverged_commits_count > 0`, in any state, while both branches exist.
                    (Some(t), Some(s)) => !labrat_git::walk::is_ancestor(&r, &t, &s)?,
                    _ => false,
                })
            }
            "mergeOngoing" => Out::val(mr.state == "locked"),
            "squashReadOnly" => Out::val(matches!(p.squash_option.as_str(), "always" | "never")),
            "autoMergeStrategy" => Out::val(if mr.merge_when_pipeline_succeeds { Some("merge_when_checks_pass") } else { None }),
            // `AutoMergeService#available_strategies`: merge-when-checks-pass, when the user can
            // merge, nothing auto-merge waits on is blocking, and the MR can't simply merge now
            // (a pipeline still running, a draft, open threads, ...).
            "availableAutoMergeStrategies" => {
                let available = match cx.user() {
                    Some(u) if mr.is_open() => {
                        let dhp = labrat_core::services::ci::diff_head_pipeline(c, p, mr)?;
                        mrs::can_be_merged_by(c, p, mr, u)?
                            && mergeability::auto_merge_blocking(c, p, mr)?.is_none()
                            && !(mergeability::mergeable(c, &cx.req.app.config, p, mr)? && !labrat_core::services::ci::in_progress(p, dhp.as_ref()))
                    }
                    _ => false,
                };
                Out::val(if available { vec!["merge_when_checks_pass"] } else { vec![] })
            }
            "retargeted" => Out::val(c.query_row("SELECT retargeted FROM merge_requests WHERE id = ?1", [mr.id], |r| r.get::<_, bool>(0))?),
            "hidden" => Out::val(false),
            "inProgressMergeCommitSha" => Out::Null,
            "rebaseCommitSha" => Out::val(mr.rebase_commit_sha.clone()),
            "mergeAfter" => match mrs::merge_after(c, mr)? {
                Some(t) => Out::val(labrat_core::time::iso_secs(t)),
                None => Out::Null,
            },
            "headPipeline" => match labrat_core::services::ci::diff_head_pipeline(c, p, mr)? {
                Some(pl) => Out::obj(PipelineObj { p: p.clone(), pl }),
                None => Out::Null,
            },
            "pipelines" => {
                // `MergeRequest.pipelines`: newest first, MR and branch pipelines together.
                let mut list = labrat_core::services::ci::mr_pipelines(c, p, mr)?;
                list.sort_by(|a, b| b.id.cmp(&a.id));
                let list: Vec<_> = list
                    .into_iter()
                    .filter(|pl| args.str("status").is_none_or(|st| st.eq_ignore_ascii_case(&pl.status)))
                    .filter(|pl| args.str("ref").is_none_or(|r| pl.ref_.as_deref() == Some(r.as_str())))
                    .filter(|pl| args.str("sha").is_none_or(|h| pl.sha == h))
                    .filter(|pl| args.str("source").is_none_or(|src| src.eq_ignore_ascii_case(&pl.source)))
                    .collect();
                let items = list.into_iter().map(|pl| (id_cursor(pl.id), Shared(Rc::new(PipelineObj { p: p.clone(), pl })))).collect();
                // `MergeRequest.pipelines` has a max page size of 500.
                Out::obj(Conn::with_max("PipelineConnection", items, args, 500)?)
            }
            "diffRefs" => match &diff {
                Some(d) => Out::val(json!({"baseSha": d.base_commit_sha, "startSha": d.start_commit_sha, "headSha": d.head_commit_sha})).into_diff_refs(),
                None => Out::Null,
            },
            "commitCount" => match &diff {
                Some(d) => Out::val(d.commits_count),
                None => Out::val(0),
            },
            "diffStatsSummary" => match &diff {
                Some(d) => {
                    let (add, del, files) = diff_stats(c, p, d)?;
                    Out::obj(JsonObj::new("DiffStatsSummary", json!({"additions": add, "deletions": del, "changes": add + del, "fileCount": files})))
                }
                None => Out::Null,
            },
            "userNotesCount" => {
                let n: i64 = c.query_row(
                    "SELECT count(*) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND system = 0",
                    [mr.id],
                    |r| r.get(0),
                )?;
                Out::val(n)
            }
            "userDiscussionsCount" => {
                let n: i64 = c.query_row(
                    "SELECT count(DISTINCT discussion_id) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND system = 0",
                    [mr.id],
                    |r| r.get(0),
                )?;
                Out::val(n)
            }
            "upvotes" | "downvotes" => {
                let name = if field == "upvotes" { "thumbsup" } else { "thumbsdown" };
                Out::val(emoji::list(c, "MergeRequest", mr.id)?.iter().filter(|a| a.name == name).count())
            }
            "awardEmoji" => award_conn(c, "MergeRequest", mr.id, args)?,
            "subscribed" => Out::val(match cx.user() {
                Some(u) => subscriptions::subscribed_to_mr(c, u.id, mr)?,
                None => false,
            }),
            "notes" => {
                let filter = args.str("filter");
                let notes: Vec<Note> = Note::for_noteable(c, "MergeRequest", mr.id)?
                    .into_iter()
                    .filter(|n| note_visible(cx, p, n))
                    .filter(|n| match filter.as_deref() {
                        Some("ONLY_COMMENTS") => !n.system,
                        Some("ONLY_ACTIVITY") => n.system,
                        _ => true,
                    })
                    .collect();
                note_conn(cx, p, mr, notes, args)?
            }
            "discussions" => {
                let notes: Vec<Note> = Note::for_noteable(c, "MergeRequest", mr.id)?.into_iter().filter(|n| note_visible(cx, p, n)).collect();
                let mut ids: Vec<String> = vec![];
                for n in &notes {
                    if !ids.contains(&n.discussion_id) {
                        ids.push(n.discussion_id.clone());
                    }
                }
                let objs = ids
                    .iter()
                    .map(|d| {
                        
                        Shared(Rc::new(DiscussionObj {
                            p: p.clone(),
                            mr: mr.clone(),
                            notes: notes.iter().filter(|n| n.discussion_id == *d).cloned().collect(),
                        }))
                    })
                    .collect();
                let objs = positions(objs);
                Out::obj(Conn::new("DiscussionConnection", objs, args)?)
            }
            "userPermissions" => Out::obj(MrPermissions { p: p.clone(), mr: mr.clone() }),
            "approvalState" if cx.req.app.config.is_ee() => Out::obj(super::approvals::ApprovalStateObj { p: p.clone(), mr: mr.clone() }),
            "approvalsRequired" if cx.req.app.config.is_ee() => Out::val(labrat_core::services::approvals::state(c, p, mr)?.approvals_required()),
            "approvalsLeft" if cx.req.app.config.is_ee() => Out::val(labrat_core::services::approvals::state(c, p, mr)?.approvals_left()),
            // `MergeRequest#mergeability_checks`: every check in GitLab's order with its status.
            "mergeabilityChecks" => {
                let ci_active = mr.merge_when_pipeline_succeeds || p.only_allow_merge_if_pipeline_succeeds;
                let ci = if !ci_active {
                    "INACTIVE"
                } else {
                    match labrat_core::services::ci::ci_check(c, p, mr)? {
                        None => "SUCCESS",
                        Some("ci_still_running") => "CHECKING",
                        Some(_) => "FAILED",
                    }
                };
                let discussions = if !p.only_allow_merge_if_all_discussions_are_resolved {
                    "INACTIVE"
                } else if mergeability::unresolved_discussions(c, mr)? {
                    "FAILED"
                } else {
                    "SUCCESS"
                };
                let merge_time = match mrs::merge_after(c, mr)? {
                    None => "INACTIVE",
                    Some(t) if t > labrat_core::time::now() => "FAILED",
                    Some(_) => "SUCCESS",
                };
                let rebase = if !matches!(p.merge_method.as_str(), "ff" | "rebase_merge") {
                    "INACTIVE"
                } else if mergeability::should_be_rebased(c, p, mr)? {
                    "FAILED"
                } else {
                    "SUCCESS"
                };
                let source = mr.source_branch_sha(c, p)?;
                let diff_head = mr.latest_diff(c)?.and_then(|d| d.head_commit_sha);
                let conflict = if source != diff_head {
                    "CHECKING"
                } else {
                    match mr.merge_status.as_str() {
                        "can_be_merged" => "SUCCESS",
                        "cannot_be_merged" => "FAILED",
                        _ => "CHECKING",
                    }
                };
                let pass = |b: bool| if b { "SUCCESS" } else { "FAILED" };
                if cx.req.app.config.is_ee() {
                    // EE 18.11's list, with the EE checks labrat has no feature for inactive.
                    let requested = mr.reviewers(c)?.iter().any(|(_, _, st, _)| *st == reviews::state::REQUESTED_CHANGES);
                    let approved = labrat_core::services::approvals::state(c, p, mr)?.approved();
                    let checks = [
                        ("NOT_OPEN", pass(mr.is_open())),
                        ("DRAFT_STATUS", pass(!mr.is_draft())),
                        ("STATUS_CHECKS_MUST_PASS", "INACTIVE"),
                        ("JIRA_ASSOCIATION_MISSING", "INACTIVE"),
                        ("MERGE_TIME", merge_time),
                        ("MERGE_REQUEST_BLOCKED", "SUCCESS"),
                        ("REQUESTED_CHANGES", pass(!requested)),
                        ("TITLE_REGEX", "INACTIVE"),
                        ("SECURITY_POLICY_VIOLATIONS", "INACTIVE"),
                        ("LOCKED_LFS_FILES", if labrat_core::repo::lfs_enabled(p) { "SUCCESS" } else { "INACTIVE" }),
                        ("DISCUSSIONS_NOT_RESOLVED", discussions),
                        ("CI_MUST_PASS", ci),
                        ("COMMITS_STATUS", pass(!mergeability::broken(c, p, mr)?)),
                        ("LOCKED_PATHS", "SUCCESS"),
                        ("NOT_APPROVED", pass(approved)),
                        ("SECURITY_POLICY_PIPELINE_CHECK", "INACTIVE"),
                        ("NEED_REBASE", rebase),
                        ("CONFLICT", conflict),
                    ];
                    return Ok(Out::List(
                        checks
                            .iter()
                            .map(|(id, st)| Out::obj(JsonObj::new("MergeRequestMergeabilityCheck", json!({"identifier": id, "status": st}))))
                            .collect(),
                    ));
                }
                let checks = [
                    ("NOT_OPEN", pass(mr.is_open())),
                    ("DRAFT_STATUS", pass(!mr.is_draft())),
                    ("MERGE_TIME", merge_time),
                    // `CheckLfsFileLocksService`: inactive when the project has LFS off.
                    ("LOCKED_LFS_FILES", if labrat_core::repo::lfs_enabled(p) { "SUCCESS" } else { "INACTIVE" }),
                    ("DISCUSSIONS_NOT_RESOLVED", discussions),
                    ("CI_MUST_PASS", ci),
                    ("COMMITS_STATUS", pass(!mergeability::broken(c, p, mr)?)),
                    ("NEED_REBASE", rebase),
                    ("CONFLICT", conflict),
                ];
                Out::List(
                    checks
                        .iter()
                        .map(|(id, st)| Out::obj(JsonObj::new("MergeRequestMergeabilityCheck", json!({"identifier": id, "status": st}))))
                        .collect(),
                )
            }
            "hasCi" => Out::val(labrat_core::services::ci::has_ci(c, p, mr)?),
            "allowCollaboration" => Out::val(false),
            "rebaseInProgress" => Out::val(mr.rebase_in_progress),
            "shouldBeRebased" => Out::val(mergeability::should_be_rebased(c, p, mr)?),
            "defaultMergeCommitMessage" => Out::val(labrat_core::services::merge_requests::merge::default_merge_commit_message(c, &cx.req.app.config, p, mr, cx.req.user.as_ref())?),
            "defaultSquashCommitMessage" => Out::val(labrat_core::services::merge_requests::merge::default_squash_commit_message(c, &cx.req.app.config, p, mr, cx.req.user.as_ref())?),
            "taskCompletionStatus" => {
                let t = crate::entities::merge_request::task_status(mr.description.as_deref());
                Out::obj(JsonObj::new("TaskCompletionStatus", json!({"count": t["count"], "completedCount": t["completed_count"]})))
            }
            "timelogs" => {
                let mut st = c.prepare("SELECT id FROM timelogs WHERE merge_request_id = ?1 ORDER BY spent_at, id")?;
                let ids: Vec<i64> = st.query_map([mr.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                let mut items = vec![];
                for id in ids {
                    items.push((id_cursor(id), Shared(Rc::new(TimelogObj::load(c, id)?))));
                }
                Out::obj(Conn::new("TimelogConnection", items, args)?)
            }
            _ => {
                let _ = base;
                return Err(not_implemented("MergeRequest", field));
            }
        })
    }
}

/// `Commit` (repository commits in MR and project contexts).
pub struct CommitObj {
    p: Project,
    oid: labrat_git::Oid,
    c: labrat_git::Commit,
}

fn git_time(sig: &labrat_git::Signature) -> Value {
    let tz = chrono::FixedOffset::east_opt(sig.offset * 60).unwrap_or(chrono::FixedOffset::east_opt(0).unwrap());
    let t = chrono::DateTime::from_timestamp(sig.time, 0).unwrap_or_default().with_timezone(&tz);
    json!(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
}

impl Object for CommitObj {
    fn typename(&self) -> String {
        "Commit".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let base = cx.req.app.config.external_url.trim_end_matches('/');
        let sha = self.oid.hex();
        let msg = &self.c.message;
        let full_title = msg.split(['\r', '\n']).next().unwrap_or("").to_string();
        // `Commit#title`: first lines of 100+ characters are cut to 81 at a word boundary.
        let long = full_title.chars().count() >= 100;
        let title = if long {
            let head: String = full_title.chars().take(78).collect();
            let cut = match head.rfind(' ') {
                Some(i) => head[..i].to_string(),
                None => head,
            };
            format!("{cut}...")
        } else {
            full_title.clone()
        };
        Ok(match field {
            // `Commit#diffs` with the default (compare-style) collapse limits; every value is a
            // String in the schema, booleans included.
            "diffs" => {
                let r = repo::repo(cx.c, &self.p);
                let parent_tree = match self.c.parents.first() {
                    Some(pp) => Some(r.commit(pp)?.tree),
                    None => None,
                };
                let changes = labrat_git::diff::diff_trees(&r, parent_tree.as_ref(), Some(&self.c.tree))?;
                let list = crate::entities::repository::compare_diff_list(&r, &changes)?;
                let to_s = |v: &Value| -> Value {
                    match v {
                        Value::Bool(b) => json!(b.to_string()),
                        Value::Null => Value::Null,
                        Value::String(_) => v.clone(),
                        other => json!(other.to_string()),
                    }
                };
                Out::List(
                    list.iter()
                        .map(|d| {
                            Out::obj(JsonObj::new("Diff", json!({
                                "aMode": to_s(&d["a_mode"]), "bMode": to_s(&d["b_mode"]), "collapsed": to_s(&d["collapsed"]),
                                "deletedFile": to_s(&d["deleted_file"]), "diff": to_s(&d["diff"]), "newFile": to_s(&d["new_file"]),
                                "newPath": to_s(&d["new_path"]), "oldPath": to_s(&d["old_path"]), "renamedFile": to_s(&d["renamed_file"]),
                                "tooLarge": to_s(&d["too_large"]),
                            })))
                        })
                        .collect(),
                )
            }
            "id" => Out::val(format!("gid://gitlab/Commit/{sha}")),
            "sha" => Out::val(sha.clone()),
            "shortId" => Out::val(sha[..8].to_string()),
            "parentSha" => Out::val(self.c.parents.first().map(|o| o.hex())),
            "title" | "name" => Out::val(title),
            "fullTitle" => Out::val(full_title.clone()),
            "tags" => Out::List(vec![]),
            "titleHtml" | "fullTitleHtml" => Out::val(crate::markdown::render_inline(msg.lines().next().unwrap_or(""))),
            "message" => Out::val(msg.clone()),
            // The whole message when the title was cut.
            "description" if long => Out::val(msg.clone()),
            "description" => {
                let rest: String = msg.split_once('\n').map(|x| x.1.to_string()).unwrap_or_default();
                Out::val(if rest.trim().is_empty() { Value::Null } else { json!(format!("\n{}", rest.trim_end_matches('\n').trim_start_matches('\n'))) })
            }
            "authorName" => Out::val(self.c.author.name.clone()),
            "authorEmail" => Out::val(self.c.author.email.clone()),
            "committerName" => Out::val(self.c.committer.name.clone()),
            "committerEmail" => Out::val(self.c.committer.email.clone()),
            "authoredDate" => Out::val(git_time(&self.c.author)),
            "committedDate" => Out::val(git_time(&self.c.committer)),
            "webUrl" => Out::val(format!("{base}/{}/-/commit/{sha}", self.p.full_path)),
            "webPath" => Out::val(format!("/{}/-/commit/{sha}", self.p.full_path)),
            "author" => {
                let id: Option<i64> = cx
                    .c
                    .query_row("SELECT id FROM users WHERE lower(email) = lower(?1) OR lower(commit_email) = lower(?1)", [&self.c.author.email], |r| r.get(0))
                    .ok();
                Out::opt(id.map(|i| User::find(cx.c, i)).transpose()?.flatten().map(|u| UserObj::new("UserCore", u)))
            }
            "authorGravatar" => {
                use sha2::{Digest, Sha256};
                let h = hex::encode(Sha256::digest(self.c.author.email.trim().to_lowercase().as_bytes()));
                Out::val(format!("https://www.gravatar.com/avatar/{h}?s=80&d=identicon"))
            }
            // `Commit.pipelines`: the project's pipelines for this commit, newest first.
            "pipelines" => {
                let mut list = labrat_core::services::ci::Pipeline::list(cx.c, self.p.id)?;
                list.retain(|pl| pl.sha == self.oid.hex());
                list.sort_by(|a, b| b.id.cmp(&a.id));
                let items = list
                    .into_iter()
                    .filter(|pl| args.str("status").is_none_or(|st| st.eq_ignore_ascii_case(&pl.status)))
                    .filter(|pl| args.str("ref").is_none_or(|r| pl.ref_.as_deref() == Some(r.as_str())))
                    .map(|pl| (id_cursor(pl.id), Shared(Rc::new(PipelineObj { p: self.p.clone(), pl }))))
                    .collect();
                Out::obj(Conn::with_max("PipelineConnection", items, args, 100)?)
            }
            "signatureHtml" | "signature" => Out::Null,
            _ => return Err(not_implemented("Commit", field)),
        })
    }
}

fn milestone_obj(cx: &Ctx, id: Option<i64>) -> FResult<Out> {
    let Some(id) = id else { return Ok(Out::Null) };
    let v = crate::entities::merge_request::milestone(cx.c, &cx.req.app.config, id)?;
    if v.is_null() {
        return Ok(Out::Null);
    }
    let base = cx.req.app.config.external_url.trim_end_matches('/');
    let web = v["web_url"].as_str().unwrap_or_default().to_string();
    let (project_id, group_id): (Option<i64>, Option<i64>) = cx.c.query_row("SELECT project_id, group_id FROM milestones WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(Out::obj(MilestoneObj {
        project_id,
        group_id,
        v: json!({
            "id": format!("gid://gitlab/Milestone/{id}"),
            "iid": v["iid"].to_string(),
            "title": v["title"],
            "description": v["description"],
            "state": v["state"],
            "dueDate": v["due_date"],
            "startDate": v["start_date"],
            "expired": v["expired"],
            "webPath": web.strip_prefix(base).unwrap_or(&web),
            "createdAt": v["created_at"].as_str().map(|t| t.split('.').next().unwrap_or(t).to_string() + "Z"),
            "updatedAt": v["updated_at"].as_str().map(|t| t.split('.').next().unwrap_or(t).to_string() + "Z"),
            "projectMilestone": project_id.is_some(),
            "groupMilestone": group_id.is_some(),
            "subgroupMilestone": false,
            // `upcoming?`: starts in the future.
            "upcoming": v["start_date"].as_str().is_some_and(|d| d > labrat_core::time::today().as_str()),
            "stats": null,
        }),
    }))
}

struct MilestoneObj {
    project_id: Option<i64>,
    group_id: Option<i64>,
    v: Value,
}

impl Object for MilestoneObj {
    fn typename(&self) -> String {
        "Milestone".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "project" => match self.project_id.map(|id| Project::find(cx.c, id)).transpose()?.flatten() {
                Some(p) => match readable_project(cx, p)? {
                    Some((p, perms)) => Out::obj(ProjectObj { p, perms }),
                    None => Out::Null,
                },
                None => Out::Null,
            },
            "group" => Out::opt(self.group_id.map(|id| Namespace::find(cx.c, id)).transpose()?.flatten().map(|n| NamespaceObj { ty: "Group", n })),
            // labrat has no issues.
            "stats" => Out::obj(JsonObj::new("MilestoneStats", json!({"totalIssuesCount": 0, "closedIssuesCount": 0}))),
            "releases" => Out::obj(Conn::new("ReleaseConnection", vec![], &Args::default())?),
            f => match self.v.get(f) {
                Some(v) => Out::val(v.clone()),
                None => return Err(not_implemented("Milestone", f)),
            },
        })
    }
}

/// `Ci::Pipeline` (external pipelines from commit statuses).
pub struct PipelineObj {
    p: Project,
    pl: labrat_core::services::ci::Pipeline,
}

impl Object for PipelineObj {
    fn typename(&self) -> String {
        "Pipeline".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let (p, pl) = (&self.p, &self.pl);
        let path = format!("/{}/-/pipelines/{}", p.full_path, pl.id);
        let (icon, text, label, group) = match pl.status.as_str() {
            "pending" => ("status_pending", "Pending", "pending", "pending"),
            "running" => ("status_running", "Running", "running", "running"),
            "success" => ("status_success", "Passed", "passed", "success"),
            "failed" => ("status_failed", "Failed", "failed", "failed"),
            "canceled" => ("status_canceled", "Canceled", "canceled", "canceled"),
            "skipped" => ("status_skipped", "Skipped", "skipped", "skipped"),
            _ => ("status_created", "Created", "created", "created"),
        };
        Ok(match field {
            // Commit statuses are the jobs of an external pipeline, in one `external` stage.
            "jobs" | "job" | "stages" => {
                let mut st = cx.c.prepare("SELECT id FROM commit_statuses WHERE pipeline_id = ?1 AND retried = 0 ORDER BY id")?;
                let ids: Vec<i64> = st.query_map([pl.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                let jobs: Vec<labrat_core::services::ci::Status> = ids.into_iter().filter_map(|id| labrat_core::services::ci::Status::find(cx.c, id).ok().flatten()).collect();
                let job_obj = |j: &labrat_core::services::ci::Status| {
                    JsonObj::new("CiJob", json!({
                        "id": format!("gid://gitlab/GenericCommitStatus/{}", j.id),
                        "name": j.name,
                        "status": j.status.to_uppercase(),
                        "allowFailure": false,
                    }))
                };
                match field {
                    "job" => {
                        let want = _args.str("name");
                        match jobs.iter().find(|j| want.as_deref().is_none_or(|w| w == j.name)) {
                            Some(j) => Out::obj(job_obj(j)),
                            None => Out::Null,
                        }
                    }
                    "jobs" => {
                        let items: Vec<Shared> = jobs.iter().map(|j| Shared(Rc::new(job_obj(j)))).collect();
                        Out::obj(Conn::new("CiJobConnection", positions(items), _args)?)
                    }
                    _ => {
                        let items: Vec<Shared> = if jobs.is_empty() {
                            vec![]
                        } else {
                            vec![Shared(Rc::new(JsonObj::new("CiStage", json!({"name": "external", "status": pl.status.to_lowercase()}))))]
                        };
                        Out::obj(Conn::new("CiStageConnection", positions(items), _args)?)
                    }
                }
            }
            "testReportSummary" => Out::obj(TestSummaryObj),
            "warningMessages" | "manualVariables" => {
                let conn = if field == "warningMessages" { "PipelineMessageConnection" } else { "PipelineManualVariableConnection" };
                Out::obj(Conn::new(conn, positions(Vec::<Shared>::new()), _args)?)
            }
            "jobArtifacts" => Out::List(vec![]),
            "sourceJob" | "testSuite" => Out::Null,
            "id" => Out::val(format!("gid://gitlab/Ci::Pipeline/{}", pl.id)),
            "iid" => Out::val(pl.iid.to_string()),
            "status" => Out::val(pl.status.to_uppercase()),
            "sha" => Out::val(pl.sha.clone()),
            "beforeSha" => Out::val("0000000000000000000000000000000000000000"),
            "ref" => Out::val(pl.ref_.clone()),
            "refPath" => Out::val(pl.ref_.as_ref().map(|r| format!("refs/{}/{r}", if pl.tag { "tags" } else { "heads" }))),
            "path" => Out::val(path.clone()),
            "source" => Out::val(pl.source.clone()),
            "type" => Out::val(if pl.tag { "tag" } else { "branch" }),
            "active" => Out::val(pl.is_active()),
            "complete" => Out::val(pl.is_complete()),
            "duration" => Out::val(pl.duration),
            "queuedDuration" => Out::val(pl.queued_duration),
            "createdAt" => Out::val(gtime(pl.created_at)),
            "updatedAt" => Out::val(gtime(pl.updated_at)),
            "startedAt" => Out::val(opt_time(pl.started_at)),
            "finishedAt" => Out::val(opt_time(pl.finished_at)),
            "committedAt" => Out::Null,
            "yamlErrors" => Out::val(false),
            "warnings" => Out::val(false),
            // `latest?`: the pipeline is for the ref's current head.
            "latest" => Out::val(match pl.ref_.as_deref() {
                Some(r) => {
                    let repo = repo::repo(cx.c, &self.p);
                    let head = if pl.tag { repo.tags()?.into_iter().find(|(n, _)| n == r).map(|(_, o)| o) } else { repo.branch(r)? };
                    head.map(|o| o.hex()) == Some(pl.sha.clone())
                }
                None => false,
            }),
            "name" => Out::val(pl.name.clone()),
            "commitPath" => Out::val(format!("/{}/-/commit/{}", p.full_path, pl.sha)),
            "commit" => {
                let r = repo::repo(cx.c, p);
                match labrat_git::Oid::from_hex(&pl.sha).ok().and_then(|o| r.commit(&o).ok().map(|c| (o, c))) {
                    Some((oid, c)) => Out::obj(CommitObj { p: p.clone(), oid, c }),
                    None => Out::Null,
                }
            }
            "project" => match readable_project(cx, p.clone())? {
                Some((p, perms)) => Out::obj(ProjectObj { p, perms }),
                None => Out::Null,
            },
            // External pipelines aren't merge request pipelines.
            "mergeRequest" | "failureReason" | "coverage" | "upstream" | "downstream" | "pipelineSchedule" => Out::Null,
            // No upstream pipeline: an empty path rather than null.
            "triggeredByPath" => Out::val(""),
            // Commit statuses can be neither retried nor cancelled through GitLab.
            "cancelable" | "retryable" | "stuck" | "child" | "hasManualActions" | "hasScheduledActions" | "usesNeeds" => Out::val(false),
            // Commit statuses have no CI configuration.
            "mergeRequestEventType" | "yamlErrorMessages" => Out::Null,
            "trigger" => Out::val(false),
            "configSource" => Out::val("UNKNOWN_SOURCE"),
            "failedJobsCount" => {
                let n: i64 = cx.c.query_row("SELECT count(*) FROM commit_statuses WHERE pipeline_id = ?1 AND status = 'failed'", [pl.id], |r| r.get(0))?;
                Out::val(n)
            }
            "totalJobs" => {
                let n: i64 = cx.c.query_row("SELECT count(*) FROM commit_statuses WHERE pipeline_id = ?1", [pl.id], |r| r.get(0))?;
                Out::val(n)
            }
            // `Ci::PipelinePresenter#ref_text` for a branch pipeline with related merge requests.
            "refText" => {
                let mrs = labrat_core::services::ci::all_merge_requests(cx.c, p, pl)?;
                if mrs.is_empty() {
                    let r = pl.ref_.clone().unwrap_or_default();
                    Out::val(format!("For <a class=\"ref-container gl-link\" href=\"/{}/-/commits/{r}\">{r}</a>", p.full_path))
                } else {
                    let links: Vec<String> = mrs
                        .iter()
                        .map(|m| format!("<a class=\"mr-iid\" href=\"/{}/-/merge_requests/{}\">!{} {}</a>", p.full_path, m.iid, m.iid, crate::markdown::render_inline(&m.title)))
                        .collect();
                    let noun = if mrs.len() == 1 { "related merge request" } else { "related merge requests" };
                    Out::val(format!("{} {noun}: {}", mrs.len(), links.join(", ")))
                }
            }
            "userPermissions" => {
                let perms = ProjectPerms::load(cx.c, cx.user(), p)?;
                let dev = cx.user().is_some() && perms.developer();
                let admin = cx.user().is_some_and(|u| u.admin) || perms.admin_project();
                Out::obj(JsonObj::new("PipelinePermissions", json!({"updatePipeline": dev, "cancelPipeline": dev, "destroyPipeline": perms.level >= 50 || admin, "adminPipeline": admin})))
            }
            "user" => Out::opt(pl.user_id.map(|i| User::find(cx.c, i)).transpose()?.flatten().map(|u| UserObj::new("UserCore", u))),
            "detailedStatus" => Out::obj(JsonObj::new(
                "DetailedStatus",
                json!({
                    "id": format!("{}-{}", pl.status, pl.id),
                    "name": pl.status.to_uppercase(),
                    "label": label,
                    "text": text,
                    "group": group,
                    "icon": icon,
                    "tooltip": label,
                    "hasDetails": true,
                    "detailsPath": path,
                    "favicon": format!("/assets/ci_favicons/favicon_{icon}-{}.png", crate::routes::pipelines::favicon_digest(icon)),
                    "action": null,
                    "deploymentDetailsPath": null,
                }),
            )),
            _ => return Err(not_implemented("Pipeline", field)),
        })
    }
}

trait IntoDiffRefs {
    fn into_diff_refs(self) -> Out;
}

impl IntoDiffRefs for Out {
    fn into_diff_refs(self) -> Out {
        match self {
            Out::Val(v) => Out::obj(JsonObj::new("DiffRefs", v)),
            other => other,
        }
    }
}

fn diff_stats(c: &rusqlite::Connection, p: &Project, d: &labrat_core::services::merge_requests::diffs::Diff) -> FResult<(i64, i64, i64)> {
    let r = repo::repo(c, p);
    let (Some(base), Some(head)) = (d.base_commit_sha.clone(), d.head_commit_sha.clone()) else { return Ok((0, 0, 0)) };
    let (Ok(b), Ok(h)) = (labrat_git::Oid::from_hex(&base), labrat_git::Oid::from_hex(&head)) else { return Ok((0, 0, 0)) };
    let bt = r.commit(&b)?.tree;
    let ht = r.commit(&h)?.tree;
    let changes = labrat_git::diff::diff_trees(&r, Some(&bt), Some(&ht))?;
    let (mut add, mut del) = (0i64, 0i64);
    for ch in &changes {
        let (a, d2) = labrat_git::diff::line_stats(&r, ch)?;
        add += a as i64;
        del += d2 as i64;
    }
    Ok((add, del, changes.len() as i64))
}

/// Plain objects whose fields are keys of a JSON map.
/// A to-do item (`Types::TodoType`).
struct TodoObj {
    id: i64,
    author_id: i64,
    project_id: Option<i64>,
    target_id: i64,
    action: i64,
    state: String,
    note_id: Option<i64>,
    created_at: i64,
}

impl TodoObj {
    fn load(c: &rusqlite::Connection, id: i64) -> FResult<TodoObj> {
        Ok(c.query_row(
            "SELECT id, author_id, project_id, target_id, action, state, note_id, created_at FROM todos WHERE id = ?1",
            [id],
            |r| {
                Ok(TodoObj {
                    id: r.get(0)?,
                    author_id: r.get(1)?,
                    project_id: r.get(2)?,
                    target_id: r.get(3)?,
                    action: r.get(4)?,
                    state: r.get(5)?,
                    note_id: r.get(6)?,
                    created_at: r.get(7)?,
                })
            },
        )?)
    }
}

impl Object for TodoObj {
    fn typename(&self) -> String {
        "Todo".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let c = cx.c;
        let mr = MergeRequest::find(c, self.target_id)?;
        let note = match self.note_id {
            Some(id) => Note::find(c, id)?,
            None => None,
        };
        Ok(match field {
            "id" => Out::val(gid("Todo", self.id)),
            "action" => Out::val(labrat_core::services::todos::action::name(self.action)),
            "state" => Out::val(self.state.clone()),
            // `Todo#body`: the note's text, or the target's title.
            "body" => Out::val(match &note {
                Some(n) => n.note.clone(),
                None => mr.map(|m| m.title).unwrap_or_default(),
            }),
            "author" => Out::opt(User::find(c, self.author_id)?.map(|u| UserObj::new("UserCore", u))),
            "createdAt" => Out::val(gtime(self.created_at)),
            "targetType" => Out::val("MERGEREQUEST"),
            "targetUrl" => match &mr {
                Some(m) => match Project::find(c, m.target_project_id)? {
                    Some(p) => {
                        let url = m.web_url(&cx.req.app.config, &p);
                        Out::val(match &note {
                            Some(n) => format!("{url}#note_{}", n.id),
                            None => url,
                        })
                    }
                    None => Out::Null,
                },
                None => Out::Null,
            },
            "target" | "targetEntity" => match &mr {
                Some(m) => match Project::find(c, m.target_project_id)? {
                    Some(p) => Out::obj(MrObj::new(p, m.clone())),
                    None => Out::Null,
                },
                None => Out::Null,
            },
            "note" => match (&note, &mr) {
                (Some(n), Some(m)) => match Project::find(c, m.target_project_id)? {
                    Some(p) => Out::obj(NoteObj { p, mr: m.clone(), n: n.clone() }),
                    None => Out::Null,
                },
                _ => Out::Null,
            },
            "snoozedUntil" => {
                let t: Option<i64> = c.query_row("SELECT snoozed_until FROM todos WHERE id = ?1", [self.id], |r| r.get(0)).ok().flatten();
                Out::val(t.map(gtime))
            }
            "group" | "memberAccessType" => Out::Null,
            "project" => match self.project_id.map(|id| Project::find(c, id)).transpose()?.flatten() {
                Some(p) => match readable_project(cx, p)? {
                    Some((p, perms)) => Out::obj(ProjectObj { p, perms }),
                    None => Out::Null,
                },
                None => Out::Null,
            },
            _ => return Err(not_implemented("Todo", field)),
        })
    }
}

pub struct JsonObj {
    ty: String,
    v: Value,
}

impl JsonObj {
    pub fn new(ty: &str, v: Value) -> JsonObj {
        JsonObj { ty: ty.into(), v }
    }
}

impl Object for JsonObj {
    fn typename(&self) -> String {
        self.ty.clone()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        match self.v.get(field) {
            Some(v) => Ok(Out::val(v.clone())),
            None => Err(not_implemented(&self.ty, field)),
        }
    }
}

struct MrPermissions {
    p: Project,
    mr: MergeRequest,
}

impl Object for MrPermissions {
    fn typename(&self) -> String {
        "MergeRequestPermissions".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let (c, p, mr) = (cx.c, &self.p, &self.mr);
        // Anonymous viewers of a public project can read it, nothing more.
        let Some(u) = cx.user() else {
            return Ok(Out::val(field == "readMergeRequest" && ProjectPerms::load(c, None, p)?.read_merge_request(p)));
        };
        let perms = ProjectPerms::load(c, Some(u), p)?;
        let update = mrs::can_update(c, p, mr, u)?;
        Ok(Out::val(match field {
            "readMergeRequest" => perms.read_merge_request(p),
            "updateMergeRequest" => update,
            "adminMergeRequest" => perms.developer() && !p.archived,
            // Locked discussions still accept comments from any member (Guest and up).
            "createNote" => !p.archived && perms.read_merge_request(p) && (mr.discussion_locked != Some(true) || perms.level >= labrat_core::model::access::GUEST || perms.admin),
            "pushToSourceBranch" => {
                repo::repo(c, p).branch(&mr.source_branch)?.is_some() && repo::can_push_to_branch(c, u, p, &mr.source_branch)?
            }
            // `can_remove_source_branch?`: not protected, not the default branch, pushable, and the
            // branch is still at the MR's head.
            "removeSourceBranch" => {
                let r = repo::repo(c, p);
                mr.is_open()
                    && !repo::is_protected(c, p, &mr.source_branch)?
                    && p.default_branch.as_deref() != Some(mr.source_branch.as_str())
                    && repo::can_push_to_branch(c, u, p, &mr.source_branch)?
                    && r.branch(&mr.source_branch)?.map(|o| o.hex()) == mr.diff_head_sha(c, p)?
            }
            "cherryPickOnCurrentMergeRequest" | "revertOnCurrentMergeRequest" => perms.developer() && mr.state == "merged",
            "canMerge" => mrs::can_be_merged_by(c, p, mr, u)?,
            "canApprove" => {
                !p.archived
                    && if cx.req.app.config.is_ee() {
                        labrat_core::services::approvals::eligibility(c, p, mr, u)? == labrat_core::services::approvals::Eligibility::Yes
                    } else {
                        reviews::can_approve(c, p, mr, u)? && !reviews::has_approved(c, mr, u)?
                    }
            }
            _ => return Err(not_implemented("MergeRequestPermissions", field)),
        }))
    }
}

// ---------------------------------------------------------------------------------------------
// Notes, discussions, labels, emoji

fn note_visible(cx: &Ctx, p: &Project, n: &Note) -> bool {
    if !n.internal {
        return true;
    }
    match cx.user() {
        Some(u) => ProjectPerms::load(cx.c, Some(u), p).map(|x| x.reporter() || x.admin).unwrap_or(false),
        None => false,
    }
}

fn note_conn(_cx: &Ctx, p: &Project, mr: &MergeRequest, notes: Vec<Note>, args: &Args) -> FResult<Out> {
    let objs = notes.into_iter().map(|n| (created_cursor(n.created_at, n.id), Shared(Rc::new(NoteObj { p: p.clone(), mr: mr.clone(), n })))).collect();
    Ok(Out::obj(Conn::new("NoteConnection", objs, args)?))
}

pub struct NoteObj {
    pub p: Project,
    pub mr: MergeRequest,
    pub n: Note,
}

fn note_gid_kind(n: &Note) -> &'static str {
    match n.kind.as_deref() {
        Some("DiffNote") => "DiffNote",
        Some("DiscussionNote") => "DiscussionNote",
        _ => "Note",
    }
}

impl Object for NoteObj {
    fn typename(&self) -> String {
        "Note".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let (c, n) = (cx.c, &self.n);
        Ok(match field {
            "id" => Out::val(gid(note_gid_kind(n), n.id)),
            "body" => Out::val(n.note.clone()),
            // Rendered (and cached by GitLab) when the note was last saved.
            "bodyHtml" => Out::val(crate::markdown::render_gfm(c, &self.p, &n.note, n.last_edited_at.unwrap_or(n.created_at))),
            "system" => Out::val(n.system),
            "internal" => Out::val(n.internal),
            "author" => Out::opt(User::find(c, n.author_id)?.map(|u| UserObj::new("UserCore", u))),
            "createdAt" => Out::val(gtime(n.created_at)),
            "updatedAt" => Out::val(gtime(n.updated_at)),
            // `Note#last_edited_at` falls back to `updated_at`.
            "lastEditedAt" => Out::val(opt_time(n.last_edited_at.or(Some(n.updated_at)))),
            "lastEditedBy" => Out::opt(n.updated_by_id.map(|id| User::find(c, id)).transpose()?.flatten().map(|u| UserObj::new("UserCore", u))),
            "resolvable" => Out::val(n.resolvable && !n.system),
            "resolved" => Out::val(n.is_resolved()),
            "resolvedAt" => Out::val(opt_time(n.resolved_at)),
            "resolvedBy" => Out::opt(n.resolved_by_id.map(|id| User::find(c, id)).transpose()?.flatten().map(|u| UserObj::new("UserCore", u))),
            "url" => Out::val(format!("{}#note_{}", self.mr.web_url(&cx.req.app.config, &self.p), n.id)),
            "systemNoteIconName" => Out::val(n.system_action.as_deref().and_then(system_icon)),
            "discussion" => {
                // Internal replies stay hidden from those who can't read internal notes.
                let notes: Vec<Note> = Note::in_discussion(c, &n.discussion_id)?.into_iter().filter(|x| note_visible(cx, &self.p, x)).collect();
                Out::obj(DiscussionObj { p: self.p.clone(), mr: self.mr.clone(), notes })
            }
            "position" => match n.position.as_deref().and_then(Position::parse) {
                Some(pos) => Out::obj(PositionObj { pos }),
                None => Out::Null,
            },
            "awardEmoji" => award_conn(c, "Note", n.id, args)?,
            "userPermissions" => Out::obj(JsonObj::new(
                "NotePermissions",
                {
                    // `NotePolicy`: authors and admins (maintainers) manage notes; resolving
                    // also allows Developers.
                    let perms = ProjectPerms::load(c, cx.user(), &self.p)?;
                    let me = cx.user();
                    let author = me.is_some_and(|u| u.id == n.author_id);
                    let manager = me.is_some_and(|u| u.admin) || perms.admin_project();
                    json!({
                        "readNote": true,
                        "adminNote": !n.system && (author || manager),
                        "createNote": me.is_some(),
                        "awardEmoji": me.is_some(),
                        "resolveNote": author || manager || perms.developer(),
                        "repositionNote": author || manager,
                    })
                },
            )),
            "noteableId" => Out::val(n.noteable_id),
            "noteableType" => Out::val(n.noteable_type.clone()),
            "imported" => Out::val(false),
            "externalAuthor" => Out::Null,
            "authorIsContributor" => Out::val(false),
            "maxAccessLevelOfAuthor" => {
                let author = User::find(c, n.author_id)?;
                let level = match author {
                    Some(a) => labrat_core::perms::project_access(c, &a, &self.p)?,
                    None => 0,
                };
                Out::val(match level {
                    50 => "Owner",
                    40 => "Maintainer",
                    30 => "Developer",
                    20 => "Reporter",
                    15 => "Planner",
                    10 => "Guest",
                    // `human_access(0)` is nil for non-members.
                    _ => return Ok(Out::Null),
                })
            }
            "bodyFirstLineHtml" => Out::val(crate::markdown::first_line_html(c, &n.note, cx.user())),
            "systemNoteMetadata" => match (&n.system, &n.system_action) {
                (true, Some(a)) => Out::obj(JsonObj::new("SystemNoteMetadata", json!({"id": format!("gid://gitlab/SystemNoteMetadata/{}", n.id), "action": a, "descriptionVersion": null}))),
                _ => Out::Null,
            },
            "project" => {
                let perms = ProjectPerms::load(c, cx.user(), &self.p)?;
                Out::obj(ProjectObj { p: self.p.clone(), perms })
            }
            _ => return Err(not_implemented("Note", field)),
        })
    }
}

/// `SystemNoteHelper::ICON_NAMES_BY_ACTION`.
/// `SystemNoteHelper#system_note_icon_name` for notes on merge requests.
fn system_icon(action: &str) -> Option<&'static str> {
    Some(match action {
        // MR-specific icons.
        "closed" => "merge-request-close",
        "merge" => "mr-system-note-empty",
        "approved" => "check",
        "unapproved" => "unapproval",
        "cherry_pick" => "cherry-pick-commit",
        "commit" => "commit",
        "description" => "pencil",
        "merged" => "merge",
        "opened" => "work-item-issue",
        "time_tracking" => "timer",
        "assignee" | "reviewer" => "user",
        "title" => "pencil",
        "task" => "todo-done",
        "label" => "label",
        "cross_reference" => "comment-dots",
        "branch" => "fork",
        "confidential" => "eye-slash",
        "visible" => "eye",
        "milestone" => "milestone",
        "discussion" => "comment",
        "moved" => "arrow-right",
        "outdated" => "pencil",
        "pinned_embed" => "thumbtack",
        "duplicate" => "duplicate",
        "locked" => "lock",
        "unlocked" => "lock-open",
        "due_date" | "start_date_or_due_date" => "calendar",
        "status" => "status",
        "contact" => "users",
        "timeline_event" => "clock",
        "requested_changes" => "error",
        "override" => "review-warning",
        "reviewed" => "comment-lines",
        _ => return None,
    })
}

struct PositionObj {
    pos: Position,
}

impl Object for PositionObj {
    fn typename(&self) -> String {
        "DiffPosition".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let p = &self.pos;
        Ok(match field {
            "diffRefs" => Out::obj(JsonObj::new("DiffRefs", json!({"baseSha": p.base_sha, "startSha": p.start_sha, "headSha": p.head_sha}))),
            "filePath" => Out::val(p.file_path()),
            "oldPath" => Out::val(p.old_path.clone()),
            "newPath" => Out::val(p.new_path.clone()),
            "oldLine" => Out::val(p.old_line),
            "newLine" => Out::val(p.new_line),
            "positionType" => Out::val(p.position_type.clone()),
            "x" => Out::val(p.x),
            "y" => Out::val(p.y),
            "width" => Out::val(p.width),
            "height" => Out::val(p.height),
            _ => return Err(not_implemented("DiffPosition", field)),
        })
    }
}

pub struct DiscussionObj {
    pub p: Project,
    pub mr: MergeRequest,
    pub notes: Vec<Note>,
}

impl Object for DiscussionObj {
    fn typename(&self) -> String {
        "Discussion".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let first = self.notes.first();
        let resolvable: Vec<&Note> = self.notes.iter().filter(|n| n.resolvable && !n.system).collect();
        Ok(match field {
            "id" => Out::val(json!(format!("gid://gitlab/Discussion/{}", first.map(|n| n.discussion_id.clone()).unwrap_or_default()))),
            "replyId" => {
                let class = match first.and_then(|n| n.kind.as_deref()) {
                    Some("DiffNote") => "DiffDiscussion",
                    Some("DiscussionNote") => "Discussion",
                    _ => "IndividualNoteDiscussion",
                };
                Out::val(json!(format!("gid://gitlab/{class}/{}", first.map(|n| n.discussion_id.clone()).unwrap_or_default())))
            }
            "userPermissions" => Out::obj(JsonObj::new(
                "DiscussionPermissions",
                json!({"resolveNote": !resolvable.is_empty() && cx.user().is_some()}),
            )),
            "createdAt" => Out::val(first.map(|n| gtime(n.created_at))),
            "resolvable" => Out::val(!resolvable.is_empty()),
            "resolved" => Out::val(!resolvable.is_empty() && resolvable.iter().all(|n| n.is_resolved())),
            "resolvedAt" => Out::val(if !resolvable.is_empty() && resolvable.iter().all(|n| n.is_resolved()) { resolvable.iter().filter_map(|n| n.resolved_at).max().map(gtime) } else { None }),
            "resolvedBy" => {
                let by = resolvable.last().and_then(|n| n.resolved_by_id);
                Out::opt(by.map(|id| User::find(cx.c, id)).transpose()?.flatten().map(|u| UserObj::new("UserCore", u)))
            }
            // `Discussion.notes` has a max page size of 200.
            "notes" => {
                let objs = self
                    .notes
                    .iter()
                    .cloned()
                    .map(|n| (created_cursor(n.created_at, n.id), Shared(Rc::new(NoteObj { p: self.p.clone(), mr: self.mr.clone(), n }))))
                    .collect();
                Out::obj(Conn::with_max("NoteConnection", objs, args, 200)?)
            }
            "noteable" => Out::obj(MrObj::new(self.p.clone(), self.mr.clone())),
            _ => return Err(not_implemented("Discussion", field)),
        })
    }
}

struct LabelObj {
    l: Label,
}

impl Object for LabelObj {
    fn typename(&self) -> String {
        "Label".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let l = &self.l;
        Ok(match field {
            "id" => Out::val(gid(if l.group_id.is_some() { "GroupLabel" } else { "ProjectLabel" }, l.id)),
            "title" => Out::val(l.title.clone()),
            "color" => Out::val(l.color.clone()),
            "textColor" => Out::val(l.text_color()),
            "description" => Out::val(l.description.clone()),
            "descriptionHtml" => Out::val(l.description.as_deref().map(crate::markdown::render_inline).unwrap_or_default()),
            "createdAt" => Out::val(gtime(l.created_at)),
            "updatedAt" => Out::val(gtime(l.updated_at)),
            "archived" => Out::val(l.archived),
            "lockOnMerge" => Out::val(l.lock_on_merge),
            _ => return Err(not_implemented("Label", field)),
        })
    }
}

fn award_conn(c: &rusqlite::Connection, kind: &str, id: i64, args: &Args) -> FResult<Out> {
    let list = emoji::list(c, kind, id)?;
    let objs = list.into_iter().map(|a| (id_cursor(a.id), Shared(Rc::new(AwardObj { a })))).collect();
    Ok(Out::obj(Conn::new("AwardEmojiConnection", objs, args)?))
}

pub struct AwardObj {
    pub a: Award,
}

impl Object for AwardObj {
    fn typename(&self) -> String {
        "AwardEmoji".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let a = &self.a;
        let info = emoji::info(&a.name);
        Ok(match field {
            "name" => Out::val(a.name.clone()),
            "emoji" => Out::val(info.map(|i| i.moji)),
            "unicode" => Out::val(info.map(|i| i.hex).unwrap_or_default()),
            "description" => Out::val(info.map(|i| i.description).unwrap_or_default()),
            "unicodeVersion" => Out::val(info.map(|i| i.unicode_version).unwrap_or_default()),
            "user" => Out::opt(User::find(cx.c, a.user_id)?.map(|u| UserObj::new("UserCore", u))),
            _ => return Err(not_implemented("AwardEmoji", field)),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Mutations

pub struct Mutation;

/// A mutation payload: `errors`, `clientMutationId` and the changed objects.
#[derive(Default)]
struct Payload {
    branch: Option<BranchObj>,
    branch_rule: Option<(labrat_core::repo::ProtectedBranch, usize, Option<String>)>,
    ty: String,
    client_id: Option<String>,
    errors: Vec<String>,
    mr: Option<(Project, MergeRequest)>,
    note: Option<(Project, MergeRequest, Note)>,
    discussion: Option<(Project, MergeRequest, String)>,
    award: Option<Award>,
    toggled_on: Option<bool>,
    quick_actions: Option<(Map<String, Value>, Vec<String>)>,
    command_names: Vec<String>,
    todo: Option<i64>,
    todos: Option<Vec<i64>>,
    label: Option<Label>,
    timelog: Option<TimelogObj>,
    /// `errors` resolves to null (a non-null violation), as when `todoCreate` creates nothing.
    errors_null: bool,
    /// `commitCreate`: the commit, each action's content, and the pipeline path.
    commit: Option<(Project, labrat_git::Oid)>,
    content: Option<Vec<Option<String>>>,
    pipeline_path: Option<String>,
    /// EE approval rule mutations.
    approval_rule: Option<labrat_core::services::approvals::Rule>,
}

use serde_json::Map;

impl Object for Payload {
    fn typename(&self) -> String {
        self.ty.clone()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "clientMutationId" => Out::val(self.client_id.clone()),
            "commit" => match &self.commit {
                Some((p, oid)) => {
                    let r = repo::repo(cx.c, p);
                    Out::obj(CommitObj { p: p.clone(), oid: *oid, c: r.commit(oid)? })
                }
                None => Out::Null,
            },
            "content" => match &self.content {
                Some(list) => Out::List(list.iter().map(|v| Out::val(v.clone())).collect()),
                None => Out::Null,
            },
            "commitPipelinePath" => Out::val(self.pipeline_path.clone()),
            "approvalRule" => match &self.approval_rule {
                Some(r) => Out::obj(super::approvals::ApprovalProjectRuleObj { rule: r.clone() }),
                None => Out::Null,
            },
            "errors" if self.errors_null => Out::Null,
            "errors" => Out::List(self.errors.iter().map(|e| Out::val(e.clone())).collect()),
            "label" => match &self.label {
                Some(l) => Out::obj(LabelObj { l: l.clone() }),
                None => Out::Null,
            },
            "timelog" => match &self.timelog {
                Some(t) => Out::obj(t.clone()),
                None => Out::Null,
            },
            "todo" => match self.todo {
                Some(id) => Out::obj(TodoObj::load(cx.c, id)?),
                None => Out::Null,
            },
            "todos" => {
                let mut out = vec![];
                for id in self.todos.clone().unwrap_or_default() {
                    out.push(Out::obj(TodoObj::load(cx.c, id)?));
                }
                Out::List(out)
            }
            "mergeRequest" => match &self.mr {
                Some((p, m)) => Out::obj(MrObj::new(p.clone(), m.reload(cx.c)?)),
                None => Out::Null,
            },
            "note" => match &self.note {
                Some((p, m, n)) => Out::obj(NoteObj { p: p.clone(), mr: m.clone(), n: Note::find(cx.c, n.id)?.unwrap_or(n.clone()) }),
                None => Out::Null,
            },
            "discussion" => match &self.discussion {
                Some((p, m, d)) => Out::obj(DiscussionObj { p: p.clone(), mr: m.clone(), notes: Note::in_discussion(cx.c, d)? }),
                None => Out::Null,
            },
            "awardEmoji" => Out::opt(self.award.clone().map(|a| AwardObj { a })),
            "toggledOn" => Out::val(self.toggled_on),
            "branch" => match &self.branch {
                Some(b) => Out::obj(b.clone()),
                None => Out::Null,
            },
            "branchRule" => match &self.branch_rule {
                Some((pb, count, default)) => match Project::find(cx.c, pb.project_id)? {
                    Some(p) => Out::obj(BranchRuleObj { pb: pb.clone(), count: *count, default: default.clone(), p }),
                    None => Out::Null,
                },
                None => Out::Null,
            },
            // `PipelineCreatePayload.pipeline`: never created here (no CI configuration).
            "pipeline" if self.ty == "PipelineCreatePayload" => Out::Null,
            "quickActionsStatus" => match &self.quick_actions {
                Some((_, messages)) => Out::obj(JsonObj::new(
                    "QuickActionsStatus",
                    json!({
                        "messages": if messages.is_empty() { Value::Null } else { json!(messages) },
                        "commandNames": self.command_names.clone(),
                        "commandsOnly": self.note.is_none(),
                        "errorMessages": Value::Null,
                    }),
                )),
                None => Out::Null,
            },
            _ => return Err(not_implemented(&self.ty, field)),
        })
    }
}

/// `errors.full_messages` of a core error.
fn full_messages(e: &labrat_core::Error) -> Vec<String> {
    // Note validation errors carry the Ruby hash (`Note {:line_code=>["can't be blank"]}`).
    if let labrat_core::Error::BadRequest(Some(msg)) = e
        && let Some(hash) = msg.strip_prefix("Note {")
    {
        let mut out = vec![];
        for part in hash.trim_end_matches('}').split("], :") {
            let part = part.trim_start_matches(':');
            let Some((attr, list)) = part.split_once("=>[") else { continue };
            let mut h = attr.replace('_', " ");
            if let Some(f) = h.get(0..1) {
                h = f.to_uppercase() + &h[1..];
            }
            for m in list.trim_end_matches(']').split("\", \"") {
                let m = m.trim_matches('"');
                // Errors on `base` have no attribute in front.
                out.push(if attr == "base" { m.to_string() } else { format!("{h} {m}") });
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    match e {
        labrat_core::Error::Validation(list) => list
            .iter()
            .map(|(attr, msg)| {
                // `full_messages`: errors on `base` have no attribute in front.
                if attr == "base" {
                    return msg.clone();
                }
                let mut h = attr.replace('_', " ");
                if let Some(f) = h.get(0..1) {
                    h = f.to_uppercase() + &h[1..];
                }
                format!("{h} {msg}")
            })
            .collect(),
        // `errors.add(:validate_branches, ...)` and friends: the attribute is humanized in front.
        labrat_core::Error::Api(_, Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str())
            .map(|m| {
                if m.starts_with("Another open merge request") {
                    format!("Validate branches {m}")
                } else if m.starts_with("You can't use same project/branch") {
                    format!("Branch conflict {m}")
                } else {
                    m.to_string()
                }
            })
            .collect(),
        labrat_core::Error::Api(_, Value::String(m)) => vec![m.clone()],
        other => vec![other.to_string()],
    }
}

fn find_mr(cx: &Ctx, args: &Args) -> FResult<(Project, MergeRequest)> {
    let path = args.str("projectPath").unwrap_or_default();
    let iid: i64 = args.str("iid").unwrap_or_default().parse().unwrap_or(-1);
    let p = Project::find_by_full_path(cx.c, &path)?.ok_or_else(unavailable)?;
    let (p, perms) = readable_project(cx, p)?.ok_or_else(unavailable)?;
    if !perms.read_merge_request(&p) {
        return Err(unavailable());
    }
    let mr = MergeRequest::find_by_iid(cx.c, p.id, iid)?.ok_or_else(unavailable)?;
    Ok((p, mr))
}

fn me<'a>(cx: &'a Ctx) -> FResult<&'a User> {
    cx.user().ok_or_else(unavailable)
}

fn user_ids(cx: &Ctx, names: &[String]) -> FResult<Vec<i64>> {
    let mut out = vec![];
    for n in names {
        if let Some(u) = User::find_by_username(cx.c, n)? {
            out.push(u.id);
        }
    }
    Ok(out)
}

fn apply_mode(current: Vec<i64>, given: Vec<i64>, mode: &str) -> Vec<i64> {
    match mode {
        "APPEND" => {
            let mut v = current;
            for g in given {
                if !v.contains(&g) {
                    v.push(g);
                }
            }
            v
        }
        "REMOVE" => current.into_iter().filter(|c| !given.contains(c)).collect(),
        _ => given,
    }
}

fn noteable(cx: &Ctx, gid_str: &str) -> FResult<(Project, MergeRequest)> {
    let id = parse_gid(gid_str, &["MergeRequest"])?;
    let mr = MergeRequest::find(cx.c, id)?.ok_or_else(unavailable)?;
    let p = Project::find(cx.c, mr.target_project_id)?.ok_or_else(unavailable)?;
    let (p, perms) = readable_project(cx, p)?.ok_or_else(unavailable)?;
    if !perms.read_merge_request(&p) {
        return Err(unavailable());
    }
    Ok((p, mr))
}

fn find_note(cx: &Ctx, gid_str: &str) -> FResult<(Project, MergeRequest, Note)> {
    let id = parse_gid(gid_str, &["Note", "DiffNote", "DiscussionNote"])?;
    let n = Note::find(cx.c, id)?.ok_or_else(unavailable)?;
    if n.noteable_type != "MergeRequest" {
        return Err(unavailable());
    }
    let (p, mr) = noteable(cx, &format!("gid://gitlab/MergeRequest/{}", n.noteable_id))?;
    if !note_visible(cx, &p, &n) {
        return Err(unavailable());
    }
    Ok((p, mr, n))
}

impl Mutation {
    fn mr_mutation(&self, cx: &Ctx, field: &str, args: &Args, input: &Args) -> FResult<Payload> {
        let user = me(cx)?.clone();
        let config = &cx.req.app.config;
        let c = cx.c;
        let mut pl = Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), ..Default::default() };
        if field == "mergeRequestCreate" {
            let path = input.str("projectPath").unwrap_or_default();
            let p = Project::find_by_full_path(c, &path)?.ok_or_else(unavailable)?;
            let (p, perms) = readable_project(cx, p)?.ok_or_else(unavailable)?;
            if !perms.create_merge_request_in(&p) {
                return Err(unavailable());
            }
            let prm = mrs::MrParams {
                title: input.str("title"),
                description: input.has("description").then(|| input.str("description")),
                source_branch: input.str("sourceBranch"),
                target_branch: input.str("targetBranch"),
                labels: input.get("labels").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
                remove_source_branch: input.bool("removeSourceBranch"),
                merge_after: input.str("mergeAfter").map(|v| labrat_core::time::parse_time(&v)),
                ..Default::default()
            };
            let mut prm = prm;
            let cmds = crate::routes::merge_requests::take_quick_actions(&mut prm);
            match mrs::create(c, config, &p, &user, prm) {
                Ok(mr) => {
                    if !cmds.is_empty() {
                        let ctx = labrat_core::services::comments::CreateParams { new_record: true, ..Default::default() };
                        labrat_core::services::quick_actions::execute(c, config, &p, &mr, &user, &cmds, &ctx)?;
                    }
                    let mr = mr.reload(c)?;
                    pl.mr = Some((p, mr));
                }
                Err(e @ (labrat_core::Error::Validation(_) | labrat_core::Error::Api(..))) => pl.errors = full_messages(&e),
                Err(e) => return Err(e.into()),
            }
            return Ok(pl);
        }
        let (p, mr) = find_mr(cx, input)?;
        let can_update = mrs::can_update(c, &p, &mr, &user)?;
        let update = |prm: mrs::MrParams, pl: &mut Payload| -> FResult<()> {
            match mrs::update(c, config, &p, &mr, &user, prm) {
                Ok(_) => {}
                Err(e @ (labrat_core::Error::Validation(_) | labrat_core::Error::Api(..))) => pl.errors = full_messages(&e),
                Err(e) => return Err(e.into()),
            }
            Ok(())
        };
        match field {
            "mergeRequestUpdate" => {
                if !can_update {
                    return Err(unavailable());
                }
                // `timeEstimate` is validated before anything changes.
                let estimate = match input.str("timeEstimate") {
                    // `TimeTrackingFormatter.parse(time_estimate, keep_zero: true)`.
                    Some(t) => match labrat_core::chronic::parse_time(&t, true).map(|n| n.to_i64()) {
                        None => return Err(FieldError::new("timeEstimate must be formatted correctly, for example `1h 30m`")),
                        Some(v) if v < 0 => {
                            return Err(FieldError::new("timeEstimate must be greater than or equal to zero. Remember that every new timeEstimate overwrites the previous value."));
                        }
                        Some(v) => Some(v),
                    },
                    None => None,
                };
                if let Some(v) = estimate {
                    crate::routes::merge_requests::set_time_estimate(c, &p, &mr, &user, v)?;
                }
                let state_event = input.str("state").map(|s| match s.as_str() {
                    "CLOSED" | "closed" => "close".to_string(),
                    _ => "reopen".to_string(),
                });
                let prm = mrs::MrParams {
                    title: input.str("title"),
                    description: input.has("description").then(|| input.str("description")),
                    target_branch: input.str("targetBranch"),
                    remove_source_branch: input.bool("removeSourceBranch"),
                    state_event,
                    merge_after: input.str("mergeAfter").map(|v| labrat_core::time::parse_time(&v)),
                    ..Default::default()
                };
                let mut prm = prm;
                let cmds = crate::routes::merge_requests::take_quick_actions(&mut prm);
                if !cmds.is_empty() {
                    // One update with the description's commands merged in (`handle_quick_actions`).
                    let ctx = labrat_core::services::comments::CreateParams { return_update: true, ..Default::default() };
                    labrat_core::services::quick_actions::execute(c, config, &p, &mr, &user, &cmds, &ctx)?;
                    prm.merge_commands(labrat_core::services::quick_actions::take_update());
                }
                // A state event the state machine refuses leaves an error on the MR; a save (any
                // other attribute) would validate again and clear it.
                let refused = match prm.state_event.as_deref() {
                    Some("close") => mr.state != "opened" && mr.state != "locked",
                    Some("reopen") => mr.state != "closed",
                    _ => false,
                };
                let only_state = input.0.iter().all(|(k, v)| v.is_null() || matches!(k.as_str(), "projectPath" | "iid" | "state" | "clientMutationId"));
                let event = prm.state_event.clone();
                if only_state && event.is_some() {
                    // `args.compact` leaves only `state_event`: `change_state` runs the close or
                    // reopen service, and with no params left nothing is saved (no `update`
                    // hook, no `updated_by`).
                    mrs::apply_state_event(c, config, &p, &mr, &user, event.as_deref())?;
                } else {
                    update(prm, &mut pl)?;
                }
                if refused && only_state && pl.errors.is_empty() {
                    pl.errors = vec![format!("State cannot transition via \"{}\"", event.unwrap_or_default())];
                }
            }
            "mergeRequestSetDraft" => {
                if !can_update {
                    return Err(unavailable());
                }
                let draft = input.bool("draft").unwrap_or(false);
                let title = if draft { mrs::draft_title(&mr.title) } else { mrs::draftless_title(&mr.title) };
                update(mrs::MrParams { title: Some(title), ..Default::default() }, &mut pl)?;
            }
            "mergeRequestSetLocked" => {
                if !can_update {
                    return Err(unavailable());
                }
                update(mrs::MrParams { discussion_locked: input.bool("locked"), ..Default::default() }, &mut pl)?;
            }
            "mergeRequestSetAssignees" | "mergeRequestSetReviewers" => {
                if !can_update {
                    return Err(unavailable());
                }
                let key = if field == "mergeRequestSetAssignees" { "assigneeUsernames" } else { "reviewerUsernames" };
                let names: Vec<String> = input.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let given = user_ids(cx, &names)?;
                let mode = input.str("operationMode").unwrap_or("REPLACE".into());
                let current = if field == "mergeRequestSetAssignees" { mr.assignee_ids(c)? } else { mr.reviewer_ids(c)? };
                let new_ids = apply_mode(current.clone(), given, &mode);
                // `new_user_ids`: everyone in the new set must be able to read the MR.
                let attr = if field == "mergeRequestSetAssignees" { "Assignees" } else { "Reviewers" };
                let mut denied = vec![];
                for id in &new_ids {
                    if let Some(u) = User::find(c, *id)?
                        && !ProjectPerms::load(c, Some(&u), &p)?.read_merge_request(&p)
                    {
                        denied.push(format!("{attr} Cannot assign @{} to !{}", u.username, mr.iid));
                    }
                }
                if !denied.is_empty() {
                    pl.errors = denied;
                    pl.mr = Some((p.clone(), mr.reload(c)?));
                    return Ok(pl);
                }
                // `UpdateAssigneesService` / `UpdateReviewersService` return early when the set
                // doesn't change: no touch, no hook.
                let same = {
                    let (mut a, mut b) = (current.clone(), new_ids.clone());
                    a.sort();
                    a.dedup();
                    b.sort();
                    b.dedup();
                    a == b
                };
                if !same {
                    let prm = if field == "mergeRequestSetAssignees" {
                        mrs::MrParams { assignee_ids: Some(new_ids), ..Default::default() }
                    } else {
                        mrs::MrParams { reviewer_ids: Some(new_ids), ..Default::default() }
                    };
                    update(prm, &mut pl)?;
                }
            }
            "mergeRequestSetLabels" => {
                if !can_update {
                    return Err(unavailable());
                }
                let ids: Vec<i64> = input
                    .get("labelIds")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str()).filter_map(|s| parse_gid(s, &[]).ok()).collect())
                    .unwrap_or_default();
                let titles: Vec<String> = ids.iter().filter_map(|id| Label::find(c, *id).ok().flatten()).map(|l| l.title).collect();
                let mode = input.str("operationMode").unwrap_or("REPLACE".into());
                let prm = match mode.as_str() {
                    "APPEND" => mrs::MrParams { add_labels: Some(titles), ..Default::default() },
                    "REMOVE" => mrs::MrParams { remove_labels: Some(titles), ..Default::default() },
                    _ => mrs::MrParams { labels: Some(titles), ..Default::default() },
                };
                update(prm, &mut pl)?;
            }
            "mergeRequestSetMilestone" => {
                if !can_update {
                    return Err(unavailable());
                }
                let ms = input.str("milestoneId").map(|s| parse_gid(&s, &["Milestone"])).transpose()?;
                if let Some(id) = ms {
                    let exists: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM milestones WHERE id = ?1)", [id], |r| r.get(0))?;
                    if !exists {
                        return Err(no_object("milestoneId", &input.str("milestoneId").unwrap_or_default()));
                    }
                }
                update(mrs::MrParams { milestone_id: Some(ms), ..Default::default() }, &mut pl)?;
            }
            "mergeRequestSetSubscription" => {
                subscriptions::set(c, user.id, "MergeRequest", mr.id, input.bool("subscribedState").unwrap_or(false))?;
            }
            "mergeRequestAccept" => {
                if !mrs::can_be_merged_by(c, &p, &mr, &user)? {
                    return Err(unavailable());
                }
                // `Mutations::MergeRequests::Accept`: the first failing check wins, then the merge
                // runs asynchronously, so the returned MR is still open.
                // With a strategy, `auto_merge_eligible?` skips the checks auto-merge can wait for.
                let strategy = input.str("strategy");
                let mergeable_for_strategy = match &strategy {
                    Some(_) => mergeability::auto_merge_blocking(c, &p, &mr)?.is_none(),
                    None => mergeability::mergeable(c, config, &p, &mr)?,
                };
                let error = if mr.merge_when_pipeline_succeeds {
                    Some("The merge request is already scheduled to be merged")
                } else if !mergeable_for_strategy {
                    Some("This branch cannot be merged")
                } else if input.str("sha") != mr.diff_head_sha(c, &p)? {
                    Some("The merge-head is not at the anticipated SHA")
                } else {
                    None
                };
                match error {
                    Some(e) => pl.errors = vec![e.to_string()],
                    None if strategy.is_some() => {
                        c.execute(
                            "UPDATE merge_requests SET merge_error = NULL, squash = ?1 WHERE id = ?2",
                            rusqlite::params![input.bool("squash").unwrap_or(false), mr.id],
                        )?;
                        let mr = mr.reload(c)?;
                        // `AutoMergeService#execute`: only merge-when-checks-pass exists, and
                        // it is available while the MR can't merge yet or its pipeline still runs.
                        let can_merge_now = mergeability::mergeable(c, config, &p, &mr)?;
                        let dhp = labrat_core::services::ci::diff_head_pipeline(c, &p, &mr)?;
                        let in_progress = labrat_core::services::ci::in_progress(&p, dhp.as_ref());
                        if strategy.as_deref() == Some("MERGE_WHEN_CHECKS_PASS") && (!can_merge_now || in_progress) {
                            let mp = labrat_core::services::merge_requests::merge::MergeParams {
                                commit_message: input.str("commitMessage"),
                                squash_commit_message: input.str("squashCommitMessage"),
                                should_remove_source_branch: input.bool("shouldRemoveSourceBranch"),
                                sha: input.str("sha"),
                                ..Default::default()
                            };
                            labrat_core::services::merge_requests::merge::enable_auto_merge(c, &p, &mr, &user, &mp)?;
                        } else {
                            pl.errors = vec!["The merge failed".into()];
                        }
                        pl.mr = Some((p.clone(), mr.reload(c)?));
                        return Ok(pl);
                    }
                    None => {
                        c.execute(
                            "UPDATE merge_requests SET merge_error = NULL, squash = ?1 WHERE id = ?2",
                            rusqlite::params![input.bool("squash").unwrap_or(false), mr.id],
                        )?;
                        labrat_core::jobs::enqueue(
                            c,
                            "mr_merge",
                            json!({"merge_request_id": mr.id, "user_id": user.id, "sha": input.str("sha"),
                                   "commit_message": input.str("commitMessage"), "squash_commit_message": input.str("squashCommitMessage"),
                                   "should_remove_source_branch": input.bool("shouldRemoveSourceBranch")}),
                        )?;
                    }
                }
            }
            "mergeRequestReviewerRereview" => {
                if !can_update {
                    return Err(unavailable());
                }
                let raw = input.str("userId").unwrap_or_default();
                let uid = parse_gid(&raw, &["User"])?;
                let target = User::find(c, uid)?.ok_or_else(|| no_object("userId", &raw))?;
                // `with_valid_reviewer`: "Reviewer not found" when the user isn't a reviewer.
                if !reviews::rerequest(c, config, &p, &mr, &user, &target)? {
                    pl.errors = vec!["Reviewer not found".into()];
                }
            }
            "mergeRequestRequestChanges" => {
                if !can_update {
                    return Err(unavailable());
                }
                if let Some(e) = reviews::update_reviewer_state(c, config, &p, &mr, &user, "requested_changes")? {
                    pl.errors = vec![e];
                }
            }
            _ => return Err(not_implemented("Mutation", field)),
        }
        pl.mr = Some((p, mr));
        let _ = args;
        Ok(pl)
    }

    fn note_mutation(&self, cx: &Ctx, field: &str, input: &Args) -> FResult<Payload> {
        use labrat_core::services::comments::{self, CreateParams, Outcome};
        let user = me(cx)?.clone();
        let config = &cx.req.app.config;
        let c = cx.c;
        let mut pl = Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), ..Default::default() };
        if let Some(body) = input.str("body") {
            pl.command_names = labrat_core::services::quick_actions::extract(&body).commands.into_iter().map(|(n, _)| n).collect();
        }
        let outcome = |pl: &mut Payload, p: &Project, mr: &MergeRequest, r: labrat_core::Result<Outcome>| -> FResult<()> {
            match r {
                Ok(Outcome::Created(n, _changes)) => {
                    pl.quick_actions = Some((Map::new(), vec![]));
                    pl.note = Some((p.clone(), mr.clone(), n));
                }
                Ok(Outcome::Commands(changes, messages)) => {
                    pl.quick_actions = Some((changes, messages));
                }
                Err(labrat_core::Error::Forbidden(_)) => return Err(unavailable()),
                Err(e @ (labrat_core::Error::Validation(_) | labrat_core::Error::Api(..) | labrat_core::Error::BadRequest(_))) => pl.errors = full_messages(&e),
                Err(e) => return Err(e.into()),
            }
            Ok(())
        };
        match field {
            "createNote" | "createDiscussion" | "createDiffNote" | "createLatestDiffNote" | "createImageDiffNote" => {
                let (p, mr) = noteable(cx, &input.str("noteableId").unwrap_or_default())?;
                let reply_to = input.str("discussionId").map(|d| d.trim_start_matches("gid://gitlab/Discussion/").to_string());
                if let Some(d) = &reply_to {
                    let exists: bool = c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM notes WHERE discussion_id = ?1 AND noteable_type = 'MergeRequest' AND noteable_id = ?2)",
                        rusqlite::params![d, mr.id],
                        |r| r.get(0),
                    )?;
                    if !exists {
                        return Err(FieldError::new("The discussion does not exist or you don't have permission to perform this action"));
                    }
                }
                if field == "createDiffNote"
                    && let Some(pos) = input.get("position")
                    && pos["oldLine"].is_null()
                    && pos["newLine"].is_null()
                {
                    return Err(FieldError::new("position oldLine or newLine arguments are required"));
                }
                // `createLatestDiffNote`: a line of the latest diff, checked against `headSha`.
                let latest = if field == "createLatestDiffNote" {
                    let d = mr.latest_diff(c)?;
                    let head = d.as_ref().and_then(|d| d.head_commit_sha.clone());
                    if input.str("headSha") != head {
                        return Err(FieldError::new(
                            "The provided head_sha does not match the current merge request diff. The diff may have been updated since you last viewed it.",
                        ));
                    }
                    let d = d.expect("diff exists");
                    let path = input.str("filePath").unwrap_or_default();
                    let mut v = json!({
                        "base_sha": d.base_commit_sha, "start_sha": d.start_commit_sha, "head_sha": d.head_commit_sha,
                        "old_path": path, "new_path": path, "position_type": "text",
                        "old_line": input.get("oldLine").cloned().unwrap_or(Value::Null), "new_line": input.get("newLine").cloned().unwrap_or(Value::Null),
                    });
                    if v["old_line"].is_null() {
                        v.as_object_mut().unwrap().shift_remove("old_line");
                    }
                    Some(Position::parse(&v.to_string()))
                } else {
                    None
                };
                let position = match (latest, input.get("position")) {
                    (Some(p), _) => p,
                    (None, Some(pos)) if field == "createImageDiffNote" => Position::parse(
                        &json!({
                            "base_sha": pos["baseSha"], "start_sha": pos["startSha"], "head_sha": pos["headSha"],
                            "old_path": pos["paths"]["oldPath"], "new_path": pos["paths"]["newPath"], "position_type": "image",
                            "width": pos["width"], "height": pos["height"], "x": pos["x"], "y": pos["y"],
                        })
                        .to_string(),
                    ),
                    (None, Some(pos)) => {
                        let mut v = json!({
                            "base_sha": pos["baseSha"], "start_sha": pos["startSha"], "head_sha": pos["headSha"],
                            "old_path": pos["paths"]["oldPath"], "new_path": pos["paths"]["newPath"],
                            "old_line": pos["oldLine"], "new_line": pos["newLine"], "position_type": "text",
                        });
                        if pos["oldLine"].is_null() {
                            v.as_object_mut().unwrap().shift_remove("old_line");
                        }
                        Position::parse(&v.to_string())
                    }
                    (None, None) => None,
                };
                let prm = CreateParams {
                    body: input.str("body").unwrap_or_default(),
                    internal: input.bool("internal").unwrap_or(false),
                    thread: field != "createNote",
                    position,
                    reply_to,
                    merge_request_diff_head_sha: input.str("mergeRequestDiffHeadSha"),
                    ..Default::default()
                };
                let r = comments::create(c, config, &p, &mr, &user, prm);
                outcome(&mut pl, &p, &mr, r)?;
            }
            "updateNote" => {
                let (p, mr, n) = find_note(cx, &input.str("id").unwrap_or_default())?;
                // `admin_note`: the author, or a maintainer of the project.
                let perms = ProjectPerms::load(c, Some(&user), &p)?;
                if n.system || (n.author_id != user.id && !perms.admin_project() && !user.admin) {
                    return Err(unavailable());
                }
                let body = input.str("body").unwrap_or(n.note.clone());
                let r = comments::update(c, config, &p, &mr, &user, &n, &body);
                outcome(&mut pl, &p, &mr, r)?;
                if pl.note.is_none()
                    && let Some(still) = Note::find(c, n.id)? {
                        pl.note = Some((p, mr, still));
                    }
            }
            "destroyNote" => {
                let (_, _, n) = find_note(cx, &input.str("id").unwrap_or_default())?;
                if n.system || (n.author_id != user.id && !user.admin) {
                    return Err(unavailable());
                }
                comments::delete(c, &n)?;
            }
            "noteConvertToThread" => {
                let raw = input.str("id").unwrap_or_default();
                // `loads: Types::Notes::NoteType`.
                if Note::find(c, parse_gid(&raw, &["Note", "DiffNote", "DiscussionNote"])?)?.is_none() {
                    return Err(no_object("id", &raw));
                }
                let (p, mr, n) = find_note(cx, &raw)?;
                // Only an individual, non-system comment can become a thread.
                if n.kind.is_some() || n.system {
                    return Err(FieldError::new("Note cannot be converted to a resolvable thread"));
                }
                c.execute("UPDATE notes SET type = 'DiscussionNote', resolvable = 1 WHERE id = ?1", [n.id])?;
                pl.note = Some((p, mr, n));
            }
            _ => return Err(not_implemented("Mutation", field)),
        }
        Ok(pl)
    }
}

fn payload_type(field: &str) -> String {
    let mut ch = field.chars();
    let first = ch.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    format!("{first}{}Payload", ch.as_str())
}

impl Object for Mutation {
    fn typename(&self) -> String {
        "Mutation".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let input = Args(args.get("input").and_then(Value::as_object).cloned().unwrap_or_default());
        let pl = match field {
            "mergeRequestUpdateApprovalRule" | "branchRuleApprovalProjectRuleCreate" | "approvalProjectRuleUpdate" | "approvalProjectRuleDelete" => {
                // EE names these payload types after the mutation, lowercase first letter included.
                let ty = if field.starts_with("mergeRequest") { payload_type(field) } else { format!("{field}Payload") };
                let mut pl = Payload { ty, client_id: input.str("clientMutationId"), ..Default::default() };
                let (errors, rule) = super::approvals::mutation(cx, field, &input)?;
                pl.errors = errors;
                pl.approval_rule = rule;
                pl
            }
            f if f.starts_with("mergeRequest") => self.mr_mutation(cx, field, args, &input)?,
            "commitCreate" => commit_create(cx, &input)?,
            "labelUpdate" => {
                me(cx)?;
                let id = parse_gid(&input.str("id").unwrap_or_default(), &["ProjectLabel", "GroupLabel", "Label"])?;
                let Some(label) = labrat_core::services::labels::Label::find(cx.c, id)? else { return Err(unavailable()) };
                if let Some(a) = input.bool("archived") {
                    cx.c.execute("UPDATE labels SET archived = ?1, updated_at = ?2 WHERE id = ?3", rusqlite::params![a, labrat_core::time::now(), label.id])?;
                }
                let label = labrat_core::services::labels::Label::find(cx.c, id)?;
                Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), label, ..Default::default() }
            }
            "repositionImageDiffNote" | "updateImageDiffNote" => {
                let pos = input.get("position").and_then(Value::as_object).cloned();
                if field == "repositionImageDiffNote" && pos.as_ref().is_none_or(|m| m.values().all(Value::is_null)) {
                    return Err(FieldError::new("At least one property of `UpdateDiffImagePositionInput` must be set"));
                }
                let (p, mr, n) = find_note(cx, &input.str("id").unwrap_or_default())?;
                let user = me(cx)?.clone();
                if n.author_id != user.id && !user.admin {
                    return Err(unavailable());
                }
                if let Some(m) = pos {
                    let mut current: Value = n.position.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(json!({}));
                    for (k, v) in m {
                        if !v.is_null() {
                            current[k] = v;
                        }
                    }
                    cx.c.execute("UPDATE notes SET position = ?1, updated_at = ?2 WHERE id = ?3", rusqlite::params![current.to_string(), labrat_core::time::now(), n.id])?;
                }
                if let Some(body) = input.str("body") {
                    cx.c.execute("UPDATE notes SET note = ?1, updated_at = ?2 WHERE id = ?3", rusqlite::params![body, labrat_core::time::now(), n.id])?;
                }
                let n = Note::find(cx.c, n.id)?.unwrap_or(n);
                Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), note: Some((p, mr, n)), ..Default::default() }
            }
            "createNote" | "createDiffNote" | "createDiscussion" | "updateNote" | "destroyNote" | "noteConvertToThread" | "createLatestDiffNote" | "createImageDiffNote" => {
                self.note_mutation(cx, field, &input)?
            }
            "todoCreate" | "todoMarkDone" | "todoRestore" | "todoRestoreMany" | "todosMarkAllDone" | "todoSnooze" | "todoUnSnooze" | "todoSnoozeMany"
            | "todoUnsnoozeMany" | "todoResolveMany" | "todoDeleteMany" | "todoDeleteAllDone" => {
                let user = me(cx)?.clone();
                let c = cx.c;
                let mut pl = Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), ..Default::default() };
                let own = |gid_s: &str| -> FResult<i64> {
                    let id = parse_gid(gid_s, &["Todo"])?;
                    let owner: Option<i64> = c.query_row("SELECT user_id FROM todos WHERE id = ?1", [id], |r| r.get(0)).ok();
                    if owner != Some(user.id) {
                        return Err(unavailable());
                    }
                    Ok(id)
                };
                match field {
                    "todoCreate" => {
                        let (p, mr) = noteable(cx, &input.str("targetId").unwrap_or_default())?;
                        match labrat_core::services::todos::mark(c, &p, &mr, &user)? {
                            Some(id) => pl.todo = Some(id),
                            None => pl.errors_null = true,
                        }
                    }
                    "todoMarkDone" | "todoRestore" => {
                        let id = own(&input.str("id").unwrap_or_default())?;
                        let state = if field == "todoMarkDone" { "done" } else { "pending" };
                        labrat_core::services::todos::set_state(c, user.id, &[id], state)?;
                        pl.todo = Some(id);
                    }
                    "todoSnooze" | "todoUnSnooze" => {
                        let id = own(&input.str("id").unwrap_or_default())?;
                        let until = if field == "todoSnooze" { input.str("snoozeUntil").and_then(|t| time::parse_time(&t)) } else { None };
                        c.execute("UPDATE todos SET snoozed_until = ?1, updated_at = ?2 WHERE id = ?3", rusqlite::params![until, time::now(), id])?;
                        pl.todo = Some(id);
                    }
                    "todoSnoozeMany" | "todoUnsnoozeMany" | "todoResolveMany" | "todoDeleteMany" => {
                        if input.get("ids").and_then(Value::as_array).is_some_and(|a| a.len() > 100) {
                            return Err(FieldError::new("Too many to-do items requested."));
                        }
                        let mut ids = vec![];
                        for g in input.get("ids").and_then(Value::as_array).cloned().unwrap_or_default() {
                            ids.push(own(g.as_str().unwrap_or_default())?);
                        }
                        ids.sort();
                        match field {
                            "todoSnoozeMany" | "todoUnsnoozeMany" => {
                                let until = if field == "todoSnoozeMany" { input.str("snoozeUntil").and_then(|t| time::parse_time(&t)) } else { None };
                                for id in &ids {
                                    c.execute("UPDATE todos SET snoozed_until = ?1, updated_at = ?2 WHERE id = ?3", rusqlite::params![until, time::now(), id])?;
                                }
                                pl.todos = Some(ids);
                            }
                            "todoResolveMany" => {
                                labrat_core::services::todos::set_state(c, user.id, &ids, "done")?;
                                pl.todos = Some(ids);
                            }
                            // Only to-dos that are already done are deleted.
                            _ => {
                                for id in &ids {
                                    c.execute("DELETE FROM todos WHERE id = ?1 AND state = 'done'", [id])?;
                                }
                            }
                        }
                    }
                    "todoDeleteAllDone" => {
                        let before = input.str("updatedBefore").and_then(|t| time::parse_time(&t)).unwrap_or_else(time::now);
                        c.execute("DELETE FROM todos WHERE user_id = ?1 AND state = 'done' AND updated_at <= ?2", rusqlite::params![user.id, before])?;
                    }
                    "todoRestoreMany" => {
                        let mut ids = vec![];
                        for g in input.get("ids").and_then(Value::as_array).cloned().unwrap_or_default() {
                            ids.push(own(g.as_str().unwrap_or_default())?);
                        }
                        pl.todos = Some(labrat_core::services::todos::set_state(c, user.id, &ids, "pending")?);
                    }
                    _ => {
                        let target = match input.str("targetId") {
                            Some(t) => Some(noteable(cx, &t).map_err(|_| FieldError::new(format!("Resource not available: {t}")))?.1.id),
                            None => None,
                        };
                        let mut st = c.prepare("SELECT id FROM todos WHERE user_id = ?1 AND state = 'pending' AND (?2 IS NULL OR (target_type = 'MergeRequest' AND target_id = ?2))")?;
                        let ids: Vec<i64> = st.query_map(rusqlite::params![user.id, target], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                        pl.todos = Some(labrat_core::services::todos::set_state(c, user.id, &ids, "done")?);
                    }
                }
                pl
            }
            // `Ci::CreatePipelineService`: projects here have no CI configuration, so no pipeline
            // runs.
            "pipelineCreate" => {
                let user = me(cx)?.clone();
                let path = input.str("projectPath").unwrap_or_default();
                let p = Project::find_by_full_path(cx.c, &path)?.ok_or_else(unavailable)?;
                let perms = ProjectPerms::load(cx.c, Some(&user), &p)?;
                if perms.level < labrat_core::model::access::DEVELOPER && !user.admin {
                    return Err(unavailable());
                }
                Payload {
                    ty: payload_type(field),
                    client_id: input.str("clientMutationId"),
                    errors: vec!["The pipeline did not run. Review the workflow:rules configuration for the pipeline.".into()],
                    ..Default::default()
                }
            }
            "createBranch" | "branchDelete" | "branchRuleCreate" => {
                let user = me(cx)?.clone();
                let c = cx.c;
                let mut pl = Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), ..Default::default() };
                let path = input.str("projectPath").unwrap_or_default();
                let p = Project::find_by_full_path(c, &path)?.ok_or_else(unavailable)?;
                let perms = ProjectPerms::load(c, Some(&user), &p)?;
                let allowed = if field == "branchRuleCreate" { perms.admin_project() } else { perms.developer() };
                if !allowed && !user.admin {
                    return Err(unavailable());
                }
                let name = input.str("name").unwrap_or_default();
                let config = &cx.req.app.config;
                match field {
                    "createBranch" => match crate::routes::repository::create_branch_in(c, config, &p, &user, &name, &input.str("ref").unwrap_or_default()) {
                        Ok(oid) => pl.branch = Some(BranchObj { p: p.clone(), name, oid }),
                        Err(e) => pl.errors = vec![api_error_text(&e)],
                    },
                    "branchDelete" => match crate::routes::repository::delete_branch_in(c, config, &p, &perms, &user, &name) {
                        Ok(Some(())) => {}
                        Ok(None) => pl.errors = vec!["No such branch".into()],
                        // A refused deletion still returns the branch.
                        Err(e) => {
                            pl.errors = vec![api_error_text(&e)];
                            if let Some(oid) = repo::repo(c, &p).branch(&name)? {
                                pl.branch = Some(BranchObj { p: p.clone(), name, oid });
                            }
                        }
                    },
                    _ => {
                        // `BranchRules::CreateService`: a protected branch with the default levels.
                        if labrat_core::repo::protected_branches(c, p.id)?.iter().any(|pb| pb.name == name) {
                            pl.errors = vec!["Name has already been taken".into()];
                        } else {
                            labrat_core::repo::create_protected_branch(c, &p, &name, 40, 40, false)?;
                            let pb = labrat_core::repo::protected_branches(c, p.id)?.into_iter().find(|pb| pb.name == name).expect("just created");
                            let branches: Vec<String> = repo::repo(c, &p).branches()?.into_iter().map(|(n, _)| n).collect();
                            let count = branches.iter().filter(|b| labrat_core::repo::ref_matches(&pb.name, b)).count();
                            pl.branch_rule = Some((pb, count, p.default_branch.clone()));
                        }
                    }
                }
                pl
            }
            "labelCreate" => {
                let user = me(cx)?.clone();
                let c = cx.c;
                let mut pl = Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), ..Default::default() };
                let path = input.str("projectPath").unwrap_or_default();
                let p = Project::find_by_full_path(c, &path)?.ok_or_else(unavailable)?;
                let perms = ProjectPerms::load(c, Some(&user), &p)?;
                if !perms.reporter() && !user.admin {
                    return Err(unavailable());
                }
                let title = input.str("title").unwrap_or_default().trim().to_string();
                let raw = input.str("color").unwrap_or("#6699cc".into());
                let color = crate::routes::labels::color(&raw);
                let mut errors = vec![];
                if labrat_core::services::labels::available(c, &p)?.iter().any(|l| l.title == title) {
                    errors.push("Title has already been taken".to_string());
                }
                if color.is_none() {
                    errors.push("Color must be a valid color code".to_string());
                }
                if errors.is_empty() {
                    let l = labrat_core::services::labels::create(c, Some(p.id), None, &title, &color.unwrap(), input.str("description").as_deref(), None)?;
                    pl.label = Some(l);
                } else {
                    pl.errors = errors;
                }
                pl
            }
            "timelogCreate" | "timelogDelete" => {
                let user = me(cx)?.clone();
                let c = cx.c;
                let mut pl = Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), ..Default::default() };
                let fmt = |s: i64| -> String { labrat_core::chronic::human(s) };
                if field == "timelogCreate" {
                    let (p, mr) = noteable(cx, &input.str("issuableId").unwrap_or_default())?;
                    if !ProjectPerms::load(c, Some(&user), &p)?.reporter() && !user.admin {
                        return Err(unavailable());
                    }
                    let Some(secs) = crate::routes::merge_requests::parse_duration_pub(&input.str("timeSpent").unwrap_or_default()) else {
                        pl.errors = vec!["Time spent must start with a number.".into()];
                        return Ok(Out::obj(pl));
                    };
                    let spent_at = input.str("spentAt").and_then(|t| time::parse_time(&t)).unwrap_or_else(time::now);
                    let summary = input.str("summary");
                    c.execute(
                        "INSERT INTO timelogs (merge_request_id, user_id, time_spent, summary, spent_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![mr.id, user.id, secs, summary, spent_at, time::now()],
                    )?;
                    let id = c.last_insert_rowid();
                    c.execute("UPDATE merge_requests SET total_time_spent = max(0, total_time_spent + ?1) WHERE id = ?2", rusqlite::params![secs, mr.id])?;
                    let body = format!("added {} of time spent at {}", fmt(secs), time::ruby_utc(spent_at));
                    labrat_core::services::notes::system_note(c, mr.id, p.id, &user, &body, "time_tracking")?;
                    pl.timelog = Some(TimelogObj::load(c, id)?);
                } else {
                    let id = parse_gid(&input.str("id").unwrap_or_default(), &["Timelog"])?;
                    let t = TimelogObj::load(c, id).map_err(|_| unavailable())?;
                    let mr = MergeRequest::find(c, t.mr_id)?.ok_or_else(unavailable)?;
                    let p = Project::find(c, mr.target_project_id)?.ok_or_else(unavailable)?;
                    if t.user_id != user.id && !ProjectPerms::load(c, Some(&user), &p)?.admin_project() && !user.admin {
                        return Err(unavailable());
                    }
                    c.execute("DELETE FROM timelogs WHERE id = ?1", [id])?;
                    c.execute("UPDATE merge_requests SET total_time_spent = max(0, total_time_spent - ?1) WHERE id = ?2", rusqlite::params![t.time_spent, mr.id])?;
                    let body = format!("deleted {} of spent time from {}", fmt(t.time_spent), time::ruby_utc(t.spent_at));
                    labrat_core::services::notes::system_note(c, mr.id, p.id, &user, &body, "time_tracking")?;
                    pl.timelog = Some(t);
                }
                pl
            }
            "discussionToggleResolve" => {
                let user = me(cx)?.clone();
                let did = input.str("id").unwrap_or_default();
                let d = did.strip_prefix("gid://gitlab/Discussion/").ok_or_else(|| FieldError::new(format!("\"{did}\" is not a valid Global ID")))?.to_string();
                let first = Note::in_discussion(cx.c, &d)?.into_iter().next().ok_or_else(unavailable)?;
                let (p, mr) = noteable(cx, &format!("gid://gitlab/MergeRequest/{}", first.noteable_id))?;
                if !first.resolvable {
                    return Err(unavailable());
                }
                labrat_core::services::comments::resolve_discussion(cx.c, &cx.req.app.config, &p, &mr, &user, &d, input.bool("resolve").unwrap_or(true))?;
                Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), discussion: Some((p, mr, d)), ..Default::default() }
            }
            "awardEmojiAdd" | "awardEmojiRemove" | "awardEmojiToggle" => {
                let user = me(cx)?.clone();
                let target = input.str("awardableId").unwrap_or_default();
                let (kind, id, mr_id) = if target.contains("/MergeRequest/") {
                    let id = parse_gid(&target, &["MergeRequest"])?;
                    ("MergeRequest", id, id)
                } else {
                    let id = parse_gid(&target, &["Note", "DiffNote", "DiscussionNote"])?;
                    let n = Note::find(cx.c, id)?.ok_or_else(unavailable)?;
                    if n.system {
                        return Err(FieldError::new("You cannot add emoji reactions to this resource."));
                    }
                    ("Note", id, n.noteable_id)
                };
                let (ep, emr) = noteable(cx, &format!("gid://gitlab/MergeRequest/{mr_id}"))?;
                let enote = if kind == "Note" { Note::find(cx.c, id)? } else { None };
                let config = &cx.req.app.config;
                let name = input.str("name").unwrap_or_default();
                let mut pl = Payload { ty: payload_type(field), client_id: input.str("clientMutationId"), ..Default::default() };
                let canon = emoji::canonical(&name).map(str::to_string);
                let existing = emoji::list(cx.c, kind, id)?.into_iter().find(|a| a.user_id == user.id && Some(&a.name) == canon.as_ref());
                match (field, existing) {
                    ("awardEmojiRemove", Some(a)) | ("awardEmojiToggle", Some(a)) => {
                        emoji::remove(cx.c, &a)?;
                        labrat_core::services::hooks::emoji_event(cx.c, config, &ep, &emr, enote.as_ref(), &user, &a, "revoke")?;
                        if field == "awardEmojiToggle" {
                            pl.toggled_on = Some(false);
                        }
                    }
                    ("awardEmojiRemove", None) => pl.errors = vec!["User has not awarded emoji of type ".to_string() + &name + " on the awardable"],
                    (_, existing) => {
                        let system = kind == "Note" && Note::find(cx.c, id)?.is_some_and(|n| n.system);
                        let _ = existing;
                        match emoji::add(cx.c, &user, kind, id, &name, system)? {
                            Ok(a) => {
                                labrat_core::services::hooks::emoji_event(cx.c, config, &ep, &emr, enote.as_ref(), &user, &a, "award")?;
                                pl.award = Some(a);
                                if field == "awardEmojiToggle" {
                                    pl.toggled_on = Some(true);
                                }
                            }
                            Err(msg) => pl.errors = vec![msg],
                        }
                    }
                }
                pl
            }
            _ => return Err(not_implemented("Mutation", field)),
        };
        Ok(Out::obj(pl))
    }
}

/// A time log entry (`Types::TimelogType`).
#[derive(Clone)]
struct TimelogObj {
    id: i64,
    mr_id: i64,
    user_id: i64,
    time_spent: i64,
    summary: Option<String>,
    spent_at: i64,
}

impl TimelogObj {
    fn load(c: &rusqlite::Connection, id: i64) -> FResult<TimelogObj> {
        Ok(c.query_row(
            "SELECT id, merge_request_id, user_id, time_spent, summary, spent_at FROM timelogs WHERE id = ?1",
            [id],
            |r| Ok(TimelogObj { id: r.get(0)?, mr_id: r.get(1)?, user_id: r.get(2)?, time_spent: r.get(3)?, summary: r.get(4)?, spent_at: r.get(5)? }),
        )?)
    }
}

impl Object for TimelogObj {
    fn typename(&self) -> String {
        "Timelog".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "id" => Out::val(gid("Timelog", self.id)),
            "timeSpent" => Out::val(self.time_spent),
            "summary" => Out::val(self.summary.clone()),
            "spentAt" => Out::val(gtime(self.spent_at)),
            "user" => Out::opt(User::find(cx.c, self.user_id)?.map(|u| UserObj::new("UserCore", u))),
            "issue" | "note" => Out::Null,
            "mergeRequest" => match MergeRequest::find(cx.c, self.mr_id)? {
                Some(m) => match Project::find(cx.c, m.target_project_id)? {
                    Some(p) => Out::obj(MrObj::new(p, m)),
                    None => Out::Null,
                },
                None => Out::Null,
            },
            _ => return Err(not_implemented("Timelog", field)),
        })
    }
}

/// `Tree`: the blobs and subtrees of one directory.
struct TreeObj {
    p: Project,
    rf: String,
    dir: String,
    entries: Vec<labrat_git::TreeEntry>,
}

impl Object for TreeObj {
    fn typename(&self) -> String {
        "Tree".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let path_of = |name: &str| if self.dir.is_empty() { name.to_string() } else { format!("{}/{name}", self.dir) };
        let mut entries = self.entries.clone();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(match field {
            "blobs" | "trees" | "submodules" => {
                let want_tree = field == "trees";
                let items: Vec<Shared> = entries
                    .iter()
                    .filter(|e| match field {
                        "trees" => e.mode == 0o40000,
                        "submodules" => e.mode == 0o160000,
                        _ => e.mode != 0o40000 && e.mode != 0o160000,
                    })
                    .map(|e| {
                        let path = path_of(&e.name);
                        let kind = if want_tree { "tree" } else { "blob" };
                        let ty = match field {
                            "trees" => "TreeEntry",
                            "submodules" => "Submodule",
                            _ => "Blob",
                        };
                        Shared(Rc::new(JsonObj::new(
                            ty,
                            json!({
                                "id": format!("gid://gitlab/{ty}/{}", e.oid.hex()),
                                "sha": e.oid.hex(), "name": e.name, "path": path, "flatPath": "", "type": kind,
                                "mode": format!("{:o}", e.mode), "lfsOid": null,
                                "webPath": format!("/{}/-/{kind}/{}/{path}", self.p.full_path, self.rf),
                                "webUrl": null,
                            }),
                        )))
                    })
                    .collect();
                let conn = match field {
                    "trees" => "TreeEntryConnection",
                    "submodules" => "SubmoduleConnection",
                    _ => "BlobConnection",
                };
                Out::obj(Conn::new(conn, positions(items), args)?)
            }
            "lastCommit" => {
                let r = repo::repo(cx.c, &self.p);
                match last_commit(&r, &self.rf, &self.dir)? {
                    Some((oid, cm)) => Out::obj(CommitObj { p: self.p.clone(), oid, c: cm }),
                    None => Out::Null,
                }
            }
            "permalinkPath" => Out::Null,
            _ => return Err(not_implemented("Tree", field)),
        })
    }
}

/// `RepositoryBlob`: one file at a ref.
struct BlobObj {
    p: Project,
    rf: String,
    path: String,
    mode: u32,
    oid: labrat_git::Oid,
    data: Vec<u8>,
}

impl Object for BlobObj {
    fn typename(&self) -> String {
        "RepositoryBlob".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path).to_string();
        let text = labrat_git::diff::is_binary(&self.data).then_some(()).is_none();
        Ok(match field {
            "id" => Out::val(format!("gid://gitlab/Blob/{}", self.oid.hex())),
            "oid" => Out::val(self.oid.hex()),
            "path" => Out::val(self.path.clone()),
            "name" => Out::val(name.clone()),
            "mode" => Out::val(format!("{:o}", self.mode)),
            // `BigInt` serializes as a string.
            "size" | "rawSize" => Out::val(self.data.len().to_string()),
            "rawBlob" => Out::val(String::from_utf8_lossy(&self.data).into_owned()),
            "rawTextBlob" => Out::val(if text { Some(String::from_utf8_lossy(&self.data).into_owned()) } else { None }),
            "base64EncodedBlob" => Out::val(base64::engine::general_purpose::STANDARD.encode(&self.data)),
            // `Gitlab::FileDetector`: only well-known files have a type.
            "fileType" => Out::val({
                let lower = name.to_lowercase();
                if lower.starts_with("readme") {
                    Some("readme")
                } else if lower.starts_with("license") || lower.starts_with("licence") || lower.starts_with("copying") {
                    Some("license")
                } else if self.path == ".gitlab-ci.yml" {
                    Some("gitlab_ci")
                } else if name == "CODEOWNERS" {
                    Some("codeowners")
                } else if lower.starts_with("changelog") || lower.starts_with("history") || lower.starts_with("changes") || lower.starts_with("news") {
                    Some("changelog")
                } else if lower.starts_with("contributing") {
                    Some("contributing")
                } else {
                    None
                }
            }),
            "webPath" => Out::val(format!("/{}/-/blob/{}/{}", self.p.full_path, self.rf, self.path)),
            "language" | "lfsOid" | "externalStorage" | "externalStorageUrl" | "simpleViewer" | "richViewer" => Out::Null,
            "rawPath" => Out::val(format!("/{}/-/raw/{}/{}", self.p.full_path, self.rf, self.path)),
            "storedExternally" | "archived" => Out::val(false),
            _ => return Err(not_implemented("RepositoryBlob", field)),
        })
    }
}

/// `ProjectMember` / `GroupMember`.
struct MemberObj {
    kind: &'static str,
    id: i64,
    user: User,
    level: i64,
    level_name: &'static str,
    expires_at: Option<String>,
    created_at: i64,
}

impl Object for MemberObj {
    fn typename(&self) -> String {
        self.kind.into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "id" => Out::val(gid(self.kind, self.id)),
            "user" => Out::obj(UserObj::new("UserCore", self.user.clone())),
            "accessLevel" => Out::obj(JsonObj::new("AccessLevel", json!({"integerValue": self.level, "stringValue": self.level_name}))),
            "expiresAt" => Out::val(self.expires_at.clone()),
            "createdAt" | "updatedAt" => Out::val(gtime(self.created_at)),
            _ => return Err(not_implemented(self.kind, field)),
        })
    }
}

/// `Branch`: a name and its commit.
#[derive(Clone)]
struct BranchObj {
    p: Project,
    name: String,
    oid: labrat_git::Oid,
}

impl Object for BranchObj {
    fn typename(&self) -> String {
        "Branch".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "name" => Out::val(self.name.clone()),
            "commit" => {
                let r = repo::repo(cx.c, &self.p);
                let c = r.commit(&self.oid)?;
                Out::obj(CommitObj { p: self.p.clone(), oid: self.oid, c })
            }
            _ => return Err(not_implemented("Branch", field)),
        })
    }
}

/// The message of an API error, as a GraphQL payload error.
fn api_error_text(e: &crate::ctx::ApiError) -> String {
    match &e.body["message"] {
        Value::String(s) => s.trim_start_matches("400 Bad request - ").to_string(),
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
        Value::Object(o) => o.iter().flat_map(|(k, v)| v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(move |m| format!("{} {m}", humanize(k)))).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

fn humanize(k: &str) -> String {
    let h = k.replace('_', " ");
    let mut ch = h.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    }
}

/// `BranchRule` for a protected branch pattern.
struct BranchRuleObj {
    pb: labrat_core::repo::ProtectedBranch,
    count: usize,
    default: Option<String>,
    p: Project,
}

/// The "All branches" branch rule.
struct AllBranchesObj {
    p: Project,
    count: usize,
}

impl Object for AllBranchesObj {
    fn typename(&self) -> String {
        "BranchRule".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        Ok(match field {
            "id" => Out::val(gid("Projects::AllBranchesRule", self.p.id)),
            "name" => Out::val("All branches"),
            "isDefault" | "isProtected" => Out::val(false),
            "matchingBranchesCount" => Out::val(self.count),
            "branchProtection" => Out::Null,
            "approvalRules" if cx.req.app.config.is_ee() => super::approvals::branch_rule_approval_rules(cx, &self.p, None, args)?,
            _ => return Err(not_implemented("BranchRule", field)),
        })
    }
}

impl Object for BranchRuleObj {
    fn typename(&self) -> String {
        "BranchRule".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        Ok(match field {
            "approvalRules" if cx.req.app.config.is_ee() => super::approvals::branch_rule_approval_rules(cx, &self.p, Some(self.pb.id), args)?,
            "branchProtection" if cx.req.app.config.is_ee() => Out::obj(JsonObj::new(
                "BranchProtection",
                json!({"allowForcePush": self.pb.allow_force_push, "codeOwnerApprovalRequired": self.pb.code_owner_approval_required}),
            )),
            "id" => Out::val(gid("Projects::BranchRule", self.pb.id)),
            "name" => Out::val(self.pb.name.clone()),
            "isDefault" => Out::val(self.default.as_deref() == Some(self.pb.name.as_str())),
            "isProtected" => Out::val(true),
            "matchingBranchesCount" => Out::val(self.count),
            "createdAt" => Out::val(gtime(self.pb.created_at)),
            "updatedAt" => Out::val(gtime(self.pb.updated_at)),
            "branchProtection" => Out::obj(JsonObj::new("BranchProtection", json!({"allowForcePush": self.pb.allow_force_push}))),
            "squashOption" => Out::Null,
            _ => return Err(not_implemented("BranchRule", field)),
        })
    }
}

/// `Mutations::Commits::Create`.
fn commit_create(cx: &Ctx, input: &Args) -> FResult<Payload> {
    use labrat_core::repo::{FileAction, NewCommit};
    use base64::Engine;
    let user = me(cx)?.clone();
    let path = input.str("projectPath").unwrap_or_default();
    let Some(p) = Project::find_by_full_path(cx.c, &path)? else { return Err(unavailable()) };
    let Some((p, perms)) = readable_project(cx, p)? else { return Err(unavailable()) };
    if !perms.developer() {
        return Err(unavailable());
    }
    let actions_in: Vec<Value> = input.get("actions").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut content = vec![];
    let mut actions = vec![];
    for a in &actions_in {
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
        let raw = s("content");
        content.push(raw.clone());
        let bytes = |v: Option<String>| -> Vec<u8> {
            let v = v.unwrap_or_default();
            if s("encoding").as_deref() == Some("BASE64") {
                base64::engine::general_purpose::STANDARD.decode(v.as_bytes()).unwrap_or_default()
            } else {
                v.into_bytes()
            }
        };
        let path = s("filePath").unwrap_or_default();
        let exec = a.get("executeFilemode").and_then(Value::as_bool);
        actions.push(match s("action").as_deref() {
            Some("CREATE") => FileAction::Create { path, content: bytes(raw), exec },
            Some("UPDATE") => FileAction::Update { path, content: bytes(raw), exec },
            Some("DELETE") => FileAction::Delete { path },
            Some("MOVE") => FileAction::Move { from: s("previousPath").unwrap_or_default(), to: path, content: raw.map(|r| bytes(Some(r))) },
            _ => FileAction::Chmod { path, exec: exec.unwrap_or(false) },
        });
    }
    let branch = input.str("branch").unwrap_or_default();
    let message = input.str("message").unwrap_or_default();
    let start_branch = input.str("startBranch");
    let r = repo::repo(cx.c, &p);
    let old = r.branch(&branch)?.unwrap_or(labrat_git::Oid::ZERO);
    let mut pl = Payload { ty: "CommitCreatePayload".into(), client_id: input.str("clientMutationId"), content: Some(content), ..Default::default() };
    let made = labrat_core::repo::create_commit(
        cx.c,
        &p,
        &user,
        NewCommit { branch: &branch, start_branch: start_branch.as_deref(), start_sha: None, message: &message, actions: &actions, author_name: None, author_email: None, force: false },
    );
    match made {
        Ok(oid) => {
            let update = labrat_git::protocol::RefUpdate { old, new: oid, name: format!("refs/heads/{branch}") };
            labrat_core::services::push::post_receive(cx.c, &cx.req.app.config, &p, &user, &[update])?;
            // GitLab builds this with a relative route helper: `/api/graphql:pipelines/sha/<sha>`.
            pl.pipeline_path = Some(format!("/api/graphql:pipelines/sha/{}", oid.hex()));
            pl.commit = Some((p, oid));
        }
        Err(e) => {
            pl.pipeline_path = Some("/api/graphql:pipelines/sha/".into());
            let msg = match e.body()["message"].clone() {
                Value::String(m) => m,
                other => other.to_string(),
            };
            pl.errors = vec![msg];
        }
    }
    Ok(pl)
}

/// The newest commit on `rf`'s first-parent history that changed `path` (the root for "").
fn last_commit(r: &labrat_git::Repo, rf: &str, path: &str) -> FResult<Option<(labrat_git::Oid, labrat_git::Commit)>> {
    let path = path.trim_matches('/');
    let Some(mut cur) = r.resolve(rf)? else { return Ok(None) };
    loop {
        let cm = r.commit(&cur)?;
        let here = if path.is_empty() { Some(cm.tree) } else { r.entry_at(&cm.tree, path)?.map(|e| e.oid) };
        let parent = cm.parents.first().copied();
        let before = match parent {
            Some(pp) => {
                let pt = r.commit(&pp)?.tree;
                if path.is_empty() { Some(pt) } else { r.entry_at(&pt, path)?.map(|e| e.oid) }
            }
            None => None,
        };
        if here != before || parent.is_none() {
            return Ok(Some((cur, cm)));
        }
        cur = parent.unwrap();
    }
}

/// `TestReportSummary` of a pipeline without test reports.
struct TestSummaryObj;

impl Object for TestSummaryObj {
    fn typename(&self) -> String {
        "TestReportSummary".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        Ok(match field {
            "total" => Out::obj(JsonObj::new("TestReportTotal", json!({"count": 0, "error": 0, "failed": 0, "skipped": 0, "success": 0, "time": 0.0, "suiteError": null}))),
            "testSuites" => Out::obj(Conn::new("TestSuiteSummaryConnection", positions(Vec::<Shared>::new()), args)?),
            _ => return Err(not_implemented("TestReportSummary", field)),
        })
    }
}

/// Linguist programming languages by file extension (a common subset; prose, data and markup such
/// as Markdown, JSON or YAML are not counted).
fn language_of(path: &str) -> Option<(&'static str, &'static str)> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_lowercase())?;
    Some(match ext.as_str() {
        "rs" => ("Rust", "#dea584"),
        "py" => ("Python", "#3572A5"),
        "rb" => ("Ruby", "#701516"),
        "go" => ("Go", "#00ADD8"),
        "js" | "mjs" | "cjs" => ("JavaScript", "#f1e05a"),
        "ts" => ("TypeScript", "#3178c6"),
        "tsx" => ("TSX", "#3178c6"),
        "java" => ("Java", "#b07219"),
        "kt" => ("Kotlin", "#A97BFF"),
        "c" | "h" => ("C", "#555555"),
        "cc" | "cpp" | "hpp" | "cxx" => ("C++", "#f34b7d"),
        "cs" => ("C#", "#178600"),
        "php" => ("PHP", "#4F5D95"),
        "sh" | "bash" => ("Shell", "#89e051"),
        "swift" => ("Swift", "#F05138"),
        "scala" => ("Scala", "#c22d40"),
        "html" | "htm" => ("HTML", "#e34c26"),
        "css" => ("CSS", "#663399"),
        "vue" => ("Vue", "#41b883"),
        _ => return None,
    })
}
