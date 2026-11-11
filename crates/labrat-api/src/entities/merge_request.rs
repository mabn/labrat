//! `MergeRequestSimple`, `MergeRequestBasic`, `MergeRequest`, `MergeRequestChanges`,
//! `MergeRequestDiff(Full)`, `MergeRequestReviewer`, `LabelBasic`, `Milestone`.

use crate::entities::repository as repo_ent;
use crate::entities::user::{self, Ctx};
use labrat_core::model::{Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::labels::Label;
use labrat_core::services::merge_requests::{
    self as mrs, MergeRequest, diffs::Diff, merge, mergeability,
};
use labrat_core::{Config, Result, repo, time};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value, json};

/// Rendering options, mirroring the Grape entity options GitLab passes.
#[derive(Default, Clone, Copy)]
pub struct Opts {
    pub with_labels_details: bool,
    pub render_html: bool,
    pub include_first_contribution: bool,
    pub include_rebase_in_progress: bool,
    pub include_diverged_commits_count: bool,
    /// `references.relative` is relative to this group (group listings) or absent project.
    pub relative_to_group: Option<i64>,
    pub global_list: bool,
    /// Rendered without `options[:project]` (to-do targets): no pipeline or build fields.
    pub without_project: bool,
}

pub fn label_basic(l: &Label) -> Value {
    json!({
        "id": l.id,
        "name": l.title,
        "description": l.description,
        "text_color": l.text_color(),
        "description_html": crate::markdown::render(l.description.as_deref().unwrap_or("")),
        "color": l.color,
        "archived": l.archived,
    })
}

pub fn milestone(c: &Connection, config: &Config, id: i64) -> Result<Value> {
    let row: Option<(i64, i64, String, Option<String>, Option<i64>, Option<i64>, String, Option<String>, Option<String>, i64, i64)> = c
        .query_row(
            "SELECT id, iid, title, description, project_id, group_id, state, due_date, start_date, created_at, updated_at FROM milestones WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?)),
        )
        .optional()?;
    let Some((
        id,
        iid,
        title,
        description,
        project_id,
        group_id,
        state,
        due,
        start,
        created,
        updated,
    )) = row
    else {
        return Ok(Value::Null);
    };
    let mut m = Map::new();
    m.insert("id".into(), json!(id));
    m.insert("iid".into(), json!(iid));
    if let Some(pid) = project_id {
        m.insert("project_id".into(), json!(pid));
    }
    if let Some(gid) = group_id {
        m.insert("group_id".into(), json!(gid));
    }
    m.insert("title".into(), json!(title));
    m.insert("description".into(), json!(description));
    m.insert("state".into(), json!(state));
    m.insert("created_at".into(), json!(time::iso(created)));
    m.insert("updated_at".into(), json!(time::iso(updated)));
    m.insert("due_date".into(), json!(due));
    m.insert("start_date".into(), json!(start));
    m.insert(
        "expired".into(),
        json!(due.as_deref().is_some_and(|d| d < time::today().as_str())),
    );
    let web = match (project_id, group_id) {
        (Some(pid), _) => {
            let path: String = c.query_row(
                "SELECT path FROM routes WHERE source_type = 'Project' AND source_id = ?1",
                [pid],
                |r| r.get(0),
            )?;
            config.url(&format!("/{path}/-/milestones/{iid}"))
        }
        (_, Some(gid)) => {
            let path: String = c.query_row(
                "SELECT path FROM routes WHERE source_type = 'Namespace' AND source_id = ?1",
                [gid],
                |r| r.get(0),
            )?;
            config.url(&format!("/groups/{path}/-/milestones/{iid}"))
        }
        _ => String::new(),
    };
    m.insert("web_url".into(), json!(web));
    Ok(Value::Object(m))
}

fn user_value(c: &Connection, ctx: &Ctx, id: Option<i64>) -> Result<Value> {
    match id {
        Some(id) => match User::find(c, id)? {
            Some(u) => Ok(user::basic_value(&u, ctx)),
            None => Ok(Value::Null),
        },
        None => Ok(Value::Null),
    }
}

/// `task_completion_status`: markdown checkboxes in the description.
pub fn task_status(desc: Option<&str>) -> Value {
    let mut count = 0;
    let mut done = 0;
    for line in desc.unwrap_or("").lines() {
        let t = line.trim_start();
        let rest = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
            .or_else(|| {
                let digits = t.chars().take_while(|ch| ch.is_ascii_digit()).count();
                if digits > 0 {
                    t[digits..]
                        .strip_prefix(". ")
                        .or_else(|| t[digits..].strip_prefix(") "))
                } else {
                    None
                }
            });
        if let Some(r) = rest {
            if r.starts_with("[ ]") {
                count += 1;
            } else if r.starts_with("[x]") || r.starts_with("[X]") {
                count += 1;
                done += 1;
            }
        }
    }
    json!({"count": count, "completed_count": done})
}

