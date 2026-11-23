//! `ADR-0011` dispatcher.
//!
//! Single-row `event_dispatch` work, expanded into `event_deliveries` + `event_delivery_sources`
//! post-commit and *outside* the domain transaction that wrote the work row, then delivered to
//! subscribed webhooks.
//!
//! `apps/worker` calls [`run_tick`] on its existing 5-second poll loop (`--concurrency` is a batch
//! multiplier, not parallelism, matching every other job this worker already runs — see
//! `apps/worker/src/main.rs`'s `process_pending_tasks`). All three tables this module touches are
//! platform tables Flow is merely the v0.4 producer/consumer of (`migrations/0054_flow_data_layer.sql`
//! "`event_dispatch` / `event_deliveries` / `event_delivery_sources`"); this module — not `crate::flow` —
//! owns them because the next non-Flow producer that starts writing `event_dispatch` rows reuses
//! this dispatcher unchanged.
//!
//! `contracts/events-v1.md` ("投递") is the schema/algorithm's sole normative source; this file
//! implements its "展开是一个事务" ordering literally: reserve the source row (`delivery_id=NULL`)
//! → lock/select or create the delivery → bind the source to it → advance `event_dispatch`, all in
//! one transaction, with the final advance guarded by `WHERE lease_token = ?` so a worker whose
//! lease already expired cannot mark a re-leased work item `expanded` out from under its new owner.
#![allow(
    clippy::items_after_statements,
    clippy::struct_field_names,
    clippy::too_many_arguments
)]

use std::sync::atomic::{AtomicI64, Ordering};

use platform::app::AppState;
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::flow::collab::limits::SUBSCRIBERS_PER_WORKSPACE_MAX;

// ---------------------------------------------------------------------------------------------
// Test-only fault injection (`events-v1.md` "展开是一个事务": the only way to prove step (b)'s
// failure rolls back the *whole* transaction — including the step (a) reservation that already
// ran on the same `tx` — is to make step (b) actually fail after (a) has already succeeded.
// Gated entirely behind `cfg(test)`: the task-local key does not exist, and this call site is a
// zero-cost `false` literal, in any non-test build (including `apps/worker`, which links this
// crate as a dependency and therefore never sees this crate's own `cfg(test)`).
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
tokio::task_local! {
    static FAIL_EXPANSION_STEP_B: std::cell::Cell<bool>;
}

#[cfg(test)]
fn step_b_fault_armed() -> bool {
    FAIL_EXPANSION_STEP_B.try_with(std::cell::Cell::get).unwrap_or(false)
}

#[cfg(not(test))]
const fn step_b_fault_armed() -> bool {
    false
}

// ---------------------------------------------------------------------------------------------
// Budgets `limits-v1.md` marks `status: unset` ("`set_by`: v0.4 实现者", "冻结前不得填入自造数值"
// for anything invented, but every value below still follows that entry's own `rule`). None of
// these back a database column default, unlike `FlowConfig::dispatch_max_attempts`
// (`crate::config::runtime()`'s doc comment explains why that one *is* deployment configuration);
// they are pure dispatcher-internal constants, so they live here rather than in `[flow]`.
// ---------------------------------------------------------------------------------------------

/// `dispatch_max_lease_reclaims`: worker-crash reclaims a work item survives before `failed`.
/// Rule: "显著大于一次发布窗口内的预期重启次数，同时使病态回收循环有界" — a rolling deploy
/// reclaims a work item at most once or twice; 5 leaves headroom without letting a poison work
/// item spin indefinitely.
const DISPATCH_MAX_LEASE_RECLAIMS: i64 = 5;

/// `dispatch_lease_ttl_ms`: expansion is a pure DB operation with a predictable p99, so this only
/// needs to be "several multiples" of that (rule) — 30s is generous for a single small
/// transaction and still short enough that a genuinely crashed worker's head-of-queue item is
/// freed well within one polling cycle.
const DISPATCH_LEASE_TTL_MS: i64 = 30_000;

