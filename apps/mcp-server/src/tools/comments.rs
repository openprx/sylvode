use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

pub fn list_comments_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "comments.list".to_string(),
        description: "List the comments of a work item, newest first, one page at a time (20 by default; `total_pages` in the answer tells how many pages there are). `author` and `mentions` of each comment are person objects \
{id, name|null, is_bot, state}; show the name, never the id (state removed with name null is a removed user)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "page": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Page number, from 1 (default 1)"
                },
                "per_page": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Comments per page (default 20, at most 100)"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListCommentsInput {
    work_item_id: String,
    page: Option<u32>,
    per_page: Option<u32>,
}

pub async fn list_comments(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListCommentsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client
        .list_comments(&input.work_item_id, input.page, input.per_page)
        .await
    {
        Ok(comments) => {
            let json = serde_json::to_string_pretty(&comments).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn create_comment_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "comments.create".to_string(),
        description: "Create a new comment on a work item. Write @Name in content to mention a member: the server \
resolves it (exact name, case-insensitive; a name that two members share mentions nobody, so pick that \
person by id in `mentions`). The answer's `mentions` lists who was really mentioned, and `author` and \
`mentions` are person objects {id, name|null, is_bot, state}."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "content": {
                    "type": "string",
                    "description": "Comment content"
                },
                "attachments": {
                    "type": "array",
                    "description": "Uploaded file URLs (optional)",
                    "items": {
                        "type": "string"
                    }
                },
                "mentions": {
                    "type": "array",
                    "description": "Optional user UUIDs of people to mention by id (in addition to the typed @Name mentions the server resolves)",
                    "items": {
                        "type": "string"
                    }
                }
            },
            "required": ["work_item_id", "content"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateCommentInput {
    work_item_id: String,
    content: String,
    attachments: Option<Vec<String>>,
    mentions: Option<Vec<String>>,
}

pub async fn create_comment(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let content = append_attachments_to_content(input.content, input.attachments);

    let mut body = json!({ "content": content });
    if let Some(mentions) = input.mentions.filter(|ids| !ids.is_empty())
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("mentions".to_string(), json!(mentions));
    }

    match client.create_comment(&input.work_item_id, body).await {
        Ok(comment) => {
            let json = serde_json::to_string_pretty(&comment).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

fn append_attachments_to_content(content: String, attachments: Option<Vec<String>>) -> String {
    use std::fmt::Write as _;
    match attachments {
        Some(items) if !items.is_empty() => {
            let mut output = content;
            output.push_str("\n\n**附件：**\n");
            for url in items {
                let name = attachment_name_from_url(&url);
                let _ = writeln!(output, "- [{name}]({url})");
            }
            output.trim_end().to_string()
        }
        _ => content,
    }
}

fn attachment_name_from_url(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(url)
        .to_string()
}

pub fn delete_comment_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "comments.delete".to_string(),
        description: "Delete a comment".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "comment_id": {
                    "type": "string",
                    "description": "UUID of the comment",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["comment_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteCommentInput {
    comment_id: String,
}

pub async fn handle_delete_comment(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: DeleteCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.delete_comment(&input.comment_id).await {
        Ok(_) => CallToolResult::success("Comment deleted"),
        Err(e) => CallToolResult::error(e),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::{create_comment, list_comments};
    use crate::client::test_api;
    use crate::tools::test_support::{Api, Policy, UUID};
    use axum::{Json, Router, routing::get};
    use serde_json::{Value, json};

    fn text(result: &crate::protocol::CallToolResult) -> Value {
        let content = serde_json::to_value(&result.content).expect("content");
        serde_json::from_str(content[0]["text"].as_str().expect("text")).expect("json")
    }

    /// The server resolves typed `@Name`: the tool sends the content as written and looks nobody up.
    #[tokio::test]
    async fn create_leaves_typed_mentions_to_the_server() {
        let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
        let client = api.client().expect("client");
        let result = create_comment(
            &client,
            json!({ "work_item_id": UUID, "content": "hi @Codex CLI, look" }),
        )
        .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let seen = api.seen();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].body, json!({ "content": "hi @Codex CLI, look" }));
    }

    #[tokio::test]
    async fn create_still_sends_mentions_picked_by_id() {
        let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
        let client = api.client().expect("client");
        let args = json!({ "work_item_id": UUID, "content": "x", "mentions": [UUID] });
        assert_ne!(create_comment(&client, args).await.is_error, Some(true));
        assert_eq!(api.seen()[0].body, json!({ "content": "x", "mentions": [UUID] }));
    }

    #[tokio::test]
    async fn list_passes_author_and_mentions_people_through() {
        let person = json!({ "id": UUID, "name": null, "is_bot": false, "state": "removed" });
        let answer = json!({ "code": 0, "message": "ok", "data": [
            { "id": UUID, "content": "c", "author": person, "mentions": [person] }
        ]});
        let router = Router::new().route(
            "/api/v1/issues/{id}/comments",
            get(move || {
                let answer = answer.clone();
                async move { Json(answer) }
            }),
        );
        let client = test_api::client(test_api::spawn(router).await.expect("api")).expect("client");
        let out = text(&list_comments(&client, json!({ "work_item_id": UUID })).await);
        assert_eq!(out["data"][0]["author"]["state"], "removed");
        assert_eq!(out["data"][0]["author"]["name"], Value::Null);
        assert_eq!(out["data"][0]["mentions"][0]["is_bot"], false);
    }

    /// `page` and `per_page` reach the API as query parameters; without them none is sent.
    #[tokio::test]
    async fn list_forwards_the_page_request() {
        use axum::extract::RawQuery;
        let router = Router::new().route(
            "/api/v1/issues/{id}/comments",
            get(|RawQuery(query): RawQuery| async move {
                Json(json!({"code": 0, "message": "ok", "data": {"query": query.unwrap_or_default()}}))
            }),
        );
        let client = test_api::client(test_api::spawn(router).await.expect("api")).expect("client");
        let paged = text(&list_comments(&client, json!({ "work_item_id": UUID, "page": 2, "per_page": 50 })).await);
        assert_eq!(paged["data"]["query"], "page=2&per_page=50");
        let plain = text(&list_comments(&client, json!({ "work_item_id": UUID })).await);
        assert_eq!(plain["data"]["query"], "");
    }
}
