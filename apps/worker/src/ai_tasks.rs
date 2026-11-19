//! Worker AI tasks: pick-up, delivery to the bot's webhook, and the end of every attempt that the
//! receiver does not end itself.
//!
//! An attempt starts when a pending task is picked up (`processing`, `attempts + 1`) and is
//! delivered to the bot's webhook. It ends in one of four ways:
//!
//! - The receiver takes the task (`dispatched`, or any other 2xx answer it does not decline) and
//!   later reports the outcome through the API's complete or fail callback.
//! - The receiver answers the delivery that it will not run the task (`no_agent`, `ignored`,
//!   `refused`, or an HTTP 4xx a repeat cannot change): the worker fails the task at once. The
//!   answer depends on the receiver's configuration, which a retry seconds later does not change,
//!   and the receiver's own fail callback may be refused (it may hold no token for this bot).
//! - The delivery does not reach a receiver that takes it (no webhook, a transport error, HTTP
//!   5xx, 408, 409, 425, 429): the worker re-queues the task while attempts remain.
//! - Nothing ends the attempt within `[ai_tasks] processing_timeout_secs`: the sweep re-queues the
//!   task while attempts remain and fails it otherwise.
//!
//! Every transition the worker makes is a compare-and-set on `status = 'processing'` and the
//! attempt number (`api::services::ai_task_service`), so a callback that commits first wins and
//! several workers never end the same attempt twice.

use api::outbound::validate_outbound_url;
use api::services::ai_task_service::{
    AttemptEnd, WorkerEndReason, end_processing_attempt, end_stalled_attempts, notify_ai_task_failed,
};
use api::webhook_trigger::{WEBHOOK_SIGNATURE_HEADER, WEBHOOK_USER_AGENT, sign_payload};
use platform::{app::AppState, timestamp::Timestamp};
use reqwest::header::CONTENT_TYPE;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

/// At most this many characters of a receiver's answer go into a task's error message.
const MAX_ANSWER_CHARS: usize = 300;

/// The `status` values with which a receiver answers that it will not run a task:
/// `sylvode-webhook` answers `no_agent` when no agent matches, `ignored` when the delivery is not
/// a bot task, and `refused` when the matched agent cannot run it (safe mode, a disabled feature,
/// an incomplete agent configuration).
const DECLINING_ANSWERS: [&str; 3] = ["no_agent", "ignored", "refused"];

#[derive(Debug, Clone, FromQueryResult)]
struct AiTaskDispatchRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
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

/// The receiver's HTTP answer to a delivery.
#[derive(Debug)]
struct DeliveryAnswer {
    webhook_id: Uuid,
    status: u16,
    body: String,
}

/// What a delivery's answer means for the attempt.
#[derive(Debug, PartialEq, Eq)]
enum DeliveryVerdict {
    /// The receiver took the task; it ends the attempt through a callback, or the timeout does.
    Accepted,
    /// The receiver will not run the task; a repeat would get the same answer.
    Declined(String),
    /// The delivery may succeed when repeated.
    Transient(String),
}

/// Reads a receiver's answer to a delivery.
///
/// A 2xx answer takes the task unless its JSON `status` is one of [`DECLINING_ANSWERS`]; an
/// answer that is not JSON, or has another status, takes it as before. HTTP 408, 409, 425, 429 and
/// 5xx are transient, and so are redirects (which this client does not follow, so the URL needs
/// fixing, but an operator may fix it without touching the task). Any other 4xx (a signature
/// mismatch, a malformed body) declines the task.
fn classify_delivery_answer(status: u16, body: &str) -> DeliveryVerdict {
    if (200..300).contains(&status) {
        let answer = serde_json::from_str::<Value>(body).ok();
        let declined = answer
            .as_ref()
            .and_then(|answer| answer.get("status"))
            .and_then(Value::as_str)
            .filter(|kind| DECLINING_ANSWERS.contains(kind));
        return match (declined, answer.as_ref()) {
            (Some(kind), Some(answer)) => DeliveryVerdict::Declined(format!(
                "the receiver answered `{kind}` and will not run the task: {}",
                declined_detail(answer, body)
            )),
            _ => DeliveryVerdict::Accepted,
        };
    }
    let detail = excerpt(body);
    if (400..500).contains(&status) && !matches!(status, 408 | 409 | 425 | 429) {
        DeliveryVerdict::Declined(format!(
            "the receiver answered HTTP {status} and will not run the task: {detail}"
        ))
    } else {
        DeliveryVerdict::Transient(format!("the receiver answered HTTP {status}: {detail}"))
    }
}

