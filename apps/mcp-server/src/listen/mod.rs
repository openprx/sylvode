//! `subscriptions/listen` of the modern era: the registry of open listen streams.
//!
//! This is the transport-neutral part. A stream belongs to the credential that opened it and is
//! counted against two caps: streams per caller key (a SHA-256 of the bearer token, never the
//! token) and streams per process. In this skeleton the acknowledgment always carries the empty
//! subset and nothing is delivered afterwards; what exists is everything a transport needs to
//! carry a long-lived request: cancellation, shutdown with a graceful final result, caps and
//! the keep-alive interval.

use crate::era::META_SUBSCRIPTION_ID;
use crate::protocol::JsonRpcError;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Open listen streams one credential may hold.
pub const MAX_STREAMS_PER_KEY: usize = 1_000;
/// Comment line interval on an idle listen response.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// `Retry-After` of a refused listen request, in seconds.
pub const RETRY_AFTER_SECS: u64 = 30;

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
    pub keepalive: Duration,
}

impl ListenConfig {
    /// The production configuration for a total of `max_streams`.
    pub const fn new(enabled: bool, max_streams: usize) -> Self {
        Self {
            enabled,
            max_streams,
            per_key_limit: MAX_STREAMS_PER_KEY,
            keepalive: KEEPALIVE_INTERVAL,
        }
    }
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self::new(true, 10_000)
    }
}

/// Why a stream was not opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// A cap is reached; the number is the cap that was hit.
    TooMany(usize),
    /// The request parameters are malformed.
    InvalidParams(String),
}

impl OpenError {
    pub fn into_rpc_error(self) -> JsonRpcError {
        match self {
            Self::TooMany(limit) => JsonRpcError::too_many_streams(u32::try_from(limit).unwrap_or(u32::MAX)),
            Self::InvalidParams(message) => JsonRpcError::invalid_params(message),
        }
    }
}

#[derive(Default)]
struct Counts {
    total: usize,
    per_key: HashMap<CallerKey, usize>,
}

struct Inner {
    config: ListenConfig,
    counts: Mutex<Counts>,
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

    /// Opens a stream for the credential `key`. `id` is the JSON-RPC id of the listen request and
    /// `params` its parameters. The acknowledgment is the first message the handle yields.
    pub fn open(&self, key: CallerKey, id: &Value, params: Option<&Value>) -> Result<ListenHandle, OpenError> {
        let ack = acknowledgment(id, params)?;
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
        Ok(ListenHandle {
            id: id.clone(),
            pending_ack: Some(ack),
            cancel: CancellationToken::new(),
            shutdown: self.inner.shutdown.clone(),
            finished: false,
            _guard: StreamGuard {
                inner: Arc::clone(&self.inner),
                key,
            },
        })
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
        }
        self.inner.ended.notify_waiters();
    }
}

/// One open listen stream. Dropping it releases its place in the caps.
pub struct ListenHandle {
    id: Value,
    pending_ack: Option<Value>,
    cancel: CancellationToken,
    shutdown: CancellationToken,
    finished: bool,
    _guard: StreamGuard,
}

impl ListenHandle {
    /// Cancels the stream; it ends without a final result (a client cancel).
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// The next message for the client: the acknowledgment first, then, once the server stops,
    /// the final result; `None` when the stream is over (after the final result or a cancel).
    /// Waits otherwise; callers also watch their connection.
    pub async fn next_message(&mut self) -> Option<Value> {
        if let Some(ack) = self.pending_ack.take() {
            return Some(ack);
        }
        if self.finished {
            return None;
        }
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => {
                self.finished = true;
                None
            }
            () = self.shutdown.cancelled() => {
                self.finished = true;
                Some(final_result(&self.id))
            }
        }
    }
}

fn subscription_meta(id: &Value) -> Value {
    json!({ META_SUBSCRIPTION_ID: id })
}

