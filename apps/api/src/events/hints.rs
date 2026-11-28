//! `GET /api/v1/bot/change-hints`: a stream of change hints for one bot token.
//!
//! A hint says that something a bot may want to read again changed. It carries ids and a kind,
//! never content: the bot reads the object through the ordinary, authorised read paths, so every
//! pushed state is also pullable.
//!
//! # Where hints come from
//!
//! Database triggers (migration `0132`) call `pg_notify` after a domain row changed, on three
//! channels: [`HINT_CHANNEL`] (a task, an issue, a draft or the tool set changed), [`ACCESS_CHANNEL`] (the
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
//! * **Revocation.** The token is looked up again by its hash every [`HintTuning::tick`] (2 s)
//!   and at once on a token notification for this bot (the lookups of one bot are shared by its
//!   streams, see [`HintTuning::shared_ttl`]); when it fails the stream sends `revoked`
//!   and ends. At `expires_at` it sends `revoked` with `{"reason":"expired"}` and ends.
//! * **Bounded memory, never blocking.** Per stream: [`HintTuning::channel_capacity`] queued
//!   frames. A stream never waits for its client: a frame that finds the queue full is dropped,
//!   the time is recorded, and a client whose queue stays full for [`HintTuning::lag_timeout`] (or
//!   that lost a frame and did not drain by then) is sent `lagged` and ended. Shutdown, expiry,
//!   revocation and the lag deadline are polled before the signals. A client that lost a frame
//!   (a dropped hint, a lagging broadcast) is therefore told it must re-read: it learns this
//!   within [`HintTuning::lag_timeout`] (30 s), when the stream sends `lagged` and ends. Per process: one
//!   broadcast of [`BROADCAST_CAPACITY`] hints. Per bot: [`STREAMS_PER_BOT`] streams; per
//!   workspace: `[api] max_change_hint_streams_per_workspace`, so the bots of one workspace cannot
//!   take every place of the process; per process: `[api] max_change_hint_streams`. A stream's place is released when its task ends. A second,
//!   larger cap ([`BODY_CAP_FACTOR`] times both) counts response bodies until they are dropped, so
//!   a client that keeps the socket of an ended stream open cannot exceed it; both refuse with
//!   `429` and `Retry-After`.
//! * **One workspace cannot flood the broadcast.** Per window of one [`HintTuning::tick`], at
//!   most [`WORKSPACE_BURST`] issue, draft and task hints of a workspace pass, and within that at
//!   most [`BURST`] per [`Scope`] (an issue or draft hint: its project; a task hint: its bot).
//!   Hints over a limit are dropped and replaced by one overflow signal per limit and window. A
//!   scope's overflow ends, with `lagged` (clients re-read), only the streams that could have
//!   received the held-back hints: those that can read the project, or the bot's own streams. The
//!   workspace's overflow ends every stream of that workspace. So a workspace puts at most
//!   `WORKSPACE_BURST` hints, one workspace overflow and at most
//!   `WORKSPACE_BURST / BURST` scope overflows into the shared broadcast per window.
//!
//! Nothing here logs a token or a payload: log fields are ids, counts and fixed reason codes.

#[cfg(test)]
mod db_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use crate::middleware::bot_auth::{BotAuthContext, BotPermission, bot_permissions_allow, sha256_hex};
use crate::middleware::scoped_bot::{ProjectFilter, readable_projects};
use crate::routes::auth::{extract_bearer_token, extract_cookie_token};

/// Path of the stream endpoint.
pub const HINTS_PATH: &str = "/api/v1/bot/change-hints";
/// A task, an issue, a draft or the tool set changed.
pub const HINT_CHANNEL: &str = "sv_change_hint";
/// The projects a bot may read changed.
pub const ACCESS_CHANNEL: &str = "sv_bot_access_changed";
/// A bot's token, its state or its user changed.
pub const TOKEN_CHANNEL: &str = "sv_bot_token_changed";