/// `dispatch_backoff_ms`: expansion-failure retry ladder. Rule: "形态对齐 `delivery_backoff_ms`，
/// 但必须是独立的值" — same shape (linear, capped) as the frozen `delivery_backoff_ms`, half its
/// magnitude since expansion failures are DB errors, not network waits.
const DISPATCH_BACKOFF_STEP_MS: i64 = 15_000;
const DISPATCH_BACKOFF_CAP_MS: i64 = 150_000;

/// `delivery_max_attempts` (frozen at 10 by `limits-v1.md`) already lives as the `event_deliveries
/// .max_attempts` column default; this mirrors it only for the lease-reclaim path, which reads the
/// row's own `max_attempts` rather than this constant — kept here purely as the doc anchor.
const DELIVERY_BACKOFF_STEP_MS: i64 = 30_000;
const DELIVERY_BACKOFF_CAP_MS: i64 = 300_000;

fn delivery_backoff_ms(attempts: i64) -> i64 {
    (attempts * DELIVERY_BACKOFF_STEP_MS).min(DELIVERY_BACKOFF_CAP_MS)
}

/// `webhook_request_timeout_ms`. Matches the worker's existing outbound `reqwest::Client` (built
/// in `apps/worker/src/main.rs` with `timeout(10s)`) so this dispatcher's requests are governed by
/// the same budget as the AI-task webhook dispatch that client already serves.
const WEBHOOK_REQUEST_TIMEOUT_MS: i64 = 10_000;

/// `delivery_lease_ttl_ms`. Rule: "≥ webhook 请求超时 + 一个安全余量" — three times the request
/// timeout comfortably covers TCP/TLS setup plus the request itself before a still-in-flight send
/// is mistaken for a dead worker and reclaimed into a concurrent second send.
const DELIVERY_LEASE_TTL_MS: i64 = WEBHOOK_REQUEST_TIMEOUT_MS * 3;

/// `content_delivery_debounce_ms`, frozen at 2,000 by `limits-v1.md`.
const CONTENT_DELIVERY_DEBOUNCE_MS: i64 = 2_000;

/// `coalesced_source_events_max`, frozen at 20 by `limits-v1.md`. Rule: "取 debounce 窗口内单文档
/// 合并事件数的 p99 上界" — the structured block derives it from two values that are themselves
/// frozen, `content_delivery_debounce_ms=2000` and the per-connection `updates_per_connection_
/// _per_second=10`: at most 10 x 2 = 20 accepted updates can land in one 2-second debounce window,
/// so 20 *is* that p99 upper bound.
///
/// This constant used to be 64 "for 3x headroom", where the 64 was borrowed from
/// `flow::collab::limits`'s warm-cache entry count — a number from another domain, which is
/// exactly the invented value `limits-v1.md` forbids. It is also not free: because the debounce is
/// *trailing*, at the frozen 10 updates/s the cap itself is what closes the window, so the cap is
/// paid directly in content-delivery latency (measured 8317 ms at 64, about 4.0 s at 20) and buys
/// only a larger full-window body (9216 B at 20 vs. 10932 B at 64).
const COALESCED_SOURCE_EVENTS_MAX: i64 = 20;

/// The `limit_kind` `limits-v1.md` freezes for [`SUBSCRIBERS_PER_WORKSPACE_MAX`]; every rejection
/// this module raises for that ceiling reports it verbatim.
pub const WORKSPACE_SUBSCRIBERS_LIMIT_KIND: &str = "workspace_subscribers";

/// Active subscribers in one workspace, judged by exactly the predicate [`expand_work`] and
/// `webhook_trigger.rs` already use for "active" (`events-v1.md` "订阅目录与 active 的判据":
/// "与源码...`WHERE active = true` 同一判据，不另立标准"). v0.4's only `subscriber_kind` is
/// `webhook`, so this is the workspace's whole subscriber directory.
///
/// Counted without the `events ? type` filter on purpose: `subscribers_per_workspace_max` bounds
/// the workspace's *subscriber total*, not the subset that happens to match one event type.
pub async fn workspace_subscriber_count<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<i64, ApiError> {
    workspace_subscriber_count_excluding(conn, workspace_id, None).await
}

