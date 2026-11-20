//! `GET /api/v1/oauth/events/stream`: Sylvode pushes notifications and task state changes to the
//! client that holds a delegated token, over one outbound server-sent-events connection.
//!
//! # How an event gets here
//!
//! A database trigger writes an outbox row with each domain write; the worker's single writer
//! (`apps/worker/src/oauth_feed.rs`) moves outbox rows into `oauth_event_feed`, which assigns a
//! sequence number in commit order, and sends `NOTIFY sv_oauth_feed`. This module listens on that
//! channel, once per process, and wakes the streams. A stream reads the feed rows after its own
//! position for its grant's workspace and decides, per row and at that moment, whether the grant
//! may see it ([`envelope::candidacy`] plus [`visible_projects`]). The same code serves live
//! delivery and backfill after a resume.
//!
//! # What a stream holds
//!
//! No database connection while idle. It waits on three in-process signals (a feed wake-up, a
//! grant change, a timer) and takes a pooled connection only for the few short queries it runs
//! when one of them fires. The `LISTEN` connection is shared: one per process, not one per
//! stream.
//!
//! # Guarantees
//!
//! * **Revocation.** `revoke_grant` sends `sv_oauth_grant_changed` after commit; every stream of
//!   that grant re-reads its grant state at once, sends `revoked` (or `reauth` when only its own
//!   token ended), drops what was still queued and closes. If a notification is missed, a stream
//!   still re-reads its grant state every [`StreamTuning::idle_tick`] (3 s) and never delivers an
//!   event on state older than [`StreamTuning::grant_cache_ttl`] (2 s). So no event is delivered
//!   later than 2 s after the revocation commits, and an idle stream is closed within 3 s, with or
//!   without the listener. A reconnect is refused with 401 by token validation.
//! * **Token expiry.** A stream never outlives the token it was opened with: at `expires_at` it
//!   sends `reauth` (`token_expired`) and closes. The deadline is computed on the database clock.
//! * **Delivery filtering.** Workspace, scope, user, the grant's project set and the user's
//!   present visibility of the project are checked when an event is delivered, not when it was
//!   produced. An event the grant may not see is not sent and leaves no trace: the cursor is
//!   sealed ([`cursor`]), so the gaps in the shared sequence are invisible to the client.
//! * **Ordering and duplicates.** Events arrive in feed order. Delivery is at-least-once across
//!   reconnects with a stable event id (the sealed position), exactly once on one connection.
//! * **Back-pressure.** Each stream queues at most [`StreamTuning::channel_capacity`] frames. A
//!   client that does not drain them for [`StreamTuning::lag_timeout`] is sent `lagged` and
//!   closed; the queue is never allowed to grow.
//!
//! Nothing here logs a token, a cursor or an event payload: log fields are ids, counts and fixed
//! reason codes.

mod cursor;
#[cfg(test)]
mod db_tests;
pub mod envelope;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{ConnectInfo, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use parking_lot::Mutex;
use platform::app::AppState;
use platform::config::OAuthConfig;
use platform::timestamp::Timestamp;
use sea_orm::{DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement, TransactionTrait};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::Instant;
use uuid::Uuid;

use self::cursor::{CursorError, CursorKey, Position};
use self::envelope::{Candidacy, EventKind, FeedRow, GrantView, ProjectInfo, TypeFilter, control};
use super::audit::{self, AuditEvent, AuditRecord, DetailValue};
use super::error::{OAuthBearerError, OAuthError};
use super::grants::{Actor, GRANT_CHANGED_CHANNEL, RevokeReason, revoke_grant};
use super::metadata::api_resource_metadata_url;
use super::rate::{self, RateBucket, RateError, RateKey};
use super::scopes::{ProjectScope, Scope, ScopeSet};
use super::validate::{AudienceRule, ForwardCredential, query_carries_access_token, validate_access_token};

/// Path of the stream endpoint.
pub const STREAM_PATH: &str = "/api/v1/oauth/events/stream";
/// Channel the worker's feed writer notifies after committing feed rows.
pub const FEED_CHANNEL: &str = "sv_oauth_feed";
/// Channel the outbox triggers notify, waking the feed writer.
pub const OUTBOX_CHANNEL: &str = "sv_oauth_outbox";

/// Concurrent streams of one grant, unless configured otherwise.
pub const DEFAULT_STREAMS_PER_GRANT: usize = 3;
/// Concurrent streams of one user across all of their grants.
pub const DEFAULT_STREAMS_PER_USER: usize = 20;
/// `Retry-After` of a refusal because a limit of open streams is reached.
const LIMIT_RETRY_AFTER_SECS: u64 = 30;
/// How long a database that keeps failing is tolerated before a stream gives up and closes.
const DB_FAILURE_GIVE_UP: Duration = Duration::from_secs(30);
/// Pause after a failed query, so a database outage is not answered with a busy loop.
const DB_FAILURE_PAUSE: Duration = Duration::from_millis(500);

/// The open-stream limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamLimits {
    pub per_grant: usize,
    pub per_user: usize,
    pub per_process: usize,
}

impl StreamLimits {
    /// The contract defaults, with the per-process cap taken from `[oauth] max_streams_per_process`.
    pub fn from_config(config: &OAuthConfig) -> Self {
        Self {
            per_grant: DEFAULT_STREAMS_PER_GRANT,
            per_user: DEFAULT_STREAMS_PER_USER,
            per_process: usize::try_from(config.max_streams_per_process).unwrap_or(usize::MAX),
        }
    }
}

