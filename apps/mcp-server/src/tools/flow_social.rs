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
        set_appearance_tool(),
        upload_appearance_tool(),
        restore_version_tool(),
        list_trash_tool(),
        purge_object_tool(),
        list_mention_candidates_tool(),
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

pub fn set_appearance_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.appearance.set".to_string(),
        description: "Set a Flow page icon and cover. Icon is {kind:emoji,emoji} or {kind:image,asset_id}. Cover is {kind:gallery,gallery_id,position} or {kind:upload,asset_id,position}. Position is 0 to 1. Requires edit access. Omit a field to clear it."
            .to_string(),
        input_schema: object_schema(
            &[
                ("icon", json!({ "type": "object" })),
                ("cover", json!({ "type": "object" })),
            ],
            &[],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct AppearanceInput {
    object_id: String,
    icon: Option<Value>,
    cover: Option<Value>,
}

pub async fn set_appearance(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: AppearanceInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let body = json!({ "icon": input.icon, "cover": input.cover });
    respond_data(client.set_flow_appearance(&input.object_id, &body).await)
}

pub fn upload_appearance_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.appearance.upload".to_string(),
        description: "Upload a png, jpeg, webp, or gif (max 2 MiB, standard base64) for a Flow page icon or cover. Requires edit access. The returned asset_id is then passed to flow.appearance.set."
            .to_string(),
        input_schema: object_schema(
            &[("data_base64", json!({ "type": "string", "minLength": 1 }))],
            &["data_base64"],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct UploadAppearanceInput {
    object_id: String,
    data_base64: String,
}

pub async fn upload_appearance(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UploadAppearanceInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(
        client
            .upload_flow_appearance(&input.object_id, &json!({ "data_base64": input.data_base64 }))
            .await,
    )
}

pub fn restore_version_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.restore".to_string(),
        description: "Restore a Flow page to an accepted history sequence as a new forward change. Concurrent edits are kept. Requires edit access."
            .to_string(),
        input_schema: object_schema(
            &[
                ("seq", json!({ "type": "integer", "minimum": 0 })),
                ("idempotency_key", json!({ "type": "string", "minLength": 1, "maxLength": 128 })),
                ("message", json!({ "type": "string", "maxLength": 500 })),
            ],
            &["seq", "idempotency_key"],
        ),
    }
}

#[derive(Debug, Deserialize)]
struct RestoreInput {
    object_id: String,
    seq: i64,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn restore_version(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: RestoreInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let mut body = json!({ "seq": input.seq, "idempotency_key": input.idempotency_key });
    if let Some(object) = body.as_object_mut()
        && let Some(message) = input.message
    {
        object.insert("message".to_string(), json!(message));
    }
    respond_data(client.restore_flow_version(&input.object_id, &body).await)
}

pub fn list_trash_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.trash.list".to_string(),
        description: "List archived Flow objects in a workspace that the caller can view.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string" },
                "cursor": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            },
            "required": ["workspace_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
struct TrashInput {
    workspace_id: String,
    cursor: Option<String>,
    limit: Option<u64>,
}

pub async fn list_trash(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: TrashInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(
        client
            .list_flow_trash(&input.workspace_id, input.cursor.as_deref(), input.limit)
            .await,
    )
}

pub fn purge_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.purge".to_string(),
        description: "Permanently delete an archived Flow object and its archived descendants. Requires full access on every object in the subtree. Active subpages must be archived first."
            .to_string(),
        input_schema: object_schema(&[], &[]),
    }
}

pub async fn purge_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(client.purge_flow_object(&input.object_id).await)
}

pub fn list_mention_candidates_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.mentions.list".to_string(),
        description: "List people who can be @mentioned on a Flow object: workspace members and guests who hold a grant. Requires comment access. Mention notifications are still sent only to workspace members."
            .to_string(),
        input_schema: object_schema(&[("q", json!({ "type": "string", "maxLength": 80 }))], &[]),
    }
}

#[derive(Debug, Deserialize)]
struct MentionInput {
    object_id: String,
    q: Option<String>,
}

pub async fn list_mention_candidates(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: MentionInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(
        client
            .list_flow_mention_candidates(&input.object_id, input.q.as_deref())
            .await,
    )
}
