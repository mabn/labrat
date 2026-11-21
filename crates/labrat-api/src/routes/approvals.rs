//! EE approval settings and approval rules: the project and group approval settings, project
//! and MR approval rules, and the MR approval state endpoints. These routes exist only when
//! labrat runs as EE; on CE they fall through to the API's `404 Not Found`.
//!
//! The behaviour follows recordings of an EE 18.11.14 instance (`fixtures-ee/approval_*`).

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::user::{self as user_entity, Ctx};
use crate::find;
use crate::pagination::paginate;
use crate::routes::merge_requests::load;
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Namespace, Project, User};
use labrat_core::perms::{GroupPerms, ProjectPerms};
use labrat_core::services::approvals::{self as ap, Rule, RuleParams, Scope};
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::{App, Config, Error};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    let mr = "/projects/{id}/merge_requests/{merge_request_iid}";
    Router::new()
        .route("/projects/{id}/approvals", get(project_approvals).post(change_project_approvals))
        .route("/projects/{id}/merge_request_approval_setting", get(project_setting).put(change_project_setting))
        .route("/groups/{id}/merge_request_approval_setting", get(group_setting).put(change_group_setting))
        .route("/groups/{id}/approval_rules", get(group_rules_disabled).post(group_rules_disabled))
        .route("/groups/{id}/approval_rules/{approval_rule_id}", axum::routing::put(group_rules_disabled))
        .route("/projects/{id}/approval_rules", get(list_project_rules).post(create_project_rule))
        .route(
            "/projects/{id}/approval_rules/{approval_rule_id}",
            get(show_project_rule).put(update_project_rule).delete(delete_project_rule),
        )
        .route(&format!("{mr}/approval_state"), get(approval_state))
        .route(&format!("{mr}/approval_settings"), get(approval_settings))
        .route(&format!("{mr}/approval_rules"), get(list_mr_rules).post(create_mr_rule))
        .route(
            &format!("{mr}/approval_rules/{{approval_rule_id}}"),
            get(show_mr_rule).put(update_mr_rule).delete(delete_mr_rule),
        )
}

fn ectx(req: &Req) -> Ctx<'_> {
    Ctx { base_url: &req.app.config.external_url, ee: req.app.config.is_ee(), current_user: req.user.as_ref() }
}

fn user_json(c: &Connection, ctx: &Ctx, uid: i64) -> labrat_core::Result<Option<Value>> {
    Ok(User::find(c, uid)?.map(|u| user_entity::basic_value(&u, ctx)))
}

fn users_json(c: &Connection, ctx: &Ctx, ids: &[i64]) -> labrat_core::Result<Vec<Value>> {
    let mut out = vec![];
    for id in ids {
        if let Some(v) = user_json(c, ctx, *id)? {
            out.push(v);
        }
    }
    Ok(out)
}

/// A rule's groups the viewer can see, and whether any were hidden.
fn groups_json(c: &Connection, req: &Req, ids: &[i64]) -> labrat_core::Result<(Vec<Value>, bool)> {
    let mut out = vec![];
    let mut hidden = false;
    for id in ids {
        if let Some(g) = Namespace::find(c, *id)? {
            let perms = GroupPerms::load(c, req.user.as_ref(), &g)?;
            if !perms.read_group() {
                hidden = true;
                continue;
            }
            out.push(Value::Object(crate::entities::group::group_with(&g, &req.app.config, perms.admin_group())));
        }
    }
    Ok((out, hidden))
}

/// Maintainers (and admins) manage approval settings and project rules.
fn admin_project(c: &Connection, req: &Req) -> Result<(Project, ProjectPerms), ApiError> {
    req.user()?;
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !perms.admin_project() {
        return Err(Error::forbidden().into());
    }
    Ok((p, perms))
}

// ---------------------------------------------------------------------------------------------
// Settings

async fn project_approvals(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = admin_project(c, &req)?;
        Ok::<_, ApiError>(ap::legacy_json(c, &p)?)
    })?;
    Ok(Resp::ok(body))
}

const LEGACY_PARAMS: &[&str] = &[
    "approvals_before_merge",
    "reset_approvals_on_push",
    "selective_code_owner_removals",
    "disable_overriding_approvers_per_merge_request",
    "merge_requests_author_approval",
    "merge_requests_disable_committers_approval",
    "require_password_to_approve",
    "require_reauthentication_to_approve",
];

