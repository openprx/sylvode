//! `GET /api/v1/bot/change-hints`: a stream of change hints for one bot token.
//!
//! A hint says that something a bot may want to read again changed. It carries ids and a kind,
//! never content: the bot reads the object through the ordinary, authorised read paths, so every
//! pushed state is also pullable.
//!
//! # Where hints come from
//!
//! Database triggers (migration `0132`) call `pg_notify` after a domain row changed, on three
//! channels: [`HINT_CHANNEL`] (a task, an issue or the tool set changed), [`ACCESS_CHANNEL`] (the
//! projects a bot may read changed) and [`TOKEN_CHANNEL`] (a bot's token or state changed). A
//! notification is delivered at commit, so a hint never precedes the state it announces. This
//! module keeps one shared `LISTEN` connection per process and fans the parsed notifications out
//! on a bounded [`broadcast`] channel; a stream that falls behind it ends with `lagged`.
//!
//! # What a stream guarantees
//!
//! * **Authentication.** The route sits behind `bot_or_user_auth_middleware`; only a bot token
//!   gets past [`change_hints`] (a person and a connected application get 403).
//! * **Delivery filter, evaluated at delivery.** A hint is sent only when its workspace is the
//!   bot's and, for an issue or a tool change, its project is one the bot can read now
//!   ([`readable_projects`], the rule every read path uses; cached at most
//!   [`HintTuning::readable_ttl`] per stream and refreshed at once on an access notification).
//!   A task hint is sent only to the bot that owns the task.
//! * **Revocation.** The token is looked up again by its hash every [`HintTuning::tick`] (3 s)
//!   and at once on a token notification for this bot; when it fails the stream sends `revoked`
//!   and ends. At `expires_at` it sends `revoked` with `{"reason":"expired"}` and ends.
//! * **Bounded memory.** Per stream: [`HintTuning::channel_capacity`] queued frames; a client that
//!   does not drain for [`HintTuning::lag_timeout`] is sent `lagged` and ended. Per process: one
//!   broadcast of [`BROADCAST_CAPACITY`] hints. Per bot: [`STREAMS_PER_BOT`] streams; per process:
//!   `[api] max_change_hint_streams`.
//!
//! Nothing here logs a token or a payload: log fields are ids, counts and fixed reason codes.

#[cfg(test)]
mod db_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::{
    Extension,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use parking_lot::Mutex;
use platform::app::AppState;
use sea_orm::{DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::Instant;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::bot_auth::{BotAuthContext, sha256_hex};
use crate::middleware::scoped_bot::{ProjectFilter, readable_projects};
use crate::routes::auth::{extract_bearer_token, extract_cookie_token};

/// Path of the stream endpoint.
pub const HINTS_PATH: &str = "/api/v1/bot/change-hints";
/// A task, an issue or the tool set changed.
pub const HINT_CHANNEL: &str = "sv_change_hint";
/// The projects a bot may read changed.
pub const ACCESS_CHANNEL: &str = "sv_bot_access_changed";
/// A bot's token, its state or its user changed.
pub const TOKEN_CHANNEL: &str = "sv_bot_token_changed";

/// Concurrent streams of one bot.
pub const STREAMS_PER_BOT: usize = 64;
/// Parsed notifications the shared channel holds for a slow stream before it is declared lagging.
pub const BROADCAST_CAPACITY: usize = 4_096;
/// `Retry-After` of a refusal because a limit of open streams is reached.
const LIMIT_RETRY_AFTER_SECS: u64 = 30;
/// Longest a new stream waits for the shared `LISTEN` to be established before it starts.
const LISTENER_READY_WAIT: Duration = Duration::from_secs(5);

const KEEPALIVE_FRAME: &str = ": keep-alive\n\n";

/// The open-stream limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HintLimits {
    pub per_bot: usize,
    pub per_process: usize,
}

impl HintLimits {
    /// The contract defaults, with the per-process cap taken from `[api] max_change_hint_streams`.
    pub fn from_runtime() -> Self {
        Self {
            per_bot: STREAMS_PER_BOT,
            per_process: usize::try_from(crate::config::runtime().api.max_change_hint_streams).unwrap_or(usize::MAX),
        }
    }
}

/// Timing and sizing of a stream. The defaults are the contract's; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct HintTuning {
    /// Whether to run the shared `LISTEN` connection. Off, streams rely on their timers alone.
    pub listen: bool,
    /// Silence after which a `: keep-alive` comment is sent.
    pub keepalive: Duration,
    /// How often a stream looks up its token and its readable projects again.
    pub tick: Duration,
    /// Oldest readable-project set a hint may be delivered on.
    pub readable_ttl: Duration,
    /// Frames a stream may have queued for its client.
    pub channel_capacity: usize,
    /// How long a full queue is waited on before the client is declared lagging.
    pub lag_timeout: Duration,
    /// How long a database that keeps failing is tolerated before a stream gives up.
    pub db_give_up: Duration,
}

