//! Protected tags (`lib/api/protected_tags.rb`), blame (`lib/api/files.rb`) and contributors
//! (`lib/api/repositories.rb`).

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::pagination::paginate;
use crate::routes::repository::code_project;
use axum::Router;
use axum::routing::get;
use labrat_core::services::protected_tags::{self, ProtectedTag};
use labrat_core::{App, Error, repo, time};
use labrat_git::{Commit, Oid, Repo};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/protected_tags", get(list_protected).post(protect))
        .route("/projects/{id}/protected_tags/{name}", get(show_protected).delete(unprotect))
        .route("/projects/{id}/repository/files/{file_path}/blame", get(blame))
        .route("/projects/{id}/repository/contributors", get(contributors))
}

// ---------------------------------------------------------------------------------------------
// Protected tags

fn level_description(level: i64) -> &'static str {
    match level {
        0 => "No one",
        30 => "Developers + Maintainers",
        60 => "Instance admins",
        _ => "Maintainers",
    }
}

fn protected_entity(t: &ProtectedTag, ee: bool) -> Value {
    json!({
        "name": t.name,
        "create_access_levels": t.create_levels.iter().map(|(id, l)| {
            let mut m = json!({
                "id": id,
                "access_level": l,
                "access_level_description": level_description(*l),
                "deploy_key_id": null,
            });
            if ee {
                // EE also allows specific users and groups.
                m["user_id"] = Value::Null;
                m["group_id"] = Value::Null;
            }
            m
        }).collect::<Vec<_>>(),
    })
}

fn not_found() -> ApiError {
    Error::Api(404, json!("404 Not found")).into()
}

/// Protected tags are managed by Maintainers.
fn maintainer(c: &Connection, req: &Req) -> Result<labrat_core::model::Project, ApiError> {
    let (p, perms) = code_project(c, req)?;
    if !perms.admin_project() {
        return Err(Error::forbidden().into());
    }
    Ok(p)
}

async fn list_protected(req: Req) -> ApiResult {
    let app = req.app.clone();
    app.db.read(|c| {
        let p = maintainer(c, &req)?;
        paginate(&req, protected_tags::list(c, p.id)?, &[], |t| Ok(protected_entity(t, req.app.config.is_ee())))
    })
}

async fn show_protected(req: Req) -> ApiResult {
    let name = req.path_param("name").to_string();
    let body = req.app.db.read(|c| {
        let p = maintainer(c, &req)?;
        let t = protected_tags::find(c, p.id, &name)?.ok_or_else(not_found)?;
        Ok::<_, ApiError>(protected_entity(&t, req.app.config.is_ee()))
    })?;
    Ok(Resp::ok(body))
}

async fn protect(req: Req) -> ApiResult {
    req.user()?;
    req.params.require(&["name"])?;
    let name = req.params.str("name").unwrap_or_default();
    let level = match req.params.int("create_access_level")? {
        None => 40,
        Some(l) if [0, 30, 40, 60].contains(&l) => l,
        Some(_) => return Err(Error::Param("create_access_level does not have a valid value".into()).into()),
    };
    let body = req.app.db.tx(|c| {
        let p = maintainer(c, &req)?;
        if protected_tags::find(c, p.id, &name)?.is_some() {
            return Err(Error::Api(422, json!(["Name has already been taken"])).into());
        }
        let t = protected_tags::create(c, p.id, &name, level)?;
        Ok::<_, ApiError>(protected_entity(&t, req.app.config.is_ee()))
    })?;
    Ok(Resp::created(body))
}

async fn unprotect(req: Req) -> ApiResult {
    let name = req.path_param("name").to_string();
    req.app.db.tx(|c| {
        let p = maintainer(c, &req)?;
        let t = protected_tags::find(c, p.id, &name)?.ok_or_else(not_found)?;
        protected_tags::delete(c, &t)?;
        Ok::<_, ApiError>(())
    })?;
    Ok(Resp::no_content())
}

