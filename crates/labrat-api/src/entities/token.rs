//! `PersonalAccessToken`, `PersonalAccessTokenWithToken`, `PersonalAccessTokenWithLastUsedIps`.

use labrat_core::model::Token;
use labrat_core::time;
use serde_json::{Map, Value, json};

pub fn token(t: &Token) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(t.id));
    m.insert("name".into(), json!(t.name));
    m.insert("revoked".into(), json!(t.revoked));
    m.insert("created_at".into(), json!(time::iso(t.created_at)));
    m.insert("description".into(), json!(t.description));
    m.insert("scopes".into(), json!(t.scopes));
    m.insert("user_id".into(), json!(t.user_id));
    m.insert("last_used_at".into(), json!(t.last_used_at.map(time::iso)));
    m.insert("active".into(), json!(t.active()));
    m.insert("granular".into(), json!(false));
    m.insert("expires_at".into(), json!(t.expires_at));
    if t.impersonation {
        m.insert("impersonation".into(), json!(true));
    }
    m
}

pub fn with_last_used_ips(t: &Token) -> Value {
    let mut m = token(t);
    m.insert(
        "last_used_ips".into(),
        if t.last_used_at.is_some() {
            json!(["127.0.0.1"])
        } else {
            json!([])
        },
    );
    Value::Object(m)
}

pub fn with_token(t: &Token, value: &str) -> Value {
    let mut m = token(t);
    m.insert("token".into(), json!(value));
    Value::Object(m)
}
