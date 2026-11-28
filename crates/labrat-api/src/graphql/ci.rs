//! CI pipelines and jobs in GraphQL (roadmap phase 7): `Pipeline`'s stages and jobs, `CiStage`,
//! `CiGroup`, `CiJob`, `CiJobArtifact`, their detailed statuses (GitLab's
//! `Gitlab::Ci::Status` factories: a core status per state, decorated with warnings and the
//! retry, cancel and play actions), and the job and pipeline mutations.

use super::exec::{Args, Ctx, FResult, FieldError, Object, Out};
use super::resolvers::{Conn, JsonObj, Shared, gtime, parse_gid, unavailable};
use labrat_core::model::{Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::ci::Pipeline;
use labrat_core::services::pipelines::{self, Artifact, Job, Stage};
use labrat_core::time;
use serde_json::{Value, json};
use std::rc::Rc;

fn opt_time(ms: Option<i64>) -> Value {
    ms.map(gtime).unwrap_or(Value::Null)
}

/// Array-connection cursors: the 1-based position.
fn positions<T>(items: Vec<T>) -> Vec<(Value, T)> {
    items.into_iter().enumerate().map(|(i, x)| (json!((i + 1).to_string()), x)).collect()
}

/// A status action (`StatusAction`).
#[derive(Clone)]
struct Action {
    icon: &'static str,
    title: &'static str,
    button_title: &'static str,
    path: String,
    /// What the action asks before it runs.
    confirmation: Option<String>,
}

/// A fabricated detailed status.
#[derive(Clone)]
pub struct Status {
    /// `Core#id`: the core status's group and the subject's id.
    core_id: String,
    name: String,
    text: String,
    label: String,
    icon: String,
    group: String,
    tooltip: String,
    has_details: bool,
    details_path: Option<String>,
    favicon: String,
    action: Option<Action>,
    /// The subject's class, for the action's id.
    class: &'static str,
}

/// The core status of a state: text, label, icon, favicon icon, name, group.
fn core(status: &str) -> (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str) {
    match status {
        "pending" => ("Pending", "pending", "status_pending", "status_pending", "PENDING", "pending"),
        "running" => ("Running", "running", "status_running", "status_running", "RUNNING", "running"),
        "success" => ("Passed", "passed", "status_success", "status_success", "SUCCESS", "success"),
        "failed" => ("Failed", "failed", "status_failed", "status_failed", "FAILED", "failed"),
        "canceled" => ("Canceled", "canceled", "status_canceled", "status_canceled", "CANCELED", "canceled"),
        "canceling" => ("Canceling", "canceling", "status_canceled", "status_canceled", "CANCELING", "canceling"),
        "skipped" => ("Skipped", "skipped", "status_skipped", "status_skipped", "SKIPPED", "skipped"),
        "manual" => ("Manual", "manual action", "status_manual", "status_manual", "MANUAL", "manual"),
        "scheduled" => ("Scheduled", "scheduled", "status_scheduled", "status_scheduled", "SCHEDULED", "scheduled"),
        "preparing" => ("Preparing", "preparing", "status_preparing", "status_preparing", "PREPARING", "preparing"),
        "waiting_for_resource" => ("Waiting", "waiting for resource", "status_pending", "status_pending", "WAITING_FOR_RESOURCE", "waiting-for-resource"),
        _ => ("Created", "created", "status_created", "status_created", "CREATED", "created"),
    }
}

fn base(status: &str, subject_id: &str, class: &'static str) -> Status {
    let (text, label, icon, fav, name, group) = core(status);
    Status {
        core_id: format!("{group}-{subject_id}"),
        name: name.into(),
        text: text.into(),
        label: label.into(),
        icon: icon.into(),
        group: group.into(),
        tooltip: label.into(),
        has_details: false,
        details_path: None,
        favicon: fav.into(),
        action: None,
        class,
    }
}

/// `Status::SuccessWarning`.
fn success_warning(s: &mut Status) {
    s.text = "Warning".into();
    s.label = "passed with warnings".into();
    s.tooltip = s.label.clone();
    s.icon = "status_warning".into();
    s.name = "SUCCESS_WITH_WARNINGS".into();
    s.group = "success-with-warnings".into();
}

/// `Pipeline::Factory`.
pub fn pipeline_status(c: &rusqlite::Connection, p: &Project, pl: &Pipeline, can_read: bool) -> labrat_core::Result<Status> {
    let mut s = base(&pl.status, &pl.id.to_string(), "Ci::Pipeline");
    s.has_details = can_read;
    s.details_path = Some(format!("/{}/-/pipelines/{}", p.full_path, pl.id));
    let jobs = Job::for_pipeline(c, pl.id, false)?;
    if pl.status == "success" && pipelines::has_warnings(&jobs) {
        success_warning(&mut s);
    } else if pl.status == "manual" {
        s.text = "Blocked".into();
        s.label = "waiting for manual action".into();
    }
    Ok(s)
}

/// `Stage::Factory`.
fn stage_status(p: &Project, st: &Stage, jobs: &[Job], can_read: bool) -> Status {
    let mut s = base(&st.status, &st.id.to_string(), "Ci::Stage");
    s.has_details = can_read;
    s.details_path = Some(format!("/{}/-/pipelines/{}#{}", p.full_path, st.pipeline_id, st.name));
    if st.status == "success" && pipelines::has_warnings(jobs) {
        success_warning(&mut s);
    }
    // `PlayManual`: a blocked or skipped stage can play its manual jobs.
    if matches!(st.status.as_str(), "manual" | "skipped") {
        // `confirm_manual_job?`: one of its manual jobs asks for confirmation.
        let confirm = jobs.iter().any(|j| j.status == "manual" && j.manual_confirmation_message().is_some());
        s.action = Some(Action {
            icon: "play",
            title: "Run all manual",
            button_title: "Run all manual",
            path: format!("/{}/-/pipelines/{}/stages/{}/play_manual", p.full_path, st.pipeline_id, st.name),
            confirmation: confirm.then(|| "This stage has one or more manual jobs that require confirmation before retrying. Do you want to proceed?".to_string()),
        });
    }
    s
}

/// `Build::Factory`.
pub fn job_status(p: &Project, j: &Job, developer: bool, can_read: bool) -> Status {
    let mut s = base(&j.status, &j.id.to_string(), "Ci::BuildPresenter");
    s.has_details = can_read;
    s.details_path = Some(format!("/{}/-/jobs/{}", p.full_path, j.id));
    let job_path = |a: &str| format!("/{}/-/jobs/{}/{a}", p.full_path, j.id);
    // Group 2: cancel or retry.
    if j.is_cancelable() {
        if developer {
            s.action = Some(Action { icon: "cancel", title: "Cancel", button_title: "Cancel this job", path: job_path("cancel"), confirmation: None });
        }
    } else if j.is_retryable() && developer {
        s.action = Some(Action { icon: "retry", title: "Run again", button_title: "Run this job again", path: job_path("retry"), confirmation: j.manual_confirmation_message().map(str::to_string) });
    }
    // Group 3: why it failed.
    if j.status == "failed" {
        s.tooltip = format!("Failed - ({})", failure_message(j.failure_reason.as_deref().unwrap_or("unknown_failure")));
    }
    // Group 4: allowed to fail, or a manual job to play.
    if j.status == "failed" && j.allow_failure {
        s.label = "failed (allowed to fail)".into();
        s.icon = "status_warning".into();
        s.group = "failed-with-warnings".into();
        s.tooltip = format!("{} (allowed to fail)", s.tooltip);
    } else if j.is_playable() {
        s.label = "manual play action".into();
        s.action = if developer { Some(Action { icon: "play", title: "Run", button_title: "Run job", path: job_path("play"), confirmation: j.manual_confirmation_message().map(str::to_string) }) } else { None };
    }
    // Group 5: a playable job's label says when it can't be played.
    if j.is_playable() && !developer {
        s.label = format!("{} (not allowed)", s.label);
    }
    // Group 6.
    if j.retried {
        s.tooltip = format!("{} (retried)", s.tooltip);
    }
    s
}

/// `External::Factory`, for commit statuses: the description is the label, the target URL the
/// details (for readers), and there are no actions.
fn external_status(c: &rusqlite::Connection, j: &Job, can_read: bool) -> labrat_core::Result<Status> {
    let (description, target_url): (Option<String>, Option<String>) =
        c.query_row("SELECT description, target_url FROM commit_statuses WHERE id = ?1", [j.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut s = base(&j.status, &j.id.to_string(), "GenericCommitStatus");
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        s.label = d;
        s.tooltip = s.label.clone();
    }
    let target_url = target_url.filter(|u| !u.is_empty());
    s.has_details = target_url.is_some() && can_read;
    s.details_path = target_url;
    Ok(s)
}

/// A job's or commit status's detailed status.
fn status_of(c: &rusqlite::Connection, p: &Project, j: &Job, developer: bool, can_read: bool) -> labrat_core::Result<Status> {
    if j.build { Ok(job_status(p, j, developer, can_read)) } else { external_status(c, j, can_read) }
}

/// `Build::Failed::REASONS`.
pub fn failure_message(reason: &str) -> &'static str {
    match reason {
        "script_failure" => "script failure",
        "api_failure" => "API failure",
        "stuck_or_timeout_failure" => "stuck or timeout failure",
        "runner_system_failure" => "runner system failure",
        "missing_dependency_failure" => "missing dependency failure",
        "runner_unsupported" => "unsupported runner",
        "stale_schedule" => "stale schedule",
        "job_execution_timeout" | "job_execution_server_timeout" => "job execution timeout",
        "archived_failure" => "archived failure",
        "unmet_prerequisites" => "unmet prerequisites",
        "scheduler_failure" => "scheduler failure",
        "data_integrity_failure" => "data integrity failure",
        "no_matching_runner" => "no matching runner available",
        "trace_size_exceeded" => "log size limit exceeded",
        "builds_disabled" => "project builds are disabled",
        "user_blocked" => "pipeline user was blocked",
        "ci_quota_exceeded" => "no more compute minutes available",
        _ => "unknown failure",
    }
}

/// `DetailedStatus`, with the GraphQL parent's id.
pub struct StatusObj {
    s: Status,
    parent_id: String,
}

impl StatusObj {
    pub fn new(s: Status, parent_id: impl Into<String>) -> StatusObj {
        StatusObj { s, parent_id: parent_id.into() }
    }
}

impl Object for StatusObj {
    fn typename(&self) -> String {
        "DetailedStatus".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let s = &self.s;
        Ok(match field {
            "id" => Out::val(format!("{}-{}", s.core_id, self.parent_id)),
            "name" => Out::val(s.name.clone()),
            "text" => Out::val(s.text.clone()),
            "label" => Out::val(s.label.clone()),
            "icon" => Out::val(s.icon.clone()),
            "group" => Out::val(s.group.clone()),
            "tooltip" => Out::val(s.tooltip.clone()),
            "hasDetails" => Out::val(s.has_details),
            "detailsPath" => Out::val(s.details_path.clone()),
            "favicon" => Out::val(crate::routes::pipelines::favicon(&s.favicon)),
            "deploymentDetailsPath" => Out::Null,
            "action" => match &s.action {
                Some(a) => Out::obj(JsonObj::new(
                    "StatusAction",
                    json!({
                        "id": format!("{}-{}", s.class, s.core_id),
                        "buttonTitle": a.button_title,
                        "icon": a.icon,
                        "method": "post",
                        "path": a.path,
                        "title": a.title,
                        "confirmationMessage": a.confirmation,
                    }),
                )),
                None => Out::Null,
            },
            _ => return Err(super::resolvers::not_implemented("DetailedStatus", field)),
        })
    }
}

/// `Gitlab::Utils::Job.group_name`: parallel jobs (`test 1/3`, `test [a]`) share a group.
fn group_name(name: &str) -> String {
    let re = regex::Regex::new(r"(?:[\s:]+(?:\[.*\]|\d+[\s:/\\]+\d+)){1,3}\s*$").unwrap();
    re.replace(name, "").trim().to_string()
}

/// `CommitStatus#sortable_name`: names compared with their numbers as numbers.
fn sortable(name: &str) -> Vec<(String, u64)> {
    let re = regex::Regex::new(r"(\d+)|(\D+)").unwrap();
    re.captures_iter(name)
        .map(|c| match c.get(1) {
            Some(n) => (String::new(), n.as_str().parse().unwrap_or(0)),
            None => (c[2].to_string(), 0),
        })
        .collect()
}

const ORDERED: &[&str] = &["failed", "preparing", "pending", "running", "waiting_for_callback", "waiting_for_resource", "manual", "scheduled", "canceling", "canceled", "success", "skipped", "created"];

/// A stage's latest jobs as GitLab orders them (`ordered_latest_statuses`).
fn ordered_jobs(jobs: Vec<Job>) -> Vec<Job> {
    let mut v = jobs;
    v.sort_by_key(|j| (ORDERED.iter().position(|s| *s == j.status).unwrap_or(99), sortable(&j.name)));
    v
}

/// Whether the user may change the job (`update_build`; see `pipelines::can_update`), and
/// whether they may read it.
fn perms(cx: &Ctx, p: &Project, j: &Job) -> FResult<(bool, bool)> {
    let perms = ProjectPerms::load(cx.c, cx.user(), p)?;
    let admin = cx.user().is_some_and(|u| u.admin);
    let read = admin || perms.read_project();
    let update = match (cx.user(), Pipeline::find(cx.c, j.pipeline_id)?) {
        (Some(u), Some(pl)) => pipelines::can_update(cx.c, u, p, &pl)?,
        _ => false,
    };
    Ok((update, read))
}

/// `CiStage`.
pub struct StageObj {
    p: Project,
    st: Stage,
}

impl Object for StageObj {
    fn typename(&self) -> String {
        "CiStage".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let (p, st) = (&self.p, &self.st);
        let jobs: Vec<Job> = Job::statuses_for_pipeline(cx.c, st.pipeline_id, false)?.into_iter().filter(|j| j.stage == st.name).collect();
        Ok(match field {
            "id" => Out::val(format!("gid://gitlab/Ci::Stage/{}", st.id)),
            "name" => Out::val(st.name.clone()),
            "status" => Out::val(st.status.clone()),
            "detailedStatus" => {
                let read = cx.user().is_some_and(|u| u.admin) || ProjectPerms::load(cx.c, cx.user(), p)?.read_project();
                Out::obj(StatusObj::new(stage_status(p, st, &jobs, read), st.id.to_string()))
            }
            "jobs" => {
                let items: Vec<Shared> = ordered_jobs(jobs).into_iter().map(|j| Shared(Rc::new(JobObj::new(p.clone(), j)))).collect();
                Out::obj(Conn::new("CiJobConnection", positions(items), args)?)
            }
            "groups" => {
                let mut sorted = jobs;
                sorted.sort_by_key(|j| sortable(&j.name));
                let mut groups: Vec<(String, Vec<Job>)> = vec![];
                for j in sorted {
                    let g = group_name(&j.name);
                    match groups.iter_mut().find(|(n, _)| *n == g) {
                        Some((_, v)) => v.push(j),
                        None => groups.push((g, vec![j])),
                    }
                }
                let items: Vec<Shared> = groups.into_iter().map(|(name, jobs)| Shared(Rc::new(GroupObj { p: p.clone(), stage_id: st.id, name, jobs }))).collect();
                Out::obj(Conn::new("CiGroupConnection", positions(items), args)?)
            }
            _ => return Err(super::resolvers::not_implemented("CiStage", field)),
        })
    }
}

/// `CiGroup`: jobs of a stage that run as one (parallel jobs), or a single job.
pub struct GroupObj {
    p: Project,
    stage_id: i64,
    name: String,
    jobs: Vec<Job>,
}

impl Object for GroupObj {
    fn typename(&self) -> String {
        "CiGroup".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let id = format!("{}-{}", self.stage_id, self.name);
        Ok(match field {
            "id" => Out::val(format!("gid://gitlab/Ci::Group/{id}")),
            "name" => Out::val(self.name.clone()),
            "size" => Out::val(self.jobs.len()),
            "detailedStatus" => {
                let (developer, read) = perms(cx, &self.p, &self.jobs[0])?;
                let s = if self.jobs.len() == 1 {
                    status_of(cx.c, &self.p, &self.jobs[0], developer, read)?
                } else {
                    let pairs: Vec<(String, bool)> = self.jobs.iter().map(|j| (j.status.clone(), j.allow_failure)).collect();
                    let status = pipelines::composite(&pairs).unwrap_or("created");
                    let mut s = base(status, &id, "Ci::Group");
                    if status == "success" && pipelines::has_warnings(&self.jobs) {
                        success_warning(&mut s);
                    }
                    s
                };
                Out::obj(StatusObj::new(s, id))
            }
            "jobs" => {
                let items: Vec<Shared> = self.jobs.iter().map(|j| Shared(Rc::new(JobObj::new(self.p.clone(), j.clone())))).collect();
                Out::obj(Conn::new("CiJobConnection", positions(items), args)?)
            }
            _ => return Err(super::resolvers::not_implemented("CiGroup", field)),
        })
    }
}

/// `CiJob`.
pub struct JobObj {
    p: Project,
    j: Job,
}

impl JobObj {
    pub fn new(p: Project, j: Job) -> JobObj {
        JobObj { p, j }
    }
}

impl Object for JobObj {
    fn typename(&self) -> String {
        "CiJob".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let (p, j) = (&self.p, &self.j);
        let path = |a: &str| format!("/{}/-/jobs/{}{a}", p.full_path, j.id);
        let pl = Pipeline::find(cx.c, j.pipeline_id)?;
        Ok(match field {
            "id" => Out::val(format!("gid://gitlab/{}/{}", if j.build { "Ci::Build" } else { "GenericCommitStatus" }, j.id)),
            "name" => Out::val(j.name.clone()),
            "status" => Out::val(j.status.to_uppercase()),
            "kind" => Out::val("BUILD"),
            "active" => Out::val(j.is_active()),
            "allowFailure" => Out::val(j.allow_failure),
            "browseArtifactsPath" => Out::val(path("/artifacts/browse")),
            "canPlayJob" => Out::val(j.is_playable() && perms(cx, p, j)?.0),
            "cancelable" => Out::val(j.is_cancelable()),
            "commitPath" => Out::val(format!("/{}/-/commit/{}", p.full_path, j.sha)),
            "coverage" => Out::val(j.coverage),
            "createdAt" => Out::val(gtime(j.created_at)),
            "createdByTag" => Out::val(pl.as_ref().is_some_and(|pl| pl.tag)),
            "duration" => Out::val(j.duration().map(|d| d as i64)),
            "erasedAt" => Out::val(opt_time(j.erased_at)),
            "exitCode" => Out::val(j.exit_code),
            // Only Job Router failures carry a message.
            "failureMessage" => Out::Null,
            "finishedAt" => Out::val(opt_time(j.finished_at)),
            "manualJob" => Out::val(j.build.then(|| j.is_manual())),
            "playPath" => Out::val(path("/play")),
            "playable" => Out::val(j.is_playable()),
            "queuedAt" => Out::val(opt_time(j.queued_at)),
            "queuedDuration" => Out::val(j.queued_duration()),
            "refName" => Out::val(j.ref_.clone()),
            "refPath" => Out::val(j.ref_.as_ref().map(|r| format!("/{}/-/commits/{r}", p.full_path))),
            "retried" => Out::val(j.retried),
            "retryPath" => Out::val(j.is_retryable().then(|| path("/retry"))),
            "retryable" => Out::val(j.is_retryable()),
            "scheduled" => Out::val(false),
            "scheduledAt" => Out::Null,
            "schedulingType" => Out::val("stage"),
            "shortSha" => Out::val(j.sha.chars().take(8).collect::<String>()),
            "source" => Out::val(pl.as_ref().map(|pl| pl.source.clone())),
            "startedAt" => Out::val(opt_time(j.started_at)),
            "stuck" => Out::val(false),
            "tags" => Out::val(j.build.then(|| j.tags.clone())),
            "triggered" => Out::val(false),
            "webPath" => Out::val(path("")),
            "inputs" | "inputsSpec" => Out::List(vec![]),
            "downstreamPipeline" | "runnerManager" => Out::Null,
            "stage" => match Stage::for_pipeline(cx.c, j.pipeline_id)?.into_iter().find(|s| s.name == j.stage) {
                Some(st) => Out::obj(StageObj { p: p.clone(), st }),
                None => Out::Null,
            },
            "pipeline" => match pl {
                Some(pl) => Out::obj(PipelineObj::new(p.clone(), pl)),
                None => Out::Null,
            },
            "project" => match super::resolvers::readable_project(cx, p.clone())? {
                Some((p, perms)) => Out::obj(super::resolvers::ProjectObj { p, perms }),
                None => Out::Null,
            },
            "runner" => match j.runner_id {
                Some(_) => Out::obj(RunnerObj),
                None => Out::Null,
            },
            "detailedStatus" => {
                let (developer, read) = perms(cx, p, j)?;
                Out::obj(StatusObj::new(status_of(cx.c, p, j, developer, read)?, j.id.to_string()))
            }
            "artifacts" => {
                let items: Vec<Shared> = Artifact::for_job(cx.c, j.id)?
                    .into_iter()
                    .rev()
                    .filter(|a| a.file_type != "trace")
                    .map(|a| Shared(Rc::new(ArtifactObj { p: p.clone(), a })))
                    .collect();
                Out::obj(Conn::new("CiJobArtifactConnection", positions(items), args)?)
            }
            "trace" => match j.trace(cx.c)? {
                Some(t) => Out::obj(TraceObj { text: String::from_utf8_lossy(&t).into_owned() }),
                None => Out::Null,
            },
            "userPermissions" => {
                let (developer, read) = perms(cx, p, j)?;
                Out::obj(JsonObj::new("JobPermissions", json!({"cancelBuild": developer, "readBuild": read, "readJobArtifacts": read, "updateBuild": developer})))
            }
            "manualVariables" | "needs" | "previousStageJobs" | "previousStageJobsOrNeeds" => {
                let ty = match field {
                    "manualVariables" => "CiManualVariableConnection",
                    "needs" => "CiBuildNeedConnection",
                    "previousStageJobs" => "CiJobConnection",
                    _ => "JobNeedUnionConnection",
                };
                Out::obj(Conn::new(ty, positions(Vec::<Shared>::new()), args)?)
            }
            _ => return Err(super::resolvers::not_implemented("CiJob", field)),
        })
    }
}

/// `CiJobTrace`: the log's last lines as HTML.
struct TraceObj {
    text: String,
}

impl Object for TraceObj {
    fn typename(&self) -> String {
        "CiJobTrace".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        match field {
            "htmlSummary" => {
                let n = args.int("lastLines").unwrap_or(10).clamp(1, 100) as usize;
                let lines: Vec<&str> = self.text.split_inclusive('\n').collect();
                let tail = lines[lines.len().saturating_sub(n)..].concat();
                Ok(Out::val(ansi_html(&tail)))
            }
            _ => Err(super::resolvers::not_implemented("CiJobTrace", field)),
        }
    }
}

/// `Gitlab::Ci::Ansi2html`: the log as HTML, one `<span>` per run of text with the same
/// style (`term-fg-green`, `term-bold`, ...), newlines as `<br/>`.
pub fn ansi_html(text: &str) -> String {
    const COLORS: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    let re = regex::Regex::new(r"\x1b\[([0-9;]*)([A-Za-z])").unwrap();
    let mut out = String::new();
    let (mut fg, mut bg): (Option<String>, Option<String>) = (None, None);
    let (mut bold, mut italic, mut underline) = (false, false, false);
    let mut buf = String::new();
    let mut classes = String::new();
    let flush = |out: &mut String, buf: &mut String, classes: &str| {
        if buf.is_empty() {
            return;
        }
        if classes.is_empty() {
            out.push_str(&format!("<span>{buf}</span>"));
        } else {
            out.push_str(&format!("<span class=\"{classes}\">{buf}</span>"));
        }
        buf.clear();
    };
    let mut last = 0;
    let push_text = |buf: &mut String, t: &str| {
        for ch in t.chars() {
            match ch {
                '\n' => buf.push_str("<br/>"),
                '\r' => {}
                '&' => buf.push_str("&amp;"),
                '<' => buf.push_str("&lt;"),
                '>' => buf.push_str("&gt;"),
                '"' => buf.push_str("&quot;"),
                '\'' => buf.push_str("&#39;"),
                c => buf.push(c),
            }
        }
    };
    for m in re.captures_iter(text) {
        let whole = m.get(0).unwrap();
        push_text(&mut buf, &text[last..whole.start()]);
        last = whole.end();
        if &m[2] != "m" {
            continue;
        }
        let codes: Vec<u32> = if m[1].is_empty() { vec![0] } else { m[1].split(';').filter_map(|x| x.parse().ok()).collect() };
        for code in codes {
            match code {
                0 => {
                    fg = None;
                    bg = None;
                    bold = false;
                    italic = false;
                    underline = false;
                }
                1 => bold = true,
                3 => italic = true,
                4 => underline = true,
                22 => bold = false,
                23 => italic = false,
                24 => underline = false,
                30..=37 => fg = Some(format!("term-fg-{}", COLORS[(code - 30) as usize])),
                39 => fg = None,
                40..=47 => bg = Some(format!("term-bg-{}", COLORS[(code - 40) as usize])),
                49 => bg = None,
                90..=97 => fg = Some(format!("term-fg-l-{}", COLORS[(code - 90) as usize])),
                100..=107 => bg = Some(format!("term-bg-l-{}", COLORS[(code - 100) as usize])),
                _ => {}
            }
        }
        let mut c: Vec<String> = vec![];
        if let Some(f) = &fg {
            c.push(f.clone());
        }
        if let Some(b) = &bg {
            c.push(b.clone());
        }
        if bold {
            c.push("term-bold".into());
        }
        if italic {
            c.push("term-italic".into());
        }
        if underline {
            c.push("term-underline".into());
        }
        let next = c.join(" ");
        if next != classes {
            flush(&mut out, &mut buf, &classes);
            classes = next;
        }
    }
    push_text(&mut buf, &text[last..]);
    flush(&mut out, &mut buf, &classes);
    out
}

/// labrat's fake runner as `CiRunner`.
struct RunnerObj;

impl Object for RunnerObj {
    fn typename(&self) -> String {
        "CiRunner".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "id" => Out::val(format!("gid://gitlab/Ci::Runner/{}", pipelines::FAKE_RUNNER_ID)),
            "description" => Out::val("labrat-fake-runner"),
            "active" => Out::val(true),
            "paused" => Out::val(false),
            "runnerType" => Out::val("INSTANCE_TYPE"),
            "shortSha" => Out::Null,
            "status" => Out::val("ONLINE"),
            "tagList" => Out::List(vec![]),
            "locked" => Out::val(false),
            "runUntagged" => Out::val(true),
            _ => return Err(super::resolvers::not_implemented("CiRunner", field)),
        })
    }
}