/// Timing and sizing of a stream. The defaults are the contract's; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct StreamTuning {
    /// Whether to run the shared `LISTEN` connection. Off, streams fall back to their timers.
    pub listen: bool,
    /// Silence after which a `: keep-alive` comment is sent.
    pub keepalive: Duration,
    /// Longest an idle stream waits before it re-reads its grant state and the feed.
    pub idle_tick: Duration,
    /// Oldest grant state an event may be delivered on.
    pub grant_cache_ttl: Duration,
    /// Frames a stream may have queued for its client.
    pub channel_capacity: usize,
    /// How long a full queue is waited on before the client is declared lagging.
    pub lag_timeout: Duration,
    /// Feed rows read per query, backfill and live alike.
    pub batch: i64,
    /// Raw feed rows examined when deciding whether a resume is within the backfill limits, as a
    /// multiple of `event_backfill_max_events`.
    pub resume_scan_factor: i64,
}

impl Default for StreamTuning {
    fn default() -> Self {
        Self {
            listen: true,
            keepalive: Duration::from_secs(20),
            idle_tick: Duration::from_secs(3),
            grant_cache_ttl: Duration::from_secs(2),
            channel_capacity: 256,
            lag_timeout: Duration::from_secs(30),
            batch: 200,
            resume_scan_factor: 5,
        }
    }
}

/// Something a stream wakes on besides its timers.
#[derive(Clone, Copy, Debug)]
enum Signal {
    /// A grant was revoked or changed.
    Grant(Uuid),
    /// The listener (re)connected: anything may have been missed.
    Everyone,
}

#[derive(Default)]
struct Registry {
    total: usize,
    grants: HashMap<Uuid, usize>,
    users: HashMap<Uuid, usize>,
}

/// Which limit refused a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitExceeded {
    Grant,
    User,
    Process,
}

/// One open stream's place in the limits. Released when dropped, which happens when the response
/// body is dropped: a connection counts for as long as its socket does.
pub struct Permit {
    hub: Arc<Hub>,
    grant_id: Uuid,
    user_id: Uuid,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut registry = self.hub.registry.lock();
        registry.total = registry.total.saturating_sub(1);
        release(&mut registry.grants, self.grant_id);
        release(&mut registry.users, self.user_id);
    }
}

fn release(counts: &mut HashMap<Uuid, usize>, id: Uuid) {
    if let Some(count) = counts.get_mut(&id) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&id);
        }
    }
}

/// The per-process state shared by every stream: the limits, the wake-up signals and the one
/// `LISTEN` connection.
pub struct Hub {
    limits: StreamLimits,
    tuning: StreamTuning,
    registry: Mutex<Registry>,
    signals: broadcast::Sender<Signal>,
    feed: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
    listener_started: AtomicBool,
}

impl Hub {
    pub fn new(limits: StreamLimits, tuning: StreamTuning) -> Arc<Self> {
        Arc::new(Self {
            limits,
            tuning,
            registry: Mutex::new(Registry::default()),
            signals: broadcast::channel(256).0,
            feed: watch::channel(0).0,
            shutdown: watch::channel(false).0,
            listener_started: AtomicBool::new(false),
        })
    }

    /// Takes a place for a stream of `grant_id` and `user_id`, or names the limit that is reached.
    pub fn try_acquire(self: &Arc<Self>, grant_id: Uuid, user_id: Uuid) -> Result<Permit, LimitExceeded> {
        let mut registry = self.registry.lock();
        if registry.total >= self.limits.per_process {
            return Err(LimitExceeded::Process);
        }
        if registry.grants.get(&grant_id).copied().unwrap_or(0) >= self.limits.per_grant {
            return Err(LimitExceeded::Grant);
        }
        if registry.users.get(&user_id).copied().unwrap_or(0) >= self.limits.per_user {
            return Err(LimitExceeded::User);
        }
        registry.total += 1;
        *registry.grants.entry(grant_id).or_insert(0) += 1;
        *registry.users.entry(user_id).or_insert(0) += 1;
        drop(registry);
        Ok(Permit {
            hub: Arc::clone(self),
            grant_id,
            user_id,
        })
    }

    /// Streams currently open in this process.
    pub fn open_streams(&self) -> usize {
        self.registry.lock().total
    }

