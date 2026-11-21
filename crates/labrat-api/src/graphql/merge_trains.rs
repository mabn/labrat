//! EE merge trains over GraphQL: `Project.ciCdSettings` and `projectCiCdSettingsUpdate`.

use super::exec::{Args, Ctx, FResult, Out};
use super::resolvers::{JsonObj, unavailable};
use labrat_core::model::Project;
use labrat_core::services::merge_trains as mt;
use serde_json::{Value, json};

/// `ProjectCiCdSetting` (the fields merge trains need), for maintainers only.
pub fn ci_cd_settings(cx: &Ctx, p: &Project) -> FResult<Out> {
    let Some(user) = cx.user() else { return Ok(Out::Null) };
    let perms = labrat_core::perms::ProjectPerms::load(cx.c, Some(user), p)?;
    if !perms.admin_project() {
        return Ok(Out::Null);
    }
    Ok(Out::obj(settings_obj(p)))
}

fn settings_obj(p: &Project) -> JsonObj {
    JsonObj::new(
        "ProjectCiCdSetting",
        json!({
            "mergePipelinesEnabled": mt::merge_pipelines_enabled(p),
            "mergeTrainsEnabled": mt::merge_trains_enabled(p),
            "mergeTrainsSkipTrainAllowed": mt::skip_train_allowed(p),
        }),
    )
}

/// `projectCiCdSettingsUpdate`: returns the updated project.
pub fn settings_update(cx: &Ctx, input: &Args) -> FResult<Project> {
    let user = cx.user().ok_or_else(unavailable)?.clone();
    let path = input.str("fullPath").unwrap_or_default();
    let p = Project::find_by_full_path(cx.c, &path)?.ok_or_else(unavailable)?;
    if !labrat_core::perms::ProjectPerms::load(cx.c, Some(&user), &p)?.admin_project() {
        return Err(unavailable());
    }
    let mut settings = vec![];
    for (arg, key) in [
        ("mergePipelinesEnabled", "merge_pipelines_enabled"),
        ("mergeTrainsEnabled", "merge_trains_enabled"),
        ("mergeTrainsSkipTrainAllowed", "merge_trains_skip_train_allowed"),
    ] {
        if let Some(b) = input.bool(arg) {
            settings.push((key.to_string(), Value::Bool(b)));
        }
    }
    if settings.is_empty() {
        return Ok(p);
    }
    let attrs = labrat_core::services::projects::ProjectAttrs { settings, ..Default::default() };
    Ok(labrat_core::services::projects::update(cx.c, &p, attrs)?)
}

/// The payload's `ciCdSettings`.
pub fn payload_settings(p: &Project) -> Out {
    Out::obj(settings_obj(p))
}

// ---------------------------------------------------------------------------------------------
// Trains, cars and the MR's train fields.

use super::exec::Object;
use super::resolvers::{Conn, MrObj, PipelineObj, Shared, UserObj, gid, id_cursor, parse_gid};
use labrat_core::model::User;
use labrat_core::services::ci;
use labrat_core::services::merge_requests::{MergeRequest, mergeability};
use labrat_core::services::merge_trains::Car;
use std::rc::Rc;

/// `MergeTrainCar`.
pub struct CarObj {
    p: Project,
    car: Car,
}

impl Object for CarObj {
    fn typename(&self) -> String {
        "MergeTrainCar".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let (c, car) = (cx.c, &self.car);
        Ok(match field {
            "id" => Out::val(gid("MergeTrains::Car", car.id)),
            "index" => Out::val(car.index(c)?),
            "status" => Out::val(car.status.to_uppercase()),
            "targetBranch" => Out::val(car.target_branch.clone()),
            "duration" => Out::val(car.duration),
            "mergedAt" => Out::val(car.merged_at.map(labrat_core::time::iso)),
            "createdAt" => Out::val(labrat_core::time::iso(car.created_at)),
            "updatedAt" => Out::val(labrat_core::time::iso(car.updated_at)),
            "mergeRequest" => Out::obj(MrObj::new(self.p.clone(), car.mr(c)?)),
            "user" => match User::find(c, car.user_id)? {
                Some(u) => Out::obj(UserObj::new("UserCore", u)),
                None => Out::Null,
            },
            "pipeline" => match car.pipeline_id.map(|id| ci::Pipeline::find(c, id)).transpose()?.flatten() {
                Some(pl) => Out::obj(PipelineObj::new(self.p.clone(), pl)),
                None => Out::Null,
            },
            "targetProject" => Out::obj(super::resolvers::JsonObj::new(
                "Project",
                json!({"id": gid("Project", self.p.id), "fullPath": self.p.full_path, "name": self.p.name}),
            )),
            "userPermissions" => {
                let ok = match cx.user() {
                    Some(u) => labrat_core::perms::ProjectPerms::load(c, Some(u), &self.p)?.developer(),
                    None => false,
                };
                Out::obj(super::resolvers::JsonObj::new("CarPermissions", json!({"deleteMergeTrainCar": ok})))
            }
            _ => return Err(super::resolvers::not_implemented("MergeTrainCar", field)),
        })
    }
}