/// Concurrent streams of one bot.
pub const STREAMS_PER_BOT: usize = 64;
/// How many times the stream limits an open response body may be held: a second, larger cap on
/// bodies that are still open, counted until the body is dropped (see [`BodyPermit`]).
pub const BODY_CAP_FACTOR: usize = 2;
/// Issue, draft or task hints of one [`Scope`] (one project, or one bot) that pass the hub within
/// one coalescing window (one [`HintTuning::tick`]). The rest of that window's hints of the scope
/// are replaced by a single overflow signal, which ends with `lagged` only the streams that could
/// have received them. Bounded together with [`WORKSPACE_BURST`].
pub const BURST: usize = 32;
/// Issue, draft and task hints of one workspace, over all its scopes, that are counted as passing
/// within one coalescing window. Checked before [`BURST`]; the rest of that window's hints of the
/// workspace are replaced by a single overflow signal that ends every stream of the workspace.
pub const WORKSPACE_BURST: usize = 256;
/// Parsed notifications the shared channel holds for a slow stream before it is declared lagging.
pub const BROADCAST_CAPACITY: usize = 4_096;
/// `Retry-After` of a refusal because a limit of open streams is reached.
const LIMIT_RETRY_AFTER_SECS: u64 = 30;
/// Longest a new stream waits for the shared `LISTEN` to be established before it starts.
const LISTENER_READY_WAIT: Duration = Duration::from_secs(5);
/// Longest the spawned task of a finished stream waits for the response body to read the closing
/// frame. The stream's place in the limits is released before this wait.
const CLOSE_GRACE: Duration = Duration::from_secs(10);

const KEEPALIVE_FRAME: &str = ": keep-alive\n\n";

/// The open-stream limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HintLimits {
    pub per_bot: usize,
    pub per_workspace: usize,
    pub per_process: usize,
}

