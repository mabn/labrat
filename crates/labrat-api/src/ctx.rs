//! The per-request context: parameters, the authenticated user and response helpers.

use crate::params::{Params, parse_query};
use axum::extract::{FromRequest, FromRequestParts, OriginalUri, RawPathParams, Request};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use labrat_core::model::{Token, User};
use labrat_core::services::tokens;
use labrat_core::{App, Error, Result};
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub struct Req {
    pub app: Arc<App>,
    pub method: Method,
    pub path: String,
    pub query: String,
    pub headers: HeaderMap,
    pub params: Params,
    pub path_params: Vec<(String, String)>,
    /// File parts of a multipart request.
    pub files: Vec<crate::params::UploadedFile>,
    pub user: Option<User>,
    pub token: Option<Token>,
    /// Set when the request authenticated with a token that failed (expired, revoked, ...).
    auth_error: Option<ApiError>,
}

impl Req {
    /// A copy of this request with other parameters, to reuse REST filters from GraphQL.
    pub fn with_params(&self, params: Params) -> Req {
        Req {
            app: self.app.clone(),
            method: self.method.clone(),
            path: self.path.clone(),
            query: String::new(),
            headers: self.headers.clone(),
            params,
            path_params: vec![],
            files: vec![],
            user: self.user.clone(),
            token: self.token.clone(),
            auth_error: None,
        }
    }

    /// `check_unmodified_since!`: 412 when the resource changed after `If-Unmodified-Since`.
    pub fn check_unmodified_since(&self, last_modified_ms: i64) -> Result<(), ApiError> {
        let Some(h) = self.headers.get("if-unmodified-since").and_then(|v| v.to_str().ok()) else { return Ok(()) };
        let since = chrono::DateTime::parse_from_rfc2822(h)
            .map(|t| t.timestamp_millis())
            .ok()
            .or_else(|| labrat_core::time::parse_time(h));
        match since {
            Some(t) if last_modified_ms > t => Err(Error::message(412, "412 Precondition Failed").into()),
            _ => Ok(()),
        }
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn path_param(&self, name: &str) -> &str {
        self.path_params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    /// The current user, or 401 if the request is anonymous.
    pub fn user(&self) -> Result<&User, ApiError> {
        if let Some(e) = &self.auth_error {
            return Err(e.clone());
        }
        self.user
            .as_ref()
            .ok_or_else(|| Error::Unauthorized(None).into())
    }

    /// The current user if any; fails only if a supplied token was rejected.
    pub fn maybe_user(&self) -> Result<Option<&User>, ApiError> {
        if let Some(e) = &self.auth_error {
            return Err(e.clone());
        }
        Ok(self.user.as_ref())
    }

    pub fn admin(&self) -> Result<&User, ApiError> {
        let u = self.user()?;
        if u.admin {
            Ok(u)
        } else {
            Err(Error::forbidden().into())
        }
    }

    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.admin)
    }

    pub fn url(&self, path: &str) -> String {
        self.app.config.url(path)
    }
}

/// Error wrapper that renders GitLab's error bodies.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub body: Value,
    pub headers: Vec<(String, String)>,
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        if let Error::Internal(err) = &e {
            tracing::error!("internal error: {err:#}");
        }
        ApiError {
            status: e.status(),
            body: e.body(),
            headers: vec![],
        }
    }
}

impl From<labrat_git::Error> for ApiError {
    fn from(e: labrat_git::Error) -> Self {
        Error::from(e).into()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Error::from(e).into()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut r = json_response(StatusCode::from_u16(self.status).unwrap(), &self.body);
        for (k, v) in self.headers {
            if let (Ok(k), Ok(v)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                r.headers_mut().insert(k, v);
            }
        }
        r
    }
}

pub type ApiResult = Result<Resp, ApiError>;

/// A successful API response.
pub struct Resp {
    pub status: StatusCode,
    pub body: Option<Value>,
    pub headers: Vec<(String, String)>,
}

impl Resp {
    pub fn ok(body: Value) -> Resp {
        Resp {
            status: StatusCode::OK,
            body: Some(body),
            headers: vec![],
        }
    }

    pub fn created(body: Value) -> Resp {
        Resp {
            status: StatusCode::CREATED,
            body: Some(body),
            headers: vec![],
        }
    }

    pub fn status(status: u16, body: Value) -> Resp {
        Resp {
            status: StatusCode::from_u16(status).unwrap(),
            body: Some(body),
            headers: vec![],
        }
    }

    pub fn no_content() -> Resp {
        Resp {
            status: StatusCode::NO_CONTENT,
            body: None,
            headers: vec![],
        }
    }

