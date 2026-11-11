//! The search API (`/search`, `/groups/:id/search`, `/projects/:id/search`) as basic search
//! answers it, without Elasticsearch: merge requests, milestones, notes, commits, users and
//! projects. Scopes outside `SearchHelpers`' lists fail validation.

use crate::ctx::{ApiError, ApiResult, Req};
use crate::entities::merge_request::{self as mr_ent, Opts};
use crate::entities::note as note_ent;
use crate::entities::project::{self as project_entity, ProjectCtx};
use crate::entities::repository as repo_ent;
use crate::entities::user::{self as user_ent, Ctx};
use crate::find;
use crate::pagination::{paginate, paginate_without_count};
use crate::params::Params;
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Namespace, Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::notes::Note;
use labrat_core::{App, Error, repo};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/search", get(global))
        .route("/groups/{id}/search", get(group))
        .route("/projects/{id}/search", get(project))
        // `:id/(-/)search`
        .route("/groups/{id}/-/search", get(group))
        .route("/projects/{id}/-/search", get(project))
}

const GLOBAL_SCOPES: &[&str] = &["projects", "issues", "work_items", "merge_requests", "milestones", "snippet_titles", "users"];
const GROUP_SCOPES: &[&str] = &["projects", "issues", "work_items", "merge_requests", "milestones", "users"];
const PROJECT_SCOPES: &[&str] = &["issues", "work_items", "merge_requests", "milestones", "notes", "wiki_blobs", "commits", "blobs", "users"];

/// Grape's validation of `search` and `scope`, in declaration order.
fn validate(req: &Req, scopes: &[&str]) -> Result<(String, String), ApiError> {
    let p = &req.params;
    let mut errors = vec![];
    let search = p.str("search");
    if search.is_none() {
        errors.push("search is missing".to_string());
    }
    let scope = p.str("scope");
    match &scope {
        None => errors.push("scope is missing, scope does not have a valid value".into()),
        Some(s) if !scopes.contains(&s.as_str()) => errors.push("scope does not have a valid value".into()),
        _ => {}
    }
    if let Some(st) = p.str("state")
        && !["all", "opened", "closed", "merged"].contains(&st.as_str())
    {
        errors.push("state does not have a valid value".into());
    }
    if !errors.is_empty() {
        return Err(Error::Param(errors.join(", ")).into());
    }
    Ok((search.unwrap_or_default(), scope.unwrap_or_default()))
}

/// `Gitlab::Search::AbuseDetection`: a query of fewer than 2 characters, a stop word, or a term
/// over 100 characters (200 for URLs) gets no results.
fn too_short(term: &str) -> bool {
    if term.trim().is_empty() {
        // Blank queries aren't abusive; they match everything.
        return false;
    }
    const STOP_WORDS: &[&str] = &[
        "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it", "no", "not", "of", "on", "or", "such",
        "that", "the", "their", "then", "there", "these", "they", "this", "to", "was", "will", "with",
    ];
    let q = term.trim();
    q.chars().count() < 2
        || STOP_WORDS.contains(&q)
        || q.split_whitespace().any(|t| t.chars().count() > if t.contains("://") { 200 } else { 100 })
}

/// `fuzzy_arel_match` with the minimum character limit (notes, milestones).
fn contains(hay: &str, term: &str) -> bool {
    let hay = hay.to_lowercase();
    let q = term.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        return true;
    }
    let long: Vec<&str> = q.split(' ').filter(|t| t.chars().count() >= 3).collect();
    if long.is_empty() { hay == q } else { long.iter().all(|t| hay.contains(t)) }
}

/// Substring matching without the minimum (projects, users).
fn contains_any_length(hay: &str, term: &str) -> bool {
    let hay = hay.to_lowercase();
    term.to_lowercase().split_whitespace().all(|t| hay.contains(t))
}

fn empty(req: &Req) -> ApiResult {
    paginate::<Value>(req, vec![], &[], |v| Ok(v.clone()))
}

