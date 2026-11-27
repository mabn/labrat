//! EE merge request dependencies: `/blocks` (what an MR waits for) and `/blockees` (what it
//! holds up). These routes exist only when labrat runs as EE.
//!
//! The behaviour follows recordings of an EE 18.11.14 instance (`fixtures-ee/mr_dependencies`).

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::merge_request::{self as ent, Opts};
use crate::find;
use crate::pagination::paginate;
use crate::routes::merge_requests::load;
use axum::Router;
use axum::routing::{delete, get};
use labrat_core::model::Project;
use labrat_core::perms::ProjectPerms;
use labrat_core::services::merge_requests::{self as mrs, MergeRequest};
use labrat_core::services::mr_blocks::{self as blocks, Block, CreateError};
use labrat_core::{App, Error};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    let mr = "/projects/{id}/merge_requests/{merge_request_iid}";
    Router::new()
        .route(&format!("{mr}/blocks"), get(list_blocks).post(create))
        .route(&format!("{mr}/blocks/{{block_id}}"), delete(destroy))
        .route(&format!("{mr}/blockees"), get(list_blockees))
}

/// Whether the current user may read `mr`.
pub fn can_read(c: &Connection, req: &Req, mr: &MergeRequest) -> labrat_core::Result<bool> {
    let Some(p) = Project::find(c, mr.target_project_id)? else { return Ok(false) };
    let perms = ProjectPerms::load(c, req.user.as_ref(), &p)?;
    Ok(perms.read_merge_request(&p))
}

/// The basic MR entity: reading `merge_status` as a user who can update the MR queues an async
/// mergeability recheck first.
fn mr_json(c: &Connection, req: &Req, mr: &MergeRequest) -> Result<Value, ApiError> {
    let p = Project::find(c, mr.target_project_id)?.expect("project exists");
    let mut mr = mr.clone();
    if let Some(u) = &req.user
        && labrat_core::services::merge_requests::can_update(c, &p, &mr, u)?
        && labrat_core::services::merge_requests::mergeability::check_async(c, &mr)?
    {
        mr = mr.reload(c)?;
    }
    Ok(Value::Object(ent::basic(c, &req.app.config, req.user.as_ref(), &p, &mr, &Opts::default())?))
}

/// A block; the blocker is left out when the user can't read it. `project_id` is the
/// blocker's project, except right after creation, where EE answers `null`.
fn block_json(c: &Connection, req: &Req, b: &Block, created: bool) -> Result<Value, ApiError> {
    let blocking = MergeRequest::get(c, b.blocking_merge_request_id)?;
    let blocked = MergeRequest::get(c, b.blocked_merge_request_id)?;
    let mut m = Map::new();
    m.insert("id".into(), json!(b.id));
    if can_read(c, req, &blocking)? {
        m.insert("blocking_merge_request".into(), mr_json(c, req, &blocking)?);
    }
    if can_read(c, req, &blocked)? {
        m.insert("blocked_merge_request".into(), mr_json(c, req, &blocked)?);
    }
    m.insert("project_id".into(), if created { Value::Null } else { json!(blocking.target_project_id) });
    Ok(Value::Object(m))
}

async fn list_blocks(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        let rows = blocks::blocking(c, mr.id)?;
        paginate(&req, rows, &[], |b| block_json(c, &req, b, false))
    })?;
    Ok(body)
}

async fn list_blockees(req: Req) -> ApiResult {
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (_, _, mr) = load(c, &req)?;
        let rows = blocks::blocked(c, mr.id)?;
        paginate(&req, rows, &[], |b| block_json(c, &req, b, false))
    })?;
    Ok(body)
}

/// The MR to block, which the user must be able to update.
fn load_for_update(c: &Connection, req: &Req) -> Result<(Project, MergeRequest), ApiError> {
    let user = req.user()?;
    let (p, _, mr) = load(c, req)?;
    if !mrs::can_update(c, &p, &mr, user)? {
        return Err(Error::forbidden().into());
    }
    Ok((p, mr))
}

fn not_found_blocker() -> ApiError {
    Error::Api(404, json!("Blocking merge request not found")).into()
}

async fn create(req: Req) -> ApiResult {
    let by_id = req.params.int("blocking_merge_request_id")?;
    let by_iid = req.params.int("blocking_merge_request_iid")?;
    match (by_id, by_iid) {
        (None, None) => {
            return Err(Error::Param(
                "blocking_merge_request_id, blocking_merge_request_iid are missing, exactly one parameter must be provided".into(),
            )
            .into());
        }
        (Some(_), Some(_)) => {
            return Err(Error::Param("blocking_merge_request_id, blocking_merge_request_iid are mutually exclusive".into()).into());
        }
        _ => {}
    }
    let blocking_project = req.params.str("blocking_project_id");
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, mr) = load_for_update(c, &req)?;
        let user = req.user()?;
        let blocking = match (by_id, by_iid) {
            (Some(id), _) => MergeRequest::find(c, id)?.filter(|b| can_read(c, &req, b).unwrap_or(false)),
            (_, Some(iid)) => {
                let bp = match &blocking_project {
                    Some(id) => find::project(c, &req, id)?.0,
                    None => p.clone(),
                };
                let perms = ProjectPerms::load(c, req.user.as_ref(), &bp)?;
                MergeRequest::find_by_iid(c, bp.id, iid)?.filter(|_| perms.read_merge_request(&bp))
            }
            _ => unreachable!(),
        };
        let blocking = blocking.ok_or_else(not_found_blocker)?;
        match blocks::create(c, user, &blocking, &mr)? {
            Ok(b) => block_json(c, &req, &b, true),
            Err(CreateError::SelfReferential) => Err(Error::Api(400, json!("This block is self-referential")).into()),
            Err(CreateError::Exists) => Err(Error::Api(409, json!("Block already exists")).into()),
        }
    })?;
    Ok(Resp::created(body))
}

async fn destroy(req: Req) -> ApiResult {
    let block_id: i64 = req.path_param("block_id").parse().map_err(|_| Error::Param("block_id is invalid".into()))?;
    let app = req.app.clone();
    app.db.tx(|c| {
        let (_, mr) = load_for_update(c, &req)?;
        let user = req.user()?;
        let b = blocks::find(c, block_id)?
            .filter(|b| b.blocked_merge_request_id == mr.id)
            .ok_or_else(|| ApiError::from(Error::Api(404, json!("404 Not found"))))?;
        // Removing a block needs access to the blocker too.
        let blocking = MergeRequest::get(c, b.blocking_merge_request_id)?;
        if !can_read(c, &req, &blocking)? {
            return Err(Error::forbidden().into());
        }
        blocks::destroy(c, user, &b)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}