impl Default for HintTuning {
    fn default() -> Self {
        Self {
            listen: true,
            keepalive: Duration::from_secs(15),
            tick: Duration::from_secs(3),
            readable_ttl: Duration::from_secs(2),
            channel_capacity: 256,
            lag_timeout: Duration::from_secs(30),
            db_give_up: Duration::from_secs(30),
        }
    }
}

// -----------------------------------------------------------------------------------------------
// Hints
// -----------------------------------------------------------------------------------------------

/// What a `sv_change_hint` notification says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    /// A task of `bot` changed.
    Task {
        id: Uuid,
        project: Option<Uuid>,
        workspace: Uuid,
        bot: Uuid,
    },
    /// A work item or one of its comments changed.
    Issue { id: Uuid, project: Uuid, workspace: Uuid },
    /// The tool set changed, for one project or (no project) for everyone.
    Tools {
        project: Option<Uuid>,
        workspace: Option<Uuid>,
    },
}

#[derive(Deserialize)]
struct RawHint {
    k: String,
    id: Option<Uuid>,
    p: Option<Uuid>,
    w: Option<Uuid>,
    b: Option<Uuid>,
}

#[derive(Deserialize)]
struct RawBot {
    b: Uuid,
}

/// Parses a `sv_change_hint` payload. `None` when it is not one the triggers write.
pub fn parse_hint(payload: &str) -> Option<Hint> {
    let raw: RawHint = serde_json::from_str(payload).ok()?;
    match raw.k.as_str() {
        "task" => Some(Hint::Task {
            id: raw.id?,
            project: raw.p,
            workspace: raw.w?,
            bot: raw.b?,
        }),
        "issue" => Some(Hint::Issue {
            id: raw.id?,
            project: raw.p?,
            workspace: raw.w?,
        }),
        "tools" => Some(Hint::Tools {
            project: raw.p,
            workspace: raw.w,
        }),
        _ => None,
    }
}

/// Parses a `sv_bot_access_changed` or `sv_bot_token_changed` payload into the bot id.
pub fn parse_bot(payload: &str) -> Option<Uuid> {
    serde_json::from_str::<RawBot>(payload).ok().map(|raw| raw.b)
}

/// Something a stream wakes on.
#[derive(Clone, Copy, Debug)]
enum Signal {
    Hint(Hint),
    Access(Uuid),
    Token(Uuid),
    /// The listener (re)connected or a payload could not be understood: anything may be missed.
    Reset,
}

// -----------------------------------------------------------------------------------------------
// Frames
// -----------------------------------------------------------------------------------------------

