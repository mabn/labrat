//! `lib/api/{branches,tags,commits,files,repositories,protected_branches}.rb`.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::entities::repository as ent;
use crate::find;
use crate::pagination::{Page, paginate};
use axum::Router;
use axum::routing::{get, post, put};
use base64::Engine;
use labrat_core::model::Project;
use labrat_core::perms::ProjectPerms;
use labrat_core::repo::{self, FileAction, NewCommit};
use labrat_core::{App, Error, time};
use labrat_git::diff;
use labrat_git::{Kind, Oid, Repo, Signature, Tag, walk};
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route(
            "/projects/{id}/repository/branches",
            get(list_branches).post(create_branch),
        )
        .route(
            "/projects/{id}/repository/branches/{branch}",
            get(show_branch).delete(delete_branch).head(head_branch),
        )
        .route(
            "/projects/{id}/repository/branches/{branch}/protect",
            put(legacy_protect),
        )
        .route(
            "/projects/{id}/repository/branches/{branch}/unprotect",
            put(legacy_unprotect),
        )
        .route(
            "/projects/{id}/repository/merged_branches",
            axum::routing::delete(delete_merged),
        )
        .route(
            "/projects/{id}/repository/tags",
            get(list_tags).post(create_tag),
        )
        .route(
            "/projects/{id}/repository/tags/{tag}",
            get(show_tag).delete(delete_tag),
        )
        .route(
            "/projects/{id}/repository/commits",
            get(list_commits).post(create_commit),
        )
        .route("/projects/{id}/repository/commits/{sha}", get(show_commit))
        .route("/projects/{id}/repository/commits/{sha}/merge_requests", get(commit_merge_requests))
        .route("/projects/{id}/repository/commits/{sha}/sequence", get(commit_sequence))
        .route("/projects/{id}/repository/commits/{sha}/signature", get(commit_signature))
        .route(
            "/projects/{id}/repository/commits/{sha}/diff",
            get(commit_diff),
        )
        .route(
            "/projects/{id}/repository/commits/{sha}/refs",
            get(commit_refs),
        )
        .route(
            "/projects/{id}/repository/commits/{sha}/cherry_pick",
            post(cherry_pick),
        )
        .route(
            "/projects/{id}/repository/commits/{sha}/revert",
            post(revert),
        )
        .route(
            "/projects/{id}/repository/files/{file_path}",
            get(get_file)
                .head(head_file)
                .post(create_file)
                .put(update_file)
                .delete(delete_file),
        )
        .route(
            "/projects/{id}/repository/files/{file_path}/raw",
            get(raw_file),
        )
        .route("/projects/{id}/repository/tree", get(tree))
        .route("/projects/{id}/repository/blobs/{sha}", get(blob))
        .route("/projects/{id}/repository/blobs/{sha}/raw", get(blob_raw))
        .route("/projects/{id}/repository/compare", get(compare))
        .route("/projects/{id}/repository/merge_base", get(merge_base))
        .route(
            "/projects/{id}/protected_branches",
            get(list_protected).post(create_protected),
        )
        .route(
            "/projects/{id}/protected_branches/{name}",
            get(show_protected)
                .delete(delete_protected)
                .patch(update_protected),
        )
}

/// A project whose code the user can read (`authorize_read_code!` gives 403).
pub(crate) fn code_project(c: &Connection, req: &Req) -> Result<(Project, ProjectPerms), ApiError> {
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if p.feature_level("repository") == labrat_core::model::feature::DISABLED {
        return Err(Error::not_found("Repository").into());
    }
    if !perms.read_code(&p) {
        return Err(Error::forbidden().into());
    }
    Ok((p, perms))
}

fn push_project(c: &Connection, req: &Req) -> Result<(Project, ProjectPerms), ApiError> {
    req.user()?;
    let (p, perms) = code_project(c, req)?;
    if !perms.developer() {
        return Err(Error::forbidden().into());
    }
    Ok((p, perms))
}

fn ensure_push_to_branch(
    c: &Connection,
    req: &Req,
    p: &Project,
    branch: &str,
) -> Result<(), ApiError> {
    let u = req.user()?;
    if !repo::can_push_to_branch(c, u, p, branch)? {
        return Err(
            Error::Forbidden(Some("You are not allowed to push into this branch".into())).into(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Branches

/// GitLab's ref search: case-insensitive substring; `^`, `$` anchor and `*` is a wildcard.
fn ref_search(names: Vec<(String, Oid)>, search: &str) -> Vec<(String, Oid)> {
    let s = search.to_lowercase();
    if s.contains('^') || s.contains('$') || s.contains('*') {
        let anchored_start = s.starts_with('^');
        let anchored_end = s.ends_with('$');
        let core = s.trim_start_matches('^').trim_end_matches('$');
        let parts: Vec<&str> = core.split('*').collect();
        return names
            .into_iter()
            .filter(|(n, _)| {
                let n = n.to_lowercase();
                let mut pos = 0;
                for (i, part) in parts.iter().enumerate() {
                    if i == 0 && anchored_start {
                        if !n.starts_with(part) {
                            return false;
                        }
                        pos = part.len();
                        continue;
                    }
                    match n[pos..].find(part) {
                        Some(j) => pos += j + part.len(),
                        None => return false,
                    }
                }
                !anchored_end || n.ends_with(parts.last().unwrap())
            })
            .collect();
    }
    let mut out: Vec<(String, Oid)> = names
        .into_iter()
        .filter(|(n, _)| n.to_lowercase().contains(&s))
        .collect();
    if let Some(i) = out.iter().position(|(n, _)| n.to_lowercase() == s) {
        let exact = out.remove(i);
        out.insert(0, exact);
    }
    out
}

fn merged_into_default(r: &Repo, p: &Project, oid: &Oid) -> Result<Option<bool>, Error> {
    let Some(default) = p
        .default_branch
        .as_deref()
        .and_then(|b| r.branch(b).ok().flatten())
    else {
        return Ok(None);
    };
    Ok(Some(
        *oid != default && walk::is_ancestor(r, oid, &default)?,
    ))
}

async fn list_branches(req: Req) -> ApiResult {
    // GitLab caches this response for 30 seconds per project, user and parameters.
    let key = format!(
        "branches:{:p}:{}:{}:{}",
        std::sync::Arc::as_ptr(&req.app),
        req.path_param("id"),
        req.user.as_ref().map(|u| u.id).unwrap_or(0),
        req.query
    );
    if let Some(hit) = crate::cache::get(&key) {
        return Ok(hit.into_resp());
    }
    let resp = list_branches_uncached(&req)?;
    crate::cache::put(&key, &resp, std::time::Duration::from_secs(30));
    Ok(resp)
}

fn list_branches_uncached(req: &Req) -> ApiResult {
    let req = req;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, req)?;
        let r = repo::repo(c, &p);
        let mut branches = r.branches()?;
        if let Some(s) = req.params.str("search") {
            branches = ref_search(branches, &s);
        }
        if let Some(rx) = req.params.str("regex") {
            let re = regex::Regex::new(&rx).map_err(|_| Error::message(400, "Regex is invalid"))?;
            branches.retain(|(n, _)| re.is_match(n));
        }
        match req
            .params
            .one_of("sort", &["name_asc", "updated_asc", "updated_desc"])?
            .as_deref()
        {
            Some("updated_asc") | Some("updated_desc") => {
                let desc = req.params.str("sort").as_deref() == Some("updated_desc");
                let mut keyed: Vec<(i64, (String, Oid))> = branches
                    .into_iter()
                    .map(|b| (r.commit(&b.1).map(|c| c.committer.time).unwrap_or(0), b))
                    .collect();
                keyed.sort_by(|a, b| if desc { b.0.cmp(&a.0) } else { a.0.cmp(&b.0) });
                branches = keyed.into_iter().map(|(_, b)| b).collect();
            }
            _ => {}
        }
        let user = req.user.clone();
        paginate(req, branches, &[], |(name, oid)| {
            let merged = merged_into_default(&r, &p, oid)?;
            Ok(ent::branch(
                c,
                &req.app.config,
                &p,
                user.as_ref(),
                name,
                oid,
                merged,
            )?)
        })
    })
}

/// `HEAD /projects/:id/repository/branches/:branch`: 204 when the branch exists.
async fn head_branch(req: Req) -> ApiResult {
    let name = req.path_param("branch").to_string();
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        match r.branch(&name)? {
            Some(_) => Ok(Resp::no_content()),
            None => Err(Error::not_found("Branch").into()),
        }
    })
}