/// `CiJobArtifact`.
struct ArtifactObj {
    p: Project,
    a: Artifact,
}

impl Object for ArtifactObj {
    fn typename(&self) -> String {
        "CiJobArtifact".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let a = &self.a;
        Ok(match field {
            "id" => Out::val(format!("gid://gitlab/Ci::JobArtifact/{}", a.id)),
            "name" => Out::val(a.filename.clone()),
            "fileType" => Out::val(a.file_type.to_uppercase()),
            "size" => Out::val(a.size.to_string()),
            "downloadPath" => Out::val(format!("/{}/-/jobs/{}/artifacts/download?file_type={}", self.p.full_path, a.job_id, a.file_type)),
            "expireAt" => Out::val(opt_time(a.expire_at)),
            "expired" => Out::val(a.expire_at.is_some_and(|t| t < time::now())),
            _ => return Err(super::resolvers::not_implemented("CiJobArtifact", field)),
        })
    }
}

pub use super::resolvers::PipelineObj;

// ---------------------------------------------------------------------------------------------
// Fields on other types.

/// `Pipeline.stages`.
pub fn pipeline_stages(cx: &Ctx, p: &Project, pl: &Pipeline, args: &Args) -> FResult<Out> {
    let items: Vec<Shared> = Stage::for_pipeline(cx.c, pl.id)?.into_iter().map(|st| Shared(Rc::new(StageObj { p: p.clone(), st }))).collect();
    Ok(Out::obj(Conn::new("CiStageConnection", positions(items), args)?))
}

