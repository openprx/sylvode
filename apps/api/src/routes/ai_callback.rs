// Explicit branches preserve the established evaluation order and mutation points.
#![allow(clippy::option_if_let_else)]

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::timestamp::Timestamp;
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::{ApiResponse, PaginatedData},
    services::{
        ai_task_service::{
            ACTIVE_TASK_STATUSES_SQL, AiTaskRow, AiTaskWithProtocol, AttemptEnd, CreateAiTaskInput, DispatchOrigin,
            create_ai_task, insert_ai_task_business_event, insert_ai_task_event, is_active_status, load_protocol_row,
            next_retry_time, notify_ai_task_failed, status_after_failed_attempt, task_with_protocol_columns,
            valid_reference_type, valid_task_type,
        },
        trust_score_service::{is_project_admin_or_owner, is_project_member, is_system_admin},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Deserialize)]
pub struct ListAiTasksQuery {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAiTaskRequest {
    pub ai_participant_id: Uuid,
    pub task_type: String,
    pub reference_type: Option<String>,
    pub reference_id: Option<Uuid>,
    pub priority: Option<i32>,
    pub payload: Option<Value>,
    pub idempotency_key: Option<String>,
    pub max_attempts: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct FailTaskRequest {
    pub error_message: Option<String>,
    pub payload: Option<Value>,
    /// `false` fails the task now instead of scheduling a retry while attempts remain: the caller
    /// knows a repeat cannot succeed (for example no agent matches the task). Defaults to `true`.
    pub retry: Option<bool>,
}

/// The optional `?attempt=N` of the complete and fail callbacks.
///
/// `N` is the attempt number (the delivery's `attempts`) the caller ran. With it, the callback is accepted only while that very
/// attempt is active (`processing` or `acknowledged`), so a late callback from an attempt the worker has already timed out
/// and re-queued, or failed, cannot end a later attempt or a re-queued task. Without it the
/// callback behaves as before.
#[derive(Debug, Default, Deserialize)]
pub struct CallbackAttemptQuery {
    pub attempt: Option<i32>,
}

/// Whether a complete (`allow_pending = false`) or fail (`allow_pending = true`) callback for
/// `attempt` may end a task in `status` at attempt `attempts`. An attempt-qualified callback needs
/// that attempt to be active; an unqualified fail also accepts a task already rescheduled to
/// `pending`, as it always has.
fn check_callback_attempt(
    status: &str,
    attempts: i32,
    attempt: Option<i32>,
    allow_pending: bool,
) -> Result<(), String> {
    let active = is_active_status(status);
    match attempt {
        Some(attempt) if active && attempt == attempts => Ok(()),
        Some(attempt) if active => Err(format!(
            "task is not active for attempt {attempt} (current attempt {attempts})"
        )),
        None if active || (allow_pending && status == "pending") => Ok(()),
        None if allow_pending => Err("task cannot be failed in current status".to_string()),
        Some(_) | None => Err("task is not active".to_string()),
    }
}

#[derive(Debug, Serialize)]
pub struct AiTaskResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub ai_participant_id: Uuid,
    pub task_type: String,
    pub reference_type: Option<String>,
    pub reference_id: Option<Uuid>,
    pub status: String,
    pub priority: i32,
    pub payload: Value,
    pub result: Option<Value>,
    pub error_message: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub next_retry_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub acknowledged_at: Option<String>,
    pub plan: Option<String>,
    pub last_progress_at: Option<String>,
    pub lease_expires_at: Option<String>,
    pub runner_phase: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ProjectRow {
    workspace_id: Uuid,
}

impl From<AiTaskWithProtocol> for AiTaskResponse {
    fn from(entry: AiTaskWithProtocol) -> Self {
        let AiTaskWithProtocol { task: value, protocol } = entry;
        Self {
            id: value.id,
            project_id: value.project_id,
            ai_participant_id: value.ai_participant_id,
            task_type: value.task_type,
            reference_type: value.reference_type,
            reference_id: value.reference_id,
            status: value.status,
            priority: value.priority,
            payload: value.payload,
            result: value.result,
            error_message: value.error_message,
            attempts: value.attempts,
            max_attempts: value.max_attempts,
            next_retry_at: value.next_retry_at.map(platform::timestamp::Timestamp::to_rfc3339),
            started_at: value.started_at.map(platform::timestamp::Timestamp::to_rfc3339),
            completed_at: value.completed_at.map(platform::timestamp::Timestamp::to_rfc3339),
            created_at: value.created_at.to_rfc3339(),
            updated_at: value.updated_at.to_rfc3339(),
            acknowledged_at: protocol.acknowledged_at.map(platform::timestamp::Timestamp::to_rfc3339),
            plan: protocol.plan,
            last_progress_at: protocol
                .last_progress_at
                .map(platform::timestamp::Timestamp::to_rfc3339),
            lease_expires_at: protocol
                .lease_expires_at
                .map(platform::timestamp::Timestamp::to_rfc3339),
            runner_phase: protocol.runner_phase,
        }
    }
}

/// The response for a task row that was just written: its protocol columns are read next to it.
async fn response_for(db: &impl ConnectionTrait, task: AiTaskRow) -> Result<AiTaskResponse, ApiError> {
    let protocol = load_protocol_row(db, task.id).await?;
    Ok(AiTaskResponse::from(AiTaskWithProtocol { task, protocol }))
}

pub async fn complete_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(task_id): Path<Uuid>,
    Query(attempt): Query<CallbackAttemptQuery>,
    Json(result): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_user_id(&claims)?;
    let mut task = find_task(&state, task_id).await?;
    ensure_actor_can_operate(&state, &task, actor_id).await?;

    check_callback_attempt(&task.status, task.attempts, attempt.attempt, false).map_err(ApiError::BadRequest)?;

