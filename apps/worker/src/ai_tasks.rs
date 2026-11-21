//! Worker AI tasks: pick-up, delivery to the bot's webhook, and the end of every attempt that the
//! runner does not end itself.
//!
//! An attempt starts when a pending task is picked up (`processing`, `attempts + 1`) and is
//! delivered to the bot's webhook. It ends in one of four ways:
//!
//! - The runner takes the task (`dispatched`, or any other 2xx answer it does not decline) and
//!   later reports the outcome through the API's complete or fail callback.
//! - The runner answers the delivery that it will not run the task (`no_agent`, `ignored`,
//!   `refused`, or an HTTP 4xx a repeat cannot change): the worker fails the task at once. The
//!   answer depends on the runner's configuration, which a retry seconds later does not change,
//!   and the runner's own fail callback may be refused (it may hold no token for this bot).
//! - The delivery does not reach a runner that takes it (no webhook, a transport error, HTTP
//!   5xx, 408, 409, 425, 429): the worker re-queues the task while attempts remain.
//! - Nothing ends the attempt before its lease runs out (`[ai_tasks] processing_timeout_secs` from
//!   the pick-up, extended by the runner's acknowledge and progress calls): the sweep re-queues
//!   the task while attempts remain and fails it otherwise.
//!
//! Every transition the worker makes is a compare-and-set on the active statuses (`processing`
//! and `acknowledged`) and the attempt number (`api::services::ai_task_service`), so a callback
//! that commits first wins and several workers never end the same attempt twice.

use api::outbound::validate_outbound_url;
use api::services::ai_task_reply::announce_start_refusal;
use api::services::ai_task_service::{
    AttemptEnd, TASK_PROTOCOL, WorkerEndReason, end_processing_attempt, end_stalled_attempts,
    fail_tasks_without_connection, is_reason_code, notify_ai_task_failed, runner_bot_block, trigger_kind_for_task,
};
use api::webhook_trigger::{WEBHOOK_SIGNATURE_HEADER, WEBHOOK_USER_AGENT, sign_payload};
use platform::{app::AppState, timestamp::Timestamp};
use reqwest::header::CONTENT_TYPE;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

/// At most this many characters of a runner's answer go into a task's error message.
const MAX_ANSWER_CHARS: usize = 300;

/// The `status` values with which a runner answers that it will not run a task:
/// `sylvode-runner` answers `no_agent` when no agent matches, `ignored` when the delivery is not
/// a bot task, and `refused` when the matched agent cannot run it (safe mode, a disabled feature,
/// an incomplete agent configuration).
const DECLINING_ANSWERS: [&str; 3] = ["no_agent", "ignored", "refused"];

#[derive(Debug, Clone, FromQueryResult)]
struct AiTaskDispatchRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    project_key: String,
    ai_participant_id: Uuid,
    ai_participant_name: Option<String>,
    ai_participant_agent_type: Option<String>,
    task_type: String,
    reference_type: Option<String>,
    reference_id: Option<Uuid>,
    payload: serde_json::Value,
    attempts: i32,
    max_attempts: i32,
    priority: i32,
}

#[derive(Debug, FromQueryResult)]
struct BotWebhookRow {
    webhook_id: Uuid,
    url: String,
    secret: String,
}

/// The runner's HTTP answer to a delivery.
#[derive(Debug)]
struct DeliveryAnswer {
    webhook_id: Uuid,
    status: u16,
    body: String,
}

/// What a delivery's answer means for the attempt.
#[derive(Debug, PartialEq, Eq)]
enum DeliveryVerdict {
    /// The runner took the task; it ends the attempt through a callback, or the timeout does.
    Accepted,
    /// The runner will not run the task; a repeat would get the same answer. `code` is the
    /// runner's own refusal code, when it sent one of the right shape.
    Declined { reason: String, code: Option<String> },
    /// The delivery may succeed when repeated.
    Transient(String),
}

/// Reads a runner's answer to a delivery.
///
/// A 2xx answer takes the task unless its JSON `status` is one of [`DECLINING_ANSWERS`]; an
/// answer that is not JSON, or has another status, takes it as before. A declining answer carries
/// the runner's refusal code in its own `reason_code` field, which is kept only when it has the
/// shape of a code; the code is never read out of the text. HTTP 408, 409, 425, 429 and 5xx are
/// transient, and so are redirects (which this client does not follow, so the URL needs fixing,
/// but an operator may fix it without touching the task). Any other 4xx (a signature mismatch, a
/// malformed body) declines the task.
///
/// The texts are what the task's error message and the issue's `Could not start:` comment say.
fn classify_delivery_answer(status: u16, body: &str, bot_name: &str) -> DeliveryVerdict {
    if (200..300).contains(&status) {
        let answer = serde_json::from_str::<Value>(body).ok();
        let declined = answer
            .as_ref()
            .and_then(|answer| answer.get("status"))
            .and_then(Value::as_str)
            .filter(|kind| DECLINING_ANSWERS.contains(kind));
        return match (declined, answer.as_ref()) {
            (Some("no_agent"), _) => DeliveryVerdict::Declined {
                reason: format!("the runner has no agent for bot {bot_name}"),
                code: None,
            },
            (Some(_), Some(answer)) => {
                let code = answer
                    .get("reason_code")
                    .and_then(Value::as_str)
                    .filter(|code| is_reason_code(code))
                    .map(str::to_string);
                let detail = declined_detail(answer, body);
                DeliveryVerdict::Declined {
                    reason: code.as_ref().map_or_else(
                        || format!("the runner refused the task: {detail}"),
                        |code| format!("the runner refused the task ({code}): {detail}"),
                    ),
                    code,
                }
            }
            _ => DeliveryVerdict::Accepted,
        };
    }
    let detail = excerpt(body);
    if (400..500).contains(&status) && !matches!(status, 408 | 409 | 425 | 429) {
        DeliveryVerdict::Declined {
            reason: format!("the runner answered HTTP {status}: {detail}"),
            code: None,
        }
    } else {
        DeliveryVerdict::Transient(format!("the runner could not be reached: HTTP {status}: {detail}"))
    }
}

