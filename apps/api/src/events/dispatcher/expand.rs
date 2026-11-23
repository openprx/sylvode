//! Expansion of `event_dispatch` work into deliveries.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use super::{
    COALESCED_SOURCE_EVENTS_MAX, CONTENT_DELIVERY_DEBOUNCE_MS, DISPATCH_LEASE_TTL_MS, dispatch_backoff_expr,
    ms_interval, step_b_fault_armed, workspace_subscriber_count, workspace_subscribers_limit_exceeded,
};
use crate::error::ApiError;
use crate::flow::collab::limits::SUBSCRIBERS_PER_WORKSPACE_MAX;

// ---------------------------------------------------------------------------------------------
// Expansion (`events-v1.md` "展开是一个事务" + "展开必须按 seq 有序")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub(super) enum ExpansionOutcome {
    Expanded,
    NoSubscribers,
    /// The final guarded `UPDATE` affected zero rows: this worker's lease was reclaimed mid-flight
    /// (`reclaim_expired_dispatch_leases` rotated the token) and another attempt now owns the row.
    /// The reservations this attempt made are safe leftovers (`event_delivery_sources`' `ON
    /// CONFLICT DO NOTHING` makes them idempotent for whoever expands next); nothing here is a
    /// failure worth counting or logging above `debug`.
    StaleLease,
}

#[derive(Debug, FromQueryResult)]
pub(super) struct DispatchLeaseRow {
    pub(super) id: Uuid,
    pub(super) event_id: Uuid,
    pub(super) workspace_id: Uuid,
    pub(super) event_type: String,
    pub(super) document_id: Option<Uuid>,
    pub(super) accepted_seq: Option<i64>,
}

/// Leases exactly one `event_dispatch` work item — the oldest-ready row that is either not a
/// content event, or *is* the current head of its document's queue (`NOT EXISTS` a smaller pending
/// `accepted_seq` on the same document) — and expands it. Non-head content rows are simply never
/// selected, which realizes `dispatch_head_wait_backoff_ms`'s "bounded wait, zero `attempts` cost"
/// requirement without a separate wait state: an unselected row is untouched and is reconsidered
/// on the dispatcher's next pass once its predecessor has left `pending`.
pub(super) async fn expand_one(db: &DatabaseConnection) -> Result<Option<ExpansionOutcome>, ApiError> {
    let lease_token = Uuid::new_v4().to_string();
    let leased = DispatchLeaseRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                UPDATE event_dispatch
                SET lease_token = $1, lease_expires_at = now() + {ttl}
                WHERE id = (
                    SELECT d.id
                    FROM event_dispatch d
                    WHERE d.status = 'pending' AND d.workspace_id IS NOT NULL
                      AND d.next_attempt_at <= now()
                      AND (d.lease_token IS NULL OR d.lease_expires_at < now())
                      AND (
                        d.document_id IS NULL
                        OR NOT EXISTS (
                            SELECT 1 FROM event_dispatch o
                            WHERE o.document_id = d.document_id
                              AND o.status = 'pending'
                              AND o.accepted_seq IS NOT NULL
                              AND o.accepted_seq < d.accepted_seq
                        )
                      )
                    ORDER BY d.next_attempt_at, d.id
                    LIMIT 1
                    FOR UPDATE SKIP LOCKED
                )
                RETURNING id, event_id, workspace_id, event_type, document_id, accepted_seq
            ",
            ttl = ms_interval(2)
        ),
        vec![lease_token.clone().into(), DISPATCH_LEASE_TTL_MS.into()],
    ))
    .one(db)
    .await?;

    let Some(work) = leased else { return Ok(None) };

    match expand_work(db, &work, &lease_token).await {
        Ok(outcome) => Ok(Some(outcome)),
        Err(err) => {
            record_expansion_failure(db, &work, &lease_token).await;
            Err(err)
        }
    }
}