/// `Pipeline.jobs`: newest first, retried attempts included unless `retried` says otherwise;
/// `statuses` and `whenExecuted` filter.
pub fn pipeline_jobs(cx: &Ctx, p: &Project, pl: &Pipeline, args: &Args) -> FResult<Out> {
    let mut jobs = Job::statuses_for_pipeline(cx.c, pl.id, true)?;
    jobs.reverse();
    let retried = args.get("retried").and_then(Value::as_bool);
    // Without `retried`, every attempt is listed.
    if let Some(r) = retried {
        jobs.retain(|j| j.retried == r);
    }
    if let Some(st) = args.get("statuses").and_then(Value::as_array) {
        let want: Vec<String> = st.iter().filter_map(|s| s.as_str().map(|s| s.to_lowercase())).collect();
        jobs.retain(|j| want.contains(&j.status));
    }
    if let Some(w) = args.get("whenExecuted").and_then(Value::as_array) {
        let want: Vec<String> = w.iter().filter_map(|s| s.as_str().map(str::to_string)).collect();
        jobs.retain(|j| want.contains(&j.when));
    }
    let items: Vec<Shared> = jobs.into_iter().map(|j| Shared(Rc::new(JobObj::new(p.clone(), j)))).collect();
    Ok(Out::obj(Conn::new("CiJobConnection", positions(items), args)?))
}