fn human_duration(secs: i64) -> Value {
    labrat_core::chronic::human_value(secs)
}

/// `MergeRequestSimple`.
pub fn simple(config: &Config, p: &Project, mr: &MergeRequest) -> Value {
    json!({
        "id": mr.id,
        "iid": mr.iid,
        "project_id": mr.target_project_id,
        "title": mr.title,
        "description": mr.description,
        "state": mr.state,
        "created_at": time::iso(mr.created_at),
        "updated_at": time::iso(mr.updated_at),
        "web_url": mr.web_url(config, p),
    })
}

/// `MergeRequestBasic`.
pub fn basic(
    c: &Connection,
    config: &Config,
    current: Option<&User>,
    p: &Project,
    mr: &MergeRequest,
    o: &Opts,
) -> Result<Map<String, Value>> {
    let ctx = Ctx {
        base_url: &config.external_url,
        current_user: current,
    };
    let mut m = Map::new();
    m.insert("id".into(), json!(mr.id));
    m.insert("iid".into(), json!(mr.iid));
    m.insert("project_id".into(), json!(mr.target_project_id));
    m.insert("title".into(), json!(mr.title));
    m.insert("description".into(), json!(mr.description));
    m.insert("state".into(), json!(mr.state));
    m.insert("created_at".into(), json!(time::iso(mr.created_at)));
    m.insert("updated_at".into(), json!(time::iso(mr.updated_at)));
    let merged_by = user_value(c, &ctx, mr.merged_by_id)?;
    m.insert("merged_by".into(), merged_by.clone());
    m.insert(
        "merge_user".into(),
        if merged_by.is_null() {
            user_value(c, &ctx, mr.merge_user_id)?
        } else {
            merged_by
        },
    );
    m.insert("merged_at".into(), json!(mr.merged_at.map(time::iso)));
    m.insert(
        "closed_by".into(),
        user_value(c, &ctx, mr.latest_closed_by_id)?,
    );
    m.insert(
        "closed_at".into(),
        json!(mr.latest_closed_at.map(time::iso)),
    );
    if o.render_html {
        m.insert(
            "title_html".into(),
            json!(crate::markdown::render_inline(&mr.title)),
        );
        m.insert(
            "description_html".into(),
            json!(crate::markdown::render(
                mr.description.as_deref().unwrap_or("")
            )),
        );
    }
    m.insert("target_branch".into(), json!(mr.target_branch));
    m.insert("source_branch".into(), json!(mr.source_branch));
    let user_notes: i64 = c.query_row(
        "SELECT count(*) FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND system = 0",
        [mr.id],
        |r| r.get(0),
    )?;
    m.insert("user_notes_count".into(), json!(user_notes));
    let votes = |name: &str| -> Result<i64> {
        Ok(c.query_row(
            "SELECT count(*) FROM award_emoji WHERE awardable_type = 'MergeRequest' AND awardable_id = ?1 AND name = ?2",
            rusqlite::params![mr.id, name],
            |r| r.get(0),
        )?)
    };
    m.insert("upvotes".into(), json!(votes("thumbsup")?));
    m.insert("downvotes".into(), json!(votes("thumbsdown")?));
    m.insert("author".into(), user_value(c, &ctx, Some(mr.author_id))?);
    let assignees: Vec<Value> = mr
        .assignee_ids(c)?
        .into_iter()
        .map(|id| user_value(c, &ctx, Some(id)))
        .collect::<Result<_>>()?;
    m.insert("assignee".into(), Value::Null);
    let first = assignees.first().cloned().unwrap_or(Value::Null);
    m.remove("assignee");
    m.insert("assignees".into(), Value::Array(assignees));
    m.insert("assignee".into(), first);
    let reviewers: Vec<Value> = mr
        .reviewer_ids(c)?
        .into_iter()
        .map(|id| user_value(c, &ctx, Some(id)))
        .collect::<Result<_>>()?;
    m.insert("reviewers".into(), Value::Array(reviewers));
    m.insert("source_project_id".into(), json!(mr.source_project_id));
    m.insert("target_project_id".into(), json!(mr.target_project_id));
    let labels = mr.labels(c)?;
    m.insert(
        "labels".into(),
        if o.with_labels_details {
            Value::Array(labels.iter().map(label_basic).collect())
        } else {
            json!(labels.iter().map(|l| l.title.clone()).collect::<Vec<_>>())
        },
    );
    let draft = mr.is_draft();
    m.insert("draft".into(), json!(draft));
    m.insert("imported".into(), json!(false));
    m.insert("imported_from".into(), json!("none"));
    m.insert("work_in_progress".into(), json!(draft));
    m.insert(
        "milestone".into(),
        match mr.milestone_id {
            Some(id) => milestone(c, config, id)?,
            None => Value::Null,
        },
    );
    m.insert(
        "merge_when_pipeline_succeeds".into(),
        json!(mr.merge_when_pipeline_succeeds),
    );
    m.insert(
        "merge_status".into(),
        json!(mergeability::public_status(&mr.merge_status)),
    );
    m.insert(
        "detailed_merge_status".into(),
        json!(mergeability::detailed_status(c, p, mr)?),
    );
    m.insert(
        "merge_after".into(),
        json!(labrat_core::services::merge_requests::merge_after(c, mr)?.map(labrat_core::time::iso)),
    );
    m.insert("sha".into(), json!(mr.diff_head_sha(c, p)?));
    m.insert("merge_commit_sha".into(), json!(mr.merge_commit_sha));
    m.insert("squash_commit_sha".into(), json!(mr.squash_commit_sha));
    m.insert("discussion_locked".into(), json!(mr.discussion_locked));
    m.insert(
        "should_remove_source_branch".into(),
        json!(mr.should_remove_source_branch()),
    );
    m.insert(
        "force_remove_source_branch".into(),
        json!(mr.force_remove_source_branch()),
    );
    m.insert("prepared_at".into(), json!(mr.prepared_at.map(time::iso)));
    let short = format!("!{}", mr.iid);
    let full = format!("{}!{}", p.full_path, mr.iid);
    // The global list (`/merge_requests`) uses short references too (recorded on 18.11).
    let relative = if let Some(gid) = o.relative_to_group {
        // Relative to a group: drop the group's path prefix.
        let gpath: String = c.query_row(
            "SELECT path FROM routes WHERE source_type = 'Namespace' AND source_id = ?1",
            [gid],
            |r| r.get(0),
        )?;
        match full.strip_prefix(&format!("{gpath}/")) {
            Some(rest) => rest.to_string(),
            None => full.clone(),
        }
    } else {
        short.clone()
    };
    m.insert(
        "reference".into(),
        json!(short.clone()),
    );
    m.insert(
        "references".into(),
        json!({"short": short, "relative": relative, "full": full}),
    );
    m.insert("web_url".into(), json!(mr.web_url(config, p)));
    m.insert(
        "time_stats".into(),
        json!({
            "time_estimate": mr.time_estimate,
            "total_time_spent": mr.total_time_spent,
            "human_time_estimate": human_duration(mr.time_estimate),
            "human_total_time_spent": human_duration(mr.total_time_spent),
        }),
    );
    m.insert("squash".into(), json!(mr.squash));
    m.insert(
        "squash_on_merge".into(),
        json!(merge::squash_on_merge(p, mr)),
    );
    m.insert(
        "task_completion_status".into(),
        task_status(mr.description.as_deref()),
    );
    m.insert(
        "has_conflicts".into(),
        json!(mr.merge_status == "cannot_be_merged"),
    );
    m.insert(
        "blocking_discussions_resolved".into(),
        json!(mergeability::blocking_discussions_resolved(c, p, mr)?),
    );
    Ok(m)
}

