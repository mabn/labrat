//! Arguments of `Project.mergeRequests` / `Group.mergeRequests` (`ResolvesMergeRequests`).
//!
//! Like GitLab, these go through the same finder as the REST lists: arguments the REST filter
//! knows are translated to its parameters, and the GraphQL-only ones are applied afterwards.

use super::exec::{Args, Ctx, FResult, FieldError};
use crate::params::Params;
use labrat_core::model::{Project, User};
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::reviews::state;
use labrat_core::services::subscriptions;
use serde_json::{Map, Value, json};
use std::cmp::Ordering;

type Rows = Vec<(Project, MergeRequest)>;

fn strs(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}

fn given(args: &Args, k: &str) -> bool {
    args.get(k).is_some_and(|v| !v.is_null())
}

fn exclusive(args: &Args, names: &[&str]) -> FResult<()> {
    if names.iter().filter(|n| given(args, n)).count() > 1 {
        return Err(FieldError::new(format!("Only one of [{}] arguments is allowed at the same time.", names.join(", "))));
    }
    Ok(())
}

fn wildcard(v: &str) -> String {
    match v {
        "NONE" => "None",
        "ANY" => "Any",
        "STARTED" => "Started",
        "UPCOMING" => "Upcoming",
        o => o,
    }
    .to_string()
}

fn review_state(v: &str) -> Option<i64> {
    state::parse(&v.to_lowercase())
}

/// Arguments shared by the top level and `not`, as REST parameters.
fn shared_params(args: &Args, out: &mut Map<String, Value>) {
    for (from, to) in [("authorUsername", "author_username"), ("reviewerUsername", "reviewer_username"), ("milestoneTitle", "milestone"), ("myReactionEmoji", "my_reaction_emoji")] {
        if let Some(v) = args.str(from) {
            out.insert(to.into(), json!(v));
        }
    }
    let mut labels = strs(args.get("labels")).unwrap_or_default();
    labels.extend(strs(args.get("labelName")).unwrap_or_default());
    if !labels.is_empty() {
        out.insert("labels".into(), json!(labels.join(",")));
    }
    // Several usernames mean "assigned to all of them"; that is applied after the REST filter.
    if let Some(a) = strs(args.get("assigneeUsernames")).filter(|a| a.len() == 1).or(args.str("assigneeUsername").map(|x| vec![x])) {
        out.insert("assignee_username".into(), json!(a));
    }
}

/// `(order_by, sort)` for the REST filter, plus whether the sort is handled here.
fn sort_of(args: &Args) -> (String, bool, Option<&'static str>) {
    let s = args.str("sort").unwrap_or("created_desc".into()).to_lowercase();
    let desc = !s.ends_with("_asc");
    let key = s.trim_end_matches("_asc").trim_end_matches("_desc");
    match key {
        "updated" => ("updated_at".into(), desc, None),
        "title" => ("title".into(), desc, None),
        "popularity" => ("popularity".into(), desc, None),
        "priority" => ("priority".into(), desc, None),
        "label_priority" => ("label_priority".into(), desc, None),
        "milestone_due" => ("milestone_due".into(), desc, None),
        "merged_at" => ("created_at".into(), desc, Some("merged_at")),
        "closed_at" => ("created_at".into(), desc, Some("closed_at")),
        _ => ("created_at".into(), desc, None),
    }
}