/// `Pipeline.job(id:, name:)`.
pub fn pipeline_job(cx: &Ctx, p: &Project, pl: &Pipeline, args: &Args) -> FResult<Out> {
    let jobs = Job::statuses_for_pipeline(cx.c, pl.id, false)?;
    let id = args.str("id").and_then(|g| parse_gid(&g, &["Ci::Build", "CommitStatus", "Ci::Processable", "GenericCommitStatus"]).ok());
    let name = args.str("name");
    let found = jobs.into_iter().rev().find(|j| id.is_none_or(|i| j.id == i) && name.as_ref().is_none_or(|n| *n == j.name));
    Ok(match found {
        Some(j) => Out::obj(JobObj::new(p.clone(), j)),
        None => Out::Null,
    })
}

/// `Project.jobs`: newest first, with the filters GitLab has. It can be asked once per query.
pub fn project_jobs(cx: &Ctx, p: &Project, args: &Args) -> FResult<Out> {
    {
        let mut calls = cx.calls.borrow_mut();
        let n = calls.entry("Project.jobs").or_insert(0);
        *n += 1;
        if *n > 1 {
            return Err(FieldError::new("\"jobs\" field can be requested only for 1 Project(s) at a time."));
        }
    }
    let mut jobs = Job::for_project(cx.c, p.id)?;
    jobs.reverse();
    // `relevant`: jobs that were never processed (still `created`) aren't listed.
    jobs.retain(|j| j.status != "created");
    if let Some(st) = args.get("statuses").and_then(Value::as_array) {
        let want: Vec<String> = st.iter().filter_map(|s| s.as_str().map(|s| s.to_lowercase())).collect();
        jobs.retain(|j| want.contains(&j.status));
    }
    if args.get("withArtifacts").and_then(Value::as_bool) == Some(true) {
        let mut keep = vec![];
        for j in jobs {
            if !Artifact::for_job(cx.c, j.id)?.is_empty() {
                keep.push(j);
            }
        }
        jobs = keep;
    }
    if let Some(n) = args.str("name") {
        jobs.retain(|j| j.name.contains(n.as_str()));
    }
    if let Some(iid) = args.str("pipelineIid").and_then(|s| s.parse::<i64>().ok()) {
        let pl = Pipeline::list(cx.c, p.id)?.into_iter().find(|pl| pl.iid == iid);
        jobs.retain(|j| pl.as_ref().is_some_and(|pl| pl.id == j.pipeline_id));
    }
    // Keyset cursors on the id.
    let items: Vec<(Value, Shared)> = jobs.into_iter().map(|j| (super::resolvers::id_cursor(j.id), Shared(Rc::new(JobObj::new(p.clone(), j))))).collect();
    Ok(Out::obj(Conn::new("CiJobConnection", items, args)?))
}

