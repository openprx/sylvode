//! Downloading the package file of an index entry (contract M15, section 3, decision O5).
//!
//! The URL comes from the cached index, which is not signed, so the download is as restricted as
//! an outbound request can be:
//!
//! - every hop (the entry's URL and each redirect target) is `https`, has no credentials, uses port
//!   443 or none, names a DNS host (never an address literal) that `index_download_hosts` allows
//!   (empty list: any host), and goes through the outbound address policy, with the connection
//!   pinned to the addresses that were checked;
//! - at most 3 redirects are followed; nothing but a fixed `User-Agent` and `Accept` is sent, so no
//!   header survives a hop;
//! - the body is counted on the bytes read and the stream stops when it passes the declared `size`;
//! - the whole exchange, redirects included, has 60 s (5 s to connect per hop).
//!
//! The bytes are not trusted when they arrive: the caller checks `size`, `sha256` and the signature.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use reqwest::header::{ACCEPT, LOCATION, USER_AGENT};
use reqwest::{StatusCode, Url};
use sv_index::{ExactHosts, HostPolicy, download_url_problem};

use crate::outbound::{delivery_client, resolve_outbound_target};

const TOTAL_TIMEOUT: Duration = Duration::from_mins(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REDIRECTS: usize = 3;
/// A redirect target longer than this is refused.
const MAX_HOP_URL_CHARS: usize = 2048;
const CLIENT_NAME: &str = "Sylvode-plugin-index";

/// Why a download gave nothing. The reason names go to `details.reason` of
/// `index_download_failed`; none carries a URL or an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DownloadError {
    /// A hop broke the URL rules, the host list or the address policy.
    UrlRefused,
    Timeout,
    /// The server sent, or announced, more than the declared size.
    TooLarge,
    /// The final answer was not `200`, or a redirect had no usable target.
    HttpStatus(u16),
    /// More than three redirects.
    RedirectLimit,
    /// Connection, TLS or body read failure.
    Transport,
}

impl DownloadError {
    pub(super) const fn reason(&self) -> &'static str {
        match self {
            Self::UrlRefused => "url_refused",
            Self::Timeout => "timeout",
            Self::TooLarge => "too_large",
            Self::HttpStatus(_) => "http_status",
            Self::RedirectLimit => "redirect_limit",
            Self::Transport => "transport",
        }
    }

    /// The HTTP status of a `http_status` failure.
    pub(super) const fn status(&self) -> Option<u16> {
        match self {
            Self::HttpStatus(status) => Some(*status),
            _ => None,
        }
    }
}

/// Downloads `url` and returns its bytes: at most `size` of them (the declared size of the entry),
/// within 60 s in total. `hosts` is the operator's `index_download_hosts`.
pub(super) async fn download(url: &str, size: u64, hosts: &[String]) -> Result<Vec<u8>, DownloadError> {
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    tokio::time::timeout(TOTAL_TIMEOUT, run(url, size, &ExactHosts(hosts.to_vec()), deadline))
        .await
        .unwrap_or(Err(DownloadError::Timeout))
}

async fn run(url: &str, size: u64, hosts: &ExactHosts, deadline: Instant) -> Result<Vec<u8>, DownloadError> {
    // The first hop follows the index's own URL rules; a redirect target is checked by `hop_refused`.
    if download_url_problem(url, hosts).is_some() {
        return Err(DownloadError::UrlRefused);
    }
    let mut current = Url::parse(url).map_err(|_| DownloadError::UrlRefused)?;
    let mut redirects = 0_usize;
    loop {
        let target = resolve_outbound_target(current.as_str()).await.map_err(|error| {
            tracing::warn!(%error, "plugin download target refused");
            DownloadError::UrlRefused
        })?;
        if target.url.scheme() != "https" {
            return Err(DownloadError::UrlRefused);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(DownloadError::Timeout);
        }
        let client = delivery_client(target.pin.as_ref(), remaining, CONNECT_TIMEOUT).map_err(|error| {
            tracing::warn!(%error, "plugin download client could not be built");
            DownloadError::Transport
        })?;
        let mut response = client
            .get(target.url)
            .header(USER_AGENT, CLIENT_NAME)
            .header(ACCEPT, "application/octet-stream, */*")
            .send()
            .await
            .map_err(classify)?;
        let status = response.status();
        if is_redirect(status) {
            if redirects >= MAX_REDIRECTS {
                return Err(DownloadError::RedirectLimit);
            }
            redirects += 1;
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(DownloadError::HttpStatus(status.as_u16()))?;
            let next = current.join(location).map_err(|_| DownloadError::UrlRefused)?;
            if hop_refused(&next, hosts) {
                return Err(DownloadError::UrlRefused);
            }
            current = next;
            continue;
        }
        if status != StatusCode::OK {
            return Err(DownloadError::HttpStatus(status.as_u16()));
        }
        if response.content_length().is_some_and(|length| length > size) {
            return Err(DownloadError::TooLarge);
        }
        let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(size.min(1 << 20)).unwrap_or(0));
        while let Some(chunk) = response.chunk().await.map_err(classify)? {
            if (bytes.len() as u64).saturating_add(chunk.len() as u64) > size {
                return Err(DownloadError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(bytes);
    }
}

const fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// The rules of a redirect target: `https`, no credentials, port 443 or none, a DNS host name
/// (no address literal) the host list allows. A query is allowed here (release hosts sign their
/// redirect URLs), unlike the entry's own URL.
fn hop_refused(url: &Url, hosts: &ExactHosts) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.as_str().len() > MAX_HOP_URL_CHARS
    {
        return true;
    }
    let Some(host) = url.host_str() else {
        return true;
    };
    host.starts_with('[') || host.parse::<IpAddr>().is_ok() || !hosts.allows(&host.to_ascii_lowercase())
}

fn classify(error: reqwest::Error) -> DownloadError {
    if error.is_timeout() {
        DownloadError::Timeout
    } else {
        tracing::warn!(error = %error.without_url(), "plugin download failed");
        DownloadError::Transport
    }
}