/// What a declining answer says about why: its `reason`, else the bot it found no agent for,
/// else the answer itself.
fn declined_detail(answer: &Value, body: &str) -> String {
    let field = |key: &str| answer.get(key).and_then(Value::as_str).filter(|v| !v.is_empty());
    field("reason").map_or_else(
        || field("bot_key").map_or_else(|| excerpt(body), |bot| format!("no agent matches bot {}", excerpt(bot))),
        excerpt,
    )
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
pub async fn process_pending_tasks(
    state: &AppState,
    client: &reqwest::Client,
    concurrency: usize,
) -> anyhow::Result<()> {
    let db = &state.db;
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    let tasks = pickup_pending_tasks(db, limit).await?;

    for task in tasks {
        let verdict = match dispatch_task(db, client, &task).await {
            Ok(answer) => match classify_delivery_answer(answer.status, &answer.body) {
                DeliveryVerdict::Accepted => DeliveryVerdict::Accepted,
                DeliveryVerdict::Declined(reason) => {
                    DeliveryVerdict::Declined(format!("webhook {}: {reason}", answer.webhook_id))
                }
                DeliveryVerdict::Transient(reason) => {
                    DeliveryVerdict::Transient(format!("webhook {}: {reason}", answer.webhook_id))
                }
            },
            Err(err) => DeliveryVerdict::Transient(err.to_string()),
        };
        match verdict {
            DeliveryVerdict::Accepted => tracing::info!(
                task_id = %task.id,
                attempt = task.attempts,
                "AI task delivered; the receiver ends it, or the processing timeout does"
            ),
            DeliveryVerdict::Declined(reason) => {
                tracing::warn!(task_id = %task.id, attempt = task.attempts, reason = %reason, "AI task declined by the receiver; failing it");
                end_attempt(
                    state,
                    &task,
                    AttemptEnd::Terminal,
                    WorkerEndReason::ReceiverDeclined,
                    &reason,
                )
                .await?;
            }
            DeliveryVerdict::Transient(reason) => {
                tracing::warn!(task_id = %task.id, attempt = task.attempts, error = %reason, "dispatch failed");
                end_attempt(
                    state,
                    &task,
                    AttemptEnd::RetryWhileAttemptsRemain,
                    WorkerEndReason::DeliveryFailed,
                    &reason,
                )
                .await?;
            }
        }
    }

    Ok(())
}

/// Ends the attempt just delivered, unless a callback already ended it, and sends
/// `ai.task_failed` when the task is failed.
async fn end_attempt(
    state: &AppState,
    task: &AiTaskDispatchRow,
    end: AttemptEnd,
    reason: WorkerEndReason,
    error_message: &str,
) -> anyhow::Result<()> {
    let ended = end_processing_attempt(&state.db, task.id, task.attempts, end, reason, error_message)
        .await
        .map_err(|err| anyhow::anyhow!("ending AI task {} attempt {} failed: {err}", task.id, task.attempts))?;
    match ended {
        Some(ended) if ended.status == "failed" => notify_failed(state, &ended).await,
        Some(_) => {}
        None => tracing::info!(
            task_id = %task.id,
            attempt = task.attempts,
            "AI task attempt was already ended by a callback; left as it is"
        ),
    }
    Ok(())
}

/// Ends every attempt that has been `processing` for longer than `timeout_secs`.
pub async fn end_stalled_tasks(state: &AppState, timeout_secs: u64, concurrency: usize) -> anyhow::Result<usize> {
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    let ended = end_stalled_attempts(&state.db, timeout_secs, limit)
        .await
        .map_err(|err| anyhow::anyhow!("ending stalled AI tasks failed: {err}"))?;
    for task in &ended {
        tracing::warn!(
            task_id = %task.id,
            attempt = task.attempts,
            max_attempts = task.max_attempts,
            status = %task.status,
            timeout_secs,
            "AI task got no completion within the processing timeout"
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

async fn pickup_pending_tasks(db: &sea_orm::DatabaseConnection, limit: i64) -> anyhow::Result<Vec<AiTaskDispatchRow>> {
    let tasks = AiTaskDispatchRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH picked AS (
                SELECT id
                FROM ai_tasks
                WHERE status = 'pending'
                  AND (next_retry_at IS NULL OR next_retry_at <= now())
                ORDER BY priority DESC, created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE ai_tasks t
            SET status = 'processing',
                attempts = attempts + 1,
                started_at = now(),
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
        vec![limit.into()],
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

/// Delivers `task` and returns the receiver's answer. An error means it was not delivered: no
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

    let body = json!({
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
    });

    let target = validate_outbound_url(&webhook.url)
        .await
        .map_err(|err| anyhow::anyhow!("webhook {} url rejected: {err}", webhook.webhook_id))?;

    // Signed over the exact bytes that are sent, the same way the API's own webhook deliveries
    // are, so a receiver can refuse unsigned traffic instead of having to trust this path.
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
            tracing::warn!(task_id = %task.id, webhook_id = %webhook.webhook_id, error = %err.without_url(), "reading the receiver's answer failed; treating it as empty");
            String::new()
        }
    };
    Ok(DeliveryAnswer {
        webhook_id: webhook.webhook_id,
        status,
        body,
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

fn trigger_kind_for_task(task_type: &str) -> &'static str {
    match task_type {
        "issue_assigned" => "assigned",
        "vote_requested" => "proposal_vote",
        "review_requested" | "comment_requested" => "mention",
        _ => "manual",
    }
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
    use super::{DeliveryVerdict, classify_delivery_answer, dispatch_request, excerpt};

    /// The AI-task delivery carries the same `User-Agent` as every other outbound webhook
    /// delivery (ADR-0020 D3), next to its signature.
    #[test]
    fn the_ai_task_delivery_carries_the_webhook_user_agent() {
        let client = reqwest::Client::new();
        let target = reqwest::Url::parse("https://receiver.example/hook").expect("valid url");
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

    fn declined(verdict: &DeliveryVerdict) -> &str {
        match verdict {
            DeliveryVerdict::Declined(reason) => reason,
            other => panic!("expected a declined delivery, got {other:?}"),
        }
    }

    #[test]
    fn a_dispatched_answer_takes_the_task() {
        for body in [
            r#"{"status":"dispatched","agent":"fixer","result":"cli agent spawned"}"#,
            r#"{"status":"ok"}"#,
            "",
            "accepted",
        ] {
            assert_eq!(classify_delivery_answer(200, body), DeliveryVerdict::Accepted, "{body}");
        }
        assert_eq!(
            classify_delivery_answer(202, r#"{"status":"dispatched"}"#),
            DeliveryVerdict::Accepted
        );
    }

    /// The receiver's answer is named in the reason the task is failed with.
    #[test]
    fn a_no_agent_answer_declines_the_task_and_says_why() {
        let current = classify_delivery_answer(
            200,
            r#"{"status":"no_agent","bot_key":"bot-b","reason":"sylvode-webhook has no agent for this AI task (bot bot-b)"}"#,
        );
        let reason = declined(&current);
        assert!(reason.contains("`no_agent`"), "{reason}");
        assert!(reason.contains("has no agent for this AI task (bot bot-b)"), "{reason}");

        // An older receiver answers without `reason`.
        let older = classify_delivery_answer(200, r#"{"status":"no_agent","bot_key":"bot-b","route":{}}"#);
        assert!(declined(&older).contains("no agent matches bot bot-b"));
    }

    #[test]
    fn ignored_and_refused_answers_decline_the_task() {
        let ignored = classify_delivery_answer(200, r#"{"status":"ignored","reason":"not_bot_task"}"#);
        assert!(declined(&ignored).contains("`ignored`"));
        assert!(declined(&ignored).contains("not_bot_task"));
        let refused = classify_delivery_answer(
            200,
            r#"{"status":"refused","agent":"fixer","reason":"cli disabled by feature flag or safe mode"}"#,
        );
        assert!(declined(&refused).contains("safe mode"));
    }

    #[test]
    fn a_client_error_declines_the_task() {
        for status in [400, 401, 403, 404, 413, 422] {
            let verdict = classify_delivery_answer(status, "Unauthorized");
            assert!(
                declined(&verdict).contains(&format!("HTTP {status}")),
                "{status}: {verdict:?}"
            );
        }
    }

    #[test]
    fn server_errors_throttling_and_conflicts_are_retried() {
        for status in [500, 502, 503, 504, 408, 409, 425, 429, 301, 302, 307] {
            assert!(
                matches!(classify_delivery_answer(status, ""), DeliveryVerdict::Transient(ref reason) if reason.contains(&format!("HTTP {status}"))),
                "{status} must be retried"
            );
        }
    }

    #[test]
    fn a_long_answer_is_cut() {
        let long = "x".repeat(1_000);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), 303);
        assert!(cut.ends_with("..."));
        let verdict = classify_delivery_answer(400, &long);
        assert!(declined(&verdict).len() < 400);
    }
}
