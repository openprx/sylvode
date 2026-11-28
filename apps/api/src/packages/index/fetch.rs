//! Fetching a document or an image of an index source (contract M15, section 3).
//!
//! Every request goes through the outbound address policy: the URL is checked and resolved first
//! and the connection is pinned to the addresses that were checked. No redirect is followed (a
//! `3xx` answer is a failure), the whole exchange has 15 s (5 s to connect), and the body is
//! counted on the bytes read, never trusted from `Content-Length`. Only `https` is fetched.

use std::time::Duration;

use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH, USER_AGENT};

use crate::outbound::{delivery_client, resolve_outbound_target};

const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_NAME: &str = "Sylvode-plugin-index";

/// Why a fetch gave nothing. The names go into the source status and the event log; none carries
/// a URL or an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FetchError {
    /// The address policy or the URL rules refused the target.
    UrlRefused,
    Timeout,
    TooLarge,
    /// The server answered something other than `200` (or `304` for a conditional request).
    HttpStatus(u16),
    /// Connection, TLS or body read failure.
    Transport,
}

impl FetchError {
    /// The text recorded as the source's `error`.
    pub(super) fn text(&self) -> String {
        match self {
            Self::UrlRefused => "url_refused".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::TooLarge => "too_large".to_string(),
            Self::HttpStatus(status) => format!("http_status:{status}"),
            Self::Transport => "transport".to_string(),
        }
    }
}

/// The answer to a fetch.
pub(super) enum Answer {
    /// `304` to a request that carried an `ETag`.
    NotModified,
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

/// Fetches `url`: at most `limit` body bytes, with `If-None-Match` when `etag` is given.
pub(super) async fn get(url: &str, limit: u64, etag: Option<&str>, accept: &str) -> Result<Answer, FetchError> {
    let target = resolve_outbound_target(url).await.map_err(|error| {
        tracing::warn!(%error, "plugin index target refused");
        FetchError::UrlRefused
    })?;
    if target.url.scheme() != "https" {
        return Err(FetchError::UrlRefused);
    }
    let client = delivery_client(target.pin.as_ref(), TOTAL_TIMEOUT, CONNECT_TIMEOUT).map_err(|error| {
        tracing::warn!(%error, "plugin index client could not be built");
        FetchError::Transport
    })?;
    let mut request = client
        .get(target.url)
        .header(USER_AGENT, CLIENT_NAME)
        .header(ACCEPT, accept);
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let mut response = request.send().await.map_err(classify)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED && etag.is_some() {
        return Ok(Answer::NotModified);
    }
    if status != reqwest::StatusCode::OK {
        return Err(FetchError::HttpStatus(status.as_u16()));
    }
    if response.content_length().is_some_and(|length| length > limit) {
        return Err(FetchError::TooLarge);
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 256 && value.bytes().all(|b| (0x20..0x7f).contains(&b)))
        .map(str::to_owned);
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify)? {
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > limit {
            return Err(FetchError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Answer::Body { bytes, etag })
}

fn classify(error: reqwest::Error) -> FetchError {
    if error.is_timeout() {
        FetchError::Timeout
    } else {
        tracing::warn!(error = %error.without_url(), "plugin index request failed");
        FetchError::Transport
    }
}
