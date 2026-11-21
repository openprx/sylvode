//! The five agent task tools: `sylvode_task_get|ack|progress|complete|fail`.
//!
//! They call the task protocol endpoints under `/api/v1/ai/tasks` with the server's bot token. The
//! API is the authority: it serves and accepts a task only from the bot the task was assigned to
//! (`ensure_task_bot`), so a task of another bot, a user token or a connected application is
//! refused there with a fixed message. These tools validate the shape of their arguments, send
//! them through, and report what the API answered; they do not re-implement the ownership rule.
//! A connected application never reaches them: all five are in `WITHHELD_TOOLS`.

use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde_json::{Map, Value, json};
use uuid::Uuid;

pub const TASK_GET: &str = "sylvode_task_get";
pub const TASK_ACK: &str = "sylvode_task_ack";
pub const TASK_PROGRESS: &str = "sylvode_task_progress";
pub const TASK_COMPLETE: &str = "sylvode_task_complete";
pub const TASK_FAIL: &str = "sylvode_task_fail";

/// The five task tool names, in registry order.
pub const TASK_TOOL_NAMES: [&str; 5] = [TASK_GET, TASK_ACK, TASK_PROGRESS, TASK_COMPLETE, TASK_FAIL];

const MAX_PLAN_CHARS: usize = 4000;
const MAX_NOTE_CHARS: usize = 1000;
const MAX_SUMMARY_CHARS: usize = 20_000;
const MAX_ARTIFACTS: usize = 20;
const MAX_ARTIFACT_REF_CHARS: usize = 500;
const MAX_ARTIFACT_LABEL_CHARS: usize = 200;
const MAX_VOTE_REASON_CHARS: usize = 4000;
const MAX_FAIL_REASON_CHARS: usize = 2000;

/// Whether `name` is one of the five task tools.
pub fn is_task_tool(name: &str) -> bool {
    TASK_TOOL_NAMES.contains(&name)
}

fn task_id_property() -> Value {
    json!({ "type": "string", "format": "uuid" })
}

fn attempt_property() -> Value {
    json!({ "type": "integer", "minimum": 1 })
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: TASK_GET.to_string(),
            description: "Read a Sylvode task: source issue, comment or proposal, project, execution settings, \
                          status. Call it first."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["task_id", "attempt"],
                "properties": { "task_id": task_id_property(), "attempt": attempt_property() }
            }),
            annotations: Some(json!({ "readOnlyHint": true })),
        },
        ToolDefinition {
            name: TASK_ACK.to_string(),
            description: "Take the task and state a short plan. Then start work at once unless the answer says \
                          proceed=false."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["task_id", "attempt", "plan"],
                "properties": {
                    "task_id": task_id_property(),
                    "attempt": attempt_property(),
                    "plan": { "type": "string", "minLength": 1, "maxLength": MAX_PLAN_CHARS }
                }
            }),
            annotations: Some(json!({ "readOnlyHint": false, "idempotentHint": true })),
        },
        ToolDefinition {
            name: TASK_PROGRESS.to_string(),
            description: "Report progress. Also keeps the task alive: call at each milestone and at least every 10 \
                          minutes. The answer says whether the report was coalesced with a recent one."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["task_id", "attempt", "note"],
                "properties": {
                    "task_id": task_id_property(),
                    "attempt": attempt_property(),
                    "note": { "type": "string", "minLength": 1, "maxLength": MAX_NOTE_CHARS },
                    "percent": { "type": "integer", "minimum": 0, "maximum": 100 }
                }
            }),
            annotations: Some(json!({ "readOnlyHint": false, "idempotentHint": true })),
        },
        ToolDefinition {
            name: TASK_COMPLETE.to_string(),
            description: "Finish the task. Read `reply_policy` in the task: Sylvode posts the reply where the task \
                          came from (`comment`, `comment+state:...`, `vote`); do not post it yourself."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["task_id", "attempt", "outcome", "summary"],
                "properties": {
                    "task_id": task_id_property(),
                    "attempt": attempt_property(),
                    "outcome": { "type": "string", "enum": ["done", "needs_review", "no_change"] },
                    "summary": {
                        "type": "string", "minLength": 1, "maxLength": MAX_SUMMARY_CHARS,
                        "description": "Markdown. What you did and how to verify it."
                    },
                    "artifacts": {
                        "type": "array", "maxItems": MAX_ARTIFACTS,
                        "items": {
                            "type": "object", "additionalProperties": false, "required": ["kind", "ref"],
                            "properties": {
                                "kind": { "type": "string", "enum": ["commit", "branch", "pull_request", "file", "url"] },
                                "ref": { "type": "string", "minLength": 1, "maxLength": MAX_ARTIFACT_REF_CHARS },
                                "label": { "type": "string", "maxLength": MAX_ARTIFACT_LABEL_CHARS }
                            }
                        }
                    },
                    "vote": {
                        "type": "string", "enum": ["yes", "no", "abstain"],
                        "description": "Required for proposal_vote tasks."
                    },
                    "vote_reason": {
                        "type": "string", "minLength": 1, "maxLength": MAX_VOTE_REASON_CHARS,
                        "description": "At least 50 characters unless the project sets another minimum."
                    }
                }
            }),
            annotations: Some(json!({ "readOnlyHint": false })),
        },
        ToolDefinition {
            name: TASK_FAIL.to_string(),
            description: "End the task as failed with the reason. Use when you cannot do it; Sylvode reports it \
                          where the task came from."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["task_id", "attempt", "reason"],
                "properties": {
                    "task_id": task_id_property(),
                    "attempt": attempt_property(),
                    "reason": { "type": "string", "minLength": 1, "maxLength": MAX_FAIL_REASON_CHARS },
                    "retryable": {
                        "type": "boolean", "default": false,
                        "description": "true only for a temporary cause (service down)."
                    }
                }
            }),
            annotations: Some(json!({ "readOnlyHint": false })),
        },
    ]
}