/// Personal events use their real source scope. Existing fanout/delivery-source machinery is
/// reused with an explicitly selected recipient workspace, never by rewriting source ownership.
pub(super) async fn expand_independent_source(db: &DatabaseConnection) -> Result<Option<ExpansionOutcome>, ApiError> {
    #[derive(FromQueryResult)]
    struct Source {
        id: Uuid,
        event_id: Uuid,
        source_flow_scope_id: Uuid,
        event_type: String,
        document_id: Option<Uuid>,
        accepted_seq: Option<i64>,
        object_id: String,
        target_object_id: Option<String>,
    }
    let tx = db.begin().await?;
    let source=Source::find_by_statement(Statement::from_string(DbBackend::Postgres,
        "SELECT d.id,d.event_id,d.source_flow_scope_id,d.event_type,d.document_id,d.accepted_seq,COALESCE(e.payload->>'object_id',e.aggregate_id) AS object_id,e.payload->>'target_object_id' AS target_object_id FROM event_dispatch d JOIN business_events e ON e.id=d.event_id WHERE d.source_flow_scope_id IS NOT NULL AND d.status='pending' AND d.next_attempt_at<=now() AND NOT EXISTS(SELECT 1 FROM event_dispatch p WHERE p.document_id=d.document_id AND p.status='pending' AND p.accepted_seq<d.accepted_seq) ORDER BY d.created_at,d.id FOR UPDATE OF d SKIP LOCKED LIMIT 1".to_string())).one(&tx).await?;
    let Some(source) = source else {
        tx.rollback().await?;
        return Ok(None);
    };
    #[derive(FromQueryResult)]
    struct Audience {
        workspace_id: Uuid,
    }
    let audiences=Audience::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT DISTINCT m.destination_workspace_id AS workspace_id FROM flow_mounts m JOIN flow_objects o ON o.id=m.source_object_id WHERE o.scope_id=$1 AND o.id IN(SELECT object_id FROM flow_mount_acl_ancestors($2::uuid,$1)) ORDER BY m.destination_workspace_id",vec![source.source_flow_scope_id.into(),source.object_id.clone().into()])).all(&tx).await?;
    for audience in audiences {
        crate::flow::collab::authz::lock_epoch_for_share(&tx, audience.workspace_id).await?;
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_id FROM flow_scope_settings WHERE scope_id=$1 FOR SHARE",
        vec![source.source_flow_scope_id.into()],
    ))
    .await?;
    #[derive(FromQueryResult)]
    struct Recipient {
        id: Uuid,
        workspace_id: Uuid,
    }
    let recipients=Recipient::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT DISTINCT w.id,w.workspace_id FROM webhooks w JOIN flow_mounts m ON m.destination_workspace_id=w.workspace_id JOIN flow_objects o ON o.id=m.source_object_id WHERE o.scope_id=$1 AND o.id IN(SELECT object_id FROM flow_mount_acl_ancestors($2::uuid,$1)) AND w.active AND w.events ? $3 AND (o.lifecycle_status='active' OR $3 IN ('flow.object.archived','flow.object.restored')) AND ($3 NOT IN ('flow.relation.linked','flow.relation.unlinked') OR EXISTS(SELECT 1 FROM flow_mounts dependency JOIN flow_objects dep_object ON dep_object.id=dependency.source_object_id WHERE dependency.destination_workspace_id=w.workspace_id AND dep_object.scope_id=$1 AND dep_object.lifecycle_status='active' AND dep_object.id IN(SELECT object_id FROM flow_mount_acl_ancestors($4::uuid,$1)))) ORDER BY w.workspace_id,w.id LIMIT 101",vec![source.source_flow_scope_id.into(),source.object_id.into(),source.event_type.clone().into(),source.target_object_id.into()])).all(&tx).await?;
    if recipients.len() > 100 {
        return Err(workspace_subscribers_limit_exceeded(
            i64::try_from(recipients.len()).unwrap_or(i64::MAX),
        ));
    }
    for recipient in &recipients {
        let recipient_work = DispatchLeaseRow {
            id: source.id,
            event_id: source.event_id,
            workspace_id: recipient.workspace_id,
            event_type: source.event_type.clone(),
            document_id: source.document_id,
            accepted_seq: source.accepted_seq,
        };
        expand_one_subscriber(&tx, &recipient_work, recipient.id).await?;
    }
    let empty = recipients.is_empty();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE event_dispatch SET status=$2,expanded_at=now(),lease_token=NULL,lease_expires_at=NULL WHERE id=$1",
        vec![
            source.id.into(),
            if empty { "no_subscribers" } else { "expanded" }.into(),
        ],
    ))
    .await?;
    tx.commit().await?;
    Ok(Some(if empty {
        ExpansionOutcome::NoSubscribers
    } else {
        ExpansionOutcome::Expanded
    }))
}

