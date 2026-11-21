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
