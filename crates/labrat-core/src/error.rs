use serde_json::{Map, Value, json};

/// Errors carry enough information to render GitLab's exact error bodies.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `{"message":"404 <Resource> Not Found"}`; `None` renders `404 Not Found`.
    #[error("404 {0:?} Not Found")]
    NotFound(Option<&'static str>),
    /// `{"message":"401 Unauthorized"}`, optionally with ` - reason`.
    #[error("401 Unauthorized")]
    Unauthorized(Option<String>),
    /// `{"message":"403 Forbidden"}`, optionally with ` - reason`.
    #[error("403 Forbidden")]
    Forbidden(Option<String>),
    /// `{"message":"400 Bad request - reason"}`.
    #[error("400 Bad request")]
    BadRequest(Option<String>),
    /// Grape parameter validation: `{"error":"..."}` with status 400.
    #[error("param error: {0}")]
    Param(String),
    /// ActiveModel validation errors: `{"message":{"field":["msg"]}}`.
    #[error("validation failed")]
    Validation(Vec<(String, String)>),
    /// Any other `{"message": <value>}` error with an explicit status.
    #[error("api error {0}")]
    Api(u16, Value),
    /// A response whose body is exactly the given value.
    #[error("api error {0}")]
    Body(u16, Value),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Internal(e.into())
    }
}

impl From<labrat_git::Error> for Error {
    fn from(e: labrat_git::Error) -> Self {
        match e {
            labrat_git::Error::Db(d) => Error::Internal(d.into()),
            other => Error::Internal(anyhow::anyhow!(other.to_string())),
        }
    }
}

impl Error {
    pub fn not_found(resource: &'static str) -> Self {
        Error::NotFound(Some(resource))
    }

    pub fn forbidden() -> Self {
        Error::Forbidden(None)
    }

    pub fn message(status: u16, msg: impl Into<String>) -> Self {
        Error::Api(status, Value::String(msg.into()))
    }

    pub fn validation(field: &str, msg: &str) -> Self {
        Error::Validation(vec![(field.to_string(), msg.to_string())])
    }

    pub fn status(&self) -> u16 {
        match self {
            Error::NotFound(_) => 404,
            Error::Unauthorized(_) => 401,
            Error::Forbidden(_) => 403,
            Error::BadRequest(_) | Error::Param(_) | Error::Validation(_) => 400,
            Error::Api(s, _) | Error::Body(s, _) => *s,
            Error::Internal(_) => 500,
        }
    }

    pub fn body(&self) -> Value {
        fn with_reason(base: &str, reason: &Option<String>) -> Value {
            match reason {
                Some(r) => json!({ "message": format!("{base} - {r}") }),
                None => json!({ "message": base }),
            }
        }
        match self {
            Error::NotFound(Some(r)) => json!({ "message": format!("404 {r} Not Found") }),
            Error::NotFound(None) => json!({ "message": "404 Not Found" }),
            Error::Unauthorized(r) => with_reason("401 Unauthorized", r),
            Error::Forbidden(r) => with_reason("403 Forbidden", r),
            Error::BadRequest(r) => with_reason("400 Bad request", r),
            Error::Param(m) => json!({ "error": m }),
            Error::Validation(errs) => {
                let mut map = Map::new();
                for (field, msg) in errs {
                    let entry = map
                        .entry(field.clone())
                        .or_insert_with(|| Value::Array(vec![]));
                    entry
                        .as_array_mut()
                        .unwrap()
                        .push(Value::String(msg.clone()));
                }
                json!({ "message": map })
            }
            Error::Api(_, v) => json!({ "message": v }),
            Error::Body(_, v) => v.clone(),
            Error::Internal(_) => json!({ "message": "500 Internal Server Error" }),
        }
    }
}