// ---------------------------------------------------------------------------------------------
// Blame

fn file_lines(r: &Repo, commit: &Commit, path: &str) -> labrat_core::Result<Option<Vec<Vec<u8>>>> {
    let Some(e) = r.entry_at(&commit.tree, path)? else { return Ok(None) };
    if e.mode == 0o40000 {
        return Ok(None);
    }
    let data = r.blob(&e.oid)?;
    Ok(Some(split_lines(&data)))
}

fn split_lines(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = data.split(|b| *b == b'\n').map(|l| l.to_vec()).collect();
    if data.ends_with(b"\n") || data.is_empty() {
        out.pop();
    }
    out
}

/// `git blame`: lines unchanged against a parent (Myers line diff) pass to it, newest commits
/// first, following the file across renames the parent's tree diff detects; what no parent
/// explains belongs to the commit. No copy detection (`-C`).
fn blame_lines(r: &Repo, start: Oid, path: &str) -> labrat_core::Result<Option<(Vec<Vec<u8>>, Vec<Oid>)>> {
    let head = r.commit(&start)?;
    let Some(lines) = file_lines(r, &head, path)? else { return Ok(None) };
    let mut owner = vec![Oid::ZERO; lines.len()];
    // Pending lines per (commit, path): (final line index, line number in that file).
    let mut pending: HashMap<(Oid, String), Vec<(usize, usize)>> = HashMap::new();
    pending.insert((start, path.to_string()), (0..lines.len()).map(|i| (i, i)).collect());
    let mut commits: HashMap<Oid, Commit> = HashMap::new();
    commits.insert(start, head);
    loop {
        // The newest commit with pending lines (by committer time, then id).
        let Some(key) = pending.keys().max_by_key(|(o, p)| (commits[o].committer.time, *o, p.clone())).cloned() else { break };
        let mine = pending.remove(&key).unwrap();
        let (cur, cur_path) = key;
        let c = commits[&cur].clone();
        let content = r.blob(&r.entry_at(&c.tree, &cur_path)?.unwrap().oid)?;
        let mut left = mine;
        for parent in &c.parents {
            if left.is_empty() {
                break;
            }
            let pc = r.commit(parent)?;
            let parent_path = match r.entry_at(&pc.tree, &cur_path)? {
                Some(_) => Some(cur_path.clone()),
                None => labrat_git::diff::diff_trees(r, Some(&pc.tree), Some(&c.tree))?
                    .into_iter()
                    .find(|ch| ch.renamed_file && ch.new_path == cur_path)
                    .map(|ch| ch.old_path),
            };
            let Some(parent_path) = parent_path else { continue };
            let Some(pe) = r.entry_at(&pc.tree, &parent_path)? else { continue };
            let old = r.blob(&pe.oid)?;
            // new line number (0-based) -> old line number (0-based) for unchanged lines.
            let mut map: HashMap<usize, usize> = HashMap::new();
            for d in labrat_git::diff::diff_lines(&old, &content) {
                if d.kind == labrat_git::diff::LineKind::Context {
                    map.insert(d.new_pos - 1, d.old_pos - 1);
                }
            }
            let (pass, keep): (Vec<_>, Vec<_>) = left.into_iter().partition(|(_, n)| map.contains_key(n));
            if !pass.is_empty() {
                commits.entry(*parent).or_insert(pc);
                pending.entry((*parent, parent_path)).or_default().extend(pass.into_iter().map(|(i, n)| (i, map[&n])));
            }
            left = keep;
        }
        for (i, _) in left {
            owner[i] = cur;
        }
    }
    Ok(Some((lines, owner)))
}

fn blame_commit(oid: &Oid, c: &Commit) -> Value {
    json!({
        "id": oid.hex(),
        "parent_ids": c.parents.iter().map(|p| p.hex()).collect::<Vec<_>>(),
        "message": c.message,
        "authored_date": time::iso_git(c.author.time, c.author.offset),
        "author_name": c.author.name,
        "author_email": c.author.email,
        "committed_date": time::iso_git(c.committer.time, c.committer.offset),
        "committer_name": c.committer.name,
        "committer_email": c.committer.email,
    })
}

