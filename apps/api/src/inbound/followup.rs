//! The state-based follow-up of alerts, run by the worker.
//!
//! One place sees every path that ends a task (the worker's own declines and sweeps and the task
//! protocol's endpoints) because it reads the result instead of hooking each
//! path: an alert whose task is `completed` or `failed` and has not been announced yet gets one
//! notification for the rule's recipients, and the alert records that it was announced in the same
//! transaction. Running a tick twice, or from two workers, announces nothing twice. An attempt
//! that fails and goes back to `pending` is not an outcome and notifies nobody.
//!
//! The same tick starts tasks that waited behind a cap and suppresses those that waited an hour
//! (see `queue`), closes alerts whose work item reached a terminal state or whose dedup window has
//! passed, and an hourly call prunes stored events past their retention.

use platform::{config::InboundConfig, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::json;
use uuid::Uuid;

use super::{
    audit,
    notify::{self, AlertNotification, Subject},
    queue,
    severity::Severity,
};
use crate::error::ApiError;

/// Alerts handled per tick and per step; a backlog drains over the next ticks.
const BATCH: i64 = 100;
/// Events deleted per statement during pruning.
const PRUNE_BATCH: i64 = 5_000;

/// What one tick did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FollowupReport {
    pub announced: usize,
    pub closed: usize,
    /// Queued tasks created because capacity freed up.
    pub started: usize,
    /// Queued tasks suppressed after waiting an hour.
    pub expired: usize,
    /// Queued tasks settled because the rule, the bot or the work item no longer allows one.
    pub dropped: usize,
}

#[derive(FromQueryResult)]
struct Pending {
    id: Uuid,
}

#[derive(FromQueryResult)]
struct Ended {
    id: Uuid,
    task_id: Uuid,
    task_status: String,
    work_item_id: Uuid,
    work_item_title: String,
    max_severity_rank: i16,
    source_name: String,
    workspace_id: Uuid,
    project_id: Uuid,
    notify_user_ids: Option<Vec<Uuid>>,
    kind: Option<String>,
}

/// One tick: announce ended tasks, work the queue, then close finished alerts.
pub async fn run_once(db: &DatabaseConnection, config: &InboundConfig) -> Result<FollowupReport, ApiError> {
    run_once_at(db, config, Timestamp::now()).await
}

/// [`run_once`] with the clock of the queue given, so its one-hour expiry can be tested.
pub async fn run_once_at(
    db: &DatabaseConnection,
    config: &InboundConfig,
    now: Timestamp,
) -> Result<FollowupReport, ApiError> {
    let announced = announce_ended_tasks(db).await?;
    let queued = queue::drain(db, config.max_active_alert_tasks_per_workspace, now).await?;
    let closed = close_finished_alerts(db).await?;
    Ok(FollowupReport {
        announced,
        closed,
        started: queued.started,
        expired: queued.expired,
        dropped: queued.dropped,
    })
}

async fn announce_ended_tasks(db: &DatabaseConnection) -> Result<usize, ApiError> {
    let pending = Pending::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT a.id FROM inbound_alerts a JOIN ai_tasks t ON t.id = a.task_id \
          WHERE t.status IN ('completed', 'failed') AND a.outcome_notified_task_id IS DISTINCT FROM a.task_id \
          ORDER BY t.updated_at LIMIT $1",
        vec![BATCH.into()],
    ))
    .all(db)
    .await?;
    let mut announced = 0;
    for alert in pending {
        if announce_one(db, alert.id).await? {
            announced += 1;
        }
    }
    Ok(announced)
}