fn frame(event: &str, data: &serde_json::Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

fn hello_frame(bot_id: Uuid, workspace_id: Uuid) -> Bytes {
    frame(
        "hello",
        &serde_json::json!({"bot_id": bot_id.to_string(), "workspace_id": workspace_id.to_string()}),
    )
}

/// The frames a hint turns into for a bot whose workspace is `workspace_id`, given whether the
/// hint's project is readable. `None` for a hint the bot is not to see.
fn frames_for(hint: Hint, bot_id: Uuid, workspace_id: Uuid, project_readable: impl Fn(Uuid) -> bool) -> Vec<Bytes> {
    match hint {
        Hint::Task { id, workspace, bot, .. } if workspace == workspace_id && bot == bot_id => vec![
            frame("task", &serde_json::json!({"id": id.to_string()})),
            frame("inbox", &serde_json::json!({})),
        ],
        Hint::Issue { id, project, workspace } if workspace == workspace_id && project_readable(project) => {
            vec![frame("issue", &serde_json::json!({"id": id.to_string()}))]
        }
        Hint::Tools { project, workspace }
            if workspace.is_none_or(|w| w == workspace_id) && project.is_none_or(&project_readable) =>
        {
            vec![frame("tools", &serde_json::json!({}))]
        }
        _ => Vec::new(),
    }
}

// -----------------------------------------------------------------------------------------------
// The hub: limits, fan-out and the shared LISTEN connection
// -----------------------------------------------------------------------------------------------

#[derive(Default)]
struct Registry {
    total: usize,
    bots: HashMap<Uuid, usize>,
}

/// Which limit refused a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitExceeded {
    Bot,
    Process,
}

/// One open stream's place in the limits. Released when dropped, which happens when the response
/// body is dropped: a connection counts for as long as its socket does.
pub struct Permit {
    hub: Arc<HintHub>,
    bot_id: Uuid,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut registry = self.hub.registry.lock();
        registry.total = registry.total.saturating_sub(1);
        if let Some(count) = registry.bots.get_mut(&self.bot_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                registry.bots.remove(&self.bot_id);
            }
        }
    }
}

/// The per-process state shared by every hint stream.
pub struct HintHub {
    limits: HintLimits,
    tuning: HintTuning,
    registry: Mutex<Registry>,
    signals: broadcast::Sender<Signal>,
    shutdown: watch::Sender<bool>,
    ready: watch::Sender<bool>,
    listener_started: AtomicBool,
}

static HUBS: Mutex<Vec<Weak<HintHub>>> = Mutex::new(Vec::new());

/// Ends every open hint stream of this process with `event: shutdown`. The API's shutdown path
/// calls it before it waits for connections to drain.
pub fn begin_shutdown() {
    let hubs: Vec<Arc<HintHub>> = {
        let mut registered = HUBS.lock();
        registered.retain(|hub| hub.strong_count() > 0);
        registered.iter().filter_map(Weak::upgrade).collect()
    };
    for hub in hubs {
        hub.shutdown.send_replace(true);
    }
}

impl HintHub {
    pub fn new(limits: HintLimits, tuning: HintTuning) -> Arc<Self> {
        let hub = Arc::new(Self {
            limits,
            tuning,
            registry: Mutex::new(Registry::default()),
            signals: broadcast::channel(BROADCAST_CAPACITY).0,
            shutdown: watch::channel(false).0,
            ready: watch::channel(false).0,
            listener_started: AtomicBool::new(false),
        });
        HUBS.lock().push(Arc::downgrade(&hub));
        hub
    }

    /// The hub of the running server: contract limits and timings.
    pub fn from_runtime() -> Arc<Self> {
        Self::new(HintLimits::from_runtime(), HintTuning::default())
    }

    /// Takes a place for a stream of `bot_id`, or names the limit that is reached.
    pub fn try_acquire(self: &Arc<Self>, bot_id: Uuid) -> Result<Permit, LimitExceeded> {
        let mut registry = self.registry.lock();
        if registry.total >= self.limits.per_process {
            return Err(LimitExceeded::Process);
        }
        if registry.bots.get(&bot_id).copied().unwrap_or(0) >= self.limits.per_bot {
            return Err(LimitExceeded::Bot);
        }
        registry.total += 1;
        *registry.bots.entry(bot_id).or_insert(0) += 1;
        drop(registry);
        Ok(Permit {
            hub: Arc::clone(self),
            bot_id,
        })
    }

