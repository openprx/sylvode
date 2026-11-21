use platform::{app::AppState, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use time::Duration;
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

/// Version of the task protocol the API, the MCP server and the runner agree on.
pub const TASK_PROTOCOL: u32 = 1;

/// The statuses of a task that has been picked up and has not ended: delivered (`processing`) or
/// acknowledged by its agent. Every compare-and-set that ends an attempt matches this set.
pub const ACTIVE_TASK_STATUSES_SQL: &str = "('processing','acknowledged')";

/// Whether `status` is in [`ACTIVE_TASK_STATUSES_SQL`].
pub fn is_active_status(status: &str) -> bool {
    matches!(status, "processing" | "acknowledged")
}

#[derive(Debug, Clone, FromQueryResult)]
pub struct AiTaskRow {
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
    pub idempotency_key: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub next_retry_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// The protocol columns of a task (`0110`), kept apart from [`AiTaskRow`] so that row and the many
/// explicit column lists that build it stay as they are.
#[derive(Debug, Clone, FromQueryResult)]
pub struct AiTaskProtocolRow {
    pub id: Uuid,
    pub acknowledged_at: Option<Timestamp>,
    pub plan: Option<String>,
    pub last_progress_at: Option<Timestamp>,
    pub lease_expires_at: Option<Timestamp>,
    pub runner_phase: Option<String>,
}

/// A task together with its protocol columns.
#[derive(Debug, Clone, FromQueryResult)]
pub struct AiTaskWithProtocol {
    #[sea_orm(nested)]
    pub task: AiTaskRow,
    #[sea_orm(nested)]
    pub protocol: AiTaskProtocolRow,
}

const TASK_WITH_PROTOCOL_COLUMNS: &str = "id, project_id, ai_participant_id, task_type, reference_type, \
    reference_id, status, priority, payload, result, error_message, idempotency_key, attempts, max_attempts, \
    next_retry_at, started_at, completed_at, created_at, updated_at, \
    acknowledged_at, plan, last_progress_at, lease_expires_at, runner_phase";

/// Loads one task with its protocol columns, or answers 404.
pub async fn load_task_with_protocol<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
) -> Result<AiTaskWithProtocol, ApiError> {
    AiTaskWithProtocol::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {TASK_WITH_PROTOCOL_COLUMNS} FROM ai_tasks WHERE id = $1"),
        vec![task_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai task not found".to_string()))
}

/// The protocol columns of one task, or 404.
pub async fn load_protocol_row<C: ConnectionTrait>(db: &C, task_id: Uuid) -> Result<AiTaskProtocolRow, ApiError> {
    AiTaskProtocolRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, acknowledged_at, plan, last_progress_at, lease_expires_at, runner_phase \
         FROM ai_tasks WHERE id = $1",
        vec![task_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai task not found".to_string()))
}

/// The columns of [`AiTaskWithProtocol`], for a query over `ai_tasks`.
pub const fn task_with_protocol_columns() -> &'static str {
    TASK_WITH_PROTOCOL_COLUMNS
}

/// The active tasks of `bot_id`, oldest `started_at` first, at most `limit`.
pub async fn list_active_tasks_of_bot<C: ConnectionTrait>(
    db: &C,
    bot_id: Uuid,
    limit: i64,
) -> Result<Vec<AiTaskWithProtocol>, ApiError> {
    Ok(AiTaskWithProtocol::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {TASK_WITH_PROTOCOL_COLUMNS} FROM ai_tasks \
             WHERE ai_participant_id = $1 AND status IN {ACTIVE_TASK_STATUSES_SQL} \
             ORDER BY started_at ASC NULLS LAST, id LIMIT $2"
        ),
        vec![bot_id.into(), limit.into()],
    ))
    .all(db)
    .await?)
}

/// What an agent is told it may do with a task, derived from the task type.
pub fn trigger_kind_for_task(task_type: &str) -> &'static str {
    match task_type {
        "issue_assigned" => "assigned",
        "vote_requested" => "proposal_vote",
        "review_requested" | "comment_requested" => "mention",
        _ => "manual",
    }
}

pub fn not_active_for_attempt(attempt: i32, current: i32) -> ApiError {
    ApiError::BadRequest(format!(
        "task is not active for attempt {attempt} (current attempt {current})"
    ))
}

/// Progress events of one kind closer together than this are folded into one.
const PROGRESS_COALESCE_SECS: i64 = 30;

/// What a progress report is, and so what it does to the `runner_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressKind {
    /// The agent reports on its own work; the phase is left as it is.
    Agent,
    RunnerQueued,
    RunnerPushed,
    RunnerAlive,
    RunnerReminded,
    RunnerBlocked,
}

impl ProgressKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "agent" => Some(Self::Agent),
            "runner.queued" => Some(Self::RunnerQueued),
            "runner.pushed" => Some(Self::RunnerPushed),
            "runner.alive" => Some(Self::RunnerAlive),
            "runner.reminded" => Some(Self::RunnerReminded),
            "runner.blocked" => Some(Self::RunnerBlocked),
            _ => None,
        }
    }

    /// The `ai_task_events.event_type` of a report of this kind.
    pub const fn event_type(self) -> &'static str {
        match self {
            Self::Agent => "progress",
            Self::RunnerQueued => "runner.queued",
            Self::RunnerPushed => "runner.pushed",
            Self::RunnerAlive => "runner.alive",
            Self::RunnerReminded => "runner.reminded",
            Self::RunnerBlocked => "runner.blocked",
        }
    }

    /// The phase this kind sets; `alive`, `reminded` and the agent's own reports set none.
    pub const fn runner_phase(self) -> Option<&'static str> {
        match self {
            Self::RunnerQueued => Some("queued"),
            Self::RunnerPushed => Some("pushed"),
            Self::RunnerBlocked => Some("blocked"),
            Self::Agent | Self::RunnerAlive | Self::RunnerReminded => None,
        }
    }
}

