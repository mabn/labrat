//! Short-lived response caches that GitLab applies to some endpoints (`cache_action`), so
//! results are stale in the same way.

use crate::ctx::Resp;
use axum::http::StatusCode;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Cached {
    status: u16,
    body: Option<Value>,
    headers: Vec<(String, String)>,
}

impl Cached {
    pub fn into_resp(self) -> Resp {
        Resp {
            status: StatusCode::from_u16(self.status).unwrap(),
            body: self.body,
            headers: self.headers,
        }
    }
}

static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Cached)>>> = LazyLock::new(Default::default);

pub fn get(key: &str) -> Option<Cached> {
    let mut c = CACHE.lock();
    match c.get(key) {
        Some((until, v)) if *until > Instant::now() => Some(v.clone()),
        Some(_) => {
            c.remove(key);
            None
        }
        None => None,
    }
}

pub fn put(key: &str, resp: &Resp, ttl: Duration) {
    let v = Cached {
        status: resp.status.as_u16(),
        body: resp.body.clone(),
        headers: resp.headers.clone(),
    };
    CACHE
        .lock()
        .insert(key.to_string(), (Instant::now() + ttl, v));
}

/// Drops everything (used by the labrat reset endpoint).
pub fn clear() {
    CACHE.lock().clear();
}
