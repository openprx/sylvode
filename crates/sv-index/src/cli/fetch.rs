//! Package download with the server's rules (contract M15 §3): https only, a DNS-name host (never an
//! IP literal), port 443, no credentials, an optional host allowlist, at most three redirects with
//! every hop checked again, a public address for every hop, a bounded body (the declared size) and a
//! bounded total time. The API's outbound policy lives in `apps/api` and would drag the API in, so
//! the same rules are implemented here.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::header::LOCATION;
use reqwest::redirect::Policy;
use url::{Host, Url};

use crate::limits;
use crate::{ExactHosts, HostPolicy};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_REDIRECTS: usize = 3;

/// Why a download failed (`details.reason` of the server's `index_download_failed`).
#[derive(Debug)]
pub enum FetchError {
    /// A hop's URL breaks the rules or resolves to a non-public address.
    UrlRefused(String),
    /// The total time ran out.
    Timeout,
    /// The body is larger than the declared size.
    TooLarge,
    /// A final answer other than 2xx.
    HttpStatus(u16),
    /// More than three redirects.
    RedirectLimit,
    /// Connection, TLS or DNS failure.
    Network(String),
}

impl FetchError {
    /// The reason code (`timeout`, `too_large`, `http_status`, `redirect_limit`, `url_refused`, `network`)
    /// followed by the detail.
    pub fn message(&self) -> String {
        match self {
            Self::UrlRefused(why) => format!("url_refused: {why}"),
            Self::Timeout => format!("timeout: no complete answer within {} s", TOTAL_TIMEOUT.as_secs()),
            Self::TooLarge => "too_large: the body is larger than the declared size".to_owned(),
            Self::HttpStatus(code) => format!("http_status: the server answered {code}"),
            Self::RedirectLimit => format!("redirect_limit: more than {MAX_REDIRECTS} redirects"),
            Self::Network(e) => format!("network: {e}"),
        }
    }
}

/// Download settings.
pub struct Fetcher {
    hosts: ExactHosts,
    /// Hosts pinned to addresses (skip the public-address rule).
    pins: Vec<(String, SocketAddr)>,
    roots: Vec<reqwest::Certificate>,
}

impl Fetcher {
    pub const fn new(hosts: Vec<String>, pins: Vec<(String, SocketAddr)>, roots: Vec<reqwest::Certificate>) -> Self {
        Self {
            hosts: ExactHosts(hosts),
            pins,
            roots,
        }
    }

    /// Downloads `start`, at most `declared` bytes.
    pub fn fetch(&self, start: &str, declared: u64) -> Result<Vec<u8>, FetchError> {
        let deadline = Instant::now() + TOTAL_TIMEOUT;
        let mut url = Url::parse(start).map_err(|_| FetchError::UrlRefused("URL does not parse".to_owned()))?;
        let mut redirects = 0usize;
        loop {
            let host = self.check_hop(&url)?;
            let addrs = self.addresses(&host)?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(FetchError::Timeout)?;
            let mut builder = Client::builder()
                .redirect(Policy::none())
                .no_proxy()
                .https_only(true)
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(remaining)
                .user_agent("sv-index")
                .resolve_to_addrs(&host, &addrs);
            for cert in &self.roots {
                builder = builder.add_root_certificate(cert.clone());
            }
            let client = builder.build().map_err(|e| FetchError::Network(e.to_string()))?;
            let resp = client.get(url.clone()).send().map_err(|e| {
                if e.is_timeout() {
                    FetchError::Timeout
                } else {
                    FetchError::Network(chain(&e))
                }
            })?;
            let status = resp.status();
            if status.is_redirection() {
                if redirects >= MAX_REDIRECTS {
                    return Err(FetchError::RedirectLimit);
                }
                let location = resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(FetchError::HttpStatus(status.as_u16()))?;
                url = url
                    .join(location)
                    .map_err(|_| FetchError::UrlRefused("redirect Location does not parse".to_owned()))?;
                redirects += 1;
                continue;
            }
            if !status.is_success() {
                return Err(FetchError::HttpStatus(status.as_u16()));
            }
            let cap = declared.min(limits::MAX_PACKAGE_BYTES);
            if resp.content_length().is_some_and(|n| n > cap) {
                return Err(FetchError::TooLarge);
            }
            return read_body(resp, cap, deadline);
        }
    }