/// `MergeTrain`: the cars for one target branch.
pub struct TrainObj {
    p: Project,
    branch: String,
}

impl Object for TrainObj {
    fn typename(&self) -> String {
        "MergeTrain".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        Ok(match field {
            "targetBranch" => Out::val(self.branch.clone()),
            // ACTIVE: front first; COMPLETED: latest first.
            "cars" => {
                let completed = args.str("activityStatus").as_deref() == Some("COMPLETED");
                let cars = if completed {
                    Car::list(cx.c, self.p.id, Some(&self.branch), Some("complete"), false)?
                } else {
                    Car::train(cx.c, self.p.id, &self.branch)?
                };
                let items = cars
                    .into_iter()
                    .map(|car| (id_cursor(car.id), Shared(Rc::new(CarObj { p: self.p.clone(), car }))))
                    .collect();
                Out::obj(Conn::new("MergeTrainCarConnection", items, args)?)
            }
            _ => return Err(super::resolvers::not_implemented("MergeTrain", field)),
        })
    }
}

/// `Project.mergeTrains(status, targetBranches)`: the trains with cars of that status (any
/// cars without one).
pub fn project_trains(cx: &Ctx, p: &Project, args: &Args) -> FResult<Out> {
    let scope = match args.str("status").as_deref() {
        Some("ACTIVE") => Some("active"),
        Some("COMPLETED") => Some("complete"),
        _ => None,
    };
    let branches_filter: Option<Vec<String>> = args.0.get("targetBranches").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
    let mut branches: Vec<String> = vec![];
    for car in Car::list(cx.c, p.id, None, scope, true)? {
        if !branches.contains(&car.target_branch) && branches_filter.as_ref().is_none_or(|f| f.contains(&car.target_branch)) {
            branches.push(car.target_branch.clone());
        }
    }
    let items = branches
        .into_iter()
        .enumerate()
        .map(|(i, b)| (id_cursor(i as i64 + 1), Shared(Rc::new(TrainObj { p: p.clone(), branch: b }))))
        .collect();
    Ok(Out::obj(Conn::new("MergeTrainConnection", items, args)?))
}

/// `mergeTrainCar`, `mergeTrainIndex` and `mergeTrainsCount` on `MergeRequest`.
pub fn mr_field(cx: &Ctx, p: &Project, mr: &MergeRequest, field: &str) -> FResult<Out> {
    let c = cx.c;
    Ok(match field {
        "mergeTrainCar" => match Car::for_mr(c, mr.id)? {
            Some(car) => Out::obj(CarObj { p: p.clone(), car }),
            None => Out::Null,
        },
        "mergeTrainIndex" => Out::val(Car::active_for_mr(c, mr.id)?.map(|car| car.index(c)).transpose()?.flatten()),
        _ => {
            if !mt::merge_trains_enabled(p) {
                return Ok(Out::Null);
            }
            Out::val(mt::train_length(c, p.id, &mr.target_branch)?)
        }
    })
}

/// `availableAutoMergeStrategies` with trains on: adding once checks pass while the pipeline
/// runs, adding now when the MR could merge.
pub fn available_strategies(cx: &Ctx, p: &Project, mr: &MergeRequest) -> FResult<Out> {
    let c = cx.c;
    let Some(u) = cx.user() else { return Ok(Out::val(Vec::<String>::new())) };
    if !mr.is_open() || !labrat_core::services::merge_requests::can_be_merged_by(c, p, mr, u)? || mergeability::auto_merge_blocking(c, p, mr)?.is_some() {
        return Ok(Out::val(Vec::<String>::new()));
    }
    let dhp = ci::diff_head_pipeline(c, p, mr)?;
    let running = dhp.as_ref().is_some_and(|pl| !pl.is_complete());
    Ok(Out::val(if running {
        vec!["add_to_merge_train_when_checks_pass"]
    } else if mergeability::mergeable(c, &cx.req.app.config, p, mr)? {
        vec!["merge_train"]
    } else {
        vec![]
    }))
}

/// `mergeTrainsDeleteCar`: Developers may take any car off its train.
pub fn delete_car(cx: &Ctx, input: &Args) -> FResult<()> {
    let user = cx.user().ok_or_else(unavailable)?.clone();
    let id = parse_gid(&input.str("carId").unwrap_or_default(), &["MergeTrains::Car"])?;
    let car = Car::find(cx.c, id)?.filter(Car::is_active).ok_or_else(unavailable)?;
    let p = Project::find(cx.c, car.project_id)?.ok_or_else(unavailable)?;
    if !labrat_core::perms::ProjectPerms::load(cx.c, Some(&user), &p)?.developer() {
        return Err(unavailable());
    }
    mt::drop_car(cx.c, &cx.req.app.config, &car, &user, None, true)?;
    Ok(())
}