/// [`workspace_subscriber_count`] with one subscriber left out of the tally — the row an update is
/// about to switch on, which must not be counted both as "already there" and as "the one being
/// added". `NULL` excludes nothing.
async fn workspace_subscriber_count_excluding<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    excluded: Option<Uuid>,
) -> Result<i64, ApiError> {
    Ok(CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*) AS count FROM webhooks \
         WHERE workspace_id = $1 AND active = true AND ($2::uuid IS NULL OR id <> $2)",
        vec![workspace_id.into(), excluded.into()],
    ))
    .one(conn)
    .await?
    .map_or(0, |row| row.count))
}

/// The one place a `subscribers_per_workspace_max` rejection is built, so the registration path and
/// the expansion path cannot report the ceiling differently. Carries the frozen `limit_kind` plus
/// the numeric `limit`/`observed` that `error-mapping-v1.md` defines for `limit_exceeded`.
fn workspace_subscribers_limit_exceeded(observed: i64) -> ApiError {
    ApiError::limit_exceeded(
        format!(
            "limit_exceeded: {WORKSPACE_SUBSCRIBERS_LIMIT_KIND} (at most {SUBSCRIBERS_PER_WORKSPACE_MAX} active subscribers per workspace)"
        ),
        WORKSPACE_SUBSCRIBERS_LIMIT_KIND,
        Some(json!(SUBSCRIBERS_PER_WORKSPACE_MAX)),
        Some(json!(observed)),
        None,
    )
}

/// Registration ceiling for `subscribers_per_workspace_max`: called before a workspace gains one
/// more *active* subscriber (a created webhook, or an existing one being switched back on).
///
/// This is the "拒绝 ceiling" half of the contract row — the exact ceiling is accepted, the one
/// past it is refused with `limit_exceeded{limit_kind:"workspace_subscribers"}`, and nothing
/// already registered is silently dropped (the same shape `limits-v1.md` spells out for
/// `object_grants_max`: "达到上限即拒绝新增条目，不静默截断已有授予"). It is *not* the delivery-budget
/// shape of `coalesced_source_events_max`, which raises no `limit_exceeded` at all: this key sits
/// in the contract's authorization/subscription table, and `workspace_subscribers` enters the
/// `limit_kind` universe (and therefore `error_kind_coverage.expected[]`) the moment it is frozen.
/// `becoming_active` names an existing subscriber row that this write is switching on, so it is
/// excluded from the "already there" tally and counted once, as the slot being taken; pass `None`
/// when a brand-new row is being inserted. An update that leaves an already-active subscriber
/// active therefore stays accepted even in a workspace sitting exactly at the ceiling.
pub async fn ensure_workspace_subscriber_slot<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    becoming_active: Option<Uuid>,
) -> Result<(), ApiError> {
    let current = workspace_subscriber_count_excluding(conn, workspace_id, becoming_active).await?;
    if current + 1 > SUBSCRIBERS_PER_WORKSPACE_MAX {
        return Err(workspace_subscribers_limit_exceeded(current + 1));
    }
    Ok(())
}

/// `changed_block_ids_per_delivery_max`. Rule: "使投递体在 p99 合并窗口下仍显著小于订阅端常见
/// 请求体上限" — 200 UUIDs is ~7KB of JSON, well under typical webhook body limits even stacked
/// with the rest of the envelope.
const CHANGED_BLOCK_IDS_PER_DELIVERY_MAX: usize = 200;

/// `delivery_retention_days`, frozen at 30 by `limits-v1.md`.
const DELIVERY_RETENTION_DAYS: i64 = 30;

/// `dispatch_no_subscribers_retention_hours`, frozen at 24 by `limits-v1.md`.
const DISPATCH_NO_SUBSCRIBERS_RETENTION_HOURS: i64 = 24;

/// `dispatch_expanded_retention_days`. Rule: "足以支撑事后排障回溯，且使队首查询所在表规模有界" —
/// two weeks covers a typical incident-investigation window.
const DISPATCH_EXPANDED_RETENTION_DAYS: i64 = 14;

