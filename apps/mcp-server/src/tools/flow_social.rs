//! Flow comment threads and public share links.
//!
//! Every tool is `OwnedBy(FlowObject)` and the REST handler is the authority for the grade
//! (`view` to list comments, `comment` to write, `full_access` to manage links). The anonymous
//! public read is not a tool.

use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

fn respond_data(result: Result<Value, String>) -> CallToolResult {
    match result {
        Ok(value) => value.get("data").map_or_else(
            || CallToolResult::error("Malformed successful API envelope: missing data".to_string()),
            |data| CallToolResult::success(serde_json::to_string_pretty(data).unwrap_or_default()),
        ),
        Err(error) => CallToolResult::error(error),
    }
}

fn object_schema(extra: &[(&str, Value)], required: &[&str]) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "object_id".to_string(),
        json!({ "type": "string", "description": "Flow object UUID" }),
    );
    for (name, schema) in extra {
        properties.insert((*name).to_string(), schema.clone());
    }
    let mut required_names = vec!["object_id".to_string()];
    required_names.extend(required.iter().map(|name| (*name).to_string()));
    json!({
        "type": "object",
        "properties": properties,
        "required": required_names,
        "additionalProperties": false
    })
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        list_comments_tool(),
        create_comment_tool(),
        update_comment_tool(),
        delete_comment_tool(),
        resolve_comment_tool(),
        list_share_links_tool(),
        create_share_link_tool(),
        revoke_share_link_tool(),
    ]
}

pub fn list_comments_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.comments.list".to_string(),
        description: "List comment threads on a Flow object. Requires view access. Deleted comments are omitted."
            .to_string(),
        input_schema: object_schema(&[], &[]),
    }
}

#[derive(Debug, Deserialize)]
struct ObjectInput {
    object_id: String,
}

pub async fn list_comments(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(client.list_flow_comments(&input.object_id).await)
}

pub fn create_comment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.comments.create".to_string(),
        description: "Create a comment or a one-level reply on a Flow object. Requires comment access. \
mentions are user UUIDs; only workspace members are notified."
            .to_string(),
        input_schema: object_schema(
            &[
                ("body", json!({ "type": "string", "minLength": 1, "maxLength": 8000 })),
                ("block_id", json!({ "type": "string", "maxLength": 128 })),
                ("parent_id", json!({ "type": "string" })),
                (
                    "mentions",
                    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 20 }),
                ),
            ],
            &["body"],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct CreateCommentInput {
    object_id: String,
    body: String,
    block_id: Option<String>,
    parent_id: Option<String>,
    #[serde(default)]
    mentions: Vec<String>,
}

pub async fn create_comment(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateCommentInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let mut body = json!({ "body": input.body, "mentions": input.mentions });
    if let Some(object) = body.as_object_mut() {
        if let Some(block_id) = input.block_id {
            object.insert("block_id".to_string(), json!(block_id));
        }
        if let Some(parent_id) = input.parent_id {
            object.insert("parent_id".to_string(), json!(parent_id));
        }
    }
    respond_data(client.create_flow_comment(&input.object_id, &body).await)
}

pub fn update_comment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.comments.update".to_string(),
        description: "Edit the caller's own Flow comment. Requires comment access.".to_string(),
        input_schema: object_schema(
            &[
                ("flow_comment_id", json!({ "type": "string" })),
                ("body", json!({ "type": "string", "minLength": 1, "maxLength": 8000 })),
                (
                    "mentions",
                    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 20 }),
                ),
            ],
            &["flow_comment_id", "body"],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateCommentInput {
    object_id: String,
    flow_comment_id: String,
    body: String,
    mentions: Option<Vec<String>>,
}

pub async fn update_comment(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UpdateCommentInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let mut body = json!({ "body": input.body });
    if let Some(object) = body.as_object_mut()
        && let Some(mentions) = input.mentions
    {
        object.insert("mentions".to_string(), json!(mentions));
    }
    respond_data(
        client
            .update_flow_comment(&input.object_id, &input.flow_comment_id, &body)
            .await,
    )
}

pub fn delete_comment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.comments.delete".to_string(),
        description: "Soft-delete a Flow comment. The author needs comment access; full access can delete any."
            .to_string(),
        input_schema: object_schema(
            &[("flow_comment_id", json!({ "type": "string" }))],
            &["flow_comment_id"],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct CommentRefInput {
    object_id: String,
    flow_comment_id: String,
}

pub async fn delete_comment(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CommentRefInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(
        client
            .delete_flow_comment(&input.object_id, &input.flow_comment_id)
            .await,
    )
}

pub fn resolve_comment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.comments.resolve".to_string(),
        description: "Resolve or reopen a top-level Flow comment. Requires edit access, or comment access when you are the author."
            .to_string(),
        input_schema: object_schema(
            &[
                ("flow_comment_id", json!({ "type": "string" })),
                ("resolved", json!({ "type": "boolean" })),
            ],
            &["flow_comment_id", "resolved"],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct ResolveCommentInput {
    object_id: String,
    flow_comment_id: String,
    resolved: bool,
}

pub async fn resolve_comment(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ResolveCommentInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(
        client
            .resolve_flow_comment(
                &input.object_id,
                &input.flow_comment_id,
                &json!({ "resolved": input.resolved }),
            )
            .await,
    )
}

pub fn list_share_links_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.shares.list".to_string(),
        description: "List public read-only links for a Flow page. Requires full access. Raw tokens are not returned."
            .to_string(),
        input_schema: object_schema(&[], &[]),
    }
}

pub async fn list_share_links(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(client.list_flow_share_links(&input.object_id).await)
}

pub fn create_share_link_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.shares.create".to_string(),
        description: "Create a public read-only link for one Flow page. Requires full access. The raw token is returned once and does not include child objects."
            .to_string(),
        input_schema: object_schema(
            &[(
                "expires_at",
                json!({ "type": "string", "description": "Optional RFC3339 expiry" }),
            )],
            &[],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct CreateShareInput {
    object_id: String,
    expires_at: Option<String>,
}

pub async fn create_share_link(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateShareInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let mut body = json!({});
    if let Some(object) = body.as_object_mut()
        && let Some(expires_at) = input.expires_at
    {
        object.insert("expires_at".to_string(), json!(expires_at));
    }
    respond_data(client.create_flow_share_link(&input.object_id, &body).await)
}

pub fn revoke_share_link_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.shares.revoke".to_string(),
        description: "Revoke a public Flow page link. Requires full access. Later reads answer not_found.".to_string(),
        input_schema: object_schema(&[("link_id", json!({ "type": "string" }))], &["link_id"]),
    }
}

#[derive(Debug, Deserialize)]
struct RevokeShareInput {
    object_id: String,
    link_id: String,
}

pub async fn revoke_share_link(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: RevokeShareInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(client.revoke_flow_share_link(&input.object_id, &input.link_id).await)
}