    /// Streams currently open in this process.
    pub fn open_streams(&self) -> usize {
        self.registry.lock().total
    }

    /// Ends every stream of this hub with `event: shutdown`.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    fn signal(&self, signal: Signal) {
        // No receiver means no stream is open; there is nobody to wake.
        let _ = self.signals.send(signal);
    }

    /// Starts the shared listener once and waits until it listens (or the wait ran out, in which
    /// case the streams' own timers still cover revocation and expiry).
    async fn ensure_listening(self: &Arc<Self>, db: &DatabaseConnection) {
        if !self.tuning.listen {
            return;
        }
        if !self.listener_started.swap(true, Ordering::AcqRel) {
            let hub = Arc::clone(self);
            let db = db.clone();
            tokio::spawn(async move { hub.listen_loop(db).await });
        }
        let mut ready = self.ready.subscribe();
        let _ = tokio::time::timeout(LISTENER_READY_WAIT, ready.wait_for(|up| *up)).await;
    }

    async fn listen_loop(self: Arc<Self>, db: DatabaseConnection) {
        use sea_orm::sqlx::postgres::PgListener;
        let mut shutdown = self.shutdown.subscribe();
        let mut connected_before = false;
        loop {
            if *shutdown.borrow() {
                return;
            }
            let connected = PgListener::connect_with(db.get_postgres_connection_pool()).await;
            let mut listener = match connected {
                Ok(listener) => listener,
                Err(error) => {
                    tracing::warn!(error = %error, "change hint listener connect failed");
                    self.ready.send_replace(false);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            if let Err(error) = listener.listen_all([HINT_CHANNEL, ACCESS_CHANNEL, TOKEN_CHANNEL]).await {
                tracing::warn!(error = %error, "change hint LISTEN failed");
                self.ready.send_replace(false);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            // Notifications sent while no listener was connected are gone.
            if connected_before {
                self.signal(Signal::Reset);
            }
            connected_before = true;
            self.ready.send_replace(true);
            loop {
                tokio::select! {
                    received = listener.recv() => match received {
                        Ok(notification) => self.dispatch(notification.channel(), notification.payload()),
                        Err(error) => {
                            tracing::warn!(error = %error, "change hint LISTEN dropped; reconnecting");
                            self.ready.send_replace(false);
                            break;
                        }
                    },
                    _ = shutdown.changed() => return,
                }
            }
        }
    }

    fn dispatch(&self, channel: &str, payload: &str) {
        let signal = match channel {
            HINT_CHANNEL => parse_hint(payload).map(Signal::Hint),
            ACCESS_CHANNEL => parse_bot(payload).map(Signal::Access),
            TOKEN_CHANNEL => parse_bot(payload).map(Signal::Token),
            _ => return,
        };
        if let Some(signal) = signal {
            self.signal(signal);
        } else {
            tracing::warn!(
                channel,
                "change hint payload not understood; streams are told to re-read"
            );
            self.signal(Signal::Reset);
        }
    }
}

// -----------------------------------------------------------------------------------------------
// Token validity
// -----------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum TokenState {
    /// Still valid; the time left, when the token expires.
    Valid(Option<Duration>),
    Revoked,
    Expired,
}

#[derive(Debug, FromQueryResult)]
struct TokenRow {
    id: Uuid,
    is_active: bool,
    user_active: bool,
    remaining_ms: Option<i64>,
}

/// The bot-token lookup of `bot_or_user_auth_middleware`, by `token_hash`: the same predicate
/// (known hash, token and user active, not expired), plus the time left on the database clock.
async fn check_token(db: &DatabaseConnection, token_hash: &str, bot_id: Uuid) -> Result<TokenState, DbErr> {
    let row = TokenRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT b.id, b.is_active, COALESCE(u.is_active, true) AS user_active, \
                CASE WHEN b.expires_at IS NULL THEN NULL \
                     ELSE (EXTRACT(EPOCH FROM (b.expires_at - now())) * 1000)::bigint END AS remaining_ms \
         FROM workspace_bots b LEFT JOIN users u ON u.id = b.id \
         WHERE b.token_hash = $1",
        vec![token_hash.into()],
    ))
    .one(db)
    .await?;
    Ok(match row {
        None => TokenState::Revoked,
        Some(row) if row.id != bot_id || !row.is_active || !row.user_active => TokenState::Revoked,
        Some(row) => match row.remaining_ms {
            Some(ms) if ms <= 0 => TokenState::Expired,
            Some(ms) => TokenState::Valid(Some(Duration::from_millis(ms.unsigned_abs()))),
            None => TokenState::Valid(None),
        },
    })
}

