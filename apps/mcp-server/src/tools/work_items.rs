use crate::client::{ListWorkItemsQuery, OpenPrClient};
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

/// The work item priorities the API accepts (`apps/api/src/routes/issue.rs`, `validate_priority`).
///
/// The one list behind the `priority` enum of every `work_items.*` tool schema and behind the
/// `--priority` values of `work-items create/update` in both executables, so the command line
/// cannot offer a value the tool and the API refuse.
pub const WORK_ITEM_PRIORITIES: [&str; 4] = ["low", "medium", "high", "urgent"];

const DEFAULT_PAGE: u64 = 1;
const DEFAULT_PER_PAGE: u64 = 50;
const MAX_PER_PAGE: u64 = 100;

pub fn list_work_items_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.list".to_string(),
        description: "List work items in a project with optional filters. Paginated: per_page defaults to 50 \
(max 100); check total_pages before treating the result as complete."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "page": {
                    "type": "integer",
                    "description": "Page number (optional)",
                    "minimum": 1,
                    "default": 1
                },
                "per_page": {
                    "type": "integer",
                    "description": "Items per page (optional)",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 50
                },
                "state": {
                    "type": "string",
                    "description": "Filter by workflow state key (optional)"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "Filter by assignee UUID (optional)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "priority": {
                    "type": "string",
                    "enum": WORK_ITEM_PRIORITIES,
                    "description": "Filter by priority (optional)"
                },
                "sprint_id": {
                    "type": "string",
                    "description": "Filter by sprint UUID, or \"none\" for the work items that are in no sprint (optional)"
                },
                "milestone_id": {
                    "type": "string",
                    "description": "Filter by milestone UUID, or \"none\" for the work items that are in no milestone (optional)"
                },
                "search": {
                    "type": "string",
                    "description": "Filter by title/description text (optional)"
                },
                "label_ids": {
                    "type": "string",
                    "description": "Comma-separated label UUIDs to filter by (optional)"
                },
                "sort_by": {
                    "type": "string",
                    "description": "Sort column (optional)"
                },
                "sort_order": {
                    "type": "string",
                    "enum": ["asc", "desc"],
                    "description": "Sort direction (optional)"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListWorkItemsInput {
    project_id: String,
    page: Option<u64>,
    per_page: Option<u64>,
    state: Option<String>,
    assignee_id: Option<String>,
    priority: Option<String>,
    sprint_id: Option<String>,
    milestone_id: Option<String>,
    search: Option<String>,
    label_ids: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
}

pub async fn list_work_items(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let (page, per_page) = match resolve_pagination(input.page, input.per_page) {
        Ok(v) => v,
        Err(e) => return CallToolResult::error(e),
    };

    let query = ListWorkItemsQuery {
        page: Some(page),
        per_page: Some(per_page),
        state: input.state.as_deref(),
        assignee_id: input.assignee_id.as_deref(),
        priority: input.priority.as_deref(),
        sprint_id: input.sprint_id.as_deref(),
        milestone_id: input.milestone_id.as_deref(),
        search: input.search.as_deref(),
        label_ids: input.label_ids.as_deref(),
        sort_by: input.sort_by.as_deref(),
        sort_order: input.sort_order.as_deref(),
    };

    match client.list_work_items(&input.project_id, &query).await {
        Ok(items) => {
            let output = build_paginated_output(&items, "items", page, per_page);
            let json = serde_json::to_string_pretty(&output).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

fn resolve_pagination(page: Option<u64>, per_page: Option<u64>) -> Result<(u64, u64), String> {
    let page = page.unwrap_or(DEFAULT_PAGE);
    let per_page = per_page.unwrap_or(DEFAULT_PER_PAGE);

    if page == 0 {
        return Err("Invalid input: page must be >= 1".to_string());
    }
    if per_page == 0 {
        return Err("Invalid input: per_page must be >= 1".to_string());
    }
    if per_page > MAX_PER_PAGE {
        return Err(format!("Invalid input: per_page must be <= {MAX_PER_PAGE}"));
    }

    Ok((page, per_page))
}

fn extract_collection(data: &Value, key: &str) -> Vec<Value> {
    if let Some(items) = data.get(key).and_then(Value::as_array) {
        return items.clone();
    }
    if let Some(items) = data.as_array() {
        return items.clone();
    }
    Vec::new()
}

fn build_paginated_output(payload: &Value, item_key: &str, page: u64, per_page: u64) -> Value {
    let data = extract_data(payload);
    let items = extract_collection(data, item_key);
    let total_count = data
        .get("total_count")
        .or_else(|| data.get("total"))
        .and_then(value_to_u64)
        .unwrap_or(items.len() as u64);
    let total_pages = data.get("total_pages").and_then(value_to_u64).unwrap_or_else(|| {
        if total_count == 0 {
            0
        } else {
            total_count.div_ceil(per_page)
        }
    });
    let current_page = data
        .get("current_page")
        .or_else(|| data.get("page"))
        .and_then(value_to_u64)
        .unwrap_or(page);

    json!({
        "items": items,
        "total_count": total_count,
        "total_pages": total_pages,
        "current_page": current_page
    })
}

pub fn get_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.get".to_string(),
        description: "Get details of a specific work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetWorkItemInput {
    work_item_id: String,
}

pub async fn get_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: GetWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.get_work_item(&input.work_item_id).await {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn get_work_item_by_identifier_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.get_by_identifier".to_string(),
        description: "Get details of a specific work item by identifier (e.g. PRX-42)".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "identifier": {
                    "type": "string",
                    "description": "Human-readable work item identifier, e.g. PRX-42",
                    "pattern": "^[A-Za-z0-9]+-[0-9]+$"
                }
            },
            "required": ["identifier"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetWorkItemByIdentifierInput {
    identifier: String,
}

pub async fn get_work_item_by_identifier(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: GetWorkItemByIdentifierInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.get_work_item_by_identifier(input.identifier.trim()).await {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

fn extract_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn value_to_u64(value: &Value) -> Option<u64> {
    if let Some(v) = value.as_u64() {
        return Some(v);
    }
    if let Some(v) = value.as_i64() {
        return u64::try_from(v).ok();
    }
    value.as_str()?.trim().parse::<u64>().ok()
}

fn validate_work_item_state(state: &str) -> Result<(), String> {
    if state.trim().is_empty() {
        return Err("Invalid state: must be a non-empty string".to_string());
    }
    Ok(())
}

fn append_attachments_to_text(text: Option<String>, attachments: Option<Vec<String>>) -> Option<String> {
    use std::fmt::Write as _;
    match attachments {
        Some(items) if !items.is_empty() => {
            let mut output = text.unwrap_or_default();
            output.push_str("\n\n**附件：**\n");
            for url in items {
                let name = attachment_name_from_url(&url);
                let _ = writeln!(output, "- [{name}]({url})");
            }
            Some(output.trim_end().to_string())
        }
        _ => text,
    }
}

fn attachment_name_from_url(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(url)
        .to_string()
}

pub fn create_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.create".to_string(),
        description: "Create a new work item in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "title": {
                    "type": "string",
                    "description": "Work item title"
                },
                "description": {
                    "type": "string",
                    "description": "Work item description (optional)"
                },
                "attachments": {
                    "type": "array",
                    "description": "Uploaded file URLs (optional)",
                    "items": {
                        "type": "string"
                    }
                },
                "state": {
                    "type": "string",
                    "description": "Work item state key. Any configured workflow state is accepted. When omitted the project workflow's initial state is used."
                },
                "priority": {
                    "type": "string",
                    "enum": WORK_ITEM_PRIORITIES,
                    "description": "Work item priority",
                    "default": "medium"
                },
                "sprint_id": {
                    "type": "string",
                    "description": "UUID of the sprint to place the work item in (optional)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "UUID of the assignee (optional)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "milestone_id": {
                    "type": "string",
                    "description": "UUID of an open milestone of the project's workspace to place the work item in (optional)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "due_at": {
                    "type": "string",
                    "description": "Due date in RFC3339 format (optional)"
                }
            },
            "required": ["project_id", "title"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateWorkItemInput {
    project_id: String,
    title: String,
    description: Option<String>,
    attachments: Option<Vec<String>>,
    state: Option<String>,
    priority: Option<String>,
    sprint_id: Option<String>,
    assignee_id: Option<String>,
    milestone_id: Option<String>,
    due_at: Option<String>,
}

pub async fn create_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    // No client-side state default: the backend resolves the project workflow's initial
    // state, which is not always "backlog".
    if let Some(state) = input.state.as_deref()
        && let Err(e) = validate_work_item_state(state)
    {
        return CallToolResult::error(e);
    }

    let description = append_attachments_to_text(input.description, input.attachments);

    let mut body = json!({
        "title": input.title,
        "description": description,
        "state": input.state,
        "priority": input.priority.unwrap_or_else(|| "medium".to_string()),
        "sprint_id": input.sprint_id,
        "assignee_id": input.assignee_id,
        "due_at": input.due_at
    });
    // A new work item has no milestone to clear: an empty value is the same as none.
    if let Some(milestone_id) = input.milestone_id.filter(|id| !id.is_empty() && id != "null")
        && let Some(object) = body.as_object_mut()
    {
        object.insert("milestone_id".to_string(), json!(milestone_id));
    }

    match client.create_work_item(&input.project_id, body).await {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.update".to_string(),
        description: "Update an existing work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "title": {
                    "type": "string",
                    "description": "New title (optional)"
                },
                "description": {
                    "type": "string",
                    "description": "New description (optional)"
                },
                "attachments": {
                    "type": "array",
                    "description": "Uploaded file URLs (optional)",
                    "items": {
                        "type": "string"
                    }
                },
                "state": {
                    "type": "string",
                    "description": "New state key (optional). Any configured workflow state is accepted."
                },
                "priority": {
                    "type": "string",
                    "enum": WORK_ITEM_PRIORITIES,
                    "description": "New priority (optional)"
                },
                "sprint_id": {
                    "type": "string",
                    "description": "New sprint UUID (optional). Send null or an empty string to take the work item out of its sprint (back to the backlog)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "New assignee UUID (optional). Send null or an empty string to unassign",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "milestone_id": {
                    "type": "string",
                    "description": "New milestone UUID (optional); it must be an open milestone of the work item's workspace. Send null or an empty string to take the work item out of its milestone",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "due_at": {
                    "type": "string",
                    "description": "New due date in RFC3339 format (optional). Send null or an empty string to remove the due date"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateWorkItemInput {
    work_item_id: String,
    title: Option<String>,
    description: Option<String>,
    attachments: Option<Vec<String>>,
    state: Option<String>,
    priority: Option<String>,
    sprint_id: Option<String>,
    assignee_id: Option<String>,
    milestone_id: Option<String>,
    due_at: Option<String>,
}

/// The body value of a field that can be cleared: absent from the arguments leaves it out; `null`,
/// an empty string and `"null"` clear it; anything else is the new value.
fn clearable(args: Option<&serde_json::Map<String, Value>>, field: &str, value: Option<&str>) -> Option<Value> {
    args.and_then(|args| args.get(field))?;
    Some(match value {
        Some("" | "null") | None => Value::Null,
        Some(value) => json!(value),
    })
}

pub async fn update_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let args_obj = args.as_object().cloned();

    let input: UpdateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let mut body = serde_json::Map::new();
    if let Some(title) = input.title {
        body.insert("title".to_string(), json!(title));
    }
    if let Some(desc) = append_attachments_to_text(input.description, input.attachments) {
        body.insert("description".to_string(), json!(desc));
    }
    if let Some(state) = input.state {
        if let Err(e) = validate_work_item_state(&state) {
            return CallToolResult::error(e);
        }
        body.insert("state".to_string(), json!(state));
    }
    if let Some(priority) = input.priority {
        body.insert("priority".to_string(), json!(priority));
    }
    for (field, value) in [
        ("sprint_id", input.sprint_id.as_deref()),
        ("assignee_id", input.assignee_id.as_deref()),
        ("milestone_id", input.milestone_id.as_deref()),
        ("due_at", input.due_at.as_deref()),
    ] {
        if let Some(value) = clearable(args_obj.as_ref(), field, value) {
            body.insert(field.to_string(), value);
        }
    }

    match client
        .update_work_item(&input.work_item_id, serde_json::Value::Object(body))
        .await
    {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn search_work_items_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.search".to_string(),
        description: "Search work items across all projects the caller can access".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (matches title and description)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional maximum number of matches to return"
                }
            },
            "required": ["query"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SearchWorkItemsInput {
    query: String,
    limit: Option<u32>,
}

pub async fn search_work_items(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: SearchWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.search(&input.query, Some("issue"), input.limit).await {
        Ok(items) => {
            let json = serde_json::to_string_pretty(&items).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn add_label_to_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.add_label".to_string(),
        description: "Add a label to a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "label_id": {
                    "type": "string",
                    "description": "UUID of the label",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id", "label_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct AddLabelToWorkItemInput {
    work_item_id: String,
    label_id: String,
}

pub async fn add_label_to_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: AddLabelToWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.add_label_to_issue(&input.work_item_id, &input.label_id).await {
        Ok(_) => CallToolResult::success("Label added to work item"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn remove_label_from_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.remove_label".to_string(),
        description: "Remove a label from a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "label_id": {
                    "type": "string",
                    "description": "UUID of the label",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id", "label_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct RemoveLabelFromWorkItemInput {
    work_item_id: String,
    label_id: String,
}

pub async fn remove_label_from_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: RemoveLabelFromWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client
        .remove_label_from_issue(&input.work_item_id, &input.label_id)
        .await
    {
        Ok(_) => CallToolResult::success("Label removed from work item"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn list_work_item_labels_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.list_labels".to_string(),
        description: "List labels of a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListWorkItemLabelsInput {
    work_item_id: String,
}

pub async fn list_work_item_labels(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListWorkItemLabelsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.get_issue_labels(&input.work_item_id).await {
        Ok(labels) => {
            let json = serde_json::to_string_pretty(&labels).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.delete".to_string(),
        description: "Delete a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteWorkItemInput {
    work_item_id: String,
}

pub async fn handle_delete_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: DeleteWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.delete_work_item(&input.work_item_id).await {
        Ok(_) => CallToolResult::success("Work item deleted"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn add_labels_to_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.add_labels".to_string(),
        description: "Add multiple labels to a work item in one request".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "label_ids": {
                    "type": "array",
                    "description": "List of label UUIDs to add",
                    "items": {
                        "type": "string",
                        "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                    },
                    "minItems": 1
                }
            },
            "required": ["work_item_id", "label_ids"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct AddLabelsToWorkItemInput {
    work_item_id: String,
    label_ids: Vec<String>,
}

pub async fn add_labels_to_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: AddLabelsToWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.add_labels_to_issue(&input.work_item_id, &input.label_ids).await {
        Ok(_) => CallToolResult::success("Labels added to work item"),
        Err(e) => CallToolResult::error(e),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::{get_work_item_by_identifier, list_work_items, update_work_item};
    use crate::client::test_api;
    use crate::tools::test_support::{Api, Policy, UUID};
    use axum::{Json, Router, http::StatusCode, routing::get};
    use serde_json::{Value, json};

    async fn last_put_body(args: Value) -> Value {
        let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
        let client = api.client().expect("client");
        let result = update_work_item(&client, args).await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        api.seen()
            .into_iter()
            .rev()
            .find(|seen| seen.method == "PUT")
            .expect("an update was sent")
            .body
    }

    #[tokio::test]
    async fn update_maps_empty_sprint_id_to_null() {
        for cleared in [json!(""), json!("null"), Value::Null] {
            let body = last_put_body(json!({ "work_item_id": UUID, "sprint_id": cleared })).await;
            assert_eq!(body, json!({ "sprint_id": null }), "for {cleared}");
        }
        let body = last_put_body(json!({ "work_item_id": UUID, "sprint_id": UUID })).await;
        assert_eq!(body, json!({ "sprint_id": UUID }));
        // Absent stays absent: a title-only update names no other field.
        let body = last_put_body(json!({ "work_item_id": UUID, "title": "t" })).await;
        assert_eq!(body, json!({ "title": "t" }));
    }

    #[tokio::test]
    async fn update_clears_assignee_and_due_date_the_same_way() {
        let body = last_put_body(json!({ "work_item_id": UUID, "assignee_id": "", "due_at": null })).await;
        assert_eq!(body, json!({ "assignee_id": null, "due_at": null }));
    }

    #[tokio::test]
    async fn update_sets_and_clears_the_milestone() {
        for cleared in [json!(""), json!("null"), Value::Null] {
            let body = last_put_body(json!({ "work_item_id": UUID, "milestone_id": cleared })).await;
            assert_eq!(body, json!({ "milestone_id": null }), "for {cleared}");
        }
        let body = last_put_body(json!({ "work_item_id": UUID, "milestone_id": UUID })).await;
        assert_eq!(body, json!({ "milestone_id": UUID }));
    }

    #[tokio::test]
    async fn create_carries_the_milestone_only_when_one_is_named() {
        let post_body = |args: Value| async move {
            let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
            let client = api.client().expect("client");
            let result = super::create_work_item(&client, args).await;
            assert_ne!(result.is_error, Some(true), "{result:?}");
            api.seen()
                .into_iter()
                .find(|seen| seen.method == "POST")
                .expect("a create was sent")
                .body
        };
        let body = post_body(json!({ "project_id": UUID, "title": "t", "milestone_id": UUID })).await;
        assert_eq!(body.get("milestone_id"), Some(&json!(UUID)));
        for none in [
            json!({ "project_id": UUID, "title": "t" }),
            json!({ "project_id": UUID, "title": "t", "milestone_id": "" }),
        ] {
            let body = post_body(none).await;
            assert!(body.get("milestone_id").is_none(), "{body}");
        }
    }

    #[tokio::test]
    async fn list_passes_the_milestone_filter() {
        for value in [UUID, "none"] {
            let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
            let client = api.client().expect("client");
            let result = list_work_items(&client, json!({ "project_id": UUID, "milestone_id": value })).await;
            assert_ne!(result.is_error, Some(true), "{result:?}");
            let seen = api.seen();
            let request = seen.iter().find(|seen| seen.method == "GET").expect("a list was sent");
            assert!(
                request.query.contains(&format!("milestone_id={value}")),
                "{}",
                request.query
            );
        }
    }

    #[tokio::test]
    async fn list_passes_the_sprint_filter() {
        let api = Api::spawn(Policy::Permissive).await.expect("stand-in api");
        let client = api.client().expect("client");
        let result = list_work_items(&client, json!({ "project_id": UUID, "sprint_id": "none" })).await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let seen = api.seen();
        let request = seen.iter().find(|seen| seen.method == "GET").expect("a list was sent");
        assert!(request.query.contains("sprint_id=none"), "{}", request.query);
    }

    #[tokio::test]
    async fn get_by_identifier_reports_the_api_error() {
        let router = Router::new().route(
            "/api/v1/issues/by-identifier/{identifier}",
            get(|| async {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "code": 404, "message": "work item 'PRX-9' not found" })),
                )
            }),
        );
        let url = test_api::spawn(router).await.expect("api");
        let client = test_api::client(url).expect("client");
        let result = get_work_item_by_identifier(&client, json!({ "identifier": "PRX-9" })).await;
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(format!("{result:?}").contains("PRX-9"), "{result:?}");
    }
}
