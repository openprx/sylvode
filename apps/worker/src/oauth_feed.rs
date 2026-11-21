//! The single writer of the delegated event feed, and the retention of everything the OAuth
//! authorization server keeps.
//!
//! # The writer
//!
//! Triggers on `notifications` and `ai_tasks` append a row to `oauth_event_outbox` in the same
//! transaction as the domain write. [`run_once`] moves outbox rows into `oauth_event_feed` in one
//! transaction:
//!
//! 1. it takes `pg_try_advisory_xact_lock`, so of any number of worker processes exactly one is
//!    writing at a time; the others see the lock taken and do nothing;
//! 2. it reads up to [`BATCH`] outbox rows in id order. Only rows whose transaction has committed
//!    are visible, so a row that commits later is simply picked up by a later run;
//! 3. it builds each subject from the current source rows and inserts it into the feed, whose
//!    `seq` identity is assigned here, inside the lock;
//! 4. it deletes the outbox rows it processed and sends `NOTIFY sv_oauth_feed`;
//! 5. it commits.
//!
//! Because `seq` is assigned only by the lock holder, and the next holder starts after the
//! previous commit, **`seq` order equals the order in which rows became visible to the writer**:
//! a row that commits later always gets the higher `seq`, and a reader that has seen `seq` N will
//! never see an `N - 1` appear later. Two outbox rows committed between the same two runs share a
//! batch and are ordered by outbox id.
//!
//! Steps 2 to 5 are one transaction, so a crash at any point either leaves the outbox untouched
//! (nothing was written) or has already written the feed rows and removed their outbox rows. An
//! event is neither lost nor written twice. Reaching a client is at-least-once, with a stable
//! event id, as the stream documents.
//!
//! # Retention
//!
//! Feed and outbox rows past `[oauth] event_feed_retention_days` are deleted, and the daily prune
//! removes audit rows (180 days), operation logs (90 days) and expired authorization requests,
//! codes and tokens. Every delete runs in batches of [`PRUNE_BATCH`] rows chosen by key, so it
//! holds row locks on at most one batch, and never takes a table lock: the writer's inserts and
//! the streams' reads are not blocked.
//!
//! Payloads and ids are never logged here; the log lines carry counts only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use platform::config::OAuthConfig;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::time::Instant;
use uuid::Uuid;

/// Key of the advisory lock the writer holds. Any process that wants to write the feed takes it.
pub const WRITER_LOCK_KEY: i64 = 0x5356_4F46_4545_4421;
/// Outbox rows moved per transaction.
pub const BATCH: i64 = 500;
/// Rows removed per delete statement by the retention sweeps.
pub const PRUNE_BATCH: i64 = 1000;
/// The most delete statements one sweep issues per table, so a long backlog is worked off over
/// several sweeps instead of one long one.
const MAX_PRUNE_ROUNDS: usize = 50;
/// How often the writer looks at the outbox when no wake-up arrives.
const TICK: Duration = Duration::from_secs(1);
/// How often the feed and outbox retention runs.
const SWEEP_INTERVAL: Duration = Duration::from_mins(10);
/// How often the audit, log and token prune runs.
const DAILY_INTERVAL: Duration = Duration::from_hours(24);
/// Pause between two delete batches.
const PRUNE_PAUSE: Duration = Duration::from_millis(50);
/// Retention days assumed when the feature is not configured, only to keep the outbox bounded.
const DEFAULT_RETENTION_DAYS: i32 = 7;
const AUDIT_RETENTION_DAYS: i32 = 180;
const OPERATION_LOG_RETENTION_DAYS: i32 = 90;