/// How far a lease may move: `now + extension_secs`, never past `started_at + max_secs`.
#[derive(Debug, Clone, Copy)]
pub struct LeaseLimits {
    pub extension_secs: i64,
    pub max_secs: i64,
}

impl LeaseLimits {
    pub fn from_config(config: &platform::config::AiTasksConfig) -> Self {
        Self {
            extension_secs: i64::try_from(config.lease_extension_secs).unwrap_or(i64::MAX),
            max_secs: i64::try_from(config.max_processing_secs).unwrap_or(i64::MAX),
        }
    }
}

#[derive(FromQueryResult)]
struct LeaseRow {
    lease_expires_at: Timestamp,
}

/// The outcome of [`report_progress`].
#[derive(Debug, Clone, Copy)]
pub struct ProgressOutcome {
    pub lease_expires_at: Timestamp,
    /// The lease was extended but no event was written: the same kind was reported within the
    /// last [`PROGRESS_COALESCE_SECS`] seconds.
    pub coalesced: bool,
}

/// Extends the lease of attempt `attempt` of an active task and records the report as an event.
///
/// The update is a compare-and-set on the active statuses and the attempt, and it never writes
/// `status`, so a heartbeat produces no outbox row. Reports of one kind less than 30 seconds
/// apart extend the lease but write a single event.
pub async fn report_progress(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    attempt: i32,
    kind: ProgressKind,
    event_payload: Value,
    limits: LeaseLimits,
) -> Result<ProgressOutcome, ApiError> {
    use sea_orm::TransactionTrait;
    let txn = db.begin().await?;
    let sql = format!(
        r"
            UPDATE ai_tasks
            SET lease_expires_at = LEAST(
                    now() + make_interval(secs => $3::bigint),
                    started_at + make_interval(secs => $4::bigint)
                ),
                last_progress_at = now(),
                runner_phase = COALESCE($5, runner_phase),
                updated_at = now()
            WHERE id = $1 AND status IN {ACTIVE_TASK_STATUSES_SQL} AND attempts = $2
            RETURNING lease_expires_at
        "
    );
    let lease = LeaseRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        vec![
            task_id.into(),
            attempt.into(),
            limits.extension_secs.into(),
            limits.max_secs.into(),
            kind.runner_phase().map(str::to_string).into(),
        ],
    ))
    .one(&txn)
    .await?;
    let Some(LeaseRow { lease_expires_at }) = lease else {
        let task = load_task_with_protocol(&txn, task_id).await?.task;
        return Err(not_active_for_attempt(attempt, task.attempts));
    };

    let recent = txn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM ai_task_events WHERE task_id = $1 AND event_type = $2 AND created_at > $3 LIMIT 1",
            vec![
                task_id.into(),
                kind.event_type().to_string().into(),
                (Timestamp::now() + Duration::seconds(-PROGRESS_COALESCE_SECS)).into(),
            ],
        ))
        .await?
        .is_some();
    if !recent {
        insert_ai_task_event(&txn, task_id, kind.event_type(), event_payload).await?;
    }
    txn.commit().await?;
    Ok(ProgressOutcome {
        lease_expires_at,
        coalesced: recent,
    })
}

/// The outcome of [`acknowledge_task`].
#[derive(Debug)]
pub enum AckOutcome {
    /// `processing` became `acknowledged` just now.
    Acknowledged,
    /// The attempt was already acknowledged; nothing was written and the stored plan stands.
    AlreadyAcknowledged,
}

/// `processing` to `acknowledged` for attempt `attempt`, storing the plan and extending the lease.
///
/// Repeating the call for an acknowledged attempt changes nothing. Any other state or attempt is a
/// 400 naming both attempt numbers. The status change is the one that reaches the outbox.
pub async fn acknowledge_task(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    attempt: i32,
    plan: &str,
    actor_id: Uuid,
    limits: LeaseLimits,
) -> Result<AckOutcome, ApiError> {
    use sea_orm::TransactionTrait;
    let txn = db.begin().await?;
    let current = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {TASK_WITH_PROTOCOL_COLUMNS} FROM ai_tasks WHERE id = $1 FOR UPDATE"),
        vec![task_id.into()],
    ))
    .one(&txn)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai task not found".to_string()))?;
    if current.attempts != attempt {
        return Err(not_active_for_attempt(attempt, current.attempts));
    }
    match current.status.as_str() {
        "acknowledged" => return Ok(AckOutcome::AlreadyAcknowledged),
        "processing" => {}
        _ => return Err(not_active_for_attempt(attempt, current.attempts)),
    }

    // `$6` is the status the compare-and-set expects; `status` is written only here and by the
    // endings, so no heartbeat ever produces an outbox row.
    let acknowledged = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                UPDATE ai_tasks
                SET status = 'acknowledged',
                    acknowledged_at = now(),
                    plan = $3,
                    last_progress_at = now(),
                    lease_expires_at = LEAST(
                        now() + make_interval(secs => $4::bigint),
                        started_at + make_interval(secs => $5::bigint)
                    ),
                    updated_at = now()
                WHERE id = $1 AND attempts = $2 AND status = $6
                RETURNING {TASK_WITH_PROTOCOL_COLUMNS}
            "
        ),
        vec![
            task_id.into(),
            attempt.into(),
            plan.to_string().into(),
            limits.extension_secs.into(),
            limits.max_secs.into(),
            "processing".into(),
        ],
    ))
    .one(&txn)
    .await?
    .ok_or_else(|| not_active_for_attempt(attempt, current.attempts))?;

    let payload = json!({ "plan": plan, "attempt": attempt });
    insert_ai_task_event(&txn, task_id, "acknowledged", payload.clone()).await?;
    insert_ai_task_business_event(
        &txn,
        &acknowledged,
        "ai_task.acknowledged",
        Some(actor_id),
        json!({ "type": "ai_task_protocol", "actor_id": actor_id }),
        payload,
        Some("processing"),
    )
    .await?;
    txn.commit().await?;
    Ok(AckOutcome::Acknowledged)
}

