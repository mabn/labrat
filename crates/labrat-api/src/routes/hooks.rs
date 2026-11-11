//! `lib/api/{project_hooks,system_hooks}.rb`, hook test/events/resend, and the small part of
//! `/application/settings` labrat models.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::{get, post, put};
use labrat_core::model::Project;
use labrat_core::services::hooks::{self as core_hooks, Hook, PROJECT_EVENTS};
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::notes::Note;
use labrat_core::{App, Error, repo, time};
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/hooks", get(list).post(create))
        .route("/projects/{id}/hooks/{hook_id}", get(show).put(update).delete(destroy))
        .route("/projects/{id}/hooks/{hook_id}/test/{trigger}", post(test))
        .route("/projects/{id}/hooks/{hook_id}/events", get(events))
        .route("/projects/{id}/hooks/{hook_id}/events/{hook_log_id}/resend", post(resend))
        .route("/projects/{id}/hooks/{hook_id}/url_variables/{key}", put(set_variable).delete(unset_variable))
        .route("/projects/{id}/hooks/{hook_id}/custom_headers/{key}", put(set_header).delete(unset_header))
        .route("/hooks", get(list_system).post(create_system))
        .route("/hooks/{hook_id}", get(show_system).put(update_system).delete(destroy_system).post(test_system))
        .route("/application/settings", get(get_settings).put(put_settings))
}

const SYSTEM_EVENTS: &[&str] = &["push_events", "tag_push_events", "merge_requests_events", "repository_update_events"];

pub fn setting(c: &Connection, key: &str) -> labrat_core::Result<Value> {
    let s: String = c.query_row("SELECT settings FROM application_settings WHERE id = 1", [], |r| r.get(0))?;
    let m: Map<String, Value> = serde_json::from_str(&s).unwrap_or_default();
    Ok(m.get(key).cloned().unwrap_or(Value::Null))
}

fn hook_value(h: &Hook, full: bool) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(h.id));
    m.insert("url".into(), json!(h.url));
    m.insert("name".into(), json!(h.name));
    m.insert("description".into(), json!(h.description));
    m.insert("created_at".into(), json!(time::iso(h.created_at)));
    for k in ["push_events", "tag_push_events", "merge_requests_events", "repository_update_events"] {
        m.insert(k.into(), h.event(k));
    }
    m.insert("enable_ssl_verification".into(), json!(h.enable_ssl_verification));
    if h.kind == "SystemHook" {
        m.insert("organization_id".into(), json!(1));
    }
    m.insert("alert_status".into(), json!("executable"));
    m.insert("disabled_until".into(), Value::Null);
    if full {
        m.insert("url_variables".into(), json!(h.url_variables.iter().map(|(k, _)| json!({"key": k})).collect::<Vec<_>>()));
    }
    m.insert("push_events_branch_filter".into(), json!(h.push_events_branch_filter));
    m.insert("branch_filter_strategy".into(), json!(h.branch_filter_strategy));
    m.insert("custom_webhook_template".into(), json!(h.custom_webhook_template));
    if full {
        m.insert("custom_headers".into(), json!(h.custom_headers.iter().map(|(k, _)| json!({"key": k})).collect::<Vec<_>>()));
    }
    if h.kind == "ProjectHook" {
        m.insert("project_id".into(), json!(h.project_id));
        for k in PROJECT_EVENTS.iter().skip(4) {
            m.insert(k.to_string(), h.event(k));
        }
    }
    Value::Object(m)
}

fn admin_project(c: &Connection, req: &Req) -> Result<Project, ApiError> {
    req.user()?;
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !perms.admin_project() {
        return Err(Error::forbidden().into());
    }
    Ok(p)
}

fn find_hook(c: &Connection, req: &Req, p: Option<&Project>) -> Result<Hook, ApiError> {
    find_hook_id(c, req.path_param("hook_id"), p)
}