/// What one run of the writer did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Outbox rows written to the feed.
    pub moved: usize,
    /// Outbox rows removed without an event: the source row is gone or the row is unusable.
    pub dropped: usize,
    /// Another process held the writer lock.
    pub contended: bool,
    /// Highest `seq` written, when any.
    pub last_seq: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct OutboxRow {
    id: i64,
    source: String,
    source_id: Uuid,
    event_type: String,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    user_id: Option<Uuid>,
    detail: Value,
    created_at: platform::timestamp::Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct NotificationRow {
    id: Uuid,
    notification_type: Option<String>,
    title: Option<String>,
    content: Option<String>,
    related_issue_id: Option<Uuid>,
    related_comment_id: Option<Uuid>,
    related_project_id: Option<Uuid>,
    is_read: Option<bool>,
}

#[derive(Debug, FromQueryResult)]
struct TaskRow {
    id: Uuid,
    task_type: String,
    status: String,
    reference_type: Option<String>,
    reference_id: Option<Uuid>,
    attempts: i32,
    max_attempts: i32,
    summary: Option<String>,
    error_message: Option<String>,
    ai_participant_id: Uuid,
    agent_name: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct WorkItemRow {
    id: Uuid,
    title: String,
    identifier: String,
}

#[derive(Debug, FromQueryResult)]
struct Flag {
    locked: bool,
}

#[derive(Debug, FromQueryResult)]
struct Seq {
    seq: i64,
}

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// The frozen subject of a notification event, from the notification as it is now.
fn notification_subject(outbox: &OutboxRow, note: &NotificationRow) -> Value {
    if outbox.event_type == "notification.read" {
        return json!({"kind": "notification", "notification_id": note.id, "is_read": true});
    }
    json!({
        "kind": "notification",
        "notification_id": note.id,
        "notification_type": note.notification_type,
        "title_key": note.title,
        "content": note.content,
        "related": {
            "issue_id": note.related_issue_id,
            "comment_id": note.related_comment_id,
            "project_id": note.related_project_id,
        },
        "is_read": note.is_read.unwrap_or(false),
    })
}

fn detail_text<'a>(detail: &'a Value, key: &str) -> Option<&'a str> {
    detail.get(key).and_then(Value::as_str)
}

/// The frozen subject of a task event. `status` and `previous_status` are the transition the
/// outbox row recorded, not whatever the task has moved on to since.
fn task_subject(outbox: &OutboxRow, task: &TaskRow, work_items: &HashMap<Uuid, WorkItemRow>) -> Value {
    let status = detail_text(&outbox.detail, "to").unwrap_or(&task.status);
    let attempts = outbox
        .detail
        .get("attempts")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| i64::from(task.attempts));
    let mut subject = json!({
        "kind": "ai_task",
        "task_id": task.id,
        "task_type": task.task_type,
        "status": status,
        "attempts": attempts,
        "max_attempts": task.max_attempts,
        "agent": {"id": task.ai_participant_id, "name": task.agent_name},
    });
    let Some(map) = subject.as_object_mut() else {
        return subject;
    };
    if let Some(previous) = detail_text(&outbox.detail, "from") {
        map.insert("previous_status".to_owned(), json!(previous));
    }
    if let (Some(kind), Some(id)) = (task.reference_type.as_deref(), task.reference_id) {
        let reference = work_items.get(&id).filter(|_| kind == "work_item").map_or_else(
            || json!({"type": kind, "id": id}),
            |item| json!({"type": kind, "id": id, "identifier": item.identifier, "title": item.title}),
        );
        map.insert("reference".to_owned(), reference);
    }
    if let Some(summary) = task.summary.as_deref().filter(|summary| !summary.is_empty()) {
        map.insert("summary".to_owned(), json!(summary));
    }
    if status == "failed"
        && let Some(error) = task.error_message.as_deref().filter(|error| !error.is_empty())
    {
        map.insert("error".to_owned(), json!(error));
    }
    subject
}

fn ids_of(rows: &[OutboxRow], source: &str) -> Vec<Uuid> {
    rows.iter()
        .filter(|row| row.source == source)
        .map(|row| row.source_id)
        .collect()
}