/// `Project.job(id:)`.
pub fn project_job(cx: &Ctx, p: &Project, args: &Args) -> FResult<Out> {
    let id = parse_gid(&args.str("id").unwrap_or_default(), &["Ci::Build", "CommitStatus", "Ci::Processable"])?;
    Ok(match Job::find_in(cx.c, p.id, id)? {
        Some(j) => Out::obj(JobObj::new(p.clone(), j)),
        None => Out::Null,
    })
}

/// `Pipeline.jobArtifacts`: every artifact of the pipeline's jobs.
pub fn pipeline_artifacts(cx: &Ctx, p: &Project, jobs: &[Job]) -> FResult<Out> {
    let mut out = vec![];
    for j in jobs {
        for a in Artifact::for_job(cx.c, j.id)? {
            out.push(Out::obj(ArtifactObj { p: p.clone(), a }));
        }
    }
    Ok(Out::List(out))
}

/// `Project.pipelineCounts`.
pub fn pipeline_counts(cx: &Ctx, p: &Project, args: &Args) -> FResult<Out> {
    let mut list = Pipeline::list(cx.c, p.id)?;
    if let Some(r) = args.str("ref") {
        list.retain(|pl| pl.ref_.as_deref() == Some(r.as_str()));
    }
    if let Some(s) = args.str("sha") {
        list.retain(|pl| pl.sha == s);
    }
    if let Some(s) = args.str("source") {
        list.retain(|pl| pl.source.eq_ignore_ascii_case(&s));
    }
    let count = |f: &dyn Fn(&Pipeline) -> bool| list.iter().filter(|pl| f(pl)).count();
    Ok(Out::obj(JsonObj::new(
        "PipelineCounts",
        json!({
            "all": list.len(),
            "finished": count(&|pl| matches!(pl.status.as_str(), "success" | "failed" | "canceled" | "skipped")),
            "pending": count(&|pl| pl.status == "pending"),
            "running": count(&|pl| pl.status == "running"),
        }),
    )))
}