// -----------------------------------------------------------------------------------------------
// One stream
// -----------------------------------------------------------------------------------------------

/// Why a stream ends.
#[derive(Clone, Debug, PartialEq, Eq)]
enum End {
    Revoked(Option<&'static str>),
    Lagged,
    /// A lookup failed. Tolerated for a while on a timer; fatal when a hint would be delivered
    /// on state that could not be refreshed.
    Db,
    ClientGone,
    Shutdown,
}

impl End {
    const fn code(&self) -> &'static str {
        match self {
            Self::Revoked(Some(reason)) => reason,
            Self::Revoked(None) => "revoked",
            Self::Lagged => "lagged",
            Self::Db => "database",
            Self::ClientGone => "client_closed",
            Self::Shutdown => "shutdown",
        }
    }

    fn final_frame(&self) -> Option<Bytes> {
        match self {
            Self::Revoked(None) => Some(frame("revoked", &serde_json::json!({}))),
            Self::Revoked(Some(reason)) => Some(frame("revoked", &serde_json::json!({"reason": reason}))),
            Self::Lagged | Self::Db => Some(frame("lagged", &serde_json::json!({}))),
            Self::Shutdown => Some(frame("shutdown", &serde_json::json!({}))),
            Self::ClientGone => None,
        }
    }
}

/// The readable-project set of the bot, as of `fetched`.
struct Readable {
    filter: ProjectFilter,
    fetched: Instant,
}

fn normalized(filter: ProjectFilter) -> ProjectFilter {
    match filter {
        ProjectFilter::Only(mut ids) => {
            ids.sort_unstable();
            ids.dedup();
            ProjectFilter::Only(ids)
        }
        all @ ProjectFilter::All => all,
    }
}

async fn fetch_readable(db: &DatabaseConnection, bot_id: Uuid) -> Result<Readable, ApiError> {
    Ok(Readable {
        filter: normalized(readable_projects(db, bot_id).await?),
        fetched: Instant::now(),
    })
}

struct HintStream {
    db: DatabaseConnection,
    bot_id: Uuid,
    workspace_id: Uuid,
    token_hash: String,
    tuning: HintTuning,
    signals: broadcast::Receiver<Signal>,
    shutdown: watch::Receiver<bool>,
    tx: mpsc::Sender<Bytes>,
    readable: Readable,
    expiry: Option<Instant>,
    db_failing_since: Option<Instant>,
    sent: u64,
}

async fn wait_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

impl HintStream {
    /// Queues a frame. A client that does not drain within the lag timeout is declared lagging.
    async fn send(&mut self, frame: Bytes) -> Result<(), End> {
        match tokio::time::timeout(self.tuning.lag_timeout, self.tx.send(frame)).await {
            Ok(Ok(())) => {
                self.sent += 1;
                Ok(())
            }
            Ok(Err(_)) => Err(End::ClientGone),
            Err(_) => Err(End::Lagged),
        }
    }

