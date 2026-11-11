//! Git smart HTTP: `/<namespace>/<project>.git/{info/refs,git-upload-pack,git-receive-pack}`.

use crate::ctx::ApiError;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use labrat_core::model::{Project, User};
use labrat_core::perms::ProjectPerms;
use labrat_core::services::{tokens, users};
use labrat_core::{App, Error, repo};
use labrat_git::protocol::{self, RefUpdate};
use labrat_git::{Oid, Repo, walk};
use std::sync::Arc;

const NOT_FOUND: &str =
    "The project you were looking for could not be found or you don't have permission to view it.";

fn text(status: StatusCode, body: String) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(body))
        .unwrap()
}

fn unauthorized(app: &App) -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, "Basic realm=\"GitLab\"")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(format!(
            "HTTP Basic: Access denied. If a password was provided for Git authentication, the password was incorrect or you're required to use a token instead of a password. If a token was provided, it was either incorrect, expired, or improperly scoped. See {}/help/topics/git/troubleshooting_git#error-on-git-fetch-http-basic-access-denied",
            app.config.external_url
        )))
        .unwrap()
}

/// The authenticated git actor and what it may do.
struct Actor {
    user: User,
    can_download: bool,
    can_push: bool,
}

enum Auth {
    None,
    Ok(Actor),
    Denied(Response),
}

fn authenticate(app: &App, headers: &HeaderMap) -> Auth {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return Auth::None;
    };
    let Some(encoded) = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))
    else {
        return Auth::None;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return Auth::Denied(unauthorized(app));
    };
    let decoded = String::from_utf8_lossy(&decoded).into_owned();
    let (login, password) = decoded.split_once(':').unwrap_or((&decoded, ""));
    let found: Result<Option<Actor>, Error> = app.db.tx(|c| {
        if let Some(t) = tokens::find_by_value(c, password)? {
            if !t.active() {
                return Ok(None);
            }
            let Some(u) = User::find(c, t.user_id)? else {
                return Ok(None);
            };
            tokens::touch(c, &t)?;
            let api = t.has_scope("api");
            return Ok(Some(Actor {
                can_download: api
                    || t.has_scope("read_api")
                    || t.has_scope("read_repository")
                    || t.has_scope("write_repository"),
                can_push: api || t.has_scope("write_repository"),
                user: u,
            }));
        }
        let user = match User::find_by_username(c, login)? {
            Some(u) => Some(u),
            None => {
                let id: Option<i64> = c
                    .query_row(
                        "SELECT id FROM users WHERE lower(email) = lower(?1)",
                        [login],
                        |r| r.get(0),
                    )
                    .ok();
                match id {
                    Some(id) => User::find(c, id)?,
                    None => None,
                }
            }
        };
        Ok(user
            .filter(|u| users::check_password(u, password))
            .map(|u| Actor {
                user: u,
                can_download: true,
                can_push: true,
            }))
    });
    match found {
        Ok(Some(a)) => {
            if !a.user.active() {
                let msg = if a.user.state == "deactivated" {
                    "Your account has been deactivated by your administrator. Please log back in from a web browser to reactivate your account"
                        .to_string()
                } else {
                    "Your account has been blocked.".to_string()
                };
                return Auth::Denied(text(StatusCode::FORBIDDEN, msg));
            }
            Auth::Ok(a)
        }
        _ => Auth::Denied(unauthorized(app)),
    }
}

/// Splits `/group/sub/project.git/info/refs` into (`group/sub/project`, `info/refs`).
pub fn split_git_path(path: &str) -> Option<(String, String)> {
    let idx = path.find(".git/")?;
    let project = path[..idx].trim_start_matches('/').to_string();
    let rest = path[idx + 5..].to_string();
    if project.is_empty() {
        return None;
    }
    Some((project, rest))
}