    pub fn header(mut self, k: &str, v: impl Into<String>) -> Resp {
        self.headers.push((k.to_string(), v.into()));
        self
    }
}

impl IntoResponse for Resp {
    fn into_response(self) -> Response {
        let mut r = match &self.body {
            Some(b) => json_response(self.status, b),
            None => {
                let mut r = Response::new(axum::body::Body::empty());
                *r.status_mut() = self.status;
                r
            }
        };
        for (k, v) in self.headers {
            if let (Ok(k), Ok(v)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                r.headers_mut().append(k, v);
            }
        }
        r
    }
}

pub fn json_response(status: StatusCode, body: &Value) -> Response {
    let mut r = Response::new(axum::body::Body::from(serde_json::to_vec(body).unwrap()));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

fn token_from_request(headers: &HeaderMap, params: &Params) -> Option<String> {
    if let Some(v) = headers.get("private-token").and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    if let Some(v) = params
        .str("private_token")
        .or_else(|| params.str("access_token"))
    {
        return Some(v);
    }
    if let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let mut parts = v.splitn(2, ' ');
        let scheme = parts.next().unwrap_or("");
        if scheme.eq_ignore_ascii_case("bearer") {
            return parts.next().map(|t| t.trim().to_string());
        }
    }
    None
}

fn oauth_error(status: u16, error: &str, description: &str) -> ApiError {
    let mut body = Map::new();
    body.insert("error".into(), json!(error));
    body.insert("error_description".into(), json!(description));
    ApiError {
        status,
        body: Value::Object(body),
        headers: vec![(
            "WWW-Authenticate".into(),
            format!(
                "Bearer realm=\"Protected by OAuth 2.0\", error=\"{error}\", error_description=\"{description}\""
            ),
        )],
    }
}

impl FromRequest<Arc<App>> for Req {
    type Rejection = ApiError;

    async fn from_request(req: Request, app: &Arc<App>) -> Result<Self, Self::Rejection> {
        let (mut parts, body) = req.into_parts();
        let path_params: Vec<(String, String)> =
            match RawPathParams::from_request_parts(&mut parts, app).await {
                Ok(p) => p
                    .iter()
                    .map(|(k, v)| {
                        let decoded = percent_encoding::percent_decode_str(v)
                            .decode_utf8_lossy()
                            .into_owned();
                        (k.to_string(), decoded)
                    })
                    .collect(),
                Err(_) => vec![],
            };
        let query = parts.uri.query().unwrap_or("").to_string();
        let mut map = Map::new();
        parse_query(&query, &mut map);
        let mut files = vec![];
        // Omnibus nginx accepts bodies up to `client_max_body_size 250m`; axum's default (2 MB)
        // would turn a large commit or GraphQL document into a bare 400.
        let bytes = axum::body::to_bytes(body, 250 * 1024 * 1024)
            .await
            .map_err(|_| ApiError::from(Error::BadRequest(None)))?;
        // Rack leaves the body of a HEAD request unread.
        if !bytes.is_empty() && parts.method != Method::HEAD {
            let ctype = parts
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if ctype.starts_with("application/json") {
                match serde_json::from_slice::<Value>(&bytes) {
                    Ok(Value::Object(o)) => map.extend(o),
                    // Rails puts non-object JSON bodies under `_json` (batched GraphQL).
                    Ok(other) => {
                        map.insert("_json".into(), other);
                    }
                    Err(_) => {
                        return Err(ApiError {
                            status: 400,
                            body: json!({"error": "The provided content-type 'application/json' is not supported."}),
                            headers: vec![],
                        });
                    }
                }
            } else if ctype.starts_with("multipart/form-data") {
                files = crate::params::parse_multipart(ctype, &bytes, &mut map);
            } else {
                parse_query(&String::from_utf8_lossy(&bytes), &mut map);
            }
        }
        for (k, v) in &path_params {
            map.insert(k.clone(), Value::String(v.clone()));
        }
        let params = Params { map };
        let mut r = Req {
            app: app.clone(),
            method: parts.method.clone(),
            path: parts
                .extensions
                .get::<OriginalUri>()
                .map(|u| u.0.path().to_string())
                .unwrap_or_else(|| parts.uri.path().to_string()),
            query,
            headers: parts.headers.clone(),
            params,
            path_params,
            files,
            user: None,
            token: None,
            auth_error: None,
        };
        r.authenticate()?;
        Ok(r)
    }
}

