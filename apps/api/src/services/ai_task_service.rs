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
        DispatchOrigin::Request => crate::oauth::delegated::refuse_agent_task()?,
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
        json!({ "type": "ai_task_service", "actor_id": input.actor_id }),
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
    /// No completion arrived within `[ai_tasks] processing_timeout_secs` of the delivery.
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

/// Ends attempt `attempt` of a task the worker delivered, if that attempt is still `processing`.
///
/// The update is a compare-and-set on `status = 'processing' AND attempts = attempt`, so it never
/// overrides a completion or failure that the receiver reported first, and two workers cannot end
/// the same attempt twice: `None` means the attempt had already ended and nothing was written.
/// Otherwise the `retried` or `failed` task event and the matching business event are written,
/// both carrying `reason` and `error_message`. The caller sends `ai.task_failed` for a failed task
/// (see [`notify_ai_task_failed`]).
pub async fn end_processing_attempt<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
    attempt: i32,
    end: AttemptEnd,
    reason: WorkerEndReason,
    error_message: &str,
) -> Result<Option<AiTaskRow>, ApiError> {
    let terminal = end == AttemptEnd::Terminal;
    let ended = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH decided AS (
                SELECT id, ($3 OR attempts >= max_attempts) AS fails
                FROM ai_tasks
                WHERE id = $1 AND status = 'processing' AND attempts = $2
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
            WHERE t.id = decided.id AND t.status = 'processing' AND t.attempts = $2
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
                t.updated_at
        ",
        vec![
            task_id.into(),
            attempt.into(),
            terminal.into(),
            error_message.to_string().into(),
        ],
    ))
    .one(db)
    .await?;

    let Some(task) = ended else {
        return Ok(None);
    };
    record_worker_attempt_end(db, &task, reason).await?;
    Ok(Some(task))
}

/// Ends up to `limit` attempts that have been `processing` for longer than `timeout_secs`.
///
/// Each goes back to `pending` while attempts remain and is `failed` after the last one, with an
/// error message naming the timeout and the attempt.
///
/// Rows are claimed with `FOR UPDATE SKIP LOCKED` and the update re-checks `status`, so several
/// workers may sweep at once without ending an attempt twice, and a completion that commits first
/// wins. The returned rows are the tasks that were ended; their events are already written, and
/// the caller sends `ai.task_failed` for the failed ones.
pub async fn end_stalled_attempts<C: ConnectionTrait>(
    db: &C,
    timeout_secs: u64,
    limit: i64,
) -> Result<Vec<AiTaskRow>, ApiError> {
    let timeout_secs = i64::try_from(timeout_secs).unwrap_or(i64::MAX);
    let ended = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH stalled AS (
                SELECT id
                FROM ai_tasks
                WHERE status = 'processing'
                  AND COALESCE(started_at, updated_at) <= now() - make_interval(secs => $1::bigint)
                ORDER BY COALESCE(started_at, updated_at)
                LIMIT $2
                FOR UPDATE SKIP LOCKED
            )
            UPDATE ai_tasks t
            SET status = CASE WHEN t.attempts >= t.max_attempts THEN 'failed' ELSE 'pending' END,
                error_message = format(
                    'no completion was reported within %s seconds of the delivery (attempt %s of %s); '
                    'the receiver may have stopped, the agent may still be running, or its callback was '
                    'refused or lost',
                    $1::bigint, t.attempts, t.max_attempts
                ),
                next_retry_at = CASE
                    WHEN t.attempts >= t.max_attempts THEN NULL
                    ELSE now() + make_interval(secs => LEAST(GREATEST(t.attempts, 1), 20) * 30)
                END,
                completed_at = CASE WHEN t.attempts >= t.max_attempts THEN now() ELSE t.completed_at END,
                updated_at = now()
            FROM stalled
            WHERE t.id = stalled.id AND t.status = 'processing'
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
                t.updated_at
        ",
        vec![timeout_secs.into(), limit.into()],
    ))
    .all(db)
    .await?;

    for task in &ended {
        record_worker_attempt_end(db, task, WorkerEndReason::ProcessingTimeout).await?;
    }
    Ok(ended)
}

/// The `retried` or `failed` task event and business event of an attempt the worker ended.
async fn record_worker_attempt_end<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    reason: WorkerEndReason,
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
        Some("processing"),
    )
    .await
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
            "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts, started_at) \
             VALUES ($1, $2, $3, 'issue_assigned', 'processing', $4, 3, now() - make_interval(secs => $5::bigint))",
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

        let ended = end_stalled_attempts(db, 3_600, 10).await.expect("the sweep runs");
        assert_eq!(ended.len(), 2);
        let status = |id: Uuid| ended.iter().find(|task| task.id == id).map(|task| task.status.clone());
        assert_eq!(status(requeued).as_deref(), Some("pending"));
        assert_eq!(status(exhausted).as_deref(), Some("failed"));
        assert_eq!(status(fresh), None, "a recent delivery is left alone");
        for task in &ended {
            let message = task.error_message.as_deref().unwrap_or_default();
            assert!(message.contains("3600 seconds"), "{message}");
            assert!(
                message.contains(&format!("attempt {} of 3", task.attempts)),
                "{message}"
            );
        }
        assert_eq!(events(db, requeued, "retried").await, 1);
        assert_eq!(events(db, exhausted, "failed").await, 1);
        assert_eq!(business_events(db, exhausted, "ai_task.failed").await, 1);

        let second = end_stalled_attempts(db, 3_600, 10).await.expect("the sweep runs");
        assert!(second.is_empty(), "an ended attempt is not swept twice");
        scratch.drop_self().await;
    }
}