impl HintLimits {
    /// The contract defaults, with the per-workspace and per-process caps taken from `[api]`
    /// (`max_change_hint_streams_per_workspace`, `max_change_hint_streams`).
    pub fn from_runtime() -> Self {
        Self {
            per_bot: STREAMS_PER_BOT,
            per_workspace: usize::try_from(crate::config::runtime().api.max_change_hint_streams_per_workspace)
                .unwrap_or(usize::MAX),
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
    /// How long one stream's token or readable-set lookup is reused by the other streams of the
    /// same bot on their ticks. A token or access notification discards it at once.
    pub shared_ttl: Duration,
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
            // With LISTEN off a revocation is found by the next tick, which may read a lookup
            // up to `shared_ttl` old: worst case `tick + shared_ttl` = 3 s, the contract bound.
            tick: Duration::from_secs(2),
            readable_ttl: Duration::from_secs(2),
            shared_ttl: Duration::from_secs(1),
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
    /// An artefact draft was created or changed state.
    Draft { id: Uuid, project: Uuid, workspace: Uuid },
    /// A package upload draft (it belongs to the workspace, not to a project) was created or
    /// changed state: only the bot that submitted it is told.
    UploadDraft { id: Uuid, workspace: Uuid, bot: Uuid },
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
        "draft" => match raw.p {
            Some(project) => Some(Hint::Draft {
                id: raw.id?,
                project,
                workspace: raw.w?,
            }),
            None => Some(Hint::UploadDraft {
                id: raw.id?,
                workspace: raw.w?,
                bot: raw.b?,
            }),
        },
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

/// What a coalescing window counts hints of, and which streams an overflow of it concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Every counted hint of one workspace: concerns all streams of the workspace.
    Workspace(Uuid),
    /// Issue and draft hints of one project: concerns the streams that can read the project.
    Project { workspace: Uuid, project: Uuid },
    /// Task hints for one bot: concerns that bot's streams (task delivery does not depend on
    /// readable projects).
    Bot { workspace: Uuid, bot: Uuid },
}

/// Something a stream wakes on.
#[derive(Clone, Copy, Debug)]
enum Signal {
    Hint(Hint),
    Access(Uuid),
    Token(Uuid),
    /// More hints than [`BURST`] came from one [`Scope`], or than [`WORKSPACE_BURST`] from one
    /// workspace, within one window: some were not forwarded, so the streams that could have
    /// received them must re-read.
    Overflow(Scope),
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
        Hint::Draft { id, project, workspace } if workspace == workspace_id && project_readable(project) => {
            vec![frame("draft", &serde_json::json!({"id": id.to_string()}))]
        }
        Hint::UploadDraft { id, workspace, bot } if workspace == workspace_id && bot == bot_id => {
            vec![frame("draft", &serde_json::json!({"id": id.to_string()}))]
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
    workspaces: HashMap<Uuid, usize>,
}

impl Registry {
    fn acquire(&mut self, who: Holder, caps: HintLimits) -> Result<(), LimitExceeded> {
        if self.total >= caps.per_process {
            return Err(LimitExceeded::Process);
        }
        if self.workspaces.get(&who.workspace_id).copied().unwrap_or(0) >= caps.per_workspace {
            return Err(LimitExceeded::Workspace);
        }
        if self.bots.get(&who.bot_id).copied().unwrap_or(0) >= caps.per_bot {
            return Err(LimitExceeded::Bot);
        }
        self.total += 1;
        *self.bots.entry(who.bot_id).or_insert(0) += 1;
        *self.workspaces.entry(who.workspace_id).or_insert(0) += 1;
        Ok(())
    }

    /// Gives a place back. `true` when it was the bot's last one.
    fn release(&mut self, who: Holder) -> bool {
        self.total = self.total.saturating_sub(1);
        if self.workspaces.get_mut(&who.workspace_id).is_some_and(|count| {
            *count = count.saturating_sub(1);
            *count == 0
        }) {
            self.workspaces.remove(&who.workspace_id);
        }
        let last = self.bots.get_mut(&who.bot_id).is_some_and(|count| {
            *count = count.saturating_sub(1);
            *count == 0
        });
        if last {
            self.bots.remove(&who.bot_id);
        }
        last
    }
}

/// Whose place a stream or an open body holds in the limits.
#[derive(Clone, Copy, Debug)]
struct Holder {
    bot_id: Uuid,
    workspace_id: Uuid,
}

/// The coalescing windows of the workspaces and scopes that sent counted hints recently, and the
/// time they were last pruned.
#[derive(Default)]
struct Windows(HashMap<Scope, Window>, Option<Instant>);

/// A hint that was held back: the limit that held it, whether it is the first of the window to be
/// held by that limit (so the overflow signal is due), and when that window started.
#[derive(Clone, Copy, Debug)]
struct Held {
    scope: Scope,
    first: bool,
    started: Instant,
}

impl Windows {
    /// Counts one hint of `scope` in `workspace`: first against the workspace ceiling (a hint over
    /// it does not touch the scope's window), then against the scope's burst. `None`: forward it.
    fn admit(&mut self, workspace: Uuid, scope: Scope, now: Instant, window: Duration) -> Option<Held> {
        // Pruning walks the whole map, so it runs at most once per window.
        if self.0.len() > 1_024 && self.1.is_none_or(|last| now.duration_since(last) >= window) {
            self.1 = Some(now);
            self.0
                .retain(|_, w| now.duration_since(w.started) < window || w.overflowed);
        }
        if let Some(held) = self.count(Scope::Workspace(workspace), WORKSPACE_BURST, now, window) {
            return Some(held);
        }
        self.count(scope, BURST, now, window)
    }

    fn count(&mut self, scope: Scope, limit: usize, now: Instant, window: Duration) -> Option<Held> {
        let entry = self.0.entry(scope).or_insert(Window {
            started: now,
            passed: 0,
            overflowed: false,
        });
        if now.duration_since(entry.started) >= window && !entry.overflowed {
            *entry = Window {
                started: now,
                passed: 0,
                overflowed: false,
            };
        }
        if entry.passed < limit {
            entry.passed += 1;
            return None;
        }
        let first = !entry.overflowed;
        entry.overflowed = true;
        Some(Held {
            scope,
            first,
            started: entry.started,
        })
    }
}

/// One workspace's or scope's hints in the current coalescing window.
struct Window {
    started: Instant,
    passed: usize,
    /// Hints were held back and an overflow signal is due at the end of the window.
    overflowed: bool,
}

/// Which limit refused a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitExceeded {
    Bot,
    Workspace,
    Process,
}

/// One open stream's place in the limits. Released when dropped. The stream's own task owns it, so
/// it is released when the stream ends, not when (or whether) a client reads the end.
pub struct Permit {
    hub: Arc<HintHub>,
    who: Holder,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let last = self.hub.registry.lock().release(self.who);
        if last {
            self.hub.shared.lock().remove(&self.who.bot_id);
        }
    }
}

/// One open response body's place in the body limits. Released when the body is dropped, that is
/// when the client's connection lets go of it, not when the stream's task ends: a stream that has
/// ended but whose client keeps the socket open still counts.
pub struct BodyPermit {
    hub: Arc<HintHub>,
    who: Holder,
}

impl Drop for BodyPermit {
    fn drop(&mut self) {
        self.hub.bodies.lock().release(self.who);
    }
}

/// The per-process state shared by every hint stream.
pub struct HintHub {
    limits: HintLimits,
    tuning: HintTuning,
    registry: Mutex<Registry>,
    /// Response bodies that are still open; capped at [`BODY_CAP_FACTOR`] times the stream limits.
    bodies: Mutex<Registry>,
    /// Per-scope coalescing windows of issue, draft and task hints.
    windows: Mutex<Windows>,
    /// Lookups shared by the open streams of one bot; an entry lives while the bot has a stream.
    shared: Mutex<HashMap<Uuid, Arc<BotShared>>>,
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
            bodies: Mutex::new(Registry::default()),
            windows: Mutex::new(Windows::default()),
            shared: Mutex::new(HashMap::new()),
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

