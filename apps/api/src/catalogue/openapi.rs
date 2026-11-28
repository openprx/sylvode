//! `GET /api/v1/openapi.json`: an `OpenAPI` 3.1 description generated from the operation catalogue.
//!
//! The catalogue is the only source: every entry becomes one operation, so the document cannot list
//! a route that is not catalogued or miss one that is. The catalogue knows no request or response
//! shapes, so bodies are described by generic object schemas; the document is a map of operations,
//! methods, paths, parameters, risk and who may call them, not a typed client contract.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use serde_json::{Map, Value, json};

use super::types::{Entry, Exposure, Method, Scope};

/// The tag of the operations of an area; the area is the first part of the capability id.
fn area(entry: &Entry) -> &'static str {
    entry.id_parts().map_or("other", |(area, _, _)| area)
}

/// The names in `{...}` segments of a path template, in order.
fn path_parameters(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|name| name.trim_start_matches('*'))
        .collect()
}

const fn method_key(method: Method) -> &'static str {
    match method {
        Method::Get => "get",
        Method::Post => "post",
        Method::Put => "put",
        Method::Patch => "patch",
        Method::Delete => "delete",
    }
}

/// The security requirement alternatives of an entry: one per channel that can authenticate it.
/// A public route and a public site route need none (`[]`).
fn security(entry: &Entry) -> Vec<Value> {
    if matches!(entry.scope, Scope::Public | Scope::Site(_)) {
        return Vec::new();
    }
    let mut alternatives = Vec::new();
    if entry.exposure.contains(Exposure::CONSOLE) {
        alternatives.push(json!({ "sessionCookie": [] }));
        alternatives.push(json!({ "bearerJwt": [] }));
    }
    if entry.exposure.contains(Exposure::BOT) {
        alternatives.push(json!({ "botToken": [] }));
    }
    if entry.exposure.contains(Exposure::OAUTH) {
        alternatives.push(json!({ "oauth2": [] }));
    }
    alternatives
}

fn operation(entry: &Entry) -> Value {
    let mut operation = Map::new();
    operation.insert("operationId".into(), json!(entry.op));
    operation.insert("tags".into(), json!([area(entry)]));
    operation.insert("summary".into(), json!(entry.capability()));
    let parameters: Vec<Value> = path_parameters(entry.path)
        .into_iter()
        .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }))
        .collect();
    if !parameters.is_empty() {
        operation.insert("parameters".into(), Value::Array(parameters));
    }
    if matches!(entry.method, Method::Post | Method::Put | Method::Patch) {
        operation.insert(
            "requestBody".into(),
            json!({ "content": { "application/json": { "schema": { "type": "object" } } } }),
        );
    }
    operation.insert("security".into(), Value::Array(security(entry)));
    operation.insert(
        "responses".into(),
        json!({
            "200": {
                "description": "The response envelope; coded refusals are also answered with HTTP 200 and the code in the body.",
                "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Envelope" } } }
            },
            "default": {
                "description": "An error status.",
                "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Envelope" } } }
            }
        }),
    );
    operation.insert("x-sylvode-capability".into(), json!(entry.capability()));
    operation.insert("x-sylvode-exposure".into(), json!(entry.exposure.names()));
    operation.insert("x-sylvode-risk".into(), json!(entry.risk.as_str()));
    operation.insert("x-sylvode-scope".into(), json!(entry.scope.name()));
    operation.insert("x-sylvode-stability".into(), json!(entry.stability.as_str()));
    if entry.comment {
        operation.insert("x-sylvode-comment-level".into(), json!(true));
    }
    Value::Object(operation)
}

/// The `OpenAPI` 3.1 document for the entries that pass `filter` (all when `None`). Paths and tags are
/// sorted, so the same catalogue always yields the same bytes.
#[must_use]
pub fn document(filter: Option<Exposure>) -> Value {
    let mut paths: BTreeMap<&'static str, Map<String, Value>> = BTreeMap::new();
    let mut tags: BTreeSet<&'static str> = BTreeSet::new();
    for entry in super::entries().filter(|entry| filter.is_none_or(|exposure| entry.exposure.contains(exposure))) {
        tags.insert(area(entry));
        paths
            .entry(entry.path)
            .or_default()
            .insert(method_key(entry.method).to_string(), operation(entry));
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Sylvode API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Generated from the operation catalogue. Request and response bodies are generic objects; the catalogue does not describe their shape.",
        },
        "tags": tags.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
        "paths": paths,
        "components": {
            "securitySchemes": {
                "sessionCookie": { "type": "apiKey", "in": "cookie", "name": "access_token" },
                "bearerJwt": { "type": "http", "scheme": "bearer", "bearerFormat": "JWT" },
                "botToken": { "type": "http", "scheme": "bearer", "description": "A bot token." },
                "oauth2": {
                    "type": "oauth2",
                    "description": "Delegated access through the Sylvode authorization server; only routes with the oauth exposure.",
                    "flows": { "authorizationCode": {
                        "authorizationUrl": "/oauth/authorize",
                        "tokenUrl": "/api/v1/oauth/token",
                        "scopes": {}
                    } }
                }
            },
            "schemas": {
                "Envelope": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "integer" },
                        "message": { "type": "string" },
                        "data": {},
                        "error_code": { "type": "string" },
                        "details": { "type": "object" }
                    },
                    "required": ["code", "message"]
                }
            }
        }
    })
}

/// `GET /api/v1/openapi.json`, behind the bot-or-user authentication.
#[allow(clippy::unused_async)]
pub async fn serve() -> Json<Value> {
    Json(document(None))
}
