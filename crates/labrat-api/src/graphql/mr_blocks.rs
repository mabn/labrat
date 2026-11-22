//! EE merge request dependencies over GraphQL: `MergeRequest.blockingMergeRequests` and the
//! `mergeRequestSetBlockingMergeRequests` mutation. Recorded on EE 18.11.14
//! (`fixtures-ee/mr_dependencies.json`).

use super::exec::{Args, Ctx, FResult, Object, Out};
use super::resolvers::MrObj;
use labrat_core::model::{Namespace, Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::mr_blocks::{self as blocks, CreateError};

fn readable(cx: &Ctx, mr: &MergeRequest) -> FResult<Option<Project>> {
    let Some(p) = Project::find(cx.c, mr.target_project_id)? else { return Ok(None) };
    let perms = ProjectPerms::load(cx.c, cx.user(), &p)?;
    Ok(perms.read_merge_request(&p).then_some(p))
}

/// `BlockingMergeRequests`: the MR's blockers, the ones the user can't read only counted.
pub struct BlockingObj {
    pub mr: MergeRequest,
}

impl Object for BlockingObj {
    fn typename(&self) -> String {
        "BlockingMergeRequests".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, _args: &Args) -> FResult<Out> {
        let mut visible = vec![];
        let mut hidden = 0;
        let all = blocks::blocking(cx.c, self.mr.id)?;
        for b in &all {
            let m = MergeRequest::get(cx.c, b.blocking_merge_request_id)?;
            match readable(cx, &m)? {
                Some(p) => visible.push(Out::obj(MrObj::new(p, m))),
                None => hidden += 1,
            }
        }
        Ok(match field {
            "hiddenCount" => Out::val(hidden),
            "totalCount" => Out::val(all.len()),
            "visibleMergeRequests" => Out::List(visible),
            _ => return Err(super::resolvers::not_implemented("BlockingMergeRequests", field)),
        })
    }
}

/// Resolves a reference as typed in the MR form: `!1`, `project!1`, `group/project!1` or the
/// merge request's URL, relative to `from`.
fn resolve_reference(cx: &Ctx, from: &Project, raw: &str) -> FResult<Option<MergeRequest>> {
    let base = format!("{}/", cx.req.app.config.external_url.trim_end_matches('/'));
    let (path, iid) = if let Some(rest) = raw.strip_prefix(&base) {
        match rest.split_once("/-/merge_requests/") {
            Some((path, iid)) => (path.to_string(), iid.trim_end_matches('/').to_string()),
            None => return Ok(None),
        }
    } else {
        match raw.rsplit_once('!') {
            Some((path, iid)) => (path.to_string(), iid.to_string()),
            None => return Ok(None),
        }
    };
    let Ok(iid) = iid.parse::<i64>() else { return Ok(None) };
    let project = if path.is_empty() {
        Some(from.clone())
    } else if path.contains('/') {
        Project::find_by_full_path(cx.c, &path)?
    } else {
        // A project in the same namespace.
        let ns = Namespace::find(cx.c, from.namespace_id)?;
        match ns {
            Some(n) => Project::find_by_full_path(cx.c, &format!("{}/{path}", n.full_path))?,
            None => None,
        }
    };
    let Some(p) = project else { return Ok(None) };
    let Some(mr) = MergeRequest::find_by_iid(cx.c, p.id, iid)? else { return Ok(None) };
    Ok(readable(cx, &mr)?.map(|_| mr))
}

/// `mergeRequestSetBlockingMergeRequests`: replaces the MR's blockers with the given ones.
/// Blockers no longer listed go first (oldest blocker first), then the new ones in the given
/// order. References that resolve to nothing readable, and blocks that fail validation, end up
/// in one error.
pub fn set_blocking(cx: &Ctx, user: &User, p: &Project, mr: &MergeRequest, refs: &[String]) -> FResult<Vec<String>> {
    let mut wanted: Vec<MergeRequest> = vec![];
    let mut failed: Vec<String> = vec![];
    for r in refs {
        match resolve_reference(cx, p, r)? {
            Some(m) => {
                if !wanted.iter().any(|w| w.id == m.id) {
                    wanted.push(m);
                }
            }
            None => failed.push(r.clone()),
        }
    }
    let mut existing = blocks::blocking(cx.c, mr.id)?;
    existing.sort_by_key(|b| b.blocking_merge_request_id);
    for b in &existing {
        if !wanted.iter().any(|w| w.id == b.blocking_merge_request_id) {
            blocks::destroy(cx.c, user, b)?;
        }
    }
    for m in &wanted {
        if existing.iter().any(|b| b.blocking_merge_request_id == m.id) {
            continue;
        }
        match blocks::create(cx.c, user, m, mr)? {
            Ok(_) => {}
            Err(CreateError::SelfReferential) => failed.push("This block is self-referential.".into()),
            Err(CreateError::Exists) => {}
        }
    }
    if failed.is_empty() {
        Ok(vec![])
    } else {
        Ok(vec![format!("Dependencies failed to save: {}", failed.join(", "))])
    }
}