/// One writer step on the caller's transaction: lock, read, write, delete, notify. The caller
/// commits. Exposed so a test can end the transaction any way it likes, which is how a crash
/// before commit is simulated.
pub async fn process_batch(tx: &DatabaseTransaction) -> Result<TickReport, DbErr> {
    let locked = Flag::find_by_statement(statement(
        "SELECT pg_try_advisory_xact_lock($1) AS locked",
        vec![WRITER_LOCK_KEY.into()],
    ))
    .one(tx)
    .await?
    .is_some_and(|flag| flag.locked);
    if !locked {
        return Ok(TickReport {
            contended: true,
            ..TickReport::default()
        });
    }

    let rows = OutboxRow::find_by_statement(statement(
        "SELECT id, source, source_id, event_type, workspace_id, project_id, user_id, detail, created_at \
         FROM oauth_event_outbox ORDER BY id LIMIT $1",
        vec![BATCH.into()],
    ))
    .all(tx)
    .await?;
    if rows.is_empty() {
        return Ok(TickReport::default());
    }

    let notifications: HashMap<Uuid, NotificationRow> = NotificationRow::find_by_statement(statement(
        "SELECT id, type AS notification_type, title, content, related_issue_id, related_comment_id, \
                related_project_id, is_read \
         FROM notifications WHERE id = ANY($1)",
        vec![ids_of(&rows, "notification").into()],
    ))
    .all(tx)
    .await?
    .into_iter()
    .map(|row| (row.id, row))
    .collect();
    let tasks: HashMap<Uuid, TaskRow> = TaskRow::find_by_statement(statement(
        "SELECT t.id, t.task_type, t.status, t.reference_type, t.reference_id, t.attempts, t.max_attempts, \
                left(t.result->>'summary', 2000) AS summary, left(t.error_message, 500) AS error_message, \
                t.ai_participant_id, u.name AS agent_name \
         FROM ai_tasks t LEFT JOIN users u ON u.id = t.ai_participant_id WHERE t.id = ANY($1)",
        vec![ids_of(&rows, "ai_task").into()],
    ))
    .all(tx)
    .await?
    .into_iter()
    .map(|row| (row.id, row))
    .collect();
    let referenced: Vec<Uuid> = tasks
        .values()
        .filter(|task| task.reference_type.as_deref() == Some("work_item"))
        .filter_map(|task| task.reference_id)
        .collect();
    let work_items: HashMap<Uuid, WorkItemRow> = WorkItemRow::find_by_statement(statement(
        "SELECT w.id, w.title, p.key || '-' || w.sequence_number::text AS identifier \
         FROM work_items w JOIN projects p ON p.id = w.project_id WHERE w.id = ANY($1)",
        vec![referenced.into()],
    ))
    .all(tx)
    .await?
    .into_iter()
    .map(|row| (row.id, row))
    .collect();

    let mut report = TickReport::default();
    let mut processed = Vec::with_capacity(rows.len());
    for row in &rows {
        processed.push(row.id);
        let subject = match row.source.as_str() {
            "notification" => notifications
                .get(&row.source_id)
                .map(|note| notification_subject(row, note)),
            "ai_task" => tasks
                .get(&row.source_id)
                .map(|task| task_subject(row, task, &work_items)),
            _ => None,
        };
        let Some(subject) = subject else {
            tracing::debug!(
                outbox_id = row.id,
                "oauth outbox row dropped: its source row is gone or unknown"
            );
            report.dropped += 1;
            continue;
        };
        let written = Seq::find_by_statement(statement(
            "INSERT INTO oauth_event_feed (event_type, workspace_id, project_id, user_id, subject, occurred_at) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING seq",
            vec![
                row.event_type.clone().into(),
                row.workspace_id.into(),
                row.project_id.into(),
                row.user_id.into(),
                subject.into(),
                row.created_at.into(),
            ],
        ))
        .one(tx)
        .await?;
        if let Some(written) = written {
            report.moved += 1;
            report.last_seq = Some(report.last_seq.map_or(written.seq, |seq| seq.max(written.seq)));
        }
    }

    tx.execute(statement(
        "DELETE FROM oauth_event_outbox WHERE id = ANY($1)",
        vec![processed.into()],
    ))
    .await?;
    if let Some(seq) = report.last_seq {
        // Delivered when the transaction commits, so a stream never wakes for rows it cannot see.
        tx.execute(statement(
            "SELECT pg_notify($1, $2)",
            vec![api::oauth::stream::FEED_CHANNEL.into(), seq.to_string().into()],
        ))
        .await?;
    }
    Ok(report)
}