/// `dispatch_failed_retention_days`. Rule: "必须显著长于 `dispatch_expanded_retention_days`" and
/// "不短于一个值班轮换周期" — a quarter is both, and dead-letter rows are exactly the ones an
/// operator needs to still find weeks after the fact.
const DISPATCH_FAILED_RETENTION_DAYS: i64 = 90;

/// `delivery_source_retention_days` (`limits-v1.md`, `status: unset`): how long an
/// `event_delivery_sources` tombstone (`delivery_id IS NULL`, left behind once
/// [`reap_delivery_retention`] deletes the delivery row it named) survives before this file's own
/// tombstone reaper deletes it too. Rule: "取 replay 窗口上限并留一个清理周期的余量", and "必须严格
/// 大于 `replay_max_window_days`" and "必须 ≥ `delivery_retention_days`" -- v0.4 has no replay
/// feature yet (that lands v0.8, per `events-v1.md` "Replay 与 `event_delivery_sources`"), so
/// there is no frozen `replay_max_window_days` to measure against; matching
/// `DISPATCH_FAILED_RETENTION_DAYS`'s own "one duty-rotation quarter" reasoning both satisfies the
/// documented `>= delivery_retention_days` floor with 3x headroom and leaves an operator a full
/// quarter to look up what a dead-lettered delivery covered before its dedup evidence disappears.
const DELIVERY_SOURCE_RETENTION_DAYS: i64 = 90;

/// `replay_max_window_days`, proposed for the v0.8 reviewed budget lock.
///
/// Sixty days keeps replay strictly inside the independently retained 90-day source tombstone
/// window and leaves one full `delivery_retention_days` cleanup cycle of safety margin.  The
/// contract remains the authority: this value must be copied into its reviewed budget artifact
/// before an official candidate can pass.
pub const REPLAY_MAX_WINDOW_DAYS: i64 = 30;

/// Header carrying the immutable consumer dedup key (`events-v1.md` "投递报文与 `delivery_id` 的
/// 位置"). `delivery.id` in the body is the same value; both are written together below.
const DELIVERY_ID_HEADER: &str = "X-Sylvode-Delivery-Id";

/// `dispatcher_liveness` readiness budget: how stale the last started tick may be before
/// [`dispatcher_is_live`] reports the process not-live.
///
/// `limits-v1.md` freezes no numeric value for this signal (it only names the mechanism), so this
/// follows the same "several multiples of the expected cadence" rule the rest of this file's
/// `status: unset` budgets use — six times the worker's 5-second poll interval
/// (`apps/worker/src/main.rs`) tolerates one slow tick without flapping, while still catching a
/// genuinely wedged poll loop within half a minute.
pub const DISPATCHER_LIVENESS_MAX_SILENCE_MS: i64 = 30_000;

/// `oldest_pending_age` alert threshold.
///
/// Several multiples of the slowest *legitimate* resolution path in this file (FIFO head-of-queue
/// wait plus the delivery backoff ladder's 300s cap), so this only fires once a backlog item has
/// genuinely stalled rather than while it is merely waiting its turn behind a healthy predecessor.
pub const OLDEST_PENDING_AGE_ALERT_MS: i64 = 15 * 60 * 1000;

fn ms_interval(param_index: usize) -> String {
    format!("(${param_index}::bigint * interval '1 millisecond')")
}

/// The `dispatch_backoff_ms` ladder as a SQL expression over the row's own `attempts` column,
/// avoiding a `format!` nested inside another `format!`'s arguments.
fn dispatch_backoff_expr() -> String {
    format!(
        "(LEAST((attempts + 1) * {DISPATCH_BACKOFF_STEP_MS}::bigint, {DISPATCH_BACKOFF_CAP_MS}::bigint) * interval '1 millisecond')"
    )
}

// ---------------------------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------------------------