async fn change_project_approvals(req: Req) -> ApiResult {
    // Grape coerces the types before checking that one parameter is there.
    let count = req.params.int("approvals_before_merge")?;
    let mut bools = vec![];
    for k in &LEGACY_PARAMS[1..] {
        if let Some(v) = req.params.bool(k)? {
            bools.push((*k, v));
        }
    }
    req.params.require_any(LEGACY_PARAMS)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _) = admin_project(c, &req)?;
        let mut values: Vec<(&str, Value)> = vec![];
        if let Some(n) = count {
            values.push(("approvals_before_merge", json!(n)));
        }
        for (k, v) in &bools {
            // `require_reauthentication_to_approve` is the same setting.
            let key = if *k == "require_reauthentication_to_approve" { "require_password_to_approve" } else { k };
            values.push((key, json!(v)));
        }
        let s = ap::project_settings(&p);
        let get = |k: &str, own: bool| values.iter().rev().find(|(n, _)| *n == k).and_then(|(_, v)| v.as_bool()).unwrap_or(own);
        if get("selective_code_owner_removals", s.selective_code_owner_removals) && get("reset_approvals_on_push", s.reset_approvals_on_push) {
            return Err(Error::Validation(vec![(
                "base".into(),
                "selective_code_owner_removals can only be enabled when reset_approvals_on_push is disabled".into(),
            )])
            .into());
        }
        ap::set_project_settings(c, &p, &values)?;
        let p = Project::find(c, p.id)?.expect("project exists");
        Ok::<_, ApiError>(ap::legacy_json(c, &p)?)
    })?;
    Ok(Resp::created(body))
}

async fn project_setting(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = admin_project(c, &req)?;
        Ok::<_, ApiError>(ap::effective(c, &p)?.json())
    })?;
    Ok(Resp::ok(body))
}

const SETTING_PARAMS: &[&str] = &[
    "allow_author_approval",
    "allow_committer_approval",
    "allow_overrides_to_approver_list_per_merge_request",
    "retain_approvals_on_push",
    "selective_code_owner_removals",
    "require_password_to_approve",
    "require_reauthentication_to_approve",
];

fn setting_params(req: &Req) -> Result<Vec<(&'static str, bool)>, ApiError> {
    let mut out = vec![];
    for k in SETTING_PARAMS {
        if let Some(v) = req.params.bool(k)? {
            out.push((*k, v));
        }
    }
    req.params.require_any(SETTING_PARAMS)?;
    Ok(out)
}

async fn change_project_setting(req: Req) -> ApiResult {
    let given = setting_params(&req)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _) = admin_project(c, &req)?;
        let s = ap::project_settings(&p);
        let mut values: Vec<(&str, Value)> = vec![];
        for (k, v) in &given {
            let (key, stored) = match *k {
                "allow_author_approval" => ("merge_requests_author_approval", *v),
                "allow_committer_approval" => ("merge_requests_disable_committers_approval", !v),
                "allow_overrides_to_approver_list_per_merge_request" => ("disable_overriding_approvers_per_merge_request", !v),
                "retain_approvals_on_push" => ("reset_approvals_on_push", !v),
                "selective_code_owner_removals" => ("selective_code_owner_removals", *v),
                _ => ("require_password_to_approve", *v),
            };
            values.push((key, json!(stored)));
        }
        let get = |k: &str, own: bool| values.iter().rev().find(|(n, _)| *n == k).and_then(|(_, v)| v.as_bool()).unwrap_or(own);
        if get("selective_code_owner_removals", s.selective_code_owner_removals) && get("reset_approvals_on_push", s.reset_approvals_on_push) {
            return Err(Error::Api(400, json!("selective_code_owner_removals can only be enabled when retain_approvals_on_push is enabled")).into());
        }
        ap::set_project_settings(c, &p, &values)?;
        let p = Project::find(c, p.id)?.expect("project exists");
        Ok::<_, ApiError>(ap::effective(c, &p)?.json())
    })?;
    Ok(Resp::ok(body))
}

/// Group settings: owners of top-level groups only.
fn owned_top_group(c: &Connection, req: &Req) -> Result<Namespace, ApiError> {
    req.user()?;
    let (g, perms): (Namespace, GroupPerms) = find::group(c, req, req.path_param("id"))?;
    if !perms.owner() || g.parent_id.is_some() {
        return Err(Error::forbidden().into());
    }
    Ok(g)
}