/// Announces the outcome of the alert's task in one transaction. `false` when another worker got
/// there first or the task is no longer ended.
async fn announce_one(db: &DatabaseConnection, alert_id: Uuid) -> Result<bool, ApiError> {
    let tx = db.begin().await?;
    // The row lock, with SKIP LOCKED, lets several workers run the tick without announcing twice.
    let locked = Pending::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT a.id FROM inbound_alerts a WHERE a.id = $1 FOR UPDATE SKIP LOCKED",
        vec![alert_id.into()],
    ))
    .one(&tx)
    .await?;
    if locked.is_none() {
        return Ok(false);
    }
    let ended = Ended::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT a.id, a.task_id, t.status AS task_status, a.work_item_id, w.title AS work_item_title, \
                a.max_severity_rank, s.name AS source_name, s.workspace_id, a.project_id, \
                r.notify_user_ids, \
                (SELECT e.kind FROM inbound_events e WHERE e.alert_id = a.id ORDER BY e.received_at LIMIT 1) AS kind \
           FROM inbound_alerts a \
           JOIN ai_tasks t ON t.id = a.task_id \
           JOIN work_items w ON w.id = a.work_item_id \
           JOIN inbound_sources s ON s.id = a.source_id \
           LEFT JOIN alert_rules r ON r.id = a.rule_id \
          WHERE a.id = $1 AND t.status IN ('completed', 'failed') \
            AND a.outcome_notified_task_id IS DISTINCT FROM a.task_id",
        vec![alert_id.into()],
    ))
    .one(&tx)
    .await?;
    let Some(ended) = ended else {
        return Ok(false);
    };
    let kind = if ended.task_status == "completed" {
        AlertNotification::TaskCompleted
    } else {
        AlertNotification::TaskFailed
    };
    notify::send(
        &tx,
        kind,
        &Subject {
            workspace_id: ended.workspace_id,
            project_id: ended.project_id,
            work_item_id: Some(ended.work_item_id),
        },
        ended.notify_user_ids.as_deref().unwrap_or_default(),
        &notify::task_outcome_content(kind, &ended.work_item_title),
        &notify::payload(
            Some(ended.id),
            Severity::from_rank(ended.max_severity_rank),
            ended.kind.as_deref(),
            &ended.source_name,
            Some(ended.task_id),
        ),
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET task_ended_at = COALESCE((SELECT completed_at FROM ai_tasks WHERE id = $2), now()), \
                outcome_notified_task_id = $2, updated_at = now() WHERE id = $1",
        vec![alert_id.into(), ended.task_id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(FromQueryResult)]
struct Closing {
    id: Uuid,
    work_item_id: Uuid,
    project_id: Uuid,
    workspace_id: Uuid,
    occurrences: i32,
    reason: String,
}

/// Closes open alerts whose work item is in a terminal state of the project's workflow, or whose
/// dedup window has passed without a new occurrence.
async fn close_finished_alerts(db: &DatabaseConnection) -> Result<usize, ApiError> {
    let tx = db.begin().await?;
    let closing = Closing::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH wf AS ( \
           SELECT p.id AS project_id, p.workspace_id, \
                  COALESCE(p.workflow_id, ws.workflow_id, \
                           (SELECT id FROM workflows WHERE is_system_default ORDER BY created_at LIMIT 1)) AS workflow_id \
             FROM projects p JOIN workspaces ws ON ws.id = p.workspace_id) \
         SELECT a.id, a.work_item_id, a.project_id, wf.workspace_id, a.occurrences, \
                CASE WHEN EXISTS (SELECT 1 FROM workflow_states st WHERE st.workflow_id = wf.workflow_id \
                                  AND st.key = w.state AND st.is_terminal) \
                     THEN 'terminal_state' ELSE 'window_expired' END AS reason \
           FROM inbound_alerts a \
           JOIN work_items w ON w.id = a.work_item_id \
           JOIN wf ON wf.project_id = a.project_id \
           LEFT JOIN alert_rules r ON r.id = a.rule_id \
          WHERE a.open AND ( \
                EXISTS (SELECT 1 FROM workflow_states st WHERE st.workflow_id = wf.workflow_id \
                        AND st.key = w.state AND st.is_terminal) \
             OR a.last_seen_at + make_interval(secs => COALESCE(r.dedup_window_secs, 3600)) < now()) \
          LIMIT $1 FOR UPDATE OF a SKIP LOCKED",
        vec![BATCH.into()],
    ))
    .all(&tx)
    .await?;
    for alert in &closing {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE inbound_alerts SET open = false, updated_at = now() WHERE id = $1",
            vec![alert.id.into()],
        ))
        .await?;
        audit::record(
            &tx,
            alert.workspace_id,
            alert.project_id,
            "alert.closed",
            "work_item",
            alert.work_item_id,
            None,
            json!({ "type": "alert_followup" }),
            json!({ "alert_id": alert.id, "occurrences": alert.occurrences, "reason": alert.reason }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(closing.len())
}

/// Deletes stored events older than `retention_days`, in batches. Returns how many were deleted.
pub async fn prune_events(db: &DatabaseConnection, retention_days: u32) -> Result<u64, ApiError> {
    let days = i32::try_from(retention_days).unwrap_or(i32::MAX);
    let mut total = 0_u64;
    loop {
        let deleted = db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM inbound_events WHERE id IN ( \
                   SELECT id FROM inbound_events WHERE received_at < now() - make_interval(days => $1) LIMIT $2)",
                vec![days.into(), PRUNE_BATCH.into()],
            ))
            .await?
            .rows_affected();
        total = total.saturating_add(deleted);
        if i64::try_from(deleted).unwrap_or(i64::MAX) < PRUNE_BATCH {
            return Ok(total);
        }
    }
}