    /// Ends every stream of this hub without a control message. Clients reconnect and resume.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    fn signal(&self, signal: Signal) {
        // No receiver means no stream is open; there is nobody to wake.
        let _ = self.signals.send(signal);
    }

    fn wake_feed(&self) {
        self.feed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    /// Starts the shared listener once. A no-op when listening is switched off.
    fn ensure_listener(self: &Arc<Self>, db: &DatabaseConnection) {
        if !self.tuning.listen || self.listener_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let hub = Arc::clone(self);
        let db = db.clone();
        tokio::spawn(async move { hub.listen_loop(db).await });
    }

    async fn listen_loop(self: Arc<Self>, db: DatabaseConnection) {
        use sea_orm::sqlx::postgres::PgListener;
        let mut shutdown = self.shutdown.subscribe();
        loop {
            if *shutdown.borrow() {
                return;
            }
            let connected = PgListener::connect_with(db.get_postgres_connection_pool()).await;
            let mut listener = match connected {
                Ok(listener) => listener,
                Err(error) => {
                    tracing::warn!(error = %error, "oauth stream listener connect failed; streams fall back to polling");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            if let Err(error) = listener.listen_all([FEED_CHANNEL, GRANT_CHANGED_CHANNEL]).await {
                tracing::warn!(error = %error, "oauth stream LISTEN failed; streams fall back to polling");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            // Notifications sent while no listener was connected are gone: wake everyone.
            self.signal(Signal::Everyone);
            self.wake_feed();
            loop {
                tokio::select! {
                    received = listener.recv() => match received {
                        Ok(notification) if notification.channel() == GRANT_CHANGED_CHANNEL => {
                            match Uuid::parse_str(notification.payload()) {
                                Ok(grant_id) => self.signal(Signal::Grant(grant_id)),
                                Err(_) => self.signal(Signal::Everyone),
                            }
                        }
                        Ok(_) => self.wake_feed(),
                        Err(error) => {
                            tracing::warn!(error = %error, "oauth stream LISTEN dropped; reconnecting");
                            break;
                        }
                    },
                    _ = shutdown.changed() => return,
                }
            }
        }
    }
}

static HUBS: Mutex<Vec<Weak<Hub>>> = Mutex::new(Vec::new());

/// Signals shutdown to every live hub in `registry` and forgets the dead ones.
fn shutdown_registered(registry: &Mutex<Vec<Weak<Hub>>>) {
    let hubs: Vec<Arc<Hub>> = {
        let mut registered = registry.lock();
        registered.retain(|hub| hub.strong_count() > 0);
        registered.iter().filter_map(Weak::upgrade).collect()
    };
    for hub in hubs {
        hub.shutdown();
    }
}

/// Ends every open stream of this process. The API's shutdown path calls it before it waits for
/// connections to drain, because a stream never ends on its own while its client is connected.
pub fn begin_shutdown() {
    shutdown_registered(&HUBS);
}

/// Everything a request handler needs besides the application state.
pub struct StreamContext {
    config: Arc<OAuthConfig>,
    hub: Arc<Hub>,
    key: OnceLock<Arc<CursorKey>>,
    resource_metadata: Option<String>,
}

impl StreamContext {
    fn cursor_key(&self, secret: &str) -> Option<Arc<CursorKey>> {
        if let Some(key) = self.key.get() {
            return Some(Arc::clone(key));
        }
        let derived = Arc::new(CursorKey::derive(secret)?);
        Some(Arc::clone(self.key.get_or_init(|| derived)))
    }

    /// Effective backfill window: the configured age, never more than the feed is kept.
    fn window_secs(&self) -> i64 {
        let age = i64::from(self.config.event_backfill_max_age_hours).saturating_mul(3600);
        let kept = i64::from(self.config.event_feed_retention_days).saturating_mul(86_400);
        age.min(kept)
    }
}

// -----------------------------------------------------------------------------------------------
// The permission function
// -----------------------------------------------------------------------------------------------

/// Which of `project_ids` the user can see in the workspace right now, with their display data.
///
/// This is the one place the delivery filter asks the existing permission model a question. In
/// Sylvode today a project is visible to every member of its workspace, so the answer is: the
/// project exists in the workspace and the user is still a member. The delegated-access package
/// replaces the body with its per-project resolver; the signature stays.
pub async fn visible_projects(
    db: &DatabaseConnection,
    user_id: Uuid,
    workspace_id: Uuid,
    project_ids: &[Uuid],
) -> Result<HashMap<Uuid, ProjectInfo>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        key: String,
        name: String,
    }
    if project_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.id, p.key, p.name FROM projects p \
         WHERE p.id = ANY($1) AND p.workspace_id = $2 \
           AND EXISTS (SELECT 1 FROM workspace_members m WHERE m.workspace_id = p.workspace_id AND m.user_id = $3)",
        vec![project_ids.to_vec().into(), workspace_id.into(), user_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.id,
                ProjectInfo {
                    key: row.key,
                    name: row.name,
                },
            )
        })
        .collect())
}

// -----------------------------------------------------------------------------------------------
// Grant state
// -----------------------------------------------------------------------------------------------

/// Why a stream ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The grant ended; `reason` is a revocation code.
    Revoked(String),
    /// Only the token this stream was opened with ended.
    Reauth(&'static str),
    Lagged,
    ClientGone,
    Shutdown,
    Internal,
}

impl EndReason {
    const fn audit_code(&self) -> &'static str {
        match self {
            Self::Revoked(_) => "revoked",
            Self::Reauth(reason) => reason,
            Self::Lagged => "lagged",
            Self::ClientGone => "client_closed",
            Self::Shutdown => "shutdown",
            Self::Internal => "internal",
        }
    }
}

enum LiveFailure {
    End(EndReason),
    Db(DbErr),
}

/// The part of the grant state that changes while a stream is open.
struct LiveState {
    projects: ProjectScope,
    token_remaining: Duration,
}

#[derive(Debug, FromQueryResult)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the fields are the columns of the lookup row"
)]
struct LiveRow {
    grant_status: String,
    revoke_reason: Option<String>,
    grant_live: bool,
    project_mode: String,
    grant_session_version: i64,
    token_revoked: bool,
    token_remaining_ms: i64,
    client_active: bool,
    user_active: bool,
    user_session_version: i64,
    is_member: bool,
    allows_apps: bool,
    project_ids: Option<Vec<Uuid>>,
}

const LIVE_SQL: &str = "\
SELECT g.status AS grant_status, g.revoke_reason, (g.expires_at > now()) AS grant_live, g.project_mode, \
       g.user_session_version AS grant_session_version, \
       (t.revoked_at IS NOT NULL) AS token_revoked, \
       (EXTRACT(EPOCH FROM (t.expires_at - now())) * 1000)::bigint AS token_remaining_ms, \
       (c.status = 'active') AS client_active, u.is_active AS user_active, \
       u.session_version AS user_session_version, \
       (m.user_id IS NOT NULL) AS is_member, \
       COALESCE(s.allow_connected_apps, true) AS allows_apps, \
       CASE WHEN g.project_mode = 'selected' \
            THEN ARRAY(SELECT gp.project_id FROM oauth_grant_projects gp \
                       WHERE gp.grant_id = g.id ORDER BY gp.project_id) END AS project_ids \
FROM oauth_tokens t \
JOIN oauth_grants g ON g.id = t.grant_id \
JOIN oauth_clients c ON c.client_id = g.client_id \
JOIN users u ON u.id = g.user_id \
LEFT JOIN workspace_members m ON m.workspace_id = g.workspace_id AND m.user_id = g.user_id \
LEFT JOIN workspace_delegation_settings s ON s.workspace_id = g.workspace_id \
WHERE t.id = $1 AND t.grant_id = $2";

