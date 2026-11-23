//! Retention reapers.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::{
    DELIVERY_RETENTION_DAYS, DELIVERY_SOURCE_RETENTION_DAYS, DISPATCH_EXPANDED_RETENTION_DAYS,
    DISPATCH_FAILED_RETENTION_DAYS, DISPATCH_NO_SUBSCRIBERS_RETENTION_HOURS,
};
use crate::error::ApiError;

// ---------------------------------------------------------------------------------------------
// Retention reapers (`events-v1.md` "reaper 的谓词只能删终态" / "计时锚点")
// ---------------------------------------------------------------------------------------------

/// Takes `now` as an explicit argument (rather than letting the `DELETE` read Postgres's own
/// `now()`) so callers — in particular exact-boundary tests — can pin the *one* reference instant
/// used both to compute a backdated anchor column and to evaluate this predicate against it. Two
/// independent `now()` calls (one in the test's `UPDATE`, one in this `DELETE`) would always have
/// drifted apart by at least the round-trip between them, which makes "exactly at the boundary"
/// unobservable. `run_tick` passes real wall-clock time; nothing else changes for production use.
pub(super) async fn reap_dispatch_retention(db: &DatabaseConnection, now: Timestamp) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_dispatch
                WHERE lease_token IS NULL
                  AND (
                    (status = 'no_subscribers' AND expanded_at < $4::timestamptz - ($1::bigint * interval '1 hour'))
                    OR (status = 'expanded' AND expanded_at < $4::timestamptz - ($2::bigint * interval '1 day'))
                    OR (status = 'failed' AND expanded_at < $4::timestamptz - ($3::bigint * interval '1 day'))
                  )
            ",
            vec![
                DISPATCH_NO_SUBSCRIBERS_RETENTION_HOURS.into(),
                DISPATCH_EXPANDED_RETENTION_DAYS.into(),
                DISPATCH_FAILED_RETENTION_DAYS.into(),
                now.into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// See [`reap_dispatch_retention`]'s doc comment for why `now` is an explicit argument.
pub(super) async fn reap_delivery_retention(db: &DatabaseConnection, now: Timestamp) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_deliveries
                WHERE lease_token IS NULL
                  AND status IN ('dispatched', 'failed', 'cancelled')
                  AND terminated_at < $2::timestamptz - ($1::bigint * interval '1 day')
            ",
            vec![DELIVERY_RETENTION_DAYS.into(), now.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// `event_delivery_sources` has no `status`/`lease_token` (`events-v1.md` "reaper 的谓词只能删
/// 终态": "`event_delivery_sources` 无 status，用它自己的 `delivery_id IS NULL` 谓词"), so its
/// reaper's predicate is not the `WHERE status IN (...) AND <anchor> < now() - retention` shape
/// [`reap_dispatch_retention`]/[`reap_delivery_retention`] share -- it is the tombstone-specific
/// one the contract writes out verbatim: `WHERE created_at < now() - delivery_source_retention_days
/// AND delivery_id IS NULL`. A row whose `delivery_id` is still non-NULL is not a tombstone yet --
/// the delivery it names has not been retention-reaped -- and deleting it here would destroy the
/// `(subscriber_kind, subscriber_id, source_event_id)` dedup evidence a still-live delivery
/// depends on, so `delivery_id IS NULL` is not an optimization, it is the entire safety property.
pub(super) async fn reap_delivery_source_tombstones(db: &DatabaseConnection, now: Timestamp) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_delivery_sources
                WHERE delivery_id IS NULL
                  AND created_at < $2::timestamptz - ($1::bigint * interval '1 day')
            ",
            vec![DELIVERY_SOURCE_RETENTION_DAYS.into(), now.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}