async fn show_branch(req: Req) -> ApiResult {
    let name = req.path_param("branch").to_string();
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        if !repo::valid_branch_name(&name) {
            return Err(Error::message(400, "The branch refname is invalid").into());
        }
        let r = repo::repo(c, &p);
        let oid = r.branch(&name)?.ok_or(Error::not_found("Branch"))?;
        let merged = merged_into_default(&r, &p, &oid)?;
        Ok::<_, ApiError>(ent::branch(
            c,
            &req.app.config,
            &p,
            req.user.as_ref(),
            &name,
            &oid,
            merged,
        )?)
    })?;
    Ok(Resp::ok(body))
}

async fn create_branch(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["branch", "ref"])?;
    let name = req.params.str("branch").unwrap();
    let rf = req.params.str("ref").unwrap();
    if name.is_empty() {
        return Err(Error::Param("branch is empty".into()).into());
    }
    if rf.is_empty() {
        return Err(Error::Param("ref is empty".into()).into());
    }
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, _) = push_project(c, &req)?;
        let r = repo::repo(c, &p);
        let user = req.user.clone().unwrap();
        if r.is_empty()? {
            // `create_default_branch`: an empty README first.
            let default = repo::default_branch(&p);
            repo::create_commit(
                c,
                &p,
                &user,
                NewCommit {
                    branch: &default,
                    start_branch: None,
                    start_sha: None,
                    message: "Add README.md",
                    actions: &[FileAction::Create { path: "README.md".into(), content: vec![], exec: None }],
                    author_name: None,
                    author_email: None,
                    force: false,
                },
            )?;
        }
        let p = Project::find(c, p.id)?.unwrap();
        let target = create_branch_in(c, &req.app.config, &p, &user, &name, &rf)?;
        let p = Project::find(c, p.id)?.unwrap();
        let merged = merged_into_default(&r, &p, &target)?;
        Ok::<_, ApiError>(ent::branch(c, &req.app.config, &p, Some(&user), &name, &target, merged)?)
    })?;
    Ok(Resp::created(body))
}

/// `Branches::CreateService` (REST and GraphQL `createBranch`): the new branch's commit, or an
/// error whose message GitLab shows.
pub(crate) fn create_branch_in(c: &Connection, config: &labrat_core::Config, p: &Project, user: &labrat_core::model::User, name: &str, rf: &str) -> Result<Oid, ApiError> {
    let r = repo::repo(c, p);
    if !repo::valid_branch_name(name) {
        return Err(Error::message(400, "Branch name is invalid").into());
    }
    if r.branch(name)?.is_some() {
        return Err(Error::message(400, "Branch already exists").into());
    }
    let target = r
        .resolve(rf)?
        .ok_or_else(|| Error::message(400, format!("Failed to create branch '{name}': invalid reference name '{rf}'")))?;
    let conflict = r.branches()?.into_iter().any(|(b, _)| b.starts_with(&format!("{name}/")) || name.starts_with(&format!("{b}/")));
    if conflict {
        return Err(Error::message(400, format!("Failed to create branch '{name}': Branch name conflicts with existing branch hierarchy.")).into());
    }
    r.set_ref(&format!("refs/heads/{name}"), &target)?;
    let update = labrat_git::protocol::RefUpdate { old: Oid::ZERO, new: target, name: format!("refs/heads/{name}") };
    labrat_core::services::push::post_receive(c, config, p, user, &[update])?;
    Ok(target)
}

/// `Branches::DeleteService`: deletes a branch (`None` when there is none) or refuses with
/// Gitaly's pre-receive message.
pub(crate) fn delete_branch_in(c: &Connection, config: &labrat_core::Config, p: &Project, perms: &ProjectPerms, user: &labrat_core::model::User, name: &str) -> Result<Option<()>, ApiError> {
    let r = repo::repo(c, p);
    let Some(oid) = r.branch(name)? else { return Ok(None) };
    // Gitaly wraps pre-receive rejections of API deletions in its own error text.
    let denied = |msg: &str| -> ApiError { Error::message(400, format!("7:deletion denied by access checks: running pre-receive hooks: GitLab: {msg}.")).into() };
    if p.default_branch.as_deref() == Some(name) {
        return Err(denied("The default branch of a project cannot be deleted."));
    }
    if !repo::matching_rules(c, p, name)?.is_empty() {
        let msg = if perms.admin_project() {
            "You can only delete protected branches using the web interface."
        } else {
            "You are not allowed to delete protected branches from this project. Only a project maintainer or owner can delete a protected branch."
        };
        return Err(denied(msg));
    }
    r.delete_ref(&format!("refs/heads/{name}"))?;
    let update = labrat_git::protocol::RefUpdate { old: oid, new: Oid::ZERO, name: format!("refs/heads/{name}") };
    labrat_core::services::push::post_receive(c, config, p, user, &[update])?;
    Ok(Some(()))
}

async fn delete_branch(req: Req) -> ApiResult {
    let name = req.path_param("branch").to_string();
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms) = push_project(c, &req)?;
        let user = req.user.clone().unwrap();
        delete_branch_in(c, &req.app.config, &p, &perms, &user, &name)?.ok_or(Error::not_found("Branch"))?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content().header("Content-Type", "application/json"))
}

async fn delete_merged(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _) = push_project(c, &req)?;
        let r = repo::repo(c, &p);
        for (name, oid) in r.branches()? {
            if p.default_branch.as_deref() == Some(name.as_str())
                || repo::is_protected(c, &p, &name)?
            {
                continue;
            }
            if merged_into_default(&r, &p, &oid)? == Some(true) {
                r.delete_ref(&format!("refs/heads/{name}"))?;
            }
        }
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::status(202, json!({"message": "202 Accepted"})))
}

async fn legacy_protect(req: Req) -> ApiResult {
    legacy(req, true).await
}

async fn legacy_unprotect(req: Req) -> ApiResult {
    legacy(req, false).await
}

