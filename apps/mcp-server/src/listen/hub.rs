//! The listen hubs: one per caller credential, fed by the API's change-hint stream.
//!
//! A hub exists only while at least one listen stream of its credential is open, and only after
//! the API accepted the credential by opening its hint stream. It owns the credential (inside the
//! client), the upstream stream and the set of downstream streams. Two credentials never share a
//! hub: the map is keyed by the SHA-256 of the bearer token and every hub holds exactly one client.
//!
//! Nothing here ever waits for a downstream consumer. A hint sets a flag on each stream and wakes
//! it; a stream that does not read costs a few flags and one entry per URI it subscribed to (at
//! most one pending notification per URI), and the upstream read goes on.
//!
//! A stream may also hold resource subscriptions. Each acknowledged URI has a key (`SubKey`: a
//! task, a work item or the bot's inbox) and a hint sets the pending flag of the URIs with that key.
//! A hint that carries no valid id marks every URI of its kind (anything may have changed). When the
//! API reports a change of project access or of the tool surface, every acknowledged URI of the hub is
//! decided again as the caller, and a stream that holds one the caller may no longer read is ended
//! with its final result; the client listens again and gets the smaller acknowledgment.

use super::CallerKey;
use super::decide::{RECHECK_BUDGET, RECHECK_PERMITS, decide};
use super::limiter::{Client, RefusalLimiter};
use super::uris::{KeyKind, SubKey, Subscribable, Verdict, parse_subscribable};
use crate::client::{FIRST_OPEN_TIMEOUT, HintEvent, HintOpenError, HintStream, OpenPrClient, RECONNECT_OPEN_TIMEOUT};
use crate::server::McpServer;
use parking_lot::Mutex;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// How long a refused credential keeps being refused without asking the API again.
pub const REFUSAL_CACHE: Duration = Duration::from_secs(10);
/// Most credentials the refusal cache remembers at once. A full cache stops remembering; it never
/// grows, so made-up tokens cannot make it a memory leak.
const REFUSAL_CACHE_CAPACITY: usize = 10_000;
/// No byte from the upstream for this long (the API sends a keep-alive every 15 s) is a drop.
const UPSTREAM_IDLE: Duration = Duration::from_secs(45);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Why the API refused a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The credential is not valid (any more): revoked, expired, deactivated, unknown.
    Unauthorized,
    /// The credential is valid but has no push: not a bot token, or a bot that may not read.
    NotABot,
}

/// What opening a stream on a hub came to.
pub(super) enum Access {
    /// The stream is registered on a hub and receives pushes.
    Push(Registration),
    /// The credential gets no push (D5).
    NoPush,
}

/// Why no hub could be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AcquireError {
    Unauthorized,
    Unavailable,
    /// The API holds too many streams for this credential or process (`429`); retry later.
    Busy,
    /// The client address has had too many credentials refused lately.
    Throttled,
}

/// What a stream finds when it looks at its shared state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Take {
    /// The hub ended the stream (revoked); the final result is next.
    Ended,
    /// A tool list change is pending; one notification is next.
    Tools,
    /// A subscribed resource changed; one notification with this URI is next.
    Uri(Arc<str>),
    Nothing,
}

/// The state one downstream stream shares with its hub: one flag per kind of notification and the
/// end marker. Setting a flag never blocks and its size does not depend on the number of hints.
#[derive(Default)]
pub(super) struct StreamShared {
    state: Mutex<SharedState>,
    wake: Notify,
}

#[derive(Default)]
struct SharedState {
    /// Whether the stream asked for, and was granted, tool list changes.
    wants_tools: bool,
    tools_pending: bool,
    ended: bool,
    /// The acknowledged URIs, as the client wrote them.
    uris: Vec<Arc<str>>,
    /// Indexes into `uris` by the key a hint is matched on.
    by_key: HashMap<SubKey, Vec<usize>>,
    /// Indexes of the URIs with a change to report, so a URI is announced once however many hints
    /// came.
    pending: BTreeSet<usize>,
}

impl StreamShared {
    /// Declares what the stream receives: tool list changes, and the acknowledged URIs with their
    /// keys (in acknowledgment order).
    pub(super) fn configure(&self, wants_tools: bool, subscriptions: &[(Arc<str>, SubKey)]) {
        let mut state = self.state.lock();
        state.wants_tools = wants_tools;
        state.uris = subscriptions.iter().map(|(uri, _)| Arc::clone(uri)).collect();
        state.by_key.clear();
        for (index, (_, key)) in subscriptions.iter().enumerate() {
            state.by_key.entry(*key).or_default().push(index);
        }
    }

