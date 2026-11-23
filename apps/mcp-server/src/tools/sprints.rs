use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

pub fn create_sprint_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "sprints.create".to_string(),
        description: "Create a sprint in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "name": {
                    "type": "string",
                    "description": "Sprint name"
                },
                "start_date": {
                    "type": "string",
                    "description": "Start date (YYYY-MM-DD, optional)"
                },
                "end_date": {
                    "type": "string",
                    "description": "End date (YYYY-MM-DD, optional)"
                },
                "description": {
                    "type": "string",
                    "description": "Sprint description (optional)"
                },
                "status": {
                    "type": "string",
                    "enum": ["planned", "active"],
                    "description": "Initial sprint status (optional, defaults to planned). A new sprint cannot start as completed or cancelled; creating an active sprint needs an owner or admin and is refused with 409 while the project already has an active sprint"
                }
            },
            "required": ["project_id", "name"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateSprintInput {
    project_id: String,
    name: String,
    start_date: Option<String>,
    end_date: Option<String>,
    description: Option<String>,
    status: Option<String>,
}

pub async fn create_sprint(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateSprintInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let body = json!({
        "name": input.name,
        "start_date": input.start_date,
        "end_date": input.end_date,
        "description": input.description,
        "status": input.status
    });

    match client.create_sprint(&input.project_id, body).await {
        Ok(sprint) => {
            let json = serde_json::to_string_pretty(&sprint).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn list_sprints_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "sprints.list".to_string(),
        description: "List all sprints in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListSprintsInput {
    project_id: String,
}

pub async fn list_sprints(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListSprintsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.list_sprints(&input.project_id).await {
        Ok(sprints) => {
            let json = serde_json::to_string_pretty(&sprints).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_sprint_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "sprints.update".to_string(),
        description: "Update sprint fields. Status moves planned -> active -> completed, and planned or active -> cancelled; completed and cancelled are final. A project has one active sprint (starting a second is refused with 409 naming it), and changing the status needs an owner or admin. Completing or cancelling moves the unfinished work items to the backlog, or to a planned sprint given as rollover_sprint_id; the answer reports rollover.moved_count".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "sprint_id": {
                    "type": "string",
                    "description": "UUID of the sprint",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "name": {
                    "type": "string",
                    "description": "Sprint name (optional)"
                },
                "description": {
                    "type": "string",
                    "description": "Sprint description (optional)"
                },
                "status": {
                    "type": "string",
                    "enum": ["planned", "active", "completed", "cancelled"],
                    "description": "Sprint status (optional)"
                },
                "rollover_sprint_id": {
                    "type": "string",
                    "description": "UUID of a planned sprint of the same project that takes the unfinished work items (optional; only with status completed or cancelled, otherwise they go to the backlog)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "start_date": {
                    "type": "string",
                    "description": "Start date (YYYY-MM-DD, optional)"
                },
                "end_date": {
                    "type": "string",
                    "description": "End date (YYYY-MM-DD, optional)"
                }
            },
            "required": ["sprint_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateSprintInput {
    sprint_id: String,
    name: Option<String>,
    description: Option<String>,
    status: Option<String>,
    rollover_sprint_id: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
}

pub async fn update_sprint(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: UpdateSprintInput = match serde_json::from_value(args) {
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
    if let Some(target) = input.rollover_sprint_id {
        body.insert("rollover_sprint_id".to_string(), json!(target));
    }
    if let Some(start) = input.start_date {
        body.insert("start_date".to_string(), json!(start));
    }
    if let Some(end) = input.end_date {
        body.insert("end_date".to_string(), json!(end));
    }

    match client
        .update_sprint(&input.sprint_id, serde_json::Value::Object(body))
        .await
    {
        Ok(sprint) => {
            let json = serde_json::to_string_pretty(&sprint).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_sprint_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "sprints.delete".to_string(),
        description: "Delete a sprint".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "sprint_id": {
                    "type": "string",
                    "description": "UUID of the sprint",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["sprint_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteSprintInput {
    sprint_id: String,
}

pub async fn handle_delete_sprint(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: DeleteSprintInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.delete_sprint(&input.sprint_id).await {
        Ok(_) => CallToolResult::success("Sprint deleted"),
        Err(e) => CallToolResult::error(e),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::update_sprint;
    use crate::tools::test_support::{Api, Policy, UUID};
    use serde_json::json;

    #[tokio::test]
    async fn update_sends_description_and_rollover() {
        let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
        let client = api.client().expect("client");
        let result = update_sprint(
            &client,
            json!({ "sprint_id": UUID, "status": "completed", "rollover_sprint_id": UUID, "description": "d" }),
        )
        .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let body = api
            .seen()
            .into_iter()
            .rev()
            .find(|seen| seen.method == "PUT")
            .expect("an update was sent")
            .body;
        assert_eq!(
            body,
            json!({ "status": "completed", "rollover_sprint_id": UUID, "description": "d" })
        );
    }

    #[test]
    fn create_offers_only_the_statuses_a_new_sprint_can_have() {
        let tool = super::create_sprint_tool();
        assert_eq!(
            tool.input_schema["properties"]["status"]["enum"],
            json!(["planned", "active"])
        );
    }
}