    let previous_status = task.status.clone();
    let now = Timestamp::now();
    // Compare-and-set on the state checked above: a worker timeout, a fail callback or a second
    // complete that commits in between makes this update match nothing instead of overwriting it.
    let complete_sql = format!(
        r"
            UPDATE ai_tasks
            SET status = 'completed',
                result = $2,
                error_message = NULL,
                completed_at = $3,
                updated_at = $3
            WHERE id = $1
              AND status IN {ACTIVE_TASK_STATUSES_SQL}
              AND attempts = $4
            RETURNING
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                result,
                error_message,
                idempotency_key,
                attempts,
                max_attempts,
                next_retry_at,
                started_at,
                completed_at,
                created_at,
                updated_at
        "
    );
    let updated = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &complete_sql,
        vec![task_id.into(), result.clone().into(), now.into(), task.attempts.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::BadRequest("task is not active".to_string()))?;

    task = updated.clone();
    insert_ai_task_event(&state.db, task_id, "completed", json!({ "result": result })).await?;
    insert_ai_task_business_event(
        &state.db,
        &task,
        "ai_task.completed",
        Some(actor_id),
        json!({ "type": "ai_callback", "actor_id": actor_id }),
        json!({ "result": task.result }),
        Some(previous_status.as_str()),
    )
    .await?;
    let project = find_project(&state, task.project_id).await?;
    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::AiTaskCompleted,
            workspace_id: project.workspace_id,
            project_id: task.project_id,
            actor_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(json!({
                "task_id": task.id.to_string(),
                "task_type": task.task_type,
                "status": task.status,
                "reference_type": task.reference_type,
                "reference_id": task.reference_id.map(|v| v.to_string()),
                "result": task.result,
            })),
        },
    );

    Ok(ApiResponse::success(response_for(&state.db, task).await?))
}

pub async fn fail_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(task_id): Path<Uuid>,
    Query(attempt): Query<CallbackAttemptQuery>,
    Json(req): Json<FailTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_user_id(&claims)?;
    let mut task = find_task(&state, task_id).await?;
    ensure_actor_can_operate(&state, &task, actor_id).await?;

    check_callback_attempt(&task.status, task.attempts, attempt.attempt, true).map_err(ApiError::BadRequest)?;

    let previous_status = task.status.clone();
    let now = Timestamp::now();
    let message = req
        .error_message
        .clone()
        .unwrap_or_else(|| "task execution failed".to_string());

    let end = if req.retry.unwrap_or(true) {
        AttemptEnd::RetryWhileAttemptsRemain
    } else {
        AttemptEnd::Terminal
    };
    let will_retry = status_after_failed_attempt(task.attempts, task.max_attempts, end) == "pending";
    let (next_status, next_retry_at, event_type) = if will_retry {
        (
            "pending",
            Some(next_retry_time(task.attempts + 1)),
            "retried".to_string(),
        )
    } else {
        ("failed", None, "failed".to_string())
    };

    let updated = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE ai_tasks
            SET status = $2,
                error_message = $3,
                next_retry_at = $4,
                completed_at = CASE WHEN $2 = 'failed' THEN $5 ELSE completed_at END,
                updated_at = $5
            WHERE id = $1
              AND status = $6
              AND attempts = $7
            RETURNING
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                result,
                error_message,
                idempotency_key,
                attempts,
                max_attempts,
                next_retry_at,
                started_at,
                completed_at,
                created_at,
                updated_at
        ",
        vec![
            task_id.into(),
            next_status.into(),
            message.clone().into(),
            next_retry_at.into(),
            now.into(),
            previous_status.clone().into(),
            task.attempts.into(),
        ],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::BadRequest("task cannot be failed in current status".to_string()))?;

    task = updated.clone();
    insert_ai_task_event(
        &state.db,
        task_id,
        &event_type,
        json!({
            "error_message": message,
            "payload": req.payload.clone(),
            "attempts": task.attempts,
            "max_attempts": task.max_attempts,
            "next_retry_at": task.next_retry_at.map(platform::timestamp::Timestamp::to_rfc3339),
        }),
    )
    .await?;
    insert_ai_task_business_event(
        &state.db,
        &task,
        if task.status == "failed" {
            "ai_task.failed"
        } else {
            "ai_task.retried"
        },
        Some(actor_id),
        json!({ "type": "ai_callback", "actor_id": actor_id }),
        json!({
            "error_message": message,
            "payload": req.payload.clone(),
            "attempts": task.attempts,
            "max_attempts": task.max_attempts,
            "next_retry_at": task.next_retry_at.map(platform::timestamp::Timestamp::to_rfc3339),
        }),
        Some(previous_status.as_str()),
    )
    .await?;

    if task.status == "failed" {
        notify_ai_task_failed(&state, &task, actor_id).await?;
    }

    Ok(ApiResponse::success(response_for(&state.db, task).await?))
}

pub async fn report_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(task_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_user_id(&claims)?;
    let task = find_task(&state, task_id).await?;
    ensure_actor_can_operate(&state, &task, actor_id).await?;

    insert_ai_task_event(&state.db, task_id, "progress", payload.clone()).await?;
    insert_ai_task_business_event(
        &state.db,
        &task,
        "ai_task.progress",
        Some(actor_id),
        json!({ "type": "ai_callback", "actor_id": actor_id }),
        json!({ "progress": payload }),
        Some(task.status.as_str()),
    )
    .await?;
    Ok(ApiResponse::ok())
}