async fn group_setting(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let g = owned_top_group(c, &req)?;
        Ok::<_, ApiError>(ap::group_json(&g))
    })?;
    Ok(Resp::ok(body))
}

async fn change_group_setting(req: Req) -> ApiResult {
    let given = setting_params(&req)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let g = owned_top_group(c, &req)?;
        // Groups have no "selective code owner removals" of their own.
        let values: Vec<(&str, bool)> = given
            .iter()
            .filter(|(k, _)| *k != "selective_code_owner_removals")
            .map(|(k, v)| (if *k == "require_password_to_approve" { "require_reauthentication_to_approve" } else { *k }, *v))
            .collect();
        ap::set_group_settings(c, &g, &values)?;
        let g = Namespace::find(c, g.id)?.expect("group exists");
        Ok::<_, ApiError>(ap::group_json(&g))
    })?;
    Ok(Resp::ok(body))
}

/// Group approval rules sit behind the `approval_group_rules` flag, off in 18.11.
async fn group_rules_disabled() -> ApiResult {
    Err(Error::NotFound(None).into())
}

// ---------------------------------------------------------------------------------------------
// Rules

/// Reads `name`, `approvals_required`, approvers and branches. Users must be project members
/// and groups visible to the caller; unknown ones are dropped.
fn rule_params(c: &Connection, req: &Req, p: &Project, scope: Scope) -> Result<RuleParams, ApiError> {
    let mut rp = RuleParams { name: req.params.str("name"), approvals_required: req.params.int("approvals_required")?, ..Default::default() };
    if scope == Scope::Project {
        rp.rule_type = req.params.str("rule_type");
        rp.report_type = req.params.str("report_type");
        rp.applies_to_all_protected_branches = req.params.bool("applies_to_all_protected_branches")?;
        if let Some(t) = &rp.rule_type
            && !["regular", "any_approver", "report_approver", "code_owner"].contains(&t.as_str()) {
                // An unknown enum value raises in the model: a 500.
                return Err(Error::Api(500, json!("500 Internal Server Error")).into());
            }
        if let Some(b) = req.params.int_list("protected_branch_ids")? {
            let mut keep = vec![];
            for id in b {
                let ok: bool = c.query_row("SELECT count(*) FROM protected_branches WHERE id = ?1 AND project_id = ?2", rusqlite::params![id, p.id], |r| r.get::<_, i64>(0))? > 0;
                if ok {
                    keep.push(id);
                }
            }
            rp.protected_branch_ids = Some(keep);
        }
    }
    let mut users: Option<Vec<i64>> = None;
    if let Some(ids) = req.params.int_list("user_ids")? {
        users = Some(ids);
    }
    if let Some(names) = req.params.list("usernames") {
        let v = users.get_or_insert_with(Vec::new);
        for n in names {
            if let Some(u) = User::find_by_username(c, &n)? {
                v.push(u.id);
            }
        }
    }
    if let Some(ids) = users {
        let mut keep = vec![];
        for id in ids {
            if ap::project_member(c, p, id)? && !keep.contains(&id) {
                keep.push(id);
            }
        }
        // A list of approvers that were all dropped still marks the rule as regular.
        rp.user_ids = Some(keep);
        if req.params.list("user_ids").is_some_and(|v| !v.is_empty()) || req.params.list("usernames").is_some_and(|v| !v.is_empty()) {
            rp.approvers_given = true;
        }
    }
    if let Some(ids) = req.params.int_list("group_ids")? {
        let mut keep = vec![];
        for id in &ids {
            if let Some(g) = Namespace::find(c, *id)?.filter(Namespace::is_group)
                && GroupPerms::load(c, req.user.as_ref(), &g)?.read_group() {
                    keep.push(g.id);
                }
        }
        if !ids.is_empty() {
            rp.approvers_given = true;
        }
        rp.group_ids = Some(keep);
    }
    Ok(rp)
}