/// Counters from one dispatcher pass.
///
/// Reclaims expired leases, expands up to `batch` `event_dispatch` work items, sends up to
/// `batch` `event_deliveries`, then runs the retention reapers. Every sub-step logs and continues
/// on error rather than aborting the tick — matching `apps/worker/src/main.rs`'s existing
/// tolerance for one failing job not blocking the others on the same poll.
#[derive(Debug, Clone, Copy, Default)]
pub struct DispatchTickReport {
    pub expanded: u64,
    pub no_subscribers: u64,
    pub delivered: u64,
    pub delivery_retried_or_failed: u64,
    pub dispatch_leases_reclaimed: u64,
    pub delivery_leases_reclaimed: u64,
    pub dispatch_rows_reaped: u64,
    pub delivery_rows_reaped: u64,
    /// `event_delivery_sources` tombstone reaper: rows deleted by the `delivery_id IS NULL`
    /// predicate (`events-v1.md` "来源表的保留期与外键语义").
    pub delivery_source_tombstones_reaped: u64,
    /// `oldest_pending_age` (dispatch half): age in ms of the oldest still-`pending`
    /// `event_dispatch` row after this pass, `None` when that backlog is empty. Anchored on
    /// `created_at`, the only timestamp such a row has before it resolves.
    pub oldest_pending_dispatch_age_ms: Option<i64>,
    /// `oldest_pending_age` (delivery half): age in ms of the oldest still-undelivered
    /// `event_deliveries` row (`pending`/`sealed`/`leased`) after this pass, `None` when empty.
    pub oldest_pending_delivery_age_ms: Option<i64>,
}

/// `dispatcher_liveness` readiness probe: `true` iff this process has *started* a `run_tick` pass
/// within `max_silence_ms` of `now`.
///
/// `false` before the first tick this process has ever run, or once the gap since the last started
/// tick exceeds the budget — either one is exactly the signal `ADR-0011`'s "dispatcher 不在线即整体
/// 告警" needs a caller (e.g. a `/readyz` route) to alert on.
pub fn dispatcher_is_live(now: Timestamp, max_silence_ms: i64) -> bool {
    let last = LAST_TICK_STARTED_AT_UNIX_MS.load(Ordering::Relaxed);
    let last = if last == 0 {
        None
    } else {
        Timestamp::from_unix_timestamp_millis(last)
    };
    dispatcher_is_live_since(last, now, max_silence_ms)
}

/// The pure core of [`dispatcher_is_live`], taking "when did the last tick start" as an explicit
/// argument instead of reading the process-global clock.
///
/// Exists so tests can exercise arbitrary `(last_tick, now)` pairs — including "no tick has ever
/// happened" — by advancing a virtual clock through the argument, without racing every *other* test
/// in the same binary that calls [`run_tick`] and therefore mutates the one shared global this
/// function's public wrapper reads.
pub fn dispatcher_is_live_since(last_tick_started_at: Option<Timestamp>, now: Timestamp, max_silence_ms: i64) -> bool {
    last_tick_started_at.is_some_and(|last| (now - last).whole_milliseconds() <= i128::from(max_silence_ms))
}

/// `oldest_pending_age` threshold check over a completed tick's report: `true` iff either half of
/// the backlog (dispatch or delivery) is older than `threshold_ms`. A caller alerts on `true`.
pub fn backlog_alert(report: &DispatchTickReport, threshold_ms: i64) -> bool {
    report
        .oldest_pending_dispatch_age_ms
        .is_some_and(|age| age > threshold_ms)
        || report
            .oldest_pending_delivery_age_ms
            .is_some_and(|age| age > threshold_ms)
}

/// Unix milliseconds of the last time [`run_tick`] *started* a pass in this process; `0` before
/// the first ever tick. No persistence by design: a fresh process is correctly not-live until its
/// own first tick, and a process whose poll loop wedges is correctly reported stale without any
/// other process needing to notice.
static LAST_TICK_STARTED_AT_UNIX_MS: AtomicI64 = AtomicI64::new(0);