// ---------------------------------------------------------------------------------------------
// Mutations.

/// A CI mutation's payload.
struct CiPayload {
    ty: String,
    client_id: Option<String>,
    errors: Vec<String>,
    job: Option<(Project, Job)>,
    pipeline: Option<(Project, Pipeline)>,
    artifact: Option<(Project, Artifact)>,
    destroyed: Option<i64>,
}

impl Object for CiPayload {
    fn typename(&self) -> String {
        self.ty.clone()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        Ok(match field {
            "clientMutationId" => Out::val(self.client_id.clone()),
            "errors" => Out::List(self.errors.iter().map(|e| Out::val(e.clone())).collect()),
            "job" => match &self.job {
                Some((p, j)) => Out::obj(JobObj::new(p.clone(), j.clone())),
                None => Out::Null,
            },
            "pipeline" => match &self.pipeline {
                Some((p, pl)) => Out::obj(PipelineObj::new(p.clone(), pl.clone())),
                None => Out::Null,
            },
            "artifact" => match &self.artifact {
                Some((p, a)) => Out::obj(ArtifactObj { p: p.clone(), a: a.clone() }),
                None => Out::Null,
            },
            "destroyedArtifactsCount" => Out::val(self.destroyed),
            _ => return Err(super::resolvers::not_implemented(&self.ty, field)),
        })
    }
}