pub async fn list_project_ai_tasks(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListAiTasksQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user_id(&claims)?;
    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    if let Some(status) = query.status.as_ref()
        && !matches!(
            status.as_str(),
            "pending" | "processing" | "acknowledged" | "completed" | "failed" | "cancelled"
        )
    {
        return Err(ApiError::BadRequest("invalid status filter".to_string()));
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let (count_sql, count_values) = if let Some(status) = query.status.clone() {
        (
            "SELECT COUNT(*)::bigint AS count FROM ai_tasks WHERE project_id = $1 AND status = $2".to_string(),
            vec![project_id.into(), status.into()],
        )
    } else {
        (
            "SELECT COUNT(*)::bigint AS count FROM ai_tasks WHERE project_id = $1".to_string(),
            vec![project_id.into()],
        )
    };

    let total = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &count_sql,
            count_values,
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")?;

    let total_pages = if total == 0 {
        0
    } else {
        total / per_page + i64::from(total % per_page != 0)
    };

    let (list_sql, list_values) = if let Some(status) = query.status {
        (
            r"
                SELECT
                    id,
                    project_id,
                    ai_participant_id,
                    task_type,
                    reference_type,
                    reference_id,
                    status,
                    priority,
                    payload,
                    result,
                    error_message,
                    idempotency_key,
                    attempts,
                    max_attempts,
                    next_retry_at,
                    started_at,
                    completed_at,
                    created_at,
                    updated_at,
                    acknowledged_at,
                    plan,
                    last_progress_at,
                    lease_expires_at,
                    runner_phase
                FROM ai_tasks
                WHERE project_id = $1 AND status = $2
                ORDER BY priority DESC, created_at DESC
                LIMIT $3 OFFSET $4
            "
            .to_string(),
            vec![project_id.into(), status.into(), per_page.into(), offset.into()],
        )
    } else {
        (
            r"
                SELECT
                    id,
                    project_id,
                    ai_participant_id,
                    task_type,
                    reference_type,
                    reference_id,
                    status,
                    priority,
                    payload,
                    result,
                    error_message,
                    idempotency_key,
                    attempts,
                    max_attempts,
                    next_retry_at,
                    started_at,
                    completed_at,
                    created_at,
                    updated_at,
                    acknowledged_at,
                    plan,
                    last_progress_at,
                    lease_expires_at,
                    runner_phase
                FROM ai_tasks
                WHERE project_id = $1
                ORDER BY priority DESC, created_at DESC
                LIMIT $2 OFFSET $3
            "
            .to_string(),
            vec![project_id.into(), per_page.into(), offset.into()],
        )
    };

    let items = AiTaskWithProtocol::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &list_sql,
        list_values,
    ))
    .all(&state.db)
    .await?
    .into_iter()
    .map(AiTaskResponse::from)
    .collect::<Vec<_>>();

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

/// At most this many tasks are listed for one issue.
const ISSUE_TASK_LIMIT: i64 = 50;
/// At most this many events are listed for one task, oldest first.
const TASK_EVENT_LIMIT: i64 = 100;
/// A string inside an event payload is cut to this many characters.
const EVENT_STRING_CHARS: usize = 2000;

#[derive(Debug, FromQueryResult)]
struct IssueScopeRow {
    project_id: Uuid,
    workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct TaskEventRow {
    task_id: Uuid,
    event_type: String,
    payload: Value,
    created_at: Timestamp,
}

/// `payload` with every string value cut to [`EVENT_STRING_CHARS`] characters.
fn cut_long_strings(payload: Value) -> Value {
    match payload {
        Value::String(text) => match text.char_indices().nth(EVENT_STRING_CHARS) {
            Some((end, _)) => Value::String(text[..end].to_string()),
            None => Value::String(text),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(cut_long_strings).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, cut_long_strings(value)))
                .collect(),
        ),
        other => other,
    }
}

/// `GET /api/v1/issues/{issue_id}/ai-tasks`: the agent tasks of one issue, newest first, each with
/// its events oldest first. Members of the issue's project and bots of its workspace may read it.
pub async fn list_issue_ai_tasks(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let scope = IssueScopeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT wi.project_id, p.workspace_id FROM work_items wi \
         INNER JOIN projects p ON p.id = wi.project_id WHERE wi.id = $1",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;
    if let Some(Extension(bot)) = bot {
        if bot.workspace_id != scope.workspace_id {
            return Err(ApiError::Forbidden("bot not authorized for this issue".to_string()));
        }
    } else if !is_project_member(&state.db, scope.project_id, parse_user_id(&claims)?).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let tasks = AiTaskWithProtocol::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {} FROM ai_tasks \
             WHERE project_id = $1 \
               AND ((reference_type = 'work_item' AND reference_id = $2) \
                    OR (reference_type = 'comment' AND payload->>'issue_id' = $2::text)) \
             ORDER BY created_at DESC, id LIMIT $3",
            task_with_protocol_columns()
        ),
        vec![scope.project_id.into(), issue_id.into(), ISSUE_TASK_LIMIT.into()],
    ))
    .all(&state.db)
    .await?;

    let task_ids: Vec<Uuid> = tasks.iter().map(|entry| entry.task.id).collect();
    let events = TaskEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT task_id, event_type, payload, created_at FROM ( \
             SELECT task_id, event_type, payload, created_at, id, \
                    ROW_NUMBER() OVER (PARTITION BY task_id ORDER BY created_at, id) AS position \
             FROM ai_task_events WHERE task_id = ANY($1) \
         ) ranked WHERE position <= $2 ORDER BY created_at, id",
        vec![task_ids.into(), TASK_EVENT_LIMIT.into()],
    ))
    .all(&state.db)
    .await?;

    let items = tasks
        .into_iter()
        .map(|entry| {
            let task_events: Vec<Value> = events
                .iter()
                .filter(|event| event.task_id == entry.task.id)
                .map(|event| {
                    json!({
                        "event_type": event.event_type,
                        "payload": cut_long_strings(event.payload.clone()),
                        "created_at": event.created_at.to_rfc3339(),
                    })
                })
                .collect();
            let mut item = json!(AiTaskResponse::from(entry));
            if let Some(object) = item.as_object_mut() {
                object.insert("events".to_string(), Value::Array(task_events));
            }
            item
        })
        .collect::<Vec<_>>();
    Ok(ApiResponse::success(items))
}

