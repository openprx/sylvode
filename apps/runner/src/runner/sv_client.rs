//! The client for the Sylvode task protocol, used with one bot's token.
//!
//! The token is held in a private field, sent only as the bearer header, never printed (`Debug` is
//! written by hand and shows `<redacted>`), and never part of an error: errors carry a status code
//! or a kind, not a URL, a header or a body.

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};

/// The task protocol version this runner speaks.
pub const TASK_PROTOCOL: u32 = 1;

/// The longest body read from an answer.
const MAX_ANSWER_BYTES: usize = 1024 * 1024;

/// Why a call to Sylvode did not give an answer to use.
#[derive(Debug, PartialEq, Eq)]
pub enum SvError {
    /// No answer in time.
    Timeout,
    /// The server could not be reached or answered something unreadable.
    Transport,
    /// An HTTP status other than success.
    Status(u16),
}

impl SvError {
    /// Whether the delivery should be answered 503 so the worker re-queues it: a timeout, a
    /// transport failure or a 5xx.
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::Timeout | Self::Transport) || matches!(self, Self::Status(code) if *code >= 500)
    }
}

impl std::fmt::Display for SvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "Sylvode did not answer in time"),
            Self::Transport => write!(f, "Sylvode could not be reached"),
            Self::Status(code) => write!(f, "Sylvode answered HTTP {code}"),
        }
    }
}

impl std::error::Error for SvError {}

/// The header that names the transport a bot token is registered with.
pub const MCP_SURFACE_HEADER: &str = "X-Sylvode-MCP-Surface";

/// One bot's connection to the Sylvode API.
#[derive(Clone)]
pub struct SvClient {
    http: reqwest::Client,
    api_url: String,
    token: String,
    surface: String,
}

impl std::fmt::Debug for SvClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvClient")
            .field("api_url", &self.api_url)
            .field("token", &"<redacted>")
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl SvClient {
    /// `api_url` is the base URL (validated by the configuration: no credentials, query or
    /// fragment). `surface` is the transport the token is registered with; `rest` sends no header.
    pub fn new(http: reqwest::Client, api_url: &str, token: &str, surface: &str) -> Self {
        Self {
            http,
            api_url: api_url.trim().trim_end_matches('/').to_string(),
            token: token.to_string(),
            surface: surface.to_string(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str, timeout: Duration) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.api_url))
            .bearer_auth(&self.token)
            .timeout(timeout);
        if self.surface != "rest" {
            request = request.header(MCP_SURFACE_HEADER, &self.surface);
        }
        request
    }

    async fn send(request: reqwest::RequestBuilder) -> Result<Value, SvError> {
        let mut response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                SvError::Timeout
            } else {
                SvError::Transport
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(SvError::Status(status.as_u16()));
        }
        // The cap is enforced while reading: a declared length over it is refused before any
        // byte is read, and a body without one (or lying about it) stops at the cap.
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ANSWER_BYTES as u64)
        {
            return Err(SvError::Transport);
        }
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            if e.is_timeout() {
                SvError::Timeout
            } else {
                SvError::Transport
            }
        })? {
            if bytes.len() + chunk.len() > MAX_ANSWER_BYTES {
                return Err(SvError::Transport);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice::<Value>(&bytes).map_err(|_| SvError::Transport)
    }

    /// `GET /api/v1/ai/tasks/{id}?attempt=N`: the task view (the `data` of the envelope).
    ///
    /// # Errors
    ///
    /// [`SvError`]; a 400 means the attempt is not active.
    pub async fn task_view(&self, task_id: &str, attempt: i64, timeout: Duration) -> Result<Value, SvError> {
        let path = format!("/api/v1/ai/tasks/{}?attempt={attempt}", encode(task_id));
        let answer = Self::send(self.request(reqwest::Method::GET, &path, timeout)).await?;
        answer.get("data").cloned().ok_or(SvError::Transport)
    }

    /// `GET /api/v1/ai/tasks/active`: the bot's active task views, oldest first.
    ///
    /// # Errors
    ///
    /// [`SvError`].
    #[allow(dead_code, reason = "restart recovery of the session manager uses it")]
    pub async fn active_tasks(&self, timeout: Duration) -> Result<Vec<Value>, SvError> {
        let answer = Self::send(self.request(reqwest::Method::GET, "/api/v1/ai/tasks/active", timeout)).await?;
        Ok(answer
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// `POST .../progress` with a `runner.*` kind.
    ///
    /// # Errors
    ///
    /// [`SvError`].
    pub async fn progress(
        &self,
        task_id: &str,
        attempt: i64,
        kind: &str,
        note: &str,
        queue_position: Option<i32>,
        timeout: Duration,
    ) -> Result<(), SvError> {
        let path = format!("/api/v1/ai/tasks/{}/progress?attempt={attempt}", encode(task_id));
        let body = json!({"kind": kind, "note": note, "queue_position": queue_position});
        Self::send(self.request(reqwest::Method::POST, &path, timeout).json(&body))
            .await
            .map(|_| ())
    }

    /// `POST .../complete` with outcome `done`: the runner ends a task whose hand-over to a
    /// forwarding agent worked.
    ///
    /// # Errors
    ///
    /// [`SvError`].
    pub async fn complete(&self, task_id: &str, attempt: i64, summary: &str, timeout: Duration) -> Result<(), SvError> {
        let path = format!("/api/v1/ai/tasks/{}/complete?attempt={attempt}", encode(task_id));
        let body = json!({"outcome": "done", "summary": summary});
        Self::send(self.request(reqwest::Method::POST, &path, timeout).json(&body))
            .await
            .map(|_| ())
    }

    /// `POST .../fail` with `source: "runner"`.
    ///
    /// # Errors
    ///
    /// [`SvError`].
    pub async fn fail(
        &self,
        task_id: &str,
        attempt: i64,
        reason: &str,
        retryable: bool,
        timeout: Duration,
    ) -> Result<(), SvError> {
        let path = format!("/api/v1/ai/tasks/{}/fail?attempt={attempt}", encode(task_id));
        let body = json!({"reason": reason, "retryable": retryable, "source": "runner"});
        Self::send(self.request(reqwest::Method::POST, &path, timeout).json(&body))
            .await
            .map(|_| ())
    }

    /// `GET /api/v1/ai/protocol`: the `data` of the answer.
    ///
    /// # Errors
    ///
    /// [`SvError`].
    pub async fn protocol(&self, timeout: Duration) -> Result<Value, SvError> {
        let answer = Self::send(self.request(reqwest::Method::GET, "/api/v1/ai/protocol", timeout)).await?;
        answer.get("data").cloned().ok_or(SvError::Transport)
    }

    /// `PUT /api/v1/ai/runner/report` with `body` (already reduced to what Sylvode keeps).
    ///
    /// # Errors
    ///
    /// [`SvError`]; a 404 means the bot has no runner connection.
    pub async fn put_report(&self, body: &Value, timeout: Duration) -> Result<(), SvError> {
        Self::send(
            self.request(reqwest::Method::PUT, "/api/v1/ai/runner/report", timeout)
                .json(body),
        )
        .await
        .map(|_| ())
    }
}

/// Percent-encodes a path segment: a task id is a UUID, but the value comes from a delivery.
fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Whether `status` means the token was refused.
pub fn token_refused(error: &SvError) -> bool {
    matches!(error, SvError::Status(code) if StatusCode::from_u16(*code).is_ok_and(|s| s == StatusCode::UNAUTHORIZED || s == StatusCode::FORBIDDEN))
}