pub async fn run_tick(state: &AppState, client: &reqwest::Client, batch: usize) -> DispatchTickReport {
    let batch = batch.max(1);
    let now = Timestamp::now();
    LAST_TICK_STARTED_AT_UNIX_MS.store(
        i64::try_from(now.unix_timestamp_millis()).unwrap_or(i64::MAX),
        Ordering::Relaxed,
    );
    let mut report = DispatchTickReport::default();
    for _ in 0..batch {
        match expand_independent_source(&state.db).await {
            Ok(Some(ExpansionOutcome::Expanded)) => report.expanded += 1,
            Ok(Some(ExpansionOutcome::NoSubscribers)) => report.no_subscribers += 1,
            Ok(Some(ExpansionOutcome::StaleLease)) => {}
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(%err,"dispatcher: independent source expansion failed");
                break;
            }
        }
    }

    match reclaim_expired_dispatch_leases(&state.db).await {
        Ok(n) => report.dispatch_leases_reclaimed = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_dispatch lease reclaim failed"),
    }
    match reclaim_expired_delivery_leases(&state.db).await {
        Ok(n) => report.delivery_leases_reclaimed = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_deliveries lease reclaim failed"),
    }

    for _ in 0..batch {
        match expand_one(&state.db).await {
            Ok(Some(ExpansionOutcome::Expanded)) => report.expanded += 1,
            Ok(Some(ExpansionOutcome::NoSubscribers)) => report.no_subscribers += 1,
            Ok(Some(ExpansionOutcome::StaleLease)) => {}
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "dispatcher: event_dispatch expansion failed");
                break;
            }
        }
    }

    for _ in 0..batch {
        match send_one(state, client).await {
            Ok(Some(true)) => report.delivered += 1,
            Ok(Some(false)) => report.delivery_retried_or_failed += 1,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "dispatcher: event_deliveries send failed");
                break;
            }
        }
    }

    match reap_dispatch_retention(&state.db, now).await {
        Ok(n) => report.dispatch_rows_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_dispatch retention reaper failed"),
    }
    match reap_delivery_retention(&state.db, now).await {
        Ok(n) => report.delivery_rows_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_deliveries retention reaper failed"),
    }
    match reap_delivery_source_tombstones(&state.db, now).await {
        Ok(n) => report.delivery_source_tombstones_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_delivery_sources tombstone reaper failed"),
    }

    match oldest_pending_dispatch_age_ms(&state.db, now).await {
        Ok(age) => report.oldest_pending_dispatch_age_ms = age,
        Err(err) => tracing::warn!(error = %err, "dispatcher: oldest_pending_dispatch_age query failed"),
    }
    match oldest_pending_delivery_age_ms(&state.db, now).await {
        Ok(age) => report.oldest_pending_delivery_age_ms = age,
        Err(err) => tracing::warn!(error = %err, "dispatcher: oldest_pending_delivery_age query failed"),
    }

    report
}

#[derive(Debug, FromQueryResult)]
struct OldestPendingRow {
    oldest_created_at: Option<Timestamp>,
}

/// `oldest_pending_age`, dispatch half: age of the oldest still-unresolved (`status = 'pending'`)
/// `event_dispatch` row. `None` when there is no such row.
async fn oldest_pending_dispatch_age_ms(db: &DatabaseConnection, now: Timestamp) -> Result<Option<i64>, ApiError> {
    let row = OldestPendingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT MIN(created_at) AS oldest_created_at FROM event_dispatch WHERE status = 'pending'",
        vec![],
    ))
    .one(db)
    .await?;
    Ok(row
        .and_then(|r| r.oldest_created_at)
        .map(|oldest| i64::try_from((now - oldest).whole_milliseconds().max(0)).unwrap_or(i64::MAX)))
}

