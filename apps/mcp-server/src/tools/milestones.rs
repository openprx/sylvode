//! Workspace milestones: one milestone spans projects ("Beta"), and a work item belongs to at most
//! one. The workspace is the caller's own: the bot token's workspace, never an argument.
//!
//! Who may do what is the API's decision, not this module's. Reading needs a member; creating,
//! changing and deleting need a workspace owner or admin; the milestone routes are refused for a
//! token limited to some projects and for a runner bot. A refusal reaches the caller as the
//! tool's error.

use crate::client::{ListMilestonesQuery, OpenPrClient};
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

const MILESTONE_STATUSES: [&str; 2] = ["open", "closed"];
const MAX_PER_PAGE: u64 = 100;

const UUID_PATTERN: &str = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

fn milestone_id_property() -> Value {
    json!({
        "type": "string",
        "description": "UUID of the milestone",
        "pattern": UUID_PATTERN
    })
}

fn pretty(value: &Value) -> CallToolResult {
    CallToolResult::success(serde_json::to_string_pretty(value).unwrap_or_default())
}

pub fn list_milestones_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "milestones.list".to_string(),
        description: "List the milestones of the workspace the calling token belongs to, with their progress \
(items done and total, in all and per project). Paginated: per_page defaults to 15 (max 100); check \
total_pages before treating the result as complete. A token limited to some projects is refused."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": MILESTONE_STATUSES,
                    "description": "Only milestones with this status (optional)"
                },
                "page": {
                    "type": "integer",
                    "description": "Page number (optional)",
                    "minimum": 1,
                    "default": 1
                },
                "per_page": {
                    "type": "integer",
                    "description": "Milestones per page (optional)",
                    "minimum": 1,
                    "maximum": MAX_PER_PAGE,
                    "default": 15
                }
            }
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListMilestonesInput {
    status: Option<String>,
    page: Option<u64>,
    per_page: Option<u64>,
}

pub async fn list_milestones(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListMilestonesInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    if input.page == Some(0) {
        return CallToolResult::error("Invalid input: page must be >= 1");
    }
    if let Some(per_page) = input.per_page
        && !(1..=MAX_PER_PAGE).contains(&per_page)
    {
        return CallToolResult::error(format!("Invalid input: per_page must be between 1 and {MAX_PER_PAGE}"));
    }
    if let Some(status) = input.status.as_deref()
        && !MILESTONE_STATUSES.contains(&status)
    {
        return CallToolResult::error("Invalid input: status must be one of: open, closed");
    }
    let query = ListMilestonesQuery {
        status: input.status.as_deref(),
        page: input.page,
        per_page: input.per_page,
    };
    match client.list_milestones(&query).await {
        Ok(milestones) => pretty(&milestones),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn get_milestone_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "milestones.get".to_string(),
        description: "Get one milestone with its progress and its work items grouped by project (at most 500 \
items; `truncated` says when there are more). Use work_items.list with milestone_id to page through all of \
them in a project."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": { "milestone_id": milestone_id_property() },
            "required": ["milestone_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct MilestoneIdInput {
    milestone_id: String,
}

pub async fn get_milestone(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: MilestoneIdInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    match client.get_milestone(&input.milestone_id).await {
        Ok(milestone) => pretty(&milestone),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn create_milestone_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "milestones.create".to_string(),
        description: "Create a milestone in the workspace the calling token belongs to. The API accepts this \
from a workspace owner or admin only, and refuses a token limited to some projects; a name already in use \
(ignoring case) is refused."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Milestone name, 1 to 100 characters, unique in the workspace ignoring case",
                    "minLength": 1,
                    "maxLength": 100
                },
                "description": {
                    "type": "string",
                    "description": "Milestone description (optional)"
                },
                "target_date": {
                    "type": "string",
                    "description": "Target date as YYYY-MM-DD (optional)"
                }
            },
            "required": ["name"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateMilestoneInput {
    name: String,
    description: Option<String>,
    target_date: Option<String>,
}

pub async fn create_milestone(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateMilestoneInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), json!(input.name));
    if let Some(description) = input.description {
        body.insert("description".to_string(), json!(description));
    }
    if let Some(target_date) = input.target_date.filter(|date| !date.is_empty()) {
        body.insert("target_date".to_string(), json!(target_date));
    }
    match client.create_milestone(Value::Object(body)).await {
        Ok(milestone) => pretty(&milestone),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_milestone_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "milestones.update".to_string(),
        description: "Change a milestone: rename it, edit its description or target date, or close or reopen \
it. Send only the fields to change. The API accepts this from a workspace owner or admin only."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "milestone_id": milestone_id_property(),
                "name": {
                    "type": "string",
                    "description": "New name, 1 to 100 characters (optional)",
                    "minLength": 1,
                    "maxLength": 100
                },
                "description": {
                    "type": "string",
                    "description": "New description (optional)"
                },
                "status": {
                    "type": "string",
                    "enum": MILESTONE_STATUSES,
                    "description": "New status (optional). A closed milestone cannot be given to work items"
                },
                "target_date": {
                    "type": "string",
                    "description": "New target date as YYYY-MM-DD (optional). Send null or an empty string to remove the date"
                }
            },
            "required": ["milestone_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateMilestoneInput {
    milestone_id: String,
    name: Option<String>,
    description: Option<String>,
    status: Option<String>,
    target_date: Option<String>,
}

pub async fn update_milestone(client: &OpenPrClient, args: Value) -> CallToolResult {
    let target_date_given = args.get("target_date").is_some();
    let input: UpdateMilestoneInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    let mut body = serde_json::Map::new();
    if let Some(name) = input.name {
        body.insert("name".to_string(), json!(name));
    }
    if let Some(description) = input.description {
        body.insert("description".to_string(), json!(description));
    }
    if let Some(status) = input.status {
        body.insert("status".to_string(), json!(status));
    }
    if target_date_given {
        let value = match input.target_date.as_deref() {
            None | Some("" | "null") => Value::Null,
            Some(date) => json!(date),
        };
        body.insert("target_date".to_string(), value);
    }
    if body.is_empty() {
        return CallToolResult::error("Invalid input: send at least one of name, description, status or target_date");
    }
    match client.update_milestone(&input.milestone_id, Value::Object(body)).await {
        Ok(milestone) => pretty(&milestone),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_milestone_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "milestones.delete".to_string(),
        description: "Delete a milestone. Every work item in it is taken out of it (each gets an activity entry \
and an update event); the items themselves are kept. The API accepts this from a workspace owner or admin only."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": { "milestone_id": milestone_id_property() },
            "required": ["milestone_id"]
        }),
    }
}