    async fn run(mut self) -> (End, u64) {
        let mut tick = tokio::time::interval_at(Instant::now() + self.tuning.tick, self.tuning.tick);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut keepalive = tokio::time::interval_at(Instant::now() + self.tuning.keepalive, self.tuning.keepalive);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let end = loop {
            if *self.shutdown.borrow() {
                break End::Shutdown;
            }
            let expiry = self.expiry;
            let step = tokio::select! {
                signal = self.signals.recv() => self.on_signal(signal).await,
                _ = tick.tick() => self.on_tick().await,
                _ = keepalive.tick() => self.send(Bytes::from_static(KEEPALIVE_FRAME.as_bytes())).await,
                () = wait_deadline(expiry) => Err(End::Revoked(Some("expired"))),
                _ = self.shutdown.changed() => Err(End::Shutdown),
                () = self.tx.closed() => Err(End::ClientGone),
            };
            if let Err(end) = step {
                break end;
            }
        };
        (end, self.sent)
    }

    async fn on_signal(&mut self, signal: Result<Signal, broadcast::error::RecvError>) -> Result<(), End> {
        match signal {
            Err(broadcast::error::RecvError::Lagged(_)) | Ok(Signal::Reset) => Err(End::Lagged),
            Err(broadcast::error::RecvError::Closed) => Err(End::Shutdown),
            Ok(Signal::Hint(hint)) => self.deliver(hint).await,
            Ok(Signal::Access(bot)) if bot == self.bot_id => {
                self.refresh_readable().await?;
                self.send(frame("access_changed", &serde_json::json!({}))).await
            }
            Ok(Signal::Token(bot)) if bot == self.bot_id => self.verify_token().await,
            Ok(Signal::Access(_) | Signal::Token(_)) => Ok(()),
        }
    }

    async fn on_tick(&mut self) -> Result<(), End> {
        let outcome = async {
            self.verify_token().await?;
            let before = self.readable.filter.clone();
            self.refresh_readable().await?;
            if self.readable.filter != before {
                self.send(frame("access_changed", &serde_json::json!({}))).await?;
            }
            Ok(())
        }
        .await;
        match outcome {
            Err(End::Db) => {
                // A database that failed this tick: tolerated for a while, never treated as a
                // revocation. A stream that cannot check its token for long gives up.
                let since = *self.db_failing_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.tuning.db_give_up {
                    Err(End::Db)
                } else {
                    Ok(())
                }
            }
            Ok(()) => {
                self.db_failing_since = None;
                Ok(())
            }
            other => other,
        }
    }

    /// Looks the token up again. `Err(End::Db)` is a database failure, not a verdict.
    async fn verify_token(&mut self) -> Result<(), End> {
        match check_token(&self.db, &self.token_hash, self.bot_id).await {
            Ok(TokenState::Valid(remaining)) => {
                self.expiry = remaining.map(|left| Instant::now() + left);
                Ok(())
            }
            Ok(TokenState::Revoked) => Err(End::Revoked(None)),
            Ok(TokenState::Expired) => Err(End::Revoked(Some("expired"))),
            Err(error) => {
                tracing::warn!(bot_id = %self.bot_id, error = %error, "change hint token check failed");
                Err(End::Db)
            }
        }
    }

    async fn refresh_readable(&mut self) -> Result<(), End> {
        match fetch_readable(&self.db, self.bot_id).await {
            Ok(readable) => {
                self.readable = readable;
                Ok(())
            }
            Err(error) => {
                tracing::warn!(bot_id = %self.bot_id, error = %error, "change hint access lookup failed");
                Err(End::Db)
            }
        }
    }

    async fn deliver(&mut self, hint: Hint) -> Result<(), End> {
        if project_to_check(hint, self.workspace_id).is_some()
            && self.readable.fetched.elapsed() > self.tuning.readable_ttl
        {
            // Never deliver on a set older than the cache bound. A failed lookup ends the stream
            // rather than delivering on stale access.
            self.refresh_readable().await?;
        }
        let frames = frames_for(hint, self.bot_id, self.workspace_id, |project| {
            self.readable.filter.allows(project)
        });
        for frame in frames {
            self.send(frame).await?;
        }
        Ok(())
    }
}