/// The live-state condition that ended a grant, if one did, as the revocation it corresponds to.
const fn state_revocation(row: &LiveRow) -> Option<RevokeReason> {
    if !row.grant_live {
        Some(RevokeReason::Expired)
    } else if !row.client_active {
        Some(RevokeReason::ClientDisabled)
    } else if !row.user_active {
        Some(RevokeReason::AccountDisabled)
    } else if row.user_session_version != row.grant_session_version {
        Some(RevokeReason::PasswordChanged)
    } else if !row.is_member {
        Some(RevokeReason::MembershipRemoved)
    } else if !row.allows_apps {
        Some(RevokeReason::WorkspaceDisabled)
    } else {
        None
    }
}

/// Ends the grant behind a stream whose live state failed, as token validation does, so the grant
/// shows as revoked and a reconnect is refused at once. Failure is logged and changes nothing.
async fn revoke_for_state(db: &DatabaseConnection, grant_id: Uuid, reason: RevokeReason) {
    let result = async {
        let tx = db.begin().await?;
        revoke_grant(&tx, grant_id, reason, Actor::System).await?;
        tx.commit().await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(
            grant_id = %grant_id,
            reason = reason.as_str(),
            error = %error,
            "could not revoke a grant whose live state failed; its stream is closed anyway"
        );
    }
}

/// Reads the live state of a grant and one of its tokens, by id. Failures that name an end of the
/// grant or of the token come back as [`LiveFailure::End`].
async fn load_live(db: &DatabaseConnection, token_id: Uuid, grant_id: Uuid) -> Result<LiveState, LiveFailure> {
    let row = LiveRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        LIVE_SQL,
        vec![token_id.into(), grant_id.into()],
    ))
    .one(db)
    .await
    .map_err(LiveFailure::Db)?;
    let Some(row) = row else {
        // The token or the grant row is gone (a deleted user or workspace).
        return Err(LiveFailure::End(EndReason::Revoked("grant_removed".to_owned())));
    };
    if row.grant_status != "active" {
        let reason = row.revoke_reason.unwrap_or_else(|| "revoked".to_owned());
        return Err(LiveFailure::End(EndReason::Revoked(reason)));
    }
    if let Some(reason) = state_revocation(&row) {
        revoke_for_state(db, grant_id, reason).await;
        return Err(LiveFailure::End(EndReason::Revoked(reason.as_str().to_owned())));
    }
    if row.token_revoked {
        return Err(LiveFailure::End(EndReason::Reauth("token_revoked")));
    }
    let remaining = u64::try_from(row.token_remaining_ms).unwrap_or(0);
    if remaining == 0 {
        return Err(LiveFailure::End(EndReason::Reauth("token_expired")));
    }
    let projects = match (row.project_mode.as_str(), row.project_ids) {
        ("all", _) => ProjectScope::All,
        ("selected", Some(ids)) => ProjectScope::Selected(Arc::from(ids)),
        _ => return Err(LiveFailure::End(EndReason::Internal)),
    };
    Ok(LiveState {
        projects,
        token_remaining: Duration::from_millis(remaining),
    })
}

// -----------------------------------------------------------------------------------------------
// Feed reads
// -----------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct FeedRecord {
    seq: i64,
    event_type: String,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    user_id: Option<Uuid>,
    subject: serde_json::Value,
    occurred_at: Timestamp,
    created_at: Timestamp,
}

impl From<FeedRecord> for FeedRow {
    fn from(record: FeedRecord) -> Self {
        Self {
            seq: record.seq,
            event_type: record.event_type,
            workspace_id: record.workspace_id,
            project_id: record.project_id,
            user_id: record.user_id,
            subject: record.subject,
            occurred_at: record.occurred_at,
            created_at: record.created_at,
        }
    }
}

/// The next `limit` feed rows of a workspace after `after`, in sequence order. Bounded by
/// `limit`; backfill and live delivery both read through it.
async fn fetch_rows(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    after: i64,
    limit: i64,
) -> Result<Vec<FeedRow>, DbErr> {
    Ok(FeedRecord::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT seq, event_type, workspace_id, project_id, user_id, subject, occurred_at, created_at \
         FROM oauth_event_feed WHERE workspace_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
        vec![workspace_id.into(), after.into(), limit.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(FeedRow::from)
    .collect())
}

#[derive(Debug, FromQueryResult)]
struct HeadRow {
    head: i64,
}

/// The newest committed sequence number, `0` for an empty feed.
async fn feed_head(db: &DatabaseConnection) -> Result<i64, DbErr> {
    Ok(HeadRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COALESCE(MAX(seq), 0)::bigint AS head FROM oauth_event_feed",
        vec![],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.head))
}

#[derive(Debug, FromQueryResult)]
struct RoutingRow {
    event_type: String,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    user_id: Option<Uuid>,
}

/// Why a resume restarts from the head instead of continuing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResyncReason {
    /// The cursor is older than the backfill window or the retained feed.
    CursorTooOld,
    /// The cursor is ahead of the feed (a restored database): its events cannot be known.
    CursorAhead,
    /// More matching events are waiting than a backfill may carry.
    TooManyEvents,
}

impl ResyncReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CursorTooOld => "cursor_too_old",
            Self::CursorAhead => "cursor_ahead",
            Self::TooManyEvents => "too_many_events",
        }
    }
}

/// Where a new stream starts.
struct StartPlan {
    after: Position,
    resync: Option<ResyncReason>,
}