async fn legacy(req: Req, protect: bool) -> ApiResult {
    req.user()?;
    let name = req.path_param("branch").to_string();
    let dev_push = req.params.bool("developers_can_push")?;
    let dev_merge = req.params.bool("developers_can_merge")?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = code_project(c, &req)?;
        if !perms.admin_project() {
            return Err(Error::forbidden().into());
        }
        let r = repo::repo(c, &p);
        let oid = r.branch(&name)?.ok_or(Error::not_found("Branch"))?;
        let existing = repo::protected_branches(c, p.id)?.into_iter().find(|pb| pb.name == name);
        if protect {
            match existing {
                Some(pb) => {
                    for (kind, flag) in [("push", dev_push), ("merge", dev_merge)] {
                        if let Some(f) = flag {
                            c.execute(
                                "UPDATE protected_branch_access_levels SET access_level = ?1 WHERE protected_branch_id = ?2 AND kind = ?3",
                                rusqlite::params![if f { 30 } else { 40 }, pb.id, kind],
                            )?;
                        }
                    }
                }
                None => {
                    repo::create_protected_branch(
                        c,
                        &p,
                        &name,
                        if dev_push == Some(true) { 30 } else { 40 },
                        if dev_merge == Some(true) { 30 } else { 40 },
                        false,
                    )?;
                }
            }
        } else if let Some(pb) = existing {
            c.execute("DELETE FROM protected_branches WHERE id = ?1", [pb.id])?;
        }
        let merged = merged_into_default(&r, &p, &oid)?;
        Ok::<_, ApiError>(ent::branch(c, &req.app.config, &p, req.user.as_ref(), &name, &oid, merged)?)
    })?;
    Ok(Resp::ok(body))
}

// ---------------------------------------------------------------------------------------------
// Tags

async fn list_tags(req: Req) -> ApiResult {
    let order_by = req
        .params
        .one_of("order_by", &["name", "updated", "version"])?
        .unwrap_or("updated".into());
    let desc = req.params.one_of("sort", &["asc", "desc"])?.as_deref() != Some("asc");
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, perms) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let mut tags = r.tags()?;
        if let Some(s) = req.params.str("search") {
            tags = ref_search(tags, &s);
        }
        let date = |o: &Oid| -> i64 {
            match r.read(o) {
                Ok((Kind::Tag, d)) => Tag::parse(&d)
                    .ok()
                    .and_then(|t| t.tagger.map(|s| s.time))
                    .unwrap_or(0),
                _ => r.commit(o).map(|c| c.committer.time).unwrap_or(0),
            }
        };
        // Like `git for-each-ref --sort`: ties fall back to ascending ref name.
        tags.sort_by(|a, b| {
            let o = match order_by.as_str() {
                "updated" => date(&a.1).cmp(&date(&b.1)),
                _ => a.0.cmp(&b.0),
            };
            let o = if desc { o.reverse() } else { o };
            o.then(a.0.cmp(&b.0))
        });
        let show_release = perms.reporter() || p.visibility_level > 0;
        // `GitalyKeysetPager#paginate_first_page_via_gitaly` serves the first page of an
        // unsearched list; its page count is unclamped (0 when there are no tags).
        let first_page = req.params.str("page").is_none_or(|p| p.is_empty() || p == "1") && req.params.str("search").is_none_or(|s| s.is_empty());
        crate::pagination::paginate_gitaly(
            &req,
            tags,
            first_page,
            &[("order_by", "updated"), ("sort", "desc")],
            |(name, oid)| {
                let _ = show_release;
                Ok(ent::tag(&req.app.config, &r, &p, name, oid, false)?)
            },
        )
    })
}

async fn show_tag(req: Req) -> ApiResult {
    let name = req.path_param("tag").to_string();
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, perms) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let oid = r
            .ref_oid(&format!("refs/tags/{name}"))?
            .ok_or(Error::not_found("Tag"))?;
        let _ = perms;
        Ok::<_, ApiError>(ent::tag(&req.app.config, &r, &p, &name, &oid, false)?)
    })?;
    Ok(Resp::ok(body))
}

async fn create_tag(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["tag_name", "ref"])?;
    let name = req.params.str("tag_name").unwrap();
    let rf = req.params.str("ref").unwrap();
    let message = req
        .params
        .str("message")
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = push_project(c, &req)?;
        let r = repo::repo(c, &p);
        if rf.is_empty() {
            return Err(Error::message(400, "Target is empty").into());
        }
        if !repo::valid_branch_name(&name) {
            return Err(Error::message(400, "Tag name invalid").into());
        }
        if r.ref_oid(&format!("refs/tags/{name}"))?.is_some() {
            return Err(Error::message(400, format!("Tag {name} already exists")).into());
        }
        if labrat_core::services::protected_tags::is_protected(c, p.id, &name)?
            && !labrat_core::services::protected_tags::can_create(c, req.user.as_ref().unwrap(), &p, &name)?
        {
            return Err(Error::message(400, "You are not allowed to create this tag as it is protected.").into());
        }
        let target = r
            .resolve(&rf)?
            .ok_or_else(|| Error::message(400, format!("Target {rf} is invalid")))?;
        let oid = match &message {
            Some(m) => {
                let user = req.user.clone().unwrap();
                let mut tagger = repo::signature(&user);
                tagger.time = r.next_time(tagger.time, 0)?;
                let tag = Tag {
                    object: target,
                    kind: "commit".into(),
                    name: name.clone(),
                    tagger: Some(tagger),
                    message: format!("{m}\n"),
                };
                r.write(Kind::Tag, &tag.encode())?
            }
            None => target,
        };
        r.set_ref(&format!("refs/tags/{name}"), &oid)?;
        let _ = perms;
        let update = labrat_git::protocol::RefUpdate { old: Oid::ZERO, new: oid, name: format!("refs/tags/{name}") };
        labrat_core::services::push::post_receive(c, &req.app.config, &p, req.user.as_ref().unwrap(), &[update])?;
        Ok::<_, ApiError>(ent::tag(&req.app.config, &r, &p, &name, &oid, true)?)
    })?;
    Ok(Resp::created(body))
}

async fn delete_tag(req: Req) -> ApiResult {
    let name = req.path_param("tag").to_string();
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, _) = push_project(c, &req)?;
        let r = repo::repo(c, &p);
        let old = r.ref_oid(&format!("refs/tags/{name}"))?
            .ok_or(Error::not_found("Tag"))?;
        r.delete_ref(&format!("refs/tags/{name}"))?;
        let update = labrat_git::protocol::RefUpdate { old, new: Oid::ZERO, name: format!("refs/tags/{name}") };
        labrat_core::services::push::post_receive(c, &req.app.config, &p, req.user.as_ref().unwrap(), &[update])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

// ---------------------------------------------------------------------------------------------
// Commits

/// Pagination without totals, as GitLab's commit list does.
fn paginate_no_count(
    req: &Req,
    rows: Vec<Value>,
    has_more: bool,
    defaults: &[(&str, &str)],
) -> Result<Resp, ApiError> {
    let page = Page::from_req(req)?;
    let fake_total = page.offset().saturating_add(rows.len() + usize::from(has_more));
    let mut resp = Resp::ok(Value::Array(rows)).paginated(req, &page, fake_total, defaults);
    resp.headers
        .retain(|(k, _)| k != "X-Total" && k != "X-Total-Pages");
    // Without a count, the `last` link is not known either.
    for (k, v) in resp.headers.iter_mut() {
        if k == "Link" {
            *v = v
                .split(", ")
                .filter(|l| !l.ends_with("rel=\"last\""))
                .collect::<Vec<_>>()
                .join(", ");
        }
    }
    Ok(resp)
}