/// One transaction of the writer.
pub async fn run_once(db: &DatabaseConnection) -> Result<TickReport, DbErr> {
    let tx = db.begin().await?;
    match process_batch(&tx).await {
        Ok(report) => {
            tx.commit().await?;
            Ok(report)
        }
        Err(error) => {
            if let Err(rollback) = tx.rollback().await {
                tracing::warn!(error = %rollback, "oauth feed rollback failed");
            }
            Err(error)
        }
    }
}

// -----------------------------------------------------------------------------------------------
// Retention
// -----------------------------------------------------------------------------------------------

/// Deletes in batches of [`PRUNE_BATCH`] until a batch comes back short or the round cap is hit.
/// `sql` takes the batch size as `$1` and, when `days` is given, the age in days as `$2`.
async fn prune(db: &DatabaseConnection, sql: &str, days: Option<i32>) -> Result<u64, DbErr> {
    let mut total = 0_u64;
    for round in 0..MAX_PRUNE_ROUNDS {
        let mut values: Vec<sea_orm::Value> = vec![PRUNE_BATCH.into()];
        if let Some(days) = days {
            values.push(days.into());
        }
        let deleted = db.execute(statement(sql, values)).await?.rows_affected();
        total = total.saturating_add(deleted);
        if i64::try_from(deleted).unwrap_or(i64::MAX) < PRUNE_BATCH {
            break;
        }
        if round + 1 < MAX_PRUNE_ROUNDS {
            tokio::time::sleep(PRUNE_PAUSE).await;
        }
    }
    Ok(total)
}

/// What a retention pass removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub feed: u64,
    pub outbox: u64,
}

/// Deletes feed and outbox rows older than `retention_days`.
pub async fn sweep_feed(db: &DatabaseConnection, retention_days: i32) -> Result<SweepReport, DbErr> {
    let feed = prune(
        db,
        "DELETE FROM oauth_event_feed WHERE seq IN ( \
           SELECT seq FROM oauth_event_feed WHERE created_at < now() - make_interval(days => $2::int) \
           ORDER BY seq LIMIT $1)",
        Some(retention_days),
    )
    .await?;
    let outbox = prune(
        db,
        "DELETE FROM oauth_event_outbox WHERE id IN ( \
           SELECT id FROM oauth_event_outbox WHERE created_at < now() - make_interval(days => $2::int) \
           ORDER BY id LIMIT $1)",
        Some(retention_days),
    )
    .await?;
    Ok(SweepReport { feed, outbox })
}

/// What the daily prune removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub audit_events: u64,
    pub operation_logs: u64,
    pub authorization_requests: u64,
    pub authorization_codes: u64,
    pub tokens: u64,
}

/// Deletes audit rows (180 days), operation logs (90 days) and authorization requests, codes and
/// tokens that expired more than a day (tokens: a week) ago. A consumed refresh token stays long
/// enough after its own expiry for a replay to be recognised as one.
pub async fn prune_daily(db: &DatabaseConnection) -> Result<PruneReport, DbErr> {
    Ok(PruneReport {
        audit_events: prune(
            db,
            "DELETE FROM oauth_audit_events WHERE id IN ( \
               SELECT id FROM oauth_audit_events WHERE occurred_at < now() - make_interval(days => $2::int) \
               ORDER BY occurred_at LIMIT $1)",
            Some(AUDIT_RETENTION_DAYS),
        )
        .await?,
        operation_logs: prune(
            db,
            "DELETE FROM oauth_operation_logs WHERE id IN ( \
               SELECT id FROM oauth_operation_logs WHERE created_at < now() - make_interval(days => $2::int) \
               ORDER BY created_at LIMIT $1)",
            Some(OPERATION_LOG_RETENTION_DAYS),
        )
        .await?,
        authorization_requests: prune(
            db,
            "DELETE FROM oauth_authorization_requests WHERE id IN ( \
               SELECT id FROM oauth_authorization_requests WHERE expires_at < now() - interval '1 day' \
               ORDER BY expires_at LIMIT $1)",
            None,
        )
        .await?,
        authorization_codes: prune(
            db,
            "DELETE FROM oauth_authorization_codes WHERE code_hash IN ( \
               SELECT code_hash FROM oauth_authorization_codes WHERE expires_at < now() - interval '1 day' \
               ORDER BY expires_at LIMIT $1)",
            None,
        )
        .await?,
        tokens: prune(
            db,
            "DELETE FROM oauth_tokens WHERE id IN ( \
               SELECT id FROM oauth_tokens WHERE expires_at < now() - interval '7 days' \
               ORDER BY expires_at LIMIT $1)",
            None,
        )
        .await?,
    })
}