    fn mark_tools(&self) {
        let mut state = self.state.lock();
        if state.wants_tools {
            state.tools_pending = true;
            drop(state);
            self.wake.notify_one();
        }
    }

    /// Marks the URIs with `key`.
    fn mark_key(&self, key: &SubKey) {
        let mut state = self.state.lock();
        let marked = state.by_key.get(key).cloned().unwrap_or_default();
        if marked.is_empty() {
            return;
        }
        state.pending.extend(marked);
        drop(state);
        self.wake.notify_one();
    }

    /// Marks every URI whose key is of `kind`.
    fn mark_kind(&self, kind: KeyKind) {
        let mut state = self.state.lock();
        let marked: Vec<usize> = state
            .by_key
            .iter()
            .filter(|(key, _)| key.kind() == Some(kind))
            .flat_map(|(_, indexes)| indexes.iter().copied())
            .collect();
        if marked.is_empty() {
            return;
        }
        state.pending.extend(marked);
        drop(state);
        self.wake.notify_one();
    }

    /// "Maybe changed": every URI, and the tool list when the stream wants it.
    fn mark_everything(&self) {
        let mut state = self.state.lock();
        let all = 0..state.uris.len();
        state.pending.extend(all);
        state.tools_pending = state.wants_tools;
        drop(state);
        self.wake.notify_one();
    }

    fn uris(&self) -> Vec<Arc<str>> {
        self.state.lock().uris.clone()
    }

    fn end(&self) {
        self.state.lock().ended = true;
        self.wake.notify_one();
    }

    /// The next thing the stream must do; clears the flag it reports.
    pub(super) fn take(&self) -> Take {
        let mut state = self.state.lock();
        if state.ended {
            Take::Ended
        } else if std::mem::take(&mut state.tools_pending) {
            Take::Tools
        } else if let Some(index) = state.pending.pop_first() {
            state
                .uris
                .get(index)
                .map_or(Take::Nothing, |uri| Take::Uri(Arc::clone(uri)))
        } else {
            Take::Nothing
        }
    }

    pub(super) async fn changed(&self) {
        self.wake.notified().await;
    }
}

struct HubInner {
    streams: HashMap<u64, Arc<StreamShared>>,
    next_id: u64,
    ended: bool,
}

/// Whether a re-check of the subscriptions is running, and whether another was asked for meanwhile.
#[derive(Default)]
struct Recheck {
    running: bool,
    again: bool,
}

/// The streams of one credential and the upstream that feeds them.
pub(super) struct Hub {
    key: CallerKey,
    inner: Mutex<HubInner>,
    /// Stops the upstream task: the last stream left, or the hub ended.
    stop: CancellationToken,
    /// Decides subscriptions as the hub's credential.
    authority: Arc<McpServer>,
    recheck: Mutex<Recheck>,
}

impl Hub {
    fn new(key: CallerKey, authority: Arc<McpServer>) -> Arc<Self> {
        Arc::new(Self {
            key,
            authority,
            recheck: Mutex::new(Recheck::default()),
            inner: Mutex::new(HubInner {
                streams: HashMap::new(),
                next_id: 0,
                ended: false,
            }),
            stop: CancellationToken::new(),
        })
    }

    /// Adds a stream; `None` when the hub has ended in the meantime.
    fn register(self: &Arc<Self>, hubs: &Arc<Hubs>) -> Option<Registration> {
        let shared = Arc::new(StreamShared::default());
        let id = {
            let mut inner = self.inner.lock();
            if inner.ended {
                return None;
            }
            let id = inner.next_id;
            inner.next_id += 1;
            inner.streams.insert(id, Arc::clone(&shared));
            id
        };
        Some(Registration {
            hub: Arc::clone(self),
            hubs: Arc::clone(hubs),
            id,
            shared,
        })
    }

    /// Asks every stream that wants it to send one `tools/list_changed` (coalesced per stream).
    fn broadcast_tools(&self) {
        for shared in self.inner.lock().streams.values() {
            shared.mark_tools();
        }
    }