/// What a declining answer says about why: its `reason`, else the answer itself.
fn declined_detail(answer: &Value, body: &str) -> String {
    answer
        .get("reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .map_or_else(|| excerpt(body), excerpt)
}

/// The first [`MAX_ANSWER_CHARS`] characters of `text`, marked when cut.
fn excerpt(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_ANSWER_CHARS {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(MAX_ANSWER_CHARS).collect();
    cut.push_str("...");
    cut
}

/// Picks up due tasks and delivers each to its bot's webhook.
///
/// Each picked-up task gets a lease of `processing_timeout_secs` from the pick-up.
pub async fn process_pending_tasks(
    state: &AppState,
    client: &reqwest::Client,
    concurrency: usize,
    processing_timeout_secs: u64,
) -> anyhow::Result<()> {
    let db = &state.db;
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    fail_unconnected_tasks(state, limit).await?;
    let tasks = pickup_pending_tasks(db, limit, processing_timeout_secs).await?;

    for task in tasks {
        deliver_task(state, client, &task).await?;
    }

    Ok(())
}

/// Delivers one picked-up task and settles the attempt by the runner's answer.
///
/// The connection is checked once more here: one disabled or removed since the pick-up still
/// stops the task, and the attempt ends like a refusal.
async fn deliver_task(state: &AppState, client: &reqwest::Client, task: &AiTaskDispatchRow) -> anyhow::Result<()> {
    let db = &state.db;
    if let Some(block) = runner_bot_block(db, task.project_id, task.ai_participant_id).await? {
        let verdict = DeliveryVerdict::Declined {
            reason: block.message().to_string(),
            code: Some(block.code().to_string()),
        };
        return settle_delivery(state, task, verdict).await;
    }
    let verdict = match dispatch_task(db, client, task).await {
        Ok(answer) => {
            tracing::debug!(
                task_id = %task.id,
                webhook_id = %answer.webhook_id,
                status = answer.status,
                "the runner answered the delivery"
            );
            classify_delivery_answer(
                answer.status,
                &answer.body,
                task.ai_participant_name.as_deref().unwrap_or("this task's bot"),
            )
        }
        Err(err) => DeliveryVerdict::Transient(format!("the runner could not be reached: {err}")),
    };
    settle_delivery(state, task, verdict).await
}

/// Fails the pending tasks of runner bots whose project has no enabled connection for them, and
/// tells the issue why (`Could not start: ...`), like a runner's refusal at delivery.
async fn fail_unconnected_tasks(state: &AppState, limit: i64) -> anyhow::Result<()> {
    let failed = fail_tasks_without_connection(&state.db, limit)
        .await
        .map_err(|err| anyhow::anyhow!("failing the tasks of unconnected projects failed: {err}"))?;
    for (task, block) in failed {
        tracing::warn!(task_id = %task.id, reason = block.code(), "AI task failed: no enabled connection for its project");
        notify_failed(state, &task).await;
        if let Err(err) = announce_start_refusal(state, &task, block.message()).await {
            tracing::warn!(task_id = %task.id, error = %err, "the refusal could not be reported on the issue");
        }
    }
    Ok(())
}

/// Acts on the runner's answer to the delivery of `task`: an accepted delivery is recorded as a
/// `delivered` event; a declined or failed one ends the attempt.
async fn settle_delivery(state: &AppState, task: &AiTaskDispatchRow, verdict: DeliveryVerdict) -> anyhow::Result<()> {
    match verdict {
        DeliveryVerdict::Accepted => {
            tracing::info!(
                task_id = %task.id,
                attempt = task.attempts,
                "AI task delivered; the runner ends it, or its lease does"
            );
            insert_task_event(&state.db, task.id, "delivered", json!({ "attempts": task.attempts })).await?;
        }
        DeliveryVerdict::Declined { reason, code } => {
            tracing::warn!(task_id = %task.id, attempt = task.attempts, reason = %reason, "AI task declined by the runner; failing it");
            let ended = end_attempt(
                state,
                task,
                AttemptEnd::Terminal,
                WorkerEndReason::RunnerDeclined,
                code.as_deref(),
                &reason,
            )
            .await?;
            // The assigner works on the issue, not in the task log: say there why it did not start.
            if let Some(ended) = ended
                && let Err(err) = announce_start_refusal(state, &ended, &reason).await
            {
                tracing::warn!(task_id = %task.id, error = %err, "the start refusal could not be reported on the issue");
            }
        }
        DeliveryVerdict::Transient(reason) => {
            tracing::warn!(task_id = %task.id, attempt = task.attempts, error = %reason, "dispatch failed");
            end_attempt(
                state,
                task,
                AttemptEnd::RetryWhileAttemptsRemain,
                WorkerEndReason::DeliveryFailed,
                None,
                &reason,
            )
            .await?;
        }
    }
    Ok(())
}

/// Ends the attempt just delivered, unless a callback already ended it, and sends
/// `ai.task_failed` when the task is failed. Returns the task when this call ended it.
async fn end_attempt(
    state: &AppState,
    task: &AiTaskDispatchRow,
    end: AttemptEnd,
    reason: WorkerEndReason,
    reason_code: Option<&str>,
    error_message: &str,
) -> anyhow::Result<Option<api::services::ai_task_service::AiTaskRow>> {
    let ended = end_processing_attempt(
        &state.db,
        task.id,
        task.attempts,
        end,
        reason,
        reason_code,
        error_message,
    )
    .await
    .map_err(|err| anyhow::anyhow!("ending AI task {} attempt {} failed: {err}", task.id, task.attempts))?;
    match &ended {
        Some(ended) if ended.status == "failed" => notify_failed(state, ended).await,
        Some(_) => {}
        None => tracing::info!(
            task_id = %task.id,
            attempt = task.attempts,
            "AI task attempt was already ended by a callback; left as it is"
        ),
    }
    Ok(ended)
}

/// Ends every active attempt whose lease has run out.
pub async fn end_stalled_tasks(state: &AppState, concurrency: usize) -> anyhow::Result<usize> {
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    let ended = end_stalled_attempts(&state.db, limit)
        .await
        .map_err(|err| anyhow::anyhow!("ending stalled AI tasks failed: {err}"))?;
    for task in &ended {
        tracing::warn!(
            task_id = %task.id,
            attempt = task.attempts,
            max_attempts = task.max_attempts,
            status = %task.status,
            "AI task lease ran out with no completion"
        );
        if task.status == "failed" {
            notify_failed(state, task).await;
        }
    }
    Ok(ended.len())
}

/// `ai.task_failed` for a task the worker failed, sent on behalf of the task's AI participant,
/// the way the API's fail callback sends it. A failure to send is logged; the task stays failed.
async fn notify_failed(state: &AppState, task: &api::services::ai_task_service::AiTaskRow) {
    if let Err(err) = notify_ai_task_failed(state, task, task.ai_participant_id).await {
        tracing::warn!(task_id = %task.id, error = %err, "ai.task_failed could not be sent");
    }
}

async fn pickup_pending_tasks(
    db: &sea_orm::DatabaseConnection,
    limit: i64,
    processing_timeout_secs: u64,
) -> anyhow::Result<Vec<AiTaskDispatchRow>> {
    let processing_timeout_secs = i64::try_from(processing_timeout_secs).unwrap_or(i64::MAX);
    let tasks = AiTaskDispatchRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH picked AS (
                SELECT id
                FROM ai_tasks
                WHERE status = 'pending'
                  AND (next_retry_at IS NULL OR next_retry_at <= now())
                  -- Delivery hold: after a connection or a credential rotation, the bot's tasks stay
                  -- pending until the runner has used the new token, for at most one hour. A runner
                  -- still on the old secret would answer 401, which fails the task at once.
                  AND NOT EXISTS (
                      SELECT 1
                      FROM bot_runners r
                      INNER JOIN workspace_bots b ON b.id = r.bot_id
                      WHERE r.bot_id = ai_tasks.ai_participant_id
                        AND r.credentials_issued_at > COALESCE(b.last_used_at, '-infinity'::timestamptz)
                        AND r.credentials_issued_at > now() - interval '1 hour'
                  )
                  -- A runner bot takes a project's tasks only while that project has an enabled
                  -- connection for it. The token and the runner are shared by every project of the
                  -- bot, so the connection of the task's own project decides; a task this refuses is
                  -- failed by `fail_tasks_without_connection`.
                  AND (
                      NOT EXISTS (SELECT 1 FROM bot_runners r WHERE r.bot_id = ai_tasks.ai_participant_id)
                      OR EXISTS (
                          SELECT 1
                          FROM project_bot_connections c
                          INNER JOIN project_execution_settings s ON s.project_id = c.project_id
                          WHERE c.project_id = ai_tasks.project_id
                            AND c.bot_id = ai_tasks.ai_participant_id
                            AND s.enabled
                      )
                  )
                ORDER BY priority DESC, created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE ai_tasks t
            SET status = 'processing',
                attempts = attempts + 1,
                started_at = now(),
                lease_expires_at = now() + make_interval(secs => $2::bigint),
                runner_phase = NULL,
                acknowledged_at = NULL,
                plan = NULL,
                last_progress_at = NULL,
                next_retry_at = NULL,
                updated_at = now()
            FROM picked
            WHERE t.id = picked.id
            RETURNING
                t.id,
                (
                    SELECT workspace_id
                    FROM projects p
                    WHERE p.id = t.project_id
                ) AS workspace_id,
                t.project_id,
                (
                    SELECT key
                    FROM projects p
                    WHERE p.id = t.project_id
                ) AS project_key,
                t.ai_participant_id,
                (
                    SELECT name
                    FROM users u
                    WHERE u.id = t.ai_participant_id
                ) AS ai_participant_name,
                (
                    SELECT agent_type
                    FROM users u
                    WHERE u.id = t.ai_participant_id
                ) AS ai_participant_agent_type,
                t.task_type,
                t.reference_type,
                t.reference_id,
                t.payload,
                t.attempts,
                t.max_attempts,
                t.priority
        ",
        vec![limit.into(), processing_timeout_secs.into()],
    ))
    .all(db)
    .await?;

    for task in &tasks {
        insert_task_event(
            db,
            task.id,
            "picked_up",
            json!({
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "priority": task.priority,
            }),
        )
        .await?;
        insert_picked_up_business_event(
            db,
            task,
            json!({
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "priority": task.priority,
            }),
        )
        .await?;
    }

    Ok(tasks)
}

/// The signed AI-task delivery, with the `User-Agent` every outbound webhook delivery carries.
fn dispatch_request(
    client: &reqwest::Client,
    target: reqwest::Url,
    signature: &str,
    raw_body: String,
) -> reqwest::RequestBuilder {
    client
        .post(target)
        .header(CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, WEBHOOK_USER_AGENT)
        .header(WEBHOOK_SIGNATURE_HEADER, format!("sha256={signature}"))
        .body(raw_body)
}

/// Delivers `task` and returns the runner's answer. An error means it was not delivered: no
/// active webhook, a rejected URL, a signing or transport failure.
async fn dispatch_task(
    db: &sea_orm::DatabaseConnection,
    client: &reqwest::Client,
    task: &AiTaskDispatchRow,
) -> anyhow::Result<DeliveryAnswer> {
    let webhook = BotWebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT
                w.id AS webhook_id,
                w.url,
                w.secret
            FROM webhooks w
            INNER JOIN projects p ON p.workspace_id = w.workspace_id
            WHERE p.id = $1
              AND w.bot_user_id = $2
              AND w.active = true
            ORDER BY w.updated_at DESC
            LIMIT 1
        ",
        vec![task.project_id.into(), task.ai_participant_id.into()],
    ))
    .one(db)
    .await?;

    let webhook = webhook.ok_or_else(|| {
        anyhow::anyhow!(
            "no active webhook found for bot {} in project {}",
            task.ai_participant_id,
            task.project_id
        )
    })?;

    let body = delivery_body(task);

    let target = validate_outbound_url(&webhook.url)
        .await
        .map_err(|err| anyhow::anyhow!("webhook {} url rejected: {err}", webhook.webhook_id))?;

    // Signed over the exact bytes that are sent, the same way the API's own webhook deliveries
    // are, so a runner can refuse unsigned traffic instead of having to trust this path.
    let raw_body = serde_json::to_string(&body)?;
    let signature = sign_payload(&webhook.secret, &raw_body)
        .map_err(|err| anyhow::anyhow!("webhook {} signing failed: {err}", webhook.webhook_id))?;

    let response = dispatch_request(client, target, &signature, raw_body)
        .send()
        .await
        .map_err(|err| anyhow::anyhow!("webhook {} delivery failed: {}", webhook.webhook_id, err.without_url()))?;
    let status = response.status().as_u16();
    let body = match response.text().await {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(task_id = %task.id, webhook_id = %webhook.webhook_id, error = %err.without_url(), "reading the runner's answer failed; treating it as empty");
            String::new()
        }
    };
    Ok(DeliveryAnswer {
        webhook_id: webhook.webhook_id,
        status,
        body,
    })
}