/// Who a new AI task is created for. Every caller of [`create_ai_task`] states it, so a caller
/// added later cannot forget the question "may a connected application cause this?".
///
/// A delegated request carries its identity in a task local that a spawned task does not inherit:
/// code that creates a task from a spawned task on behalf of a request must say so with
/// [`Self::Delegated`], not rely on [`Self::Request`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchOrigin {
    /// Created by the code of the request running on this task. A delegated request (a connected
    /// application's token) is refused.
    Request,
    /// Created on behalf of a delegated request from a task the request spawned: refused.
    Delegated,
    /// Created by the server itself (the governance watcher, the worker): no caller to refuse.
    System,
    /// Created by an alert rule for an authenticated inbound event. Guarded like [`Self::Request`]:
    /// the inbound route never runs inside a delegated request, so the guard only refuses if code
    /// is wired wrongly. The ids are recorded in the `ai_task.created` business event.
    InboundRule {
        rule_id: Uuid,
        inbound_event_id: Uuid,
        source_id: Uuid,
    },
}

pub struct CreateAiTaskInput {
    pub origin: DispatchOrigin,
    pub project_id: Uuid,
    pub ai_participant_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub task_type: String,
    pub reference_type: Option<String>,
    pub reference_id: Option<Uuid>,
    pub priority: i32,
    pub payload: Value,
    pub idempotency_key: Option<String>,
    pub max_attempts: i32,
}

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspaceRow {
    workspace_id: Uuid,
}

pub async fn create_ai_task<C: ConnectionTrait>(
    db: &C,
    input: CreateAiTaskInput,
) -> Result<Option<AiTaskRow>, ApiError> {
    match input.origin {
        DispatchOrigin::Request | DispatchOrigin::InboundRule { .. } => crate::oauth::delegated::refuse_agent_task()?,
        DispatchOrigin::Delegated => return Err(crate::oauth::delegated::agent_task_refused()),
        DispatchOrigin::System => {}
    }
    let now = Timestamp::now();
    let max_attempts = input.max_attempts.max(1);

    let created = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO ai_tasks (
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                idempotency_key,
                max_attempts,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8, $9, $10, $11, $12)
            ON CONFLICT (idempotency_key) DO NOTHING
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
            Uuid::new_v4().into(),
            input.project_id.into(),
            input.ai_participant_id.into(),
            input.task_type.into(),
            input.reference_type.clone().into(),
            input.reference_id.into(),
            input.priority.into(),
            input.payload.clone().into(),
            input.idempotency_key.clone().into(),
            max_attempts.into(),
            now.into(),
            now.into(),
        ],
    ))
    .one(db)
    .await?;

    let Some(task) = created else {
        return Ok(None);
    };

    insert_ai_task_event(
        db,
        task.id,
        "created",
        json!({
            "task_type": task.task_type,
            "reference_type": task.reference_type,
            "reference_id": task.reference_id.map(|v| v.to_string()),
            "idempotency_key": task.idempotency_key,
        }),
    )
    .await?;
    insert_ai_task_business_event(
        db,
        &task,
        "ai_task.created",
        input.actor_id,
        created_event_source(input.origin, input.actor_id),
        json!({
            "task_type": task.task_type,
            "reference_type": task.reference_type,
            "reference_id": task.reference_id.map(|v| v.to_string()),
            "idempotency_key": task.idempotency_key,
        }),
        None,
    )
    .await?;

    Ok(Some(task))
}

/// The `source` of the `ai_task.created` business event: the service itself, or the rule and the
/// inbound event that caused the task.
fn created_event_source(origin: DispatchOrigin, actor_id: Option<Uuid>) -> Value {
    match origin {
        DispatchOrigin::InboundRule {
            rule_id,
            inbound_event_id,
            source_id,
        } => json!({
            "type": "inbound_rule",
            "rule_id": rule_id,
            "inbound_event_id": inbound_event_id,
            "source_id": source_id,
            "actor_id": actor_id,
        }),
        DispatchOrigin::Request | DispatchOrigin::Delegated | DispatchOrigin::System => {
            json!({ "type": "ai_task_service", "actor_id": actor_id })
        }
    }
}