pub async fn create_project_ai_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateAiTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user_id(&claims)?;
    let is_project_admin = is_project_admin_or_owner(&state.db, project_id, user_id).await?;
    let is_sys_admin = is_system_admin(&state.db, user_id).await?;
    if !is_project_admin && !is_sys_admin {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    let task_type = req.task_type.trim().to_ascii_lowercase();
    if !valid_task_type(&task_type) {
        return Err(ApiError::BadRequest("invalid task_type".to_string()));
    }

    if let Some(reference_type) = req.reference_type.as_deref()
        && !valid_reference_type(reference_type)
    {
        return Err(ApiError::BadRequest("invalid reference_type".to_string()));
    }

    ensure_reference_in_project(&state.db, project_id, req.reference_type.as_deref(), req.reference_id).await?;
    ensure_project_ai_participant(&state, project_id, req.ai_participant_id).await?;

    let task = create_ai_task(
        &state.db,
        CreateAiTaskInput {
            origin: DispatchOrigin::Request,
            project_id,
            ai_participant_id: req.ai_participant_id,
            actor_id: Some(user_id),
            task_type,
            reference_type: req.reference_type,
            reference_id: req.reference_id,
            priority: req.priority.unwrap_or(0),
            payload: req.payload.unwrap_or_else(|| json!({})),
            idempotency_key: req.idempotency_key,
            max_attempts: req.max_attempts.unwrap_or(3),
        },
    )
    .await?;

    let task = task.ok_or_else(|| ApiError::Conflict("idempotency key already exists".to_string()))?;
    Ok(ApiResponse::success(response_for(&state.db, task).await?))
}

async fn ensure_project_ai_participant(
    state: &AppState,
    project_id: Uuid,
    ai_participant_id: Uuid,
) -> Result<(), ApiError> {
    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT 1
                FROM ai_participants ap
                INNER JOIN users u ON u.id::text = ap.id
                WHERE ap.project_id = $1
                  AND ap.is_active = true
                  AND u.id = $2
                  AND u.entity_type = 'bot'
                LIMIT 1
            ",
            vec![project_id.into(), ai_participant_id.into()],
        ))
        .await?
        .is_some();

    if !exists {
        return Err(ApiError::BadRequest(
            "ai participant is not active in this project".to_string(),
        ));
    }

    Ok(())
}

/// Refuses a task reference that does not name an entity of this project.
///
/// The reference is handed to the project's AI participant, which acts on it with its own
/// credentials, and recorded as the causation of the task's events, so it must belong to the
/// project the task is created in: a work item of the project, a comment on one, or a proposal
/// of the project's workspace (proposal ids are `PROP-<uuid>` or the bare uuid). A reference to
/// anything else — another project's or workspace's entity, or one that does not exist — is
/// refused the same way, so the response does not reveal which. A `reference_id` needs a
/// `reference_type`; a type without an id names nothing and is accepted as before.
async fn ensure_reference_in_project(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    reference_type: Option<&str>,
    reference_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let Some(reference_id) = reference_id else {
        return Ok(());
    };
    let sql = match reference_type {
        Some("work_item") => "SELECT 1 FROM work_items WHERE id = $2 AND project_id = $1",
        Some("comment") => {
            "SELECT 1 FROM comments c INNER JOIN work_items w ON w.id = c.work_item_id \
             WHERE c.id = $2 AND w.project_id = $1"
        }
        Some("proposal") => {
            "SELECT 1 FROM proposals p INNER JOIN projects pr ON pr.workspace_id = p.workspace_id \
             WHERE pr.id = $1 AND p.id IN ($2::text, 'PROP-' || $2::text)"
        }
        Some(_) => return Err(ApiError::BadRequest("invalid reference_type".to_string())),
        None => {
            return Err(ApiError::BadRequest("reference_id requires reference_type".to_string()));
        }
    };
    let found = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![project_id.into(), reference_id.into()],
        ))
        .await?
        .is_some();
    if found {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "reference_id does not name an entity of this project".to_string(),
        ))
    }
}

async fn ensure_actor_can_operate(state: &AppState, task: &AiTaskRow, actor_id: Uuid) -> Result<(), ApiError> {
    if task.ai_participant_id == actor_id {
        return Ok(());
    }
    if is_system_admin(&state.db, actor_id).await? {
        return Ok(());
    }
    Err(ApiError::Forbidden(
        "task can only be updated by assigned ai participant".to_string(),
    ))
}

async fn find_task(state: &AppState, task_id: Uuid) -> Result<AiTaskRow, ApiError> {
    AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                result,
                error_message,
                idempotency_key,
                attempts,
                max_attempts,
                next_retry_at,
                started_at,
                completed_at,
                created_at,
                updated_at
            FROM ai_tasks
            WHERE id = $1
        ",
        vec![task_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai task not found".to_string()))
}

async fn find_project(state: &AppState, project_id: Uuid) -> Result<ProjectRow, ApiError> {
    ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))
}

fn parse_user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

