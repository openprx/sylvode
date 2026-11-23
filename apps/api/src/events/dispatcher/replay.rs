//! Requeue and replay of deliveries.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::REPLAY_MAX_WINDOW_DAYS;
use super::expand::ReservedSourceRow;
use crate::error::ApiError;

#[cfg(test)]
tokio::task_local! {
    pub(super) static TEST_REPLAY_CANDIDATE_BARRIER: std::sync::Arc<tokio::sync::Barrier>;
}

/// Revives dead-lettered (`status='failed'`) `event_deliveries` rows for a workspace back into the
/// live queue, `events-v1.md`/`gate-commands.md`'s `requeue_failed` mode.
///
/// Clears `document_id` on every revived row -- content or not -- per the schema's own comment on
/// `event_deliveries.document_id` ("NULL ... for replay/requeue_failed-revived rows (deliberately,
/// so the partial coalescing unique index never applies to them)") and `gate-commands.md`'s
/// negative fixture: reviving a content delivery *without* clearing `document_id` lets it re-enter
/// `idx_event_deliveries_coalesce_pending`'s `(subscriber_kind, subscriber_id, document_id) WHERE
/// status='pending'` partial unique index under its original `document_id`, where it can collide
/// with a live pending row for the same document, or -- if there is none yet -- squat that
/// document's FIFO coalescing slot with a resurrected retry ahead of genuinely new real-time
/// updates.
///
/// Never revives `cancelled` rows: that status is `subscriber_gone`'s deliberate terminal state
/// (the subscription itself was deleted), not a retryable failure, and `events-v1.md` requires it
/// stays that way -- reviving a cancelled row would resurrect deliveries for a subscriber that no
/// longer exists.
///
/// Clears `terminated_at` back to `NULL`: the schema's own comment on that column says "只有
/// `requeue_failed` 可以清空它（复活即离开终态），其余路径不得改写" -- required for
/// `event_deliveries_terminated_at_check` (`status IN ('dispatched','failed','cancelled') =
/// (terminated_at IS NOT NULL)`) to still hold once `status` becomes `pending`.
///
/// `pub`: the v0.4 scope this module ships is the dispatcher primitive itself, proven against a
/// real database below; wiring it to an operator-facing REST/CLI surface is `events-v1.md`'s
/// fuller v0.8 `mode=requeue_failed` admin replay feature (`"v0.8 的 admin replay ... 全程不经过
/// event_dispatch"`), out of this task's scope.
pub async fn requeue_failed(db: &DatabaseConnection, workspace_id: Uuid, now: Timestamp) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET status = 'pending',
                    document_id = NULL,
                    terminated_at = NULL,
                    attempts = 0,
                    next_attempt_at = $2,
                    lease_token = NULL,
                    lease_expires_at = NULL,
                    last_error_code = NULL
                WHERE workspace_id = $1 AND status = 'failed'
            ",
            vec![workspace_id.into(), now.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMode {
    Rebuild,
    RequeueFailed,
}

#[derive(Debug, Clone)]
pub struct ReplayRequest {
    pub workspace_id: Uuid,
    pub mode: ReplayMode,
    pub event_type: Option<String>,
    pub subscriber_kind: Option<String>,
    pub subscriber_id: Option<Uuid>,
    pub from: Timestamp,
    pub to: Timestamp,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReplayWindow {
    pub from: Timestamp,
    pub to: Timestamp,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ReplayResult {
    Rebuild {
        replayed: u64,
        skipped_already_delivered: u64,
        rebuilt_delivery_ids: Vec<Uuid>,
        window: ReplayWindow,
    },
    RequeueFailed {
        requeued: u64,
        skipped_not_failed: u64,
        requeued_delivery_ids: Vec<Uuid>,
        window: ReplayWindow,
    },
}

#[derive(Debug, FromQueryResult)]
pub(super) struct ReplayCandidate {
    pub(super) event_id: Uuid,
    pub(super) subscriber_id: Uuid,
    pub(super) already_delivered: bool,
}

#[derive(Debug, FromQueryResult)]
pub(super) struct RequeueCandidate {
    pub(super) id: Uuid,
    pub(super) status: String,
}

pub(super) fn validate_replay_window(request: &ReplayRequest, now: Timestamp) -> Result<(), ApiError> {
    if request.from >= request.to || request.to > now {
        return Err(ApiError::BadRequest(
            "replay window must be non-empty, half-open, and not extend into the future".to_string(),
        ));
    }
    let oldest = now - time::Duration::days(REPLAY_MAX_WINDOW_DAYS);
    if request.from <= oldest {
        return Err(ApiError::BadRequest(
            "replay window is outside replay_max_window_days".to_string(),
        ));
    }
    if request.subscriber_kind.as_deref().is_some_and(|kind| kind != "webhook") {
        return Err(ApiError::BadRequest("unsupported subscriber_kind".to_string()));
    }
    Ok(())
}

/// Rebuilds missing webhook deliveries or revives failed deliveries inside one exact window.
///
/// `rebuild` filters source events by `business_events.created_at`, reserves the existing
/// `(subscriber, source_event)` dedup key before it creates a delivery, and binds both rows in one
/// transaction. `requeue_failed` instead filters `event_deliveries.terminated_at` and preserves
/// each delivery id. Dry-run executes the same candidate queries but performs zero writes.
pub async fn replay_deliveries(
    db: &DatabaseConnection,
    request: &ReplayRequest,
    now: Timestamp,
) -> Result<ReplayResult, ApiError> {
    validate_replay_window(request, now)?;
    let window = ReplayWindow {
        from: request.from,
        to: request.to,
    };
    match request.mode {
        ReplayMode::Rebuild => {
            let candidates = ReplayCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT be.id AS event_id, wh.id AS subscriber_id,
                           EXISTS (
                             SELECT 1 FROM event_delivery_sources src
                              WHERE src.subscriber_kind = 'webhook'
                                AND src.subscriber_id = wh.id
                                AND src.source_event_id = be.id
                           ) AS already_delivered
                      FROM business_events be
                      JOIN webhooks wh ON wh.workspace_id = be.workspace_id
                                      AND wh.active = true
                                      AND wh.events ? be.event_type
                     WHERE be.workspace_id = $1
                       AND be.created_at >= $2 AND be.created_at < $3
                       AND ($4::text IS NULL OR be.event_type = $4)
                       AND ($5::uuid IS NULL OR wh.id = $5)
                     ORDER BY be.created_at, be.id, wh.id
                ",
                vec![
                    request.workspace_id.into(),
                    request.from.into(),
                    request.to.into(),
                    request.event_type.clone().into(),
                    request.subscriber_id.into(),
                ],
            ))
            .all(db)
            .await?;
            #[cfg(test)]
            if let Ok(barrier) = TEST_REPLAY_CANDIDATE_BARRIER.try_with(Clone::clone) {
                barrier.wait().await;
            }
            let skipped = candidates.iter().filter(|row| row.already_delivered).count() as u64;
            let planned = candidates.len() as u64 - skipped;
            if request.dry_run {
                return Ok(ReplayResult::Rebuild {
                    replayed: planned,
                    skipped_already_delivered: skipped,
                    rebuilt_delivery_ids: Vec::new(),
                    window,
                });
            }

            let tx = db.begin().await?;
            let mut rebuilt = Vec::new();
            let mut concurrent_skips = 0_u64;
            for candidate in candidates.into_iter().filter(|row| !row.already_delivered) {
                let reserved = ReservedSourceRow::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "INSERT INTO event_delivery_sources \
                       (workspace_id, delivery_id, subscriber_kind, subscriber_id, source_event_id) \
                     VALUES ($1, NULL, 'webhook', $2, $3) \
                     ON CONFLICT (subscriber_kind, subscriber_id, source_event_id) DO NOTHING \
                     RETURNING id",
                    vec![
                        request.workspace_id.into(),
                        candidate.subscriber_id.into(),
                        candidate.event_id.into(),
                    ],
                ))
                .one(&tx)
                .await?;
                let Some(reserved) = reserved else {
                    concurrent_skips += 1;
                    continue;
                };
                let delivery_id = Uuid::new_v4();
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "INSERT INTO event_deliveries \
                       (id, dispatch_id, event_id, workspace_id, subscriber_kind, subscriber_id, \
                        document_id, status, next_attempt_at) \
                     VALUES ($1, NULL, $2, $3, 'webhook', $4, NULL, 'pending', $5)",
                    vec![
                        delivery_id.into(),
                        candidate.event_id.into(),
                        request.workspace_id.into(),
                        candidate.subscriber_id.into(),
                        now.into(),
                    ],
                ))
                .await?;
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE event_delivery_sources SET delivery_id = $1 WHERE id = $2",
                    vec![delivery_id.into(), reserved.id.into()],
                ))
                .await?;
                rebuilt.push(delivery_id);
            }
            tx.commit().await?;
            Ok(ReplayResult::Rebuild {
                replayed: rebuilt.len() as u64,
                skipped_already_delivered: skipped + concurrent_skips,
                rebuilt_delivery_ids: rebuilt,
                window,
            })
        }
        ReplayMode::RequeueFailed => {
            let candidates = RequeueCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT d.id, d.status
                      FROM event_deliveries d
                      JOIN business_events be ON be.id = d.event_id
                     WHERE d.workspace_id = $1
                       AND d.terminated_at >= $2 AND d.terminated_at < $3
                       AND ($4::text IS NULL OR be.event_type = $4)
                       AND ($5::uuid IS NULL OR d.subscriber_id = $5)
                     ORDER BY d.terminated_at, d.id
                ",
                vec![
                    request.workspace_id.into(),
                    request.from.into(),
                    request.to.into(),
                    request.event_type.clone().into(),
                    request.subscriber_id.into(),
                ],
            ))
            .all(db)
            .await?;
            let ids = candidates
                .iter()
                .filter(|row| row.status == "failed")
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let skipped = candidates.len() as u64 - ids.len() as u64;
            if !request.dry_run && !ids.is_empty() {
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE event_deliveries \
                        SET status='pending', document_id=NULL, terminated_at=NULL, attempts=0, \
                            next_attempt_at=$2, lease_token=NULL, lease_expires_at=NULL, last_error_code=NULL \
                      WHERE workspace_id=$1 AND status='failed' AND id = ANY($3)",
                    vec![request.workspace_id.into(), now.into(), ids.clone().into()],
                ))
                .await?;
            }
            Ok(ReplayResult::RequeueFailed {
                requeued: ids.len() as u64,
                skipped_not_failed: skipped,
                requeued_delivery_ids: if request.dry_run { Vec::new() } else { ids },
                window,
            })
        }
    }
}
