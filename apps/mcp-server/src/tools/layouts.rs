//! Layout authoring tools: check a layout (`sv.layout.v1`) and propose it as a draft.

use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

const DRAFT_CONTRACT: &str = "The draft changes nothing until a person who owns or administers the workspace \
approves it in the console; a bot or this tool cannot approve. The answer is the stored draft with its \
validation report and the capabilities the layout would use: an invalid spec is stored with its typed errors \
and can not be approved, so read validation.valid and validation.errors and create a corrected draft. A newer \
draft of yours for the same slot and scope replaces your older pending one.";

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

pub fn validate_layout_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "layouts.validate".to_string(),
        description: "Check a layout spec (sv.layout.v1) without storing anything. Returns {valid, errors, \
analysis}: every error has a JSON Pointer path and a code; analysis lists the capabilities, packages and forms \
the layout uses. Pass slot_id to also check the layout against the slot it will sit in (list the slots and the \
components with the layouts catalogue endpoints of the guide). Use it before layouts.create_draft. \
project_id names the project the check is made for; it is not part of the spec."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID the layout is meant for" },
                "spec": { "type": "object", "description": "The layout spec, schema \"sv.layout.v1\"" },
                "slot_id": { "type": "string", "description": "Slot id, for example form.record.detail" },
                "scope_kind": { "type": "string", "enum": ["workspace", "project", "form"] },
                "scope_id": { "type": "string", "description": "UUID of the workspace, project or form named by scope_kind" }
            },
            "required": ["project_id", "spec"]
        }),
    }
}

pub fn create_layout_draft_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "layouts.create_draft".to_string(),
        description: format!(
            "Propose a layout (sv.layout.v1) for a slot at a scope as a draft for review; on approval the layout is \
stored and placed in the slot. {DRAFT_CONTRACT}"
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" },
                "slot_id": { "type": "string", "description": "Slot id, for example form.record.detail" },
                "scope_kind": { "type": "string", "enum": ["workspace", "project", "form"] },
                "scope_id": { "type": "string", "description": "UUID of the workspace, project or form named by scope_kind" },
                "spec": { "type": "object", "description": "The layout spec, schema \"sv.layout.v1\"" },
                "note": { "type": "string", "maxLength": 2000, "description": "Why, for the reviewer" }
            },
            "required": ["project_id", "slot_id", "scope_kind", "scope_id", "spec"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ValidateLayoutInput {
    // Names the project whose agent policy governs the call; the API check does not take it.
    #[allow(
        dead_code,
        reason = "read by the policy gate, which looks at the arguments before the call"
    )]
    project_id: String,
    spec: Value,
    slot_id: Option<String>,
    scope_kind: Option<String>,
    scope_id: Option<String>,
}

pub async fn validate_layout(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ValidateLayoutInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "spec": input.spec,
        "slot_id": input.slot_id,
        "scope_kind": input.scope_kind,
        "scope_id": input.scope_id
    });
    match client.validate_layout(body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

#[derive(Debug, Deserialize)]
struct CreateLayoutDraftInput {
    project_id: String,
    slot_id: String,
    scope_kind: String,
    scope_id: String,
    spec: Value,
    note: Option<String>,
}

pub async fn create_layout_draft(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateLayoutDraftInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "kind": "layout",
        "project_id": input.project_id,
        "target": {
            "slot_id": input.slot_id,
            "scope_kind": input.scope_kind,
            "scope_id": input.scope_id
        },
        "spec": input.spec,
        "note": input.note
    });
    match client.create_draft(body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::{create_layout_draft, validate_layout};
    use crate::client::test_api;
    use axum::{Json, Router, extract::Request, routing::post};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    async fn recording_api() -> Result<(crate::client::OpenPrClient, Seen), Box<dyn std::error::Error>> {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let record = |seen: Seen| {
            move |request: Request| {
                let seen = seen.clone();
                async move {
                    let path = request.uri().path().to_string();
                    let bytes = axum::body::to_bytes(request.into_body(), 1 << 20)
                        .await
                        .unwrap_or_default();
                    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    if let Ok(mut seen) = seen.lock() {
                        seen.push((path, body));
                    }
                    Json(json!({ "code": 0, "message": "success", "data": { "valid": true } }))
                }
            }
        };
        let router = Router::new()
            .route("/api/v1/layouts/validate", post(record(seen.clone())))
            .route("/api/v1/drafts", post(record(seen.clone())));
        let client = test_api::client(test_api::spawn(router).await?)?;
        Ok((client, seen))
    }

    #[tokio::test]
    async fn validate_sends_the_spec_and_never_the_project() -> Result<(), Box<dyn std::error::Error>> {
        let (client, seen) = recording_api().await?;
        let result = validate_layout(
            &client,
            json!({ "project_id": "p", "spec": { "schema": "sv.layout.v1" }, "slot_id": "form.record.detail" }),
        )
        .await;
        assert_ne!(result.is_error, Some(true));
        let seen = seen.lock().map_err(|_| "poisoned")?;
        let (path, body) = seen.first().ok_or("no request")?;
        assert_eq!(path, "/api/v1/layouts/validate");
        assert!(body.get("project_id").is_none(), "{body}");
        assert_eq!(body["slot_id"], "form.record.detail");
        Ok(())
    }

    #[tokio::test]
    async fn create_draft_posts_a_layout_draft_with_its_target() -> Result<(), Box<dyn std::error::Error>> {
        let (client, seen) = recording_api().await?;
        let result = create_layout_draft(
            &client,
            json!({
                "project_id": "p", "slot_id": "form.record.detail", "scope_kind": "form", "scope_id": "f",
                "spec": { "schema": "sv.layout.v1" }, "note": "why"
            }),
        )
        .await;
        assert_ne!(result.is_error, Some(true));
        let seen = seen.lock().map_err(|_| "poisoned")?;
        let (path, body) = seen.first().ok_or("no request")?;
        assert_eq!(path, "/api/v1/drafts");
        assert_eq!(body["kind"], "layout");
        assert_eq!(body["project_id"], "p");
        assert_eq!(
            body["target"],
            json!({ "slot_id": "form.record.detail", "scope_kind": "form", "scope_id": "f" })
        );
        assert_eq!(body["note"], "why");
        Ok(())
    }
}