pub async fn handle_delete_milestone(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: MilestoneIdInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    match client.delete_milestone(&input.milestone_id).await {
        Ok(_) => CallToolResult::success("Milestone deleted"),
        Err(e) => CallToolResult::error(e),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::{create_milestone, handle_delete_milestone, list_milestones, update_milestone};
    use crate::tools::test_support::{Api, Policy, UUID};
    use serde_json::{Value, json};

    async fn run<F, Fut>(call: F, args: Value) -> (Api, crate::protocol::CallToolResult)
    where
        F: FnOnce(crate::client::OpenPrClient, Value) -> Fut,
        Fut: std::future::Future<Output = crate::protocol::CallToolResult>,
    {
        let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
        let client = api.client().expect("client");
        let result = call(client, args).await;
        (api, result)
    }

    #[tokio::test]
    async fn list_sends_the_filters_to_the_workspace_route() {
        let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
        let client = api.client().expect("client");
        let result = list_milestones(&client, json!({"status": "open", "page": 2, "per_page": 5})).await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let seen = api.seen();
        let [request] = seen.as_slice() else {
            panic!("one request expected: {seen:?}")
        };
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/v1/workspaces/workspace-1/milestones");
        assert_eq!(request.query, "status=open&page=2&per_page=5");
    }

    #[tokio::test]
    async fn list_refuses_a_bad_page_or_status_before_any_request() {
        for args in [
            json!({"page": 0}),
            json!({"per_page": 0}),
            json!({"per_page": 101}),
            json!({"status": "done"}),
        ] {
            let (api, result) = run(|client, args| async move { list_milestones(&client, args).await }, args).await;
            assert_eq!(result.is_error, Some(true));
            assert!(api.seen().is_empty());
        }
    }

    #[tokio::test]
    async fn create_posts_only_what_was_given() {
        let (api, result) = run(
            |client, args| async move { create_milestone(&client, args).await },
            json!({"name": "Beta", "target_date": "2026-12-31"}),
        )
        .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let seen = api.seen();
        let [request] = seen.as_slice() else {
            panic!("one request expected: {seen:?}")
        };
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/v1/workspaces/workspace-1/milestones");
        assert_eq!(request.body, json!({"name": "Beta", "target_date": "2026-12-31"}));
    }

    #[tokio::test]
    async fn update_clears_the_date_with_null_or_an_empty_string_and_leaves_absent_fields_out() {
        for date in [Value::Null, json!(""), json!("null")] {
            let (api, result) = run(
                |client, args| async move { update_milestone(&client, args).await },
                json!({"milestone_id": UUID, "target_date": date}),
            )
            .await;
            assert_ne!(result.is_error, Some(true), "{result:?}");
            let seen = api.seen();
            let [request] = seen.as_slice() else {
                panic!("one request expected: {seen:?}")
            };
            assert_eq!(request.method, "PUT");
            assert_eq!(request.path, format!("/api/v1/milestones/{UUID}"));
            assert_eq!(request.body, json!({"target_date": null}));
        }
        let (api, _) = run(
            |client, args| async move { update_milestone(&client, args).await },
            json!({"milestone_id": UUID, "status": "closed", "description": ""}),
        )
        .await;
        assert_eq!(api.seen()[0].body, json!({"status": "closed", "description": ""}));
    }

    #[tokio::test]
    async fn update_without_a_field_is_refused_before_any_request() {
        let (api, result) = run(
            |client, args| async move { update_milestone(&client, args).await },
            json!({"milestone_id": UUID}),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        assert!(api.seen().is_empty());
    }

    #[tokio::test]
    async fn delete_calls_the_milestone_route() {
        let (api, result) = run(
            |client, args| async move { handle_delete_milestone(&client, args).await },
            json!({"milestone_id": UUID}),
        )
        .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let seen = api.seen();
        let [request] = seen.as_slice() else {
            panic!("one request expected: {seen:?}")
        };
        assert_eq!(
            (request.method.as_str(), request.path.as_str()),
            ("DELETE", format!("/api/v1/milestones/{UUID}").as_str())
        );
    }
}
