//! `subscriptions/listen` of the modern era: the registry of open listen streams.
//!
//! This is the transport-neutral part. A stream belongs to the credential that opened it and is
//! counted against two caps: streams per caller key (a SHA-256 of the bearer token, never the
//! token) and streams per process.
//!
//! What a stream carries depends on who opened it. A bot token (`opr_`) that asks for
//! `toolsListChanged` gets a stream on the hub of its credential ([`hub`]): the API has accepted
//! the credential by opening its change-hint stream, every hint is fanned out to the streams of
//! that credential, and the API's revocation ends them. A stream with nothing to deliver ends at
//! once with its final result, so it holds no slot: that is every user token, every delegated
//! caller (which has no push, D5, and whose token the API has already vouched for) and every
//! request that asks for nothing the server honours. Nothing outlives the credential: a held
//! stream exists only on a hub, and a hub ends when the API revokes or expires the credential.

mod decide;
mod hub;
mod limiter;
mod uris;

use crate::client::OpenPrClient;
use crate::era::META_SUBSCRIPTION_ID;
use crate::protocol::{JsonRpcError, JsonRpcRequest};
use crate::server::McpServer;
use hub::{Access, AcquireError, Hubs, Registration, Take};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

pub use hub::REFUSAL_CACHE;
pub use uris::{MAX_URI_BYTES, MAX_URIS_PER_REQUEST, SubKey, Subscribable, Verdict};

/// Open listen streams one credential may hold.
pub const MAX_STREAMS_PER_KEY: usize = 1_000;
/// Acknowledged resource subscriptions, over all its streams, one credential may hold. A URI over
/// the cap is left out of the acknowledgment like any other URI that is not honoured.
pub const MAX_SUBSCRIPTIONS_PER_KEY: usize = 2_000;
/// Acknowledged resource subscriptions the process holds in total.
pub const MAX_SUBSCRIPTIONS: usize = 20_000;
/// Comment line interval on an idle listen response.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// `Retry-After` of a refused listen request, in seconds.
pub const RETRY_AFTER_SECS: u64 = 30;
/// How long a stream's consumer may leave the transport's queue full before the stream is ended.
pub const STALL_LIMIT: Duration = Duration::from_secs(30);

/// Who a stream belongs to: the SHA-256 of the bearer token.
pub type CallerKey = [u8; 32];

/// The key of a credential.
pub fn caller_key(token: &str) -> CallerKey {
    Sha256::digest(token.as_bytes()).into()
}

/// Limits and timing of a registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenConfig {
    /// `[mcp] listen`; `false` makes the method not exist.
    pub enabled: bool,
    /// Streams the process holds in total.
    pub max_streams: usize,
    /// Streams one credential holds.
    pub per_key_limit: usize,
    /// Resource subscriptions the process holds in total.
    pub max_subscriptions: usize,
    /// Resource subscriptions one credential holds.
    pub per_key_subscriptions: usize,
    pub keepalive: Duration,
}

impl ListenConfig {
    /// The production configuration for a total of `max_streams`.
    pub const fn new(enabled: bool, max_streams: usize) -> Self {
        Self {
            enabled,
            max_streams,
            per_key_limit: MAX_STREAMS_PER_KEY,
            max_subscriptions: MAX_SUBSCRIPTIONS,
            per_key_subscriptions: MAX_SUBSCRIPTIONS_PER_KEY,
            keepalive: KEEPALIVE_INTERVAL,
        }
    }
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self::new(true, 10_000)
    }
}

/// The envelope checks a listen request needs before a stream is opened: `jsonrpc` is `"2.0"` and
/// the id is a string or a number.
pub fn validate_envelope(req: &JsonRpcRequest) -> Result<(), JsonRpcError> {
    if req.jsonrpc != "2.0" {
        return Err(JsonRpcError::invalid_request("Invalid jsonrpc version"));
    }
    match req.id {
        Some(Value::String(_) | Value::Number(_)) => Ok(()),
        _ => Err(JsonRpcError::invalid_request("id must be a string or a number")),
    }
}

/// Who opens a stream, as far as the registry needs to know.
pub enum ListenAuth {
    /// A bot or user token. `client` acts as that token; a hub opened for it keeps the client.
    /// `server` is built on the same client and decides the subscriptions the token asks for.
    Token {
        key: CallerKey,
        client: OpenPrClient,
        server: Arc<McpServer>,
    },
    /// A delegated caller: counted under its key, never given a hub.
    Delegated { key: CallerKey },
}