/// `Entities::MergeRequest` (single MR).
pub fn single(
    c: &Connection,
    config: &Config,
    current: Option<&User>,
    p: &Project,
    mr: &MergeRequest,
    o: &Opts,
) -> Result<Value> {
    let mut m = basic(c, config, current, p, mr, o)?;
    // `subscribed?(user, options[:project])`: without a project, subscriptions (stored per
    // project) aren't found and the participant fallback doesn't apply.
    let subscribed = match current {
        Some(u) if !o.without_project => labrat_core::services::subscriptions::subscribed_to_mr(c, u.id, mr)?,
        _ => false,
    };
    m.insert("subscribed".into(), json!(subscribed));
    let diff = mr.latest_diff(c)?;
    m.insert(
        "changes_count".into(),
        json!(diff.as_ref().and_then(|d| d.real_size.clone())),
    );
    let perms = ProjectPerms::load(c, current, p)?;
    let (metrics_pipeline, build_started, build_finished) = labrat_core::services::ci::metrics(c, mr)?;
    if !o.without_project && perms.feature(p, "builds", labrat_core::model::access::GUEST) {
        m.insert("latest_build_started_at".into(), json!(build_started.map(labrat_core::time::iso)));
        m.insert("latest_build_finished_at".into(), json!(build_finished.map(labrat_core::time::iso)));
        m.insert("first_deployed_to_production_at".into(), Value::Null);
        m.insert(
            "pipeline".into(),
            match &metrics_pipeline {
                Some(pl) => Value::Object(crate::routes::pipelines::pipeline_basic(config, p, pl)),
                None => Value::Null,
            },
        );
    }
    if !o.without_project && (perms.reporter() || (p.visibility_level > 0 && perms.read_project())) {
        let head = match labrat_core::services::ci::head_pipeline(c, mr)? {
            Some(pl) => Value::Object(crate::routes::pipelines::pipeline_detailed_for(c, config, current, p, &pl, current.is_some())?),
            None => Value::Null,
        };
        m.insert("head_pipeline".into(), head);
    }
    m.insert(
        "diff_refs".into(),
        match &diff {
            Some(d) => json!({"base_sha": d.base_commit_sha, "head_sha": d.head_commit_sha, "start_sha": d.start_commit_sha}),
            None => Value::Null,
        },
    );
    m.insert("merge_error".into(), json!(mr.merge_error));
    if o.include_rebase_in_progress {
        m.insert("rebase_in_progress".into(), json!(mr.rebase_in_progress));
    }
    if o.include_diverged_commits_count {
        let r = repo::repo(c, p);
        let n = match (r.branch(&mr.source_branch)?, r.branch(&mr.target_branch)?) {
            (Some(s), Some(t)) => labrat_git::walk::rev_list(&r, &[t], &[s])?.len(),
            _ => 0,
        };
        m.insert("diverged_commits_count".into(), json!(n));
    }
    if o.include_first_contribution {
        let first = if mr.state == "merged" {
            mr.first_contribution
        } else {
            let n: i64 = c.query_row(
                "SELECT count(*) FROM merge_requests WHERE target_project_id = ?1 AND author_id = ?2 AND state = 'merged'",
                rusqlite::params![p.id, mr.author_id],
                |r| r.get(0),
            )?;
            n == 0
        };
        m.insert("first_contribution".into(), json!(first));
    }
    let can_merge = match current {
        Some(u) => mrs::can_be_merged_by(c, p, mr, u)?,
        None => false,
    };
    m.insert("user".into(), json!({"can_merge": can_merge}));
    Ok(Value::Object(m))
}