/// The canonical task id and the attempt of a call. The id goes into a URL path, so only a UUID
/// is accepted and it is re-rendered in canonical form.
pub fn task_target(args: &Value) -> Result<(String, u32), String> {
    let task_id = args
        .get("task_id")
        .and_then(Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or("Invalid input: task_id must be a UUID")?
        .to_string();
    let attempt = args
        .get("attempt")
        .and_then(Value::as_u64)
        .and_then(|attempt| u32::try_from(attempt).ok())
        .filter(|attempt| (1..=i32::MAX as u32).contains(attempt))
        .ok_or("Invalid input: attempt must be an integer of at least 1")?;
    Ok((task_id, attempt))
}

fn object<'a>(args: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let object = args.as_object().ok_or("Invalid input: arguments must be an object")?;
    if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("Invalid input: unknown argument '{unknown}'"));
    }
    Ok(object)
}

/// A required text of 1 to `max` characters (after trimming, like the API does).
fn text(object: &Map<String, Value>, key: &str, max: usize) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Invalid input: {key} is required and must be a string"))?;
    bounded(key, value, max)
}

fn bounded(key: &str, value: &str, max: usize) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max {
        return Err(format!("Invalid input: {key} must be 1 to {max} characters"));
    }
    Ok(trimmed.to_string())
}

fn optional_text(object: &Map<String, Value>, key: &str, max: usize) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => bounded(key, value, max).map(Some),
        Some(_) => Err(format!("Invalid input: {key} must be a string")),
    }
}

fn one_of(object: &Map<String, Value>, key: &str, allowed: &[&str], required: bool) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) if !required => Ok(None),
        Some(Value::String(value)) if allowed.contains(&value.as_str()) => Ok(Some(value.clone())),
        _ => Err(format!("Invalid input: {key} must be one of {}", allowed.join(", "))),
    }
}

fn success(data: &Value) -> CallToolResult {
    CallToolResult::success(serde_json::to_string_pretty(data).unwrap_or_default())
}

/// The `data` of an API envelope; the envelope itself carries no information the agent needs.
fn data_of(envelope: &Value) -> Value {
    envelope.get("data").cloned().unwrap_or(Value::Null)
}

pub async fn get(client: &OpenPrClient, args: Value) -> CallToolResult {
    let target = object(&args, &["task_id", "attempt"]).and_then(|_| task_target(&args));
    match target {
        Ok((task_id, attempt)) => match client.get_ai_task(&task_id, attempt).await {
            Ok(envelope) => success(&data_of(&envelope)),
            Err(error) => CallToolResult::error(error),
        },
        Err(error) => CallToolResult::error(error),
    }
}

pub async fn ack(client: &OpenPrClient, args: Value) -> CallToolResult {
    let parsed = object(&args, &["task_id", "attempt", "plan"])
        .and_then(|object| text(object, "plan", MAX_PLAN_CHARS))
        .and_then(|plan| task_target(&args).map(|target| (target, plan)));
    match parsed {
        Ok(((task_id, attempt), plan)) => match client.ack_ai_task(&task_id, attempt, json!({ "plan": plan })).await {
            Ok(envelope) => success(&data_of(&envelope)),
            Err(error) => CallToolResult::error(error),
        },
        Err(error) => CallToolResult::error(error),
    }
}