pub(super) async fn record_expansion_failure(db: &DatabaseConnection, work: &DispatchLeaseRow, lease_token: &str) {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                r"
                    UPDATE event_dispatch
                    SET attempts = attempts + 1,
                        lease_token = NULL,
                        lease_expires_at = NULL,
                        status = CASE WHEN attempts + 1 >= max_attempts THEN 'failed' ELSE 'pending' END,
                        expanded_at = CASE WHEN attempts + 1 >= max_attempts THEN now() ELSE NULL END,
                        next_attempt_at = CASE
                            WHEN attempts + 1 >= max_attempts THEN next_attempt_at
                            ELSE now() + {backoff}
                        END,
                        last_error_code = CASE WHEN attempts + 1 >= max_attempts THEN 'expansion_failed' ELSE 'expansion_retry' END
                    WHERE id = $1 AND lease_token = $2
                ",
                backoff = dispatch_backoff_expr()
            ),
            vec![work.id.into(), lease_token.into()],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(dispatch_id = %work.id, error = %err, "dispatcher: recording expansion failure itself failed");
    }
}

#[derive(Debug, FromQueryResult)]
pub(super) struct SubscriberRow {
    pub(super) id: Uuid,
}

pub(super) async fn expand_work(
    db: &DatabaseConnection,
    work: &DispatchLeaseRow,
    lease_token: &str,
) -> Result<ExpansionOutcome, ApiError> {
    let tx = db.begin().await?;

    // `subscribers_per_workspace_max` = 100 (`limits-v1.md`, `limit_kind: workspace_subscribers`)
    // on the expansion path. The contract's row for this key describes it as the **dispatcher
    // fan-out amplification bound**: one domain transaction writes exactly one `event_dispatch`
    // row, and expanding it may produce at most this many `event_deliveries` rows.
    //
    // A workspace over the ceiling makes the expansion refuse rather than expand a truncated
    // subset: the contract's rejection ceilings say "达到上限即拒绝新增条目，不静默截断已有" — a
    // silently dropped subscriber would be exactly that silent truncation, and it would be
    // invisible (the delivery it should have got simply never exists). Refusing raises
    // `limit_exceeded{limit_kind:"workspace_subscribers"}`, which [`expand_one`] turns into the
    // ordinary expansion-failure path (`attempts + 1`, backoff, dead-letter at `max_attempts`
    // with `last_error_code`) — loud, bounded and operator-visible.
    //
    // [`ensure_workspace_subscriber_slot`] on the registration path is what keeps this unreachable
    // in practice: a workspace can only cross the ceiling through rows written before this ceiling
    // existed, or written around the API. This is a fixed contract anchor, deliberately *not* a
    // lease-budget computation — `limits-v1.md` records that its own `rule` cannot bind (expansion
    // would need ~39,700 subscribers to saturate `dispatch_lease_ttl_ms`).
    let subscriber_total = workspace_subscriber_count(&tx, work.workspace_id).await?;
    if subscriber_total > SUBSCRIBERS_PER_WORKSPACE_MAX {
        tx.rollback().await?;
        return Err(workspace_subscribers_limit_exceeded(subscriber_total));
    }

    // "active" subscribers, judged the same way `webhook_trigger.rs`'s existing fan-out judges
    // them (`events-v1.md` "订阅目录与 active 的判据": "与源码...WHERE active = true AND events ?
    // $2 同一判据，不另立标准"). v0.4's only `subscriber_kind` is `webhook`.
    //
    // `LIMIT` restates the same ceiling structurally, so the "at most
    // `subscribers_per_workspace_max` delivery rows per work item" invariant holds for this query
    // even if a subscriber were inserted between the count above and this select. The guard above
    // is what makes that limit unreachable rather than a silent truncation.
    let subscribers = SubscriberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webhooks WHERE workspace_id = $1 AND active = true AND events ? $2 \
         ORDER BY id LIMIT $3",
        vec![
            work.workspace_id.into(),
            work.event_type.clone().into(),
            SUBSCRIBERS_PER_WORKSPACE_MAX.into(),
        ],
    ))
    .all(&tx)
    .await?;

    if subscribers.is_empty() {
        let affected = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE event_dispatch SET status = 'no_subscribers', expanded_at = now(), \
                 lease_token = NULL, lease_expires_at = NULL WHERE id = $1 AND lease_token = $2",
                vec![work.id.into(), lease_token.into()],
            ))
            .await?
            .rows_affected();
        if affected == 0 {
            tx.rollback().await?;
            return Ok(ExpansionOutcome::StaleLease);
        }
        tx.commit().await?;
        return Ok(ExpansionOutcome::NoSubscribers);
    }

    for subscriber in &subscribers {
        expand_one_subscriber(&tx, work, subscriber.id).await?;
    }

    let affected = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE event_dispatch SET status = 'expanded', expanded_at = now(), \
             lease_token = NULL, lease_expires_at = NULL WHERE id = $1 AND lease_token = $2",
            vec![work.id.into(), lease_token.into()],
        ))
        .await?
        .rows_affected();

    if affected == 0 {
        tx.rollback().await?;
        return Ok(ExpansionOutcome::StaleLease);
    }
    tx.commit().await?;
    Ok(ExpansionOutcome::Expanded)
}