/// Decides where a stream starts: after the presented cursor when it is inside the backfill
/// limits, at the head otherwise. The check reads at most `event_backfill_max_events *
/// resume_scan_factor + 1` light rows, so a cursor far in the past costs a bounded query.
async fn plan_start(
    db: &DatabaseConnection,
    ctx: &StreamContext,
    grant: &GrantView,
    types: TypeFilter,
    presented: Option<Position>,
    now: i64,
) -> Result<StartPlan, DbErr> {
    let head = feed_head(db).await?;
    let at_head = Position { seq: head, at: now };
    let Some(position) = presented else {
        return Ok(StartPlan {
            after: at_head,
            resync: None,
        });
    };
    let resync = |reason| StartPlan {
        after: at_head,
        resync: Some(reason),
    };
    if position.at < now.saturating_sub(ctx.window_secs()) {
        return Ok(resync(ResyncReason::CursorTooOld));
    }
    if position.seq > head {
        return Ok(resync(ResyncReason::CursorAhead));
    }
    let max_events = i64::from(ctx.config.event_backfill_max_events);
    let cap = max_events
        .saturating_mul(ctx.hub.tuning.resume_scan_factor)
        .saturating_add(1);
    let rows = RoutingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT event_type, workspace_id, project_id, user_id FROM oauth_event_feed \
         WHERE workspace_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
        vec![grant.workspace_id.into(), position.seq.into(), cap.into()],
    ))
    .all(db)
    .await?;
    if i64::try_from(rows.len()).unwrap_or(i64::MAX) >= cap {
        return Ok(resync(ResyncReason::TooManyEvents));
    }
    let candidacies: Vec<Candidacy> = rows
        .iter()
        .map(|row| {
            envelope::candidacy(
                grant,
                types,
                envelope::Routing {
                    event_type: &row.event_type,
                    workspace_id: row.workspace_id,
                    project_id: row.project_id,
                    user_id: row.user_id,
                },
            )
        })
        .collect();
    let wanted = distinct_projects(&candidacies);
    let visible = visible_projects(db, grant.user_id, grant.workspace_id, &wanted).await?;
    let matching = candidacies
        .iter()
        .filter(|candidacy| is_deliverable(**candidacy, &visible))
        .count();
    if i64::try_from(matching).unwrap_or(i64::MAX) > max_events {
        return Ok(resync(ResyncReason::TooManyEvents));
    }
    Ok(StartPlan {
        after: position,
        resync: None,
    })
}

fn distinct_projects(candidacies: &[Candidacy]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    candidacies
        .iter()
        .filter_map(|candidacy| match candidacy {
            Candidacy::Project(project) if seen.insert(*project) => Some(*project),
            _ => None,
        })
        .collect()
}

fn is_deliverable(candidacy: Candidacy, visible: &HashMap<Uuid, ProjectInfo>) -> bool {
    match candidacy {
        Candidacy::Refused => false,
        Candidacy::Unscoped => true,
        Candidacy::Project(project) => visible.contains_key(&project),
    }
}

// -----------------------------------------------------------------------------------------------
// One open stream
// -----------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Progress {
    More,
    Idle,
}

struct Connection {
    db: DatabaseConnection,
    ctx: Arc<StreamContext>,
    key: Arc<CursorKey>,
    client_id: Arc<str>,
    token_id: Uuid,
    grant: GrantView,
    types: TypeFilter,
    position: Position,
    live_at: Instant,
    token_deadline: Instant,
    force_refresh: bool,
    tx: mpsc::Sender<Bytes>,
    close: watch::Sender<Option<Bytes>>,
    feed_rx: watch::Receiver<u64>,
    signal_rx: broadcast::Receiver<Signal>,
    shutdown_rx: watch::Receiver<bool>,
    last_write: Instant,
    sent: i64,
    failing_since: Option<Instant>,
}

impl Connection {
    fn tuning(&self) -> StreamTuning {
        self.ctx.hub.tuning
    }

    fn cursor_text(&self) -> Option<String> {
        self.key.encode(self.grant.grant_id, self.position)
    }

    fn apply(&mut self, live: LiveState) {
        self.grant.projects = live.projects;
        self.live_at = Instant::now();
        self.token_deadline = self.live_at + live.token_remaining;
        self.force_refresh = false;
    }

    fn note_signal(&mut self, received: &Result<Signal, broadcast::error::RecvError>) {
        match received {
            Ok(Signal::Grant(id)) => {
                if *id == self.grant.grant_id {
                    self.force_refresh = true;
                }
            }
            Ok(Signal::Everyone) | Err(broadcast::error::RecvError::Lagged(_)) => self.force_refresh = true,
            Err(broadcast::error::RecvError::Closed) => {}
        }
    }