pub async fn progress(client: &OpenPrClient, args: Value) -> CallToolResult {
    let parsed = object(&args, &["task_id", "attempt", "note", "percent"]).and_then(|object| {
        let note = text(object, "note", MAX_NOTE_CHARS)?;
        let percent = match object.get("percent") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|percent| *percent <= 100)
                    .ok_or("Invalid input: percent must be an integer from 0 to 100")?,
            ),
        };
        let mut body = json!({ "note": note });
        if let (Some(percent), Some(map)) = (percent, body.as_object_mut()) {
            map.insert("percent".to_string(), json!(percent));
        }
        Ok(body)
    });
    match parsed.and_then(|body| task_target(&args).map(|target| (target, body))) {
        Ok(((task_id, attempt), body)) => match client.report_ai_task_progress(&task_id, attempt, body).await {
            Ok(envelope) => success(&data_of(&envelope)),
            Err(error) => CallToolResult::error(error),
        },
        Err(error) => CallToolResult::error(error),
    }
}

fn complete_body(args: &Value) -> Result<Value, String> {
    let allowed = [
        "task_id",
        "attempt",
        "outcome",
        "summary",
        "artifacts",
        "vote",
        "vote_reason",
    ];
    let object = object(args, &allowed)?;
    let outcome = one_of(object, "outcome", &["done", "needs_review", "no_change"], true)?;
    let summary = text(object, "summary", MAX_SUMMARY_CHARS)?;
    let mut body = Map::new();
    body.insert("outcome".to_string(), json!(outcome));
    body.insert("summary".to_string(), json!(summary));
    match object.get("artifacts") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            if items.len() > MAX_ARTIFACTS {
                return Err(format!(
                    "Invalid input: artifacts must have at most {MAX_ARTIFACTS} items"
                ));
            }
            let mut artifacts = Vec::with_capacity(items.len());
            for item in items {
                let artifact = self::object(item, &["kind", "ref", "label"])?;
                let kind = one_of(
                    artifact,
                    "kind",
                    &["commit", "branch", "pull_request", "file", "url"],
                    true,
                )?;
                let reference = text(artifact, "ref", MAX_ARTIFACT_REF_CHARS)?;
                let mut entry = Map::new();
                entry.insert("kind".to_string(), json!(kind));
                entry.insert("ref".to_string(), json!(reference));
                if let Some(label) = optional_text(artifact, "label", MAX_ARTIFACT_LABEL_CHARS)? {
                    entry.insert("label".to_string(), json!(label));
                }
                artifacts.push(Value::Object(entry));
            }
            body.insert("artifacts".to_string(), Value::Array(artifacts));
        }
        Some(_) => return Err("Invalid input: artifacts must be an array".to_string()),
    }
    if let Some(vote) = one_of(object, "vote", &["yes", "no", "abstain"], false)? {
        body.insert("vote".to_string(), json!(vote));
    }
    if let Some(reason) = optional_text(object, "vote_reason", MAX_VOTE_REASON_CHARS)? {
        body.insert("vote_reason".to_string(), json!(reason));
    }
    Ok(Value::Object(body))
}

/// A 400 from the API: the request was understood and refused (for a task that already ended at
/// this attempt, among other reasons). The API carries its status in the envelope of an HTTP 200
/// (`API error 400 from ...`); a real HTTP 400 from something in front of it counts the same.
/// Transport failures, 403 and 404 do not.
fn is_bad_request(error: &str) -> bool {
    error.starts_with("API error 400 ") || error.starts_with("HTTP 400 ")
}

/// After a refused `complete` or `fail`: when the task ended at this attempt in exactly the state
/// the caller asked for, the call is reported as already done instead of an error. Any other state
/// keeps the API's own error.
async fn already_ended(
    client: &OpenPrClient,
    task_id: &str,
    attempt: u32,
    status: &str,
    flag: &str,
    api_error: String,
) -> CallToolResult {
    let Ok(view) = client.get_ai_task(task_id, attempt).await else {
        return CallToolResult::error(api_error);
    };
    let task = data_of(&view);
    let same_attempt = task.get("attempt").and_then(Value::as_u64) == Some(u64::from(attempt));
    if same_attempt && task.get("status").and_then(Value::as_str) == Some(status) {
        let mut result = Map::new();
        result.insert(flag.to_string(), json!(true));
        result.insert("task".to_string(), task);
        success(&Value::Object(result))
    } else {
        CallToolResult::error(api_error)
    }
}