    /// Rules for every hop; returns the host.
    fn check_hop(&self, url: &Url) -> Result<String, FetchError> {
        let refuse = |why: &str| Err(FetchError::UrlRefused(format!("{url}: {why}")));
        if url.scheme() != "https" {
            return refuse("must use https");
        }
        if !url.username().is_empty() || url.password().is_some() {
            return refuse("must not carry credentials");
        }
        if url.port().is_some() {
            return refuse("must use port 443");
        }
        let host = match url.host() {
            Some(Host::Domain(d)) if !d.is_empty() => d.to_owned(),
            Some(Host::Domain(_)) | None => return refuse("has no host"),
            Some(Host::Ipv4(_) | Host::Ipv6(_)) => return refuse("host must be a DNS name, not an IP address"),
        };
        if !self.hosts.allows(&host) {
            return refuse(&format!("host `{host}` is not an allowed download host"));
        }
        Ok(host)
    }

    /// The addresses to connect to: the pinned ones, or the resolved ones, all of them public.
    fn addresses(&self, host: &str) -> Result<Vec<SocketAddr>, FetchError> {
        let pinned: Vec<SocketAddr> = self
            .pins
            .iter()
            .filter(|(h, _)| h.eq_ignore_ascii_case(host))
            .map(|(_, addr)| *addr)
            .collect();
        if !pinned.is_empty() {
            return Ok(pinned);
        }
        let resolved: Vec<SocketAddr> = (host, 443u16)
            .to_socket_addrs()
            .map_err(|e| FetchError::Network(format!("cannot resolve {host}: {e}")))?
            .collect();
        if resolved.is_empty() {
            return Err(FetchError::Network(format!("{host} has no address")));
        }
        if let Some(bad) = resolved.iter().find(|a| !is_public(a.ip())) {
            return Err(FetchError::UrlRefused(format!(
                "{host} resolves to the non-public address {}",
                bad.ip()
            )));
        }
        Ok(resolved)
    }
}

/// An error with its causes: `error sending request: connection refused`.
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(inner) = source {
        out.push_str(": ");
        out.push_str(&inner.to_string());
        source = inner.source();
    }
    out
}

fn read_body(mut resp: reqwest::blocking::Response, cap: u64, deadline: Instant) -> Result<Vec<u8>, FetchError> {
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        if Instant::now() >= deadline {
            return Err(FetchError::Timeout);
        }
        match resp.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => {
                if let Some(chunk) = buf.get(..n) {
                    out.extend_from_slice(chunk);
                }
                if out.len() as u64 > cap {
                    return Err(FetchError::TooLarge);
                }
            }
            Err(e) => {
                let timed_out = e.kind() == std::io::ErrorKind::TimedOut
                    || e.get_ref()
                        .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
                        .is_some_and(reqwest::Error::is_timeout);
                return Err(if timed_out {
                    FetchError::Timeout
                } else {
                    FetchError::Network(e.to_string())
                });
            }
        }
    }
}

/// True for an address a public host can have: not loopback, private, link-local, shared (CGNAT),
/// documentation, benchmarking, multicast, reserved, unspecified or unique-local.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 240)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let seg = ip.segments();
    let first = seg.first().copied().unwrap_or(0);
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xffc0) == 0xfec0
        || (first == 0x2001 && seg.get(1) == Some(&0x0db8))
        || (first == 0x0064 && seg.get(1) == Some(&0xff9b))
        || (first == 0x0100 && seg.get(1) == Some(&0)))
}