/// `ensure_reference_in_project` against a migrated scratch database with two tenants.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod reference_scope_database_tests {
    use super::ensure_reference_in_project;
    use crate::error::ApiError;
    use crate::routes::context::tenant_fixture::{exec, seed_proposal, seed_tenant, seed_work_item};
    use crate::scratch_or_skip;
    use uuid::Uuid;

    fn refused(result: &Result<(), ApiError>) -> bool {
        matches!(result, Err(ApiError::BadRequest(_)))
    }

    #[tokio::test]
    async fn a_task_reference_must_name_an_entity_of_the_project() {
        let scratch = scratch_or_skip!("ai_task_reference_scope");
        let db = &scratch.db;
        let own = seed_tenant(db, "ra").await;
        let other = seed_tenant(db, "rb").await;
        let own_item = seed_work_item(db, &own, "own").await;
        let other_item = seed_work_item(db, &other, "other").await;
        let own_comment = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, 'c')",
            vec![own_comment.into(), own_item.into(), own.member_id.into()],
        )
        .await;
        let other_comment = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, 'c')",
            vec![other_comment.into(), other_item.into(), other.member_id.into()],
        )
        .await;
        let proposal_uuid =
            |id: &str| Uuid::parse_str(id.trim_start_matches("PROP-")).expect("proposal ids carry a uuid");
        let own_proposal = proposal_uuid(&seed_proposal(db, Some(own.workspace_id), own.member_id).await);
        let other_proposal = proposal_uuid(&seed_proposal(db, Some(other.workspace_id), other.member_id).await);
        let unattributed = proposal_uuid(&seed_proposal(db, None, own.member_id).await);
        let project = own.project_id;

        for (kind, id) in [
            ("work_item", own_item),
            ("comment", own_comment),
            ("proposal", own_proposal),
        ] {
            ensure_reference_in_project(db, project, Some(kind), Some(id))
                .await
                .unwrap_or_else(|err| panic!("own {kind} must be accepted: {err:?}"));
        }
        for (kind, id) in [
            ("work_item", other_item),
            ("work_item", own_comment),
            ("work_item", Uuid::new_v4()),
            ("comment", other_comment),
            ("comment", own_item),
            ("proposal", other_proposal),
            ("proposal", unattributed),
            ("proposal", own_item),
        ] {
            assert!(
                refused(&ensure_reference_in_project(db, project, Some(kind), Some(id)).await),
                "{kind} {id} is not an entity of the project and must be refused"
            );
        }
        assert!(refused(
            &ensure_reference_in_project(db, project, None, Some(own_item)).await
        ));
        ensure_reference_in_project(db, project, Some("work_item"), None)
            .await
            .expect("a type without an id names nothing");
        ensure_reference_in_project(db, project, None, None)
            .await
            .expect("no reference at all");
        scratch.drop_self().await;
    }

    pub(super) fn state_for(db: sea_orm::DatabaseConnection) -> platform::app::AppState {
        use platform::config::{AppConfig, Secret};
        platform::app::AppState {
            cfg: AppConfig {
                app_name: "ai-task-reference-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("ai-task-reference-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    /// The route applies the check: a foreign work item is refused before anything else about
    /// the task is looked at, and the project's own work item passes it (and then fails only on
    /// the absent AI participant).
    #[tokio::test]
    async fn create_project_ai_task_refuses_a_reference_outside_the_project() {
        use super::{CreateAiTaskRequest, create_project_ai_task};
        use axum::extract::{Extension, Json, Path, State};
        use platform::auth::{JwtClaims, TokenType};

        let scratch = scratch_or_skip!("ai_task_reference_route");
        let db = &scratch.db;
        let own = seed_tenant(db, "rr").await;
        let other = seed_tenant(db, "rs").await;
        let own_item = seed_work_item(db, &own, "own").await;
        let other_item = seed_work_item(db, &other, "other").await;
        exec(
            db,
            "UPDATE users SET role = 'admin' WHERE id = $1",
            vec![own.member_id.into()],
        )
        .await;
        let state = state_for(db.clone());
        let claims = JwtClaims {
            sub: own.member_id.to_string(),
            email: format!("{}@tenant.test", own.member_id),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
            sv: 0,
        };
        let request = |reference_id| CreateAiTaskRequest {
            ai_participant_id: Uuid::new_v4(),
            task_type: "review_requested".to_string(),
            reference_type: Some("work_item".to_string()),
            reference_id: Some(reference_id),
            priority: None,
            payload: None,
            idempotency_key: None,
            max_attempts: None,
        };

        let foreign = create_project_ai_task(
            State(state.clone()),
            Extension(claims.clone()),
            Path(own.project_id),
            Json(request(other_item)),
        )
        .await;
        let Err(ApiError::BadRequest(message)) = foreign else {
            panic!("a foreign reference must be refused");
        };
        assert!(message.contains("reference_id"), "{message}");

        let own_reference = create_project_ai_task(
            State(state),
            Extension(claims),
            Path(own.project_id),
            Json(request(own_item)),
        )
        .await;
        let Err(ApiError::BadRequest(message)) = own_reference else {
            panic!("no AI participant exists, so the task cannot be created");
        };
        assert!(message.contains("ai participant"), "{message}");
        scratch.drop_self().await;
    }
}

/// Only a task's own AI participant (or a system administrator) may complete or fail it, whatever
/// kind of bot or person asks.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod participant_authority_database_tests {
    use super::{ensure_actor_can_operate, find_task};
    use crate::error::ApiError;
    use crate::routes::context::tenant_fixture::{exec, seed_bot_user, seed_tenant};
    use crate::scratch_or_skip;
    use uuid::Uuid;

    #[tokio::test]
    async fn only_the_participant_bot_may_operate_its_task() {
        let scratch = scratch_or_skip!("ai_task_participant_authority");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "pa").await;
        let participant = seed_bot_user(db, &tenant).await;
        let other_bot = seed_bot_user(db, &tenant).await;
        let task_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status) \
             VALUES ($1, $2, $3, 'issue_assigned', 'processing')",
            vec![task_id.into(), tenant.project_id.into(), participant.into()],
        )
        .await;
        let state = super::reference_scope_database_tests::state_for(db.clone());
        let task = find_task(&state, task_id).await.expect("task exists");

        ensure_actor_can_operate(&state, &task, participant)
            .await
            .expect("the participant operates its own task");
        for outsider in [other_bot, tenant.member_id] {
            assert!(
                matches!(
                    ensure_actor_can_operate(&state, &task, outsider).await,
                    Err(ApiError::Forbidden(_))
                ),
                "{outsider} is not the task's participant"
            );
        }
        scratch.drop_self().await;
    }
}

#[cfg(test)]
mod callback_attempt_tests {
    use super::check_callback_attempt;

    #[test]
    fn an_attempt_qualified_callback_needs_that_attempt_to_be_active() {
        assert!(check_callback_attempt("processing", 2, Some(2), false).is_ok());
        assert!(check_callback_attempt("processing", 2, Some(2), true).is_ok());
        assert!(check_callback_attempt("acknowledged", 2, Some(2), false).is_ok());
        assert!(check_callback_attempt("acknowledged", 2, Some(2), true).is_ok());
        let wrong = check_callback_attempt("acknowledged", 3, Some(2), true).expect_err("a later attempt runs");
        assert!(
            wrong.contains("not active") && wrong.contains("current attempt 3"),
            "{wrong}"
        );
        let late = check_callback_attempt("processing", 3, Some(2), false).expect_err("a later attempt runs");
        assert!(
            late.contains("attempt 2") && late.contains("current attempt 3"),
            "{late}"
        );
        for status in ["pending", "failed", "completed", "cancelled"] {
            for allow_pending in [false, true] {
                assert_eq!(
                    check_callback_attempt(status, 2, Some(2), allow_pending),
                    Err("task is not active".to_string()),
                    "{status} is not an active state of the attempt"
                );
            }
        }
    }