pub async fn handle(
    app: Arc<App>,
    method: Method,
    path: &str,
    query: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some((project_path, action)) = split_git_path(path) else {
        return text(StatusCode::NOT_FOUND, "Not Found".into());
    };
    let service = url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "service")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let push = match action.as_str() {
        "info/refs" => service == "git-receive-pack",
        "git-upload-pack" => false,
        "git-receive-pack" => true,
        _ => return text(StatusCode::NOT_FOUND, "Not Found".into()),
    };
    if action == "info/refs" && method == Method::HEAD {
        return text(StatusCode::FORBIDDEN, String::new());
    }
    let auth = authenticate(&app, &headers);
    let project = match app
        .db
        .read(|c| Project::find_by_full_path(c, &project_path))
    {
        Ok(p) => p,
        Err(e) => return ApiError::from(e).into_response_text(),
    };
    let actor = match auth {
        Auth::Denied(r) => return r,
        Auth::None => {
            // Anonymous clones of public repositories only.
            let public_ok = !push
                && project.as_ref().is_some_and(|p| {
                    app.db
                        .read(|c| ProjectPerms::load(c, None, p).map(|perms| perms.read_code(p)))
                        .unwrap_or(false)
                });
            if !public_ok {
                return unauthorized(&app);
            }
            None
        }
        Auth::Ok(a) => Some(a),
    };
    let Some(project) = project else {
        return text(StatusCode::NOT_FOUND, NOT_FOUND.into());
    };
    if let Some(a) = &actor {
        let perms = match app
            .db
            .read(|c| ProjectPerms::load(c, Some(&a.user), &project))
        {
            Ok(p) => p,
            Err(e) => return ApiError::from(e).into_response_text(),
        };
        if !perms.read_project() {
            return text(StatusCode::NOT_FOUND, NOT_FOUND.into());
        }
        if push {
            if !a.can_push {
                return text(
                    StatusCode::FORBIDDEN,
                    "You are not allowed to upload code.".into(),
                );
            }
            if project.archived {
                return text(
                    StatusCode::FORBIDDEN,
                    "You can't push code to an archived project.".into(),
                );
            }
            if !perms.developer() {
                return text(
                    StatusCode::FORBIDDEN,
                    "You are not allowed to push code to this project.".into(),
                );
            }
        } else {
            if !a.can_download {
                return text(
                    StatusCode::FORBIDDEN,
                    "You are not allowed to download code.".into(),
                );
            }
            if !perms.read_code(&project) {
                return text(
                    StatusCode::FORBIDDEN,
                    "You are not allowed to download code from this project.".into(),
                );
            }
        }
    }
    let result = match action.as_str() {
        "info/refs" => advertise(&app, &project, &service),
        "git-upload-pack" => upload_pack(&app, &project, &body),
        _ => receive_pack(&app, &project, &actor.unwrap().user, &body),
    };
    match result {
        Ok(r) => r,
        Err(e) => ApiError::from(e).into_response_text(),
    }
}

impl ApiError {
    fn into_response_text(self) -> Response {
        let msg = self.body["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| self.body.to_string());
        text(
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            msg,
        )
    }
}

fn hidden_for_upload(name: &str) -> bool {
    !(name.starts_with("refs/heads/")
        || name.starts_with("refs/tags/")
        || name.starts_with("refs/merge-requests/"))
}

fn hidden_for_receive(name: &str) -> bool {
    !(name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
}

fn advertise(app: &App, project: &Project, service: &str) -> Result<Response, Error> {
    if service != "git-upload-pack" && service != "git-receive-pack" {
        return Ok(text(StatusCode::FORBIDDEN, "Unsupported service".into()));
    }
    let body = app.db.read(|c| {
        let r = repo::repo(c, project);
        let head = project.default_branch.clone();
        let hidden: &dyn Fn(&str) -> bool = if service == "git-upload-pack" {
            &hidden_for_upload
        } else {
            &hidden_for_receive
        };
        protocol::advertise(&r, service, head.as_deref(), hidden).map_err(Error::from)
    })?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            format!("application/x-{service}-advertisement"),
        )
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(body))
        .unwrap())
}

