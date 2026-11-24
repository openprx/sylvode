//! Real-database tests of the dispatcher (opt-in via `OPENPR_TEST_DATABASE_URL`): own throwaway
//! database per run, migrated from `migrations/*.sql` on disk, dropped on the way out.
//! Colocated with the private functions of the dispatcher so tests can call `expand_one`, `send_one`
//! and `build_delivery_body` directly rather than only through the public `run_tick` surface.

use platform::timestamp::Timestamp;
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::dispatcher::deliver::{
    BusinessEventRow, TEST_ALLOW_PRIVATE_DELIVERY_TARGET, build_delivery_body, envelope_json, send_one,
};
use crate::events::dispatcher::expand::{ExpansionOutcome, expand_one};
use crate::events::dispatcher::replay::{
    ReplayMode, ReplayRequest, ReplayResult, TEST_REPLAY_CANDIDATE_BARRIER, replay_deliveries, requeue_failed,
};
use crate::events::dispatcher::retention::{reap_delivery_retention, reap_delivery_source_tombstones};
use crate::events::dispatcher::{
    DISPATCHER_LIVENESS_MAX_SILENCE_MS, FAIL_EXPANSION_STEP_B, OLDEST_PENDING_AGE_ALERT_MS, REPLAY_MAX_WINDOW_DAYS,
    backlog_alert, delivery_backoff_ms, dispatcher_is_live, dispatcher_is_live_since, ensure_workspace_subscriber_slot,
    oldest_pending_delivery_age_ms, oldest_pending_dispatch_age_ms, reclaim_expired_delivery_leases,
    reclaim_expired_dispatch_leases, run_tick, workspace_subscriber_count,
};
use crate::events::{BusinessEventInput, insert_business_event};
use crate::flow::collab::limits::SUBSCRIBERS_PER_WORKSPACE_MAX;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

fn frozen_delivery_fixture(name: &str) -> Value {
    let root = std::env::var_os("OPENPR_TEST_FLOW_EVENT_FIXTURE_DIR").map_or_else(
        || {
            std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../testing/fixtures/flow-event-v1"
            ))
        },
        std::path::PathBuf::from,
    );
    let path = root.join(format!("{name}.json"));
    serde_json::from_slice(
        &std::fs::read(&path)
            .unwrap_or_else(|error| panic!("frozen Flow event fixture {} must be readable: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("frozen Flow event fixture {} must be JSON: {error}", path.display()))
}

fn normalize_delivery_body(body: &Value, coalesced_sources: usize, changed_blocks: usize) -> Value {
    let mut normalized = body.clone();
    normalized["delivery"]["id"] = json!("<delivery_id>");
    normalized["event"]["event_id"] = json!("<event_id>");
    normalized["event"]["workspace_id"] = json!("<workspace_id>");
    normalized["event"]["aggregate"]["id"] = json!("<aggregate_id>");
    normalized["event"]["created_at"] = json!("<created_at>");
    if coalesced_sources > 0 {
        normalized["delivery"]["source_event_ids"] = Value::Array(
            (1..=coalesced_sources)
                .map(|index| json!(format!("<source_event_{index}>")))
                .collect(),
        );
    }
    if changed_blocks > 0 {
        normalized["event"]["payload"]["changed_block_ids"] = Value::Array(
            (1..=changed_blocks)
                .map(|index| json!(format!("<changed_block_{index}>")))
                .collect(),
        );
    }
    normalized
}

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
    url: String,
}

impl Scratch {
    async fn drop_self(self) {
        let Self {
            db,
            name,
            admin_url,
            url: _,
        } = self;
        drop(db);
        let Ok(admin) = Database::connect(&admin_url).await else {
            return;
        };
        let _ = admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await;
    }

    /// A second, independent connection to the same scratch database — needed to prove
    /// something about cross-connection concurrency (a held lock, a simultaneous lease race)
    /// that a single `DatabaseConnection`'s own pool cannot observe from the inside.
    async fn second_connection(&self) -> DatabaseConnection {
        Database::connect(&self.url)
            .await
            .unwrap_or_else(|err| panic!("could not open a second connection to {}: {err}", self.name))
    }
}

async fn scratch(label: &str) -> Option<Scratch> {
    let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
    let admin = Database::connect(&admin_url)
        .await
        .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

    let name = format!("sylvode_dispatcher_{label}");
    let quoted = format!("\"{name}\"");
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
        .await
        .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
    admin
        .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
        .await
        .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

    let (prefix, _) = admin_url.rsplit_once('/')?;
    let url = format!("{prefix}/{name}");
    // The API lib shard creates many independent scratch databases concurrently. Once this
    // database exists the admin pool has no more work, so release it before asking PostgreSQL
    // for the scratch connection. Keep the wait bounded, but allow the connection peak from
    // other database tests to drain instead of failing at SeaORM's short default acquire
    // timeout.
    drop(admin);
    let mut options = ConnectOptions::new(url.clone());
    options.acquire_timeout(std::time::Duration::from_mins(3));
    let db = Database::connect(options)
        .await
        .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

    migrate(&db).await;
    Some(Scratch {
        db,
        name,
        admin_url,
        url,
    })
}

async fn migrate(db: &DatabaseConnection) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("migrations directory is readable")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no migration file was found in {dir}");
    for path in files {
        let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
    }
}