    #[test]
    fn an_unqualified_callback_keeps_the_previous_rules() {
        assert!(check_callback_attempt("processing", 1, None, false).is_ok());
        assert!(check_callback_attempt("acknowledged", 1, None, false).is_ok());
        assert!(check_callback_attempt("acknowledged", 1, None, true).is_ok());
        assert!(check_callback_attempt("pending", 1, None, false).is_err());
        assert!(check_callback_attempt("processing", 1, None, true).is_ok());
        assert!(check_callback_attempt("pending", 1, None, true).is_ok());
        assert_eq!(
            check_callback_attempt("failed", 1, None, true),
            Err("task cannot be failed in current status".to_string())
        );
        assert_eq!(
            check_callback_attempt("completed", 1, None, false),
            Err("task is not active".to_string())
        );
    }
}

/// The complete and fail callbacks against a migrated scratch database: the attempt guard, the
/// compare-and-set on the task's state, and `retry = false`.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod callback_guard_database_tests {
    use super::{CallbackAttemptQuery, FailTaskRequest, complete_task, fail_task, find_task};
    use crate::error::ApiError;
    use crate::routes::context::tenant_fixture::{Tenant, count, exec, seed_bot_user, seed_tenant};
    use crate::scratch_or_skip;
    use axum::extract::{Extension, Json, Path, Query, State};
    use platform::auth::{JwtClaims, TokenType};
    use serde_json::json;
    use uuid::Uuid;

    fn claims(user: Uuid) -> JwtClaims {
        JwtClaims {
            sub: user.to_string(),
            email: format!("{user}@bot.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
            sv: 0,
        }
    }

    async fn seed_task(
        db: &sea_orm::DatabaseConnection,
        tenant: &Tenant,
        bot: Uuid,
        status: &str,
        attempts: i32,
    ) -> Uuid {
        let task_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts, started_at) \
             VALUES ($1, $2, $3, 'issue_assigned', $4, $5, 3, now())",
            vec![
                task_id.into(),
                tenant.project_id.into(),
                bot.into(),
                status.to_string().into(),
                attempts.into(),
            ],
        )
        .await;
        task_id
    }

    fn fail_request(retry: Option<bool>) -> FailTaskRequest {
        FailTaskRequest {
            error_message: Some("no agent".to_string()),
            payload: None,
            retry,
        }
    }

    #[tokio::test]
    async fn a_late_complete_from_an_earlier_attempt_is_refused() {
        let scratch = scratch_or_skip!("ai_callback_late_complete");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "lc").await;
        let bot = seed_bot_user(db, &tenant).await;
        let task_id = seed_task(db, &tenant, bot, "processing", 2).await;
        let state = super::reference_scope_database_tests::state_for(db.clone());

        let late = complete_task(
            State(state.clone()),
            Extension(claims(bot)),
            Path(task_id),
            Query(CallbackAttemptQuery { attempt: Some(1) }),
            Json(json!({"status": "success"})),
        )
        .await;
        let Err(ApiError::BadRequest(message)) = late else {
            panic!("attempt 1 is no longer processing");
        };
        assert!(message.contains("attempt 1"), "{message}");
        assert_eq!(find_task(&state, task_id).await.expect("task").status, "processing");

        complete_task(
            State(state.clone()),
            Extension(claims(bot)),
            Path(task_id),
            Query(CallbackAttemptQuery { attempt: Some(2) }),
            Json(json!({"status": "success"})),
        )
        .await
        .unwrap_or_else(|err| panic!("the current attempt completes: {err:?}"));
        assert_eq!(find_task(&state, task_id).await.expect("task").status, "completed");

        let again = complete_task(
            State(state.clone()),
            Extension(claims(bot)),
            Path(task_id),
            Query(CallbackAttemptQuery::default()),
            Json(json!({"status": "success"})),
        )
        .await;
        assert!(
            matches!(again, Err(ApiError::BadRequest(_))),
            "a completed task stays completed"
        );
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type = 'completed'",
                vec![task_id.into()],
            )
            .await,
            1
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn an_attempt_qualified_fail_does_not_touch_a_requeued_task() {
        let scratch = scratch_or_skip!("ai_callback_late_fail");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "lf").await;
        let bot = seed_bot_user(db, &tenant).await;
        // Timed out and re-queued by the worker: pending, attempt 1 already counted.
        let task_id = seed_task(db, &tenant, bot, "pending", 1).await;
        let state = super::reference_scope_database_tests::state_for(db.clone());

        let late = fail_task(
            State(state.clone()),
            Extension(claims(bot)),
            Path(task_id),
            Query(CallbackAttemptQuery { attempt: Some(1) }),
            Json(fail_request(None)),
        )
        .await;
        assert!(matches!(late, Err(ApiError::BadRequest(_))), "attempt 1 already ended");
        let task = find_task(&state, task_id).await.expect("task");
        assert_eq!((task.status.as_str(), task.attempts), ("pending", 1));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn retry_false_fails_the_task_with_attempts_left() {
        let scratch = scratch_or_skip!("ai_callback_fail_terminal");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ft").await;
        let bot = seed_bot_user(db, &tenant).await;
        let terminal = seed_task(db, &tenant, bot, "processing", 1).await;
        let retried = seed_task(db, &tenant, bot, "processing", 1).await;
        let state = super::reference_scope_database_tests::state_for(db.clone());

        fail_task(
            State(state.clone()),
            Extension(claims(bot)),
            Path(terminal),
            Query(CallbackAttemptQuery { attempt: Some(1) }),
            Json(fail_request(Some(false))),
        )
        .await
        .unwrap_or_else(|err| panic!("the participant fails its task: {err:?}"));
        let task = find_task(&state, terminal).await.expect("task");
        assert_eq!(task.status, "failed");
        assert!(task.completed_at.is_some());

        fail_task(
            State(state.clone()),
            Extension(claims(bot)),
            Path(retried),
            Query(CallbackAttemptQuery::default()),
            Json(fail_request(None)),
        )
        .await
        .unwrap_or_else(|err| panic!("the participant fails its task: {err:?}"));
        let task = find_task(&state, retried).await.expect("task");
        assert_eq!(
            task.status, "pending",
            "the default still retries while attempts remain"
        );
        assert!(task.next_retry_at.is_some());
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_legacy_callbacks_end_an_acknowledged_attempt() {
        let scratch = scratch_or_skip!("ai_callback_acknowledged");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ak").await;
        let bot = seed_bot_user(db, &tenant).await;
        let state = super::reference_scope_database_tests::state_for(db.clone());

        // A wrong attempt is refused with the "not active" message and changes nothing.
        let task_id = seed_task(db, &tenant, bot, "acknowledged", 2).await;
        let wrong = complete_task(
            State(state.clone()),
            Extension(claims(bot)),
            Path(task_id),
            Query(CallbackAttemptQuery { attempt: Some(1) }),
            Json(json!({"status": "success"})),
        )
        .await;
        let Err(ApiError::BadRequest(message)) = wrong else {
            panic!("attempt 1 is not the current attempt");
        };
        assert!(
            message.contains("not active") && message.contains("current attempt 2"),
            "{message}"
        );
        assert_eq!(find_task(&state, task_id).await.expect("task").status, "acknowledged");

        for attempt in [Some(2), None] {
            let task_id = seed_task(db, &tenant, bot, "acknowledged", 2).await;
            complete_task(
                State(state.clone()),
                Extension(claims(bot)),
                Path(task_id),
                Query(CallbackAttemptQuery { attempt }),
                Json(json!({"status": "success"})),
            )
            .await
            .unwrap_or_else(|err| panic!("an acknowledged task completes ({attempt:?}): {err:?}"));
            assert_eq!(find_task(&state, task_id).await.expect("task").status, "completed");

            let task_id = seed_task(db, &tenant, bot, "acknowledged", 2).await;
            fail_task(
                State(state.clone()),
                Extension(claims(bot)),
                Path(task_id),
                Query(CallbackAttemptQuery { attempt }),
                Json(fail_request(Some(false))),
            )
            .await
            .unwrap_or_else(|err| panic!("an acknowledged task fails ({attempt:?}): {err:?}"));
            let task = find_task(&state, task_id).await.expect("task");
            assert_eq!(task.status, "failed");
        }
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_list_filter_accepts_acknowledged() {
        let scratch = scratch_or_skip!("ai_callback_list_acknowledged");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "la").await;
        let bot = seed_bot_user(db, &tenant).await;
        let task_id = seed_task(db, &tenant, bot, "acknowledged", 1).await;
        let state = super::reference_scope_database_tests::state_for(db.clone());

        let listed = super::list_project_ai_tasks(
            State(state),
            Extension(claims(tenant.member_id)),
            Path(tenant.project_id),
            Query(super::ListAiTasksQuery {
                status: Some("acknowledged".to_string()),
                page: None,
                per_page: None,
            }),
        )
        .await;
        assert!(listed.is_ok(), "acknowledged is a valid status filter");
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM ai_tasks WHERE id = $1 AND status = 'acknowledged'",
                vec![task_id.into()],
            )
            .await,
            1
        );
        scratch.drop_self().await;
    }

    /// The participant rule is unchanged: another bot's token cannot fail the task, with or
    /// without `retry = false`.
    #[tokio::test]
    async fn another_bot_still_cannot_fail_the_task() {
        let scratch = scratch_or_skip!("ai_callback_other_bot");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ob").await;
        let bot = seed_bot_user(db, &tenant).await;
        let other = seed_bot_user(db, &tenant).await;
        let task_id = seed_task(db, &tenant, bot, "processing", 1).await;
        let state = super::reference_scope_database_tests::state_for(db.clone());

        let refused = fail_task(
            State(state.clone()),
            Extension(claims(other)),
            Path(task_id),
            Query(CallbackAttemptQuery { attempt: Some(1) }),
            Json(fail_request(Some(false))),
        )
        .await;
        assert!(matches!(refused, Err(ApiError::Forbidden(_))));
        assert_eq!(find_task(&state, task_id).await.expect("task").status, "processing");
        scratch.drop_self().await;
    }
}