async fn list_commits(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let all = req.params.bool_or("all", false)?;
        let with_stats = req.params.bool_or("with_stats", false)?;
        let first_parent = req.params.bool_or("first_parent", false)?;
        let tips: Vec<Oid> = if all {
            r.refs()?
                .into_iter()
                .filter(|(n, _)| n.starts_with("refs/heads/") || n.starts_with("refs/tags/"))
                .map(|(_, o)| o)
                .collect()
        } else {
            let rf = req
                .params
                .str("ref_name")
                .unwrap_or_else(|| repo::default_branch(&p));
            if let Some((a, b)) = rf.split_once("...") {
                match (r.resolve(a)?, r.resolve(b)?) {
                    (Some(x), Some(y)) => vec![y, x],
                    _ => vec![],
                }
            } else if let Some((a, b)) = rf.split_once("..") {
                match (r.resolve(a)?, r.resolve(b)?) {
                    (Some(x), Some(y)) => {
                        let list = walk::rev_list(&r, &[y], &[x])?;
                        return render_commits(&req, &r, &p, list, with_stats);
                    }
                    _ => vec![],
                }
            } else {
                r.resolve(&rf)?.into_iter().collect()
            }
        };
        let mut list = walk::rev_list_opts(&r, &tips, &[], first_parent)?;
        if let Some(path) = req.params.str("path") {
            let path = path.trim_start_matches('/').to_string();
            list.retain(|o| commit_touches(&r, o, &path).unwrap_or(false));
        }
        let since = req.params.time("since")?;
        let until = req.params.time("until")?;
        if since.is_some() || until.is_some() {
            list.retain(|o| {
                let t = r.commit(o).map(|c| c.committer.time * 1000).unwrap_or(0);
                since.is_none_or(|s| t >= s) && until.is_none_or(|u| t <= u)
            });
        }
        if let Some(author) = req.params.str("author") {
            let a = author.to_lowercase();
            list.retain(|o| {
                r.commit(o)
                    .map(|c| {
                        c.author.name.to_lowercase().contains(&a)
                            || c.author.email.to_lowercase().contains(&a)
                    })
                    .unwrap_or(false)
            });
        }
        render_commits(&req, &r, &p, list, with_stats)
    })
}

fn render_commits(req: &Req, r: &Repo, p: &Project, list: Vec<Oid>, with_stats: bool) -> ApiResult {
    let page = Page::from_req(req)?;
    page.check_gitaly_offset()?;
    // `commit_count = offset + commits.size + 1`: a full page always announces a next one.
    let has_more = list.len() >= page.offset().saturating_add(page.per_page as usize);
    let mut rows = vec![];
    for o in list.iter().skip(page.offset()).take(page.per_page as usize) {
        let c = r.commit(o)?;
        let mut m = ent::commit(&req.app.config, p, o, &c);
        if with_stats {
            m.insert("stats".into(), ent::stats(r, &c)?);
        }
        rows.push(Value::Object(m));
    }
    paginate_no_count(
        req,
        rows,
        has_more,
        &[("order", "default"), ("trailers", "false")],
    )
}

fn commit_touches(r: &Repo, oid: &Oid, path: &str) -> Result<bool, Error> {
    let c = r.commit(oid)?;
    let here = r.entry_at(&c.tree, path)?.map(|e| e.oid);
    let parent = match c.parents.first() {
        Some(pp) => r.entry_at(&r.commit(pp)?.tree, path)?.map(|e| e.oid),
        None => None,
    };
    Ok(here != parent)
}

fn resolve_commit(r: &Repo, sha: &str) -> Result<Oid, ApiError> {
    let oid = r.resolve(sha)?.ok_or(Error::not_found("Commit"))?;
    if r.kind(&oid)? != Some(Kind::Commit) {
        return Err(Error::not_found("Commit").into());
    }
    Ok(oid)
}

/// `GET /projects/:id/repository/commits/:sha/merge_requests`: `by_related_commit_sha`, the
/// MRs whose latest diff contains the commit or that merged or squashed into it.
async fn commit_merge_requests(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let state = req.params.str("state");
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.read_merge_request(&p) {
            return Err(Error::forbidden().into());
        }
        let r = repo::repo(c, &p);
        let oid = resolve_commit(&r, &sha)?;
        let hex = oid.hex();
        let mut rows = vec![];
        for mr in labrat_core::services::merge_requests::MergeRequest::list_where(c, "target_project_id = ?1 ORDER BY created_at DESC, id DESC", [p.id])? {
            if state.as_ref().is_some_and(|s| s != "all" && *s != mr.state) {
                continue;
            }
            let direct = [&mr.merge_commit_sha, &mr.squash_commit_sha, &mr.merged_commit_sha].iter().any(|x| x.as_deref() == Some(hex.as_str()));
            let in_diff = match mr.latest_diff(c)? {
                Some(d) => d.commits(&r)?.contains(&oid),
                None => false,
            };
            if direct || in_diff {
                rows.push((p.clone(), mr));
            }
        }
        crate::routes::merge_requests::render_basic_list(c, &req, rows)
    })
}

async fn show_commit(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let with_stats = req.params.bool_or("stats", true)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let oid = resolve_commit(&r, &sha)?;
        let commit = r.commit(&oid)?;
        Ok::<_, ApiError>(ent::commit_detail(
            &req.app.config,
            &r,
            &p,
            &oid,
            &commit,
            with_stats,
        )?)
    })?;
    Ok(Resp::ok(body))
}

async fn commit_diff(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let oid = resolve_commit(&r, &sha)?;
        let commit = r.commit(&oid)?;
        let parent_tree = match commit.parents.first() {
            Some(pp) => Some(r.commit(pp)?.tree),
            None => None,
        };
        let changes = diff::diff_trees(&r, parent_tree.as_ref(), Some(&commit.tree))?;
        let uni = req.params.bool_or("unidiff", false)?;
        paginate(&req, changes, &[("unidiff", "false")], |ch| {
            let mut v = ent::diff(&r, ch)?;
            if uni {
                ent::unidiff(&mut v, ch);
            }
            Ok(v)
        })
    })
}

async fn commit_refs(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let kind = req
        .params
        .one_of("type", &["branch", "tag", "all"])?
        .unwrap_or("all".into());
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let oid = resolve_commit(&r, &sha)?;
        let mut rows = vec![];
        if kind != "tag" {
            for (name, tip) in r.branches()? {
                if walk::is_ancestor(&r, &oid, &tip)? {
                    rows.push(json!({"type": "branch", "name": name}));
                }
            }
        }
        if kind != "branch" {
            for (name, tip) in r.tags()? {
                let peeled = r.peel(&tip)?;
                if r.kind(&peeled)? == Some(Kind::Commit) && walk::is_ancestor(&r, &oid, &peeled)? {
                    rows.push(json!({"type": "tag", "name": name}));
                }
            }
        }
        let page = Page::from_req(&req)?;
        let has_more = rows.len() > page.offset().saturating_add(page.per_page as usize);
        let rows: Vec<Value> = rows
            .into_iter()
            .skip(page.offset())
            .take(page.per_page as usize)
            .collect();
        paginate_no_count(&req, rows, has_more, &[("type", "all")])
    })
}

fn decode_content(
    content: Option<&Value>,
    encoding: Option<&str>,
    idx: Option<usize>,
) -> Result<Vec<u8>, ApiError> {
    let text = match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    match encoding.unwrap_or("text") {
        "text" => Ok(text.into_bytes()),
        "base64" => base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| Error::BadRequest(Some("Invalid base64 content".into())).into()),
        _ => Err(Error::BadRequest(Some(match idx {
            Some(i) => format!("actions[{i}][encoding] must be text or base64"),
            None => "encoding must be text or base64".into(),
        }))
        .into()),
    }
}