#[derive(Debug, FromQueryResult)]
pub(super) struct ReservedSourceRow {
    pub(super) id: Uuid,
}

/// One `(work, subscriber)` pair. Order frozen by `events-v1.md`: reserve the source row first
/// (`delivery_id=NULL`); a conflict there means this pair is already registered from a prior
/// expansion attempt, and the function returns without touching any delivery row at all — the
/// literal "冲突分支保持零 delivery 变化" requirement.
pub(super) async fn expand_one_subscriber<C: ConnectionTrait>(
    tx: &C,
    work: &DispatchLeaseRow,
    subscriber_id: Uuid,
) -> Result<(), ApiError> {
    let reserved = ReservedSourceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO event_delivery_sources (workspace_id, delivery_id, subscriber_kind, subscriber_id, source_event_id)
            VALUES ($1, NULL, 'webhook', $2, $3)
            ON CONFLICT (subscriber_kind, subscriber_id, source_event_id) DO NOTHING
            RETURNING id
        ",
        vec![work.workspace_id.into(), subscriber_id.into(), work.event_id.into()],
    ))
    .one(tx)
    .await?;

    let Some(reserved) = reserved else { return Ok(()) };

    if step_b_fault_armed() {
        // Test-only: step (a) above already ran on `tx`; returning `Err` here forces the whole
        // transaction to roll back without a `COMMIT`, exactly like a worker dying mid-expansion.
        return Err(ApiError::Conflict("test_injected_expansion_step_b_failure".to_string()));
    }

    let delivery_id = if let (Some(document_id), Some(accepted_seq)) = (work.document_id, work.accepted_seq) {
        bind_content_delivery(tx, work, subscriber_id, document_id, accepted_seq).await?
    } else {
        bind_plain_delivery(tx, work, subscriber_id).await?
    };

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE event_delivery_sources SET delivery_id = $1 WHERE id = $2",
        vec![delivery_id.into(), reserved.id.into()],
    ))
    .await?;

    Ok(())
}