/// The issue timeline endpoint and the protocol fields of the project list, against a migrated
/// scratch database.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]
mod issue_timeline_tests {
    use super::{ListAiTasksQuery, list_issue_ai_tasks, list_project_ai_tasks};
    use crate::error::ApiError;
    use crate::routes::context::tenant_fixture::{
        bot_context_for, claims_for, exec, seed_bot_user, seed_tenant, seed_work_item, state_for,
    };
    use crate::scratch_or_skip;
    use crate::services::ai_task_fixture::seed_task_for;
    use axum::{
        Extension,
        extract::{Path, Query, State},
        response::IntoResponse,
    };
    use serde_json::{Value, json};
    use uuid::Uuid;

    async fn body(answer: Result<impl IntoResponse, ApiError>) -> Result<Value, ApiError> {
        let response = answer?.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads");
        Ok(serde_json::from_slice(&bytes).expect("the body is JSON"))
    }

    async fn timeline(
        db: &sea_orm::DatabaseConnection,
        user: Uuid,
        bot: Option<crate::middleware::bot_auth::BotAuthContext>,
        issue: Uuid,
    ) -> Result<Value, ApiError> {
        body(
            list_issue_ai_tasks(
                State(state_for(db.clone())),
                Extension(claims_for(user)),
                bot.map(Extension),
                Path(issue),
            )
            .await,
        )
        .await
    }