// -----------------------------------------------------------------------------------------------
// The loop
// -----------------------------------------------------------------------------------------------

/// Wakes the writer when a trigger notifies the outbox channel. Reconnects forever; the writer's
/// own tick covers the time the connection is down.
async fn listen_for_outbox(db: DatabaseConnection, wake: Arc<Notify>) {
    use sea_orm::sqlx::postgres::PgListener;
    loop {
        let mut listener = match PgListener::connect_with(db.get_postgres_connection_pool()).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(error = %error, "oauth outbox listener connect failed; the writer polls");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        if let Err(error) = listener.listen(api::oauth::stream::OUTBOX_CHANNEL).await {
            tracing::warn!(error = %error, "oauth outbox LISTEN failed; the writer polls");
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        // Rows may have arrived while no listener was connected.
        wake.notify_one();
        loop {
            match listener.recv().await {
                Ok(_) => wake.notify_one(),
                Err(error) => {
                    tracing::warn!(error = %error, "oauth outbox LISTEN dropped; reconnecting");
                    break;
                }
            }
        }
    }
}

/// Moves outbox rows until a run finds fewer than a full batch.
async fn drain_outbox(db: &DatabaseConnection) {
    loop {
        match run_once(db).await {
            Ok(report) if report.moved + report.dropped >= usize::try_from(BATCH).unwrap_or(usize::MAX) => {
                tracing::debug!(
                    moved = report.moved,
                    dropped = report.dropped,
                    "oauth feed batch written"
                );
            }
            Ok(report) => {
                if report.moved + report.dropped > 0 {
                    tracing::debug!(
                        moved = report.moved,
                        dropped = report.dropped,
                        "oauth feed batch written"
                    );
                }
                return;
            }
            Err(error) => {
                tracing::warn!(error = %error, "oauth feed writer failed");
                return;
            }
        }
    }
}

/// Runs the writer and the retention for the life of the process. With `[oauth]` configured it
/// writes the feed; without it only the retention runs, so the outbox the triggers keep filling
/// stays bounded.
pub async fn run(db: DatabaseConnection, config: Option<Arc<OAuthConfig>>) {
    let enabled = config.is_some();
    let retention_days = config.as_ref().map_or(DEFAULT_RETENTION_DAYS, |config| {
        i32::try_from(config.event_feed_retention_days).unwrap_or(i32::MAX)
    });
    let wake = Arc::new(Notify::new());
    if enabled {
        tokio::spawn(listen_for_outbox(db.clone(), Arc::clone(&wake)));
    }
    let mut next_sweep = Instant::now();
    let mut next_daily = Instant::now();
    tracing::info!(writer = enabled, retention_days, "oauth event feed worker started");
    loop {
        if enabled {
            drain_outbox(&db).await;
        }
        if Instant::now() >= next_sweep {
            match sweep_feed(&db, retention_days).await {
                Ok(report) if report.feed + report.outbox > 0 => {
                    tracing::info!(
                        feed = report.feed,
                        outbox = report.outbox,
                        "oauth feed retention removed rows"
                    );
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(error = %error, "oauth feed retention failed"),
            }
            next_sweep = Instant::now() + SWEEP_INTERVAL;
        }
        if Instant::now() >= next_daily {
            match prune_daily(&db).await {
                Ok(report) => tracing::info!(?report, "oauth daily prune completed"),
                Err(error) => tracing::warn!(error = %error, "oauth daily prune failed"),
            }
            next_daily = Instant::now() + DAILY_INTERVAL;
        }
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(TICK) => {}
        }
    }
}

#[cfg(test)]
mod tests;