pub async fn insert_ai_task_event<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
    event_type: &str,
    payload: Value,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO ai_task_events (id, task_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, $5)
        ",
        vec![
            Uuid::new_v4().into(),
            task_id.into(),
            event_type.to_string().into(),
            payload.into(),
            Timestamp::now().into(),
        ],
    ))
    .await?;
    Ok(())
}

pub async fn insert_ai_task_business_event<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    event_type: &str,
    actor_id: Option<Uuid>,
    source: Value,
    event_payload: Value,
    previous_status: Option<&str>,
) -> Result<(), ApiError> {
    let project = ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![task.project_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: project.workspace_id,
            project_id: Some(task.project_id),
            event_type: event_type.to_string(),
            aggregate_type: "ai_task".to_string(),
            aggregate_id: task.id.to_string(),
            actor_id,
            source,
            payload: json!({
                "task_id": task.id,
                "project_id": task.project_id,
                "ai_participant_id": task.ai_participant_id,
                "task_type": task.task_type,
                "reference_type": task.reference_type,
                "reference_id": task.reference_id,
                "status": task.status,
                "previous_status": previous_status,
                "priority": task.priority,
                "payload": task.payload,
                "result": task.result,
                "error_message": task.error_message,
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "next_retry_at": task.next_retry_at.map(platform::timestamp::Timestamp::to_rfc3339),
                "event_payload": event_payload
            }),
            metadata: json!({
                "ai_task_id": task.id,
                "ai_participant_id": task.ai_participant_id,
                "task_type": task.task_type,
                "reference_type": task.reference_type,
                "reference_id": task.reference_id,
                "status": task.status,
                "previous_status": previous_status
            }),
            correlation_id: Some(task.id),
            causation_id: task.reference_id,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

pub fn next_retry_time(attempts: i32) -> Timestamp {
    let safe_attempts = attempts.max(1);
    let backoff_seconds = (safe_attempts * 30).min(30 * 20);
    Timestamp::now() + Duration::seconds(i64::from(backoff_seconds))
}

/// How an attempt that did not complete ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptEnd {
    /// Back to `pending` while attempts remain, `failed` after the last one.
    RetryWhileAttemptsRemain,
    /// `failed` now, whatever attempts remain: a repeat cannot change the outcome.
    Terminal,
}

/// The status a task is left in when its attempt number `attempts` (of `max_attempts`) ends
/// without completing. `attempts` counts the attempt that just ended.
pub const fn status_after_failed_attempt(attempts: i32, max_attempts: i32, end: AttemptEnd) -> &'static str {
    match end {
        AttemptEnd::RetryWhileAttemptsRemain if attempts < max_attempts => "pending",
        AttemptEnd::RetryWhileAttemptsRemain | AttemptEnd::Terminal => "failed",
    }
}

/// Why the worker ended an attempt, recorded in the task and business events as `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerEndReason {
    /// The receiver answered the delivery that it will not run the task (`no_agent`, `ignored`,
    /// `refused`, or an HTTP 4xx that a repeat cannot change).
    ReceiverDeclined,
    /// The delivery did not reach a receiver that took it (no webhook, transport error, 5xx).
    DeliveryFailed,
    /// The task's lease ran out: no completion or progress arrived within `[ai_tasks]
    /// processing_timeout_secs` of the delivery, or within the lease a later heartbeat granted.
    ProcessingTimeout,
}

impl WorkerEndReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReceiverDeclined => "receiver_declined",
            Self::DeliveryFailed => "delivery_failed",
            Self::ProcessingTimeout => "processing_timeout",
        }
    }
}

/// Ends attempt `attempt` of a task the worker delivered, if that attempt is still active.
///
/// Active means `processing` or `acknowledged` ([`ACTIVE_TASK_STATUSES_SQL`]). The update is a
/// compare-and-set on `status IN active AND attempts = attempt`, so it never overrides a
/// completion or failure that the receiver reported first, and two workers cannot end the same
/// attempt twice: `None` means the attempt had already ended and nothing was written. Otherwise
/// the `retried` or `failed` task event and the matching business event are written, both
/// carrying `reason` and `error_message`, and the business event records the status the attempt
/// really had. The caller sends `ai.task_failed` for a failed task (see [`notify_ai_task_failed`]).
pub async fn end_processing_attempt<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
    attempt: i32,
    end: AttemptEnd,
    reason: WorkerEndReason,
    error_message: &str,
) -> Result<Option<AiTaskRow>, ApiError> {
    let Some(EndedAttemptRow { task, previous_status }) =
        cas_end_attempt(db, task_id, attempt, end, error_message).await?
    else {
        return Ok(None);
    };
    record_worker_attempt_end(db, &task, reason, &previous_status).await?;
    Ok(Some(task))
}

