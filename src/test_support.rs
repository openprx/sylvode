//! A recording HTTP server for unit tests of outbound requests.

use axum::{Router, body::Bytes, extract::State, http::HeaderMap, routing::post};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;

/// One request the server received.
#[derive(Debug)]
pub struct Received {
    pub headers: HeaderMap,
    pub raw: Bytes,
    pub body: Value,
}

/// Answers every POST with a JSON-RPC success and records the request.
pub struct Recorder {
    pub url: String,
    received: mpsc::UnboundedReceiver<Received>,
    task: tokio::task::JoinHandle<()>,
}

async fn record(State(tx): State<mpsc::UnboundedSender<Received>>, headers: HeaderMap, raw: Bytes) -> &'static str {
    let body = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let _ = tx.send(Received { headers, raw, body });
    r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#
}

impl Recorder {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind recorder");
        let url = format!("http://{}/hook", listener.local_addr().expect("recorder address"));
        let (tx, received) = mpsc::unbounded_channel();
        let app = Router::new().route("/hook", post(record)).with_state(tx);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { url, received, task }
    }

    /// The next request, or `None` when none arrives within `timeout`.
    pub async fn next(&mut self, timeout: Duration) -> Option<Received> {
        tokio::time::timeout(timeout, self.received.recv()).await.ok().flatten()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.task.abort();
    }
}