/// `oldest_pending_age`, delivery half: age of the oldest still-undelivered `event_deliveries` row
/// (`pending`/`sealed`/`leased` — everything short of a terminal state counts as "未投递"). `None`
/// when there is no such row.
async fn oldest_pending_delivery_age_ms(db: &DatabaseConnection, now: Timestamp) -> Result<Option<i64>, ApiError> {
    let row = OldestPendingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT MIN(created_at) AS oldest_created_at FROM event_deliveries WHERE status IN ('pending', 'sealed', 'leased')",
        vec![],
    ))
    .one(db)
    .await?;
    Ok(row
        .and_then(|r| r.oldest_created_at)
        .map(|oldest| i64::try_from((now - oldest).whole_milliseconds().max(0)).unwrap_or(i64::MAX)))
}

// ---------------------------------------------------------------------------------------------
// Lease reclaim (`events-v1.md` "leased 行的回收必须有独立的 reaper" / dispatch's dual guard)
// ---------------------------------------------------------------------------------------------

/// Rotates the lease on any `event_dispatch` row whose owner disappeared. Reclaiming does **not**
/// count toward `attempts` (expansion is deterministic, so a vanished worker is never a "poison
/// payload"); it counts toward the separate `lease_reclaims` budget, which — once exhausted — puts
/// the row into `failed` so a permanently crash-looping owner cannot wedge that document's queue
/// head forever.
async fn reclaim_expired_dispatch_leases(db: &DatabaseConnection) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                WITH candidates AS (
                    SELECT id FROM event_dispatch
                    WHERE status = 'pending' AND lease_token IS NOT NULL AND lease_expires_at < now()
                    FOR UPDATE SKIP LOCKED
                )
                UPDATE event_dispatch d
                SET lease_token = NULL,
                    lease_expires_at = NULL,
                    lease_reclaims = d.lease_reclaims + 1,
                    status = CASE WHEN d.lease_reclaims + 1 >= $1 THEN 'failed' ELSE 'pending' END,
                    expanded_at = CASE WHEN d.lease_reclaims + 1 >= $1 THEN now() ELSE NULL END,
                    last_error_code = CASE WHEN d.lease_reclaims + 1 >= $1 THEN 'lease_reclaims_exhausted' ELSE NULL END
                FROM candidates c
                WHERE d.id = c.id
            ",
            vec![DISPATCH_MAX_LEASE_RECLAIMS.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// Same idea for `event_deliveries`, with the "回 `sealed`, 绝不回 `pending`" exception for replay
/// rows (`document_id IS NULL`) `events-v1.md` freezes.
async fn reclaim_expired_delivery_leases(db: &DatabaseConnection) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                WITH candidates AS (
                    SELECT id FROM event_deliveries
                    WHERE status = 'leased' AND lease_expires_at < now()
                    FOR UPDATE SKIP LOCKED
                )
                UPDATE event_deliveries d
                SET lease_token = NULL,
                    lease_expires_at = NULL,
                    attempts = d.attempts + 1,
                    status = CASE
                        WHEN d.attempts + 1 >= d.max_attempts THEN 'failed'
                        WHEN d.document_id IS NULL THEN 'pending'
                        ELSE 'sealed'
                    END,
                    terminated_at = CASE WHEN d.attempts + 1 >= d.max_attempts THEN now() ELSE NULL END,
                    next_attempt_at = CASE
                        WHEN d.attempts + 1 >= d.max_attempts THEN d.next_attempt_at
                        ELSE now() + (LEAST((d.attempts + 1) * $1::bigint, $2::bigint) * interval '1 millisecond')
                    END,
                    last_error_code = CASE
                        WHEN d.attempts + 1 >= d.max_attempts THEN 'lease_expired_retries_exhausted'
                        ELSE d.last_error_code
                    END
                FROM candidates c
                WHERE d.id = c.id
            ",
            vec![DELIVERY_BACKOFF_STEP_MS.into(), DELIVERY_BACKOFF_CAP_MS.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

mod deliver;
mod expand;
mod replay;
mod retention;

use deliver::send_one;
use expand::{CountRow, ExpansionOutcome, expand_independent_source, expand_one};
use retention::{reap_delivery_retention, reap_delivery_source_tombstones, reap_dispatch_retention};

pub use replay::{ReplayMode, ReplayRequest, ReplayResult, ReplayWindow, replay_deliveries, requeue_failed};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests;