async fn create_commit(req: Req) -> ApiResult {
    req.user()?;
    let p = &req.params;
    let bad = |m: &str| -> ApiError { Error::BadRequest(Some(m.to_string())).into() };
    let branch = p.str("branch");
    let message = p.str("commit_message");
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (project, _) = code_project(c, &req)?;
        if let Some(b) = &branch {
            ensure_push_to_branch(c, &req, &project, b)?;
        }
        let branch = branch.clone().ok_or_else(|| bad("branch is required"))?;
        let message = message
            .clone()
            .ok_or_else(|| bad("commit_message is required"))?;
        let actions_v = p.raw("actions").cloned().unwrap_or(Value::Null);
        let allow_empty = p.bool_or("allow_empty", false)?;
        let empty_actions = match &actions_v {
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Null => true,
            _ => false,
        };
        if empty_actions && !allow_empty {
            return Err(bad(
                "Provide at least one action, or set allow_empty to true",
            ));
        }
        // Form-encoded `actions[][...]` arrives as an array of objects; `actions[0][...]` as an object.
        let list: Vec<Value> = match actions_v {
            Value::Array(a) => a,
            Value::Object(o) => o.into_iter().map(|(_, v)| v).collect(),
            Value::Null => vec![],
            _ => return Err(bad("actions must be an array")),
        };
        if p.has("start_branch") && p.has("start_sha") {
            return Err(bad("start_branch, start_sha are mutually exclusive"));
        }
        let mut actions = vec![];
        for (i, a) in list.iter().enumerate() {
            let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
            let action =
                s("action").ok_or_else(|| bad(&format!("actions[{i}][action] is required")))?;
            let path = s("file_path")
                .ok_or_else(|| bad(&format!("actions[{i}][file_path] is required")))?;
            let enc = s("encoding");
            let exec = a.get("execute_filemode").and_then(|v| match v {
                Value::Bool(b) => Some(*b),
                Value::String(s) => Some(s == "true"),
                _ => None,
            });
            let fa = match action.as_str() {
                "create" => FileAction::Create {
                    path,
                    content: decode_content(a.get("content"), enc.as_deref(), Some(i))?,
                    exec,
                },
                "update" => {
                    if a.get("content").is_none() {
                        return Err(bad(&format!(
                            "actions[{i}][content] is required for update action"
                        )));
                    }
                    FileAction::Update {
                        path,
                        content: decode_content(a.get("content"), enc.as_deref(), Some(i))?,
                        exec,
                    }
                }
                "delete" => FileAction::Delete { path },
                "move" => {
                    let from = s("previous_path").ok_or_else(|| {
                        bad(&format!(
                            "actions[{i}][previous_path] is required for move action"
                        ))
                    })?;
                    let content = match a.get("content") {
                        None | Some(Value::Null) => None,
                        Some(_) => Some(decode_content(a.get("content"), enc.as_deref(), Some(i))?),
                    };
                    FileAction::Move {
                        from,
                        to: path,
                        content,
                    }
                }
                "chmod" => FileAction::Chmod {
                    path,
                    exec: exec.ok_or_else(|| {
                        bad(&format!(
                            "actions[{i}][execute_filemode] is required for chmod action"
                        ))
                    })?,
                },
                _ => {
                    return Err(bad(&format!(
                        "actions[{i}][action] must be one of: create, update, move, delete, chmod"
                    )));
                }
            };
            actions.push(fa);
        }
        let start_sha = match p.str("start_sha") {
            Some(s) => Some(
                Oid::from_hex(&s)
                    .map_err(|_| Error::message(400, format!("Invalid start_sha '{s}'")))?,
            ),
            None => None,
        };
        let start_branch = p.str("start_branch");
        let user = req.user.clone().unwrap();
        let r = repo::repo(c, &project);
        let old = r.branch(&branch)?.unwrap_or(Oid::ZERO);
        let oid = repo::create_commit(
            c,
            &project,
            &user,
            NewCommit {
                branch: &branch,
                start_branch: start_branch.as_deref(),
                start_sha,
                message: &message,
                actions: &actions,
                author_name: p.str("author_name").as_deref(),
                author_email: p.str("author_email").as_deref(),
                force: p.bool_or("force", false)?,
            },
        )?;
        let update = labrat_git::protocol::RefUpdate {
            old,
            new: oid,
            name: format!("refs/heads/{branch}"),
        };
        labrat_core::services::push::post_receive(c, &req.app.config, &project, &user, &[update])?;
        let commit = r.commit(&oid)?;
        let stats = p.bool("stats")? != Some(false);
        Ok::<_, ApiError>(ent::commit_detail(
            &req.app.config,
            &r,
            &project,
            &oid,
            &commit,
            stats,
        )?)
    })?;
    Ok(Resp::created(body))
}

async fn cherry_pick(req: Req) -> ApiResult {
    pick(req, false).await
}

async fn revert(req: Req) -> ApiResult {
    pick(req, true).await
}