fn base_rule_json(c: &Connection, req: &Req, r: &Rule, eligible: bool) -> Result<Map<String, Value>, ApiError> {
    let ctx = ectx(req);
    let mut m = Map::new();
    m.insert("id".into(), json!(r.id));
    m.insert("name".into(), json!(r.name));
    m.insert("rule_type".into(), json!(r.rule_type));
    if eligible {
        m.insert("eligible_approvers".into(), json!(users_json(c, &ctx, &ap::rule_approver_ids(c, r)?)?));
    }
    m.insert("approvals_required".into(), json!(r.approvals_required));
    m.insert("users".into(), json!(users_json(c, &ctx, &r.user_ids)?));
    let (groups, hidden) = groups_json(c, req, &r.group_ids)?;
    m.insert("groups".into(), json!(groups));
    m.insert("contains_hidden_groups".into(), json!(hidden));
    m.insert("report_type".into(), json!(r.report_type));
    Ok(m)
}

/// A project approval rule, as EE renders it.
fn project_rule_json(c: &Connection, req: &Req, p: &Project, r: &Rule) -> Result<Value, ApiError> {
    let mut m = base_rule_json(c, req, r, true)?;
    let mut branches = vec![];
    for pb in labrat_core::repo::protected_branches(c, p.id)? {
        if ap::rule_protected_branch_ids(c, p, r)?.contains(&pb.id) {
            branches.push(crate::entities::repository::protected_branch(&pb, true));
        }
    }
    m.insert("protected_branches".into(), json!(branches));
    m.insert("applies_to_all_protected_branches".into(), json!(r.applies_to_all_protected_branches));
    Ok(Value::Object(m))
}

fn source_rule(c: &Connection, p: &Project, r: &Rule) -> labrat_core::Result<Option<Rule>> {
    match r.source_id {
        Some(id) => ap::project_rule(c, p, id),
        None => Ok(None),
    }
}

/// An MR approval rule, as EE renders it.
fn mr_rule_json(c: &Connection, req: &Req, p: &Project, r: &Rule) -> Result<Value, ApiError> {
    let mut m = base_rule_json(c, req, r, true)?;
    let source = source_rule(c, p, r)?;
    m.insert("section".into(), Value::Null);
    m.insert("source_rule".into(), source.as_ref().map(|s| json!({"approvals_required": s.approvals_required})).unwrap_or(Value::Null));
    let w = ap::Wrapped { rule: r.clone(), source, approvers: vec![], approved_by: vec![] };
    m.insert("overridden".into(), json!(w.overridden()));
    Ok(Value::Object(m))
}

/// Listing and showing project rules: anyone who can read the project.
fn read_project(c: &Connection, req: &Req) -> Result<Project, ApiError> {
    req.user()?;
    Ok(find::project(c, req, req.path_param("id"))?.0)
}

fn rule_id(req: &Req) -> Result<i64, ApiError> {
    req.path_param("approval_rule_id").parse().map_err(|_| Error::Param("approval_rule_id is invalid".into()).into())
}

fn rule_not_found() -> ApiError {
    Error::Api(404, json!("404 Not found")).into()
}

async fn list_project_rules(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let p = read_project(c, &req)?;
        let rules = ap::project_rules_listed(c, &p)?;
        paginate(&req, rules, &[], |r| project_rule_json(c, &req, &p, r))
    })
}

async fn show_project_rule(req: Req) -> ApiResult {
    let id = rule_id(&req)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let p = read_project(c, &req)?;
        let r = ap::project_rule(c, &p, id)?.ok_or_else(rule_not_found)?;
        project_rule_json(c, &req, &p, &r)
    })?;
    Ok(Resp::ok(body))
}

async fn create_project_rule(req: Req) -> ApiResult {
    req.params.require(&["name", "approvals_required"])?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _) = admin_project(c, &req)?;
        let rp = rule_params(c, &req, &p, Scope::Project)?;
        match ap::create_rule(c, Scope::Project, p.id, &rp)? {
            Ok(r) => project_rule_json(c, &req, &p, &r),
            Err(errs) => Err(Error::Validation(errs).into()),
        }
    })?;
    Ok(Resp::created(body))
}

async fn update_project_rule(req: Req) -> ApiResult {
    let id = rule_id(&req)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _) = admin_project(c, &req)?;
        let r = ap::project_rule(c, &p, id)?.ok_or_else(rule_not_found)?;
        let rp = rule_params(c, &req, &p, Scope::Project)?;
        match ap::update_rule(c, &r, &rp)? {
            Ok(r) => project_rule_json(c, &req, &p, &r),
            Err(errs) => Err(Error::Validation(errs).into()),
        }
    })?;
    Ok(Resp::ok(body))
}