impl ListenAuth {
    const fn key(&self) -> CallerKey {
        match self {
            Self::Token { key, .. } | Self::Delegated { key } => *key,
        }
    }
}

/// Why a stream was not opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// A cap is reached; the number is the cap that was hit.
    TooMany(usize),
    /// The request parameters are malformed.
    InvalidParams(String),
    /// The API refuses the credential (now, or in the last [`REFUSAL_CACHE`]).
    Unauthorized,
    /// The API could not be asked; the caller retries later.
    Unavailable,
    /// The API holds too many streams for the credential or the process; the caller retries later.
    Busy,
    /// The client address has had too many credentials refused lately.
    Throttled,
}

impl OpenError {
    pub fn into_rpc_error(self) -> JsonRpcError {
        match self {
            Self::TooMany(limit) => JsonRpcError::too_many_streams(u32::try_from(limit).unwrap_or(u32::MAX)),
            Self::InvalidParams(message) => JsonRpcError::invalid_params(message),
            Self::Unauthorized => JsonRpcError {
                code: -32603,
                message: "Unauthorized".to_string(),
                data: Some(json!({ "reason": "unauthorized" })),
            },
            Self::Unavailable => JsonRpcError::internal_error("push unavailable, retry"),
            Self::Busy => JsonRpcError::too_many_requests("busy"),
            Self::Throttled => JsonRpcError::too_many_requests("throttled"),
        }
    }
}

#[derive(Default)]
struct Counts {
    total: usize,
    per_key: HashMap<CallerKey, usize>,
    subscriptions: usize,
    subscriptions_per_key: HashMap<CallerKey, usize>,
}

struct Inner {
    config: ListenConfig,
    counts: Mutex<Counts>,
    hubs: Arc<Hubs>,
    /// Fired by [`ListenRegistry::shutdown`]: every stream ends with its final result.
    shutdown: CancellationToken,
    /// Notified whenever a stream ends.
    ended: Notify,
}

/// The listen streams of one process.
#[derive(Clone)]
pub struct ListenRegistry {
    inner: Arc<Inner>,
}