/// The project whose readability decides whether `hint` is delivered to a bot of `workspace_id`.
fn project_to_check(hint: Hint, workspace_id: Uuid) -> Option<Uuid> {
    match hint {
        Hint::Issue { project, workspace, .. } if workspace == workspace_id => Some(project),
        Hint::Tools {
            project: Some(project),
            workspace,
        } if workspace.is_none_or(|w| w == workspace_id) => Some(project),
        _ => None,
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

fn limit_response(status: StatusCode, message: &'static str) -> Response {
    let body = serde_json::json!({"code": status.as_u16(), "message": message});
    let mut response = (status, axum::Json(body)).into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&LIMIT_RETRY_AFTER_SECS.to_string()).unwrap_or_else(|_| HeaderValue::from_static("30")),
    );
    response
}

/// The token the request authenticated with, as the middleware read it (header, else cookie).
fn presented_bot_token(headers: &HeaderMap) -> Option<String> {
    extract_bearer_token(headers)
        .or_else(|| extract_cookie_token(headers, "access_token"))
        .filter(|token| token.starts_with("opr_"))
}

/// `GET /api/v1/bot/change-hints`: the hint stream of the calling bot.
pub async fn change_hints(
    State(state): State<AppState>,
    Extension(hub): Extension<Arc<HintHub>>,
    bot: Option<Extension<BotAuthContext>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Some(Extension(bot)) = bot else {
        return Err(ApiError::Forbidden(
            "change hints are available to bot tokens".to_string(),
        ));
    };
    let Some(token) = presented_bot_token(&headers) else {
        return Err(ApiError::Forbidden(
            "change hints are available to bot tokens".to_string(),
        ));
    };
    if *hub.shutdown.borrow() {
        return Ok(limit_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "the server is shutting down",
        ));
    }
    let permit = match hub.try_acquire(bot.bot_id) {
        Ok(permit) => permit,
        Err(limit) => {
            tracing::info!(bot_id = %bot.bot_id, limit = ?limit, "change hint stream refused: limit reached");
            return Ok(limit_response(
                StatusCode::TOO_MANY_REQUESTS,
                "too many open change-hint streams",
            ));
        }
    };

    hub.ensure_listening(&state.db).await;
    // Subscribe before the first lookups, so nothing committed after them is missed.
    let signals = hub.signals.subscribe();
    let token_hash = sha256_hex(&token);
    let expiry = match check_token(&state.db, &token_hash, bot.bot_id)
        .await
        .map_err(ApiError::internal_database_failure)?
    {
        TokenState::Valid(remaining) => remaining.map(|left| Instant::now() + left),
        TokenState::Revoked | TokenState::Expired => {
            return Err(ApiError::Unauthorized("bot token is no longer valid".to_string()));
        }
    };
    let readable = fetch_readable(&state.db, bot.bot_id).await?;

    let (tx, rx) = mpsc::channel(hub.tuning.channel_capacity.max(2));
    let (close_tx, close_rx) = watch::channel(None);
    // Cannot fail: the queue is empty and its receiver is alive.
    let _ = tx.try_send(hello_frame(bot.bot_id, bot.workspace_id));

    let stream = HintStream {
        db: state.db.clone(),
        bot_id: bot.bot_id,
        workspace_id: bot.workspace_id,
        token_hash,
        tuning: hub.tuning,
        signals,
        shutdown: hub.shutdown.subscribe(),
        tx,
        readable,
        expiry,
        db_failing_since: None,
        sent: 0,
    };
    let bot_id = bot.bot_id;
    tracing::debug!(bot_id = %bot_id, "change hint stream opened");
    tokio::spawn(async move {
        let (end, sent) = stream.run().await;
        tracing::info!(bot_id = %bot_id, reason = end.code(), events_sent = sent, "change hint stream closed");
        // The receiver is gone when the client left; nothing is left to tell it then.
        let _ = close_tx.send(end.final_frame());
        // Keep the sender alive until the body has read the closing frame.
        close_tx.closed().await;
    });

    let mut response = Response::new(body(rx, close_rx, permit));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}