    /// Takes a place for a stream of `bot_id` in `workspace_id`, or names the limit that is reached.
    pub fn try_acquire(self: &Arc<Self>, bot_id: Uuid, workspace_id: Uuid) -> Result<Permit, LimitExceeded> {
        let who = Holder { bot_id, workspace_id };
        self.registry.lock().acquire(who, self.limits)?;
        Ok(Permit {
            hub: Arc::clone(self),
            who,
        })
    }

    /// Takes a place for an open response body of `bot_id` in `workspace_id`, or names the limit
    /// that is reached. The caps are [`BODY_CAP_FACTOR`] times the stream caps, so a refusal here
    /// means the bot (or the workspace, or the process) holds many more sockets open than it has
    /// live streams.
    pub fn try_acquire_body(self: &Arc<Self>, bot_id: Uuid, workspace_id: Uuid) -> Result<BodyPermit, LimitExceeded> {
        let who = Holder { bot_id, workspace_id };
        let caps = HintLimits {
            per_bot: self.limits.per_bot.saturating_mul(BODY_CAP_FACTOR),
            per_workspace: self.limits.per_workspace.saturating_mul(BODY_CAP_FACTOR),
            per_process: self.limits.per_process.saturating_mul(BODY_CAP_FACTOR),
        };
        self.bodies.lock().acquire(who, caps)?;
        Ok(BodyPermit {
            hub: Arc::clone(self),
            who,
        })
    }

    /// Response bodies currently open in this process.
    pub fn open_bodies(&self) -> usize {
        self.bodies.lock().total
    }

    /// Streams currently open in this process.
    pub fn open_streams(&self) -> usize {
        self.registry.lock().total
    }

    /// Ends every stream of this hub with `event: shutdown`.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    fn shared_for(&self, bot_id: Uuid) -> Arc<BotShared> {
        Arc::clone(self.shared.lock().entry(bot_id).or_default())
    }

