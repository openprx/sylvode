//! Shared helpers of the tool tests: placeholder arguments built from a tool's schema, and an
//! in-process Sylvode API stand-in that records every request.

use crate::client::{OpenPrClient, test_api};
use crate::protocol::ToolDefinition;
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;

pub const UUID: &str = "11111111-1111-4111-8111-111111111111";

/// A minimal valid value for a schema node.
pub fn placeholder(schema: &Value, name: &str) -> Value {
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && let Some(first) = options.first()
    {
        return first.clone();
    }
    if let Some(options) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        && let Some(first) = options.first()
    {
        return placeholder(first, name);
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        return match pattern {
            r"^[A-Za-z0-9]+-[0-9]+$" => json!("PRX-1"),
            r"^[a-z_]+(\.[a-z_]+)+$" => json!("a.b"),
            // Both UUID shapes.
            _ => json!(UUID),
        };
    }
    if schema.get("format").and_then(Value::as_str) == Some("uuid") {
        return json!(UUID);
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("integer" | "number") => schema.get("minimum").cloned().unwrap_or_else(|| json!(1)),
        Some("boolean") => json!(false),
        Some("array") => {
            let item = schema
                .get("items")
                .map_or_else(|| json!("x"), |items| placeholder(items, name));
            let count = schema.get("minItems").and_then(Value::as_u64).unwrap_or(1).max(1);
            Value::Array((0..count).map(|_| item.clone()).collect())
        }
        Some("object") => {
            let mut object = serde_json::Map::new();
            let properties = schema.get("properties").and_then(Value::as_object);
            for key in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
                if let (Some(key), Some(properties)) = (key.as_str(), properties)
                    && let Some(property) = properties.get(key)
                {
                    object.insert(key.to_string(), placeholder(property, key));
                }
            }
            Value::Object(object)
        }
        _ => {
            let description = schema.get("description").and_then(Value::as_str).unwrap_or_default();
            if name == "id" || name.ends_with("_id") || description.contains("UUID") {
                json!(UUID)
            } else {
                let length = schema.get("minLength").and_then(Value::as_u64).unwrap_or(1).max(1);
                json!("x".repeat(usize::try_from(length).unwrap_or(1)))
            }
        }
    }
}

/// Per tool arguments for the tools whose schema alone does not describe a valid call.
pub fn overrides(name: &str) -> Option<Value> {
    Some(match name {
        "events.tail" => json!({ "form_id": UUID }),
        "work_items.get_by_identifier" => json!({ "identifier": "PRX-1" }),
        "objects.search" => json!({ "workspace_id": UUID, "q": "x", "unprojected": true }),
        "objects.integrity" => json!({ "workspace_id": UUID }),
        "files.upload" => json!({ "filename": "a.txt", "content_base64": "eA==" }),
        "flow.feature_set" => json!({ "workspace_id": UUID, "idempotency_key": "k", "enabled": true }),
        "objects.import_artifact" => {
            json!({ "workspace_id": UUID, "idempotency_key": "k", "package_base64": "eA==" })
        }
        "collab.compact" => json!({
            "object_id": UUID, "document_id": UUID, "confirm_document_id": UUID, "dry_run": false,
            "expected_head_seq": 1, "idempotency_key": "k"
        }),
        "collab.repair_quarantine" => json!({
            "workspace_id": UUID, "dry_run": false, "confirm_quarantine": true, "idempotency_key": "k"
        }),
        _ => return None,
    })
}

pub fn minimal_arguments(tool: &ToolDefinition) -> Value {
    overrides(&tool.name).unwrap_or_else(|| placeholder(&tool.input_schema, ""))
}

/// What the stand-in API answers for `GET .../agent-policy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// No tool list: the policy restricts nothing.
    Permissive,
    /// An empty tool list: the policy enables nothing.
    DenyAll,
}

/// One request the stand-in API saw.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: Value,
    /// `X-Sylvode-MCP-Tool`: the tool name the client bound to the request.
    pub tool: Option<String>,
}

/// The stand-in API: every request is recorded and answered with a generic success envelope whose
/// `data` carries the ids the ownership lookups read (`project_id`, `form_id`, `project.id`).
#[derive(Clone)]
pub struct Api {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[derive(Clone)]
struct State_ {
    seen: Arc<Mutex<Vec<Seen>>>,
    policy: Policy,
}

fn header_text(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn record(State(state): State<State_>, request: Request) -> impl IntoResponse {
    let (method, path, query) = (
        request.method().to_string(),
        request.uri().path().to_string(),
        request.uri().query().unwrap_or_default().to_string(),
    );
    let tool = header_text(&request, "x-sylvode-mcp-tool");
    let bytes = to_bytes(request.into_body(), 1 << 20).await.unwrap_or_default();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let data = if path.ends_with("/agent-policy") && state.policy == Policy::DenyAll {
        json!({ "mcp": { "tool_registry": { "enabled_tools": [] } } })
    } else if path.ends_with("/agent-policy") {
        json!({})
    } else {
        json!({
            "document_id": UUID, "project_id": UUID, "form_id": UUID, "id": UUID,
            "project": { "id": UUID }
        })
    };
    state.seen.lock().push(Seen {
        method,
        path,
        query,
        body,
        tool,
    });
    (
        StatusCode::OK,
        axum::Json(json!({ "code": 0, "message": "success", "data": data })),
    )
}

impl Api {
    pub async fn spawn(policy: Policy) -> Result<Self, Box<dyn std::error::Error>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new().fallback(record).with_state(State_ {
            seen: Arc::clone(&seen),
            policy,
        });
        Ok(Self {
            url: test_api::spawn(router).await?,
            seen,
        })
    }

    pub fn client(&self) -> Result<OpenPrClient, String> {
        test_api::client(self.url.clone())
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().clone()
    }

    pub fn clear(&self) {
        self.seen.lock().clear();
    }
}