/// Non-content event: exactly one `event_deliveries` row per `(dispatch_id, subscriber)`. The
/// `ON CONFLICT ... DO UPDATE` (rather than `DO NOTHING`) is the "re-expanding the same work must
/// not mint a new `delivery_id`" requirement — a `DO NOTHING` would return no row on the
/// (theoretical, since the source-table reservation above already de-duplicates this) re-expansion
/// path, leaving nothing to bind the source row to.
pub(super) async fn bind_plain_delivery<C: ConnectionTrait>(
    tx: &C,
    work: &DispatchLeaseRow,
    subscriber_id: Uuid,
) -> Result<Uuid, ApiError> {
    #[derive(FromQueryResult)]
    struct IdRow {
        id: Uuid,
    }
    let new_id = Uuid::new_v4();
    let row = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO event_deliveries (id, dispatch_id, event_id, workspace_id, subscriber_kind, subscriber_id, status, next_attempt_at)
            VALUES ($1, $2, $3, $4, 'webhook', $5, 'pending', now())
            ON CONFLICT (dispatch_id, subscriber_kind, subscriber_id)
            DO UPDATE SET dispatch_id = event_deliveries.dispatch_id
            RETURNING id
        ",
        vec![
            new_id.into(),
            work.id.into(),
            work.event_id.into(),
            work.workspace_id.into(),
            subscriber_id.into(),
        ],
    ))
    .one(tx)
    .await?
    .ok_or(ApiError::Internal)?;
    Ok(row.id)
}

#[derive(Debug, FromQueryResult)]
pub(super) struct PendingDeliveryRow {
    pub(super) id: Uuid,
}

#[derive(Debug, FromQueryResult)]
pub(super) struct CountRow {
    pub(super) count: i64,
}

/// `flow.content.accepted`: merges into the document's current `pending` delivery for this
/// subscriber when one exists and has room (`coalesced_source_events_max`), otherwise seals that
/// row and opens a fresh one. Safe to call without extra cross-document locking: the head-of-queue
/// rule in [`expand_one`] guarantees at most one `event_dispatch` work item per document is ever
/// being expanded at a time, so there is never a concurrent writer for this `(subscriber,
/// document)` pending row.
pub(super) async fn bind_content_delivery<C: ConnectionTrait>(
    tx: &C,
    work: &DispatchLeaseRow,
    subscriber_id: Uuid,
    document_id: Uuid,
    accepted_seq: i64,
) -> Result<Uuid, ApiError> {
    let existing = PendingDeliveryRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id FROM event_deliveries
            WHERE subscriber_kind = 'webhook' AND subscriber_id = $1 AND document_id = $2 AND status = 'pending'
            FOR UPDATE
        ",
        vec![subscriber_id.into(), document_id.into()],
    ))
    .one(tx)
    .await?;

    if let Some(existing) = existing {
        let count = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*) AS count FROM event_delivery_sources WHERE delivery_id = $1",
            vec![existing.id.into()],
        ))
        .one(tx)
        .await?
        .map_or(0, |row| row.count);

        if count < COALESCED_SOURCE_EVENTS_MAX {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    r"
                        UPDATE event_deliveries
                        SET latest_seq = GREATEST(latest_seq, $2), next_attempt_at = now() + {debounce}
                        WHERE id = $1
                    ",
                    debounce = ms_interval(3)
                ),
                vec![
                    existing.id.into(),
                    accepted_seq.into(),
                    CONTENT_DELIVERY_DEBOUNCE_MS.into(),
                ],
            ))
            .await?;
            return Ok(existing.id);
        }

        // Cap reached: freeze the row (guarded, so a racing lease-claim mid-transition cannot be
        // clobbered back to `pending`) and fall through to open a new one below.
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE event_deliveries SET status = 'sealed' WHERE id = $1 AND status = 'pending'",
            vec![existing.id.into()],
        ))
        .await?;
    }

    let new_id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                INSERT INTO event_deliveries
                    (id, dispatch_id, event_id, workspace_id, subscriber_kind, subscriber_id, document_id,
                     status, first_seq, latest_seq, next_attempt_at)
                VALUES ($1, $2, $3, $4, 'webhook', $5, $6, 'pending', $7, $7, now() + {debounce})
            ",
            debounce = ms_interval(8)
        ),
        vec![
            new_id.into(),
            work.id.into(),
            work.event_id.into(),
            work.workspace_id.into(),
            subscriber_id.into(),
            document_id.into(),
            accepted_seq.into(),
            CONTENT_DELIVERY_DEBOUNCE_MS.into(),
        ],
    ))
    .await?;
    Ok(new_id)
}