    fn drain_signals(&mut self) {
        loop {
            match self.signal_rx.try_recv() {
                Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => return,
                Ok(signal) => self.note_signal(&Ok(signal)),
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    self.note_signal(&Err(broadcast::error::RecvError::Lagged(skipped)));
                }
            }
        }
    }

    /// Records a failed query: tolerated for a while (with a pause, not a busy loop), fatal after
    /// [`DB_FAILURE_GIVE_UP`].
    async fn db_failed(&mut self, error: &DbErr) -> Result<(), EndReason> {
        let since = *self.failing_since.get_or_insert_with(Instant::now);
        tracing::warn!(grant_id = %self.grant.grant_id, error = %error, "oauth stream query failed");
        if since.elapsed() >= DB_FAILURE_GIVE_UP {
            return Err(EndReason::Internal);
        }
        tokio::time::sleep(DB_FAILURE_PAUSE).await;
        Ok(())
    }

    const fn db_ok(&mut self) {
        self.failing_since = None;
    }

    async fn refresh(&mut self) -> Result<(), EndReason> {
        match load_live(&self.db, self.token_id, self.grant.grant_id).await {
            Ok(live) => {
                self.apply(live);
                self.db_ok();
                Ok(())
            }
            Err(LiveFailure::End(end)) => Err(end),
            Err(LiveFailure::Db(error)) => self.db_failed(&error).await,
        }
    }

    /// Queues one frame for the client, waiting for room. Waiting is bounded: a client that does
    /// not drain its queue for `lag_timeout` ends the stream. While waiting, a grant change is
    /// still acted on, so a revocation is not held up by a slow reader.
    async fn send_frame(&mut self, frame: Bytes) -> Result<(), EndReason> {
        let deadline = tokio::time::sleep(self.tuning().lag_timeout);
        tokio::pin!(deadline);
        let tx = self.tx.clone();
        loop {
            tokio::select! {
                reserved = tx.reserve() => {
                    return match reserved {
                        Ok(slot) => {
                            slot.send(frame);
                            self.last_write = Instant::now();
                            Ok(())
                        }
                        Err(_) => Err(EndReason::ClientGone),
                    };
                }
                () = &mut deadline => return Err(EndReason::Lagged),
                signal = self.signal_rx.recv() => {
                    self.note_signal(&signal);
                    if self.force_refresh {
                        self.refresh().await?;
                    }
                }
                _ = self.shutdown_rx.changed() => return Err(EndReason::Shutdown),
            }
        }
    }

    /// Reads one batch after the stream's position and queues what the grant may see.
    async fn deliver_batch(&mut self) -> Result<Progress, EndReason> {
        let tuning = self.tuning();
        if self.live_at.elapsed() >= tuning.grant_cache_ttl {
            // The refresh that should have happened failed; nothing is delivered on stale state.
            return Ok(Progress::Idle);
        }
        let rows = match fetch_rows(&self.db, self.grant.workspace_id, self.position.seq, tuning.batch).await {
            Ok(rows) => rows,
            Err(error) => {
                self.db_failed(&error).await?;
                return Ok(Progress::Idle);
            }
        };
        if rows.is_empty() {
            self.db_ok();
            return Ok(Progress::Idle);
        }
        let full = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= tuning.batch;
        let candidacies: Vec<Candidacy> = rows
            .iter()
            .map(|row| envelope::candidacy(&self.grant, self.types, row.routing()))
            .collect();
        let wanted = distinct_projects(&candidacies);
        let visible = match visible_projects(&self.db, self.grant.user_id, self.grant.workspace_id, &wanted).await {
            Ok(visible) => visible,
            Err(error) => {
                self.db_failed(&error).await?;
                return Ok(Progress::Idle);
            }
        };
        self.db_ok();
        for (row, candidacy) in rows.into_iter().zip(candidacies) {
            self.drain_signals();
            // Never deliver on state older than the cache bound, or after the token ended: leave
            // the row unconsumed and let the main loop re-read the grant first.
            if self.force_refresh
                || self.live_at.elapsed() >= tuning.grant_cache_ttl
                || Instant::now() >= self.token_deadline
            {
                return Ok(Progress::More);
            }
            if is_deliverable(candidacy, &visible) {
                let Some(kind) = EventKind::from_name(&row.event_type) else {
                    continue;
                };
                let position = Position {
                    seq: row.seq,
                    at: row.created_at.unix_timestamp(),
                };
                let id = self
                    .key
                    .encode(self.grant.grant_id, position)
                    .ok_or(EndReason::Internal)?;
                let json = envelope::event_json(&id, kind, &row, &visible, &self.ctx.config.issuer);
                self.send_frame(envelope::event_frame(&id, &json)).await?;
                self.sent += 1;
            }
            self.position = Position {
                seq: row.seq,
                at: row.created_at.unix_timestamp(),
            };
        }
        Ok(if full { Progress::More } else { Progress::Idle })
    }

    /// Waits until something may have changed: a feed wake-up, a grant change, a timer, or the
    /// client leaving. Sends the heartbeat when the stream has been silent for too long.
    async fn wait(&mut self) -> Result<(), EndReason> {
        let tuning = self.tuning();
        let until_keepalive = tuning.keepalive.saturating_sub(self.last_write.elapsed());
        let until_token = self.token_deadline.saturating_duration_since(Instant::now());
        let wait = tuning
            .idle_tick
            .min(until_keepalive)
            .min(until_token)
            .max(Duration::from_millis(1));
        let tx = self.tx.clone();
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            changed = self.feed_rx.changed() => {
                if changed.is_err() {
                    return Err(EndReason::Internal);
                }
            }
            signal = self.signal_rx.recv() => self.note_signal(&signal),
            _ = self.shutdown_rx.changed() => return Err(EndReason::Shutdown),
            () = tx.closed() => return Err(EndReason::ClientGone),
        }
        if self.last_write.elapsed() >= tuning.keepalive {
            self.send_frame(envelope::keep_alive_frame()).await?;
        }
        Ok(())
    }

    async fn run(&mut self, opening: Vec<Bytes>) -> EndReason {
        for frame in opening {
            if let Err(end) = self.send_frame(frame).await {
                return end;
            }
        }
        loop {
            if *self.shutdown_rx.borrow() {
                return EndReason::Shutdown;
            }
            if Instant::now() >= self.token_deadline {
                return EndReason::Reauth("token_expired");
            }
            self.drain_signals();
            if (self.force_refresh || self.live_at.elapsed() >= self.tuning().grant_cache_ttl)
                && let Err(end) = self.refresh().await
            {
                return end;
            }
            match self.deliver_batch().await {
                Err(end) => return end,
                Ok(Progress::More) => continue,
                Ok(Progress::Idle) => {}
            }
            if let Err(end) = self.wait().await {
                return end;
            }
        }
    }

    /// Hands the response body its last frame (which discards whatever is still queued) and
    /// writes the audit row.
    async fn finish(&self, reason: &EndReason) {
        let last = match reason {
            EndReason::Revoked(code) => Some(envelope::control_frame(&control::revoked(code))),
            EndReason::Reauth(code) => Some(envelope::control_frame(&control::reauth(code))),
            EndReason::Lagged => self
                .cursor_text()
                .map(|cursor| envelope::control_frame(&control::lagged(&cursor))),
            EndReason::ClientGone | EndReason::Shutdown | EndReason::Internal => None,
        };
        self.close.send_replace(last);
        let record = AuditRecord::new(AuditEvent::StreamClosed)
            .with_client(&self.client_id)
            .with_grant(self.grant.grant_id)
            .with_token(self.token_id)
            .with_user(self.grant.user_id)
            .with_workspace(self.grant.workspace_id)
            .with_detail("reason", DetailValue::Code(reason.audit_code()))
            .with_detail("events_sent", DetailValue::Count(self.sent));
        if let Err(error) = audit::write(&self.db, &record).await {
            tracing::warn!(grant_id = %self.grant.grant_id, error = %error, "could not write the stream_closed audit row");
        }
    }
}

