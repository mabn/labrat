//! Webhook delivery (`WebHookService`): POSTs queued payloads with GitLab's headers and
//! records each attempt in `web_hook_logs`.

use labrat_core::services::hooks::{Hook, uuid};
use labrat_core::{App, GITLAB_VERSION, time};
use rusqlite::params;
use serde_json::{Map, Value, json};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("http client")
});

static CLIENT_INSECURE: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(true)
        .build()
        .expect("http client")
});

/// `X-Gitlab-Event` for a hook name: `merge_request_hooks` -> `Merge Request Hook`.
pub fn event_header(hook: &Hook, scope: &str) -> String {
    if hook.kind == "SystemHook" {
        return "System Hook".into();
    }
    let singular = scope.strip_suffix('s').unwrap_or(scope);
    singular
        .split('_')
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Renders a custom webhook template: `{{a.b}}` replaced by JSON values (strings unquoted).
fn render_template(tpl: &str, data: &Value) -> Result<Value, String> {
    let mut out = String::new();
    let mut rest = tpl;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        let path = after[..j].trim();
        let mut v = data;
        let mut missing = false;
        for part in path.split('.') {
            match v.get(part) {
                Some(x) => v = x,
                None => {
                    missing = true;
                    break;
                }
            }
        }
        if missing {
            out.push_str("null");
        } else if let Value::String(s) = v {
            let enc = serde_json::to_string(s).unwrap();
            out.push_str(&enc[1..enc.len() - 1]);
        } else {
            out.push_str(&v.to_string());
        }
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    serde_json::from_str(&out).map_err(|e| format!("Error while parsing rendered custom webhook template: {e}"))
}

/// Ruby's `URI::MailTo::EMAIL_REGEXP`.
fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    let local_ok = !local.is_empty() && local.chars().all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c));
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    local_ok && domain.split('.').all(label_ok)
}

/// Replaces leaf strings that are whole email addresses with `[REDACTED]` (stored logs only).
fn redact_emails(v: &mut Value) {
    match v {
        Value::String(s) => {
            if is_email(s) {
                *s = "[REDACTED]".into();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(redact_emails),
        Value::Object(m) => m.values_mut().for_each(redact_emails),
        _ => {}
    }
}

pub struct Outcome {
    pub status: Option<u16>,
    pub body: String,
    pub error: Option<String>,
}

/// Sends one payload to one hook and logs it. Used by the job worker and by test deliveries.
pub async fn send(app: &Arc<App>, hook: &Hook, scope: &str, data: &Value, idempotency_key: &str) -> Outcome {
    let payload = match &hook.custom_webhook_template {
        Some(t) if !t.is_empty() => match render_template(t, data) {
            Ok(v) => v,
            Err(e) => {
                log(app, hook, scope, &[], &json!({}), None, "", 0.0, Some(&e), &e);
                return Outcome { status: None, body: String::new(), error: Some(e) };
            }
        },
        _ => data.clone(),
    };
    let mut headers: Vec<(String, String)> = hook.custom_headers.clone();
    let mut built = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), format!("GitLab/{GITLAB_VERSION}")),
        ("Idempotency-Key".to_string(), idempotency_key.to_string()),
        ("X-Gitlab-Event".to_string(), event_header(hook, scope)),
        ("X-Gitlab-Webhook-UUID".to_string(), uuid()),
        ("X-Gitlab-Instance".to_string(), app.config.external_url.clone()),
    ];
    if let Some(t) = hook.token.as_ref().filter(|t| !t.is_empty()) {
        built.push(("X-Gitlab-Token".to_string(), t.replace(['\n', '\r'], "")));
    }
    built.push(("X-Gitlab-Event-UUID".to_string(), uuid()));
    for (k, v) in built {
        headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(&k));
        headers.push((k, v));
    }
    let url = hook.interpolated_url();
    let client = if hook.enable_ssl_verification { &*CLIENT } else { &*CLIENT_INSECURE };
    let mut rb = client.post(&url).body(serde_json::to_vec(&payload).unwrap());
    if let Ok(parsed) = url::Url::parse(&url)
        && !parsed.username().is_empty() {
            let user = percent_encoding::percent_decode_str(parsed.username()).decode_utf8_lossy().into_owned();
            let pass = parsed.password().map(|p| percent_encoding::percent_decode_str(p).decode_utf8_lossy().into_owned());
            let mut clean = parsed.clone();
            let _ = clean.set_username("");
            let _ = clean.set_password(None);
            rb = client.post(clean.as_str()).body(serde_json::to_vec(&payload).unwrap()).basic_auth(user, pass);
        }
    for (k, v) in &headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    // Net::HTTP as used by GitLab closes the connection after each delivery.
    rb = rb.header("Connection", "close");
    let start = Instant::now();
    let result = rb.send().await;
    let duration = start.elapsed().as_secs_f64();
    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let resp_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .take(50)
                .map(|(k, v)| (capitalize(k.as_str()), v.to_str().unwrap_or("").chars().take(1024).collect()))
                .collect();
            let body = resp.text().await.unwrap_or_default();
            let short: String = body.chars().take(8192).collect();
            log(app, hook, scope, &headers, &payload, Some(&resp_headers), &short, duration, None, &status.to_string());
            Outcome { status: Some(status), body, error: None }
        }
        Err(e) => {
            let msg = e.to_string();
            log(app, hook, scope, &headers, &payload, None, "", duration, Some(&msg), "internal error");
            Outcome { status: None, body: String::new(), error: Some(msg) }
        }
    }
}

fn capitalize(name: &str) -> String {
    name.split('-')
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

#[allow(clippy::too_many_arguments)]
fn log(
    app: &Arc<App>,
    hook: &Hook,
    scope: &str,
    req_headers: &[(String, String)],
    data: &Value,
    resp_headers: Option<&[(String, String)]>,
    body: &str,
    duration: f64,
    error: Option<&str>,
    status: &str,
) {
    let mut rh = Map::new();
    for (k, v) in req_headers {
        let custom = hook.custom_headers.iter().any(|(ck, _)| ck == k);
        let value = if custom || k == "X-Gitlab-Token" { "[REDACTED]".to_string() } else { v.clone() };
        rh.insert(k.clone(), json!(value));
    }
    let mut stored = data.clone();
    redact_emails(&mut stored);
    let resp: Map<String, Value> = resp_headers.unwrap_or(&[]).iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let _ = app.db.tx(|c| {
        c.execute(
            "INSERT INTO web_hook_logs (web_hook_id, trigger, url, request_headers, request_data, response_headers, response_body,
                response_status, execution_duration, internal_error_message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                hook.id,
                scope,
                hook.url,
                Value::Object(rh).to_string(),
                stored.to_string(),
                Value::Object(resp).to_string(),
                body,
                status,
                duration,
                error,
                time::now()
            ],
        )?;
        Ok::<_, labrat_core::Error>(())
    });
}

/// Delivers one queued `web_hook` job.
pub async fn deliver(app: &Arc<App>, payload: &Value) {
    let Some(hook_id) = payload["hook_id"].as_i64() else { return };
    let hook = match app.db.read(|c| Hook::find(c, hook_id)) {
        Ok(Some(h)) => h,
        _ => return,
    };
    let scope = payload["hook_name"].as_str().unwrap_or("push_hooks");
    let key = payload["idempotency_key"].as_str().map(str::to_string).unwrap_or_else(uuid);
    send(app, &hook, scope, &payload["data"], &key).await;
}