    /// A task or work item changed (`id`), or an unknown one of that kind did (`None`).
    fn broadcast_object(&self, kind: KeyKind, id: Option<uuid::Uuid>) {
        let key = id.map(|id| match kind {
            KeyKind::Task => SubKey::Task(id),
            KeyKind::Issue => SubKey::Issue(id),
        });
        for shared in self.inner.lock().streams.values() {
            match &key {
                Some(key) => shared.mark_key(key),
                None => shared.mark_kind(kind),
            }
        }
    }

    fn broadcast_inbox(&self) {
        for shared in self.inner.lock().streams.values() {
            shared.mark_key(&SubKey::Inbox);
        }
    }

    /// Hints may have been lost: every stream re-reads everything it follows.
    fn broadcast_everything(&self) {
        for shared in self.inner.lock().streams.values() {
            shared.mark_everything();
        }
    }

    /// Decides every acknowledged URI of the hub again; see [`recheck_once`]. One re-check runs at a
    /// time; a request that comes while one runs makes it run once more.
    fn request_recheck(self: &Arc<Self>) {
        {
            let mut state = self.recheck.lock();
            if state.running {
                state.again = true;
                return;
            }
            state.running = true;
        }
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = hub.stop.cancelled() => {}
                    () = recheck_once(&hub) => {}
                }
                let again = {
                    let mut state = hub.recheck.lock();
                    let again = state.again && !hub.stop.is_cancelled();
                    state.again = false;
                    state.running = again;
                    again
                };
                if !again {
                    return;
                }
            }
        });
    }

    /// Ends every stream with its final result and refuses further registrations.
    fn end_all(&self) {
        let streams = {
            let mut inner = self.inner.lock();
            inner.ended = true;
            std::mem::take(&mut inner.streams)
        };
        for shared in streams.values() {
            shared.end();
        }
        self.stop.cancel();
    }
}

/// A stream's place on its hub. Dropping it removes the stream, and the last one out stops the
/// upstream and removes the hub: no hub without a stream.
pub(super) struct Registration {
    hub: Arc<Hub>,
    hubs: Arc<Hubs>,
    id: u64,
    pub(super) shared: Arc<StreamShared>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let last = {
            let mut inner = self.hub.inner.lock();
            inner.streams.remove(&self.id);
            let last = inner.streams.is_empty() && !inner.ended;
            if last {
                inner.ended = true;
            }
            last
        };
        if last {
            self.hub.stop.cancel();
            self.hubs.remove(&self.hub);
        }
    }
}

#[derive(Default)]
struct HubsState {
    hubs: HashMap<CallerKey, Arc<Hub>>,
    /// One gate per credential whose hub is being opened, so concurrent first listens share one
    /// upstream request.
    opening: HashMap<CallerKey, Arc<tokio::sync::Mutex<()>>>,
    refused: HashMap<CallerKey, (Instant, Refusal)>,
}

/// All hubs of a process and the refusal cache.
#[derive(Default)]
pub(super) struct Hubs {
    state: Mutex<HubsState>,
    /// Counts the credentials the API refused, per client address.
    limiter: RefusalLimiter,
}

impl Hubs {
    /// Hubs alive right now.
    pub(super) fn hub_count(&self) -> usize {
        self.state.lock().hubs.len()
    }

    fn remove(&self, hub: &Arc<Hub>) {
        let mut state = self.state.lock();
        if state.hubs.get(&hub.key).is_some_and(|known| Arc::ptr_eq(known, hub)) {
            state.hubs.remove(&hub.key);
        }
    }

    fn refuse(&self, key: CallerKey, refusal: Refusal) {
        let now = Instant::now();
        let mut state = self.state.lock();
        if state.refused.len() >= REFUSAL_CACHE_CAPACITY && !state.refused.contains_key(&key) {
            state.refused.retain(|_, (until, _)| *until > now);
            if state.refused.len() >= REFUSAL_CACHE_CAPACITY {
                return;
            }
        }
        state.refused.insert(key, (now + REFUSAL_CACHE, refusal));
    }

    /// A revoked credential: every stream of its hub ends at once and the key is refused for
    /// [`REFUSAL_CACHE`].
    fn revoke(&self, hub: &Arc<Hub>, refusal: Refusal) {
        hub.end_all();
        self.remove(hub);
        self.refuse(hub.key, refusal);
    }