impl ListenRegistry {
    pub fn new(config: ListenConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                counts: Mutex::new(Counts::default()),
                hubs: Arc::new(Hubs::default()),
                shutdown: CancellationToken::new(),
                ended: Notify::new(),
            }),
        }
    }

    /// Whether `subscriptions/listen` exists (`[mcp] listen`).
    pub fn enabled(&self) -> bool {
        self.inner.config.enabled
    }

    pub fn keepalive(&self) -> Duration {
        self.inner.config.keepalive
    }

    /// Streams open right now.
    pub fn open_streams(&self) -> usize {
        self.inner.counts.lock().total
    }

    /// Hubs alive right now: credentials with at least one held stream.
    pub fn hub_count(&self) -> usize {
        self.inner.hubs.hub_count()
    }

    /// Takes a place under both stream caps.
    fn reserve(&self, key: CallerKey) -> Result<StreamGuard, OpenError> {
        {
            let mut counts = self.inner.counts.lock();
            if counts.total >= self.inner.config.max_streams {
                return Err(OpenError::TooMany(self.inner.config.max_streams));
            }
            let held = counts.per_key.entry(key).or_insert(0);
            if *held >= self.inner.config.per_key_limit {
                return Err(OpenError::TooMany(self.inner.config.per_key_limit));
            }
            *held += 1;
            counts.total += 1;
        }
        Ok(StreamGuard {
            inner: Arc::clone(&self.inner),
            key,
            subscriptions: 0,
        })
    }

    /// Takes up to `wanted` places under both subscription caps for the stream of `guard`; the
    /// number granted is returned and released with the guard.
    // The two counts are read and raised under one guard, so concurrent opens cannot pass the cap.
    #[allow(clippy::significant_drop_tightening)]
    fn reserve_subscriptions(&self, guard: &mut StreamGuard, wanted: usize) -> usize {
        let mut counts = self.inner.counts.lock();
        let held = counts.subscriptions_per_key.get(&guard.key).copied().unwrap_or(0);
        let granted = wanted
            .min(self.inner.config.per_key_subscriptions.saturating_sub(held))
            .min(self.inner.config.max_subscriptions.saturating_sub(counts.subscriptions));
        if granted > 0 {
            counts.subscriptions += granted;
            *counts.subscriptions_per_key.entry(guard.key).or_insert(0) += granted;
            guard.subscriptions += granted;
        }
        granted
    }

    /// Opens a stream for `auth`. `id` is the JSON-RPC id of the listen request and `params` its
    /// parameters. The acknowledgment is the first message the handle yields; a stream is
    /// registered for delivery before it is handed out, but nothing is yielded ahead of the
    /// acknowledgment.
    ///
    /// Order: parameters, caps, then (only for a token that asks for something the server can push)
    /// the hub, which asks the API, then the requested URIs, each decided as the caller. A
    /// credential the API refuses fails here, before any stream exists.
    pub async fn open(&self, auth: ListenAuth, id: &Value, params: Option<&Value>) -> Result<ListenHandle, OpenError> {
        self.open_from(None, auth, id, params).await
    }

    /// [`Self::open`] for a request that came from `peer`: a credential the server does not know
    /// is not offered to the API once `peer` has had too many refused (see [`limiter`]).
    pub async fn open_from(
        &self,
        peer: Option<IpAddr>,
        auth: ListenAuth,
        id: &Value,
        params: Option<&Value>,
    ) -> Result<ListenHandle, OpenError> {
        let request = ListenRequest::parse(params)?;
        let mut guard = self.reserve(auth.key())?;
        let candidates = uris::candidates(&request.uris);
        let mut honoured = Honoured::default();
        let mut registration = None;
        if (request.tools_list_changed || !candidates.is_empty())
            && self.inner.config.enabled
            && let ListenAuth::Token { key, client, server } = &auth
        {
            match self
                .inner
                .hubs
                .acquire(*key, client, server, peer, &self.inner.shutdown)
                .await
            {
                Ok(Access::Push(held)) => {
                    honoured.tools_list_changed = request.tools_list_changed;
                    let subscriptions = self.decide_subscriptions(&mut guard, server, candidates).await;
                    held.shared.configure(request.tools_list_changed, &subscriptions);
                    honoured.uris = subscriptions.into_iter().map(|(uri, _)| uri).collect();
                    // A stream with nothing to deliver holds no place on the hub.
                    if honoured.tools_list_changed || !honoured.uris.is_empty() {
                        registration = Some(held);
                    }
                }
                Ok(Access::NoPush) => {}
                Err(AcquireError::Unauthorized) => return Err(OpenError::Unauthorized),
                Err(AcquireError::Unavailable) => return Err(OpenError::Unavailable),
                Err(AcquireError::Busy) => return Err(OpenError::Busy),
                Err(AcquireError::Throttled) => return Err(OpenError::Throttled),
            }
        }
        let ack = acknowledgment(id, &request, &honoured);
        Ok(ListenHandle {
            id: id.clone(),
            pending_ack: Some(ack),
            cancel: CancellationToken::new(),
            shutdown: self.inner.shutdown.clone(),
            finished: false,
            registration,
            _guard: guard,
        })
    }

    /// The requested URIs the caller may follow, in request order, each with its key: decided as
    /// the caller within [`decide::ACK_BUDGET`], then cut to the subscription caps. Whatever is
    /// refused, unknown, undecided or over a cap is simply left out.
    async fn decide_subscriptions(
        &self,
        guard: &mut StreamGuard,
        server: &Arc<McpServer>,
        candidates: Vec<(String, Subscribable)>,
    ) -> Vec<(Arc<str>, SubKey)> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let items: Vec<(Arc<str>, Subscribable)> = candidates
            .into_iter()
            .map(|(uri, parsed)| (Arc::from(uri), parsed))
            .collect();
        let verdicts = decide::decide(server, &items, decide::ACK_BUDGET).await;
        let mut granted: Vec<(Arc<str>, SubKey)> = items
            .into_iter()
            .zip(verdicts)
            .filter_map(|((uri, _), verdict)| match verdict {
                Some(Verdict::Granted(key)) => Some((uri, key)),
                _ => None,
            })
            .collect();
        let allowed = self.reserve_subscriptions(guard, granted.len());
        granted.truncate(allowed);
        granted
    }

    /// Ends every stream with its final result.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Resolves when no stream is open any more.
    pub async fn wait_idle(&self) {
        loop {
            let ended = self.inner.ended.notified();
            tokio::pin!(ended);
            ended.as_mut().enable();
            if self.inner.counts.lock().total == 0 {
                return;
            }
            ended.await;
        }
    }
}