pub async fn complete(client: &OpenPrClient, args: Value) -> CallToolResult {
    let parsed = complete_body(&args).and_then(|body| task_target(&args).map(|target| (target, body)));
    let ((task_id, attempt), body) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return CallToolResult::error(error),
    };
    match client.complete_ai_task(&task_id, attempt, body).await {
        Ok(envelope) => success(&data_of(&envelope)),
        Err(error) if is_bad_request(&error) => {
            already_ended(client, &task_id, attempt, "completed", "already_completed", error).await
        }
        Err(error) => CallToolResult::error(error),
    }
}

pub async fn fail(client: &OpenPrClient, args: Value) -> CallToolResult {
    let parsed = object(&args, &["task_id", "attempt", "reason", "retryable"]).and_then(|object| {
        let reason = text(object, "reason", MAX_FAIL_REASON_CHARS)?;
        let retryable = match object.get("retryable") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err("Invalid input: retryable must be a boolean".to_string()),
        };
        Ok(json!({ "reason": reason, "retryable": retryable }))
    });
    let ((task_id, attempt), body) = match parsed.and_then(|body| task_target(&args).map(|target| (target, body))) {
        Ok(parsed) => parsed,
        Err(error) => return CallToolResult::error(error),
    };
    match client.fail_ai_task(&task_id, attempt, body).await {
        Ok(envelope) => success(&data_of(&envelope)),
        Err(error) if is_bad_request(&error) => {
            already_ended(client, &task_id, attempt, "failed", "already_failed", error).await
        }
        Err(error) => CallToolResult::error(error),
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    clippy::significant_drop_tightening
)]
mod tests {
    use super::*;
    use crate::client::test_api;
    use crate::protocol::ToolContent;
    use crate::server::McpServer;
    use axum::body::to_bytes;
    use axum::extract::{Request, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::{Json, Router};
    use parking_lot::Mutex;
    use std::sync::Arc;

    const TASK: &str = "11111111-1111-4111-8111-111111111111";
    const PROJECT: &str = "22222222-2222-4222-8222-222222222222";

    #[derive(Clone, Debug)]
    struct Seen {
        method: String,
        path: String,
        query: String,
        body: Value,
        surface: Option<String>,
        tool: Option<String>,
    }

    type Answer = Arc<dyn Fn(&Seen) -> Value + Send + Sync>;

    #[derive(Clone)]
    struct Mock {
        seen: Arc<Mutex<Vec<Seen>>>,
        answer: Answer,
    }

    fn header_text(request: &Request, name: &str) -> Option<String> {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    async fn handle(State(mock): State<Mock>, request: Request) -> impl IntoResponse {
        let (method, path, query) = (
            request.method().to_string(),
            request.uri().path().to_string(),
            request.uri().query().unwrap_or_default().to_string(),
        );
        let (surface, tool) = (
            header_text(&request, "x-sylvode-mcp-surface"),
            header_text(&request, "x-sylvode-mcp-tool"),
        );
        let bytes = to_bytes(request.into_body(), 1 << 20).await.unwrap_or_default();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let seen = Seen {
            method,
            path,
            query,
            body,
            surface,
            tool,
        };
        let reply = (mock.answer)(&seen);
        mock.seen.lock().push(seen);
        (StatusCode::OK, Json(reply))
    }

    fn ok(data: Value) -> Value {
        json!({ "code": 0, "message": "success", "data": data })
    }

    fn view(status: &str, attempt: u64) -> Value {
        json!({
            "task_id": TASK, "attempt": attempt, "status": status, "active": status == "processing",
            "project": { "id": PROJECT, "key": "SVWL", "name": "n", "workspace_id": PROJECT },
            "reply_policy": { "on_complete": "comment", "on_fail": "comment" }
        })
    }

    /// The default answers of the real endpoints.
    fn protocol_answer(seen: &Seen) -> Value {
        match (seen.method.as_str(), seen.path.rsplit('/').next().unwrap_or_default()) {
            ("GET", id) if id == TASK => ok(view("processing", 2)),
            (_, "ack") => ok(json!({ "task": view("acknowledged", 2), "proceed": true })),
            (_, "progress") => ok(json!({ "lease_expires_at": "2026-11-21T10:00:00+00:00", "coalesced": false })),
            (_, "complete") => ok(json!({ "task": view("completed", 2), "reply": { "comment_id": null } })),
            (_, "fail") => ok(json!({ "task": view("failed", 2), "will_retry": false, "reply": {} })),
            (_, "agent-policy") => ok(json!({})),
            _ => json!({ "code": 404, "message": "not found" }),
        }
    }

    async fn spawn_mock(answer: Answer) -> Result<(String, Arc<Mutex<Vec<Seen>>>), Box<dyn std::error::Error>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new().fallback(handle).with_state(Mock {
            seen: Arc::clone(&seen),
            answer,
        });
        Ok((test_api::spawn(router).await?, seen))
    }

    async fn mock(answer: Answer) -> Result<(McpServer, Arc<Mutex<Vec<Seen>>>), Box<dyn std::error::Error>> {
        let (url, seen) = spawn_mock(answer).await?;
        Ok((
            McpServer::new(test_api::client(url)?, platform::config::McpToolMode::Full),
            seen,
        ))
    }

    fn text(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|item| match item {
                ToolContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn json_of(result: &CallToolResult) -> Value {
        serde_json::from_str(&text(result)).unwrap_or(Value::Null)
    }

    // ---- schemas ----

    #[test]
    fn the_schemas_match_the_golden_file() {
        let golden: Value = serde_json::from_str(include_str!("../../tests/golden/task-tools.json"))
            .expect("the golden file is valid JSON");
        let current = serde_json::to_value(tool_definitions()).expect("definitions serialize");
        assert_eq!(
            current, golden,
            "a task tool schema changed: review and update the golden file"
        );
    }

    #[test]
    fn every_task_tool_is_defined_once_with_a_closed_object_schema() {
        let definitions = tool_definitions();
        assert_eq!(
            definitions.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            TASK_TOOL_NAMES
        );
        for tool in &definitions {
            assert_eq!(
                tool.input_schema.get("additionalProperties"),
                Some(&json!(false)),
                "{}",
                tool.name
            );
            assert!(
                tool.input_schema.pointer("/properties/task_id").is_some(),
                "{}",
                tool.name
            );
            assert!(
                tool.input_schema.pointer("/properties/attempt").is_some(),
                "{}",
                tool.name
            );
        }
        let complete = definitions
            .iter()
            .find(|tool| tool.name == TASK_COMPLETE)
            .expect("complete");
        assert_eq!(
            complete.input_schema.pointer("/properties/vote_reason/minLength"),
            Some(&json!(1))
        );
        assert!(
            complete
                .input_schema
                .pointer("/properties/vote_reason/description")
                .and_then(Value::as_str)
                .is_some_and(|text| text.starts_with("At least 50 characters unless the project sets another minimum"))
        );
        assert!(
            complete.description.contains("`reply_policy`") && complete.description.contains("do not post it yourself")
        );
    }

    // ---- one mock-API test per tool ----

    #[tokio::test]
    async fn get_reads_the_view_of_the_attempt() -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(Arc::new(protocol_answer)).await?;
        let result = server
            .execute_tool_for_catalog_witness(TASK_GET, json!({ "task_id": TASK, "attempt": 2 }))
            .await;
        assert_eq!(result.is_error, None, "{}", text(&result));
        assert_eq!(json_of(&result).get("status"), Some(&json!("processing")));
        let seen = seen.lock();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path.as_str()),
            ("GET", format!("/api/v1/ai/tasks/{TASK}").as_str())
        );
        assert_eq!(seen[0].query, "attempt=2");
        assert_eq!(seen[0].surface.as_deref(), Some("mcp_stdio"));
        Ok(())
    }