async fn blame(req: Req) -> ApiResult {
    let path = req.path_param("file_path").to_string();
    let rf = req.params.str("ref").unwrap_or_default();
    if rf.is_empty() {
        return Err(Error::Param("ref is missing, ref is empty".into()).into());
    }
    let start = req.params.int("range[start]")?.or(req.params.map.get("range").and_then(|v| v.get("start")).and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or(v.as_i64())));
    let end = req.params.int("range[end]")?.or(req.params.map.get("range").and_then(|v| v.get("end")).and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or(v.as_i64())));
    if let (Some(s), Some(e)) = (start, end)
        && s > e
    {
        return Err(Error::message(400, "range[start] must be less than or equal to range[end]").into());
    }
    let body = req.app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let oid = r.resolve(&rf)?.ok_or(Error::not_found("Commit"))?;
        let oid = r.peel(&oid)?;
        let (lines, owner) = blame_lines(&r, oid, &path)?.ok_or(Error::not_found("File"))?;
        let (from, to) = match (start, end) {
            (Some(s), Some(e)) => ((s.max(1) - 1) as usize, (e as usize).min(lines.len())),
            _ => (0, lines.len()),
        };
        let mut out: Vec<Value> = vec![];
        let mut last: Option<Oid> = None;
        for i in from..to {
            let line = Value::String(String::from_utf8_lossy(&lines[i]).into_owned());
            if last == Some(owner[i]) {
                out.last_mut().unwrap()["lines"].as_array_mut().unwrap().push(line);
            } else {
                let cm = r.commit(&owner[i])?;
                out.push(json!({"commit": blame_commit(&owner[i], &cm), "lines": [line]}));
                last = Some(owner[i]);
            }
        }
        Ok::<_, ApiError>(Value::Array(out))
    })?;
    Ok(Resp::ok(body))
}

// ---------------------------------------------------------------------------------------------
// Contributors

/// `Repository#contributors`: up to 2000 non-merge commits of the ref, grouped by author email
/// (the name of the newest commit), sorted by `order_by`.
async fn contributors(req: Req) -> ApiResult {
    let order_by = req.params.one_of("order_by", &["email", "name", "commits"])?.unwrap_or_else(|| "commits".into());
    let sort = req.params.one_of("sort", &["asc", "desc"])?.unwrap_or_else(|| "asc".into());
    let app = req.app.clone();
    app.db.read(|c| {
        let (p, _) = code_project(c, &req)?;
        let r = repo::repo(c, &p);
        let rf = req.params.str("ref").or(p.default_branch.clone()).unwrap_or_default();
        let tip = r.resolve(&rf)?.ok_or(Error::not_found("404"))?;
        let mut groups: Vec<(String, String, i64)> = vec![];
        let mut n = 0;
        for oid in labrat_git::walk::rev_list(&r, &[tip], &[])? {
            let cm = r.commit(&oid)?;
            if cm.parents.len() > 1 {
                continue;
            }
            n += 1;
            if n > 2000 {
                break;
            }
            match groups.iter_mut().find(|g| g.1 == cm.author.email) {
                Some(g) => {
                    if g.0.trim().is_empty() {
                        g.0 = cm.author.name.clone();
                    }
                    g.2 += 1;
                }
                None => groups.push((cm.author.name.clone(), cm.author.email.clone(), 1)),
            }
        }
        groups.sort_by(|a, b| {
            let o = match order_by.as_str() {
                "email" => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
                "name" => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
                _ => a.2.cmp(&b.2),
            };
            if sort == "desc" { o.reverse() } else { o }
        });
        paginate(&req, groups, &[("order_by", "commits"), ("sort", "asc")], |(name, email, commits)| {
            Ok(json!({"name": name, "email": email, "commits": commits, "additions": 0, "deletions": 0}))
        })
    })
}