macro_rules! scratch_or_skip {
    ($label:expr) => {
        match scratch($label).await {
            Some(scratch) => scratch,
            None => {
                eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                return;
            }
        }
    };
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "dispatcher-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("dispatcher-route-test-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) \
         VALUES ($1, $2, '!', 'test', 'user', true)",
        vec![owner_id.into(), format!("{owner_id}@dispatcher.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'dispatcher test', $3)",
        vec![
            workspace_id.into(),
            format!("ws-{workspace_id}").into(),
            owner_id.into(),
        ],
    )
    .await;
    workspace_id
}

/// Writes one `business_events` row plus its **exactly one** `event_dispatch` row, the same
/// way a real domain transaction does (`flow::command::create_object`,
/// `flow::collab::write::insert_event_dispatch_content`) — this is the "domain 事务只插一行
/// `event_dispatch`" half of the required proof; [`run_tick`]/[`expand_one`] below is the other
/// half ("`event_deliveries` 是 commit 之后由 dispatcher 展开的，不在域事务内").
async fn commit_dispatch_work(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    event_type: &str,
    document_id: Option<Uuid>,
    accepted_seq: Option<i64>,
    payload: serde_json::Value,
) -> Uuid {
    let event_id = insert_business_event(
        db,
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: event_type.to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: None,
            source: json!({ "surface": "rest" }),
            payload,
            metadata: json!({}),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await
    .expect("business event insert succeeds");

    exec(
        db,
        "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, document_id, accepted_seq, max_attempts) \
         VALUES ($1, $2, $3, $4, $5, $6, 10)",
        vec![
            Uuid::new_v4().into(),
            event_id.into(),
            workspace_id.into(),
            event_type.into(),
            document_id.into(),
            accepted_seq.into(),
        ],
    )
    .await;

    event_id
}

async fn seed_webhook(db: &DatabaseConnection, workspace_id: Uuid, url: &str, events: &[&str]) -> Uuid {
    let webhook_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) \
         VALUES ($1, $2, '!', 'test', 'user', true)",
        vec![owner_id.into(), format!("{owner_id}@dispatcher.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by) \
         VALUES ($1, $2, 'test hook', $3, 'shh', $4::jsonb, true, $5)",
        vec![
            webhook_id.into(),
            workspace_id.into(),
            url.into(),
            serde_json::to_value(events).unwrap_or_default().into(),
            owner_id.into(),
        ],
    )
    .await;
    webhook_id
}

async fn count(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    let row = db
        .query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .expect("count query runs")
        .expect("count query returns a row");
    row.try_get("", "n").expect("n column reads")
}

/// Reads a single named column off the one row a query returns. Generic sibling of [`count`]
/// for everything that isn't a `count(*)`.
async fn get_col<T: sea_orm::TryGetable>(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
    col: &str,
) -> T {
    db.query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .expect("query runs")
        .expect("row exists")
        .try_get("", col)
        .expect("column reads")
}

/// This suite's **clock-advancement primitive**: backdates one timestamp column on the row
/// matched by `id_column = id` by `offset` (positive moves it further into the past). Every
/// retention-boundary / backlog-age test below reaches its boundary by rewriting a row's own
/// timestamp this way — never by sleeping real wall-clock time. `table`/`timestamp_column`/
/// `id_column` are always `&'static str` literals fixed by the call site, never caller input.
async fn backdate(
    db: &DatabaseConnection,
    table: &'static str,
    timestamp_column: &'static str,
    id_column: &'static str,
    id: Uuid,
    offset: time::Duration,
) {
    exec(
        db,
        &format!(
            "UPDATE {table} SET {timestamp_column} = now() - ($2::bigint * interval '1 millisecond') \
             WHERE {id_column} = $1"
        ),
        vec![id.into(), i64::try_from(offset.whole_milliseconds()).unwrap().into()],
    )
    .await;
}

/// [`backdate`]'s sibling for *exact*-boundary fixtures: sets a timestamp column to a literal
/// value computed on the Rust side, rather than relative to Postgres's own `now()`. Pair this
/// with the retention reapers' explicit `now` argument so the test's write and the reaper's
/// read share one frozen reference instant instead of two independent, always-slightly-drifted
/// `now()` calls — required for asserting "exactly at the boundary survives, one ms past it
/// does not" deterministically.
async fn set_timestamp(
    db: &DatabaseConnection,
    table: &'static str,
    timestamp_column: &'static str,
    id_column: &'static str,
    id: Uuid,
    value: platform::timestamp::Timestamp,
) {
    exec(
        db,
        &format!("UPDATE {table} SET {timestamp_column} = $2 WHERE {id_column} = $1"),
        vec![id.into(), value.into()],
    )
    .await;
}

/// Deletes a webhook outright — the "subscriber deleted" half of the snapshot-semantics gate,
/// distinguished from the `active` toggle case, which is exercised separately.
async fn delete_webhook(db: &DatabaseConnection, webhook_id: Uuid) {
    exec(db, "DELETE FROM webhooks WHERE id = $1", vec![webhook_id.into()]).await;
}

/// Drains `expand_one` against `db` until nothing is left to lease, counting how many work
/// items this task personally expanded. Used to run several simulated dispatcher instances
/// concurrently against one shared backlog.
async fn drain_expand_one(db: &DatabaseConnection) -> u32 {
    let mut expanded = 0u32;
    loop {
        match expand_one(db).await {
            Ok(Some(ExpansionOutcome::Expanded | ExpansionOutcome::NoSubscribers)) => expanded += 1,
            Ok(Some(ExpansionOutcome::StaleLease)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    expanded
}

/// [`drain_expand_one`]'s sibling for the send side.
async fn drain_send_one(state: AppState) -> u32 {
    let client = reqwest::Client::new();
    let mut sent = 0u32;
    while let Ok(Some(_)) = send_one(&state, &client).await {
        sent += 1;
    }
    sent
}

#[tokio::test]
async fn a_dispatch_leases_expired_worker_is_reclaimed_and_eventually_dead_lettered() {
    let scratch = scratch_or_skip!("dispatch-lease-reclaim");
    let workspace_id = seed_workspace(&scratch.db).await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    // Simulate a worker that leased the work item and then vanished: an expired lease still
    // sitting on a `pending` row.
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET lease_token = 'stale', lease_expires_at = now() - interval '1 minute' \
         WHERE event_id = $1",
        vec![event_id.into()],
    )
    .await;

    let reclaimed = reclaim_expired_dispatch_leases(&scratch.db)
        .await
        .expect("reclaim runs");
    assert_eq!(reclaimed, 1);

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        status: String,
        lease_reclaims: i32,
        lease_token: Option<String>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, lease_reclaims, lease_token FROM event_dispatch WHERE event_id = $1",
        vec![event_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("dispatch query runs")
    .expect("one dispatch row exists");
    assert_eq!(row.status, "pending", "a first reclaim must not exhaust the budget");
    assert_eq!(row.lease_reclaims, 1);
    assert!(row.lease_token.is_none());

    scratch.drop_self().await;
}

// =============================================================================================
// Gate: dispatcher_liveness_and_backlog
// =============================================================================================

#[test]
fn dispatcher_liveness_pure_boundary_via_virtual_clock() {
    let last_tick =
        platform::timestamp::Timestamp::parse_rfc3339("2026-01-01T00:00:00Z").expect("fixed timestamp parses");

    assert!(
        !dispatcher_is_live_since(None, last_tick, DISPATCHER_LIVENESS_MAX_SILENCE_MS),
        "before any tick has ever happened, liveness must be false regardless of `now`"
    );

    let exactly_at_budget = last_tick + time::Duration::milliseconds(DISPATCHER_LIVENESS_MAX_SILENCE_MS);
    assert!(
        dispatcher_is_live_since(Some(last_tick), exactly_at_budget, DISPATCHER_LIVENESS_MAX_SILENCE_MS),
        "exactly at the silence budget, the dispatcher must still be reported live"
    );

    let one_ms_past_budget = exactly_at_budget + time::Duration::milliseconds(1);
    assert!(
        !dispatcher_is_live_since(Some(last_tick), one_ms_past_budget, DISPATCHER_LIVENESS_MAX_SILENCE_MS),
        "one millisecond past the silence budget, the dispatcher must be reported stale"
    );
}

#[tokio::test]
async fn dispatcher_reports_live_immediately_after_a_real_tick() {
    let scratch = scratch_or_skip!("liveness-integration");
    let state = state_for(scratch.db.clone());
    run_tick(&state, &reqwest::Client::new(), 1).await;
    assert!(
        dispatcher_is_live(Timestamp::now(), DISPATCHER_LIVENESS_MAX_SILENCE_MS),
        "the process-global wrapper must report live immediately after any run_tick call in \
         this process (this assertion only ever requires *some* recent tick, so it cannot be \
         made to fail by other tests concurrently ticking the same process)"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn oldest_pending_dispatch_age_advances_with_a_backdated_created_at_not_a_sleep() {
    let scratch = scratch_or_skip!("oldest-pending-age");
    let workspace_id = seed_workspace(&scratch.db).await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    let dispatch_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id = $1",
        vec![event_id.into()],
        "id",
    )
    .await;

    let fresh_age = oldest_pending_dispatch_age_ms(&scratch.db, Timestamp::now())
        .await
        .expect("query runs")
        .expect("one pending row exists");
    assert!(
        fresh_age < 5_000,
        "a freshly committed row must be reported as ~0ms old, got {fresh_age}ms"
    );

    // Advance the virtual clock by backdating the row's own `created_at`, not by sleeping.
    let now = Timestamp::now();
    set_timestamp(
        &scratch.db,
        "event_dispatch",
        "created_at",
        "id",
        dispatch_id,
        now - time::Duration::milliseconds(OLDEST_PENDING_AGE_ALERT_MS + 1),
    )
    .await;

    let aged = oldest_pending_dispatch_age_ms(&scratch.db, now)
        .await
        .expect("query runs")
        .expect("still one pending row");
    assert!(aged > OLDEST_PENDING_AGE_ALERT_MS, "aged={aged}ms");

    let report = super::DispatchTickReport {
        oldest_pending_dispatch_age_ms: Some(aged),
        ..Default::default()
    };
    assert!(
        backlog_alert(&report, OLDEST_PENDING_AGE_ALERT_MS),
        "a backlog item older than the alert threshold must trip backlog_alert"
    );
    assert!(
        !backlog_alert(&super::DispatchTickReport::default(), OLDEST_PENDING_AGE_ALERT_MS),
        "an empty report (no known backlog) must never alert"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn oldest_pending_delivery_age_counts_pending_sealed_and_leased_but_not_terminal_rows() {
    let scratch = scratch_or_skip!("oldest-pending-delivery-age");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![event_id.into()],
        "id",
    )
    .await;
    let now = Timestamp::now();
    set_timestamp(
        &scratch.db,
        "event_deliveries",
        "created_at",
        "id",
        delivery_id,
        now - time::Duration::milliseconds(OLDEST_PENDING_AGE_ALERT_MS + 1),
    )
    .await;

    let age = oldest_pending_delivery_age_ms(&scratch.db, now)
        .await
        .expect("query runs")
        .expect("one undelivered row exists");
    assert!(age > OLDEST_PENDING_AGE_ALERT_MS, "age={age}ms");

    // Once it reaches a terminal state, it must stop counting toward the backlog age at all.
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET status = 'dispatched', terminated_at = now(), \
         lease_token = NULL, lease_expires_at = NULL WHERE id = $1",
        vec![delivery_id.into()],
    )
    .await;
    let age_after_terminal = oldest_pending_delivery_age_ms(&scratch.db, Timestamp::now())
        .await
        .expect("query runs");
    assert_eq!(
        age_after_terminal, None,
        "a dispatched (terminal) row must not count toward oldest_pending_delivery_age"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn flow_delivery_v08_dispatch_and_delivery_lease_pairs_reject_both_unreachable_halves() {
    let scratch = scratch_or_skip!("lease-pair-checks");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    let dispatch_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id=$1",
        vec![event_id.into()],
        "id",
    )
    .await;
    for sql in [
        "UPDATE event_dispatch SET lease_token='impossible' WHERE id=$1",
        "UPDATE event_dispatch SET lease_expires_at=now()+interval '1 minute' WHERE id=$1",
    ] {
        assert!(
            scratch
                .db
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    vec![dispatch_id.into()],
                ))
                .await
                .is_err(),
            "dispatch lease half-pair must violate its CHECK: {sql}"
        );
    }

    expand_one(&scratch.db).await.expect("expansion runs");
    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id=$1",
        vec![event_id.into()],
        "id",
    )
    .await;
    for sql in [
        "UPDATE event_deliveries SET lease_token='impossible' WHERE id=$1",
        "UPDATE event_deliveries SET lease_expires_at=now()+interval '1 minute' WHERE id=$1",
    ] {
        assert!(
            scratch
                .db
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    vec![delivery_id.into()],
                ))
                .await
                .is_err(),
            "delivery lease half-pair must violate its CHECK: {sql}"
        );
    }

    scratch.drop_self().await;
}

#[tokio::test]
async fn delivery_lease_expiry_is_reclaimed_by_its_own_reaper_content_to_sealed_plain_to_pending() {
    let scratch = scratch_or_skip!("delivery-leased-reaper");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created", "flow.content.accepted"],
    )
    .await;

    // A plain (non-content) delivery, stuck `leased` by a worker that vanished.
    let plain_event =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let plain_delivery: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![plain_event.into()],
        "id",
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET status = 'leased', lease_token = 'dead', \
         lease_expires_at = now() - interval '1 minute' WHERE id = $1",
        vec![plain_delivery.into()],
    )
    .await;

    // A content delivery, likewise stuck `leased`.
    let document_id = Uuid::new_v4();
    let content_event = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({}),
    )
    .await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let content_delivery: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![content_event.into()],
        "id",
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET status = 'leased', lease_token = 'dead', \
         lease_expires_at = now() - interval '1 minute' WHERE id = $1",
        vec![content_delivery.into()],
    )
    .await;

    // The plain lease-claim query (`status IN ('pending','sealed')`) cannot reach either row
    // while they sit at `leased` -- only the dedicated reclaim reaper can.
    let claimed_before_reclaim = send_one(&state_for(scratch.db.clone()), &reqwest::Client::new())
        .await
        .expect("send_one runs");
    assert_eq!(
        claimed_before_reclaim, None,
        "a stuck leased row must not be reachable by the ordinary lease-claim query"
    );

    let reclaimed = reclaim_expired_delivery_leases(&scratch.db)
        .await
        .expect("reclaim runs");
    assert_eq!(reclaimed, 2);

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        status: String,
        attempts: i32,
        lease_token: Option<String>,
    }
    let plain_row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, attempts, lease_token FROM event_deliveries WHERE id = $1",
        vec![plain_delivery.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("query runs")
    .expect("row exists");
    assert_eq!(
        plain_row.status, "pending",
        "a non-content row reclaims to pending, never sealed"
    );
    assert_eq!(
        plain_row.attempts, 1,
        "lease expiry counts toward attempts, same as a delivery failure"
    );
    assert!(plain_row.lease_token.is_none());

    let content_row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, attempts, lease_token FROM event_deliveries WHERE id = $1",
        vec![content_delivery.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("query runs")
    .expect("row exists");
    assert_eq!(
        content_row.status, "sealed",
        "a content row's reclaim goes to sealed, never back to pending, \
         per events-v1.md's 'sealed 是一个真状态'"
    );
    assert_eq!(content_row.attempts, 1);

    scratch.drop_self().await;
}

fn delivery_lease_row_for_test(
    id: Uuid,
    event_id: Uuid,
    subscriber_id: Option<Uuid>,
) -> crate::events::dispatcher::deliver::DeliveryLeaseRow {
    crate::events::dispatcher::deliver::DeliveryLeaseRow {
        id,
        event_id,
        subscriber_id: subscriber_id.unwrap_or_else(Uuid::new_v4),
        document_id: None,
        attempts: 0,
        max_attempts: 10,
        first_seq: None,
        latest_seq: None,
    }
}

mod coalesce;
mod deliver;
mod expand;
mod form_events;
mod replay;
mod retention;