/// The compare-and-set that ends attempt `attempt` of an active task without completing it.
///
/// The task goes back to `pending` while attempts remain (and `end` allows), otherwise to
/// `failed`. `None` when the attempt is not active. Events are the caller's.
pub async fn cas_end_attempt<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
    attempt: i32,
    end: AttemptEnd,
    error_message: &str,
) -> Result<Option<EndedAttemptRow>, ApiError> {
    let terminal = end == AttemptEnd::Terminal;
    let sql = format!(
        r"
            WITH decided AS (
                SELECT id, status AS previous_status, ($3 OR attempts >= max_attempts) AS fails
                FROM ai_tasks
                WHERE id = $1 AND status IN {ACTIVE_TASK_STATUSES_SQL} AND attempts = $2
                FOR UPDATE
            )
            UPDATE ai_tasks t
            SET status = CASE WHEN decided.fails THEN 'failed' ELSE 'pending' END,
                error_message = $4,
                next_retry_at = CASE
                    WHEN decided.fails THEN NULL
                    ELSE now() + make_interval(secs => LEAST(GREATEST(t.attempts, 1), 20) * 30)
                END,
                completed_at = CASE WHEN decided.fails THEN now() ELSE t.completed_at END,
                updated_at = now()
            FROM decided
            WHERE t.id = decided.id AND t.status IN {ACTIVE_TASK_STATUSES_SQL} AND t.attempts = $2
            RETURNING
                t.id,
                t.project_id,
                t.ai_participant_id,
                t.task_type,
                t.reference_type,
                t.reference_id,
                t.status,
                t.priority,
                t.payload,
                t.result,
                t.error_message,
                t.idempotency_key,
                t.attempts,
                t.max_attempts,
                t.next_retry_at,
                t.started_at,
                t.completed_at,
                t.created_at,
                t.updated_at,
                decided.previous_status
        "
    );
    EndedAttemptRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        vec![
            task_id.into(),
            attempt.into(),
            terminal.into(),
            error_message.to_string().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(ApiError::from)
}

/// A task an attempt of which was just ended, with the status that attempt had.
#[derive(Debug, FromQueryResult)]
pub struct EndedAttemptRow {
    #[sea_orm(nested)]
    pub task: AiTaskRow,
    pub previous_status: String,
}

/// Ends up to `limit` active attempts whose lease has run out.
///
/// An attempt is stalled when its `lease_expires_at` has passed; pick-up always sets one. Each
/// goes back to `pending` while attempts remain and is `failed` after the last one, with an error
/// message naming the lease and the last progress.
///
/// Rows are claimed with `FOR UPDATE SKIP LOCKED` and the update re-checks `status`, so several
/// workers may sweep at once without ending an attempt twice, and a completion that commits first
/// wins. The returned rows are the tasks that were ended; their events are already written, and
/// the caller sends `ai.task_failed` for the failed ones.
pub async fn end_stalled_attempts<C: ConnectionTrait>(db: &C, limit: i64) -> Result<Vec<AiTaskRow>, ApiError> {
    let sql = format!(
        r#"
            WITH stalled AS (
                SELECT id, status AS previous_status
                FROM ai_tasks
                WHERE status IN {ACTIVE_TASK_STATUSES_SQL}
                  AND lease_expires_at <= now()
                ORDER BY lease_expires_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE ai_tasks t
            SET status = CASE WHEN t.attempts >= t.max_attempts THEN 'failed' ELSE 'pending' END,
                error_message = CASE
                    WHEN t.last_progress_at IS NULL THEN format(
                        'the task lease expired at %s and no progress was ever reported (attempt %s of %s); '
                        'the runner may have stopped, the agent may still be running, or its callback was '
                        'refused or lost',
                        to_char(t.lease_expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                        t.attempts, t.max_attempts
                    )
                    ELSE format(
                        'the task lease expired at %s with no progress since %s (attempt %s of %s)',
                        to_char(t.lease_expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                        to_char(t.last_progress_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                        t.attempts, t.max_attempts
                    )
                END,
                next_retry_at = CASE
                    WHEN t.attempts >= t.max_attempts THEN NULL
                    ELSE now() + make_interval(secs => LEAST(GREATEST(t.attempts, 1), 20) * 30)
                END,
                completed_at = CASE WHEN t.attempts >= t.max_attempts THEN now() ELSE t.completed_at END,
                updated_at = now()
            FROM stalled
            WHERE t.id = stalled.id AND t.status IN {ACTIVE_TASK_STATUSES_SQL}
            RETURNING
                t.id,
                t.project_id,
                t.ai_participant_id,
                t.task_type,
                t.reference_type,
                t.reference_id,
                t.status,
                t.priority,
                t.payload,
                t.result,
                t.error_message,
                t.idempotency_key,
                t.attempts,
                t.max_attempts,
                t.next_retry_at,
                t.started_at,
                t.completed_at,
                t.created_at,
                t.updated_at,
                stalled.previous_status
        "#
    );
    let ended = EndedAttemptRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        vec![limit.into()],
    ))
    .all(db)
    .await?;

    let mut tasks = Vec::with_capacity(ended.len());
    for EndedAttemptRow { task, previous_status } in ended {
        record_worker_attempt_end(db, &task, WorkerEndReason::ProcessingTimeout, &previous_status).await?;
        tasks.push(task);
    }
    Ok(tasks)
}

/// The `retried` or `failed` task event and business event of an attempt the worker ended.
async fn record_worker_attempt_end<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    reason: WorkerEndReason,
    previous_status: &str,
) -> Result<(), ApiError> {
    let failed = task.status == "failed";
    let event_payload = json!({
        "reason": reason.as_str(),
        "error_message": task.error_message,
        "attempts": task.attempts,
        "max_attempts": task.max_attempts,
        "next_retry_at": task.next_retry_at.map(Timestamp::to_rfc3339),
    });
    insert_ai_task_event(
        db,
        task.id,
        if failed { "failed" } else { "retried" },
        event_payload.clone(),
    )
    .await?;
    insert_ai_task_business_event(
        db,
        task,
        if failed { "ai_task.failed" } else { "ai_task.retried" },
        None,
        json!({ "type": "worker", "reason": reason.as_str() }),
        event_payload,
        Some(previous_status),
    )
    .await
}

/// The compare-and-set that completes attempt `attempt` of an active task with `result`. `None`
/// when the attempt is not active (it ended, was re-queued, or is another attempt). Events are the
/// caller's.
pub async fn complete_active_attempt<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
    attempt: i32,
    result: &Value,
) -> Result<Option<EndedAttemptRow>, ApiError> {
    let sql = format!(
        r"
            WITH decided AS (
                SELECT id, status AS previous_status
                FROM ai_tasks
                WHERE id = $1 AND status IN {ACTIVE_TASK_STATUSES_SQL} AND attempts = $2
                FOR UPDATE
            )
            UPDATE ai_tasks t
            SET status = 'completed',
                result = $3,
                error_message = NULL,
                completed_at = now(),
                updated_at = now()
            FROM decided
            WHERE t.id = decided.id AND t.status IN {ACTIVE_TASK_STATUSES_SQL} AND t.attempts = $2
            RETURNING
                t.id, t.project_id, t.ai_participant_id, t.task_type, t.reference_type, t.reference_id,
                t.status, t.priority, t.payload, t.result, t.error_message, t.idempotency_key, t.attempts,
                t.max_attempts, t.next_retry_at, t.started_at, t.completed_at, t.created_at, t.updated_at,
                decided.previous_status
        "
    );
    Ok(EndedAttemptRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        vec![task_id.into(), attempt.into(), result.clone().into()],
    ))
    .one(db)
    .await?)
}