/// The response body: queued frames in order, then the closing frame. The permit rides along so
/// the open-stream count follows the life of the connection.
fn body(rx: mpsc::Receiver<Bytes>, close: watch::Receiver<Option<Bytes>>, permit: Permit) -> Body {
    struct State {
        rx: mpsc::Receiver<Bytes>,
        close: watch::Receiver<Option<Bytes>>,
        done: bool,
        _permit: Permit,
    }
    let state = State {
        rx,
        close,
        done: false,
        _permit: permit,
    };
    Body::from_stream(futures_util::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        tokio::select! {
            biased;
            _ = state.close.changed() => {
                state.done = true;
                let last = state.close.borrow().clone();
                last.map(|frame| (Ok::<Bytes, Infallible>(frame), state))
            }
            frame = state.rx.recv() => frame.map(|frame| (Ok(frame), state)),
        }
    }))
}

// -----------------------------------------------------------------------------------------------
// The request
// -----------------------------------------------------------------------------------------------

fn bearer_token(headers: &HeaderMap) -> Result<&str, OAuthBearerError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let Some(first) = values.next() else {
        return Err(OAuthBearerError::invalid_token("a bearer token is required"));
    };
    if values.next().is_some() {
        return Err(OAuthBearerError::invalid_request(
            "exactly one Authorization header is required",
        ));
    }
    let text = first
        .to_str()
        .map_err(|_| OAuthBearerError::invalid_token("the Authorization header is not a bearer token"))?;
    match text.split_once(' ') {
        Some((scheme, token)) if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() && !token.contains(' ') => {
            Ok(token)
        }
        _ => Err(OAuthBearerError::invalid_token(
            "the Authorization header is not a bearer token",
        )),
    }
}

/// `types=a,b`: a comma separated subset of the catalogue.
fn parse_types(text: &str) -> Result<TypeFilter, OAuthBearerError> {
    let mut kinds = Vec::new();
    for name in text.split(',') {
        kinds.push(
            EventKind::from_name(name.trim())
                .ok_or(OAuthBearerError::invalid_request("types names an unknown event type"))?,
        );
    }
    Ok(TypeFilter::from_kinds(kinds))
}

fn presented_cursor<'a>(headers: &'a HeaderMap, query: &'a HashMap<String, String>) -> Option<&'a str> {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            query
                .get("cursor")
                .map(String::as_str)
                .filter(|value| !value.is_empty())
        })
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn limit_error(limit: LimitExceeded) -> OAuthBearerError {
    tracing::info!(limit = ?limit, "event stream refused: too many open streams");
    OAuthBearerError::rate_limited(LIMIT_RETRY_AFTER_SECS)
}