impl Req {
    fn authenticate(&mut self) -> Result<(), ApiError> {
        let Some(value) = token_from_request(&self.headers, &self.params) else {
            if self.params.str("sudo").is_some() || self.headers.contains_key("sudo") {
                return Err(Error::Unauthorized(None).into());
            }
            return Ok(());
        };
        let app = self.app.clone();
        let (user, token) = app.db.tx(|c| {
            let Some(token) = tokens::find_by_value(c, &value)? else {
                return Ok::<_, Error>((None, None));
            };
            let mut user = User::find(c, token.user_id)?;
            if token.active() {
                tokens::touch(c, &token)?;
                if let Some(u) = &user
                    && u.active() {
                        labrat_core::services::users::touch_activity(c, u)?;
                        user = User::find(c, u.id)?;
                    }
            }
            Ok((user, Some(token)))
        })?;
        let (Some(user), Some(token)) = (user, token) else {
            return Err(Error::Unauthorized(None).into());
        };
        if token.revoked {
            return Err(oauth_error(
                401,
                "invalid_token",
                "Token was revoked. You have to re-authorize from the user.",
            ));
        }
        if token.expired() {
            return Err(oauth_error(
                401,
                "invalid_token",
                "Token is expired. You can either do re-authorization or token refresh.",
            ));
        }
        match user.state.as_str() {
            "active" => {}
            "blocked" | "ldap_blocked" => {
                return Err(Error::Forbidden(Some("Your account has been blocked.".into())).into());
            }
            "blocked_pending_approval" => {
                return Err(Error::Forbidden(Some(
                    "Your account is pending approval from your administrator and hence blocked."
                        .into(),
                ))
                .into());
            }
            "deactivated" => {
                return Err(Error::Forbidden(Some(format!(
                    "Your account has been deactivated by your administrator. Please log back in from a web browser to reactivate your account at {}",
                    self.app.config.external_url
                )))
                .into());
            }
            _ => return Err(Error::Unauthorized(None).into()),
        }
        self.check_scopes(&token)?;
        self.user = Some(user);
        self.token = Some(token);
        self.sudo()?;
        Ok(())
    }

    /// API-wide scope rules: `api` allows everything, `read_api` allows reads; some API
    /// classes register extra scopes (e.g. `read_user` on the users API).
    fn check_scopes(&self, token: &Token) -> Result<(), ApiError> {
        let read = self.method == Method::GET || self.method == Method::HEAD;
        let p = self.path.trim_start_matches("/api/v4");
        if p == "/personal_access_tokens/self" || p.starts_with("/personal_access_tokens/self/") {
            return Ok(());
        }
        let users_api =
            p == "/user" || p.starts_with("/user/") || p == "/users" || p.starts_with("/users/");
        let ai_api = users_api
            || p == "/projects"
            || p.starts_with("/projects/")
            || p == "/groups"
            || p.starts_with("/groups/");
        if token.has_scope("api") || (read && token.has_scope("read_api")) {
            return Ok(());
        }
        if users_api && read && token.has_scope("read_user") {
            return Ok(());
        }
        // Registered scopes in registration order: the endpoint class first, then API::API's.
        let generic_members = p.contains("/members") || p.starts_with("/personal_access_tokens");
        let scopes = if users_api {
            "read_user ai_workflows api read_api"
        } else if ai_api && !generic_members {
            "ai_workflows api read_api"
        } else {
            "api read_api"
        };
        let mut body = Map::new();
        body.insert("error".into(), json!("insufficient_scope"));
        body.insert(
            "error_description".into(),
            json!("The request requires higher privileges than provided by the access token."),
        );
        body.insert("scope".into(), json!(scopes));
        Err(ApiError {
            status: 403,
            body: Value::Object(body),
            headers: vec![],
        })
    }

    fn sudo(&mut self) -> Result<(), ApiError> {
        let ident = self.params.str("sudo").or_else(|| {
            self.headers
                .get("sudo")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        });
        let Some(ident) = ident else { return Ok(()) };
        let user = self.user.as_ref().unwrap();
        if !user.admin {
            return Err(Error::Forbidden(Some("Must be admin to use sudo".into())).into());
        }
        if !self.token.as_ref().unwrap().has_scope("sudo") {
            let mut body = Map::new();
            body.insert("error".into(), json!("insufficient_scope"));
            body.insert(
                "error_description".into(),
                json!("The request requires higher privileges than provided by the access token."),
            );
            body.insert("scope".into(), json!("sudo"));
            return Err(ApiError {
                status: 403,
                body: Value::Object(body),
                headers: vec![],
            });
        }
        let found = self.app.db.read(|c| match ident.parse::<i64>() {
            Ok(id) => User::find(c, id),
            Err(_) => User::find_by_username(c, &ident),
        })?;
        match found {
            Some(u) => {
                self.user = Some(u);
                Ok(())
            }
            None => Err(Error::Api(
                404,
                json!(format!("404 User with ID or username '{ident}' Not Found")),
            )
            .into()),
        }
    }
}