fn upload_pack(app: &App, project: &Project, body: &[u8]) -> Result<Response, Error> {
    let body = maybe_gunzip(body);
    let out = app
        .db
        .read(|c| protocol::upload_pack(&repo::repo(c, project), &body).map_err(Error::from))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-git-upload-pack-result")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(out))
        .unwrap())
}

fn maybe_gunzip(body: &[u8]) -> Vec<u8> {
    if body.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut out = vec![];
        if flate2::read::GzDecoder::new(body)
            .read_to_end(&mut out)
            .is_ok()
        {
            return out;
        }
    }
    body.to_vec()
}

/// Per-ref checks GitLab runs in its pre-receive hook. Returns the rejection message.
fn check_update(
    c: &rusqlite::Connection,
    base: &str,
    user: &User,
    project: &Project,
    u: &RefUpdate,
) -> Result<Option<String>, Error> {
    let r = Repo::new(c, project.id);
    if let Some(branch) = u.name.strip_prefix("refs/heads/") {
        let deletion = u.new.is_zero();
        let creation = u.old.is_zero();
        if deletion && project.default_branch.as_deref() == Some(branch) {
            return Ok(Some(
                "The default branch of a project cannot be deleted.".into(),
            ));
        }
        if !deletion {
            if branch.len() == 40 && branch.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Ok(Some(
                    "You cannot create a branch with a SHA-1 or SHA-256 branch name.".into(),
                ));
            }
            if !repo::valid_branch_name(branch) {
                return Ok(Some(
                    "You cannot create a branch with an invalid name.".into(),
                ));
            }
        }
        if repo::is_protected(c, project, branch)? {
            let empty = r.is_empty()?;
            let rules = repo::matching_rules(c, project, branch)?;
            if !creation
                && !deletion
                && !walk::is_ancestor(&r, &u.old, &u.new)?
                && !rules.iter().any(|x| x.allow_force_push)
            {
                return Ok(Some(
                    "You are not allowed to force push code to a protected branch on this project."
                        .into(),
                ));
            }
            if deletion {
                let perms = ProjectPerms::load(c, Some(user), project)?;
                if !perms.admin_project() {
                    return Ok(Some("You are not allowed to delete protected branches from this project. Only a project maintainer or owner can delete a protected branch.".into()));
                }
                return Ok(Some(
                    "You can only delete protected branches using the web interface.".into(),
                ));
            }
            if !repo::can_push_to_branch(c, user, project, branch)? {
                if empty {
                    return Ok(Some(format!(
                        "A default branch (e.g. main) does not yet exist for {}\nAsk a project Owner or Maintainer to create a default branch:\n\n  {}/{}/-/project_members\n",
                        project.full_path, base, project.full_path
                    )));
                }
                if creation {
                    return Ok(Some(
                        "You are not allowed to create protected branches on this project.".into(),
                    ));
                }
                return Ok(Some(
                    "You are not allowed to push code to protected branches on this project."
                        .into(),
                ));
            }
        }
        return Ok(None);
    }
    if let Some(tag) = u.name.strip_prefix("refs/tags/") {
        if !u.old.is_zero() && !u.new.is_zero() {
            let perms = ProjectPerms::load(c, Some(user), project)?;
            if !perms.admin_project() {
                return Ok(Some(
                    "You are not allowed to change existing tags on this project.".into(),
                ));
            }
        }
        if !u.new.is_zero() && project.default_branch.as_deref() == Some(tag) {
            return Ok(Some(
                "You cannot use default branch name to create a tag".into(),
            ));
        }
        // `TagCheck#protected_tag_checks`.
        if labrat_core::services::protected_tags::is_protected(c, project.id, tag)? {
            if !u.old.is_zero() && !u.new.is_zero() {
                return Ok(Some("Protected tags cannot be updated.".into()));
            }
            if u.new.is_zero() {
                let perms = ProjectPerms::load(c, Some(user), project)?;
                if !perms.admin_project() {
                    return Ok(Some("You are not allowed to delete protected tags from this project. Only a project maintainer or owner can delete a protected tag.".into()));
                }
                return Ok(Some("You can only delete protected tags using the web interface.".into()));
            }
            if !labrat_core::services::protected_tags::can_create(c, user, project, tag)? {
                return Ok(Some("You are not allowed to create this tag as it is protected.".into()));
            }
        }
        return Ok(None);
    }
    Ok(Some(
        "You are not allowed to push code to this project.".into(),
    ))
}