fn find_hook_id(c: &Connection, id: &str, p: Option<&Project>) -> Result<Hook, ApiError> {
    let id: i64 = id.parse().map_err(|_| Error::Param("hook_id is invalid".into()))?;
    Hook::find(c, id)?
        .filter(|h| match p {
            Some(p) => h.kind == "ProjectHook" && h.project_id == Some(p.id),
            None => h.kind == "SystemHook",
        })
        .ok_or_else(|| Error::Api(404, json!("404 Not found")).into())
}

/// `PublicUrlValidator`: http(s) only; local addresses unless the instance allows them.
fn url_ok(c: &Connection, url: &str, system: bool) -> labrat_core::Result<bool> {
    let Ok(u) = url::Url::parse(url) else { return Ok(false) };
    if !matches!(u.scheme(), "http" | "https") || url.len() > 8192 {
        return Ok(false);
    }
    let allow = if system {
        setting(c, "allow_local_requests_from_system_hooks")?.as_bool().unwrap_or(true)
    } else {
        setting(c, "allow_local_requests_from_web_hooks_and_services")?.as_bool().unwrap_or(false)
    };
    if allow {
        return Ok(true);
    }
    let host = u.host_str().unwrap_or("").trim_matches(['[', ']']).to_lowercase();
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified(),
            Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unspecified(),
            Err(_) => false,
        };
    Ok(!local)
}

struct HookParams {
    fields: Map<String, Value>,
}

fn hook_params(req: &Req, system: bool) -> Result<HookParams, ApiError> {
    let p = &req.params;
    let mut fields = Map::new();
    for k in ["url", "name", "description", "token", "push_events_branch_filter", "custom_webhook_template"] {
        if p.has(k) {
            fields.insert(k.into(), json!(p.str(k)));
        }
    }
    if let Some(s) = p.one_of("branch_filter_strategy", &["wildcard", "regex", "all_branches"])? {
        fields.insert("branch_filter_strategy".into(), json!(s));
    }
    if let Some(b) = p.bool("enable_ssl_verification")? {
        fields.insert("enable_ssl_verification".into(), json!(b));
    }
    let events: &[&str] = if system { SYSTEM_EVENTS } else { PROJECT_EVENTS };
    let mut ev = Map::new();
    for k in events {
        if let Some(b) = p.bool(k)? {
            ev.insert(k.to_string(), json!(b));
        }
    }
    if !ev.is_empty() {
        fields.insert("events".into(), Value::Object(ev));
    }
    for (param, key) in [("url_variables", "variables"), ("custom_headers", "headers")] {
        if let Some(Value::Array(items)) = p.raw(param) {
            fields.insert(key.into(), Value::Array(items.clone()));
        }
    }
    Ok(HookParams { fields })
}