    /// What is already known about `key`: its refusal, or a registration on its live hub.
    // The refusal check, the hub lookup, the registration and the removal of an ended hub must stay
    // under one guard; dropping it earlier would split the check-then-mutate.
    #[allow(clippy::significant_drop_tightening)]
    fn known(self: &Arc<Self>, key: &CallerKey) -> Result<Option<Access>, AcquireError> {
        let mut state = self.state.lock();
        if let Some((until, refusal)) = state.refused.get(key).copied() {
            if until > Instant::now() {
                return match refusal {
                    Refusal::Unauthorized => Err(AcquireError::Unauthorized),
                    Refusal::NotABot => Ok(Some(Access::NoPush)),
                };
            }
            state.refused.remove(key);
        }
        let Some(hub) = state.hubs.get(key).cloned() else {
            return Ok(None);
        };
        if let Some(registration) = hub.register(self) {
            return Ok(Some(Access::Push(registration)));
        }
        // The hub ended between its last stream leaving and its removal.
        state.hubs.remove(key);
        Ok(None)
    }

    fn gate(&self, key: CallerKey) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.state.lock().opening.entry(key).or_default())
    }

    /// The registration of a new stream of the credential behind `client`: on the live hub of
    /// its key, else on a new one once the API has accepted the credential.
    pub(super) async fn acquire(
        self: &Arc<Self>,
        key: CallerKey,
        client: &OpenPrClient,
        server: &Arc<McpServer>,
        peer: Option<Client>,
        shutdown: &CancellationToken,
    ) -> Result<Access, AcquireError> {
        if let Some(access) = self.known(&key)? {
            return Ok(access);
        }
        // Past this point the credential costs the API a request.
        if peer.is_some_and(|client| self.limiter.throttled(client)) {
            return Err(AcquireError::Throttled);
        }
        let gate = GateRelease {
            hubs: self,
            key,
            gate: self.gate(key),
        };
        let _open = gate.gate.lock().await;
        self.open_locked(key, client, server, peer, shutdown).await
    }

    async fn open_locked(
        self: &Arc<Self>,
        key: CallerKey,
        client: &OpenPrClient,
        server: &Arc<McpServer>,
        peer: Option<Client>,
        shutdown: &CancellationToken,
    ) -> Result<Access, AcquireError> {
        // Another first listen may have finished while this one waited for the gate.
        if let Some(access) = self.known(&key)? {
            return Ok(access);
        }
        // The attempt is counted before the API is asked, so the limit holds under concurrency; it
        // is given back at the end of this function unless the API refuses the credential.
        let attempt = match peer {
            Some(client) => Some(self.limiter.admit(client).ok_or(AcquireError::Throttled)?),
            None => None,
        };
        match client.open_change_hints(FIRST_OPEN_TIMEOUT).await {
            Ok(stream) => {
                let hub = Hub::new(key, Arc::clone(server));
                let Some(registration) = hub.register(self) else {
                    return Err(AcquireError::Unavailable);
                };
                self.state.lock().hubs.insert(key, Arc::clone(&hub));
                tokio::spawn(run_hub(Arc::clone(self), hub, client.clone(), stream, shutdown.clone()));
                Ok(Access::Push(registration))
            }
            Err(HintOpenError::Unauthorized) => {
                self.refuse(key, Refusal::Unauthorized);
                if let Some(attempt) = attempt {
                    attempt.refused();
                }
                Err(AcquireError::Unauthorized)
            }
            Err(HintOpenError::NotABot) => {
                self.refuse(key, Refusal::NotABot);
                Ok(Access::NoPush)
            }
            // A full API is not a verdict on the credential; the caller backs off and retries.
            Err(HintOpenError::TooMany) => Err(AcquireError::Busy),
            Err(HintOpenError::Unavailable) => Err(AcquireError::Unavailable),
        }
    }
}

/// Removes the opening gate of a key when its last user leaves, also when the opening request is
/// dropped half way (a client that hung up), so made-up tokens cannot leave gates behind.
struct GateRelease<'a> {
    hubs: &'a Hubs,
    key: CallerKey,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl Drop for GateRelease<'_> {
    fn drop(&mut self) {
        let mut state = self.hubs.state.lock();
        // The map's reference and this one: nobody else is waiting on the gate.
        if Arc::strong_count(&self.gate) <= 2 {
            state.opening.remove(&self.key);
        }
    }
}

