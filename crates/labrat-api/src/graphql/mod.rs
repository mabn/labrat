//! `POST/GET /api/graphql`.

mod analysis;
pub mod approvals;
mod did_you_mean;
pub mod exec;
pub mod mr_filters;
pub mod resolvers;
pub mod schema;
pub mod validate;

use crate::ctx::{ApiResult, Req, Resp};
use async_graphql_parser::types::OperationType;
use axum::Router;
use axum::routing::get;
use labrat_core::App;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/graphql", get(handle).post(handle))
}

/// One request of a (possibly batched) GraphQL call.
fn run_one(req: &Req, body: &Value) -> Value {
    let query = body["query"].as_str().unwrap_or_default();
    let variables: Map<String, Value> = match &body["variables"] {
        Value::Object(m) => m.clone(),
        Value::String(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or_default(),
        _ => Map::new(),
    };
    let op_name = body["operationName"].as_str().filter(|s| !s.is_empty());
    if query.trim().is_empty() {
        return json!({"errors": [{"message": "Unexpected end of document", "locations": []}]});
    }
    let doc = match async_graphql_parser::parse_query(query) {
        Ok(d) => d,
        Err(e) => return validate::parse_error(query, &e),
    };
    let ops: Vec<_> = doc.operations.iter().collect();
    let op = match op_name {
        Some(n) => ops.iter().find(|(name, _)| name.map(|x| x.as_str()) == Some(n)).map(|(_, o)| *o),
        None if ops.len() == 1 => Some(ops[0].1),
        None => None,
    };
    let Some(op) = op else {
        let msg = match op_name {
            Some(n) => format!("No operation named \"{n}\""),
            None => "An operation name is required".to_string(),
        };
        return json!({"errors": [{"message": msg}]});
    };
    if let Some(errors) = validate::validate(query, &doc, op, &variables) {
        return json!({"errors": errors});
    }
    if let Some(errors) = analysis::analyze(&doc, op, &variables, req.user.as_ref()) {
        return json!({"errors": errors});
    }
    exec::QUERY.with(|q| *q.borrow_mut() = query.to_string());
    let mutation = op.node.ty == OperationType::Mutation;
    let app = req.app.clone();
    let run = |c: &rusqlite::Connection| -> Value {
        if mutation {
            exec::run(c, req, &doc, op, variables.clone(), &resolvers::Mutation)
        } else {
            exec::run(c, req, &doc, op, variables.clone(), &resolvers::Query)
        }
    };
    let out = if mutation {
        app.db.tx(|c| Ok::<_, labrat_core::Error>(run(c))).unwrap_or_else(|e| json!({"errors": [{"message": e.to_string()}]}))
    } else {
        app.db.read(|c| Ok::<_, labrat_core::Error>(run(c))).unwrap_or_else(|e| json!({"errors": [{"message": e.to_string()}]}))
    };
    if mutation {
        crate::jobs::kick(&req.app);
    }
    out
}

/// `GraphqlController#introspection_query?`: such requests get GitLab's pre-generated schema file,
/// whatever the query text asks for.
fn is_introspection(body: &Value) -> bool {
    let q = body["query"].as_str().unwrap_or_default();
    match body.get("operationName") {
        Some(v) => v.as_str() == Some("IntrospectionQuery"),
        None => q.contains("__schema") || (q.contains("__type") && !q.contains("__typename")),
    }
}

fn json_utf8(status: u16, body: Vec<u8>) -> axum::response::Response {
    let mut r = axum::response::Response::new(axum::body::Body::from(body));
    *r.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
    r.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    r
}

/// `GraphqlController::MAX_QUERY_SIZE` and its extended allowance.
const MAX_QUERY_SIZE: usize = 10_000;
const EXTENDED_MAX_QUERY_SIZE: usize = 12_000;
const EXTENDED_QUERY_SIZE_ALLOWLIST: &[&str] = &[
    "workItemById",
    "namespaceWorkItem",
    "workItemUpdate",
    "createWorkItem",
    "workItemConvert",
    "addLinkedItems",
    "workItemUpdated",
    "getWorkItemsFull",
    "getWorkItemsFullEE",
    "getWorkItemsSlim",
    "getWorkItemsSlimEE",
];

async fn handle(req: Req) -> axum::response::Response {
    let edition = req.app.config.edition;
    schema::with_edition(edition, || handle_sync(req))
}

fn handle_sync(req: Req) -> axum::response::Response {
    // GET carries the request in the query string; POST in the JSON body (or form params).
    let raw = req.params.raw_all();
    if req.maybe_user().is_err() {
        return json_utf8(401, br#"{"errors":[{"message":"Invalid token"}]}"#.to_vec());
    }
    // `limit_query_size`: 10,000 characters (12,000 for a few work-item operations); a
    // multiplex counts all its queries together.
    let chars = |b: &Value| b["query"].as_str().map(|q| q.chars().count()).unwrap_or(0);
    let too_large = match &raw {
        Value::Array(items) => items.iter().map(chars).sum::<usize>() > MAX_QUERY_SIZE,
        b => {
            let extended = b["operationName"].as_str().is_some_and(|n| EXTENDED_QUERY_SIZE_ALLOWLIST.contains(&n));
            chars(b) > if extended { EXTENDED_MAX_QUERY_SIZE } else { MAX_QUERY_SIZE }
        }
    };
    if too_large {
        return json_utf8(422, br#"{"errors":[{"message":"Query too large"}]}"#.to_vec());
    }
    let body = match raw {
        Value::Array(items) => {
            let results: Vec<Value> = items.iter().map(|b| run_one(&req, b)).collect();
            return json_utf8(200, serde_json::to_vec(&Value::Array(results)).unwrap());
        }
        other => other,
    };
    let is_get = req.method == axum::http::Method::GET || req.method == axum::http::Method::HEAD;
    if is_get {
        let q = body["query"].as_str().unwrap_or_default();
        let mutation = match async_graphql_parser::parse_query(q) {
            Ok(doc) => doc.operations.iter().any(|(_, o)| o.node.ty == OperationType::Mutation),
            Err(_) => true,
        };
        if mutation {
            return json_utf8(422, br#"{"errors":[{"message":"Mutations are forbidden in GET requests"}]}"#.to_vec());
        }
    }
    if is_introspection(&body) {
        return json_utf8(200, schema::introspection_bytes().to_vec());
    }
    resolvers::INTERNAL_ERROR.with(|f| f.set(false));
    let out = run_one(&req, &body);
    if resolvers::INTERNAL_ERROR.with(|f| f.replace(false)) {
        // An exception inside a resolver (e.g. a cursor GitLab can't apply) is a 500.
        return json_utf8(500, br#"{"errors":[{"message":"Internal server error"}]}"#.to_vec());
    }
    json_utf8(200, serde_json::to_vec(&out).unwrap())
}

#[allow(dead_code)]
fn _unused(_: ApiResult, _: Resp) {}
