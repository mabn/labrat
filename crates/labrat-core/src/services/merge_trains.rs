//! EE merge trains: the project settings, the cars and their pipelines on
//! `refs/merge-requests/:iid/train`, and the train refresh that merges, drops and restarts
//! cars.

use crate::model::Project;
use serde_json::Value;

fn stored(p: &Project, key: &str) -> bool {
    p.setting(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Merged results pipelines on.
pub fn merge_pipelines_enabled(p: &Project) -> bool {
    stored(p, "merge_pipelines_enabled")
}

/// Merge trains on: they read as off while merged results pipelines are off.
pub fn merge_trains_enabled(p: &Project) -> bool {
    merge_pipelines_enabled(p) && stored(p, "merge_trains_enabled")
}

/// Skipping the train allowed: only while merge trains are on.
pub fn skip_train_allowed(p: &Project) -> bool {
    merge_trains_enabled(p) && stored(p, "merge_trains_skip_train_allowed")
}

use crate::db::OptionalExt;
use crate::services::merge_requests::MergeRequest;
use crate::{Result, time};
use rusqlite::{Connection, Row, params};

/// Car statuses while on the train, and once off it after merging.
pub const ACTIVE: &[&str] = &["idle", "stale", "fresh"];
pub const COMPLETE: &[&str] = &["merging", "merged", "skip_merged"];

/// One MR on a train (a car).
#[derive(Debug, Clone)]
pub struct Car {
    pub id: i64,
    pub project_id: i64,
    pub merge_request_id: i64,
    pub user_id: i64,
    pub target_branch: String,
    pub status: String,
    pub pipeline_id: Option<i64>,
    pub previous_ref_sha: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub merged_at: Option<i64>,
    pub duration: Option<i64>,
}

const CAR_COLS: &str = "id, project_id, merge_request_id, user_id, target_branch, status, pipeline_id, previous_ref_sha, created_at, updated_at, merged_at, duration";

fn car_row(r: &Row) -> rusqlite::Result<Car> {
    Ok(Car {
        id: r.get(0)?,
        project_id: r.get(1)?,
        merge_request_id: r.get(2)?,
        user_id: r.get(3)?,
        target_branch: r.get(4)?,
        status: r.get(5)?,
        pipeline_id: r.get(6)?,
        previous_ref_sha: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        merged_at: r.get(10)?,
        duration: r.get(11)?,
    })
}

fn in_list(list: &[&str]) -> String {
    list.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", ")
}

impl Car {
    pub fn find(c: &Connection, id: i64) -> Result<Option<Car>> {
        c.query_row(&format!("SELECT {CAR_COLS} FROM merge_train_cars WHERE id = ?1"), [id], car_row).optional()
    }

    /// The MR's car: its active one, else its latest completed one.
    pub fn for_mr(c: &Connection, mr_id: i64) -> Result<Option<Car>> {
        c.query_row(
            &format!("SELECT {CAR_COLS} FROM merge_train_cars WHERE merge_request_id = ?1 ORDER BY (status IN ({})) DESC, id DESC LIMIT 1", in_list(ACTIVE)),
            [mr_id],
            car_row,
        )
        .optional()
    }

    /// The MR's car while it is on a train.
    pub fn active_for_mr(c: &Connection, mr_id: i64) -> Result<Option<Car>> {
        c.query_row(
            &format!("SELECT {CAR_COLS} FROM merge_train_cars WHERE merge_request_id = ?1 AND status IN ({}) ORDER BY id DESC LIMIT 1", in_list(ACTIVE)),
            [mr_id],
            car_row,
        )
        .optional()
    }

    /// Cars of a project, optionally of one target branch and scope (`active`/`complete`),
    /// by id.
    pub fn list(c: &Connection, project_id: i64, target_branch: Option<&str>, scope: Option<&str>, asc: bool) -> Result<Vec<Car>> {
        let status = match scope {
            Some("active") => format!(" AND status IN ({})", in_list(ACTIVE)),
            Some("complete") => format!(" AND status IN ({})", in_list(COMPLETE)),
            _ => String::new(),
        };
        let mut st = c.prepare(&format!(
            "SELECT {CAR_COLS} FROM merge_train_cars WHERE project_id = ?1 AND (?2 IS NULL OR target_branch = ?2){status} ORDER BY id {}",
            if asc { "ASC" } else { "DESC" }
        ))?;
        Ok(st.query_map(params![project_id, target_branch], car_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// The active cars of one train, front first.
    pub fn train(c: &Connection, project_id: i64, target_branch: &str) -> Result<Vec<Car>> {
        Car::list(c, project_id, Some(target_branch), Some("active"), true)
    }

    pub fn is_active(&self) -> bool {
        ACTIVE.contains(&self.status.as_str())
    }

    /// The car's 0-based position on its train while active (`MergeTrainCar.index`).
    pub fn index(&self, c: &Connection) -> Result<Option<i64>> {
        if !self.is_active() {
            return Ok(None);
        }
        Ok(Some(c.query_row(
            &format!("SELECT count(*) FROM merge_train_cars WHERE project_id = ?1 AND target_branch = ?2 AND status IN ({}) AND id < ?3", in_list(ACTIVE)),
            params![self.project_id, self.target_branch, self.id],
            |r| r.get(0),
        )?))
    }

    pub fn reload(&self, c: &Connection) -> Result<Car> {
        Ok(Car::find(c, self.id)?.unwrap_or_else(|| self.clone()))
    }

    pub fn mr(&self, c: &Connection) -> Result<MergeRequest> {
        MergeRequest::get(c, self.merge_request_id)
    }
}

/// The number of active cars on the MR's target branch train (`mergeTrainsCount`).
pub fn train_length(c: &Connection, project_id: i64, target_branch: &str) -> Result<i64> {
    Ok(Car::train(c, project_id, target_branch)?.len() as i64)
}

fn touch(c: &Connection, car: &Car, status: &str) -> Result<()> {
    c.execute("UPDATE merge_train_cars SET status = ?1, updated_at = ?2 WHERE id = ?3", params![status, time::now(), car.id])?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Adding, removing and the train refresh.

use crate::model::User;
use crate::services::ci::Pipeline;
use crate::services::merge_requests::{merge as mrmerge, mergeability};
use crate::services::notes;
use crate::{Config, Error, jobs, repo};
use labrat_git::Oid;
use serde_json::json;

#[derive(Debug, Default, Clone)]
pub struct AddOpts {
    /// Add once the checks pass (`auto_merge`, formerly `when_pipeline_succeeds`).
    pub auto_merge: bool,
    pub sha: Option<String>,
    pub squash: Option<bool>,
}

pub enum Added {
    /// The MR is on the train (201).
    Now,
    /// The MR joins when its checks pass (202).
    WhenChecksPass,
}

fn train_link(config: &Config, p: &Project) -> String {
    format!("[merge train]({})", config.url(&format!("/{}/-/merge_trains", p.full_path)))
}

/// Whether the MR could join the train right now: mergeable, with a finished pipeline (a
/// failed one too: the train runs its own).
fn ready(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest) -> Result<bool> {
    let finished = crate::services::ci::diff_head_pipeline(c, p, mr)?.is_some_and(|pl| pl.is_complete());
    Ok(finished && mergeability::mergeable(c, config, p, mr)?)
}

/// `POST /merge_trains/merge_requests/:iid` and the
/// train merge strategies of `mergeRequestAccept`.
pub fn add(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, user: &User, opts: &AddOpts) -> Result<Added> {
    if !merge_trains_enabled(p) {
        return Err(Error::BadRequest(Some("Merge trains are not enabled for this project".into())));
    }
    if !crate::services::merge_requests::can_be_merged_by(c, p, mr, user)? {
        return Err(Error::Unauthorized(None));
    }
    if mr.merge_when_pipeline_succeeds {
        return Err(Error::Api(409, json!("Merge request is already set to Auto-Merge")));
    }
    let head = mr.diff_head_sha(c, p)?.unwrap_or_default();
    if let Some(sha) = &opts.sha
        && *sha != head
    {
        return Err(Error::Api(409, json!(format!("SHA does not match HEAD of source branch: {head}"))));
    }
    if let Some(sq) = opts.squash {
        c.execute("UPDATE merge_requests SET squash = ?1 WHERE id = ?2", params![sq, mr.id])?;
    }
    let mr = mr.reload(c)?;
    if ready(c, config, p, &mr)? {
        start_car(c, config, p, &mr, user)?;
        return Ok(Added::Now);
    }
    if !opts.auto_merge {
        return Err(Error::BadRequest(Some("Failed to merge".into())));
    }
    set_strategy(c, &mr, user, "add_to_merge_train_when_checks_pass", &head)?;
    notes::system_note(c, mr.id, p.id, user, "enabled automatic add to merge train when checks pass", "merge")?;
    Ok(Added::WhenChecksPass)
}

fn set_strategy(c: &Connection, mr: &MergeRequest, user: &User, strategy: &str, sha: &str) -> Result<()> {
    let mut mp = mr.merge_params.clone();
    mp.insert("auto_merge_strategy".into(), json!(strategy));
    mp.insert("sha".into(), json!(sha));
    c.execute(
        "UPDATE merge_requests SET merge_when_pipeline_succeeds = 1, merge_user_id = ?1, merge_params = ?2 WHERE id = ?3",
        params![user.id, Value::Object(mp).to_string(), mr.id],
    )?;
    Ok(())
}

/// The MR's auto-merge strategy, when auto-merge is on.
pub fn strategy(mr: &MergeRequest) -> Option<String> {
    if !mr.merge_when_pipeline_succeeds {
        return None;
    }
    mr.merge_params.get("auto_merge_strategy").and_then(Value::as_str).map(String::from)
}

/// Puts the MR on its target branch's train as the last car.
fn start_car(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, user: &User) -> Result<Car> {
    let head = mr.diff_head_sha(c, p)?.unwrap_or_default();
    set_strategy(c, mr, user, "merge_train", &head)?;
    let now = time::now();
    c.execute(
        "INSERT INTO merge_train_cars (project_id, merge_request_id, user_id, target_branch, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'idle', ?5, ?5)",
        params![p.id, mr.id, user.id, mr.target_branch, now],
    )?;
    let car = Car::find(c, c.last_insert_rowid())?.expect("just inserted");
    let position = car.index(c)?.unwrap_or(0) + 1;
    let link = train_link(config, p);
    let body = if position == 1 { format!("started a {link}") } else { format!("added this merge request to the {link} at position {position}") };
    notes::system_note(c, mr.id, p.id, user, &body, "merge")?;
    enqueue_refresh(c, p.id, &mr.target_branch)?;
    Ok(car)
}

pub fn enqueue_refresh(c: &Connection, project_id: i64, target_branch: &str) -> Result<()> {
    jobs::enqueue(c, "merge_train_refresh", json!({"project_id": project_id, "target_branch": target_branch}))
}

/// `add_to_merge_train_when_checks_pass`: once the MR's pipeline passes, it joins the train.
pub fn auto_add(c: &Connection, config: &Config, mr: &MergeRequest) -> Result<bool> {
    if strategy(mr).as_deref() != Some("add_to_merge_train_when_checks_pass") || !mr.is_open() {
        return Ok(false);
    }
    let Some(p) = Project::find(c, mr.target_project_id)? else { return Ok(false) };
    if !ready(c, config, &p, mr)? {
        return Ok(true);
    }
    let Some(user) = mr.merge_user_id.map(|id| User::find(c, id)).transpose()?.flatten() else { return Ok(true) };
    start_car(c, config, &p, mr, &user)?;
    Ok(true)
}

/// Takes the car off its train: the car is deleted,
/// auto-merge goes off, its pipeline is canceled when `cancel_pipeline`, and the cars behind it
/// restart. `reason` ends the system note.
pub fn drop_car(c: &Connection, config: &Config, car: &Car, user: &User, reason: Option<&str>, cancel_pipeline: bool) -> Result<()> {
    let p = Project::find(c, car.project_id)?.ok_or(Error::not_found("Project"))?;
    let mr = car.mr(c)?;
    let behind: Vec<Car> = Car::train(c, car.project_id, &car.target_branch)?.into_iter().filter(|x| x.id > car.id).collect();
    c.execute("DELETE FROM merge_train_cars WHERE id = ?1", [car.id])?;
    clear_auto_merge(c, &mr)?;
    if cancel_pipeline && let Some(pl) = car.pipeline_id {
        crate::services::fake_ci::cancel_pipeline(c, config, pl)?;
    }
    let body = match reason {
        Some(r) => format!("removed this merge request from the merge train because {r}"),
        None => "removed this merge request from the merge train".to_string(),
    };
    notes::system_note(c, mr.id, p.id, user, &body, "merge")?;
    restart(c, config, &behind)?;
    enqueue_refresh(c, p.id, &car.target_branch)
}

fn clear_auto_merge(c: &Connection, mr: &MergeRequest) -> Result<()> {
    let mut mp = mr.merge_params.clone();
    for k in ["should_remove_source_branch", "commit_message", "squash_commit_message", "auto_merge_strategy"] {
        mp.remove(k);
    }
    c.execute(
        "UPDATE merge_requests SET merge_when_pipeline_succeeds = 0, merge_user_id = NULL, merge_params = ?1 WHERE id = ?2",
        params![Value::Object(mp).to_string(), mr.id],
    )?;
    Ok(())
}

/// Marks cars stale and cancels their pipelines; the refresh gives them new ones.
fn restart(c: &Connection, config: &Config, cars: &[Car]) -> Result<()> {
    for car in cars {
        touch(c, car, "stale")?;
        if let Some(pl) = car.pipeline_id {
            crate::services::fake_ci::cancel_pipeline(c, config, pl)?;
        }
    }
    Ok(())
}

/// Restarts a whole train (after an MR merged past it).
pub fn restart_train(c: &Connection, config: &Config, project_id: i64, target_branch: &str) -> Result<()> {
    let cars = Car::train(c, project_id, target_branch)?;
    if cars.is_empty() {
        return Ok(());
    }
    restart(c, config, &cars)?;
    enqueue_refresh(c, project_id, target_branch)
}

/// The car of an MR that leaves the train for a reason: closed,
/// marked as draft, pushed to.
pub fn abort_for_mr(c: &Connection, config: &Config, mr: &MergeRequest, user: &User, reason: &str, clear_now: bool) -> Result<bool> {
    let Some(car) = Car::active_for_mr(c, mr.id)? else { return Ok(false) };
    // GitLab drops the car from a worker, after the request; closing clears auto-merge at once.
    let _ = config;
    if clear_now {
        clear_auto_merge(c, mr)?;
    }
    jobs::enqueue(c, "merge_train_abort", json!({"car_id": car.id, "user_id": user.id, "reason": reason}))?;
    Ok(true)
}

/// Drops the MR's car right away (a push to its source branch).
pub fn abort_now(c: &Connection, config: &Config, mr: &MergeRequest, user: &User, reason: &str) -> Result<bool> {
    let Some(car) = Car::active_for_mr(c, mr.id)? else { return Ok(false) };
    drop_car(c, config, &car, user, Some(reason), true)?;
    Ok(true)
}

/// The `merge_train_abort` job.
pub fn abort_job(c: &Connection, config: &Config, car_id: i64, user_id: i64, reason: &str) -> Result<()> {
    let Some(car) = Car::find(c, car_id)? else { return Ok(()) };
    if !car.is_active() {
        return Ok(());
    }
    let Some(user) = User::find(c, user_id)? else { return Ok(()) };
    drop_car(c, config, &car, &user, Some(reason), true)
}

/// `cancel_merge_when_pipeline_succeeds` for an MR on a train: removes it.
pub fn remove(c: &Connection, config: &Config, mr: &MergeRequest, user: &User) -> Result<bool> {
    let Some(car) = Car::active_for_mr(c, mr.id)? else { return Ok(false) };
    drop_car(c, config, &car, user, None, true)?;
    Ok(true)
}

/// Builds the train ref of `mr` on `base`: a merge of the MR head into it. `None` on conflicts.
fn build_train_ref(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, base: &Oid) -> Result<Option<Oid>> {
    let r = repo::repo(c, p);
    let Some(head) = mr.diff_head_sha(c, p)?.and_then(|h| Oid::from_hex(&h).ok()) else { return Ok(None) };
    let mb = labrat_git::walk::merge_base(&r, base, &head)?;
    let base_tree = match mb {
        Some(b) => Some(r.commit(&b)?.tree),
        None => None,
    };
    let merged = labrat_git::merge::merge_trees(&r, base_tree.as_ref(), &r.commit(base)?.tree, &r.commit(&head)?.tree)?;
    if !merged.clean() {
        return Ok(None);
    }
    let author = User::get(c, mr.author_id)?;
    let sig = repo::signature(&author);
    let message = mrmerge::default_merge_commit_message(c, config, p, mr, Some(&author))?;
    let oid = r.write_commit(&labrat_git::Commit { tree: merged.tree, parents: vec![*base, head], author: sig.clone(), committer: sig, extra: vec![], message })?;
    r.set_ref(&format!("refs/merge-requests/{}/train", mr.iid), &oid)?;
    Ok(Some(oid))
}

/// The train refresh: merges the front car once its pipeline passes, drops cars
/// whose pipeline failed (restarting the ones behind), and gives cars without a current
/// pipeline a new one on top of the car ahead.
pub fn refresh(c: &Connection, config: &Config, project_id: i64, target_branch: &str) -> Result<()> {
    let Some(p) = Project::find(c, project_id)? else { return Ok(()) };
    let r = repo::repo(c, &p);
    // Each pass handles at most one merge or drop, then starts over with the new train.
    for _ in 0..64 {
        let cars = Car::train(c, project_id, target_branch)?;
        let Some(mut base) = r.branch(target_branch)? else { return Ok(()) };
        let mut changed = false;
        // EE creates one car pipeline per refresh and refreshes again for the next car.
        let mut created = false;
        for (i, car) in cars.iter().enumerate() {
            let pl = car.pipeline_id.map(|id| Pipeline::find(c, id)).transpose()?.flatten();
            let user = User::get(c, car.user_id)?;
            let mr = car.mr(c)?;
            if car.status != "stale"
                && let Some(pl) = &pl
            {
                if matches!(pl.status.as_str(), "failed" | "canceled" | "skipped") {
                    drop_car(c, config, car, &user, Some(&format!(
                        "the pipeline did not succeed. [Learn more]({}).",
                        config.url("/help/ci/pipelines/merge_trains.md#merge-request-dropped-from-the-merge-train")
                    )), false)?;
                    changed = true;
                    break;
                }
                if i == 0 && pl.status == "success" {
                    merge_car(c, config, &p, car, &mr, &user)?;
                    changed = true;
                    break;
                }
                base = Oid::from_hex(&pl.sha).map_err(Error::from)?;
                continue;
            }
            // A new pipeline on top of the car ahead.
            if created {
                return enqueue_refresh(c, project_id, target_branch);
            }
            created = true;
            let Some(train_sha) = build_train_ref(c, config, &p, &mr, &base)? else {
                drop_car(c, config, car, &user, Some("the merge request could not be merged with the previous merge requests"), false)?;
                changed = true;
                break;
            };
            let head = mr.diff_head_sha(c, &p)?.unwrap_or_default();
            let new = crate::services::fake_ci::create(
                c,
                config,
                &p,
                &user,
                &train_sha.hex(),
                &format!("refs/merge-requests/{}/train", mr.iid),
                Some((&mr, &head, &base.hex())),
            )?;
            c.execute(
                "UPDATE merge_train_cars SET pipeline_id = ?1, previous_ref_sha = ?2, status = 'fresh', updated_at = ?3 WHERE id = ?4",
                params![new.id, base.hex(), time::now(), car.id],
            )?;
            crate::services::ci::update_head_pipeline(c, &p, &mr)?;
            base = train_sha;
        }
        if !changed {
            return Ok(());
        }
    }
    Ok(())
}

/// Merges the front car's MR.
fn merge_car(c: &Connection, config: &Config, p: &Project, car: &Car, mr: &MergeRequest, user: &User) -> Result<()> {
    touch(c, car, "merging")?;
    let mp = mrmerge::MergeParams { sha: mr.merge_params.get("sha").and_then(Value::as_str).map(String::from), ..Default::default() };
    let merged = mrmerge::merge(c, config, p, mr, user, &mp)?;
    if !merged {
        let reason = mr.reload(c)?.merge_error.unwrap_or_else(|| "the merge failed".into());
        return drop_car(c, config, &car.reload(c)?, user, Some(&format!("the merge failed: {reason}")), false);
    }
    let now = time::now();
    c.execute(
        "UPDATE merge_train_cars SET status = 'merged', merged_at = ?1, duration = ?2, updated_at = ?1 WHERE id = ?3",
        params![now, (now - car.created_at + 500) / 1000, car.id],
    )?;
    Ok(())
}

/// A pipeline finished: refresh the train of a car that ran it, or add an MR waiting for its
/// checks to pass.
pub fn on_pipeline_complete(c: &Connection, _config: &Config, pl: &Pipeline) -> Result<()> {
    let Some(mr_id) = pl.merge_request_id else { return Ok(()) };
    crate::services::approvals::finish_sync(c, mr_id)?;
    let car: Option<Car> = c
        .query_row(&format!("SELECT {CAR_COLS} FROM merge_train_cars WHERE pipeline_id = ?1"), [pl.id], car_row)
        .optional()?;
    if let Some(car) = car {
        return enqueue_refresh(c, car.project_id, &car.target_branch);
    }
    if let Some(mr) = MergeRequest::find(c, mr_id)?
        && strategy(&mr).as_deref() == Some("add_to_merge_train_when_checks_pass")
    {
        jobs::enqueue(c, "merge_train_auto_add", json!({"merge_request_id": mr.id}))?;
    }
    Ok(())
}

/// An MR merged directly while its target branch has a train ("merge immediately"): the train
/// restarts on top of it. With `skip_merge_train` allowed, the MR is recorded as a
/// `skip_merged` car.
pub fn merged_past_train(c: &Connection, config: &Config, p: &Project, mr: &MergeRequest, user: &User, skip: bool) -> Result<()> {
    if !merge_trains_enabled(p) {
        return Ok(());
    }
    if skip && skip_train_allowed(p) {
        let now = time::now();
        c.execute(
            "INSERT INTO merge_train_cars (project_id, merge_request_id, user_id, target_branch, status, created_at, updated_at, merged_at, duration)
             VALUES (?1, ?2, ?3, ?4, 'skip_merged', ?5, ?5, ?5, NULL)",
            params![p.id, mr.id, user.id, mr.target_branch, now],
        )?;
    }
    restart_train(c, config, p.id, &mr.target_branch)
}
