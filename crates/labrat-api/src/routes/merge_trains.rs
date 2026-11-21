//! EE merge trains: the train listings, one MR's
//! car and adding an MR. These routes exist only when labrat runs as EE.
//!
//! The behaviour follows recordings of an EE 18.11.14 instance (`fixtures-ee/merge_train_*`).

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::user::{self as user_entity, Ctx};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::routing::get;
use labrat_core::model::{Project, User};
use labrat_core::services::ci::Pipeline;
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::merge_trains::{self as mt, Car};
use labrat_core::{App, Error, time};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/merge_trains", get(list))
        .route("/projects/{id}/merge_trains/merge_requests/{merge_request_iid}", get(show).post(add))
        .route("/projects/{id}/merge_trains/{target_branch}", get(list_branch))
}

/// A merge train car, as EE renders it.
pub fn car_json(c: &Connection, req: &Req, p: &Project, car: &Car) -> Result<Value, ApiError> {
    let ctx = Ctx { base_url: &req.app.config.external_url, ee: true, current_user: req.user.as_ref() };
    let mr = MergeRequest::get(c, car.merge_request_id)?;
    let mut m = Map::new();
    m.insert("id".into(), json!(car.id));
    m.insert(
        "merge_request".into(),
        json!({
            "id": mr.id,
            "iid": mr.iid,
            "project_id": mr.target_project_id,
            "title": mr.title,
            "description": mr.description,
            "state": mr.state,
            "created_at": time::iso(mr.created_at),
            "updated_at": time::iso(mr.updated_at),
            "web_url": req.app.config.url(&format!("/{}/-/merge_requests/{}", p.full_path, mr.iid)),
        }),
    );
    m.insert("user".into(), User::find(c, car.user_id)?.map(|u| user_entity::basic_value(&u, &ctx)).unwrap_or(Value::Null));
    let pipeline = match car.pipeline_id.map(|id| Pipeline::find(c, id)).transpose()?.flatten() {
        Some(pl) => Value::Object(crate::routes::pipelines::pipeline_basic(&req.app.config, p, &pl)),
        None => Value::Null,
    };
    m.insert("pipeline".into(), pipeline);
    m.insert("created_at".into(), json!(time::iso(car.created_at)));
    m.insert("updated_at".into(), json!(time::iso(car.updated_at)));
    m.insert("target_branch".into(), json!(car.target_branch));
    m.insert("status".into(), json!(car.status));
    m.insert("merged_at".into(), json!(car.merged_at.map(time::iso)));
    m.insert("duration".into(), json!(car.duration));
    Ok(Value::Object(m))
}

fn list_params(req: &Req) -> Result<(Option<String>, bool), ApiError> {
    let scope = req.params.str("scope");
    if let Some(s) = &scope
        && !matches!(s.as_str(), "active" | "complete")
    {
        return Err(Error::Param("scope does not have a valid value".into()).into());
    }
    let asc = match req.params.str("sort").as_deref() {
        None | Some("desc") => false,
        Some("asc") => true,
        Some(_) => return Err(Error::Param("sort does not have a valid value".into()).into()),
    };
    Ok((scope, asc))
}

/// The project for the train endpoints: readable, with the merge request feature.
fn project(c: &Connection, req: &Req) -> Result<Project, ApiError> {
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !perms.read_merge_request(&p) {
        return Err(Error::Forbidden(None).into());
    }
    Ok(p)
}

async fn list(req: Req) -> ApiResult {
    req.user()?;
    req.app.db.read(|c| {
        let p = project(c, &req)?;
        let (scope, asc) = list_params(&req)?;
        let cars = Car::list(c, p.id, None, scope.as_deref(), asc)?;
        paginate(&req, cars, &[("sort", "desc")], |car| car_json(c, &req, &p, car))
    })
}

async fn list_branch(req: Req) -> ApiResult {
    req.user()?;
    req.app.db.read(|c| {
        let p = project(c, &req)?;
        let (scope, asc) = list_params(&req)?;
        let branch = req.path_param("target_branch").to_string();
        let cars = Car::list(c, p.id, Some(&branch), scope.as_deref(), asc)?;
        paginate(&req, cars, &[("sort", "desc")], |car| car_json(c, &req, &p, car))
    })
}

fn load_mr(c: &Connection, req: &Req, p: &Project) -> Result<MergeRequest, ApiError> {
    let iid: i64 = req.path_param("merge_request_iid").parse().map_err(|_| Error::Param("merge_request_iid is invalid".into()))?;
    MergeRequest::find_by_iid(c, p.id, iid)?.ok_or_else(|| Error::Api(404, json!("404 Not found")).into())
}

async fn show(req: Req) -> ApiResult {
    req.user()?;
    let body = req.app.db.read(|c| {
        let p = project(c, &req)?;
        let mr = load_mr(c, &req, &p)?;
        let car = Car::for_mr(c, mr.id)?.ok_or_else(|| ApiError::from(Error::not_found("Merge Train Merge Request")))?;
        car_json(c, &req, &p, &car)
    })?;
    Ok(Resp::ok(body))
}

async fn add(req: Req) -> ApiResult {
    let me = req.user()?.clone();
    let config = req.app.config.clone();
    let res = req.app.db.tx(|c| {
        let p = project(c, &req)?;
        let mr = load_mr(c, &req, &p)?;
        let opts = mt::AddOpts {
            auto_merge: req.params.bool("auto_merge")?.or(req.params.bool("when_pipeline_succeeds")?).unwrap_or(false),
            sha: req.params.str("sha"),
            squash: req.params.bool("squash")?,
        };
        // The links of the paginated answer carry the declared params that were given.
        let given: Vec<(String, String)> = ["auto_merge", "sha", "squash", "when_pipeline_succeeds"]
            .iter()
            .filter_map(|k| {
                req.params.raw(k).map(|v| (k.to_string(), match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }))
            })
            .collect();
        let defaults: Vec<(&str, &str)> = given.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (cars, status) = match mt::add(c, &config, &p, &mr, &me, &opts)? {
            // Every car of the target branch, merged ones too.
            mt::Added::Now => (Car::list(c, p.id, Some(&mr.target_branch), None, true)?, axum::http::StatusCode::CREATED),
            mt::Added::WhenChecksPass => (vec![], axum::http::StatusCode::ACCEPTED),
        };
        let mut resp = paginate(&req, cars, &defaults, |car| car_json(c, &req, &p, car))?;
        resp.status = status;
        Ok::<_, ApiError>(resp)
    })?;
    crate::jobs::kick(&req.app);
    Ok(res)
}