pub fn diff_version(d: &Diff) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(d.id));
    m.insert("head_commit_sha".into(), json!(d.head_commit_sha));
    m.insert("base_commit_sha".into(), json!(d.base_commit_sha));
    m.insert("start_commit_sha".into(), json!(d.start_commit_sha));
    m.insert("created_at".into(), json!(time::iso(d.created_at)));
    m.insert("merge_request_id".into(), json!(d.merge_request_id));
    m.insert("state".into(), json!(d.state));
    m.insert("real_size".into(), json!(d.real_size));
    m.insert("patch_id_sha".into(), json!(d.patch_id_sha));
    m
}

pub fn diff_version_full(c: &Connection, config: &Config, p: &Project, d: &Diff) -> Result<Value> {
    let mut m = diff_version(d);
    let r = repo::repo(c, p);
    let commits: Vec<Value> = d
        .commits(&r)?
        .iter()
        .map(|o| {
            r.commit(o)
                .map(|cm| Value::Object(repo_ent::commit(config, p, o, &cm)))
        })
        .collect::<std::result::Result<_, _>>()?;
    m.insert("commits".into(), Value::Array(commits));
    let mut changes = d.changes(&r)?;
    labrat_git::diff::sort_like_gitlab(&mut changes);
    let diffs = repo_ent::mr_diff_list(&r, d.oids().1, &changes, 0, changes.len(), repo_ent::MrDiffView::Expanded)?;
    m.insert("diffs".into(), Value::Array(diffs));
    Ok(Value::Object(m))
}

pub fn reviewer_state_name(s: i64) -> &'static str {
    match s {
        1 => "reviewed",
        2 => "requested_changes",
        3 => "approved",
        4 => "unapproved",
        5 => "review_started",
        _ => "unreviewed",
    }
}