/// Sends `ai.task_completed` for a task that has just been completed, on behalf of `actor_id`.
pub async fn notify_ai_task_completed(state: &AppState, task: &AiTaskRow, actor_id: Uuid) -> Result<(), ApiError> {
    let project = ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![task.project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;
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
    Ok(())
}

/// Sends `ai.task_failed` for a task that has just been failed.
///
/// It goes to the subscribed webhooks of the task's workspace. `actor_id` is who failed it: the calling participant, or the task's own AI
/// participant when the worker ended it.
pub async fn notify_ai_task_failed(state: &AppState, task: &AiTaskRow, actor_id: Uuid) -> Result<(), ApiError> {
    let project = ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![task.project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;
    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::AiTaskFailed,
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
                "error_message": task.error_message,
            })),
        },
    );
    Ok(())
}

pub fn valid_task_type(value: &str) -> bool {
    matches!(
        value,
        "issue_assigned" | "review_requested" | "comment_requested" | "vote_requested"
    )
}

pub fn valid_reference_type(value: &str) -> bool {
    matches!(value, "work_item" | "proposal" | "comment")
}

pub async fn queue_vote_requested_tasks_for_project<C: ConnectionTrait>(
    db: &C,
    origin: DispatchOrigin,
    project_id: Uuid,
    proposal_id: &str,
    payload: Value,
) -> Result<usize, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct BotRow {
        user_id: Uuid,
    }

    let bots = BotRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT u.id AS user_id
            FROM ai_participants ap
            INNER JOIN users u
                ON u.id::text = ap.id
            WHERE ap.project_id = $1
              AND ap.is_active = true
              AND u.entity_type = 'bot'
        ",
        vec![project_id.into()],
    ))
    .all(db)
    .await?;

    let proposal_uuid = Uuid::parse_str(proposal_id).ok();
    let mut created = 0usize;
    for bot in bots {
        let idempotency_key = Some(format!("vote_requested:{project_id}:{proposal_id}:{}", bot.user_id));
        let task = create_ai_task(
            db,
            CreateAiTaskInput {
                origin,
                project_id,
                ai_participant_id: bot.user_id,
                actor_id: None,
                task_type: "vote_requested".to_string(),
                reference_type: Some("proposal".to_string()),
                reference_id: proposal_uuid,
                priority: 5,
                payload: payload.clone(),
                idempotency_key,
                max_attempts: 3,
            },
        )
        .await?;
        if task.is_some() {
            created += 1;
        }
    }

    Ok(created)
}

#[cfg(test)]
mod attempt_end_tests {
    use super::{AttemptEnd, WorkerEndReason, status_after_failed_attempt};

    #[test]
    fn a_retryable_end_requeues_while_attempts_remain() {
        assert_eq!(
            status_after_failed_attempt(1, 3, AttemptEnd::RetryWhileAttemptsRemain),
            "pending"
        );
        assert_eq!(
            status_after_failed_attempt(2, 3, AttemptEnd::RetryWhileAttemptsRemain),
            "pending"
        );
        assert_eq!(
            status_after_failed_attempt(3, 3, AttemptEnd::RetryWhileAttemptsRemain),
            "failed"
        );
        assert_eq!(
            status_after_failed_attempt(4, 3, AttemptEnd::RetryWhileAttemptsRemain),
            "failed"
        );
    }

    #[test]
    fn a_terminal_end_fails_at_once() {
        for attempts in 1..=3 {
            assert_eq!(status_after_failed_attempt(attempts, 3, AttemptEnd::Terminal), "failed");
        }
    }