/// What a job mutation needs: to change the job (`update_build`, which a protected ref
/// limits), or only the project's Developer role.
#[derive(PartialEq)]
enum Need {
    Update,
    Developer,
}

fn load_job(cx: &Ctx, gid: &str, need: Need) -> FResult<(Project, Job, User)> {
    let user = cx.user().ok_or_else(unavailable)?.clone();
    let id = parse_gid(gid, &["Ci::Build", "CommitStatus", "Ci::Processable"])?;
    let j = Job::find(cx.c, id)?.filter(|j| j.build).ok_or_else(unavailable)?;
    let p = Project::find(cx.c, j.project_id)?.ok_or_else(unavailable)?;
    let perms = ProjectPerms::load(cx.c, Some(&user), &p)?;
    let allowed = match need {
        Need::Update => Pipeline::find(cx.c, j.pipeline_id)?.map(|pl| pipelines::can_update(cx.c, &user, &p, &pl)).transpose()?.unwrap_or(false),
        Need::Developer => user.admin || perms.developer(),
    };
    if !allowed {
        return Err(unavailable());
    }
    Ok((p, j, user))
}

fn load_pipeline(cx: &Ctx, gid: &str, owner: bool) -> FResult<(Project, Pipeline, User)> {
    let user = cx.user().ok_or_else(unavailable)?.clone();
    let id = parse_gid(gid, &["Ci::Pipeline"])?;
    let pl = Pipeline::find(cx.c, id)?.ok_or_else(unavailable)?;
    let p = Project::find(cx.c, pl.project_id)?.ok_or_else(unavailable)?;
    let perms = ProjectPerms::load(cx.c, Some(&user), &p)?;
    if !(if owner { user.admin || perms.owner() } else { pipelines::can_update(cx.c, &user, &p, &pl)? }) {
        return Err(unavailable());
    }
    Ok((p, pl, user))
}