    /// Makes the cached lookups of `bot_id` (every bot for `None`) unusable, so the next check of
    /// each stream reads the database.
    fn invalidate(&self, bot_id: Option<Uuid>) {
        let shared = self.shared.lock();
        match bot_id {
            Some(bot_id) => {
                if let Some(entry) = shared.get(&bot_id) {
                    entry.generation.fetch_add(1, Ordering::AcqRel);
                }
            }
            None => {
                for entry in shared.values() {
                    entry.generation.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
    }

    /// [`check_token`], reused for `max_age` across the streams of `bot_id`. A zero `max_age`
    /// always reads the database.
    async fn shared_token(
        &self,
        db: &DatabaseConnection,
        token_hash: &str,
        bot_id: Uuid,
        max_age: Duration,
    ) -> Result<TokenState, DbErr> {
        let shared = self.shared_for(bot_id);
        let mut slot = shared.token.lock().await;
        let generation = shared.generation.load(Ordering::Acquire);
        if let Some(cached) = slot.as_ref()
            && cached.generation == generation
            && cached.hash == token_hash
            && cached.fetched.elapsed() < max_age
            && let Some(state) = cached.aged()
        {
            return Ok(state);
        }
        let fetched = Instant::now();
        let state = check_token(db, token_hash, bot_id).await?;
        *slot = Some(CachedToken {
            hash: token_hash.to_string(),
            state,
            fetched,
            generation,
        });
        drop(slot);
        Ok(state)
    }

    /// [`fetch_readable`], reused for `max_age` across the streams of `bot_id`.
    async fn shared_readable(
        &self,
        db: &DatabaseConnection,
        bot_id: Uuid,
        max_age: Duration,
    ) -> Result<Readable, ApiError> {
        let shared = self.shared_for(bot_id);
        let mut slot = shared.readable.lock().await;
        let generation = shared.generation.load(Ordering::Acquire);
        if let Some(cached) = slot.as_ref()
            && cached.generation == generation
            && cached.readable.fetched.elapsed() < max_age
        {
            return Ok(cached.readable.clone());
        }
        let readable = fetch_readable(db, bot_id).await?;
        *slot = Some(CachedReadable {
            readable: readable.clone(),
            generation,
        });
        drop(slot);
        Ok(readable)
    }

    fn signal(&self, signal: Signal) {
        // A change of access or token makes what the streams of that bot looked up unusable
        // before any of them is woken, so none of them reads a lookup older than the change.
        match signal {
            Signal::Access(bot) | Signal::Token(bot) => self.invalidate(Some(bot)),
            Signal::Reset => self.invalidate(None),
            Signal::Hint(_) | Signal::Overflow(_) => {}
        }
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
                    // `try_recv`, not `recv`: `recv` reconnects silently after a lost connection,
                    // and what was notified in the gap is gone. `Ok(None)` reports exactly that.
                    received = listener.try_recv() => match received {
                        Ok(Some(notification)) => self.dispatch(notification.channel(), notification.payload()),
                        Ok(None) => {
                            tracing::warn!("change hint LISTEN connection lost; streams are told to re-read");
                            self.signal(Signal::Reset);
                        }
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

    /// Forwards a row hint, or holds it back when its scope is over its burst for this window.
    ///
    /// Issue, draft and task hints are counted: they are the ones a bulk write fans out per row.
    /// In a window of one [`HintTuning::tick`], the first [`WORKSPACE_BURST`] hints of a workspace
    /// are counted as passing, and of those the first [`BURST`] of each [`Scope`] are forwarded
    /// at once; later ones are dropped and replaced by one [`Signal::Overflow`] per limit, sent
    /// when the window ends. So one workspace puts at most `WORKSPACE_BURST` hints, one workspace
    /// overflow and `WORKSPACE_BURST / BURST` scope overflows per window into the shared
    /// broadcast, however its writes are spread over projects and bots. Tool hints are not
    /// counted: they are not written per row.
    fn dispatch_hint(self: &Arc<Self>, hint: Hint) {
        let (workspace, scope) = match hint {
            Hint::Issue { workspace, project, .. } | Hint::Draft { workspace, project, .. } => {
                (workspace, Scope::Project { workspace, project })
            }
            Hint::Task { workspace, bot, .. } | Hint::UploadDraft { workspace, bot, .. } => {
                (workspace, Scope::Bot { workspace, bot })
            }
            Hint::Tools { .. } => return self.signal(Signal::Hint(hint)),
        };
        let window = self.tuning.tick;
        let now = Instant::now();
        let held = self.windows.lock().admit(workspace, scope, now, window);
        match held {
            None => self.signal(Signal::Hint(hint)),
            // Later hints of an overflowing window are covered by the overflow signal.
            Some(Held { first: false, .. }) => {}
            Some(Held {
                scope,
                first: true,
                started,
            }) => {
                let hub = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep_until(started + window).await;
                    hub.windows.lock().0.remove(&scope);
                    hub.signal(Signal::Overflow(scope));
                });
            }
        }
    }

    fn dispatch(self: &Arc<Self>, channel: &str, payload: &str) {
        let signal = match channel {
            HINT_CHANNEL => match parse_hint(payload) {
                Some(hint) => return self.dispatch_hint(hint),
                None => None,
            },
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

/// What the streams of one bot share: the last token lookup and the last readable set, each
/// valid only for the generation it was read in.
#[derive(Default)]
struct BotShared {
    generation: AtomicU64,
    token: tokio::sync::Mutex<Option<CachedToken>>,
    readable: tokio::sync::Mutex<Option<CachedReadable>>,
}

struct CachedToken {
    hash: String,
    state: TokenState,
    fetched: Instant,
    generation: u64,
}

impl CachedToken {
    /// The cached state with the time it has been cached taken off the time left; `None` when the
    /// token would have expired meanwhile, so that the expiry is read from the database.
    fn aged(&self) -> Option<TokenState> {
        match self.state {
            TokenState::Valid(Some(left)) => left
                .checked_sub(self.fetched.elapsed())
                .filter(|left| !left.is_zero())
                .map(|left| TokenState::Valid(Some(left))),
            other => Some(other),
        }
    }
}

struct CachedReadable {
    readable: Readable,
    generation: u64,
}

// -----------------------------------------------------------------------------------------------
// Token validity
// -----------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    permissions: serde_json::Value,
    remaining_ms: Option<i64>,
}

fn can_read(permissions: &serde_json::Value) -> bool {
    let names: Vec<String> = permissions
        .as_array()
        .map(|list| list.iter().filter_map(|name| name.as_str().map(String::from)).collect())
        .unwrap_or_default();
    bot_permissions_allow(&names, BotPermission::Read)
}

/// The bot-token lookup of `bot_or_user_auth_middleware`, by `token_hash`: the same predicate
/// (known hash, token and user active, not expired), plus the time left on the database clock.
async fn check_token(db: &DatabaseConnection, token_hash: &str, bot_id: Uuid) -> Result<TokenState, DbErr> {
    let row = TokenRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT b.id, b.is_active, COALESCE(u.is_active, true) AS user_active, b.permissions, \
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
        // The stream is a read of the bot's data: a token that lost `read` (and `write`, `admin`)
        // is treated as revoked, as the middleware would refuse its next GET.
        Some(row) if !can_read(&row.permissions) => TokenState::Revoked,
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
#[derive(Clone)]
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
    hub: Arc<HintHub>,
    db: DatabaseConnection,
    bot_id: Uuid,
    workspace_id: Uuid,
    token_hash: String,
    tuning: HintTuning,
    signals: broadcast::Receiver<Signal>,
    shutdown: watch::Receiver<bool>,
    tx: mpsc::Sender<Bytes>,
    readable: Readable,
    /// The project filter as of the last time the client was told about its access (the hello, or an
    /// `access_changed`). `deliver` and `on_overflow` replace `readable` without announcing, so the
    /// tick compares against this, not against `readable`.
    announced: ProjectFilter,
    expiry: Option<Instant>,
    db_failing_since: Option<Instant>,
    /// Since when the client's queue has been full, if it is.
    full_since: Option<Instant>,
    /// A frame was dropped because the queue was full: the client missed something and has to
    /// re-read, whether or not it drains afterwards.
    lost: bool,
    sent: u64,
}

async fn wait_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

impl HintStream {
    /// Queues a frame without waiting. A full queue drops the frame and starts the lag clock; the
    /// client is ended with `lagged` by [`Self::on_lag_deadline`], never by blocking here.
    fn send(&mut self, frame: Bytes) -> Result<(), End> {
        match self.tx.try_send(frame) {
            Ok(()) => {
                self.sent += 1;
                if !self.lost {
                    self.full_since = None;
                }
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.full_since.get_or_insert_with(Instant::now);
                self.lost = true;
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(End::ClientGone),
        }
    }

    /// A keep-alive comment carries nothing, so a full queue only starts the lag clock.
    fn send_keepalive(&mut self) -> Result<(), End> {
        match self.tx.try_send(Bytes::from_static(KEEPALIVE_FRAME.as_bytes())) {
            Ok(()) => {
                if !self.lost {
                    self.full_since = None;
                }
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.full_since.get_or_insert_with(Instant::now);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(End::ClientGone),
        }
    }

    /// The queue has been full for the lag timeout: a client that lost a frame or still does not
    /// drain is ended; one that drained without losing anything keeps its stream.
    fn on_lag_deadline(&mut self) -> Result<(), End> {
        if self.lost || self.tx.capacity() == 0 {
            Err(End::Lagged)
        } else {
            self.full_since = None;
            Ok(())
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
            let lag_deadline = self.full_since.map(|since| since + self.tuning.lag_timeout);
            // Biased: what ends a stream is polled before what feeds it, and nothing here awaits
            // the client, so one slow consumer cannot hold any arm back.
            let step = tokio::select! {
                biased;
                _ = self.shutdown.changed() => Err(End::Shutdown),
                () = wait_deadline(expiry) => Err(End::Revoked(Some("expired"))),
                () = self.tx.closed() => Err(End::ClientGone),
                () = wait_deadline(lag_deadline) => self.on_lag_deadline(),
                _ = tick.tick() => self.on_tick().await,
                _ = keepalive.tick() => self.send_keepalive(),
                signal = self.signals.recv() => self.on_signal(signal).await,
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
            Ok(Signal::Overflow(scope)) => self.on_overflow(scope).await,
            Ok(Signal::Access(bot)) if bot == self.bot_id => {
                // A permission change is an access change too: check the token (read) first.
                self.verify_token().await?;
                self.refresh_readable().await?;
                self.announce_access()
            }
            Ok(Signal::Token(bot)) if bot == self.bot_id => self.verify_token().await,
            Ok(Signal::Access(_) | Signal::Token(_)) => Ok(()),
        }
    }

    /// Ends the stream with `lagged` when it could have received the hints the overflow held
    /// back. Readability is judged like a hint's: on a set no older than the cache bound.
    async fn on_overflow(&mut self, scope: Scope) -> Result<(), End> {
        match scope {
            Scope::Workspace(workspace) if workspace == self.workspace_id => Err(End::Lagged),
            Scope::Bot { workspace, bot } if workspace == self.workspace_id && bot == self.bot_id => Err(End::Lagged),
            Scope::Project { workspace, project } if workspace == self.workspace_id => {
                if self.readable.fetched.elapsed() > self.tuning.readable_ttl {
                    self.refresh_readable_within(self.tuning.readable_ttl).await?;
                }
                if self.readable.filter.allows(project) {
                    Err(End::Lagged)
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    async fn on_tick(&mut self) -> Result<(), End> {
        let outcome = async {
            self.verify_token().await?;
            self.refresh_readable().await?;
            if self.readable.filter != self.announced {
                self.announce_access()?;
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

    /// Tells the client its access changed and records the filter it was told about.
    fn announce_access(&mut self) -> Result<(), End> {
        self.send(frame("access_changed", &serde_json::json!({})))?;
        self.announced = self.readable.filter.clone();
        Ok(())
    }

    /// Looks the token up again. `Err(End::Db)` is a database failure, not a verdict.
    async fn verify_token(&mut self) -> Result<(), End> {
        match self
            .hub
            .shared_token(&self.db, &self.token_hash, self.bot_id, self.tuning.shared_ttl)
            .await
        {
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
        self.refresh_readable_within(self.tuning.shared_ttl).await
    }

    async fn refresh_readable_within(&mut self, max_age: Duration) -> Result<(), End> {
        match self.hub.shared_readable(&self.db, self.bot_id, max_age).await {
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
        // An upload draft goes to its submitting bot while that bot still reads the whole workspace.
        let upload_workspace_wide =
            matches!(hint, Hint::UploadDraft { workspace, .. } if workspace == self.workspace_id);
        if (project_to_check(hint, self.workspace_id).is_some() || upload_workspace_wide)
            && self.readable.fetched.elapsed() > self.tuning.readable_ttl
        {
            // Never deliver on a set older than the cache bound. A failed lookup ends the stream
            // rather than delivering on stale access.
            self.refresh_readable_within(self.tuning.readable_ttl).await?;
        }
        if upload_workspace_wide && !matches!(self.readable.filter, ProjectFilter::All) {
            return Ok(());
        }
        let frames = frames_for(hint, self.bot_id, self.workspace_id, |project| {
            self.readable.filter.allows(project)
        });
        for frame in frames {
            self.send(frame)?;
        }
        Ok(())
    }
}

/// The project whose readability decides whether `hint` is delivered to a bot of `workspace_id`.
fn project_to_check(hint: Hint, workspace_id: Uuid) -> Option<Uuid> {
    match hint {
        Hint::Issue { project, workspace, .. } | Hint::Draft { project, workspace, .. }
            if workspace == workspace_id =>
        {
            Some(project)
        }
        Hint::Tools {
            project: Some(project),
            workspace,
        } if workspace.is_none_or(|w| w == workspace_id) => Some(project),
        _ => None,
    }
}

/// The response body: queued frames in order, then the closing frame. It owns a [`BodyPermit`]
/// (not a place in the stream limits, which the stream's task holds), so the body counts against
/// the body caps until it is dropped.
fn body(rx: mpsc::Receiver<Bytes>, close: watch::Receiver<Option<Bytes>>, permit: BodyPermit) -> Body {
    struct State {
        rx: mpsc::Receiver<Bytes>,
        close: watch::Receiver<Option<Bytes>>,
        done: bool,
        _permit: BodyPermit,
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
    // The body cap is checked first: it is the larger of the two, and a refusal here costs nothing.
    let body_permit = match hub.try_acquire_body(bot.bot_id, bot.workspace_id) {
        Ok(permit) => permit,
        Err(limit) => {
            tracing::info!(bot_id = %bot.bot_id, limit = ?limit, "change hint stream refused: open bodies limit reached");
            return Ok(limit_response(
                StatusCode::TOO_MANY_REQUESTS,
                "too many open change-hint streams",
            ));
        }
    };
    let permit = match hub.try_acquire(bot.bot_id, bot.workspace_id) {
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
    let expiry = match hub
        .shared_token(&state.db, &token_hash, bot.bot_id, Duration::ZERO)
        .await
        .map_err(ApiError::internal_database_failure)?
    {
        TokenState::Valid(remaining) => remaining.map(|left| Instant::now() + left),
        TokenState::Revoked | TokenState::Expired => {
            return Err(ApiError::Unauthorized("bot token is no longer valid".to_string()));
        }
    };
    let readable = hub.shared_readable(&state.db, bot.bot_id, Duration::ZERO).await?;

    let (tx, rx) = mpsc::channel(hub.tuning.channel_capacity.max(2));
    let (close_tx, close_rx) = watch::channel(None);
    // Cannot fail: the queue is empty and its receiver is alive.
    let _ = tx.try_send(hello_frame(bot.bot_id, bot.workspace_id));

    let stream = HintStream {
        hub: Arc::clone(&hub),
        db: state.db.clone(),
        bot_id: bot.bot_id,
        workspace_id: bot.workspace_id,
        token_hash,
        tuning: hub.tuning,
        signals,
        shutdown: hub.shutdown.subscribe(),
        tx,
        announced: readable.filter.clone(),
        readable,
        expiry,
        db_failing_since: None,
        full_since: None,
        lost: false,
        sent: 0,
    };
    let bot_id = bot.bot_id;
    tracing::debug!(bot_id = %bot_id, "change hint stream opened");
    // The task owns the place in the limits: it is released when the stream ends, whether or not
    // the client ever reads the end.
    tokio::spawn(async move {
        let (end, sent) = stream.run().await;
        drop(permit);
        tracing::info!(bot_id = %bot_id, reason = end.code(), events_sent = sent, "change hint stream closed");
        // The receiver is gone when the client left; nothing is left to tell it then.
        let _ = close_tx.send(end.final_frame());
        // Keep the sender alive until the body has read the closing frame, but not for a client
        // that never reads it.
        let _ = tokio::time::timeout(CLOSE_GRACE, close_tx.closed()).await;
    });

    let mut response = Response::new(body(rx, close_rx, body_permit));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}