struct StreamGuard {
    inner: Arc<Inner>,
    key: CallerKey,
    /// Subscription places this stream holds under the caps.
    subscriptions: usize,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        {
            let mut counts = self.inner.counts.lock();
            counts.total = counts.total.saturating_sub(1);
            if let Some(held) = counts.per_key.get_mut(&self.key) {
                *held = held.saturating_sub(1);
                if *held == 0 {
                    counts.per_key.remove(&self.key);
                }
            }
            if self.subscriptions > 0 {
                counts.subscriptions = counts.subscriptions.saturating_sub(self.subscriptions);
                if let Some(held) = counts.subscriptions_per_key.get_mut(&self.key) {
                    *held = held.saturating_sub(self.subscriptions);
                    if *held == 0 {
                        counts.subscriptions_per_key.remove(&self.key);
                    }
                }
            }
        }
        self.inner.ended.notify_waiters();
    }
}

/// One open listen stream. Dropping it releases its place in the caps and on its hub, so whoever
/// owns the handle owns the slot: the task that forwards the stream, never the response body.
pub struct ListenHandle {
    id: Value,
    pending_ack: Option<Value>,
    cancel: CancellationToken,
    shutdown: CancellationToken,
    finished: bool,
    /// Present only on a stream that receives pushes.
    registration: Option<Registration>,
    _guard: StreamGuard,
}

impl ListenHandle {
    /// Cancels the stream; it ends without a final result (a client cancel).
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// The next message for the client: the acknowledgment first, then notifications as hints
    /// arrive, then the final result (when the stream has nothing to deliver, at once; when the
    /// credential is revoked or the server stops, then). `None` when the stream is over (after the
    /// final result or a cancel). Waits otherwise; callers also watch their connection.
    pub async fn next_message(&mut self) -> Option<Value> {
        if let Some(ack) = self.pending_ack.take() {
            return Some(ack);
        }
        if self.finished {
            return None;
        }
        let Some(shared) = self
            .registration
            .as_ref()
            .map(|registration| Arc::clone(&registration.shared))
        else {
            self.finished = true;
            return Some(final_result(&self.id));
        };
        loop {
            match shared.take() {
                Take::Ended => {
                    self.finished = true;
                    return Some(final_result(&self.id));
                }
                Take::Tools => return Some(tools_list_changed(&self.id)),
                Take::Uri(uri) => return Some(resource_updated(&self.id, &uri)),
                Take::Nothing => {}
            }
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => {
                    self.finished = true;
                    return None;
                }
                () = self.shutdown.cancelled() => {
                    self.finished = true;
                    return Some(final_result(&self.id));
                }
                () = shared.changed() => {}
            }
        }
    }

    /// Moves every message of the stream into `tx` (wrapped by `wrap`) until the stream or the
    /// receiver ends. This task owns the stream: a consumer that leaves `tx` full for
    /// [`STALL_LIMIT`] has its stream ended (no final result; it is not reading), so a client
    /// that stops reading cannot pin a slot, and no hint ever waits for it.
    pub async fn forward<T>(mut self, tx: &mpsc::Sender<T>, stall: Duration, wrap: impl Fn(String) -> T) {
        loop {
            let message = tokio::select! {
                message = self.next_message() => message,
                () = tx.closed() => return,
            };
            let Some(message) = message else { return };
            match tokio::time::timeout(stall, tx.send(wrap(message.to_string()))).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return,
                Err(_) => {
                    tracing::info!("A listen stream was ended: its consumer stopped reading");
                    return;
                }
            }
        }
    }
}

fn subscription_meta(id: &Value) -> Value {
    json!({ META_SUBSCRIPTION_ID: id })
}

/// What a listen request asks for, validated.
#[derive(Debug, Default, PartialEq, Eq)]
struct ListenRequest {
    tools_list_changed: bool,
    /// Whether the request carried `resourceSubscriptions` (the acknowledgment then carries it).
    resource_subscriptions: bool,
    /// The strings of `resourceSubscriptions`, in request order.
    uris: Vec<String>,
}