async fn pick(req: Req, revert: bool) -> ApiResult {
    req.user()?;
    req.params.require(&["branch"])?;
    let branch = req.params.str("branch").unwrap();
    let sha = req.path_param("sha").to_string();
    let dry_run = req.params.bool_or("dry_run", false)?;
    let custom_message = req.params.str("message");
    let app = req.app.clone();
    let result = app.db.tx(|c| {
        let (p, _) = code_project(c, &req)?;
        ensure_push_to_branch(c, &req, &p, &branch)?;
        let r = repo::repo(c, &p);
        let oid = resolve_commit(&r, &sha)?;
        let head = r.branch(&branch)?.ok_or(Error::not_found("Branch"))?;
        let commit = r.commit(&oid)?;
        let user = req.user.clone().unwrap();
        let parent_tree = match commit.parents.first() {
            Some(pp) => Some(r.commit(pp)?.tree),
            None => None,
        };
        let head_tree = r.commit(&head)?.tree;
        let (base, theirs) = if revert { (Some(commit.tree), parent_tree.unwrap_or(head_tree)) } else { (parent_tree, commit.tree) };
        let merged = labrat_git::merge::merge_trees(&r, base.as_ref(), &head_tree, &theirs)?;
        let action = if revert { "revert" } else { "cherry-pick" };
        // `Commit#merged_merge_request`: a merge commit recorded as an MR's `merge_commit_sha`.
        let merged_mr = if commit.parents.len() > 1 {
            labrat_core::services::merge_requests::MergeRequest::list_where(
                c,
                "target_project_id = ?1 AND state = 'merged' AND merge_commit_sha = ?2",
                rusqlite::params![p.id, oid.hex()],
            )?
            .into_iter()
            .next()
        } else {
            None
        };
        let kind = if merged_mr.is_some() { "Merge request" } else { "Commit" };
        let fail = |code: &str| -> Value {
            let mut m = serde_json::Map::new();
            m.insert(
                "message".into(),
                json!(format!(
                    "{kind} {action} failed: Can't perform this action automatically. It may have already been done, or a more recent commit may have updated some of this content. Please perform this action locally."
                )),
            );
            m.insert("error_code".into(), json!(code));
            if dry_run {
                m.insert("dry_run".into(), json!("error"));
            }
            Value::Object(m)
        };
        if !merged.clean() {
            return Ok::<_, ApiError>(Err(fail("conflict")));
        }
        if merged.tree == head_tree {
            return Ok(Err(fail("empty")));
        }
        if dry_run {
            return Ok(Ok(None));
        }
        let message = if revert {
            match &merged_mr {
                Some(m) => format!("Revert \"{}\"\n\nThis reverts merge request !{}", commit.title(), m.iid),
                None => format!("Revert \"{}\"\n\nThis reverts commit {oid}", commit.title()),
            }
        } else {
            match custom_message.clone() {
                Some(m) => m,
                None => {
                    // `cherry_pick_description`: an MR's merge commit lists the MR's commits,
                    // oldest first.
                    let mut description = format!("(cherry picked from commit {oid})");
                    if let Some(m) = &merged_mr
                        && let Some(d) = m.latest_diff(c)?
                    {
                        let mr_commits = d.commits(&r)?;
                        if !mr_commits.is_empty() {
                            description.push('\n');
                            for o in mr_commits.iter().rev() {
                                let cm = r.commit(o)?;
                                description.push_str(&format!("\n{} {}", &o.hex()[..8], cm.title()));
                            }
                        }
                    }
                    format!(
                        "{}\n\n{description}\n\nCo-authored-by: {} <{}>",
                        commit.message.trim_end(),
                        commit.author.name,
                        commit.author.email
                    )
                }
            }
        };
        let me = repo::signature(&user);
        let new = labrat_git::Commit {
            tree: merged.tree,
            parents: vec![head],
            author: if revert { me.clone() } else { Signature { ..commit.author.clone() } },
            committer: me,
            extra: vec![],
            message,
        };
        let new_oid = r.write_commit(&new)?;
        r.set_ref(&format!("refs/heads/{branch}"), &new_oid)?;
        let update = labrat_git::protocol::RefUpdate { old: head, new: new_oid, name: format!("refs/heads/{branch}") };
        labrat_core::services::push::post_receive(c, &req.app.config, &p, &user, &[update])?;
        // `SystemNoteService.picked_into_branch` on the MR whose merge commit was picked.
        if !revert && let Some(m) = &merged_mr {
            let body = format!("picked the changes into the branch [`{branch}`](/{}/-/tree/{branch}) with commit {}", p.full_path, new_oid.hex());
            labrat_core::services::notes::system_note(c, m.id, p.id, &user, &body, "cherry_pick")?;
        }
        let stored = r.commit(&new_oid)?;
        Ok(Ok(Some(Value::Object(ent::commit(&req.app.config, &p, &new_oid, &stored)))))
    })?;
    match result {
        Ok(Some(v)) => Ok(Resp::created(v)),
        Ok(None) => Ok(Resp::ok(json!({"dry_run": "success"}))),
        Err(body) => Err(ApiError {
            status: 400,
            body,
            headers: vec![],
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// Files

fn file_path_param(req: &Req) -> Result<String, ApiError> {
    let path = req.path_param("file_path").to_string();
    if path.is_empty() || path.split('/').any(|s| s == "..") {
        return Err(Error::Param("file_path should be a valid file path".into()).into());
    }
    Ok(path)
}

struct FileInfo {
    value: Value,
    headers: Vec<(String, String)>,
    data: Vec<u8>,
}

fn load_file(c: &Connection, req: &Req, ref_required: bool) -> Result<FileInfo, ApiError> {
    let (p, _) = code_project(c, req)?;
    let path = file_path_param(req)?;
    let rf = match req.params.str("ref") {
        Some(r) if r.is_empty() => return Err(Error::Param("ref is empty".into()).into()),
        Some(r) => r,
        None if ref_required => return Err(Error::Param("ref is missing".into()).into()),
        None => repo::default_branch(&p),
    };
    let r = repo::repo(c, &p);
    let oid = r.resolve(&rf)?.ok_or(Error::not_found("Commit"))?;
    let commit = r.commit(&oid)?;
    let entry = r
        .entry_at(&commit.tree, &path)?
        .filter(|e| !e.is_tree())
        .ok_or(Error::not_found("File"))?;
    let data = r.blob(&entry.oid)?;
    let last = repo::last_commit_for_path(&r, &oid, &path)?.unwrap_or(oid);
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    let sha256 = hex::encode(Sha256::digest(&data));
    let exec = entry.mode == labrat_git::object::MODE_EXEC;
    let value = json!({
        "file_name": name,
        "file_path": path,
        "size": data.len(),
        "encoding": "base64",
        "content_sha256": sha256,
        "ref": rf,
        "blob_id": entry.oid.hex(),
        "commit_id": oid.hex(),
        "last_commit_id": last.hex(),
        "execute_filemode": exec,
        "content": base64::engine::general_purpose::STANDARD.encode(&data),
    });
    let headers = vec![
        ("X-Gitlab-File-Name".into(), name),
        ("X-Gitlab-File-Path".into(), path),
        ("X-Gitlab-Size".into(), data.len().to_string()),
        ("X-Gitlab-Encoding".into(), "base64".into()),
        ("X-Gitlab-Content-Sha256".into(), sha256),
        ("X-Gitlab-Ref".into(), rf),
        ("X-Gitlab-Blob-Id".into(), entry.oid.hex()),
        ("X-Gitlab-Commit-Id".into(), oid.hex()),
        ("X-Gitlab-Last-Commit-Id".into(), last.hex()),
        ("X-Gitlab-Execute-Filemode".into(), exec.to_string()),
    ];
    Ok(FileInfo {
        value,
        headers,
        data,
    })
}

async fn get_file(req: Req) -> ApiResult {
    let app = req.app.clone();
    let f = app.db.read(|c| load_file(c, &req, true))?;
    let mut resp = Resp::ok(f.value);
    resp.headers = f.headers;
    Ok(resp)
}

async fn head_file(req: Req) -> ApiResult {
    let app = req.app.clone();
    match app.db.read(|c| load_file(c, &req, true)) {
        Ok(f) => Ok(Resp {
            status: axum::http::StatusCode::OK,
            body: None,
            headers: f.headers,
        }),
        Err(e) => Ok(Resp {
            status: axum::http::StatusCode::from_u16(e.status).unwrap(),
            body: None,
            headers: vec![],
        }),
    }
}

async fn raw_file(req: Req) -> axum::response::Response {
    let app = req.app.clone();
    match app.db.read(|c| load_file(c, &req, false)) {
        Ok(f) => raw_response(f.data, f.headers),
        Err(e) => axum::response::IntoResponse::into_response(e),
    }
}

fn raw_response(data: Vec<u8>, headers: Vec<(String, String)>) -> axum::response::Response {
    let ctype = if diff::is_binary(&data) {
        "application/octet-stream"
    } else {
        "text/plain; charset=utf-8"
    };
    let mut b = axum::response::Response::builder()
        .status(200)
        .header("Content-Type", ctype);
    for (k, v) in headers {
        b = b.header(k, v);
    }
    b.body(axum::body::Body::from(data)).unwrap()
}

fn file_write(
    req: &Req,
    kind: &str,
) -> Result<(String, String, Option<String>, Vec<u8>, Option<bool>), ApiError> {
    let p = &req.params;
    let bad = |m: &str| -> ApiError { Error::BadRequest(Some(m.to_string())).into() };
    let branch = p.str("branch").ok_or_else(|| bad("branch is required"))?;
    let message = p
        .str("commit_message")
        .ok_or_else(|| bad("commit_message is required"))?;
    let content = if kind == "delete" {
        vec![]
    } else {
        if !p.has("content") {
            return Err(bad("content is required"));
        }
        decode_content(p.raw("content"), p.str("encoding").as_deref(), None)?
    };
    Ok((
        branch,
        message,
        p.str("start_branch"),
        content,
        p.bool("execute_filemode")?,
    ))
}

async fn write_file(req: Req, kind: &'static str) -> ApiResult {
    req.user()?;
    let path = file_path_param(&req)?;
    let (branch, message, start_branch, content, exec) = file_write(&req, kind)?;
    let last_commit_id = req.params.str("last_commit_id");
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms) = code_project(c, &req)?;
        if kind == "delete" {
            if !perms.developer() {
                return Err(Error::forbidden().into());
            }
            if !repo::can_push_to_branch(c, req.user.as_ref().unwrap(), &p, &branch)? {
                return Err(Error::message(400, "You are not allowed to push into this branch").into());
            }
        } else {
            ensure_push_to_branch(c, &req, &p, &branch)?;
        }
        let r = repo::repo(c, &p);
        if let Some(last) = &last_commit_id {
            let from = start_branch.as_deref().unwrap_or(&branch);
            if let Some(head) = r.branch(from)? {
                let current = repo::last_commit_for_path(&r, &head, &path)?;
                if current.is_some_and(|cur| cur.hex() != *last) {
                    let msg = if kind == "delete" {
                        "You are attempting to delete a file that has been previously updated."
                    } else {
                        "You are attempting to update a file that has changed since you started editing it."
                    };
                    return Err(Error::message(400, msg).into());
                }
            }
        }
        let action = match kind {
            "create" => FileAction::Create { path: path.clone(), content: content.clone(), exec },
            "update" => FileAction::Update { path: path.clone(), content: content.clone(), exec },
            _ => FileAction::Delete { path: path.clone() },
        };
        let user = req.user.clone().unwrap();
        let old = r.branch(&branch)?.unwrap_or(Oid::ZERO);
        let oid = repo::create_commit(
            c,
            &p,
            &user,
            NewCommit {
                branch: &branch,
                start_branch: start_branch.as_deref(),
                start_sha: None,
                message: &message,
                actions: &[action],
                author_name: req.params.str("author_name").as_deref(),
                author_email: req.params.str("author_email").as_deref(),
                force: false,
            },
        )?;
        let update = labrat_git::protocol::RefUpdate { old, new: oid, name: format!("refs/heads/{branch}") };
        labrat_core::services::push::post_receive(c, &req.app.config, &p, &user, &[update])?;
        Ok::<_, ApiError>(())
    })?;
    match kind {
        "create" => Ok(Resp::created(json!({"file_path": path, "branch": branch}))),
        "update" => Ok(Resp::ok(json!({"file_path": path, "branch": branch}))),
        _ => Ok(Resp::no_content()),
    }
}

async fn create_file(req: Req) -> ApiResult {
    write_file(req, "create").await
}

async fn update_file(req: Req) -> ApiResult {
    write_file(req, "update").await
}

async fn delete_file(req: Req) -> ApiResult {
    write_file(req, "delete").await
}

// ---------------------------------------------------------------------------------------------
// Tree, blobs, compare

async fn tree(req: Req) -> ApiResult {
    let recursive = req.params.bool_or("recursive", false)?;
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let rf = req
            .params
            .str("ref")
            .unwrap_or_else(|| repo::default_branch(&p));
        let oid = r.resolve(&rf)?.ok_or(Error::not_found("Tree"))?;
        let root = r.commit(&oid)?.tree;
        let base = req
            .params
            .str("path")
            .unwrap_or_default()
            .trim_matches('/')
            .to_string();
        let start = if base.is_empty() {
            Some(root)
        } else {
            r.entry_at(&root, &base)?
                .filter(|e| e.is_tree())
                .map(|e| e.oid)
        };
        let mut rows: Vec<Value> = vec![];
        if let Some(t) = start {
            collect_tree(&r, &t, &base, recursive, &mut rows)?;
        }
        let rank = |v: &Value| match v["type"].as_str() {
            Some("tree") => 0,
            Some("blob") => 1,
            _ => 2,
        };
        rows.sort_by(|a, b| {
            rank(a).cmp(&rank(b)).then(
                a["path"]
                    .as_str()
                    .unwrap_or("")
                    .as_bytes()
                    .cmp(b["path"].as_str().unwrap_or("").as_bytes()),
            )
        });
        paginate(&req, rows, &[("pagination", "legacy")], |v| Ok(v.clone()))
    })
}