    #[test]
    fn reasons_have_stable_names() {
        assert_eq!(WorkerEndReason::ReceiverDeclined.as_str(), "receiver_declined");
        assert_eq!(WorkerEndReason::DeliveryFailed.as_str(), "delivery_failed");
        assert_eq!(WorkerEndReason::ProcessingTimeout.as_str(), "processing_timeout");
    }
}

/// The worker's attempt transitions against a migrated scratch database.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod worker_attempt_database_tests {
    use super::{AttemptEnd, WorkerEndReason, end_processing_attempt, end_stalled_attempts};
    use crate::routes::context::tenant_fixture::{Tenant, count, exec, seed_bot_user, seed_tenant};
    use crate::scratch_or_skip;
    use uuid::Uuid;

    async fn seed_task(
        db: &sea_orm::DatabaseConnection,
        tenant: &Tenant,
        bot: Uuid,
        attempts: i32,
        started_secs_ago: i64,
    ) -> Uuid {
        let task_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts, started_at, \
                                   lease_expires_at) \
             VALUES ($1, $2, $3, 'issue_assigned', 'processing', $4, 3, now() - make_interval(secs => $5::bigint), \
                     now() - make_interval(secs => $5::bigint) + interval '1 hour')",
            vec![
                task_id.into(),
                tenant.project_id.into(),
                bot.into(),
                attempts.into(),
                started_secs_ago.into(),
            ],
        )
        .await;
        task_id
    }

    async fn events(db: &sea_orm::DatabaseConnection, task_id: Uuid, event_type: &str) -> i64 {
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type = $2",
            vec![task_id.into(), event_type.to_string().into()],
        )
        .await
    }

    async fn business_events(db: &sea_orm::DatabaseConnection, task_id: Uuid, event_type: &str) -> i64 {
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM business_events \
             WHERE aggregate_type = 'ai_task' AND aggregate_id = $1 AND event_type = $2 \
               AND source->>'type' = 'worker' AND payload->'event_payload'->>'reason' IS NOT NULL",
            vec![task_id.to_string().into(), event_type.to_string().into()],
        )
        .await
    }

    #[tokio::test]
    async fn a_declined_delivery_fails_the_attempt_once() {
        let scratch = scratch_or_skip!("ai_task_worker_declined");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "wd").await;
        let bot = seed_bot_user(db, &tenant).await;
        let task_id = seed_task(db, &tenant, bot, 1, 0).await;

        let ended = end_processing_attempt(
            db,
            task_id,
            1,
            AttemptEnd::Terminal,
            WorkerEndReason::ReceiverDeclined,
            "receiver answered no_agent",
        )
        .await
        .expect("the update runs")
        .expect("attempt 1 was processing");
        assert_eq!(ended.status, "failed");
        assert_eq!(ended.error_message.as_deref(), Some("receiver answered no_agent"));
        assert!(ended.completed_at.is_some());
        assert_eq!(events(db, task_id, "failed").await, 1);
        assert_eq!(business_events(db, task_id, "ai_task.failed").await, 1);

        let again = end_processing_attempt(
            db,
            task_id,
            1,
            AttemptEnd::Terminal,
            WorkerEndReason::ReceiverDeclined,
            "again",
        )
        .await
        .expect("the update runs");
        assert!(again.is_none(), "an ended attempt is not ended twice");
        assert_eq!(events(db, task_id, "failed").await, 1);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_failed_delivery_retries_and_never_ends_another_attempt() {
        let scratch = scratch_or_skip!("ai_task_worker_retry");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "wr").await;
        let bot = seed_bot_user(db, &tenant).await;
        let task_id = seed_task(db, &tenant, bot, 2, 0).await;

        let stale = end_processing_attempt(
            db,
            task_id,
            1,
            AttemptEnd::RetryWhileAttemptsRemain,
            WorkerEndReason::DeliveryFailed,
            "late",
        )
        .await
        .expect("the update runs");
        assert!(stale.is_none(), "attempt 1 is over; attempt 2 is untouched");

        let ended = end_processing_attempt(
            db,
            task_id,
            2,
            AttemptEnd::RetryWhileAttemptsRemain,
            WorkerEndReason::DeliveryFailed,
            "HTTP 503",
        )
        .await
        .expect("the update runs")
        .expect("attempt 2 was processing");
        assert_eq!(ended.status, "pending");
        assert!(ended.next_retry_at.is_some());
        assert_eq!(events(db, task_id, "retried").await, 1);
        assert_eq!(business_events(db, task_id, "ai_task.retried").await, 1);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn stalled_attempts_are_requeued_or_failed_once() {
        let scratch = scratch_or_skip!("ai_task_worker_stalled");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ws").await;
        let bot = seed_bot_user(db, &tenant).await;
        let requeued = seed_task(db, &tenant, bot, 1, 7_200).await;
        let exhausted = seed_task(db, &tenant, bot, 3, 7_200).await;
        let fresh = seed_task(db, &tenant, bot, 1, 10).await;

        let ended = end_stalled_attempts(db, 10).await.expect("the sweep runs");
        assert_eq!(ended.len(), 2);
        let status = |id: Uuid| ended.iter().find(|task| task.id == id).map(|task| task.status.clone());
        assert_eq!(status(requeued).as_deref(), Some("pending"));
        assert_eq!(status(exhausted).as_deref(), Some("failed"));
        assert_eq!(status(fresh), None, "a recent delivery is left alone");
        for task in &ended {
            let message = task.error_message.as_deref().unwrap_or_default();
            assert!(message.contains("no progress was ever reported"), "{message}");
            assert!(
                message.contains(&format!("attempt {} of 3", task.attempts)),
                "{message}"
            );
        }
        assert_eq!(events(db, requeued, "retried").await, 1);
        assert_eq!(events(db, exhausted, "failed").await, 1);
        assert_eq!(business_events(db, exhausted, "ai_task.failed").await, 1);

        let second = end_stalled_attempts(db, 10).await.expect("the sweep runs");
        assert!(second.is_empty(), "an ended attempt is not swept twice");
        scratch.drop_self().await;
    }

    async fn set_status(db: &sea_orm::DatabaseConnection, task_id: Uuid, status: &str) {
        exec(
            db,
            "UPDATE ai_tasks SET status = $2 WHERE id = $1",
            vec![task_id.into(), status.to_string().into()],
        )
        .await;
    }

    async fn previous_status_of_last_business_event(db: &sea_orm::DatabaseConnection, task_id: Uuid) -> String {
        use sea_orm::FromQueryResult;

        #[derive(Debug, FromQueryResult)]
        struct Row {
            previous_status: String,
        }
        Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT payload->>'previous_status' AS previous_status FROM business_events \
             WHERE aggregate_type = 'ai_task' AND aggregate_id = $1 ORDER BY created_at DESC LIMIT 1",
            vec![task_id.to_string().into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("a business event exists")
        .previous_status
    }

    #[tokio::test]
    async fn an_acknowledged_attempt_is_ended_with_its_real_previous_status() {
        let scratch = scratch_or_skip!("ai_task_worker_acknowledged");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "wa").await;
        let bot = seed_bot_user(db, &tenant).await;
        let task_id = seed_task(db, &tenant, bot, 1, 0).await;
        set_status(db, task_id, "acknowledged").await;

        let ended = end_processing_attempt(
            db,
            task_id,
            1,
            AttemptEnd::Terminal,
            WorkerEndReason::ReceiverDeclined,
            "declined",
        )
        .await
        .expect("the update runs")
        .expect("an acknowledged attempt is active");
        assert_eq!(ended.status, "failed");
        assert_eq!(
            previous_status_of_last_business_event(db, task_id).await,
            "acknowledged"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_sweep_honours_the_lease() {
        let scratch = scratch_or_skip!("ai_task_worker_lease");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "wl").await;
        let bot = seed_bot_user(db, &tenant).await;
        // Both started two hours ago, so the start time alone would sweep each of them.
        let leased = seed_task(db, &tenant, bot, 1, 7_200).await;
        let expired = seed_task(db, &tenant, bot, 1, 7_200).await;
        exec(
            db,
            "UPDATE ai_tasks SET lease_expires_at = now() + interval '10 minutes', status = 'acknowledged' WHERE id = $1",
            vec![leased.into()],
        )
        .await;
        exec(
            db,
            "UPDATE ai_tasks SET lease_expires_at = now() - interval '1 minute', \
             last_progress_at = now() - interval '31 minutes', status = 'acknowledged' WHERE id = $1",
            vec![expired.into()],
        )
        .await;

        let ended = end_stalled_attempts(db, 10).await.expect("the sweep runs");
        let find = |id: Uuid| ended.iter().find(|task| task.id == id);
        assert!(find(leased).is_none(), "a future lease protects an old attempt");

        let expired_row = find(expired).expect("an expired lease is swept");
        let message = expired_row.error_message.as_deref().unwrap_or_default();
        assert!(
            message.contains("the task lease expired at") && message.contains("no progress since"),
            "{message}"
        );
        assert_eq!(
            previous_status_of_last_business_event(db, expired).await,
            "acknowledged"
        );

        scratch.drop_self().await;
    }
}