/// Decides every acknowledged URI of the hub again, once per distinct URI, and ends each stream
/// that holds a URI the credential may not be shown to still read: refused, or not decided (the API
/// failed or the budget ran out).
async fn recheck_once(hub: &Arc<Hub>) {
    let streams: Vec<Arc<StreamShared>> = hub.inner.lock().streams.values().cloned().collect();
    let mut seen: HashSet<Arc<str>> = HashSet::new();
    let mut items: Vec<(Arc<str>, Subscribable)> = Vec::new();
    for shared in &streams {
        for uri in shared.uris() {
            if seen.insert(Arc::clone(&uri))
                && let Some(parsed) = parse_subscribable(&uri)
            {
                items.push((uri, parsed));
            }
        }
    }
    if items.is_empty() {
        return;
    }
    let verdicts = decide(&hub.authority, &items, RECHECK_BUDGET, Some(&RECHECK_PERMITS)).await;
    // Fail closed: only a URI the caller may read now keeps its stream. Push only speeds things up
    // (the client re-listens and reads), so ending a stream whose URI could not be decided costs a
    // re-listen, while keeping it would keep announcing changes to something maybe no longer readable.
    let denied: HashSet<Arc<str>> = items
        .iter()
        .zip(verdicts)
        .filter(|(_, verdict)| !matches!(verdict, Some(Verdict::Granted(_))))
        .map(|((uri, _), _)| Arc::clone(uri))
        .collect();
    if denied.is_empty() {
        return;
    }
    let mut ended = 0_usize;
    for shared in &streams {
        if shared.uris().iter().any(|uri| denied.contains(uri)) {
            shared.end();
            ended += 1;
        }
    }
    tracing::info!(
        ended,
        "Ended listen streams: a subscribed resource is no longer confirmed readable"
    );
}

/// How one pass over an upstream stream ended.
enum Pass {
    /// The hub or the process is stopping.
    Stop,
    /// The API says the credential is no longer good.
    Revoked,
    /// The stream broke, lagged or ended; reconnect.
    Dropped,
}

async fn read_upstream(hub: &Arc<Hub>, stream: &mut HintStream, shutdown: &CancellationToken) -> Pass {
    loop {
        let next = tokio::select! {
            biased;
            () = hub.stop.cancelled() => return Pass::Stop,
            () = shutdown.cancelled() => return Pass::Stop,
            next = tokio::time::timeout(UPSTREAM_IDLE, stream.next_event()) => next,
        };
        match next {
            Ok(Ok(Some(event))) => match event {
                // A change of project access can change what `tools/list` shows, so it is a
                // tool-list hint too. Pushing more than needed only costs a re-list.
                HintEvent::Tools | HintEvent::AccessChanged => {
                    hub.broadcast_tools();
                    hub.request_recheck();
                }
                HintEvent::Task(id) => hub.broadcast_object(KeyKind::Task, id),
                HintEvent::Issue(id) => hub.broadcast_object(KeyKind::Issue, id),
                HintEvent::Inbox => hub.broadcast_inbox(),
                HintEvent::Revoked => return Pass::Revoked,
                HintEvent::Lagged | HintEvent::Shutdown => return Pass::Dropped,
                HintEvent::Draft | HintEvent::Unknown => {}
            },
            Ok(Ok(None) | Err(_)) | Err(_) => return Pass::Dropped,
        }
    }
}