fn save(c: &Connection, existing: Option<&Hook>, kind: &str, project_id: Option<i64>, hp: HookParams) -> Result<Hook, ApiError> {
    let f = &hp.fields;
    let url = f.get("url").and_then(Value::as_str).map(str::to_string).or(existing.map(|h| h.url.clone())).unwrap_or_default();
    let mut vars: Vec<(String, String)> = existing.map(|h| h.url_variables.clone()).unwrap_or_default();
    let mut heads: Vec<(String, String)> = existing.map(|h| h.custom_headers.clone()).unwrap_or_default();
    let merge = |list: &mut Vec<(String, String)>, items: &Value| {
        for it in items.as_array().into_iter().flatten() {
            if let (Some(k), Some(v)) = (it["key"].as_str(), it["value"].as_str()) {
                list.retain(|(lk, _)| lk != k);
                list.push((k.to_string(), v.to_string()));
            }
        }
    };
    if let Some(v) = f.get("variables") {
        if existing.is_none() {
            vars.clear();
        }
        merge(&mut vars, v);
    }
    if let Some(v) = f.get("headers") {
        if existing.is_none() {
            heads.clear();
        }
        merge(&mut heads, v);
    }
    let mut interpolated = url.clone();
    for (k, v) in &vars {
        interpolated = interpolated.replace(&format!("{{{k}}}"), v);
    }
    if url.is_empty() || interpolated.contains('{') || !url_ok(c, &interpolated, kind == "SystemHook")? {
        return Err(Error::Body(422, json!({"error": "Invalid url given"})).into());
    }
    if let Some(b) = f.get("push_events_branch_filter").and_then(Value::as_str) {
        let strategy = f.get("branch_filter_strategy").and_then(Value::as_str).unwrap_or("wildcard");
        if strategy == "wildcard" && !b.is_empty() && !repo::valid_branch_name(&b.replace('*', "x")) {
            return Err(Error::message(422, "Invalid branch filter given").into());
        }
        if strategy == "regex" && regex::Regex::new(b).is_err() {
            return Err(Error::Body(422, json!({"error": "Invalid branch filter given"})).into());
        }
    }
    // Changing the URL without sending the token again drops the token.
    let token = match (f.get("token"), existing) {
        (Some(t), _) => t.as_str().map(str::to_string),
        (None, Some(h)) if f.contains_key("url") && h.url != url => None,
        (None, Some(h)) => h.token.clone(),
        (None, None) => None,
    };
    let mut events = existing.map(|h| h.events.clone()).unwrap_or_default();
    if let Some(Value::Object(ev)) = f.get("events") {
        for (k, v) in ev {
            events.insert(k.clone(), v.clone());
        }
    }
    let strategy = f
        .get("branch_filter_strategy")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(existing.map(|h| h.branch_filter_strategy.clone()))
        .unwrap_or("wildcard".into());
    let filter = if strategy == "all_branches" {
        None
    } else {
        match f.get("push_events_branch_filter") {
            Some(v) => v.as_str().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")),
            None => existing.and_then(|h| h.push_events_branch_filter.clone()),
        }
    };
    let pick = |k: &str, old: Option<String>| -> Option<String> {
        match f.get(k) {
            Some(v) => v.as_str().map(str::to_string),
            None => old,
        }
    };
    let name = pick("name", existing.and_then(|h| h.name.clone()));
    let description = pick("description", existing.and_then(|h| h.description.clone()));
    let template = pick("custom_webhook_template", existing.and_then(|h| h.custom_webhook_template.clone()));
    if let Some(t) = template.as_deref().filter(|t| !t.trim().is_empty()) {
        if let Some(msg) = template_problem(t) {
            return Err(ApiError { status: 422, body: json!({"message": {"custom_webhook_template": [msg]}}), headers: vec![] });
        }
    }
    let ssl = f.get("enable_ssl_verification").and_then(Value::as_bool).unwrap_or(existing.map(|h| h.enable_ssl_verification).unwrap_or(true));
    let pairs = |l: &[(String, String)]| -> Value { Value::Array(l.iter().map(|(k, v)| json!({"key": k, "value": v})).collect()) };
    let extra = json!({"headers": pairs(&heads), "variables": pairs(&vars)}).to_string();
    let now = time::now();
    let id = match existing {
        Some(h) => {
            c.execute(
                "UPDATE web_hooks SET url = ?1, name = ?2, description = ?3, token = ?4, events = ?5, push_events_branch_filter = ?6,
                    branch_filter_strategy = ?7, enable_ssl_verification = ?8, custom_headers = ?9, custom_webhook_template = ?10, updated_at = ?11
                 WHERE id = ?12",
                params![url, name, description, token, Value::Object(events).to_string(), filter, strategy, ssl, extra, template, now, h.id],
            )?;
            h.id
        }
        None => {
            c.execute(
                "INSERT INTO web_hooks (project_id, type, url, name, description, token, events, push_events_branch_filter, branch_filter_strategy,
                    enable_ssl_verification, custom_headers, custom_webhook_template, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
                params![project_id, kind, url, name, description, token, Value::Object(events).to_string(), filter, strategy, ssl, extra, template, now],
            )?;
            c.last_insert_rowid()
        }
    };
    Ok(Hook::find(c, id)?.expect("saved"))
}

async fn list(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let p = admin_project(c, &req)?;
        let hooks = Hook::list(c, "ProjectHook", Some(p.id))?;
        paginate(&req, hooks, &[], |h| Ok(hook_value(h, false)))
    })
}

async fn show(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let p = admin_project(c, &req)?;
        Ok::<_, ApiError>(hook_value(&find_hook(c, &req, Some(&p))?, true))
    })?;
    Ok(Resp::ok(body))
}