impl ListenRequest {
    fn parse(params: Option<&Value>) -> Result<Self, OpenError> {
        let mut request = Self::default();
        let Some(notifications) = params.and_then(|params| params.get("notifications")) else {
            return Ok(request);
        };
        let Some(notifications) = notifications.as_object() else {
            return Err(OpenError::InvalidParams("notifications must be an object".to_string()));
        };
        request.tools_list_changed = notifications.get("toolsListChanged") == Some(&Value::Bool(true));
        if let Some(subscriptions) = notifications.get("resourceSubscriptions") {
            let valid = subscriptions
                .as_array()
                .is_some_and(|uris| uris.iter().all(Value::is_string));
            if !valid {
                return Err(OpenError::InvalidParams(
                    "resourceSubscriptions must be an array of strings".to_string(),
                ));
            }
            request.resource_subscriptions = true;
            request.uris = subscriptions
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|uri| uri.as_str().map(str::to_string))
                .collect();
        }
        Ok(request)
    }
}

/// What the server honours of a request.
#[derive(Debug, Default, Clone)]
struct Honoured {
    tools_list_changed: bool,
    /// The honoured URIs, in request order, as the client wrote them.
    uris: Vec<Arc<str>>,
}

/// The acknowledgment of a listen request. Boolean keys appear only when requested and honoured;
/// `resourceSubscriptions` appears iff the request had it, holding the honoured subset.
fn acknowledgment(id: &Value, request: &ListenRequest, honoured: &Honoured) -> Value {
    let mut notifications = Map::new();
    if honoured.tools_list_changed {
        notifications.insert("toolsListChanged".to_string(), Value::Bool(true));
    }
    if request.resource_subscriptions {
        notifications.insert("resourceSubscriptions".to_string(), json!(honoured.uris));
    }
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/subscriptions/acknowledged",
        "params": { "_meta": subscription_meta(id), "notifications": notifications },
    })
}

/// `notifications/resources/updated` for one subscribed URI, echoed as the client wrote it.
fn resource_updated(id: &Value, uri: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/resources/updated",
        "params": { "_meta": subscription_meta(id), "uri": uri },
    })
}

fn tools_list_changed(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/tools/list_changed",
        "params": { "_meta": subscription_meta(id) },
    })
}