fn receive_pack(app: &App, project: &Project, user: &User, body: &[u8]) -> Result<Response, Error> {
    let body = maybe_gunzip(body);
    let req = protocol::parse_receive(&body).map_err(Error::from)?;
    let outcome: Result<(Option<String>, Vec<(String, Option<String>)>, String), Error> =
        app.db.tx(|c| {
            let r = repo::repo(c, project);
            let written = match labrat_git::pack::unpack(&r, &req.pack) {
                Ok(w) => w,
                Err(e) => {
                    let results = req
                        .updates
                        .iter()
                        .map(|u| (u.name.clone(), Some("unpacker error".to_string())))
                        .collect();
                    return Ok((Some(format!("{e}")), results, String::new()));
                }
            };
            // The pre-receive `LfsCheck` rejects the push when it brings LFS pointers.
            if repo::lfs_enabled(project) && written.iter().any(|o| r.blob(o).map(|b| repo::is_lfs_pointer(&b)).unwrap_or(false)) {
                let results = req.updates.iter().map(|x| (x.name.clone(), Some("pre-receive hook declined".to_string()))).collect();
                return Ok((None, results, format!("GitLab: {}\n", repo::LFS_MISSING)));
            }
            // GitLab's pre-receive hook rejects the whole push if any ref fails.
            for u in &req.updates {
                let current = r.ref_oid(&u.name)?.unwrap_or(Oid::ZERO);
                if current != u.old {
                    let results = req
                        .updates
                        .iter()
                        .map(|x| {
                            (
                                x.name.clone(),
                                Some(
                                    if x.name == u.name {
                                        "fetch first"
                                    } else {
                                        "atomic push failed"
                                    }
                                    .to_string(),
                                ),
                            )
                        })
                        .collect();
                    return Ok((None, results, String::new()));
                }
                if let Some(msg) = check_update(c, &app.config.external_url, user, project, u)? {
                    let results = req
                        .updates
                        .iter()
                        .map(|x| {
                            (
                                x.name.clone(),
                                Some("pre-receive hook declined".to_string()),
                            )
                        })
                        .collect();
                    let mut text = String::new();
                    for line in format!("GitLab: {msg}").lines() {
                        text.push_str(line);
                        text.push('\n');
                    }
                    return Ok((None, results, text));
                }
            }
            for u in &req.updates {
                if u.new.is_zero() {
                    r.delete_ref(&u.name)?;
                } else {
                    r.set_ref(&u.name, &u.new)?;
                }
            }
            // Push options run synchronously, before the MR links are printed.
            // `ci.skip`: the push's pipelines are skipped.
            let skip = req.push_options.iter().any(|o| o == "ci.skip" || o == "skip-ci");
            labrat_core::services::ci::PUSH_SKIPS_CI.with(|f| f.set(skip));
            let result = labrat_core::services::push::post_receive_with_options(c, &app.config, project, user, &req.updates, |c| {
                labrat_core::services::push_options::handle(c, &app.config, project, user, &req.updates, &req.push_options)
            });
            labrat_core::services::ci::PUSH_SKIPS_CI.with(|f| f.set(false));
            let messages = result?;
            let results = req.updates.iter().map(|u| (u.name.clone(), None)).collect();
            Ok((None, results, messages))
        });
    let (unpack_err, results, messages) = outcome?;
    let out = protocol::receive_response(&req, unpack_err.as_deref(), &results, &messages);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/x-git-receive-pack-result",
        )
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(out))
        .unwrap())
}