async fn stream_endpoint(
    State(state): State<AppState>,
    Extension(ctx): Extension<Arc<StreamContext>>,
    request: Request,
) -> Response {
    match open(&state, &ctx, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

/// Authenticates the request, checks scope, rate and limits, plans the start, and returns the
/// streaming response. Everything before the response is an ordinary error with a real status.
async fn open(state: &AppState, ctx: &Arc<StreamContext>, request: Request) -> Result<Response, OAuthBearerError> {
    let (parts, _body) = request.into_parts();
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
    let with_metadata = |error: OAuthBearerError| match &ctx.resource_metadata {
        Some(url) => error.with_resource_metadata(url.as_str()),
        None => error,
    };
    if query_carries_access_token(parts.uri.query().unwrap_or("")) {
        return Err(OAuthBearerError::invalid_request(
            "access tokens are accepted only in the Authorization header",
        ));
    }
    let query = Query::<HashMap<String, String>>::try_from_uri(&parts.uri)
        .map_err(|_| OAuthBearerError::invalid_request("the query string is not valid"))?
        .0;
    let token = bearer_token(&parts.headers).map_err(with_metadata)?;
    let ip = rate::client_address(&parts.headers, peer);

    // Only API-audience tokens open a stream: no forwarding credential is accepted here.
    let validated = match validate_access_token(&state.db, token, AudienceRule::Api(ForwardCredential::absent())).await
    {
        Ok(validated) => validated,
        Err(failure) => {
            if failure.reason.is_audited() {
                let mut record = failure.audit_record().with_client_ip(ip);
                if let Some(agent) = user_agent(&parts.headers) {
                    record = record.with_user_agent(&agent);
                }
                if let Err(error) = audit::write_token_rejected_sampled(&state.db, &record).await {
                    tracing::warn!(error = %error, "could not write the token_rejected audit row");
                }
            }
            return Err(failure.to_error(ctx.resource_metadata.as_deref()));
        }
    };

    let permitted = TypeFilter::permitted_by(validated.scopes);
    if permitted.is_empty() {
        return Err(OAuthBearerError::insufficient_scope(
            "the event stream needs the notifications:read or tasks:read scope",
            Some(ScopeSet::from_scopes(&[Scope::NotificationsRead, Scope::TasksRead])),
        ));
    }
    let requested = match query.get("types") {
        Some(text) => parse_types(text)?,
        None => permitted,
    };
    let types = permitted.intersect(requested);
    if types.is_empty() {
        let needed: Vec<Scope> = EventKind::ALL
            .into_iter()
            .filter(|kind| requested.contains(*kind))
            .map(EventKind::required_scope)
            .collect();
        return Err(OAuthBearerError::insufficient_scope(
            "the grant does not allow the requested event types",
            Some(ScopeSet::from_scopes(&needed)),
        ));
    }

    let key = ctx
        .cursor_key(state.cfg.jwt_secret.expose())
        .ok_or_else(OAuthBearerError::server_error)?;
    let presented = presented_cursor(&parts.headers, &query)
        .map(|text| key.decode(validated.grant_id, text))
        .transpose()
        .map_err(|error| {
            OAuthBearerError::invalid_request(match error {
                CursorError::Malformed | CursorError::Invalid => "the cursor is not valid for this connection",
            })
        })?;

    match rate::enforce(
        &state.db,
        RateBucket::StreamConnect,
        &RateKey::from_id(validated.grant_id),
    )
    .await
    {
        Ok(()) => {}
        Err(RateError::Limited(limited)) => return Err(limited.into()),
        Err(RateError::Store(error)) => {
            tracing::error!(error = %error, "stream connection rate store failed");
            return Err(OAuthBearerError::server_error());
        }
    }
    let permit = ctx
        .hub
        .try_acquire(validated.grant_id, validated.user_id)
        .map_err(limit_error)?;

    let internal = |error: &DbErr| {
        tracing::error!(error = %error, "could not open an event stream");
        OAuthBearerError::server_error()
    };
    // The first read of the live state also closes the gap between validation and the stream.
    let live = match load_live(&state.db, validated.token_id, validated.grant_id).await {
        Ok(live) => live,
        Err(LiveFailure::End(_)) => return Err(failure_after_validation(ctx)),
        Err(LiveFailure::Db(error)) => return Err(internal(&error)),
    };
    let grant = GrantView {
        grant_id: validated.grant_id,
        user_id: validated.user_id,
        workspace_id: validated.workspace_id,
        scopes: validated.scopes,
        projects: live.projects.clone(),
    };
    let now = Timestamp::now();
    let plan = plan_start(&state.db, ctx, &grant, types, presented, now.unix_timestamp())
        .await
        .map_err(|error| internal(&error))?;

    ctx.hub.ensure_listener(&state.db);
    let started = Instant::now();
    let (tx, rx) = mpsc::channel(ctx.hub.tuning.channel_capacity.max(1));
    let (close_tx, close_rx) = watch::channel(None);
    let mut opening = Vec::with_capacity(2);
    let cursor = key
        .encode(grant.grant_id, plan.after)
        .ok_or_else(OAuthBearerError::server_error)?;
    opening.push(envelope::control_frame(&control::hello(
        grant.grant_id,
        grant.workspace_id,
        &cursor,
        &envelope::millis(now),
    )));
    if let Some(reason) = plan.resync {
        let oldest =
            Timestamp::from_unix_timestamp(now.unix_timestamp().saturating_sub(ctx.window_secs()), 0).unwrap_or(now);
        opening.push(envelope::control_frame(&control::resync_required(
            reason.as_str(),
            &envelope::millis(oldest),
        )));
    }

    let mut record = AuditRecord::new(AuditEvent::StreamOpened)
        .with_client(&validated.client_id)
        .with_grant(validated.grant_id)
        .with_token(validated.token_id)
        .with_user(validated.user_id)
        .with_workspace(validated.workspace_id)
        .with_client_ip(ip)
        .with_detail("resumed", DetailValue::Flag(presented.is_some()))
        .with_detail("resync", DetailValue::Flag(plan.resync.is_some()));
    if let Some(agent) = user_agent(&parts.headers) {
        record = record.with_user_agent(&agent);
    }
    if let Err(error) = audit::write(&state.db, &record).await {
        tracing::warn!(grant_id = %validated.grant_id, error = %error, "could not write the stream_opened audit row");
    }

    let mut connection = Connection {
        db: state.db.clone(),
        ctx: Arc::clone(ctx),
        key,
        client_id: Arc::clone(&validated.client_id),
        token_id: validated.token_id,
        grant,
        types,
        position: plan.after,
        live_at: started,
        token_deadline: started + live.token_remaining,
        force_refresh: false,
        tx,
        close: close_tx,
        feed_rx: ctx.hub.feed.subscribe(),
        signal_rx: ctx.hub.signals.subscribe(),
        shutdown_rx: ctx.hub.shutdown.subscribe(),
        last_write: started,
        sent: 0,
        failing_since: None,
    };
    tokio::spawn(async move {
        let reason = connection.run(opening).await;
        connection.finish(&reason).await;
    });

    let mut response = Response::new(body(rx, close_rx, permit));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

/// The token passed validation a moment ago but its grant ended since: answered like any other
/// refused token.
fn failure_after_validation(ctx: &StreamContext) -> OAuthBearerError {
    let error = OAuthBearerError::invalid_token("the access token is invalid or has expired");
    match &ctx.resource_metadata {
        Some(url) => error.with_resource_metadata(url.as_str()),
        None => error,
    }
}

async fn not_found() -> Response {
    OAuthError::not_found().into_response()
}

/// The stream route with explicit limits and timings, and the hub that owns them. Production
/// code uses [`routes`]; tests use this to shorten timers and to reach the hub.
pub fn routes_with(
    config: Arc<OAuthConfig>,
    limits: StreamLimits,
    tuning: StreamTuning,
) -> (Router<AppState>, Arc<Hub>) {
    let hub = Hub::new(limits, tuning);
    HUBS.lock().push(Arc::downgrade(&hub));
    let context = Arc::new(StreamContext {
        resource_metadata: api_resource_metadata_url(&config),
        config,
        hub: Arc::clone(&hub),
        key: OnceLock::new(),
    });
    let router = Router::new()
        .route(STREAM_PATH, get(stream_endpoint))
        .layer(Extension(context));
    (router, hub)
}

/// The stream route. Without `[oauth] issuer` the path answers 404.
pub fn routes(config: Option<&Arc<OAuthConfig>>) -> Router<AppState> {
    config.map_or_else(
        || Router::new().route(STREAM_PATH, any(not_found)),
        |config| {
            routes_with(
                Arc::clone(config),
                StreamLimits::from_config(config),
                StreamTuning::default(),
            )
            .0
        },
    )
}
