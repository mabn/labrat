//! Markdown uploads (`lib/api/markdown_uploads.rb`): files attached to descriptions and comments.

use crate::ctx::{ApiError, ApiResult, Req, Resp};
use crate::find;
use crate::pagination::paginate;
use axum::Router;
use axum::response::IntoResponse;
use axum::routing::get;
use labrat_core::model::{Project, User};
use labrat_core::{App, Error, time};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/uploads", get(list).post(create))
        .route("/projects/{id}/uploads/{upload_id}", get(show).delete(destroy))
        .route("/projects/{id}/uploads/{secret}/{filename}", get(show_by_secret).delete(destroy_by_secret))
}

/// `FileUploader`'s sanitized file name.
fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect()
}

fn is_embeddable(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tiff" | "ico" | "webp" | "svg" | "mp4" | "m4v" | "mov" | "webm" | "ogv" | "mp3" | "oga" | "ogg" | "spx" | "wav"
    )
}

/// `Entities::ProjectUpload`.
fn upload_value(p: &Project, id: i64, secret: &str, filename: &str) -> Value {
    let url = format!("/uploads/{secret}/{filename}");
    let (alt, markdown) = if is_embeddable(filename) {
        let alt = filename.rsplit_once('.').map(|(b, _)| b).unwrap_or(filename).to_string();
        (alt.clone(), format!("![{alt}]({url})"))
    } else {
        (filename.to_string(), format!("[{filename}]({url})"))
    };
    json!({"id": id, "alt": alt, "url": url, "full_path": format!("/-/project/{}{url}", p.id), "markdown": markdown})
}

fn admin_project(c: &Connection, req: &Req) -> Result<Project, ApiError> {
    let (p, perms) = find::project(c, req, req.path_param("id"))?;
    if !(perms.admin || perms.level >= labrat_core::model::access::MAINTAINER) {
        return Err(Error::forbidden().into());
    }
    Ok(p)
}

async fn create(req: Req) -> ApiResult {
    let user = req.user()?.clone();
    let Some(file) = req.files.iter().find(|f| f.field == "file").cloned() else {
        return Err(Error::Param("file is missing".into()).into());
    };
    let app = req.app.clone();
    let body = app.db.tx(|c| {
        let (p, perms) = find::project(c, &req, req.path_param("id"))?;
        if !(perms.admin || perms.level >= labrat_core::model::access::REPORTER) {
            return Err(Error::forbidden().into());
        }
        let secret = hex::encode(rand::random::<[u8; 16]>());
        let filename = sanitize(&file.filename);
        c.execute(
            "INSERT INTO uploads (project_id, secret, filename, data, uploaded_by_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![p.id, secret, filename, file.data, user.id, time::now()],
        )?;
        Ok::<_, ApiError>(upload_value(&p, c.last_insert_rowid(), &secret, &filename))
    })?;
    Ok(Resp::created(body))
}

async fn list(req: Req) -> ApiResult {
    req.user()?;
    let app = req.app.clone();
    app.db.read(|c| {
        let p = admin_project(c, &req)?;
        let mut st = c.prepare("SELECT id, filename, length(data), created_at, uploaded_by_id FROM uploads WHERE project_id = ?1 ORDER BY id DESC")?;
        let rows: Vec<(i64, String, i64, i64, Option<i64>)> =
            st.query_map([p.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
        paginate(&req, rows, &[], |(id, name, size, created, by)| {
            let by = match by.map(|u| User::find(c, u)).transpose()?.flatten() {
                Some(u) => json!({"id": u.id, "username": u.username, "public_email": u.public_email, "name": u.name}),
                None => Value::Null,
            };
            Ok(json!({"id": id, "size": size, "filename": name, "created_at": time::iso(*created), "uploaded_by": by}))
        })
    })
}

fn file_response(data: Vec<u8>, filename: &str) -> axum::response::Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
        ],
        data,
    )
        .into_response()
}

fn not_found() -> ApiError {
    Error::Api(404, json!("404 Not found")).into()
}

fn fetch(req: &Req, by_secret: bool) -> Result<(i64, String, Vec<u8>), ApiError> {
    req.user()?;
    req.app.db.read(|c| {
        let p = admin_project(c, req)?;
        let row: Option<(i64, String, Vec<u8>)> = if by_secret {
            c.query_row(
                "SELECT id, filename, data FROM uploads WHERE project_id = ?1 AND secret = ?2 AND filename = ?3",
                params![p.id, req.path_param("secret"), req.path_param("filename")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()
        } else {
            let id: i64 = req.path_param("upload_id").parse().unwrap_or(-1);
            c.query_row("SELECT id, filename, data FROM uploads WHERE project_id = ?1 AND id = ?2", params![p.id, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .ok()
        };
        row.ok_or_else(not_found)
    })
}

async fn show(req: Req) -> Result<axum::response::Response, ApiError> {
    let (_, name, data) = fetch(&req, false)?;
    Ok(file_response(data, &name))
}

async fn show_by_secret(req: Req) -> Result<axum::response::Response, ApiError> {
    let (_, name, data) = fetch(&req, true)?;
    Ok(file_response(data, &name))
}

async fn destroy(req: Req) -> ApiResult {
    let (id, _, _) = fetch(&req, false)?;
    req.app.db.tx(|c| c.execute("DELETE FROM uploads WHERE id = ?1", [id]).map_err(labrat_core::Error::from))?;
    Ok(Resp::no_content())
}

async fn destroy_by_secret(req: Req) -> ApiResult {
    let (id, _, _) = fetch(&req, true)?;
    req.app.db.tx(|c| c.execute("DELETE FROM uploads WHERE id = ?1", [id]).map_err(labrat_core::Error::from))?;
    Ok(Resp::no_content())
}