async fn create(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["url"])?;
    let hp = hook_params(&req, false)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let p = admin_project(c, &req)?;
        let count: i64 = c.query_row("SELECT count(*) FROM web_hooks WHERE project_id = ?1", [p.id], |r| r.get(0))?;
        if count >= 100 {
            return Err(Error::message(422, "Maximum number of project hooks (100) exceeded").into());
        }
        Ok::<_, ApiError>(hook_value(&save(c, None, "ProjectHook", Some(p.id), hp)?, true))
    })?;
    Ok(Resp::created(body))
}

async fn update(req: Req) -> ApiResult {
    req.user()?;
    let hp = hook_params(&req, false)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let p = admin_project(c, &req)?;
        let h = find_hook(c, &req, Some(&p))?;
        if hp.fields.is_empty() {
            return Err(Error::message(400, "No parameters provided").into());
        }
        Ok::<_, ApiError>(hook_value(&save(c, Some(&h), "ProjectHook", Some(p.id), hp)?, true))
    })?;
    Ok(Resp::ok(body))
}

async fn destroy(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let p = admin_project(c, &req)?;
        let h = find_hook(c, &req, Some(&p))?;
        c.execute("DELETE FROM web_hooks WHERE id = ?1", [h.id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn set_kv(req: Req, column: &'static str, set: bool) -> ApiResult {
    let key = req.path_param("key").to_string();
    let value = if set {
        req.params.require(&["value"])?;
        req.params.str("value")
    } else {
        None
    };
    let app = req.app.clone();
    app.db.tx(|c| {
        let p = admin_project(c, &req)?;
        let h = find_hook(c, &req, Some(&p))?;
        let mut vars = h.url_variables.clone();
        let mut heads = h.custom_headers.clone();
        // Changing a variable the URL uses changes the interpolated URL, which resets all URL
        // variables (`reset_url_variables`), so the URL then misses its keys.
        if set && column == "variables" {
            let used = h.url.contains(&format!("{{{key}}}"));
            let changes = vars.iter().any(|(k, v)| *k == key && Some(v) != value.as_ref());
            if used && changes {
                return Err(ApiError { status: 422, body: json!({"error": "Illegal key or value"}), headers: vec![] });
            }
        }
        let list = if column == "variables" { &mut vars } else { &mut heads };
        if set {
            list.retain(|(k, _)| k != &key);
            list.push((key.clone(), value.clone().unwrap_or_default()));
        } else {
            if !list.iter().any(|(k, _)| k == &key) {
                let what = if column == "variables" { "URL variable" } else { "Custom header" };
                return Err(Error::Api(404, json!(format!("404 {what} Not Found"))).into());
            }
            list.retain(|(k, _)| k != &key);
        }
        let pairs = |l: &[(String, String)]| -> Value { Value::Array(l.iter().map(|(k, v)| json!({"key": k, "value": v})).collect()) };
        c.execute(
            "UPDATE web_hooks SET custom_headers = ?1 WHERE id = ?2",
            params![json!({"headers": pairs(&heads), "variables": pairs(&vars)}).to_string(), h.id],
        )?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn set_variable(req: Req) -> ApiResult {
    set_kv(req, "variables", true).await
}
async fn unset_variable(req: Req) -> ApiResult {
    set_kv(req, "variables", false).await
}
async fn set_header(req: Req) -> ApiResult {
    set_kv(req, "headers", true).await
}
async fn unset_header(req: Req) -> ApiResult {
    set_kv(req, "headers", false).await
}

const TRIGGERS: &[&str] = &[
    "confidential_issues_events", "confidential_note_events", "deployment_events", "emoji_events", "feature_flag_events",
    "issues_events", "job_events", "merge_requests_events", "milestone_events", "note_events", "pipeline_events",
    "push_events", "releases_events", "resource_access_token_events", "resource_deploy_token_events", "tag_push_events",
    "wiki_page_events", "vulnerability_events",
];

/// Test payload for a trigger, or the no-data message.
fn test_data(c: &Connection, req: &Req, p: &Project, trigger: &str) -> Result<(String, Value), String> {
    let config = &req.app.config;
    let user = req.user.clone().unwrap();
    let r = repo::repo(c, p);
    let err = |m: &str| m.to_string();
    let push_like = |tag: bool| -> Result<(String, Value), String> {
        let branch = repo::default_branch(p);
        let head = r.branch(&branch).ok().flatten();
        let Some(head) = head else { return Err(err("Testing not available for this hook")) };
        let mut commits = labrat_git::walk::rev_list(&r, &[head], &[]).map_err(|e| e.to_string())?;
        commits.truncate(3);
        let old = *commits.last().unwrap();
        commits.reverse();
        let ref_name = if tag {
            let t = r.tags().ok().and_then(|t| t.into_iter().last()).map(|(n, _)| n).unwrap_or("v1.0.0".into());
            format!("refs/tags/{t}")
        } else {
            format!("refs/heads/{branch}")
        };
        let mut v = json!(null);
        let total = commits.len();
        let _ = core_hooks::push_event;
        let built = push_sample(c, config, p, &user, &old, &head, &ref_name, &commits, total).map_err(|e| e.to_string())?;
        v = built;
        Ok((if tag { "tag_push_hooks" } else { "push_hooks" }.to_string(), v))
    };
    match trigger {
        "push_events" => push_like(false),
        "tag_push_events" => push_like(true),
        "merge_requests_events" => {
            let mrs = MergeRequest::list_where(c, "target_project_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1", [p.id]).map_err(|e| e.to_string())?;
            let Some(mr) = mrs.into_iter().next() else { return Err(err("Ensure the project has merge requests.")) };
            let mut v = core_hooks::mr_payload(c, config, p, &mr, Some(&user), "open", None, None).map_err(|e| e.to_string())?;
            if let Some(a) = v["object_attributes"].as_object_mut() {
                a.shift_remove("actioned_at");
            }
            Ok(("merge_request_hooks".into(), v))
        }
        "note_events" | "emoji_events" => {
            let last: Option<i64> = c
                .query_row("SELECT id FROM notes WHERE project_id = ?1 ORDER BY id DESC LIMIT 1", [p.id], |r| r.get(0))
                .ok();
            let Some(note) = last.and_then(|id| Note::find(c, id).ok().flatten()) else {
                return Err(err("Ensure the project has notes."));
            };
            // The newest note may be on a commit.
            if note.noteable_type == "Commit" && trigger == "note_events" {
                let v = core_hooks::commit_note_payload(c, config, p, &note, "create").map_err(|e| e.to_string())?.ok_or_else(|| err("Ensure the project has notes."))?;
                return Ok(("note_hooks".into(), v));
            }
            let mr = MergeRequest::find(c, note.noteable_id).map_err(|e| e.to_string())?.ok_or_else(|| err("Ensure the project has notes."))?;
            if trigger == "note_events" {
                let v = core_hooks::note_payload(c, config, p, &mr, &user, &note, "create").map_err(|e| e.to_string())?;
                return Ok(("note_hooks".into(), v));
            }
            let now = time::now();
            let award = labrat_core::services::emoji::Award {
                id: 1,
                name: "thumbsup".into(),
                user_id: user.id,
                awardable_type: "Note".into(),
                awardable_id: note.id,
                created_at: now,
                updated_at: now,
            };
            let v = core_hooks::emoji_payload(c, config, p, &mr, Some(&note), &user, &award, "award").map_err(|e| e.to_string())?;
            Ok(("emoji_hooks".into(), v))
        }
        "issues_events" | "confidential_issues_events" => Err(err("Ensure the project has issues.")),
        "job_events" => Err(err("Ensure the project has CI jobs.")),
        "pipeline_events" => Err(err("Ensure the project has CI pipelines.")),
        "wiki_page_events" => Err(err("Ensure the wiki is enabled and has pages.")),
        "releases_events" => Err(err("Ensure the project has releases.")),
        "vulnerability_events" => Err(err("Ensure the project has vulnerabilities.")),
        _ => Err(err("Testing not available for this hook")),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_sample(
    c: &Connection,
    config: &labrat_core::Config,
    p: &Project,
    user: &labrat_core::model::User,
    old: &labrat_git::Oid,
    new: &labrat_git::Oid,
    ref_name: &str,
    commits: &[labrat_git::Oid],
    total: usize,
) -> labrat_core::Result<Value> {
    // Build without firing by temporarily collecting into a fake hook-less path.
    let r = repo::repo(c, p);
    let shown: Vec<Value> = commits.iter().map(|o| core_hooks::commit_attrs(config, p, &r, o, true)).collect::<labrat_core::Result<_>>()?;
    let kind = if ref_name.starts_with("refs/tags/") { "tag_push" } else { "push" };
    let mut m = Map::new();
    m.insert("object_kind".into(), json!(kind));
    m.insert("event_name".into(), json!(kind));
    m.insert("before".into(), json!(old.hex()));
    m.insert("after".into(), json!(new.hex()));
    m.insert("ref".into(), json!(ref_name));
    m.insert("ref_protected".into(), json!(repo::is_protected(c, p, ref_name.trim_start_matches("refs/heads/"))?));
    // Tags: the commit the named tag points to.
    let checkout = if ref_name.starts_with("refs/tags/") {
        match r.ref_oid(ref_name)? {
            Some(t) => json!(r.peel(&t)?.hex()),
            None => Value::Null,
        }
    } else {
        json!(new.hex())
    };
    m.insert("checkout_sha".into(), checkout);
    m.insert("message".into(), Value::Null);
    m.insert("user_id".into(), json!(user.id));
    m.insert("user_name".into(), json!(user.name));
    m.insert("user_username".into(), json!(user.username));
    m.insert("user_email".into(), json!(user.public_email.clone().filter(|e| !e.is_empty())));
    m.insert("user_avatar".into(), core_hooks::user_attrs(user)["avatar_url"].clone());
    m.insert("project_id".into(), json!(p.id));
    m.insert("project".into(), core_hooks::project_attrs(c, config, p)?);
    m.insert("commits".into(), Value::Array(shown));
    m.insert("total_commits_count".into(), json!(total));
    m.insert("push_options".into(), json!({}));
    let pa = core_hooks::project_attrs(c, config, p)?;
    m.insert(
        "repository".into(),
        json!({"name": pa["name"], "url": pa["url"], "description": pa["description"], "homepage": pa["homepage"],
               "git_http_url": pa["git_http_url"], "git_ssh_url": pa["git_ssh_url"], "visibility_level": pa["visibility_level"]}),
    );
    Ok(Value::Object(m))
}

/// `web_hook_test` rate limit: 5 requests per minute per project and user.
fn rate_limit(req: &Req, project_id: i64) -> Result<(), ApiError> {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    static HITS: std::sync::LazyLock<parking_lot::Mutex<HashMap<(usize, i64, i64), Vec<Instant>>>> =
        std::sync::LazyLock::new(Default::default);
    let key = (Arc::as_ptr(&req.app) as usize, project_id, req.user.as_ref().map(|u| u.id).unwrap_or(0));
    let mut hits = HITS.lock();
    let list = hits.entry(key).or_default();
    let now = Instant::now();
    list.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
    if list.len() >= 5 {
        return Err(Error::Api(429, json!({"error": "This endpoint has been requested too many times. Try again later."})).into());
    }
    list.push(now);
    Ok(())
}

async fn test(req: Req) -> ApiResult {
    req.user()?;
    let trigger = req.path_param("trigger").to_string();
    if !TRIGGERS.contains(&trigger.as_str()) {
        return Err(Error::Param("trigger does not have a valid value".into()).into());
    }
    let app = req.app.clone();
    let prepared = app.db.read(|c| {
        let p = admin_project(c, &req)?;
        let h = find_hook(c, &req, Some(&p))?;
        rate_limit(&req, p.id)?;
        Ok::<_, ApiError>((h, test_data(c, &req, &p, &trigger)))
    })?;
    let (hook, data) = prepared;
    let (scope, data) = data.map_err(|m| ApiError::from(Error::message(422, m)))?;
    let out = crate::hooks::send(&req.app, &hook, &scope, &data, &core_hooks::uuid()).await;
    match (out.status, out.error) {
        (Some(s), _) if (200..300).contains(&s) => Ok(Resp::created(json!({"message": "201 Created"}))),
        (Some(_), _) => Err(Error::message(422, out.body).into()),
        (None, Some(e)) => Err(Error::message(422, e).into()),
        (None, None) => Err(Error::message(422, "").into()),
    }
}

async fn events(mut req: Req) -> ApiResult {
    // `hook_id` is not a declared param of this route, so it stays out of pagination links.
    let hook_id = req.path_param("hook_id").to_string();
    req.path_params.retain(|(k, _)| k != "hook_id");
    let statuses = req.params.list("status");
    if let Some(pp) = req.params.str("per_page")
        && !pp.parse::<i64>().is_ok_and(|n| (1..=20).contains(&n)) {
            return Err(Error::Param("per_page does not have a valid value".into()).into());
        }
    let app = req.app.clone();
    app.db.read(|c| {
        let p = admin_project(c, &req)?;
        let h = find_hook_id(c, &hook_id, Some(&p))?;
        let mut st = c.prepare(
            "SELECT id, url, trigger, request_headers, request_data, response_headers, response_body, execution_duration, response_status, created_at
             FROM web_hook_logs WHERE web_hook_id = ?1 AND created_at >= ?2 ORDER BY created_at DESC, id DESC",
        )?;
        let since = time::now() - 7 * 86_400_000;
        let rows: Vec<Value> = st
            .query_map(params![h.id, since], |r| {
                let parse = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "url": r.get::<_, String>(1)?,
                    "trigger": r.get::<_, String>(2)?,
                    "request_headers": parse(r.get(3)?),
                    "request_data": parse(r.get(4)?),
                    "response_headers": parse(r.get::<_, Option<String>>(5)?.unwrap_or("{}".into())),
                    "response_body": r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    "execution_duration": r.get::<_, f64>(7)?,
                    "response_status": r.get::<_, String>(8)?,
                    "created_at": time::iso(r.get(9)?),
                }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let rows: Vec<Value> = rows
            .into_iter()
            .filter(|v| {
                let Some(list) = &statuses else { return true };
                let s = v["response_status"].as_str().unwrap_or("");
                let code: i64 = s.parse().unwrap_or(0);
                list.iter().any(|f| match f.as_str() {
                    "successful" => (200..300).contains(&code),
                    "client_failure" => (400..500).contains(&code),
                    "server_failure" => (500..600).contains(&code) || s == "internal error",
                    other => other == s,
                })
            })
            .collect();
        paginate(&req, rows, &[], |v| Ok(v.clone()))
    })
}

async fn resend(req: Req) -> ApiResult {
    let app = req.app.clone();
    let log_id: i64 = req.path_param("hook_log_id").parse().map_err(|_| Error::Param("hook_log_id is invalid".into()))?;
    let (hook, scope, data) = app.db.read(|c| {
        let p = admin_project(c, &req)?;
        let h = find_hook(c, &req, Some(&p))?;
        let row: Option<(String, String, String)> = c
            .query_row(
                "SELECT trigger, request_data, url FROM web_hook_logs WHERE id = ?1 AND web_hook_id = ?2",
                params![log_id, h.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let (trigger, data, url) = row.ok_or_else(|| Error::Api(404, json!("404 Not found")))?;
        if url != h.url {
            return Err(Error::message(422, "The hook URL has changed. This log entry cannot be retried.").into());
        }
        Ok::<_, ApiError>((h, trigger, serde_json::from_str::<Value>(&data).unwrap_or(Value::Null)))
    })?;
    let out = crate::hooks::send(&req.app, &hook, &scope, &data, &core_hooks::uuid()).await;
    match out.status {
        Some(s) => Ok(Resp::created(json!({"response_status": s}))),
        None => Err(Error::message(422, out.error.unwrap_or_default()).into()),
    }
}

// System hooks

async fn list_system(req: Req) -> ApiResult {
    req.admin()?;
    let app = req.app.clone();
    app.db.read(|c| {
        let hooks = Hook::list(c, "SystemHook", None)?;
        paginate(&req, hooks, &[], |h| Ok(hook_value(h, true)))
    })
}

async fn show_system(req: Req) -> ApiResult {
    req.admin()?;
    let app = req.app.clone();
    let body = app.db.read(|c| Ok::<_, ApiError>(hook_value(&find_hook(c, &req, None)?, true)))?;
    Ok(Resp::ok(body))
}

async fn create_system(req: Req) -> ApiResult {
    req.admin()?;
    req.params.require(&["url"])?;
    let hp = hook_params(&req, true)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| Ok::<_, ApiError>(hook_value(&save(c, None, "SystemHook", None, hp)?, true)))?;
    Ok(Resp::created(body))
}

async fn update_system(req: Req) -> ApiResult {
    req.admin()?;
    let hp = hook_params(&req, true)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let h = find_hook(c, &req, None)?;
        Ok::<_, ApiError>(hook_value(&save(c, Some(&h), "SystemHook", None, hp)?, true))
    })?;
    Ok(Resp::ok(body))
}

async fn destroy_system(req: Req) -> ApiResult {
    req.admin()?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let h = find_hook(c, &req, None)?;
        c.execute("DELETE FROM web_hooks WHERE id = ?1", [h.id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

async fn test_system(req: Req) -> ApiResult {
    req.admin()?;
    let app = req.app.clone();
    let hook = app.db.read(|c| find_hook(c, &req, None))?;
    let data = json!({"event_name": "project_create", "name": "Ruby", "path": "ruby", "project_id": 1,
                      "owner_name": "Someone", "owner_email": "example@gitlabhq.com"});
    crate::hooks::send(&req.app, &hook, "system_hooks", &data, &core_hooks::uuid()).await;
    Ok(Resp::created(data))
}

// Application settings (subset)

async fn get_settings(req: Req) -> ApiResult {
    req.admin()?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let mut m = Map::new();
        m.insert("id".into(), json!(1));
        m.insert(
            "allow_local_requests_from_web_hooks_and_services".into(),
            json!(setting(c, "allow_local_requests_from_web_hooks_and_services")?.as_bool().unwrap_or(false)),
        );
        m.insert(
            "allow_local_requests_from_system_hooks".into(),
            json!(setting(c, "allow_local_requests_from_system_hooks")?.as_bool().unwrap_or(true)),
        );
        Ok::<_, ApiError>(Value::Object(m))
    })?;
    Ok(Resp::ok(body))
}

async fn put_settings(req: Req) -> ApiResult {
    req.admin()?;
    let mut updates = vec![];
    for k in ["allow_local_requests_from_web_hooks_and_services", "allow_local_requests_from_system_hooks"] {
        if let Some(b) = req.params.bool(k)? {
            updates.push((k, b));
        }
    }
    let app = req.app.clone();
    app.db.tx(|c| {
        let s: String = c.query_row("SELECT settings FROM application_settings WHERE id = 1", [], |r| r.get(0))?;
        let mut m: Map<String, Value> = serde_json::from_str(&s).unwrap_or_default();
        for (k, v) in &updates {
            m.insert(k.to_string(), json!(v));
        }
        c.execute("UPDATE application_settings SET settings = ?1 WHERE id = 1", [Value::Object(m).to_string()])?;
        Ok::<_, ApiError>(())
    })?;
    get_settings(req).await
}

/// `validate_custom_webhook_template_numeric_safety`: with every `{{...}}` replaced by `0` the
/// template must parse as JSON, without huge exponents.
fn template_problem(t: &str) -> Option<&'static str> {
    let exponent = regex::Regex::new(r"\d+(\.\d+)?[eE][+-]?\d{4,}").unwrap();
    if exponent.is_match(t) {
        return Some("contains a numeric value that is too large");
    }
    let sanitized = regex::Regex::new(r"\{\{.+?\}\}").unwrap().replace_all(t, "0");
    match serde_json::from_str::<Value>(&sanitized) {
        Ok(_) => None,
        Err(_) => Some("exceeds allowed JSON complexity limits"),
    }
}