/// The CI mutations; `None` for other mutations.
pub fn mutation(cx: &Ctx, field: &str, input: &Args) -> FResult<Option<Out>> {
    let config = &cx.req.app.config;
    let mut pl = CiPayload {
        ty: {
            let mut ch = field.chars();
            format!("{}{}Payload", ch.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default(), ch.as_str())
        },
        client_id: input.str("clientMutationId"),
        errors: vec![],
        job: None,
        pipeline: None,
        artifact: None,
        destroyed: None,
    };
    match field {
        "jobPlay" | "jobRetry" | "jobCancel" => {
            let (p, j, user) = load_job(cx, &input.str("id").unwrap_or_default(), Need::Update)?;
            let out = match field {
                // Any job: `job.play` enqueues it, retries it, or leaves it (see `play_job`).
                // A job that can be neither enqueued nor retried keeps the failed transition's
                // error.
                "jobPlay" => {
                    if !matches!(j.status.as_str(), "created" | "skipped" | "manual" | "scheduled") && !j.is_retryable() {
                        pl.errors.push("Status cannot transition via \"enqueue\"".into());
                    }
                    pipelines::play_job(cx.c, config, &j, &user)?
                }
                "jobRetry" => {
                    if !j.is_retryable() {
                        pl.errors.push("Job is not retryable".into());
                        j.clone()
                    } else {
                        pipelines::retry_job(cx.c, config, &j, &user)?
                    }
                }
                // `Ci::BuildCancelService`: only an active or created job; any other is left
                // as it is, with no error.
                _ if j.is_cancelable() => pipelines::cancel_job(cx.c, config, &j)?,
                _ => j.clone(),
            };
            if !(field == "jobRetry" && !pl.errors.is_empty()) {
                pl.job = Some((p, out));
            }
        }
        "pipelineCancel" => {
            let (_, p_, _) = load_pipeline(cx, &input.str("id").unwrap_or_default(), false)?;
            if pipelines::pipeline_cancelable(&p_) {
                pipelines::cancel_pipeline(cx.c, config, p_.id)?;
            } else {
                pl.errors.push("Pipeline is not cancelable".into());
            }
        }
        "pipelineRetry" => {
            let (p, p_, user) = load_pipeline(cx, &input.str("id").unwrap_or_default(), false)?;
            pipelines::retry_pipeline(cx.c, config, p_.id, &user)?;
            pl.pipeline = Some((p, Pipeline::find(cx.c, p_.id)?.expect("exists")));
        }
        "pipelineDestroy" => {
            let (p, p_, _) = load_pipeline(cx, &input.str("id").unwrap_or_default(), true)?;
            let mrs = labrat_core::services::ci::all_merge_requests(cx.c, &p, &p_)?;
            cx.c.execute("DELETE FROM ci_pipelines WHERE id = ?1", [p_.id])?;
            for mr in mrs {
                labrat_core::services::ci::update_head_pipeline(cx.c, &p, &mr)?;
            }
        }
        "jobArtifactsDestroy" => {
            let (p, j, _) = load_job(cx, &input.str("id").unwrap_or_default(), Need::Developer)?;
            // `Ci::JobArtifacts::DeleteService`: the erasable artifacts (all but the log).
            let n = Artifact::for_job(cx.c, j.id)?.iter().filter(|a| a.file_type != "trace").count() as i64;
            cx.c.execute("DELETE FROM ci_job_artifacts WHERE job_id = ?1 AND file_type != 'trace'", [j.id])?;
            pl.destroyed = Some(n);
            pl.job = Some((p, Job::find(cx.c, j.id)?.expect("exists")));
        }
        "artifactDestroy" => {
            let user = cx.user().ok_or_else(unavailable)?.clone();
            let id = parse_gid(&input.str("id").unwrap_or_default(), &["Ci::JobArtifact"])?;
            let job_id: Option<i64> = cx.c.query_row("SELECT job_id FROM ci_job_artifacts WHERE id = ?1", [id], |r| r.get(0)).ok();
            let j = job_id.map(|i| Job::find(cx.c, i)).transpose()?.flatten().ok_or_else(unavailable)?;
            let p = Project::find(cx.c, j.project_id)?.ok_or_else(unavailable)?;
            if !(user.admin || ProjectPerms::load(cx.c, Some(&user), &p)?.admin_project()) {
                return Err(unavailable());
            }
            Artifact::for_job(cx.c, j.id)?.into_iter().find(|a| a.id == id).ok_or_else(unavailable)?;
            cx.c.execute("DELETE FROM ci_job_artifacts WHERE id = ?1", [id])?;
            // The payload's artifact is the destroyed record: null.
            let _ = p;
        }
        _ => return Ok(None),
    }
    Ok(Some(Out::obj(pl)))
}