fn collect_tree(
    r: &Repo,
    tree: &Oid,
    prefix: &str,
    recursive: bool,
    out: &mut Vec<Value>,
) -> Result<(), Error> {
    let entries = r.tree(tree)?;
    let kind_of = |e: &labrat_git::TreeEntry| {
        if e.is_tree() {
            0
        } else if e.mode == labrat_git::object::MODE_COMMIT {
            2
        } else {
            1
        }
    };
    let mut sorted = entries.clone();
    sorted.sort_by(|a, b| {
        kind_of(a)
            .cmp(&kind_of(b))
            .then(a.name.as_bytes().cmp(b.name.as_bytes()))
    });
    for e in sorted {
        let path = if prefix.is_empty() {
            e.name.clone()
        } else {
            format!("{prefix}/{}", e.name)
        };
        let ty = match kind_of(&e) {
            0 => "tree",
            2 => "commit",
            _ => "blob",
        };
        out.push(json!({"id": e.oid.hex(), "name": e.name, "type": ty, "path": path, "mode": format!("{:06o}", e.mode)}));
        if recursive && e.is_tree() {
            collect_tree(r, &e.oid, &path, true, out)?;
        }
    }
    Ok(())
}

async fn blob(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let oid = r.resolve(&sha)?.ok_or(Error::not_found("Blob"))?;
        let data = r.blob(&oid).map_err(|_| Error::not_found("Blob"))?;
        Ok::<_, ApiError>(json!({
            "size": data.len(),
            "encoding": "base64",
            "content": base64::engine::general_purpose::STANDARD.encode(&data),
            "sha": oid.hex(),
        }))
    })?;
    Ok(Resp::ok(body))
}

async fn blob_raw(req: Req) -> axum::response::Response {
    let sha = req.path_param("sha").to_string();
    let app = req.app.clone();
    let result = app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let oid = r.resolve(&sha)?.ok_or(Error::not_found("Blob"))?;
        Ok::<_, ApiError>(r.blob(&oid).map_err(|_| Error::not_found("Blob"))?)
    });
    match result {
        Ok(data) => raw_response(data, vec![]),
        Err(e) => axum::response::IntoResponse::into_response(e),
    }
}