/// The result that ends a listen request when the server ends it.
fn final_result(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "resultType": "complete", "_meta": subscription_meta(id) },
    })
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::client::{ClientConfig, TRANSPORT_LABEL_HTTP};
    use platform::config::Secret;

    /// A client that never reaches an API: a stream that needs one fails as unavailable.
    fn dead_client() -> OpenPrClient {
        OpenPrClient::new(ClientConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            credential: Some(Secret::new("opr_unit")),
            workspace_id: None,
            transport_label: TRANSPORT_LABEL_HTTP,
        })
        .expect("a client builds")
    }

    fn token(name: &str) -> ListenAuth {
        let client = dead_client();
        ListenAuth::Token {
            key: caller_key(name),
            server: Arc::new(McpServer::new(client.clone(), platform::config::McpToolMode::Full)),
            client,
        }
    }

    fn delegated(name: &str) -> ListenAuth {
        ListenAuth::Delegated { key: caller_key(name) }
    }

    fn registry(max_streams: usize, per_key_limit: usize) -> ListenRegistry {
        ListenRegistry::new(ListenConfig {
            enabled: true,
            max_streams,
            per_key_limit,
            max_subscriptions: 100,
            per_key_subscriptions: 100,
            keepalive: Duration::from_millis(50),
        })
    }

    #[tokio::test]
    async fn a_stream_with_nothing_to_deliver_ends_after_the_acknowledgment_and_frees_its_slot() {
        let registry = registry(1, 1);
        for round in 0..5 {
            let Ok(mut handle) = registry.open(delegated("a"), &json!(round), None).await else {
                panic!("round {round} must open");
            };
            let ack = handle.next_message().await.unwrap_or(Value::Null);
            assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
            let end = handle.next_message().await.unwrap_or(Value::Null);
            assert_eq!(end["id"], round);
            assert_eq!(end["result"]["resultType"], "complete");
            assert_eq!(handle.next_message().await, None);
            drop(handle);
            assert_eq!(registry.open_streams(), 0);
        }
    }

    #[test]
    fn the_envelope_needs_version_two_and_a_string_or_number_id() {
        let request = |jsonrpc: &str, id: Option<Value>| JsonRpcRequest {
            jsonrpc: jsonrpc.to_string(),
            id,
            method: "subscriptions/listen".to_string(),
            params: None,
        };
        assert!(validate_envelope(&request("2.0", Some(json!(1)))).is_ok());
        assert!(validate_envelope(&request("2.0", Some(json!("a")))).is_ok());
        for bad in [
            request("1.0", Some(json!(1))),
            request("2.0", Some(json!({"a": 1}))),
            request("2.0", Some(json!([1]))),
            request("2.0", Some(json!(null))),
            request("2.0", Some(json!(true))),
            request("2.0", None),
        ] {
            assert_eq!(validate_envelope(&bad).err().map(|error| error.code), Some(-32600));
        }
    }

    #[tokio::test]
    async fn a_delegated_caller_gets_the_empty_subset_and_no_hub() {
        let registry = registry(10, 10);
        let params =
            json!({"notifications": {"toolsListChanged": true, "resourceSubscriptions": ["sylvode://tasks/0F"]}});
        let Ok(mut handle) = registry.open(delegated("a"), &json!(7), Some(&params)).await else {
            panic!("must open");
        };
        let ack = handle.next_message().await;
        assert_eq!(
            ack,
            Some(json!({
                "jsonrpc": "2.0",
                "method": "notifications/subscriptions/acknowledged",
                "params": {
                    "_meta": {"io.modelcontextprotocol/subscriptionId": 7},
                    "notifications": {"resourceSubscriptions": []}
                }
            }))
        );
        assert_eq!(registry.hub_count(), 0);
        assert_eq!(handle.next_message().await.map(|end| end["id"].clone()), Some(json!(7)));
    }

    #[tokio::test]
    async fn the_subscription_id_keeps_the_json_type_of_the_request_id() {
        let registry = registry(10, 10);
        for id in [json!(7), json!("7"), json!("abc")] {
            let Ok(mut handle) = registry.open(delegated("a"), &id, None).await else {
                panic!("must open");
            };
            let ack = handle.next_message().await.unwrap_or(Value::Null);
            assert_eq!(ack["params"]["_meta"][META_SUBSCRIPTION_ID], id);
            assert_eq!(ack["params"]["notifications"], json!({}));
        }
    }

    #[tokio::test]
    async fn malformed_notifications_are_refused() {
        let registry = registry(10, 10);
        for params in [
            json!({"notifications": 1}),
            json!({"notifications": {"resourceSubscriptions": "x"}}),
            json!({"notifications": {"resourceSubscriptions": [1]}}),
        ] {
            assert!(matches!(
                registry.open(delegated("a"), &json!(1), Some(&params)).await,
                Err(OpenError::InvalidParams(_))
            ));
        }
        assert_eq!(registry.open_streams(), 0);
    }

    #[tokio::test]
    async fn a_token_that_asks_for_tools_and_cannot_reach_the_api_is_unavailable_and_holds_nothing() {
        let registry = registry(10, 10);
        let params = json!({"notifications": {"toolsListChanged": true}});
        let opened = registry.open(token("opr_a"), &json!(1), Some(&params)).await;
        assert!(matches!(opened, Err(OpenError::Unavailable)));
        assert_eq!(registry.open_streams(), 0);
        assert_eq!(registry.hub_count(), 0);
    }

    #[tokio::test]
    async fn a_token_that_asks_for_nothing_never_asks_the_api() {
        let registry = registry(10, 10);
        let Ok(mut handle) = registry.open(token("opr_a"), &json!(1), None).await else {
            panic!("must open");
        };
        let _ack = handle.next_message().await;
        assert_eq!(handle.next_message().await.map(|end| end["id"].clone()), Some(json!(1)));
        assert_eq!(registry.hub_count(), 0);
    }

    #[tokio::test]
    async fn the_per_key_cap_counts_one_credential_only() {
        let registry = registry(10, 2);
        let a1 = registry.open(delegated("a"), &json!(1), None).await;
        let a2 = registry.open(delegated("a"), &json!(2), None).await;
        assert!(a1.is_ok() && a2.is_ok());
        assert!(matches!(
            registry.open(delegated("a"), &json!(3), None).await,
            Err(OpenError::TooMany(2))
        ));
        assert!(registry.open(delegated("b"), &json!(1), None).await.is_ok());
        drop(a1);
        assert!(registry.open(delegated("a"), &json!(4), None).await.is_ok());
    }

    #[tokio::test]
    async fn the_process_cap_counts_every_credential() {
        let registry = registry(2, 10);
        let first = registry.open(delegated("a"), &json!(1), None).await;
        let second = registry.open(delegated("b"), &json!(1), None).await;
        assert!(first.is_ok() && second.is_ok());
        assert!(matches!(
            registry.open(delegated("c"), &json!(1), None).await,
            Err(OpenError::TooMany(2))
        ));
        assert_eq!(registry.open_streams(), 2);
    }

    #[test]
    fn the_errors_map_to_the_documented_json_rpc_errors() {
        let error = OpenError::TooMany(1_000).into_rpc_error();
        assert_eq!(error.code, -32000);
        assert_eq!(error.data, Some(json!({"limit": 1000})));
        let error = OpenError::Unauthorized.into_rpc_error();
        assert_eq!(error.code, -32603);
        assert_eq!(error.data, Some(json!({"reason": "unauthorized"})));
        let error = OpenError::Unavailable.into_rpc_error();
        assert_eq!(error.code, -32603);
        assert_eq!(error.data, None);
    }

    #[test]
    fn the_key_is_a_digest_not_the_token() {
        let key = caller_key("opr_secret_token");
        assert_ne!(&key[..], b"opr_secret_token");
        assert_eq!(key, caller_key("opr_secret_token"));
        assert_ne!(key, caller_key("opr_other_token"));
    }

    #[test]
    fn the_acknowledgment_follows_the_rules_for_each_request_shape() {
        let id = json!(3);
        let ask = |tools: bool, uris: bool| ListenRequest {
            tools_list_changed: tools,
            resource_subscriptions: uris,
            uris: Vec::new(),
        };
        let notifications = |request: &ListenRequest, honoured: &Honoured| {
            acknowledgment(&id, request, honoured)["params"]["notifications"].clone()
        };
        let honoured = Honoured {
            tools_list_changed: true,
            uris: Vec::new(),
        };
        assert_eq!(
            notifications(&ask(true, false), &honoured),
            json!({"toolsListChanged": true})
        );
        assert_eq!(notifications(&ask(true, false), &Honoured::default()), json!({}));
        assert_eq!(
            notifications(&ask(true, true), &honoured),
            json!({"toolsListChanged": true, "resourceSubscriptions": []})
        );
        assert_eq!(notifications(&ask(false, false), &Honoured::default()), json!({}));
    }

    #[test]
    fn only_a_literal_true_requests_the_tool_list() {
        for (params, expected) in [
            (json!({"notifications": {"toolsListChanged": true}}), true),
            (json!({"notifications": {"toolsListChanged": false}}), false),
            (json!({"notifications": {"toolsListChanged": "true"}}), false),
            (json!({"notifications": {"toolsListChanged": 1}}), false),
            (json!({}), false),
        ] {
            let parsed = ListenRequest::parse(Some(&params));
            assert_eq!(
                parsed.map(|request| request.tools_list_changed),
                Ok(expected),
                "{params}"
            );
        }
    }

    #[tokio::test]
    async fn a_stalled_consumer_frees_the_slot() {
        // The stall rule through `forward`, with a consumer whose queue is full.
        let registry = registry(10, 10);
        let Ok(handle) = registry.open(delegated("a"), &json!(1), None).await else {
            panic!("must open");
        };
        let (tx, _rx) = mpsc::channel::<String>(1);
        // The ack fills the queue; the final result then waits and the stall ends the stream.
        handle.forward(&tx, Duration::from_millis(50), |message| message).await;
        assert_eq!(registry.open_streams(), 0);
    }

    #[tokio::test]
    async fn wait_idle_resolves_when_the_last_stream_is_dropped() {
        let registry = registry(10, 10);
        let handle = registry.open(delegated("a"), &json!(7), None).await;
        assert!(handle.is_ok());
        let waiter = {
            let registry = registry.clone();
            tokio::spawn(async move { registry.wait_idle().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        drop(handle);
        let finished = tokio::time::timeout(Duration::from_secs(2), waiter).await;
        assert!(finished.is_ok());
        assert_eq!(registry.open_streams(), 0);
    }
}