fn reviewer_states(cx: &Ctx, mr: &MergeRequest) -> FResult<Vec<(i64, i64)>> {
    let mut st = cx.c.prepare("SELECT user_id, state FROM merge_request_reviewers WHERE merge_request_id = ?1")?;
    let rows = st.query_map([mr.id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn user_id(cx: &Ctx, name: &str) -> FResult<i64> {
    Ok(User::find_by_username(cx.c, name)?.map(|u| u.id).unwrap_or(-1))
}

fn time_arg(args: &Args, k: &str) -> Option<i64> {
    args.str(k).and_then(|v| labrat_core::time::parse_time(&v))
}

/// Filters and sorts `rows`. `None` when an empty list argument rules out any result.
pub fn apply(cx: &Ctx, rows: Rows, args: &Args) -> FResult<Rows> {
    exclusive(args, &["assigneeUsernames", "assigneeUsername", "assigneeWildcardId"])?;
    exclusive(args, &["reviewerUsername", "reviewerWildcardId"])?;
    exclusive(args, &["milestoneTitle", "milestoneWildcardId"])?;
    let (closed_after, closed_before) = (time_arg(args, "closedAfter"), time_arg(args, "closedBefore"));
    if given(args, "closedAfter") || given(args, "closedBefore") {
        let (Some(a), Some(b)) = (closed_after, closed_before) else {
            return Err(FieldError::new("You must provide both closedAfter and closedBefore."));
        };
        if b < a {
            return Err(FieldError::new("closedBefore must be on or after closedAfter."));
        }
        if b - a > 2 * 365 * 86_400_000 + 86_400_000 {
            return Err(FieldError::new("Time between closedAfter and closedBefore must be 2 years or less."));
        }
    }
    // `some_argument_is_empty?`
    if args.0.values().any(|v| v.as_array().is_some_and(|a| a.is_empty())) {
        return Ok(vec![]);
    }

    let mut p = Map::new();
    shared_params(args, &mut p);
    if let Some(v) = args.str("state") {
        p.insert("state".into(), json!(v));
    }
    if let Some(v) = strs(args.get("iids")) {
        p.insert("iids".into(), json!(v));
    }
    if let Some(v) = args.str("assigneeWildcardId") {
        p.insert("assignee_id".into(), json!(wildcard(&v)));
    }
    if let Some(v) = args.str("reviewerWildcardId") {
        p.insert("reviewer_id".into(), json!(wildcard(&v)));
    }
    if let Some(v) = args.str("milestoneWildcardId") {
        p.insert("milestone".into(), json!(wildcard(&v)));
    }
    if let Some(v) = args.str("mergedBy") {
        p.insert("merge_user_username".into(), json!(v));
    }
    for (from, to) in [("createdAfter", "created_after"), ("createdBefore", "created_before"), ("updatedAfter", "updated_after"), ("updatedBefore", "updated_before")] {
        if let Some(v) = args.str(from) {
            p.insert(to.into(), json!(v));
        }
    }
    if let Some(v) = args.str("search") {
        p.insert("search".into(), json!(v));
        if let Some(i) = strs(args.get("in")) {
            p.insert("in".into(), json!(i.iter().map(|x| x.to_lowercase()).collect::<Vec<_>>().join(",")));
        }
    }
    if let Some(d) = args.bool("draft") {
        p.insert("wip".into(), json!(if d { "yes" } else { "no" }));
    }
    let not = args.get("not").and_then(Value::as_object).cloned().map(Args);
    if let Some(n) = &not {
        let mut m = Map::new();
        shared_params(n, &mut m);
        p.insert("not".into(), Value::Object(m));
    }
    let (order_by, desc, own_sort) = sort_of(args);
    p.insert("order_by".into(), json!(order_by));
    p.insert("sort".into(), json!(if desc { "desc" } else { "asc" }));
    let req = cx.req.with_params(Params { map: p });
    let mut rows = crate::routes::merge_requests::filter(cx.c, &req, rows, None)?;
    // `MergeRequests::ByApprovalsFinder` (at most 5 names): `None`, `Any`, or approved by all.
    if let Some(names) = strs(args.get("approvedBy")) {
        let mut names: Vec<String> = names.into_iter().fold(vec![], |mut v, n| {
            if !v.contains(&n) {
                v.push(n);
            }
            v
        });
        names.truncate(5);
        if !names.is_empty() {
            let special = |w: &str| names.iter().any(|n| n.eq_ignore_ascii_case(w));
            let mut ids = vec![];
            for n in &names {
                ids.push(labrat_core::model::User::find_by_username(cx.c, n)?.map(|u| u.id).unwrap_or(-1));
            }
            let mut kept = vec![];
            for (proj, mr) in rows {
                let approvers: Vec<i64> = labrat_core::services::reviews::approvals(cx.c, &mr)?.into_iter().map(|(u, _)| u).collect();
                let ok = if special("none") {
                    approvers.is_empty()
                } else if special("any") {
                    !approvers.is_empty()
                } else {
                    ids.iter().all(|i| approvers.contains(i))
                };
                if ok {
                    kept.push((proj, mr));
                }
            }
            rows = kept;
        }
    }

    // GraphQL-only arguments.
    let me = cx.req.user.clone();
    let branches = |k: &str, a: &Args| strs(a.get(k)).filter(|v| !v.is_empty());
    let (src, tgt) = (branches("sourceBranches", args), branches("targetBranches", args));
    let states: Option<Vec<i64>> = match (args.str("reviewState"), strs(args.get("reviewStates"))) {
        (Some(s), _) => Some(review_state(&s).into_iter().collect()),
        (None, Some(v)) => Some(v.iter().filter_map(|s| review_state(s)).collect()),
        _ => None,
    };
    let (merged_after, merged_before) = (time_arg(args, "mergedAfter"), time_arg(args, "mergedBefore"));
    let subscribed = args.str("subscribed");
    // `ignoredReviewerUsername`: that reviewer's state doesn't count.
    let ignored = match args.str("ignoredReviewerUsername") {
        Some(n) => Some(user_id(cx, &n)?),
        None => None,
    };
    let or = args.get("or").and_then(Value::as_object).cloned().map(Args);
    let mut out = vec![];
    for (proj, mr) in rows {
        if src.as_ref().is_some_and(|b| !b.contains(&mr.source_branch)) || tgt.as_ref().is_some_and(|b| !b.contains(&mr.target_branch)) {
            continue;
        }
        if let Some(names) = strs(args.get("assigneeUsernames")).filter(|a| a.len() > 1) {
            let ids = mr.assignee_ids(cx.c)?;
            let mut all = true;
            for name in names {
                if !ids.contains(&user_id(cx, &name)?) {
                    all = false;
                }
            }
            if !all {
                continue;
            }
        }
        if let Some(n) = &not
            && let Some(names) = strs(n.get("assigneeUsernames")).filter(|a| a.len() > 1)
        {
            let ids = mr.assignee_ids(cx.c)?;
            let mut any = false;
            for name in names {
                if ids.contains(&user_id(cx, &name)?) {
                    any = true;
                }
            }
            if any {
                continue;
            }
        }
        let rs: Vec<(i64, i64)> = reviewer_states(cx, &mr)?.into_iter().filter(|(u, _)| Some(*u) != ignored).collect();
        if let Some(want) = &states
            && !rs.iter().any(|(_, s)| want.contains(s)) {
                continue;
            }
        if merged_after.is_some_and(|t| mr.merged_at.is_none_or(|m| m < t)) || merged_before.is_some_and(|t| mr.merged_at.is_none_or(|m| m > t)) {
            continue;
        }
        if let (Some(a), Some(b)) = (closed_after, closed_before)
            && (mr.state != "closed" || mr.latest_closed_at.is_none_or(|t| t < a || t > b)) {
                continue;
            }
        if let (Some(sub), Some(u)) = (&subscribed, &me) {
            let explicit = subscriptions::explicit(cx.c, u.id, "MergeRequest", mr.id)?;
            let ok = match sub.as_str() {
                "EXPLICITLY_SUBSCRIBED" => explicit == Some(true),
                "EXPLICITLY_UNSUBSCRIBED" => explicit == Some(false),
                _ => true,
            };
            if !ok {
                continue;
            }
        }
        if let Some(n) = &not {
            if branches("sourceBranches", n).is_some_and(|b| b.contains(&mr.source_branch)) || branches("targetBranches", n).is_some_and(|b| b.contains(&mr.target_branch)) {
                continue;
            }
            // `no_review_states`: some reviewer exists and none is in those states.
            if let Some(v) = strs(n.get("reviewStates")) {
                let want: Vec<i64> = v.iter().filter_map(|s| review_state(s)).collect();
                if rs.is_empty() || rs.iter().any(|(_, s)| want.contains(s)) {
                    continue;
                }
            }
            if let Some(names) = strs(n.get("approvedBy")) {
                let approvers: Vec<i64> = labrat_core::services::reviews::approvals(cx.c, &mr)?.into_iter().map(|(u, _)| u).collect();
                let mut hit = false;
                for name in names {
                    if approvers.contains(&user_id(cx, &name)?) {
                        hit = true;
                    }
                }
                if hit {
                    continue;
                }
            }
        }
        // `or`: assignees alone; the reviewer keys only in GitLab's combinations with
        // `reviewerWildcard: NONE` ("no reviewers, or ..."). Other combinations filter nothing.
        if let Some(o) = &or {
            if let Some(names) = strs(o.get("assigneeUsernames")) {
                let ids = mr.assignee_ids(cx.c)?;
                let mut any = false;
                for name in names {
                    if ids.contains(&user_id(cx, &name)?) {
                        any = true;
                    }
                }
                if !any {
                    continue;
                }
            }
            if o.str("reviewerWildcard").as_deref() == Some("NONE") {
                let only = o.str("onlyReviewerUsername");
                let states = strs(o.get("reviewStates"));
                if only.is_some() || states.is_some() {
                    let mut ok = rs.is_empty();
                    if let Some(name) = &only {
                        let id = user_id(cx, name)?;
                        ok |= rs.len() == 1 && rs[0].0 == id;
                    }
                    if let Some(v) = &states {
                        let want: Vec<i64> = v.iter().filter_map(|s| review_state(s)).collect();
                        ok |= rs.iter().any(|(_, s)| want.contains(s));
                    }
                    if !ok {
                        continue;
                    }
                }
            }
        }
        out.push((proj, mr));
    }
    if let Some(col) = own_sort {
        // `merged_at` / `closed_at` sorts: NULLS LAST either way, ties id-descending.
        let key = |m: &MergeRequest| if col == "merged_at" { m.merged_at } else if m.state == "closed" { m.latest_closed_at } else { None };
        out.sort_by(|(_, a), (_, b)| {
            let o = match (key(a), key(b)) {
                (Some(x), Some(y)) => if desc { y.cmp(&x) } else { x.cmp(&y) },
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            o.then(b.id.cmp(&a.id))
        });
    }
    Ok(out)
}