    #[tokio::test]
    async fn a_member_sees_the_assignment_and_mention_tasks_with_their_events() {
        let scratch = scratch_or_skip!("timeline_members");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "tl").await;
        let bot = seed_bot_user(db, &tenant).await;
        let issue = seed_work_item(db, &tenant, "Fix").await;
        let other_issue = seed_work_item(db, &tenant, "Other").await;
        let comment = Uuid::new_v4();

        let assignment = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("issue_assigned", Some("work_item"), Some(issue)),
            json!({}),
        )
        .await;
        let mention = seed_task_for(
            db,
            &tenant,
            bot,
            "completed",
            1,
            0,
            ("comment_requested", Some("comment"), Some(comment)),
            json!({"issue_id": issue.to_string()}),
        )
        .await;
        let unrelated = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("issue_assigned", Some("work_item"), Some(other_issue)),
            json!({}),
        )
        .await;
        exec(
            db,
            "UPDATE ai_tasks SET created_at = now() - interval '1 hour' WHERE id = $1",
            vec![assignment.into()],
        )
        .await;

        // 105 events on the assignment, the first with a long string, in a known order.
        exec(
            db,
            "INSERT INTO ai_task_events (id, task_id, event_type, payload, created_at) \
             SELECT gen_random_uuid(), $1, 'progress', \
                    jsonb_build_object('n', n, 'note', CASE WHEN n = 1 THEN repeat('y', 2500) ELSE 'ok' END), \
                    now() - interval '1 day' + make_interval(secs => n) \
             FROM generate_series(1, 105) AS n",
            vec![assignment.into()],
        )
        .await;
        exec(
            db,
            "UPDATE ai_tasks SET status = 'acknowledged', plan = 'the plan', acknowledged_at = now(), \
             last_progress_at = now(), runner_phase = 'pushed' WHERE id = $1",
            vec![assignment.into()],
        )
        .await;

        let answer = timeline(db, tenant.member_id, None, issue)
            .await
            .expect("a member reads");
        let items = answer["data"].as_array().expect("a list");
        assert_eq!(items.len(), 2, "the unrelated issue's task is not listed");
        assert_eq!(items[0]["id"], mention.to_string(), "newest first");
        assert_eq!(items[1]["id"], assignment.to_string());
        assert!(items.iter().all(|item| item["id"] != unrelated.to_string()));

        let events = items[1]["events"].as_array().expect("events");
        assert_eq!(events.len(), 100, "at most 100 events");
        assert_eq!(events[0]["payload"]["n"], 1, "oldest first");
        assert_eq!(events[99]["payload"]["n"], 100);
        assert_eq!(
            events[0]["payload"]["note"].as_str().expect("note").chars().count(),
            2000
        );
        assert_eq!(items[1]["plan"], "the plan");
        assert_eq!(items[1]["runner_phase"], "pushed");
        assert!(items[1]["acknowledged_at"].is_string());
        assert_eq!(items[0]["events"], json!([]));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_timeline_is_for_members_and_bots_of_the_workspace() {
        let scratch = scratch_or_skip!("timeline_access");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ta").await;
        let other = seed_tenant(db, "tb").await;
        let bot = seed_bot_user(db, &tenant).await;
        let foreign_bot = seed_bot_user(db, &other).await;
        let issue = seed_work_item(db, &tenant, "Fix").await;

        let outsider = timeline(db, other.member_id, None, issue).await;
        assert!(
            matches!(outsider, Err(ApiError::Forbidden(_))),
            "a non-member is refused"
        );
        let own = timeline(
            db,
            bot,
            Some(bot_context_for(bot, tenant.workspace_id, &["read"])),
            issue,
        )
        .await
        .expect("a bot of the workspace reads");
        assert_eq!(own["data"], json!([]));
        let foreign = timeline(
            db,
            foreign_bot,
            Some(bot_context_for(foreign_bot, other.workspace_id, &["read"])),
            issue,
        )
        .await;
        assert!(matches!(foreign, Err(ApiError::Forbidden(_))));
        let missing = timeline(db, tenant.member_id, None, Uuid::new_v4()).await;
        assert!(matches!(missing, Err(ApiError::NotFound(_))));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_project_list_carries_the_protocol_fields() {
        let scratch = scratch_or_skip!("timeline_project_list");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "pf").await;
        let bot = seed_bot_user(db, &tenant).await;
        let old = seed_task_for(db, &tenant, bot, "completed", 1, 0, ("manual", None, None), Value::Null).await;
        exec(
            db,
            "UPDATE ai_tasks SET lease_expires_at = NULL WHERE id = $1",
            vec![old.into()],
        )
        .await;
        let fresh = seed_task_for(
            db,
            &tenant,
            bot,
            "acknowledged",
            1,
            0,
            ("manual", None, None),
            Value::Null,
        )
        .await;
        exec(
            db,
            "UPDATE ai_tasks SET plan = 'p', runner_phase = 'queued', acknowledged_at = now(), \
             last_progress_at = now() WHERE id = $1",
            vec![fresh.into()],
        )
        .await;

        let answer = body(
            list_project_ai_tasks(
                State(state_for(db.clone())),
                Extension(claims_for(tenant.member_id)),
                axum::extract::Path(tenant.project_id),
                Query(ListAiTasksQuery {
                    status: None,
                    page: None,
                    per_page: None,
                }),
            )
            .await,
        )
        .await
        .expect("the list loads");
        let items = answer["data"]["items"].as_array().expect("items");
        let find = |id: Uuid| items.iter().find(|item| item["id"] == id.to_string()).expect("listed");
        let old_item = find(old);
        for field in [
            "acknowledged_at",
            "plan",
            "last_progress_at",
            "lease_expires_at",
            "runner_phase",
        ] {
            assert!(
                old_item.get(field).is_some_and(Value::is_null),
                "{field} is null for an old row"
            );
        }
        let fresh_item = find(fresh);
        assert_eq!(fresh_item["plan"], "p");
        assert_eq!(fresh_item["runner_phase"], "queued");
        assert!(fresh_item["acknowledged_at"].is_string());
        assert!(fresh_item["lease_expires_at"].is_string());
        scratch.drop_self().await;
    }
}