/// The upstream task of one hub: reads hints, fans them out, reconnects after a drop and ends the
/// hub when the API revokes the credential.
async fn run_hub(
    hubs: Arc<Hubs>,
    hub: Arc<Hub>,
    client: OpenPrClient,
    mut stream: HintStream,
    shutdown: CancellationToken,
) {
    let mut failures = 0_u32;
    let mut opened = Instant::now();
    loop {
        match read_upstream(&hub, &mut stream, &shutdown).await {
            Pass::Stop => return,
            Pass::Revoked => {
                tracing::info!("A listen hub ended: the credential is revoked or expired");
                hubs.revoke(&hub, Refusal::Unauthorized);
                return;
            }
            Pass::Dropped => {}
        }
        // A stream that lived a while earns a fresh start; one that drops at once backs off.
        failures = if opened.elapsed() >= BACKOFF_MAX {
            0
        } else {
            failures.saturating_add(1)
        };
        loop {
            tokio::select! {
                biased;
                () = hub.stop.cancelled() => return,
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(jittered(backoff(failures), rand::random::<f64>())) => {}
            }
            match client.open_change_hints(RECONNECT_OPEN_TIMEOUT).await {
                Ok(next) => {
                    stream = next;
                    opened = Instant::now();
                    // Hints committed while the upstream was away are lost: maybe changed.
                    hub.broadcast_everything();
                    break;
                }
                Err(HintOpenError::Unauthorized) => {
                    tracing::info!("A listen hub ended: the API no longer accepts the credential");
                    hubs.revoke(&hub, Refusal::Unauthorized);
                    return;
                }
                Err(HintOpenError::NotABot) => {
                    tracing::info!("A listen hub ended: the credential no longer has push");
                    hubs.revoke(&hub, Refusal::NotABot);
                    return;
                }
                Err(HintOpenError::TooMany | HintOpenError::Unavailable) => {
                    failures = failures.saturating_add(1);
                }
            }
        }
    }
}

/// The wait before reconnect attempt number `failures` (1 s, doubling, at most 30 s).
fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(5);
    (BACKOFF_START * 2_u32.pow(doublings)).min(BACKOFF_MAX)
}

/// Equal jitter: `base` scaled into `[base / 2, base]` by `fraction` in `[0, 1]`, so hubs dropped
/// together do not reconnect in lockstep.
fn jittered(base: Duration, fraction: f64) -> Duration {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        1.0
    };
    base.mul_f64(0.5 + fraction / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jitter_stays_between_half_and_all_of_the_backoff() {
        let base = Duration::from_secs(8);
        assert_eq!(jittered(base, 0.0), Duration::from_secs(4));
        assert_eq!(jittered(base, 0.5), Duration::from_secs(6));
        assert_eq!(jittered(base, 1.0), base);
        assert_eq!(jittered(base, 7.0), base);
        assert_eq!(jittered(base, -1.0), Duration::from_secs(4));
        assert_eq!(jittered(base, f64::NAN), base);
    }

    #[test]
    fn the_backoff_starts_at_one_second_doubles_and_stops_at_thirty() {
        let waits: Vec<u64> = [0, 1, 2, 3, 4, 5, 6, 7, 100]
            .iter()
            .map(|n| backoff(*n).as_secs())
            .collect();
        assert_eq!(waits, vec![1, 1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[tokio::test]
    async fn a_failed_or_abandoned_open_leaves_no_gate_behind() {
        use crate::client::{ClientConfig, TRANSPORT_LABEL_HTTP};
        let client = OpenPrClient::new(ClientConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            credential: Some(platform::config::Secret::new("opr_unit")),
            workspace_id: None,
            transport_label: TRANSPORT_LABEL_HTTP,
        })
        .expect("a client builds");
        let hubs = Arc::new(Hubs::default());
        let shutdown = CancellationToken::new();
        let key = [7_u8; 32];
        let server = Arc::new(McpServer::new(client.clone(), platform::config::McpToolMode::Full));
        let failed = hubs.acquire(key, &client, &server, None, &shutdown).await;
        assert!(matches!(failed, Err(AcquireError::Unavailable)));
        assert!(hubs.state.lock().opening.is_empty());
        // An open that is dropped while it waits (the client hung up) cleans up too.
        let pending = {
            let hubs = Arc::clone(&hubs);
            let client = client.clone();
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                hubs.acquire(key, &client, &server, None, &CancellationToken::new())
                    .await
                    .is_ok()
            })
        };
        pending.abort();
        let _ = pending.await;
        assert!(hubs.state.lock().opening.is_empty());
        assert_eq!(hubs.hub_count(), 0);
    }

    #[test]
    fn a_full_refusal_cache_stops_remembering_instead_of_growing() {
        let hubs = Hubs::default();
        for n in 0..REFUSAL_CACHE_CAPACITY + 50 {
            let mut key = [0_u8; 32];
            key[..8].copy_from_slice(&(n as u64).to_le_bytes());
            hubs.refuse(key, Refusal::Unauthorized);
        }
        assert_eq!(hubs.state.lock().refused.len(), REFUSAL_CACHE_CAPACITY);
    }
}