/// The JSON body of a delivery. Every field a runner already reads is unchanged; `project_key`
/// and `protocol` are additions.
fn delivery_body(task: &AiTaskDispatchRow) -> Value {
    json!({
        "task_id": task.id.to_string(),
        "project_id": task.project_id.to_string(),
        "ai_participant_id": task.ai_participant_id.to_string(),
        "ai_participant_name": task.ai_participant_name.as_deref(),
        "ai_participant_agent_type": task.ai_participant_agent_type.as_deref(),
        "task_type": task.task_type,
        "reference_type": task.reference_type,
        "reference_id": task.reference_id.map(|v| v.to_string()),
        "payload": task.payload,
        "attempts": task.attempts,
        "max_attempts": task.max_attempts,
        "trigger_kind": trigger_kind_for_task(&task.task_type),
        "project_key": task.project_key,
        "protocol": TASK_PROTOCOL,
    })
}

async fn insert_picked_up_business_event(
    db: &sea_orm::DatabaseConnection,
    task: &AiTaskDispatchRow,
    event_payload: serde_json::Value,
) -> anyhow::Result<()> {
    let payload = json!({
        "task_id": task.id,
        "project_id": task.project_id,
        "ai_participant_id": task.ai_participant_id,
        "task_type": task.task_type,
        "reference_type": task.reference_type,
        "reference_id": task.reference_id,
        "status": "processing",
        "previous_status": "pending",
        "priority": task.priority,
        "payload": task.payload,
        "attempts": task.attempts,
        "max_attempts": task.max_attempts,
        "event_payload": event_payload
    });
    let metadata = json!({
        "ai_task_id": task.id,
        "ai_participant_id": task.ai_participant_id,
        "task_type": task.task_type,
        "reference_type": task.reference_type,
        "reference_id": task.reference_id,
        "status": "processing",
        "previous_status": "pending",
        "worker_event": true
    });

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO business_events (
                id, workspace_id, project_id, event_type, aggregate_type, aggregate_id,
                actor_id, source, payload, metadata, correlation_id, causation_id,
                idempotency_key, created_at
            )
            VALUES ($1, $2, $3, 'ai_task.picked_up', 'ai_task', $4, NULL, $5, $6, $7, $8, $9, NULL, now())
        ",
        vec![
            Uuid::new_v4().into(),
            task.workspace_id.into(),
            Some(task.project_id).into(),
            task.id.to_string().into(),
            json!({ "type": "worker" }).into(),
            payload.into(),
            metadata.into(),
            Some(task.id).into(),
            task.reference_id.into(),
        ],
    ))
    .await?;

    Ok(())
}