fn readable_projects(c: &Connection, req: &Req, namespaces: Option<&[i64]>) -> Result<Vec<(Project, ProjectPerms)>, ApiError> {
    let projects = match namespaces {
        Some(ns) => {
            let list = ns.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            Project::list_where(c, &format!("p.namespace_id IN ({list})"), [])?
        }
        None => Project::list_where(c, "1 = 1", [])?,
    };
    let mut out = vec![];
    for p in projects {
        let perms = ProjectPerms::load(c, req.user.as_ref(), &p)?;
        if perms.read_project() {
            out.push((p, perms));
        }
    }
    Ok(out)
}

fn merge_requests(c: &Connection, req: &Req, projects: &[(Project, ProjectPerms)], term: &str, defaults: &[(&str, &str)]) -> ApiResult {
    if too_short(term) {
        return empty(req);
    }
    let mut rows = vec![];
    for (p, perms) in projects {
        if !perms.read_merge_request(p) {
            continue;
        }
        for mr in MergeRequest::list_where(c, "target_project_id = ?1", [p.id])? {
            rows.push((p.clone(), mr));
        }
    }
    // The MR finder with `search`, `state`, `order_by` and `sort`. A query ending in `#N` is an
    // iid lookup (`issuable_params`); a blank one matches everything.
    let mut m = serde_json::Map::new();
    let iid = term.rsplit_once('#').and_then(|(_, n)| (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then(|| n.to_string()));
    match iid {
        Some(n) => {
            m.insert("iids".into(), json!([n]));
        }
        None if term.trim().is_empty() => {}
        None => {
            m.insert("search".into(), json!(term));
            m.insert("__min_chars".into(), json!(true));
        }
    }
    if let Some(v) = req.params.str("state") {
        m.insert("state".into(), json!(v));
    }
    // `Gitlab::Search::SortOptions`: only created_at/updated_at with asc/desc; anything else
    // falls back to the default (created_at desc).
    let order_by = req.params.str("order_by");
    let sort = req.params.str("sort");
    if let (Some(o), Some(d)) = (&order_by, &sort)
        && matches!(o.as_str(), "created_at" | "updated_at")
        && matches!(d.as_str(), "asc" | "desc")
    {
        m.insert("order_by".into(), json!(o));
        m.insert("sort".into(), json!(d));
    }
    m.insert("scope".into(), json!("all"));
    let sub = req.with_params(Params { map: m });
    let rows = crate::routes::merge_requests::filter(c, &sub, rows, None)?;
    paginate_without_count(req, rows, defaults, |(p, mr)| {
        Ok(Value::Object(mr_ent::basic(c, &req.app.config, req.user.as_ref(), p, mr, &Opts::default())?))
    })
}

fn milestones(c: &Connection, req: &Req, projects: &[(Project, ProjectPerms)], extra_groups: &[i64], term: &str, defaults: &[(&str, &str)]) -> ApiResult {
    if too_short(term) {
        return empty(req);
    }
    // Project milestones plus those of the projects' groups (and the searched groups).
    let mut groups: Vec<i64> = extra_groups.to_vec();
    for (p, _) in projects {
        for g in ancestors(c, p.namespace_id)? {
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
    }
    let mut rows: Vec<(i64, i64)> = vec![];
    let mut push = |c: &Connection, sql: &str, id: i64| -> Result<(), ApiError> {
        let mut st = c.prepare(sql)?;
        let found: Vec<(i64, String, Option<String>, i64)> = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        for (mid, title, desc, updated) in found {
            if (contains(&title, term) || desc.as_deref().is_some_and(|d| contains(d, term))) && !rows.iter().any(|(m, _)| *m == mid) {
                rows.push((mid, updated));
            }
        }
        Ok(())
    };
    for (p, _) in projects {
        push(c, "SELECT id, title, description, updated_at FROM milestones WHERE project_id = ?1", p.id)?;
    }
    for g in &groups {
        push(c, "SELECT id, title, description, updated_at FROM milestones WHERE group_id = ?1", *g)?;
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    let ids: Vec<i64> = rows.into_iter().map(|(m, _)| m).collect();
    paginate_without_count(req, ids, defaults, |id| Ok(mr_ent::milestone(c, &req.app.config, *id)?))
}

fn users(c: &Connection, req: &Req, ids: Option<Vec<i64>>, term: &str, defaults: &[(&str, &str)], counted: bool) -> ApiResult {
    let all: Vec<User> = match ids {
        Some(ids) => ids.into_iter().filter_map(|id| User::find(c, id).ok().flatten()).collect(),
        None => {
            let mut st = c.prepare("SELECT id FROM users ORDER BY id")?;
            let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            ids.into_iter().filter_map(|id| User::find(c, id).ok().flatten()).collect()
        }
    };
    // `User.search`: a leading `@` is ignored; exact matches first, then by name.
    let q = term.trim().trim_start_matches('@');
    let lq = q.to_lowercase();
    let mut found: Vec<User> = all
        .into_iter()
        .filter(|u| u.state == "active" && (contains_any_length(&u.username, q) || contains_any_length(&u.name, q)))
        .collect();
    found.sort_by(|a, b| {
        let rank = |u: &User| if u.username.to_lowercase() == lq { 0 } else if u.name.to_lowercase() == lq { 1 } else { 2 };
        rank(a).cmp(&rank(b)).then(a.name.cmp(&b.name)).then(a.id.cmp(&b.id))
    });
    let ctx = Ctx { base_url: &req.app.config.external_url, current_user: req.user.as_ref() };
    if counted {
        paginate(req, found, defaults, |u| Ok(user_ent::basic_value(u, &ctx)))
    } else {
        paginate_without_count(req, found, defaults, |u| Ok(user_ent::basic_value(u, &ctx)))
    }
}

fn projects_scope(c: &Connection, req: &Req, projects: Vec<(Project, ProjectPerms)>, term: &str, defaults: &[(&str, &str)]) -> ApiResult {
    if too_short(term) {
        return empty(req);
    }
    let mut found: Vec<(Project, ProjectPerms)> = projects
        .into_iter()
        .filter(|(p, _)| {
            contains_any_length(&p.name, term)
                || contains_any_length(&p.path, term)
                || contains_any_length(&p.full_path, term)
                || contains_any_length(&p.full_name, term)
                || p.description.as_deref().is_some_and(|d| contains_any_length(d, term))
        })
        .collect();
    found.sort_by(|a, b| b.0.id.cmp(&a.0.id));
    let ctx = ProjectCtx { config: &req.app.config, user: req.user.as_ref(), single: false };
    paginate_without_count(req, found, defaults, |(p, perms)| {
        let ns = Namespace::find(c, p.namespace_id)?.ok_or(Error::not_found("Namespace"))?;
        Ok(Value::Object(project_entity::basic_details(c, p, &ns, perms, &ctx)?))
    })
}

async fn global(req: Req) -> ApiResult {
    // `before { authenticate! }` runs ahead of parameter validation.
    req.user()?;
    let (term, scope) = validate(&req, GLOBAL_SCOPES)?;
    let defaults = [("include_archived", "false")];
    let app = req.app.clone();
    let include_archived = req.params.bool_or("include_archived", false)?;
    app.db.read(|c| {
        if too_short(&term) {
            return empty(&req);
        }
        let mut projects = readable_projects(c, &req, None)?;
        if !include_archived {
            projects.retain(|(p, _)| !p.archived);
        }
        match scope.as_str() {
            "merge_requests" => merge_requests(c, &req, &projects, &term, &defaults),
            "milestones" => milestones(c, &req, &projects, &[], &term, &defaults),
            "users" => users(c, &req, None, &term, &defaults, false),
            "projects" => projects_scope(c, &req, projects, &term, &defaults),
            _ => empty(&req),
        }
    })
}

async fn group(req: Req) -> ApiResult {
    req.user()?;
    let (term, scope) = validate(&req, GROUP_SCOPES)?;
    let defaults = [("include_archived", "false")];
    let app = req.app.clone();
    app.db.read(|c| {
        let (g, _) = find::group(c, &req, req.path_param("id"))?;
        if too_short(&term) {
            return empty(&req);
        }
        let ns = Namespace::self_and_descendant_ids(c, g.id)?;
        let mut projects = readable_projects(c, &req, Some(&ns))?;
        if !req.params.bool_or("include_archived", false)? {
            projects.retain(|(p, _)| !p.archived);
        }
        match scope.as_str() {
            "merge_requests" => merge_requests(c, &req, &projects, &term, &defaults),
            "milestones" => milestones(c, &req, &projects, &ns, &term, &defaults),
            "users" => {
                let mut st = c.prepare("SELECT user_id FROM members WHERE source_type = 'Namespace' AND source_id = ?1 ORDER BY user_id")?;
                let ids: Vec<i64> = st.query_map([g.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                users(c, &req, Some(ids), &term, &defaults, false)
            }
            "projects" => projects_scope(c, &req, projects, &term, &defaults),
            _ => empty(&req),
        }
    })
}

async fn project(req: Req) -> ApiResult {
    req.user()?;
    let (term, scope) = validate(&req, PROJECT_SCOPES)?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if too_short(&term) {
            return empty(&req);
        }
        let projects = vec![(p.clone(), perms)];
        match scope.as_str() {
            "merge_requests" if perms.read_merge_request(&p) => merge_requests(c, &req, &projects, &term, &[]),
            "milestones" => milestones(c, &req, &projects, &[], &term, &[]),
            "notes" => {
                // `notes_finder`: user notes ordered by `updated_at DESC`; internal notes need Reporter.
                let internal_ok = perms.admin || perms.level >= labrat_core::model::access::REPORTER;
                let mut st = c.prepare(
                    "SELECT id FROM notes WHERE project_id = ?1 AND system = 0 AND (internal = 0 OR ?2) AND noteable_type IN ('MergeRequest', 'Commit') ORDER BY updated_at DESC, id DESC",
                )?;
                let ids: Vec<i64> = st.query_map(rusqlite::params![p.id, internal_ok], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                let mut found = vec![];
                for id in ids {
                    let Some(n) = Note::find(c, id)? else { continue };
                    if contains(&n.note, &term) && (n.noteable_type == "Commit" && perms.read_code(&p) || n.noteable_type == "MergeRequest" && perms.read_merge_request(&p)) {
                        found.push(n);
                    }
                }
                let ctx = Ctx { base_url: &req.app.config.external_url, current_user: req.user.as_ref() };
                paginate(&req, found, &[], |n| {
                    Ok(if n.noteable_type == "Commit" {
                        note_ent::commit_note(c, &ctx, n)?
                    } else {
                        let mr = MergeRequest::get(c, n.noteable_id)?;
                        note_ent::note(c, &ctx, &p, &mr, n)?
                    })
                })
            }
            "commits" if perms.read_code(&p) => {
                let r = repo::repo(c, &p);
                let rf = req.params.str("ref").unwrap_or(p.default_branch.clone().unwrap_or_default());
                let mut found = vec![];
                if let Some(head) = r.resolve(&rf)? {
                    for oid in labrat_git::walk::rev_list(&r, &[head], &[])? {
                        let cm = r.commit(&oid)?;
                        if contains(&cm.message, &term) {
                            found.push((oid, cm));
                        }
                    }
                }
                paginate(&req, found, &[], |(oid, cm)| Ok(repo_ent::commit_detail(&req.app.config, &r, &p, oid, cm, false)?))
            }
            "users" => {
                let mut ids = vec![];
                let mut st = c.prepare("SELECT user_id FROM members WHERE source_type = 'Project' AND source_id = ?1")?;
                ids.extend(st.query_map([p.id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
                let ns = Namespace::find(c, p.namespace_id)?.ok_or(Error::not_found("Namespace"))?;
                if ns.kind == "Group" {
                    for gid in ancestors(c, ns.id)? {
                        let mut st = c.prepare("SELECT user_id FROM members WHERE source_type = 'Namespace' AND source_id = ?1")?;
                        ids.extend(st.query_map([gid], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
                    }
                }
                ids.sort();
                ids.dedup();
                users(c, &req, Some(ids), &term, &[], true)
            }
            _ => empty(&req),
        }
    })
}

fn ancestors(c: &Connection, id: i64) -> Result<Vec<i64>, ApiError> {
    let mut out = vec![];
    let mut cur = Some(id);
    while let Some(i) = cur {
        out.push(i);
        cur = Namespace::find(c, i)?.and_then(|n| n.parent_id);
    }
    Ok(out)
}