/// Every compare-and-set must match the active set, not `processing` alone: a task the agent has
/// acknowledged would otherwise be impossible to end.
#[cfg(test)]
#[allow(clippy::expect_used)]
mod active_status_literal_tests {
    use std::path::{Path, PathBuf};

    /// Files that may name the `processing` status: the worker's pick-up writes it, and three test
    /// fixtures put a row into it or count rows in it.
    const ALLOWED: [&str; 4] = [
        "apps/worker/src/ai_tasks.rs",
        "apps/worker/src/oauth_feed/tests.rs",
        "apps/api/src/oauth/migration_tests.rs",
        "apps/api/src/services/runner_protocol_migration_tests.rs",
    ];

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source directory is readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn no_status_comparison_names_processing_alone() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let needles = [
            format!("status = '{}'", "processing"),
            format!("status='{}'", "processing"),
        ];
        let mut files = Vec::new();
        rust_files(&root.join("apps/api/src"), &mut files);
        rust_files(&root.join("apps/worker/src"), &mut files);
        assert!(files.len() > 50, "the source tree was not found");

        let mut offenders = Vec::new();
        for file in files {
            let relative = file
                .strip_prefix(&root)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            // This file spells the needles out in its own tests only through `format!`.
            let text = std::fs::read_to_string(&file).expect("source file is readable");
            if needles.iter().any(|needle| text.contains(needle.as_str())) && !ALLOWED.contains(&relative.as_str()) {
                offenders.push(relative);
            }
        }
        assert!(
            offenders.is_empty(),
            "use ACTIVE_TASK_STATUSES_SQL instead of a `processing`-only comparison in: {offenders:?}"
        );
    }
}