async fn insert_task_event(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{
        AiTaskDispatchRow, DeliveryVerdict, classify_delivery_answer, delivery_body, dispatch_request, excerpt,
    };
    use serde_json::json;
    use uuid::Uuid;

    /// The delivery body is a contract with the runner: every existing field keeps its name and
    /// value, and `project_key` and `protocol` are the only additions.
    #[test]
    fn the_delivery_body_carries_the_project_key_and_protocol() {
        let id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").expect("uuid");
        let project = Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("uuid");
        let bot = Uuid::parse_str("33333333-3333-4333-8333-333333333333").expect("uuid");
        let issue = Uuid::parse_str("44444444-4444-4444-8444-444444444444").expect("uuid");
        let task = AiTaskDispatchRow {
            id,
            workspace_id: Uuid::nil(),
            project_id: project,
            project_key: "SVWL".to_string(),
            ai_participant_id: bot,
            ai_participant_name: Some("fixer".to_string()),
            ai_participant_agent_type: Some("cli".to_string()),
            task_type: "issue_assigned".to_string(),
            reference_type: Some("work_item".to_string()),
            reference_id: Some(issue),
            payload: json!({"issue_id": issue.to_string()}),
            attempts: 1,
            max_attempts: 3,
            priority: 5,
        };
        assert_eq!(
            delivery_body(&task),
            json!({
                "task_id": "11111111-1111-4111-8111-111111111111",
                "project_id": "22222222-2222-4222-8222-222222222222",
                "ai_participant_id": "33333333-3333-4333-8333-333333333333",
                "ai_participant_name": "fixer",
                "ai_participant_agent_type": "cli",
                "task_type": "issue_assigned",
                "reference_type": "work_item",
                "reference_id": "44444444-4444-4444-8444-444444444444",
                "payload": {"issue_id": "44444444-4444-4444-8444-444444444444"},
                "attempts": 1,
                "max_attempts": 3,
                "trigger_kind": "assigned",
                "project_key": "SVWL",
                "protocol": 1
            })
        );
    }

    /// The AI-task delivery carries the same `User-Agent` as every other outbound webhook
    /// delivery (ADR-0020 D3), next to its signature.
    #[test]
    fn the_ai_task_delivery_carries_the_webhook_user_agent() {
        let client = reqwest::Client::new();
        let target = reqwest::Url::parse("https://runner.example/hook").expect("valid url");
        let request = dispatch_request(&client, target, "abc", "{}".to_string())
            .build()
            .expect("the request builds");

        assert_eq!(
            request
                .headers()
                .get(reqwest::header::USER_AGENT)
                .map(reqwest::header::HeaderValue::as_bytes),
            Some(b"Sylvode-Webhook/1.0 (compatible; OpenPR-Webhook/1.0)".as_slice())
        );
        assert_eq!(
            request
                .headers()
                .get("X-Webhook-Signature")
                .map(reqwest::header::HeaderValue::as_bytes),
            Some(b"sha256=abc".as_slice())
        );
    }

    fn declined(verdict: &DeliveryVerdict) -> (&str, Option<&str>) {
        match verdict {
            DeliveryVerdict::Declined { reason, code } => (reason, code.as_deref()),
            other => panic!("expected a declined delivery, got {other:?}"),
        }
    }

    fn classify(status: u16, body: &str) -> DeliveryVerdict {
        classify_delivery_answer(status, body, "Build bot")
    }

    #[test]
    fn a_dispatched_answer_takes_the_task() {
        for body in [
            r#"{"status":"dispatched","agent":"fixer","result":"cli agent spawned"}"#,
            r#"{"status":"ok"}"#,
            "",
            "accepted",
        ] {
            assert_eq!(classify(200, body), DeliveryVerdict::Accepted, "{body}");
        }
        assert_eq!(classify(202, r#"{"status":"dispatched"}"#), DeliveryVerdict::Accepted);
    }

    #[test]
    fn a_no_agent_answer_declines_the_task_and_names_the_bot() {
        let verdict = classify(200, r#"{"status":"no_agent","bot_key":"bot-b","reason":"anything"}"#);
        assert_eq!(declined(&verdict), ("the runner has no agent for bot Build bot", None));
    }

    /// The refusal code is the runner's own field, never parsed out of the text.
    #[test]
    fn a_refusal_carries_the_code_the_runner_sent() {
        let refused = classify(
            200,
            r#"{"status":"refused","reason_code":"path_outside_allowlist","reason":"the directory is outside the allowed roots","agent":"fixer"}"#,
        );
        assert_eq!(
            declined(&refused),
            (
                "the runner refused the task (path_outside_allowlist): the directory is outside the allowed roots",
                Some("path_outside_allowlist")
            )
        );
        // A code that is not a code is dropped, and the text does not mention it.
        for bad in ["Bad Code", "ab", "tmux_unavailable (tmux 3.0 < 3.2)", ""] {
            let body = format!(r#"{{"status":"refused","reason_code":"{bad}","reason":"no tmux"}}"#);
            let verdict = classify(200, &body);
            assert_eq!(
                declined(&verdict),
                ("the runner refused the task: no tmux", None),
                "{bad}"
            );
        }
        // A code in the text only is not a code.
        let in_text = classify(
            200,
            r#"{"status":"refused","reason":"tmux_unavailable (tmux 3.0 < 3.2)"}"#,
        );
        assert_eq!(
            declined(&in_text),
            ("the runner refused the task: tmux_unavailable (tmux 3.0 < 3.2)", None)
        );
        // `ignored` reads the same way.
        let ignored = classify(200, r#"{"status":"ignored","reason":"not_bot_task"}"#);
        assert_eq!(declined(&ignored), ("the runner refused the task: not_bot_task", None));
        // Without a reason the answer itself is the detail.
        let bare = classify(200, r#"{"status":"refused"}"#);
        assert!(declined(&bare).0.starts_with("the runner refused the task: "));
    }

    #[test]
    fn a_client_error_declines_the_task() {
        for status in [400, 401, 403, 404, 413, 422] {
            let verdict = classify(status, "Unauthorized");
            assert_eq!(
                declined(&verdict),
                (
                    format!("the runner answered HTTP {status}: Unauthorized").as_str(),
                    None
                ),
                "{status}"
            );
        }
    }

    #[test]
    fn server_errors_throttling_and_conflicts_are_retried() {
        for status in [500, 502, 503, 504, 408, 409, 425, 429, 301, 302, 307] {
            assert!(
                matches!(classify(status, ""), DeliveryVerdict::Transient(ref reason)
                    if reason.starts_with("the runner could not be reached: ") && reason.contains(&format!("HTTP {status}"))),
                "{status} must be retried"
            );
        }
    }

    /// No message the worker writes calls the runner anything else.
    #[test]
    fn the_worker_never_says_receiver() {
        let source = include_str!("ai_tasks.rs");
        let mut parts = source.splitn(2, "#[cfg(test)]");
        let production = parts.next().unwrap_or_default();
        assert!(
            !production.to_lowercase().contains("receiver"),
            "a message says receiver"
        );
    }

    #[test]
    fn a_long_answer_is_cut() {
        let long = "x".repeat(1_000);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), 303);
        assert!(cut.ends_with("..."));
        let verdict = classify(400, &long);
        assert!(declined(&verdict).0.len() < 400);
    }
}

/// Pick-up and delivery settlement against a migrated scratch database.
///
/// Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::print_stderr)]
mod database_tests {
    use super::{DeliveryVerdict, fail_tasks_without_connection, pickup_pending_tasks, settle_delivery};
    use platform::app::{AppState, FlowPermissionCacheSlot};
    use platform::config::{AppConfig, Secret};
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use uuid::Uuid;

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            if let Ok(admin) = Database::connect(&admin_url).await {
                let _ = admin
                    .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                    .await;
            }
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var("OPENPR_TEST_DATABASE_URL").ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("OPENPR_TEST_DATABASE_URL is set but unusable: {err}"));
        let name = format!("sylvode_worker_{label}");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await
            .expect("scratch database reset");
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
            .await
            .expect("scratch database created");
        let (prefix, _) = admin_url.rsplit_once('/')?;
        let db = Database::connect(&format!("{prefix}/{name}"))
            .await
            .expect("scratch database connects");
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
        Some(Scratch { db, name, admin_url })
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "worker-ai-task-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("worker-ai-task-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: FlowPermissionCacheSlot::default(),
        }
    }

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    /// A project with the key `WKR` and one pending `issue_assigned` task for a bot user.
    async fn seed_pending_task(db: &DatabaseConnection) -> Uuid {
        let user = Uuid::new_v4();
        let bot = Uuid::new_v4();
        let workspace = Uuid::new_v4();
        let project = Uuid::new_v4();
        let task = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'owner', 'user', true)",
            vec![user.into(), format!("{user}@worker.test").into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
             VALUES ($1, $2, '!', 'bot', 'user', true, 'bot')",
            vec![bot.into(), format!("{bot}@worker.test").into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'worker', $3)",
            vec![workspace.into(), format!("ws-{workspace}").into(), user.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'WKR', 'worker', $3)",
            vec![project.into(), workspace.into(), user.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, max_attempts) \
             VALUES ($1, $2, $3, 'issue_assigned', 'pending', 3)",
            vec![task.into(), project.into(), bot.into()],
        )
        .await;
        task
    }

    #[derive(Debug, FromQueryResult)]
    struct Count {
        n: i64,
    }

    #[derive(Debug, FromQueryResult)]
    struct Body {
        body: String,
    }

    #[derive(Debug, FromQueryResult)]
    struct PickedTask {
        lease_secs: f64,
        phase_is_null: bool,
        attempt_fields_are_null: bool,
    }

    async fn events(db: &DatabaseConnection, task: Uuid, event_type: &str) -> i64 {
        Count::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM ai_task_events WHERE task_id = $1 AND event_type = $2",
            vec![task.into(), event_type.to_string().into()],
        ))
        .one(db)
        .await
        .expect("count query runs")
        .map_or(0, |row| row.n)
    }

    #[tokio::test]
    async fn pick_up_starts_the_lease_clears_the_phase_and_returns_the_key() {
        let scratch = scratch_or_skip!("pickup_lease");
        let db = &scratch.db;
        let task_id = seed_pending_task(db).await;
        // A re-queued task carries what its earlier attempt acknowledged and reported.
        exec(
            db,
            "UPDATE ai_tasks SET runner_phase = 'pushed', plan = 'old plan', acknowledged_at = now(), \
             last_progress_at = now() WHERE id = $1",
            vec![task_id.into()],
        )
        .await;

        let picked = pickup_pending_tasks(db, 10, 3_600).await.expect("pick-up runs");
        let [first] = picked.as_slice() else {
            panic!("exactly one task is picked up, got {}", picked.len());
        };
        assert_eq!(first.project_key, "WKR");
        assert_eq!(first.attempts, 1);

        let lease = PickedTask::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT EXTRACT(EPOCH FROM (lease_expires_at - started_at))::float8 AS lease_secs, \
                    (runner_phase IS NULL) AS phase_is_null, \
                    (plan IS NULL AND acknowledged_at IS NULL AND last_progress_at IS NULL) AS attempt_fields_are_null \
             FROM ai_tasks WHERE id = $1",
            vec![task_id.into()],
        ))
        .one(db)
        .await
        .expect("lease query runs")
        .expect("task exists");
        assert!((lease.lease_secs - 3_600.0).abs() < 1.0, "lease {}", lease.lease_secs);
        assert!(lease.phase_is_null);
        assert!(
            lease.attempt_fields_are_null,
            "a new attempt starts without the previous attempt's plan and progress"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn delivered_is_written_only_for_an_accepting_answer() {
        let scratch = scratch_or_skip!("delivered_event");
        let db = &scratch.db;
        let state = state_for(db.clone());

        let accepted = seed_pending_task(db).await;
        let declined = seed_pending_task(db).await;
        let transient = seed_pending_task(db).await;
        let picked = pickup_pending_tasks(db, 10, 3_600).await.expect("pick-up runs");
        assert_eq!(picked.len(), 3);
        let row = |id: Uuid| picked.iter().find(|task| task.id == id).expect("picked");

        settle_delivery(&state, row(accepted), DeliveryVerdict::Accepted)
            .await
            .expect("accepted settles");
        settle_delivery(
            &state,
            row(declined),
            DeliveryVerdict::Declined {
                reason: "no agent".to_string(),
                code: None,
            },
        )
        .await
        .expect("declined settles");
        settle_delivery(&state, row(transient), DeliveryVerdict::Transient("503".to_string()))
            .await
            .expect("transient settles");

        assert_eq!(events(db, accepted, "delivered").await, 1);
        assert_eq!(events(db, declined, "delivered").await, 0);
        assert_eq!(events(db, transient, "delivered").await, 0);
        scratch.drop_self().await;
    }

    /// A runner that refuses a task at delivery leaves a comment where the assigner works, for a
    /// project that has execution settings, enabled or not: a project the person has configured
    /// wants to know why a task did not start.
    #[tokio::test]
    async fn a_declined_delivery_is_reported_on_the_issue_of_a_configured_project() {
        let scratch = scratch_or_skip!("declined_comment");
        let db = &scratch.db;
        let state = state_for(db.clone());
        let enabled = seed_pending_task(db).await;
        let disabled = seed_pending_task(db).await;
        let plain = seed_pending_task(db).await;
        for (task, settings) in [(enabled, Some(true)), (disabled, Some(false)), (plain, None)] {
            let issue = Uuid::new_v4();
            exec(
                db,
                "INSERT INTO work_items (id, project_id, title, state, sequence_number, created_by) \
                 SELECT $1, t.project_id, 'Issue', 'todo', 1, p.created_by FROM ai_tasks t \
                 INNER JOIN projects p ON p.id = t.project_id WHERE t.id = $2",
                vec![issue.into(), task.into()],
            )
            .await;
            exec(
                db,
                "UPDATE ai_tasks SET task_type = 'issue_assigned', reference_type = 'work_item', reference_id = $2, \
                 payload = jsonb_build_object('issue_id', $2::text) WHERE id = $1",
                vec![task.into(), issue.into()],
            )
            .await;
            if let Some(on) = settings {
                exec(
                    db,
                    "INSERT INTO project_execution_settings (project_id, working_directory, enabled) \
                     SELECT project_id, '/work/a', $2 FROM ai_tasks WHERE id = $1",
                    vec![task.into(), on.into()],
                )
                .await;
            }
        }
        let picked = pickup_pending_tasks(db, 10, 3_600).await.expect("pick-up runs");
        for task in [enabled, disabled, plain] {
            let row = picked.iter().find(|row| row.id == task).expect("picked");
            settle_delivery(
                &state,
                row,
                DeliveryVerdict::Declined {
                    reason: "the runner refused the task (path_outside_allowlist): the working directory is outside the allowed roots"
                        .to_string(),
                    code: Some("path_outside_allowlist".to_string()),
                },
            )
            .await
            .expect("declined settles");
        }

        let comments = |task: Uuid| {
            Body::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT c.body FROM comments c INNER JOIN ai_tasks t ON t.reference_id = c.work_item_id WHERE t.id = $1",
                vec![task.into()],
            ))
            .all(db)
        };
        for task in [enabled, disabled] {
            let found = comments(task).await.expect("comments load");
            assert_eq!(found.len(), 1);
            assert_eq!(
                found.first().map(|row| row.body.as_str()),
                Some(
                    "Could not start: the runner refused the task (path_outside_allowlist): \
                     the working directory is outside the allowed roots"
                ),
                "a configured project gets the comment whether or not its connection is enabled"
            );
        }
        assert!(comments(plain).await.expect("comments load").is_empty());
        // The runner's code is stored from its own field.
        let code = Count::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM ai_task_events WHERE task_id = $1 AND event_type = 'failed' \
             AND payload->>'reason' = 'runner_declined' AND payload->>'reason_code' = 'path_outside_allowlist'",
            vec![enabled.into()],
        ))
        .one(db)
        .await
        .expect("count runs")
        .map_or(0, |row| row.n);
        assert_eq!(code, 1);
        scratch.drop_self().await;
    }

    /// Connects the bot of `task`: credentials issued `hours_ago` hours ago, and its token last used
    /// `used_after` hours ago, if ever.
    async fn connect_the_bot_of(db: &DatabaseConnection, task: Uuid, hours_ago: f64, used_after: Option<f64>) {
        exec(
            db,
            "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, transport_surface, \
                                         is_active, last_used_at) \
             SELECT t.ai_participant_id, p.workspace_id, 'runner bot', md5(t.id::text), 'opr_test', '[\"read\"]'::jsonb, \
                    'mcp_stdio', true, \
                    CASE WHEN $2::float8 IS NULL THEN NULL ELSE now() - make_interval(secs => $2::float8 * 3600) END \
             FROM ai_tasks t INNER JOIN projects p ON p.id = t.project_id WHERE t.id = $1",
            vec![task.into(), used_after.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, bot_user_id, created_by) \
             SELECT gen_random_uuid(), p.workspace_id, 'hook', 'https://runner.example.net/webhook', 's', '[]'::jsonb, true, \
                    t.ai_participant_id, p.created_by \
             FROM ai_tasks t INNER JOIN projects p ON p.id = t.project_id WHERE t.id = $1",
            vec![task.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO bot_runners (bot_id, workspace_id, webhook_id, runner_url, credentials_issued_at) \
             SELECT t.ai_participant_id, p.workspace_id, \
                    (SELECT id FROM webhooks WHERE bot_user_id = t.ai_participant_id), \
                    'https://runner.example.net', now() - make_interval(secs => $2::float8 * 3600) \
             FROM ai_tasks t INNER JOIN projects p ON p.id = t.project_id WHERE t.id = $1",
            vec![task.into(), hours_ago.into()],
        )
        .await;
        // An enabled connection of the task's project: without one the bot takes no task of it.
        for sql in [
            "INSERT INTO project_execution_settings (project_id, enabled, working_directory) \
             SELECT project_id, true, '/work/a' FROM ai_tasks WHERE id = $1",
            "INSERT INTO ai_participants (id, project_id, name, is_active, registered_by) \
             SELECT t.ai_participant_id::text, t.project_id, 'runner bot', true, p.created_by \
             FROM ai_tasks t INNER JOIN projects p ON p.id = t.project_id WHERE t.id = $1",
            "INSERT INTO project_bot_connections (project_id, bot_id) \
             SELECT project_id, ai_participant_id FROM ai_tasks WHERE id = $1",
        ] {
            exec(db, sql, vec![task.into()]).await;
        }
    }

    async fn status_of(db: &DatabaseConnection, task: Uuid) -> String {
        #[derive(Debug, FromQueryResult)]
        struct Status {
            status: String,
        }
        Status::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status FROM ai_tasks WHERE id = $1",
            vec![task.into()],
        ))
        .one(db)
        .await
        .expect("status query runs")
        .expect("task exists")
        .status
    }

    /// After a connection or a rotation the bot's tasks stay `pending` (never `processing` without
    /// a lease) until the runner has used the new token, for at most one hour; other bots are
    /// not held.
    #[tokio::test]
    async fn the_delivery_hold_keeps_a_new_connections_tasks_pending() {
        let scratch = scratch_or_skip!("delivery_hold");
        let db = &scratch.db;
        let fresh = seed_pending_task(db).await; // issued a minute ago, never used
        let used = seed_pending_task(db).await; // issued a minute ago, used since
        let old = seed_pending_task(db).await; // issued two hours ago, never used
        let other = seed_pending_task(db).await; // a bot with no connection at all
        connect_the_bot_of(db, fresh, 1.0 / 60.0, None).await;
        connect_the_bot_of(db, used, 1.0 / 60.0, Some(1.0 / 120.0)).await;
        connect_the_bot_of(db, old, 2.0, None).await;

        let picked = pickup_pending_tasks(db, 10, 3_600).await.expect("pick-up runs");
        let ids: Vec<Uuid> = picked.iter().map(|task| task.id).collect();
        assert!(
            !ids.contains(&fresh),
            "a runner that has not used its new token yet is waited for"
        );
        assert_eq!(status_of(db, fresh).await, "pending", "held tasks stay pending");
        assert_eq!(events(db, fresh, "picked_up").await, 0);
        assert!(
            ids.contains(&used),
            "tasks flow once the token was used after it was issued"
        );
        assert!(
            ids.contains(&old),
            "the hold ends after one hour whether or not the runner showed up"
        );
        assert!(ids.contains(&other), "other bots are unaffected");

        // The runner uses its token: the held task is taken on the next pick-up.
        exec(
            db,
            "UPDATE workspace_bots SET last_used_at = now() WHERE id = (SELECT ai_participant_id FROM ai_tasks WHERE id = $1)",
            vec![fresh.into()],
        )
        .await;
        let later = pickup_pending_tasks(db, 10, 3_600).await.expect("pick-up runs");
        assert_eq!(later.iter().map(|task| task.id).collect::<Vec<_>>(), vec![fresh]);
        let lease = Count::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM ai_tasks WHERE id = $1 AND status = 'processing' AND lease_expires_at IS NOT NULL",
            vec![fresh.into()],
        ))
        .one(db)
        .await
        .expect("count runs")
        .map_or(0, |row| row.n);
        assert_eq!(lease, 1, "a picked-up task always has its lease");
        scratch.drop_self().await;
    }

    /// One workspace, one runner bot (credentials issued two hours ago and used since, so the
    /// delivery hold is over) and three projects: `disabled` has a disabled connection, `enabled`
    /// an enabled one, `never` none. Each project has one pending task of the bot.
    struct GateWorld {
        bot: Uuid,
        disabled: (Uuid, Uuid),
        enabled: (Uuid, Uuid),
        never: (Uuid, Uuid),
    }

    async fn gate_world(db: &DatabaseConnection) -> GateWorld {
        let owner = Uuid::new_v4();
        let bot = Uuid::new_v4();
        let workspace = Uuid::new_v4();
        let hook = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'owner', 'user', true)",
            vec![owner.into(), format!("{owner}@gate.test").into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
             VALUES ($1, $2, '!', 'gate bot', 'user', true, 'bot')",
            vec![bot.into(), format!("{bot}@gate.test").into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'gate', $3)",
            vec![workspace.into(), format!("ws-{workspace}").into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, transport_surface, \
                                         is_active, last_used_at) \
             VALUES ($1, $2, 'gate bot', md5($1::text), 'opr_gate', '[\"read\"]'::jsonb, 'mcp_stdio', true, now())",
            vec![bot.into(), workspace.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, bot_user_id, created_by) \
             VALUES ($1, $2, 'hook', 'https://runner.example.net/webhook', 's', '[]'::jsonb, true, $3, $4)",
            vec![hook.into(), workspace.into(), bot.into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO bot_runners (bot_id, workspace_id, webhook_id, runner_url, credentials_issued_at) \
             VALUES ($1, $2, $3, 'https://runner.example.net', now() - interval '2 hours')",
            vec![bot.into(), workspace.into(), hook.into()],
        )
        .await;
        let project = |key: &'static str, connection: Option<bool>| {
            let id = Uuid::new_v4();
            let task = Uuid::new_v4();
            (id, task, key, connection)
        };
        let rows = [
            project("GA", Some(false)),
            project("GB", Some(true)),
            project("GC", None),
        ];
        for (id, task, key, connection) in rows {
            exec(
                db,
                "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, $3, $3, $4)",
                vec![id.into(), workspace.into(), key.into(), owner.into()],
            )
            .await;
            if let Some(enabled) = connection {
                exec(
                    db,
                    "INSERT INTO project_execution_settings (project_id, enabled, working_directory) VALUES ($1, $2, '/work/a')",
                    vec![id.into(), enabled.into()],
                )
                .await;
                exec(
                    db,
                    "INSERT INTO ai_participants (id, project_id, name, is_active, registered_by) VALUES ($1::text, $2, 'gate bot', $3, $4)",
                    vec![bot.into(), id.into(), enabled.into(), owner.into()],
                )
                .await;
                exec(
                    db,
                    "INSERT INTO project_bot_connections (project_id, bot_id) VALUES ($1, $2)",
                    vec![id.into(), bot.into()],
                )
                .await;
            }
            exec(
                db,
                "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, max_attempts) \
                 VALUES ($1, $2, $3, 'issue_assigned', 'pending', 3)",
                vec![task.into(), id.into(), bot.into()],
            )
            .await;
        }
        let [a, b, c] = rows.map(|(id, task, _, _)| (id, task));
        GateWorld {
            bot,
            disabled: a,
            enabled: b,
            never: c,
        }
    }

    async fn status_by(db: &DatabaseConnection, task: Uuid) -> String {
        #[derive(Debug, FromQueryResult)]
        struct S {
            status: String,
        }
        S::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status FROM ai_tasks WHERE id = $1",
            vec![task.into()],
        ))
        .one(db)
        .await
        .expect("query")
        .expect("task")
        .status
    }

    /// Only a project with an enabled connection has its tasks taken: a disabled connection and a
    /// project that was never connected keep theirs out of the pick-up, though the bot is connected
    /// elsewhere and its runner is online.
    #[tokio::test]
    async fn pick_up_takes_only_the_tasks_of_projects_with_an_enabled_connection() {
        let scratch = scratch_or_skip!("gate_pickup");
        let db = &scratch.db;
        let w = gate_world(db).await;
        let picked = pickup_pending_tasks(db, 10, 3_600).await.expect("pick-up runs");
        assert_eq!(
            picked.iter().map(|task| task.id).collect::<Vec<_>>(),
            vec![w.enabled.1],
            "only the task of the enabled connection is picked up"
        );
        assert_eq!(status_by(db, w.disabled.1).await, "pending");
        assert_eq!(status_by(db, w.never.1).await, "pending");
        scratch.drop_self().await;
    }

    /// A task still waiting for a project whose connection is disabled or gone is failed with the
    /// reason stored, a running task is left alone, and the sweep does not repeat itself.
    #[tokio::test]
    async fn waiting_tasks_of_a_project_without_an_enabled_connection_are_failed_with_a_reason() {
        let scratch = scratch_or_skip!("gate_sweep");
        let db = &scratch.db;
        let state = state_for(db.clone());
        let w = gate_world(db).await;
        // A task of the disabled project that is already running.
        let running = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts, lease_expires_at) \
             VALUES ($1, $2, $3, 'issue_assigned', 'processing', 1, 3, now() + interval '1 hour')",
            vec![running.into(), w.disabled.0.into(), w.bot.into()],
        )
        .await;

        super::fail_unconnected_tasks(&state, 10).await.expect("the sweep runs");
        assert_eq!(status_by(db, w.disabled.1).await, "failed");
        assert_eq!(status_by(db, w.never.1).await, "failed");
        assert_eq!(
            status_by(db, w.enabled.1).await,
            "pending",
            "an enabled connection keeps its task"
        );
        assert_eq!(
            status_by(db, running).await,
            "processing",
            "a running task continues until it ends"
        );
        for (task, code, message) in [
            (
                w.disabled.1,
                "connection_disabled",
                "the bot's connection to this project is disabled",
            ),
            (
                w.never.1,
                "bot_not_connected_to_project",
                "the bot is not connected to this project",
            ),
        ] {
            let found = Count::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT COUNT(*)::bigint AS n FROM ai_task_events e JOIN ai_tasks t ON t.id = e.task_id \
                 WHERE e.task_id = $1 AND e.event_type = 'failed' AND e.payload->>'reason' = 'connection_unavailable' \
                   AND e.payload->>'reason_code' = $2 AND t.error_message = $3",
                vec![task.into(), code.into(), message.into()],
            ))
            .one(db)
            .await
            .expect("count runs")
            .map_or(0, |row| row.n);
            assert_eq!(found, 1, "{code}");
        }
        assert_eq!(events(db, w.enabled.1, "failed").await, 0);
        // A second sweep finds nothing more to fail.
        let again = fail_tasks_without_connection(db, 10)
            .await
            .expect("the second sweep runs");
        assert!(again.is_empty());
        scratch.drop_self().await;
    }

    /// A connection disabled after a task was picked up still stops its delivery: the attempt
    /// ends like a refusal and nothing is sent.
    #[tokio::test]
    async fn a_connection_disabled_after_pick_up_stops_the_delivery() {
        let scratch = scratch_or_skip!("gate_dispatch");
        let db = &scratch.db;
        let state = state_for(db.clone());
        let w = gate_world(db).await;
        let picked = pickup_pending_tasks(db, 10, 3_600).await.expect("pick-up runs");
        let task = picked
            .iter()
            .find(|task| task.id == w.enabled.1)
            .expect("the enabled task is picked up");
        exec(
            db,
            "UPDATE project_execution_settings SET enabled = false WHERE project_id = $1",
            vec![w.enabled.0.into()],
        )
        .await;
        // The runner address is unreachable by design: a delivery attempt would fail differently.
        let client = reqwest::Client::new();
        super::deliver_task(&state, &client, task)
            .await
            .expect("the attempt settles");
        assert_eq!(status_by(db, w.enabled.1).await, "failed");
        let reason = Count::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM ai_task_events WHERE task_id = $1 AND event_type = 'failed' \
             AND payload->>'reason' = 'runner_declined' AND payload->>'reason_code' = 'connection_disabled'",
            vec![w.enabled.1.into()],
        ))
        .one(db)
        .await
        .expect("count runs")
        .map_or(0, |row| row.n);
        assert_eq!(reason, 1);
        scratch.drop_self().await;
    }
}