    #[tokio::test]
    async fn ack_sends_the_plan() -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(Arc::new(protocol_answer)).await?;
        let result = server
            .execute_tool_for_catalog_witness(
                TASK_ACK,
                json!({ "task_id": TASK, "attempt": 2, "plan": "  read, fix, test " }),
            )
            .await;
        assert_eq!(result.is_error, None, "{}", text(&result));
        assert_eq!(json_of(&result).get("proceed"), Some(&json!(true)));
        let seen = seen.lock();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path.as_str()),
            ("POST", format!("/api/v1/ai/tasks/{TASK}/ack").as_str())
        );
        assert_eq!(seen[0].query, "attempt=2");
        assert_eq!(seen[0].body, json!({ "plan": "read, fix, test" }));
        assert_eq!(seen[0].surface.as_deref(), Some("mcp_stdio"));
        Ok(())
    }

    #[tokio::test]
    async fn progress_sends_note_and_percent_without_a_kind() -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(Arc::new(protocol_answer)).await?;
        let first = server
            .execute_tool_for_catalog_witness(
                TASK_PROGRESS,
                json!({ "task_id": TASK, "attempt": 2, "note": "half", "percent": 50 }),
            )
            .await;
        let second = server
            .execute_tool_for_catalog_witness(TASK_PROGRESS, json!({ "task_id": TASK, "attempt": 2, "note": "again" }))
            .await;
        for result in [&first, &second] {
            assert_eq!(result.is_error, None, "{}", text(result));
            assert_eq!(json_of(result).get("coalesced"), Some(&json!(false)));
            assert!(json_of(result).get("lease_expires_at").is_some());
        }
        let seen = seen.lock();
        assert_eq!(seen[0].path, format!("/api/v1/ai/tasks/{TASK}/progress"));
        assert_eq!(seen[0].query, "attempt=2");
        assert_eq!(seen[0].body, json!({ "note": "half", "percent": 50 }));
        assert_eq!(seen[1].body, json!({ "note": "again" }));
        for request in seen.iter() {
            assert!(request.body.get("kind").is_none(), "the agent never sends a kind");
        }
        Ok(())
    }

    #[tokio::test]
    async fn complete_sends_the_outcome_and_the_optional_parts() -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(Arc::new(protocol_answer)).await?;
        let result = server
            .execute_tool_for_catalog_witness(
                TASK_COMPLETE,
                json!({
                    "task_id": TASK, "attempt": 2, "outcome": "done", "summary": "fixed",
                    "artifacts": [{ "kind": "commit", "ref": "abc123", "label": "fix" }, { "kind": "url", "ref": "https://x" }],
                    "vote": "yes", "vote_reason": "because"
                }),
            )
            .await;
        assert_eq!(result.is_error, None, "{}", text(&result));
        assert!(json_of(&result).get("reply").is_some());
        let seen = seen.lock();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].path, format!("/api/v1/ai/tasks/{TASK}/complete"));
        assert_eq!(seen[0].query, "attempt=2");
        assert_eq!(
            seen[0].body,
            json!({
                "outcome": "done", "summary": "fixed",
                "artifacts": [{ "kind": "commit", "ref": "abc123", "label": "fix" }, { "kind": "url", "ref": "https://x" }],
                "vote": "yes", "vote_reason": "because"
            })
        );
        assert_eq!(seen[0].surface.as_deref(), Some("mcp_stdio"));
        Ok(())
    }

    #[tokio::test]
    async fn fail_sends_the_reason_and_no_source() -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(Arc::new(protocol_answer)).await?;
        let result = server
            .execute_tool_for_catalog_witness(
                TASK_FAIL,
                json!({ "task_id": TASK, "attempt": 2, "reason": "cannot", "retryable": true }),
            )
            .await;
        assert_eq!(result.is_error, None, "{}", text(&result));
        assert_eq!(json_of(&result).get("will_retry"), Some(&json!(false)));
        let seen = seen.lock();
        assert_eq!(seen[0].path, format!("/api/v1/ai/tasks/{TASK}/fail"));
        assert_eq!(seen[0].query, "attempt=2");
        assert_eq!(seen[0].body, json!({ "reason": "cannot", "retryable": true }));
        assert!(seen[0].body.get("source").is_none());
        Ok(())
    }

    // ---- already ended ----

    fn refused_then_view(status: &'static str) -> Answer {
        Arc::new(move |seen: &Seen| {
            if seen.method == "POST" {
                json!({ "code": 400, "message": "task is not active for attempt 2 (current attempt 2)" })
            } else {
                ok(view(status, 2))
            }
        })
    }

    #[tokio::test]
    async fn complete_after_complete_is_reported_as_already_completed() -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(refused_then_view("completed")).await?;
        let result = server
            .execute_tool_for_catalog_witness(
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 2, "outcome": "done", "summary": "s" }),
            )
            .await;
        assert_eq!(result.is_error, None, "{}", text(&result));
        let body = json_of(&result);
        assert_eq!(body.get("already_completed"), Some(&json!(true)));
        assert_eq!(body.pointer("/task/status"), Some(&json!("completed")));
        let seen = seen.lock();
        assert_eq!(
            seen.iter().map(|s| s.method.as_str()).collect::<Vec<_>>(),
            ["POST", "GET"]
        );
        assert_eq!(seen[1].query, "attempt=2");
        Ok(())
    }

    #[tokio::test]
    async fn fail_after_fail_is_reported_as_already_failed() -> Result<(), Box<dyn std::error::Error>> {
        let (server, _) = mock(refused_then_view("failed")).await?;
        let result = server
            .execute_tool_for_catalog_witness(TASK_FAIL, json!({ "task_id": TASK, "attempt": 2, "reason": "r" }))
            .await;
        assert_eq!(result.is_error, None, "{}", text(&result));
        let body = json_of(&result);
        assert_eq!(body.get("already_failed"), Some(&json!(true)));
        assert_eq!(body.pointer("/task/status"), Some(&json!("failed")));
        Ok(())
    }

    #[tokio::test]
    async fn any_other_state_keeps_the_api_error() -> Result<(), Box<dyn std::error::Error>> {
        // complete after the task failed, fail after the task completed, complete of a stale attempt
        for (tool, args, status, attempt) in [
            (
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 2, "outcome": "done", "summary": "s" }),
                "failed",
                2,
            ),
            (
                TASK_FAIL,
                json!({ "task_id": TASK, "attempt": 2, "reason": "r" }),
                "completed",
                2,
            ),
            (
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 2, "outcome": "done", "summary": "s" }),
                "completed",
                3,
            ),
            (
                TASK_FAIL,
                json!({ "task_id": TASK, "attempt": 2, "reason": "r" }),
                "processing",
                2,
            ),
        ] {
            let answer: Answer = Arc::new(move |seen: &Seen| {
                if seen.method == "POST" {
                    json!({ "code": 400, "message": "task is not active for attempt 2 (current attempt 3)" })
                } else {
                    ok(view(status, attempt))
                }
            });
            let (server, _) = mock(answer).await?;
            let result = server.execute_tool_for_catalog_witness(tool, args).await;
            assert_eq!(result.is_error, Some(true), "{tool} after {status}@{attempt}");
            assert!(text(&result).contains("task is not active"), "{}", text(&result));
        }
        Ok(())
    }

    #[tokio::test]
    async fn errors_other_than_a_400_are_not_turned_into_success() -> Result<(), Box<dyn std::error::Error>> {
        let answer: Answer = Arc::new(|seen: &Seen| {
            if seen.method == "POST" {
                json!({ "code": 403, "message": "task can only be operated by its own bot" })
            } else {
                ok(view("completed", 2))
            }
        });
        let (server, seen) = mock(answer).await?;
        let result = server
            .execute_tool_for_catalog_witness(
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 2, "outcome": "done", "summary": "s" }),
            )
            .await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(seen.lock().len(), 1, "a 403 must not trigger the already-ended read");
        Ok(())
    }

    // ---- argument validation: nothing reaches the API ----

    #[tokio::test]
    async fn invalid_arguments_never_reach_the_api() -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(Arc::new(protocol_answer)).await?;
        let long = "x".repeat(MAX_PLAN_CHARS + 1);
        let cases = [
            (TASK_GET, json!({ "task_id": "../admin", "attempt": 1 })),
            (TASK_GET, json!({ "task_id": TASK, "attempt": 0 })),
            (TASK_GET, json!({ "task_id": TASK })),
            (TASK_GET, json!({ "task_id": TASK, "attempt": 1, "extra": 1 })),
            (TASK_ACK, json!({ "task_id": TASK, "attempt": 1, "plan": "  " })),
            (TASK_ACK, json!({ "task_id": TASK, "attempt": 1, "plan": long })),
            (
                TASK_PROGRESS,
                json!({ "task_id": TASK, "attempt": 1, "note": "n", "percent": 101 }),
            ),
            (
                TASK_PROGRESS,
                json!({ "task_id": TASK, "attempt": 1, "note": "n", "kind": "runner.alive" }),
            ),
            (
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 1, "outcome": "ok", "summary": "s" }),
            ),
            (
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 1, "outcome": "done", "summary": "s", "vote": "maybe" }),
            ),
            (
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 1, "outcome": "done", "summary": "s", "artifacts": [{ "kind": "zip", "ref": "r" }] }),
            ),
            (
                TASK_FAIL,
                json!({ "task_id": TASK, "attempt": 1, "reason": "r", "source": "runner" }),
            ),
            (
                TASK_FAIL,
                json!({ "task_id": TASK, "attempt": 1, "reason": "r", "retryable": "yes" }),
            ),
            (TASK_FAIL, json!([])),
        ];
        for (tool, args) in cases {
            let result = server.execute_tool_for_catalog_witness(tool, args.clone()).await;
            assert_eq!(result.is_error, Some(true), "{tool} {args}");
        }
        assert!(seen.lock().is_empty(), "{:?}", seen.lock());
        Ok(())
    }

    // ---- the policy chain, the owner lookup and the refusal of a foreign task ----

    #[tokio::test]
    async fn a_task_call_resolves_its_project_applies_its_policy_and_logs_the_tool()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server, seen) = mock(Arc::new(protocol_answer)).await?;
        let result = server
            .call_tool(TASK_ACK, json!({ "task_id": TASK, "attempt": 2, "plan": "p" }))
            .await;
        assert_eq!(result.is_error, None, "{}", text(&result));
        let seen = seen.lock();
        assert_eq!(
            seen.iter()
                .map(|s| (s.method.as_str(), s.path.clone()))
                .collect::<Vec<_>>(),
            [
                ("GET", format!("/api/v1/ai/tasks/{TASK}")),
                ("GET", format!("/api/v1/projects/{PROJECT}/agent-policy")),
                ("POST", format!("/api/v1/ai/tasks/{TASK}/ack")),
            ]
        );
        for request in seen.iter() {
            assert_eq!(request.tool.as_deref(), Some(TASK_ACK));
            assert_eq!(request.surface.as_deref(), Some("mcp_stdio"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn the_project_agent_policy_applies_to_a_task_tool() -> Result<(), Box<dyn std::error::Error>> {
        let answer: Answer = Arc::new(|seen: &Seen| {
            if seen.path.ends_with("/agent-policy") {
                ok(json!({ "mcp": { "tool_registry": { "enabled_tools": ["work_items.list"] } } }))
            } else {
                protocol_answer(seen)
            }
        });
        let (server, seen) = mock(answer).await?;
        let result = server
            .call_tool(
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 2, "outcome": "done", "summary": "s" }),
            )
            .await;
        assert_eq!(result.is_error, Some(true));
        assert!(
            text(&result).contains("disabled by project agent policy"),
            "{}",
            text(&result)
        );
        assert!(seen.lock().iter().all(|request| request.method == "GET"));
        Ok(())
    }

    /// A task of another bot is refused by the API; the tool reports that refusal and nothing else.
    /// The mock leaks a secret in a field the tool must never relay, to prove the answer is the
    /// API's message alone, and no write follows.
    #[tokio::test]
    async fn a_task_of_another_bot_is_a_tool_error_without_its_content() -> Result<(), Box<dyn std::error::Error>> {
        const SECRET: &str = "SECRET-TASK-CONTENT";
        let answer: Answer = Arc::new(|_: &Seen| {
            json!({
                "code": 403, "message": "task can only be operated by its own bot",
                "data": { "plan": SECRET, "source": { "issue": { "title": SECRET } } }
            })
        });
        let (server, seen) = mock(answer).await?;
        for (tool, args) in [
            (TASK_GET, json!({ "task_id": TASK, "attempt": 1 })),
            (TASK_ACK, json!({ "task_id": TASK, "attempt": 1, "plan": "p" })),
            (TASK_PROGRESS, json!({ "task_id": TASK, "attempt": 1, "note": "n" })),
            (
                TASK_COMPLETE,
                json!({ "task_id": TASK, "attempt": 1, "outcome": "done", "summary": "s" }),
            ),
            (TASK_FAIL, json!({ "task_id": TASK, "attempt": 1, "reason": "r" })),
        ] {
            let through_the_chain = server.call_tool(tool, args.clone()).await;
            let direct = server.execute_tool_for_catalog_witness(tool, args).await;
            for result in [through_the_chain, direct] {
                assert_eq!(result.is_error, Some(true), "{tool}");
                let message = text(&result);
                assert!(
                    message.contains("task can only be operated by its own bot"),
                    "{message}"
                );
                assert!(!message.contains(SECRET), "{tool} leaked: {message}");
            }
        }
        assert!(
            seen.lock()
                .iter()
                .all(|request| request.method == "GET" || request.path.starts_with("/api/v1/ai/tasks/")),
            "no write outside the task endpoints"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_delegated_server_refuses_every_task_tool_before_any_request() -> Result<(), Box<dyn std::error::Error>> {
        let (url, seen) = spawn_mock(Arc::new(protocol_answer)).await?;
        let client = test_api::client(url)?;
        let grant = crate::oauth::DelegatedGrant {
            scopes: api::oauth::scopes::ScopeSet::all(),
            workspace_id: Uuid::from_u128(1),
            projects: None,
        };
        let delegated = McpServer::new_delegated(client, grant);
        for tool in TASK_TOOL_NAMES {
            let result = delegated
                .call_tool(tool, json!({ "task_id": TASK, "attempt": 1, "plan": "p" }))
                .await;
            assert_eq!(result.is_error, Some(true), "{tool}");
            assert_eq!(
                text(&result),
                "Error: tool not available to connected applications",
                "{tool}"
            );
        }
        assert!(seen.lock().is_empty());
        Ok(())
    }
}