/// The acknowledgment of a listen request: the honoured subset, which is empty here.
fn acknowledgment(id: &Value, params: Option<&Value>) -> Result<Value, OpenError> {
    let mut honoured = Map::new();
    if let Some(notifications) = params.and_then(|params| params.get("notifications")) {
        let Some(notifications) = notifications.as_object() else {
            return Err(OpenError::InvalidParams("notifications must be an object".to_string()));
        };
        if let Some(subscriptions) = notifications.get("resourceSubscriptions") {
            let valid = subscriptions
                .as_array()
                .is_some_and(|uris| uris.iter().all(Value::is_string));
            if !valid {
                return Err(OpenError::InvalidParams(
                    "resourceSubscriptions must be an array of strings".to_string(),
                ));
            }
            honoured.insert("resourceSubscriptions".to_string(), json!([]));
        }
    }
    Ok(json!({
        "jsonrpc": "2.0",
        "method": "notifications/subscriptions/acknowledged",
        "params": { "_meta": subscription_meta(id), "notifications": honoured },
    }))
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

    fn registry(max_streams: usize, per_key_limit: usize) -> ListenRegistry {
        ListenRegistry::new(ListenConfig {
            enabled: true,
            max_streams,
            per_key_limit,
            keepalive: Duration::from_millis(50),
        })
    }

    #[tokio::test]
    async fn the_first_message_is_the_acknowledgment_with_the_empty_subset() {
        let registry = registry(10, 10);
        let id = json!(7);
        let params =
            json!({"notifications": {"toolsListChanged": true, "resourceSubscriptions": ["sylvode://tasks/0F"]}});
        let Ok(mut handle) = registry.open(caller_key("a"), &id, Some(&params)) else {
            assert!(registry.enabled(), "open must succeed");
            return;
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
    }

    #[tokio::test]
    async fn the_subscription_id_keeps_the_json_type_of_the_request_id() {
        let registry = registry(10, 10);
        for id in [json!(7), json!("7"), json!("abc")] {
            let handle = registry.open(caller_key("a"), &id, None);
            assert!(handle.is_ok());
            let Ok(mut handle) = handle else { return };
            let ack = handle.next_message().await.unwrap_or(Value::Null);
            assert_eq!(ack["params"]["_meta"][META_SUBSCRIPTION_ID], id);
            assert_eq!(ack["params"]["notifications"], json!({}));
        }
    }

    #[test]
    fn malformed_notifications_are_refused() {
        let registry = registry(10, 10);
        for params in [
            json!({"notifications": 1}),
            json!({"notifications": {"resourceSubscriptions": "x"}}),
            json!({"notifications": {"resourceSubscriptions": [1]}}),
        ] {
            assert!(matches!(
                registry.open(caller_key("a"), &json!(1), Some(&params)),
                Err(OpenError::InvalidParams(_))
            ));
        }
        assert_eq!(registry.open_streams(), 0);
    }

    #[test]
    fn the_per_key_cap_counts_one_credential_only() {
        let registry = registry(10, 2);
        let a1 = registry.open(caller_key("a"), &json!(1), None);
        let a2 = registry.open(caller_key("a"), &json!(2), None);
        assert!(a1.is_ok() && a2.is_ok());
        assert!(matches!(
            registry.open(caller_key("a"), &json!(3), None),
            Err(OpenError::TooMany(2))
        ));
        assert!(registry.open(caller_key("b"), &json!(1), None).is_ok());
        drop(a1);
        assert!(registry.open(caller_key("a"), &json!(4), None).is_ok());
    }

    #[test]
    fn the_process_cap_counts_every_credential() {
        let registry = registry(2, 10);
        let first = registry.open(caller_key("a"), &json!(1), None);
        let second = registry.open(caller_key("b"), &json!(1), None);
        assert!(first.is_ok() && second.is_ok());
        assert!(matches!(
            registry.open(caller_key("c"), &json!(1), None),
            Err(OpenError::TooMany(2))
        ));
        assert_eq!(registry.open_streams(), 2);
    }

    #[test]
    fn too_many_maps_to_the_documented_error() {
        let error = OpenError::TooMany(1_000).into_rpc_error();
        assert_eq!(error.code, -32000);
        assert_eq!(error.data, Some(json!({"limit": 1000})));
    }

    #[test]
    fn the_key_is_a_digest_not_the_token() {
        let key = caller_key("opr_secret_token");
        assert_ne!(&key[..], b"opr_secret_token");
        assert_eq!(key, caller_key("opr_secret_token"));
        assert_ne!(key, caller_key("opr_other_token"));
    }

    #[tokio::test]
    async fn shutdown_ends_a_stream_with_the_final_result_once() {
        let registry = registry(10, 10);
        let Ok(mut handle) = registry.open(caller_key("a"), &json!(7), None) else {
            return;
        };
        let _ack = handle.next_message().await;
        registry.shutdown();
        let result = handle.next_message().await;
        assert_eq!(
            result,
            Some(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "result": {"resultType": "complete", "_meta": {"io.modelcontextprotocol/subscriptionId": 7}}
            }))
        );
        assert_eq!(handle.next_message().await, None);
    }

    #[tokio::test]
    async fn a_client_cancel_ends_a_stream_without_a_final_result() {
        let registry = registry(10, 10);
        let Ok(mut handle) = registry.open(caller_key("a"), &json!(7), None) else {
            return;
        };
        let _ack = handle.next_message().await;
        handle.cancel_token().cancel();
        assert_eq!(handle.next_message().await, None);
    }

    #[tokio::test]
    async fn wait_idle_resolves_when_the_last_stream_is_dropped() {
        let registry = registry(10, 10);
        let handle = registry.open(caller_key("a"), &json!(7), None);
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