async fn delete_project_rule(req: Req) -> ApiResult {
    let id = rule_id(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _) = admin_project(c, &req)?;
        let r = ap::project_rule(c, &p, id)?.ok_or_else(rule_not_found)?;
        ap::delete_rule(c, &r)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

/// Who may change an MR's rules: its author (developer or above), maintainers and admins, and
/// only while the project allows overrides.
fn edit_mr_rules(c: &Connection, req: &Req) -> Result<(Project, MergeRequest), ApiError> {
    let me = req.user()?.clone();
    let (p, perms, mr) = load(c, req)?;
    let allowed = perms.admin || perms.admin_project() || (mr.author_id == me.id && perms.developer());
    if !allowed || !ap::effective(c, &p)?.allow_overrides_to_approver_list_per_merge_request.value {
        return Err(Error::forbidden().into());
    }
    Ok((p, mr))
}

async fn list_mr_rules(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        req.user()?;
        let (p, _, mr) = load(c, &req)?;
        let rules = ap::mr_rules(c, &mr)?;
        paginate(&req, rules, &[], |r| mr_rule_json(c, &req, &p, r))
    })
}

async fn show_mr_rule(req: Req) -> ApiResult {
    let id = rule_id(&req)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        req.user()?;
        let (p, _, mr) = load(c, &req)?;
        let r = ap::mr_rule(c, &mr, id)?.ok_or_else(rule_not_found)?;
        mr_rule_json(c, &req, &p, &r)
    })?;
    Ok(Resp::ok(body))
}

async fn create_mr_rule(req: Req) -> ApiResult {
    req.params.require(&["name", "approvals_required"])?;
    let source = req.params.int("approval_project_rule_id")?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, mr) = edit_mr_rules(c, &req)?;
        let mut rp = rule_params(c, &req, &p, Scope::MergeRequest)?;
        // A project rule given as the source supplies the name and approvers.
        if let Some(sid) = source
            && let Some(src) = ap::project_rule(c, &p, sid)? {
                rp.name = Some(src.name.clone());
                rp.user_ids = Some(src.user_ids.clone());
                rp.group_ids = Some(src.group_ids.clone());
                rp.rule_type = Some(src.rule_type.clone());
                rp.source_id = Some(src.id);
            }
        match ap::create_rule(c, Scope::MergeRequest, mr.id, &rp)? {
            Ok(r) => mr_rule_json(c, &req, &p, &r),
            Err(errs) => Err(Error::Validation(errs).into()),
        }
    })?;
    Ok(Resp::created(body))
}

async fn update_mr_rule(req: Req) -> ApiResult {
    let id = rule_id(&req)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, mr) = edit_mr_rules(c, &req)?;
        let r = ap::mr_rule(c, &mr, id)?.ok_or_else(rule_not_found)?;
        let rp = rule_params(c, &req, &p, Scope::MergeRequest)?;
        match ap::update_rule(c, &r, &rp)? {
            Ok(r) => mr_rule_json(c, &req, &p, &r),
            Err(errs) => Err(Error::Validation(errs).into()),
        }
    })?;
    Ok(Resp::ok(body))
}