async fn compare(req: Req) -> ApiResult {
    req.params.require(&["from", "to"])?;
    let from = req.params.str("from").unwrap();
    let to = req.params.str("to").unwrap();
    let straight = req.params.bool_or("straight", false)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let (Some(a), Some(b)) = (r.resolve(&from)?, r.resolve(&to)?) else {
            return Err::<Value, ApiError>(Error::not_found("Ref").into());
        };
        let base = if straight { Some(a) } else { walk::merge_base(&r, &a, &b)? };
        let mut commits = walk::rev_list(&r, &[b], &base.into_iter().collect::<Vec<_>>())?;
        commits.reverse();
        let same = a == b;
        let commit_values: Vec<Value> = if same {
            vec![]
        } else {
            commits
                .iter()
                .map(|o| r.commit(o).map(|c| Value::Object(ent::commit(&req.app.config, &p, o, &c))))
                .collect::<Result<_, _>>()?
        };
        let diffs: Vec<Value> = if same {
            vec![]
        } else {
            let base_tree = match base {
                Some(b) => Some(r.commit(&b)?.tree),
                None => None,
            };
            let head_tree = r.commit(&b)?.tree;
            ent::compare_diff_list(&r, &diff::diff_trees(&r, base_tree.as_ref(), Some(&head_tree))?)?
        };
        let from_sha = base.unwrap_or(a).hex();
        let sep = if straight { ".." } else { "..." };
        Ok(json!({
            "commit": commit_values.last().cloned().unwrap_or(Value::Null),
            "commits": commit_values,
            "diffs": diffs,
            "compare_timeout": false,
            "compare_same_ref": same,
            "web_url": req.app.config.url(&format!("/{}/-/compare/{from_sha}{sep}{}", p.full_path, b.hex())),
        }))
    })?;
    Ok(Resp::ok(body))
}

async fn merge_base(req: Req) -> ApiResult {
    let refs = req.params.list("refs").unwrap_or_default();
    if refs.len() < 2 {
        return Err(Error::BadRequest(Some("Provide at least 2 refs".into())).into());
    }
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let mut oids = vec![];
        for rf in &refs {
            oids.push(
                r.resolve(rf)?
                    .ok_or_else(|| Error::message(404, format!("Could not find ref: {rf}")))?,
            );
        }
        let mut base = Some(oids[0]);
        for o in &oids[1..] {
            base = match base {
                Some(b) => walk::merge_base(&r, &b, o)?,
                None => None,
            };
        }
        let base = base.ok_or(Error::not_found("Merge Base"))?;
        let commit = r.commit(&base)?;
        Ok::<_, ApiError>(Value::Object(ent::commit(
            &req.app.config,
            &p,
            &base,
            &commit,
        )))
    })?;
    Ok(Resp::ok(body))
}

// ---------------------------------------------------------------------------------------------
// Protected branches

async fn list_protected(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let mut list = repo::protected_branches(c, p.id)?;
        if let Some(s) = req.params.str("search") {
            let s = s.to_lowercase();
            list.retain(|pb| pb.name.to_lowercase().contains(&s));
        }
        paginate(&req, list, &[], |pb| Ok(ent::protected_branch(pb, req.app.config.is_ee())))
    })
}

fn find_protected(
    c: &Connection,
    p: &Project,
    name: &str,
) -> Result<repo::ProtectedBranch, ApiError> {
    repo::protected_branches(c, p.id)?
        .into_iter()
        .find(|pb| pb.name == name)
        .ok_or_else(|| Error::Api(404, json!("404 Not found")).into())
}

async fn show_protected(req: Req) -> ApiResult {
    let name = req.path_param("name").to_string();
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        Ok::<_, ApiError>(ent::protected_branch(&find_protected(c, &p, &name)?, req.app.config.is_ee()))
    })?;
    Ok(Resp::ok(body))
}

fn level_param(req: &Req, name: &str) -> Result<i64, ApiError> {
    let v = req.params.int(name)?.unwrap_or(40);
    if ![0, 30, 40, 60].contains(&v) {
        return Err(Error::Param(format!("{name} does not have a valid value")).into());
    }
    Ok(v)
}

async fn create_protected(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["name"])?;
    let name = req.params.str("name").unwrap();
    let push = level_param(&req, "push_access_level")?;
    let merge = level_param(&req, "merge_access_level")?;
    let force = req.params.bool_or("allow_force_push", false)?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = code_project(c, &req)?;
        if !perms.admin_project() {
            return Err(Error::forbidden().into());
        }
        if repo::protected_branches(c, p.id)?
            .iter()
            .any(|pb| pb.name == name)
        {
            return Err(
                Error::message(409, format!("Protected branch '{name}' already exists")).into(),
            );
        }
        if name.trim().is_empty() {
            return Err(Error::Api(422, json!(["Name can't be blank"])).into());
        }
        repo::create_protected_branch(c, &p, &name, push, merge, force)?;
        if req.app.config.is_ee() {
            let code_owner = req.params.bool_or("code_owner_approval_required", false)?;
            c.execute(
                "UPDATE protected_branches SET code_owner_approval_required = ?1, unprotect_access_level = 40 WHERE project_id = ?2 AND name = ?3",
                rusqlite::params![code_owner, p.id, name],
            )?;
        }
        Ok::<_, ApiError>(ent::protected_branch(&find_protected(c, &p, &name)?, req.app.config.is_ee()))
    })?;
    Ok(Resp::created(body))
}

async fn update_protected(req: Req) -> ApiResult {
    req.user()?;
    let name = req.path_param("name").to_string();
    let force = req.params.bool("allow_force_push")?;
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = code_project(c, &req)?;
        let pb = find_protected(c, &p, &name)?;
        if !perms.admin_project() {
            return Err(Error::forbidden().into());
        }
        if let Some(f) = force {
            c.execute("UPDATE protected_branches SET allow_force_push = ?1, updated_at = ?2 WHERE id = ?3", rusqlite::params![f, time::now(), pb.id])?;
        }
        if req.app.config.is_ee()
            && let Some(v) = req.params.bool("code_owner_approval_required")? {
                c.execute("UPDATE protected_branches SET code_owner_approval_required = ?1, updated_at = ?2 WHERE id = ?3", rusqlite::params![v, time::now(), pb.id])?;
            }
        Ok::<_, ApiError>(ent::protected_branch(&find_protected(c, &p, &name)?, req.app.config.is_ee()))
    })?;
    Ok(Resp::ok(body))
}

async fn delete_protected(req: Req) -> ApiResult {
    req.user()?;
    let name = req.path_param("name").to_string();
    let app = req.app.clone();
    app.db.tx(|c| {
        let (p, perms) = code_project(c, &req)?;
        let pb = find_protected(c, &p, &name)?;
        if !perms.admin_project() {
            return Err(Error::forbidden().into());
        }
        c.execute("DELETE FROM protected_branches WHERE id = ?1", [pb.id])?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

/// `GET .../commits/:sha/sequence`: the number of commits reachable from the commit.
async fn commit_sequence(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let first_parent = req.params.bool_or("first_parent", false)?;
    let app = req.app.clone();
    let body = app.db.read(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !perms.read_code(&p) {
            return Err(Error::forbidden().into());
        }
        let r = repo::repo(c, &p);
        let oid = resolve_commit(&r, &sha)?;
        let count = if first_parent {
            let mut n = 0;
            let mut cur = Some(oid);
            while let Some(o) = cur {
                n += 1;
                cur = r.commit(&o)?.parents.first().copied();
            }
            n
        } else {
            labrat_git::walk::rev_list(&r, &[oid], &[])?.len()
        };
        Ok::<_, ApiError>(json!({"count": count}))
    })?;
    Ok(Resp::ok(body))
}

/// labrat doesn't verify signatures: every commit is unsigned.
async fn commit_signature(req: Req) -> ApiResult {
    let sha = req.path_param("sha").to_string();
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = find::project(c, &req, req.path_param("id"))?;
        let r = repo::repo(c, &p);
        resolve_commit(&r, &sha)?;
        Err::<Resp, ApiError>(Error::not_found("Signature").into())
    })
}