async fn delete_mr_rule(req: Req) -> ApiResult {
    let id = rule_id(&req)?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (_, mr) = edit_mr_rules(c, &req)?;
        let r = ap::mr_rule(c, &mr, id)?.ok_or_else(rule_not_found)?;
        ap::delete_rule(c, &r)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

// ---------------------------------------------------------------------------------------------
// MR approval state

/// The MR's approval state (`/approval_state`), and the older `approval_settings` shape.
fn state_json(c: &Connection, req: &Req, p: &Project, mr: &MergeRequest, settings_shape: bool) -> Result<Value, ApiError> {
    let ctx = ectx(req);
    let st = ap::state(c, p, mr)?;
    let mut rules = vec![];
    for w in &st.rules {
        let mut m = Map::new();
        m.insert("id".into(), json!(w.rule.id));
        m.insert("name".into(), json!(w.name()));
        m.insert("rule_type".into(), json!(w.rule.rule_type));
        if !settings_shape {
            m.insert("eligible_approvers".into(), json!(users_json(c, &ctx, &w.approvers)?));
        }
        m.insert("approvals_required".into(), json!(w.rule.approvals_required));
        m.insert("users".into(), json!(users_json(c, &ctx, &w.rule.user_ids)?));
        let (groups, hidden) = groups_json(c, req, &w.rule.group_ids)?;
        m.insert("groups".into(), json!(groups));
        m.insert("contains_hidden_groups".into(), json!(hidden));
        m.insert("report_type".into(), json!(w.rule.report_type));
        m.insert("section".into(), Value::Null);
        let source = if st.overwritten { w.source.as_ref() } else { None };
        m.insert("source_rule".into(), source.map(|s| json!({"approvals_required": s.approvals_required})).unwrap_or(Value::Null));
        m.insert("overridden".into(), json!(st.overwritten && w.overridden()));
        m.insert("code_owner".into(), json!(false));
        m.insert("approved_by".into(), json!(users_json(c, &ctx, &w.approved_by)?));
        m.insert("approved".into(), json!(w.approved()));
        if settings_shape {
            m.insert("approvers".into(), json!(users_json(c, &ctx, &w.approvers)?));
            m.insert("commented_by".into(), json!([]));
        }
        rules.push(Value::Object(m));
    }
    Ok(json!({"approval_rules_overwritten": st.overwritten, "rules": rules}))
}

async fn approval_state(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        req.user()?;
        let (p, _, mr) = load(c, &req)?;
        state_json(c, &req, &p, &mr, false)
    })?;
    Ok(Resp::ok(body))
}

async fn approval_settings(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        req.user()?;
        let (p, _, mr) = load(c, &req)?;
        state_json(c, &req, &p, &mr, true)
    })?;
    Ok(Resp::ok(body))
}

/// The approvals body as EE's `/approvals`, `/approve` and `/unapprove` return it.
pub fn approvals_json(c: &Connection, ctx: &Ctx, config: &Config, p: &Project, mr: &MergeRequest, current: Option<&User>) -> labrat_core::Result<Value> {
    let st = ap::state(c, p, mr)?;
    let mut approved_by = vec![];
    for (uid, at) in &st.approvals {
        if let Some(u) = User::find(c, *uid)? {
            approved_by.push(json!({"user": user_entity::basic_value(&u, ctx), "approved_at": labrat_core::time::iso(*at)}));
        }
    }
    let (has, can) = match current {
        Some(u) => {
            let has = st.has_approved(u.id);
            (has, !has && ap::eligibility(c, p, mr, u)? == ap::Eligibility::Yes)
        }
        None => (false, false),
    };
    let left: Vec<Value> = st
        .rules
        .iter()
        .filter(|w| !w.approved())
        .map(|w| json!({"id": w.rule.id, "name": w.name(), "rule_type": w.rule.rule_type}))
        .collect();
    let mut m = Map::new();
    m.insert("id".into(), json!(mr.id));
    m.insert("iid".into(), json!(mr.iid));
    m.insert("project_id".into(), json!(mr.target_project_id));
    m.insert("title".into(), json!(mr.title));
    m.insert("description".into(), json!(mr.description));
    m.insert("state".into(), json!(mr.state));
    m.insert("created_at".into(), json!(labrat_core::time::iso(mr.created_at)));
    m.insert("updated_at".into(), json!(labrat_core::time::iso(mr.updated_at)));
    m.insert(
        "merge_status".into(),
        json!(labrat_core::services::merge_requests::mergeability::public_status(&mr.merge_status)),
    );
    m.insert("approved".into(), json!(st.approved()));
    m.insert("approvals_required".into(), json!(st.approvals_required()));
    m.insert("approvals_left".into(), json!(st.approvals_left()));
    m.insert("require_password_to_approve".into(), json!(ap::effective(c, p)?.require_password_to_approve.value));
    m.insert("approved_by".into(), json!(approved_by));
    m.insert("suggested_approvers".into(), json!(users_json(c, ctx, &st.suggested_approvers())?));
    m.insert("approvers".into(), json!([]));
    m.insert("approver_groups".into(), json!([]));
    m.insert("user_has_approved".into(), json!(has));
    m.insert("user_can_approve".into(), json!(can));
    m.insert("approval_rules_left".into(), json!(left));
    m.insert("has_approval_rules".into(), json!(!st.rules.is_empty()));
    m.insert("merge_request_approvers_available".into(), json!(true));
    m.insert("multiple_approval_rules_available".into(), json!(true));
    m.insert("invalid_approvers_rules".into(), json!([]));
    let _ = config;
    Ok(Value::Object(m))
}
