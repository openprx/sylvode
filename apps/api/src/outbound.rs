use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Truncates diagnostics by Unicode scalar count so persistence limits never split UTF-8.
pub fn truncate_string(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value
    } else {
        value.chars().take(max_chars).collect()
    }
}

fn outbound_allowlist() -> String {
    crate::config::runtime().outbound.allowlist_csv()
}

fn outbound_private_targets_allowed() -> bool {
    crate::config::runtime().outbound.allow_private
}

/// Matches a host, optionally including its port, against a comma-separated allowlist.
pub fn host_is_allowlisted(host: &str, port: Option<u16>, allowlist: &str) -> bool {
    let host = normalize_host(host);
    allowlist
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let entry = entry.to_ascii_lowercase();
            match entry.rsplit_once(':') {
                Some((entry_host, entry_port))
                    if !entry_port.is_empty() && entry_port.chars().all(|character| character.is_ascii_digit()) =>
                {
                    normalize_host(entry_host) == host && port.is_some_and(|value| value.to_string() == entry_port)
                }
                _ => normalize_host(&entry) == host,
            }
        })
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// Rejects addresses that must never be reachable from a user-supplied endpoint.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => is_blocked_ipv4(ipv4),
        IpAddr::V6(ipv6) => is_blocked_ipv6(ipv6),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 0
        || ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || (first == 100 && (64..128).contains(&second))
        || (first == 192 && second == 0)
        || (first == 198 && (18..20).contains(&second))
        || first >= 240
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return true;
    }
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(mapped);
    }
    let segments = ip.segments();
    let [first, second, ..] = segments;
    if first == 0x0064 && second == 0xff9b {
        return if segments[2..6].iter().all(|segment| *segment == 0) {
            is_blocked_ipv4(embedded_ipv4(segments[6], segments[7]))
        } else {
            true
        };
    }
    if first == 0x2002 {
        return is_blocked_ipv4(embedded_ipv4(segments[1], segments[2]));
    }
    if segments[..6].iter().all(|segment| *segment == 0) {
        return is_blocked_ipv4(embedded_ipv4(segments[6], segments[7]));
    }
    (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xffc0) == 0xfec0
        || first == 0x2001 && second == 0x0db8
}

fn embedded_ipv4(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::from((u32::from(high) << 16) | u32::from(low))
}

fn literal_host_ip(host: &str) -> Option<IpAddr> {
    normalize_host(host).parse::<IpAddr>().ok()
}

/// Performs scheme, credential and literal-address checks without DNS resolution.
pub fn parse_outbound_url(raw: &str) -> Result<reqwest::Url, String> {
    parse_outbound_url_with(raw, &outbound_allowlist(), outbound_private_targets_allowed())
}

/// [`parse_outbound_url`] against an explicit allowlist (comma-separated hosts, each optionally
/// with a port) and private-address switch instead of the installed configuration.
pub fn parse_outbound_url_with(raw: &str, allowlist: &str, allow_private: bool) -> Result<reqwest::Url, String> {
    parse_outbound_url_using(raw, allowlist, allow_private, is_blocked_ip)
}

/// [`parse_outbound_url_with`] with the blocked-address rule given.
fn parse_outbound_url_using(
    raw: &str,
    allowlist: &str,
    allow_private: bool,
    blocked: BlockedFn,
) -> Result<reqwest::Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("endpoint must not be empty".to_string());
    }
    let url = reqwest::Url::parse(trimmed).map_err(|error| format!("endpoint is not a valid absolute URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "endpoint scheme {} is not allowed, use http or https",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("endpoint must not embed credentials".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "endpoint must contain a host".to_string())?
        .to_string();
    if allow_private || host_is_allowlisted(&host, url.port_or_known_default(), allowlist) {
        return Ok(url);
    }
    if literal_host_ip(&host).is_some_and(blocked) {
        return Err(format!("endpoint host {host} points at a blocked address"));
    }
    Ok(url)
}

/// Performs full outbound target validation, including DNS resolution.
pub async fn validate_outbound_url(raw: &str) -> Result<reqwest::Url, String> {
    validate_outbound_url_with(raw, &outbound_allowlist(), outbound_private_targets_allowed()).await
}

/// [`validate_outbound_url`] against an explicit allowlist and private-address switch.
pub async fn validate_outbound_url_with(
    raw: &str,
    allowlist: &str,
    allow_private: bool,
) -> Result<reqwest::Url, String> {
    resolve_outbound_target_with(raw, allowlist, allow_private)
        .await
        .map(|target| target.url)
}

/// A validated outbound URL, with the addresses its check saw for a resolved host name.
///
/// A caller that connects must connect to exactly these (see
/// `reqwest::ClientBuilder::resolve_to_addrs`): resolving the name again at connect time would let
/// a name that answered a public address to the check answer a private one to the connection.
#[derive(Debug, Clone)]
pub struct OutboundTarget {
    pub url: reqwest::Url,
    /// The host name and the checked addresses, or `None` when no resolution was needed (an
    /// allow-listed host, a literal address, or private targets switched on).
    pub pin: Option<(String, Vec<std::net::SocketAddr>)>,
}

/// [`validate_outbound_url`], returning the checked addresses as well.
pub async fn resolve_outbound_target(raw: &str) -> Result<OutboundTarget, String> {
    resolve_outbound_target_with(raw, &outbound_allowlist(), outbound_private_targets_allowed()).await
}

/// [`resolve_outbound_target`] against an explicit allowlist and private-address switch.
pub async fn resolve_outbound_target_with(
    raw: &str,
    allowlist: &str,
    allow_private: bool,
) -> Result<OutboundTarget, String> {
    let url = parse_outbound_url_with(raw, allowlist, allow_private)?;
    if allow_private {
        return Ok(OutboundTarget { url, pin: None });
    }
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default();
    if host_is_allowlisted(&host, port, allowlist) || literal_host_ip(&host).is_some() {
        return Ok(OutboundTarget { url, pin: None });
    }
    let addresses = tokio::net::lookup_host((host.as_str(), port.unwrap_or(443)))
        .await
        .map_err(|error| format!("endpoint host {host} could not be resolved: {error}"))?
        .collect::<Vec<_>>();
    let addresses = checked_addresses(&host, addresses)?;
    Ok(OutboundTarget {
        url,
        pin: Some((host, addresses)),
    })
}

/// Why [`resolve_outbound_host`] gave no address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundHostError {
    /// The address policy forbids the target. Retrying cannot help until the policy changes.
    Refused,
    /// The name did not resolve. A later attempt may succeed.
    Unresolved,
}

impl std::fmt::Display for OutboundHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Refused => "refused by the outbound address policy",
            Self::Unresolved => "the host name could not be resolved",
        })
    }
}

/// The checked addresses of a `host` and `port` that is not a URL (an SMTP relay), against an
/// explicit allowlist (comma-separated hosts, each optionally with a port) and private-address
/// switch.
///
/// Applies the rules of [`validate_outbound_url_with`]: with `allow_private` or an allow listed
/// host the addresses are used as they resolve; otherwise a literal or resolved address that is
/// private, loopback or otherwise blocked refuses the whole target. The caller must connect to
/// exactly these addresses.
pub async fn resolve_outbound_host(
    host: &str,
    port: u16,
    allowlist: &str,
    allow_private: bool,
) -> Result<Vec<std::net::SocketAddr>, OutboundHostError> {
    let exempt = allow_private || host_is_allowlisted(host, Some(port), allowlist);
    let addresses = if let Some(ip) = literal_host_ip(host) {
        vec![std::net::SocketAddr::new(ip, port)]
    } else {
        tokio::net::lookup_host((normalize_host(host).as_str(), port))
            .await
            .map_err(|_| OutboundHostError::Unresolved)?
            .collect()
    };
    if addresses.is_empty() {
        return Err(OutboundHostError::Unresolved);
    }
    if !exempt && addresses.iter().any(|address| is_blocked_ip(address.ip())) {
        return Err(OutboundHostError::Refused);
    }
    Ok(addresses)
}

/// The client for one delivery: no redirects, the given timeouts, and, for a host name that was
/// resolved and checked, connecting only to the addresses the check saw (see [`OutboundTarget`]).
pub fn delivery_client(
    pin: Option<&(String, Vec<std::net::SocketAddr>)>,
    timeout: std::time::Duration,
    connect_timeout: std::time::Duration,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .redirect(reqwest::redirect::Policy::none());
    if let Some((host, addresses)) = pin {
        builder = builder.resolve_to_addrs(host, addresses);
    }
    builder.build()
}

/// The resolved `addresses` of `host` when none is blocked.
fn checked_addresses(host: &str, addresses: Vec<std::net::SocketAddr>) -> Result<Vec<std::net::SocketAddr>, String> {
    if addresses.is_empty() {
        return Err(format!("endpoint host {host} could not be resolved"));
    }
    if addresses.iter().any(|address| is_blocked_ip(address.ip())) {
        return Err(format!("endpoint host {host} resolves to a blocked address"));
    }
    Ok(addresses)
}

/// The rule that decides whether an address is refused. [`is_blocked_ip`] in production; a test
/// swaps in a rule that lets its own loopback server through.
pub type BlockedFn = fn(IpAddr) -> bool;

/// The outbound address policy: which targets are exempt from the address check, and which
/// addresses the check refuses.
#[derive(Clone, Debug)]
pub struct AddressPolicy {
    /// Comma-separated hosts, each optionally with a port, that are exempt.
    pub allowlist: String,
    /// Turns the address check off for every target.
    pub allow_private: bool,
    /// The refusal rule. Defaults to [`is_blocked_ip`].
    pub blocked: BlockedFn,
}

impl AddressPolicy {
    pub fn new(allowlist: impl Into<String>, allow_private: bool) -> Self {
        Self {
            allowlist: allowlist.into(),
            allow_private,
            blocked: is_blocked_ip,
        }
    }

    /// The policy of the installed configuration (`[outbound]`).
    pub fn from_runtime() -> Self {
        Self::new(outbound_allowlist(), outbound_private_targets_allowed())
    }

    /// Whether the policy exempts `host` and `port` from the address check: the private switch is
    /// on, or the authority is allow-listed (a list entry with a port matches that port only).
    pub fn is_exempt(&self, host: &str, port: Option<u16>) -> bool {
        self.allow_private || host_is_allowlisted(host, port, &self.allowlist)
    }

    /// A hash of the allowlist (as a set of lower-case entries) and of the private switch. It does
    /// not leave the process and is not stable across Rust versions.
    pub fn fingerprint(&self) -> u64 {
        let entries: BTreeSet<String> = self
            .allowlist
            .split(',')
            .map(|entry| entry.trim().to_ascii_lowercase())
            .filter(|entry| !entry.is_empty())
            .collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for entry in &entries {
            entry.hash(&mut hasher);
        }
        entries.len().hash(&mut hasher);
        self.allow_private.hash(&mut hasher);
        hasher.finish()
    }
}

/// A DNS lookup. Production uses the system resolver; a test scripts the answers.
pub trait Lookup: Send + Sync {
    fn lookup<'a>(&'a self, host: &'a str) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>>;
}

/// [`Lookup`] through the system resolver (`tokio::net::lookup_host`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLookup;

impl Lookup for SystemLookup {
    fn lookup<'a>(&'a self, host: &'a str) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>> {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host, 0)).await?;
            Ok(addresses.map(|address| address.ip()).collect())
        })
    }
}

/// Why [`PolicyResolver`] gave no address. The texts carry no address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveRefusal {
    /// The lookup failed.
    Failed(String),
    /// The lookup answered no address.
    Empty,
    /// At least one address of the answer is blocked.
    Blocked,
}

impl std::fmt::Display for ResolveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(error) => write!(f, "the host name could not be resolved: {error}"),
            Self::Empty => f.write_str("the host name resolved to no address"),
            Self::Blocked => f.write_str("the host name resolves to a blocked address"),
        }
    }
}

impl std::error::Error for ResolveRefusal {}

/// Counters of a [`DeliveryPool`]. They carry no host, address, URL or body.
#[derive(Debug, Default)]
struct PoolCounters {
    requests: AtomicU64,
    resolver_lookups: AtomicU64,
    resolver_refusals: AtomicU64,
    connections_opened: AtomicU64,
    permits_waiting: AtomicUsize,
}

/// Counts every connection either pool client sets out to open, before the address is known to be
/// reachable: the connector is asked once per new connection, never for a reused one.
#[derive(Clone)]
struct CountConnections(Arc<PoolCounters>);

impl<S> tower::Layer<S> for CountConnections {
    type Service = CountedConnector<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CountedConnector {
            inner,
            counters: Arc::clone(&self.0),
        }
    }
}

#[derive(Clone)]
struct CountedConnector<S> {
    inner: S,
    counters: Arc<PoolCounters>,
}

impl<S, R> tower::Service<R> for CountedConnector<S>
where
    S: tower::Service<R>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: R) -> Self::Future {
        self.counters.connections_opened.fetch_add(1, Ordering::Relaxed);
        self.inner.call(request)
    }
}

/// A point-in-time copy of a pool's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Requests sent.
    pub requests: u64,
    /// Name lookups the checked client made: one per new connection to a host name.
    pub resolver_lookups: u64,
    /// Lookups the policy refused (a blocked address, an empty or a failed answer).
    pub resolver_refusals: u64,
    /// Connections either client set out to open, to a host name or a literal address.
    pub connections_opened: u64,
    /// Targets with a concurrency limit entry.
    pub targets: usize,
    /// Requests waiting for a permit of their target.
    pub permits_waiting: usize,
}

/// A `reqwest` DNS resolver that checks every answer against the address policy.
///
/// hyper asks it for every new connection to a host name (never for an IP literal) and connects
/// only to what it returns, so the addresses that were checked are the addresses connected to.
/// The whole answer is refused when any address in it is blocked, the rule of
/// [`resolve_outbound_target_with`]. Nothing is cached.
pub struct PolicyResolver {
    lookup: Arc<dyn Lookup>,
    blocked: BlockedFn,
    counters: Arc<PoolCounters>,
}

impl PolicyResolver {
    fn new(lookup: Arc<dyn Lookup>, blocked: BlockedFn, counters: Arc<PoolCounters>) -> Self {
        Self {
            lookup,
            blocked,
            counters,
        }
    }

    /// Looks `host` up and returns its addresses (port 0) when the answer is non-empty and
    /// none of it is blocked.
    pub async fn resolve_checked(&self, host: &str) -> Result<Vec<SocketAddr>, ResolveRefusal> {
        let outcome = self.resolve_inner(host).await;
        if outcome.is_err() {
            self.counters.resolver_refusals.fetch_add(1, Ordering::Relaxed);
        }
        outcome
    }

    async fn resolve_inner(&self, host: &str) -> Result<Vec<SocketAddr>, ResolveRefusal> {
        let addresses = if let Some(ip) = literal_host_ip(host) {
            // hyper does not ask for a literal, but a literal must get the same answer if asked.
            vec![ip]
        } else {
            self.counters.resolver_lookups.fetch_add(1, Ordering::Relaxed);
            self.lookup
                .lookup(host)
                .await
                .map_err(|error| ResolveRefusal::Failed(error.kind().to_string()))?
        };
        if addresses.is_empty() {
            return Err(ResolveRefusal::Empty);
        }
        if addresses.iter().any(|address| (self.blocked)(*address)) {
            return Err(ResolveRefusal::Blocked);
        }
        Ok(addresses
            .into_iter()
            .map(|address| SocketAddr::new(address, 0))
            .collect())
    }
}

impl reqwest::dns::Resolve for PolicyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let lookup = Arc::clone(&self.lookup);
        let resolver = Self::new(lookup, self.blocked, Arc::clone(&self.counters));
        Box::pin(async move {
            let addresses = resolver
                .resolve_checked(&host)
                .await
                .map_err(|refusal| Box::new(refusal) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// How a [`DeliveryPool`] builds its clients and limits its targets.
#[derive(Debug, Clone, Copy)]
pub struct PoolSettings {
    /// Whole-request timeout.
    pub timeout: Duration,
    /// Connection timeout.
    pub connect_timeout: Duration,
    /// An idle connection is closed after this long.
    pub idle_timeout: Duration,
    /// Idle connections kept per authority.
    pub max_idle_per_host: usize,
    /// TCP keepalive interval, `None` for none.
    pub tcp_keepalive: Option<Duration>,
    /// Requests in flight per `(scheme, host, port)`.
    pub per_target_concurrency: usize,
}

/// The client a target is sent with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientClass {
    /// Every new connection is resolved and checked by the [`PolicyResolver`].
    Checked,
    /// The policy exempts the target (`allow_private`, or an allow-listed `host:port`): the
    /// system resolver, no address check, as before the pool.
    Exempt,
}

#[derive(Debug)]
struct Clients {
    checked: reqwest::Client,
    exempt: reqwest::Client,
}

#[derive(Debug)]
struct Active {
    policy: AddressPolicy,
    fingerprint: u64,
    clients: Clients,
}

type TargetKey = (String, String, u16);

struct TargetEntry {
    semaphore: Arc<Semaphore>,
    last_used: Instant,
}

/// Why [`DeliveryPool::send`] sent nothing or got no answer.
#[derive(Debug)]
pub enum PoolError {
    /// The URL failed the scheme, credential or literal-address checks.
    Rejected(String),
    /// The address policy refused the target while a connection was being opened: the name
    /// resolved to a blocked address, to none, or did not resolve. The text carries no address.
    Refused(ResolveRefusal),
    /// The request failed; the error has its URL removed.
    Transport(reqwest::Error),
}

/// The [`ResolveRefusal`] in an error's source chain, if the address policy caused the error.
fn refusal_in(error: &(dyn std::error::Error + 'static)) -> Option<ResolveRefusal> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(refusal) = error.downcast_ref::<ResolveRefusal>() {
            return Some(refusal.clone());
        }
        current = error.source();
    }
    None
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(reason) => f.write_str(reason),
            Self::Refused(refusal) => write!(f, "the outbound address policy refused the target: {refusal}"),
            Self::Transport(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PoolError {}

/// A response, holding the permit of its target until it is dropped.
#[derive(Debug)]
pub struct PooledResponse {
    pub response: reqwest::Response,
    pub permit: OwnedSemaphorePermit,
}

/// Long-lived delivery clients that keep connections alive and still check every address.
///
/// Two clients share one configuration (no redirects, the given timeouts, idle and keepalive
/// limits, HTTP/1 only, the workspace's TLS with no relaxed certificate check): `checked` resolves
/// through the [`PolicyResolver`] and serves every target the policy does not exempt; `exempt`
/// uses the system resolver and serves the exempt ones. Every byte goes over a connection whose
/// address passed the rule when that connection opened. A per-target semaphore limits requests in
/// flight to one `(scheme, host, port)`.
///
/// `checked` ignores the proxy environment variables (`no_proxy()`): a proxy would resolve the
/// target itself and the address rule would never see the name. `exempt` serves only targets the
/// policy lists, and keeps honouring them (see `the_checked_client_ignores_the_proxy_environment`).
pub struct DeliveryPool {
    settings: PoolSettings,
    lookup: Arc<dyn Lookup>,
    counters: Arc<PoolCounters>,
    active: RwLock<Arc<Active>>,
    targets: Mutex<HashMap<TargetKey, TargetEntry>>,
}

impl DeliveryPool {
    /// A pool with the system resolver.
    pub fn new(policy: AddressPolicy, settings: PoolSettings) -> Result<Self, reqwest::Error> {
        Self::with_lookup(policy, settings, Arc::new(SystemLookup))
    }

    /// A pool with the given lookup (a test's scripted resolver).
    pub fn with_lookup(
        policy: AddressPolicy,
        settings: PoolSettings,
        lookup: Arc<dyn Lookup>,
    ) -> Result<Self, reqwest::Error> {
        let counters = Arc::new(PoolCounters::default());
        let active = Self::build(policy, &settings, &lookup, &counters)?;
        Ok(Self {
            settings,
            lookup,
            counters,
            active: RwLock::new(Arc::new(active)),
            targets: Mutex::new(HashMap::new()),
        })
    }

    fn build(
        policy: AddressPolicy,
        settings: &PoolSettings,
        lookup: &Arc<dyn Lookup>,
        counters: &Arc<PoolCounters>,
    ) -> Result<Active, reqwest::Error> {
        let base = || {
            reqwest::Client::builder()
                .connector_layer(CountConnections(Arc::clone(counters)))
                .timeout(settings.timeout)
                .connect_timeout(settings.connect_timeout)
                .redirect(reqwest::redirect::Policy::none())
                .pool_idle_timeout(Some(settings.idle_timeout))
                .pool_max_idle_per_host(settings.max_idle_per_host)
                .tcp_keepalive(settings.tcp_keepalive)
                .http1_only()
        };
        let resolver = PolicyResolver::new(Arc::clone(lookup), policy.blocked, Arc::clone(counters));
        let checked = base().dns_resolver(Arc::new(resolver)).no_proxy().build()?;
        let exempt = base().build()?;
        Ok(Active {
            fingerprint: policy.fingerprint(),
            policy,
            clients: Clients { checked, exempt },
        })
    }

    fn active(&self) -> Arc<Active> {
        Arc::clone(&self.active.read())
    }

    /// The fingerprint of the policy the clients were built with.
    pub fn policy_fingerprint(&self) -> u64 {
        self.active().fingerprint
    }

    /// Rebuilds both clients when `policy` differs from the one they were built with; returns
    /// whether it did. Connections of the old clients close as their requests end.
    pub fn for_policy(&self, policy: AddressPolicy) -> Result<bool, reqwest::Error> {
        if policy.fingerprint() == self.policy_fingerprint() {
            return Ok(false);
        }
        let active = Self::build(policy, &self.settings, &self.lookup, &self.counters)?;
        *self.active.write() = Arc::new(active);
        Ok(true)
    }

    /// Rebuilds both clients with the current policy, dropping every pooled connection.
    pub fn reset(&self) -> Result<(), reqwest::Error> {
        let policy = self.active().policy.clone();
        let active = Self::build(policy, &self.settings, &self.lookup, &self.counters)?;
        *self.active.write() = Arc::new(active);
        Ok(())
    }

    /// Which client serves `url`, from its `(host, port)`.
    pub fn class_of(&self, url: &reqwest::Url) -> ClientClass {
        Self::class_in(&self.active().policy, url)
    }

    fn class_in(policy: &AddressPolicy, url: &reqwest::Url) -> ClientClass {
        let host = url.host_str().unwrap_or_default();
        if policy.is_exempt(host, url.port_or_known_default()) {
            ClientClass::Exempt
        } else {
            ClientClass::Checked
        }
    }

    /// Validates `raw_url`, waits for a permit of its target, and sends the request `build` makes
    /// with the client that serves it. The permit is held by the returned response.
    pub async fn send(
        &self,
        raw_url: &str,
        build: impl FnOnce(&reqwest::Client, reqwest::Url) -> reqwest::RequestBuilder,
    ) -> Result<PooledResponse, PoolError> {
        let active = self.active();
        let url = parse_outbound_url_using(
            raw_url,
            &active.policy.allowlist,
            active.policy.allow_private,
            active.policy.blocked,
        )
        .map_err(PoolError::Rejected)?;
        let client = match Self::class_in(&active.policy, &url) {
            ClientClass::Checked => &active.clients.checked,
            ClientClass::Exempt => &active.clients.exempt,
        };
        let permit = self.acquire(&url).await?;
        self.counters.requests.fetch_add(1, Ordering::Relaxed);
        let response = build(client, url).send().await.map_err(|error| {
            refusal_in(&error).map_or_else(|| PoolError::Transport(error.without_url()), PoolError::Refused)
        })?;
        Ok(PooledResponse { response, permit })
    }

    async fn acquire(&self, url: &reqwest::Url) -> Result<OwnedSemaphorePermit, PoolError> {
        let key: TargetKey = (
            url.scheme().to_string(),
            normalize_host(url.host_str().unwrap_or_default()),
            url.port_or_known_default().unwrap_or_default(),
        );
        let semaphore = {
            let mut targets = self.targets.lock();
            let now = Instant::now();
            let idle = self.settings.idle_timeout;
            let limit = self.settings.per_target_concurrency.max(1);
            targets.retain(|other, entry| {
                *other == key
                    || now.duration_since(entry.last_used) <= idle
                    || entry.semaphore.available_permits() < limit
            });
            let entry = targets.entry(key).or_insert_with(|| TargetEntry {
                semaphore: Arc::new(Semaphore::new(limit)),
                last_used: now,
            });
            entry.last_used = now;
            Arc::clone(&entry.semaphore)
        };
        self.counters.permits_waiting.fetch_add(1, Ordering::Relaxed);
        let permit = semaphore.acquire_owned().await;
        self.counters.permits_waiting.fetch_sub(1, Ordering::Relaxed);
        // Nothing closes a target's semaphore; the error arm only keeps the type total.
        permit.map_err(|_| PoolError::Rejected("the target's concurrency limit is closed".to_string()))
    }

    /// The counters, for the worker's periodic pool line.
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            requests: self.counters.requests.load(Ordering::Relaxed),
            resolver_lookups: self.counters.resolver_lookups.load(Ordering::Relaxed),
            resolver_refusals: self.counters.resolver_refusals.load(Ordering::Relaxed),
            connections_opened: self.counters.connections_opened.load(Ordering::Relaxed),
            targets: self.targets.lock().len(),
            permits_waiting: self.counters.permits_waiting.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{checked_addresses, delivery_client, host_is_allowlisted, is_blocked_ip, truncate_string};
    use std::time::Duration;

    /// A pinned client connects to the checked address and never asks DNS: `.invalid` never
    /// resolves, so a successful request proves the pin was used.
    #[tokio::test]
    async fn a_pinned_client_connects_to_the_checked_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0_u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\npinned")
                    .await;
            }
        });
        let url = format!("http://pinned.invalid:{}/", addr.port());
        let pin = ("pinned.invalid".to_string(), vec![addr]);
        let client = delivery_client(Some(&pin), Duration::from_secs(5), Duration::from_secs(2)).expect("client");
        let response = client.get(&url).send().await.expect("the pinned address answers");
        assert_eq!(response.text().await.expect("text"), "pinned");
        server.abort();
        let unpinned = delivery_client(None, Duration::from_secs(5), Duration::from_secs(2)).expect("client");
        assert!(unpinned.get(&url).send().await.is_err());
    }

    #[test]
    fn checked_addresses_refuse_a_name_with_any_blocked_address() {
        let public: std::net::SocketAddr = "93.184.216.34:443".parse().expect("addr");
        let private: std::net::SocketAddr = "10.1.2.3:443".parse().expect("addr");
        assert_eq!(checked_addresses("h", vec![public]).expect("public"), vec![public]);
        assert!(checked_addresses("h", vec![public, private]).is_err());
        assert!(checked_addresses("h", Vec::new()).is_err());
    }

    #[tokio::test]
    async fn a_host_target_follows_the_address_rules() {
        use super::{OutboundHostError, resolve_outbound_host};
        let refused = resolve_outbound_host("127.0.0.1", 25, "", false).await;
        assert_eq!(refused, Err(OutboundHostError::Refused));
        let private = resolve_outbound_host("169.254.169.254", 587, "", false).await;
        assert_eq!(private, Err(OutboundHostError::Refused));
        let public = resolve_outbound_host("93.184.216.34", 587, "", false).await;
        assert_eq!(public.map(|found| found.len()), Ok(1));
        // An allow listed host, with or without its port, and the private switch lift the refusal.
        assert!(
            resolve_outbound_host("127.0.0.1", 2525, "127.0.0.1:2525", false)
                .await
                .is_ok()
        );
        assert_eq!(
            resolve_outbound_host("127.0.0.1", 25, "127.0.0.1:2525", false).await,
            Err(OutboundHostError::Refused)
        );
        assert!(resolve_outbound_host("127.0.0.1", 25, "", true).await.is_ok());
        assert_eq!(
            resolve_outbound_host("name.invalid", 25, "", false).await,
            Err(OutboundHostError::Unresolved)
        );
    }

    #[test]
    fn diagnostics_truncate_on_character_boundaries() {
        assert_eq!(truncate_string("abcdef".to_string(), 3), "abc");
        assert_eq!(truncate_string("好好好".to_string(), 2), "好好");
    }

    #[test]
    fn private_and_transition_addresses_are_blocked() {
        for raw in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            let address = raw.parse().ok();
            assert!(address.is_some_and(is_blocked_ip), "{raw} must be blocked");
        }
    }

    #[test]
    fn host_allowlist_matches_exact_host_and_optional_port() {
        assert!(host_is_allowlisted("API", Some(8080), " api , webhook "));
        assert!(host_is_allowlisted("api", Some(8080), "api:8080"));
        assert!(!host_is_allowlisted("api", Some(9090), "api:8080"));
        assert!(!host_is_allowlisted("evil.example.com", Some(443), "api,webhook"));
    }
}

/// The delivery pool and its resolver, against a counting server on 127.0.0.1 (ephemeral port)
/// and a scripted DNS lookup.
#[cfg(test)]
mod pool_tests {
    use super::{
        AddressPolicy, BlockedFn, ClientClass, DeliveryPool, Lookup, PolicyResolver, PoolCounters, PoolError,
        PoolSettings, ResolveRefusal, is_blocked_ip,
    };
    use std::net::IpAddr;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The production rule, except that loopback is let through so a local server can answer.
    fn loopback_allowed(ip: IpAddr) -> bool {
        !ip.is_loopback() && is_blocked_ip(ip)
    }

    /// Answers each lookup with the next scripted answer; the last one repeats.
    struct Scripted {
        answers: Vec<std::io::Result<Vec<IpAddr>>>,
        calls: AtomicUsize,
    }

    impl Scripted {
        fn new(answers: Vec<std::io::Result<Vec<IpAddr>>>) -> Arc<Self> {
            Arc::new(Self {
                answers,
                calls: AtomicUsize::new(0),
            })
        }

        fn always(addresses: &[&str]) -> Arc<Self> {
            Self::new(vec![Ok(ips(addresses))])
        }
    }

    impl Lookup for Scripted {
        fn lookup<'a>(
            &'a self,
            _host: &'a str,
        ) -> Pin<Box<dyn std::future::Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>> {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            let answer = match self.answers.get(index).or_else(|| self.answers.last()) {
                Some(Ok(addresses)) => Ok(addresses.clone()),
                Some(Err(error)) => Err(std::io::Error::new(error.kind(), "scripted failure")),
                None => Ok(Vec::new()),
            };
            Box::pin(async move { answer })
        }
    }

    fn ips(addresses: &[&str]) -> Vec<IpAddr> {
        addresses.iter().map(|raw| raw.parse().expect("address")).collect()
    }

    fn settings(per_target: usize, idle: Duration) -> PoolSettings {
        PoolSettings {
            timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(2),
            idle_timeout: idle,
            max_idle_per_host: 4,
            tcp_keepalive: None,
            per_target_concurrency: per_target,
        }
    }

    fn test_policy(allowlist: &str) -> AddressPolicy {
        let mut policy = AddressPolicy::new(allowlist, false);
        policy.blocked = loopback_allowed as BlockedFn;
        policy
    }

    fn resolver(answers: Arc<Scripted>, blocked: BlockedFn) -> PolicyResolver {
        PolicyResolver::new(answers, blocked, Arc::new(PoolCounters::default()))
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Ok,
        Redirect(u16),
        /// Holds each request for 150 ms and tracks the requests in flight.
        Slow,
    }

    struct Server {
        port: u16,
        accepted: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
    }

    /// Answers requests on one connection until it closes.
    async fn handle(
        mut socket: tokio::net::TcpStream,
        mode: Mode,
        in_flight: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
    ) {
        let mut seen: Vec<u8> = Vec::new();
        loop {
            if !read_head(&mut socket, &mut seen).await {
                return;
            }
            seen.clear();
            let reply = match mode {
                Mode::Ok => "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string(),
                Mode::Redirect(target) => {
                    format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target}/\r\nContent-Length: 0\r\n\r\n")
                }
                Mode::Slow => {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()
                }
            };
            if socket.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
        }
    }

    /// Reads one request head into `seen`; false when the connection closed first.
    async fn read_head(socket: &mut tokio::net::TcpStream, seen: &mut Vec<u8>) -> bool {
        let mut chunk = [0_u8; 1024];
        while !seen.windows(4).any(|window| window == b"\r\n\r\n") {
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(read) => seen.extend_from_slice(chunk.get(..read).unwrap_or_default()),
            }
        }
        true
    }

    async fn serve(mode: Mode) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let (accepted_in, max_in) = (accepted.clone(), max_in_flight.clone());
        let in_flight = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                accepted_in.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(handle(socket, mode, in_flight.clone(), max_in.clone()));
            }
        });
        Server {
            port,
            accepted,
            max_in_flight,
        }
    }

    async fn get(pool: &DeliveryPool, url: &str) -> Result<u16, PoolError> {
        let super::PooledResponse { response, permit } = pool.send(url, reqwest::Client::get).await?;
        let status = response.status().as_u16();
        let _ = response.text().await;
        drop(permit);
        Ok(status)
    }

    #[tokio::test]
    async fn the_resolver_refuses_a_name_with_any_blocked_address() {
        let public = "93.184.216.34";
        for blocked in [
            "10.1.2.3",
            "127.0.0.1",
            "169.254.169.254",
            "192.168.0.9",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::10.0.0.1",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "2002:7f00:1::1",
        ] {
            let lookup = Scripted::always(&[public, blocked]);
            let outcome = resolver(lookup, is_blocked_ip).resolve_checked("runner.test").await;
            assert_eq!(
                outcome,
                Err(ResolveRefusal::Blocked),
                "{blocked} must refuse the whole answer"
            );
            let alone = resolver(Scripted::always(&[blocked]), is_blocked_ip)
                .resolve_checked("runner.test")
                .await;
            assert_eq!(alone, Err(ResolveRefusal::Blocked), "{blocked} alone must be refused");
        }
        let allowed = resolver(Scripted::always(&[public, "2606:4700::1111"]), is_blocked_ip)
            .resolve_checked("runner.test")
            .await
            .expect("a public answer passes");
        assert_eq!(allowed.len(), 2);
        let empty = resolver(Scripted::new(vec![Ok(Vec::new())]), is_blocked_ip)
            .resolve_checked("runner.test")
            .await;
        assert_eq!(empty, Err(ResolveRefusal::Empty));
        let failed = resolver(Scripted::new(vec![Err(std::io::Error::other("down"))]), is_blocked_ip)
            .resolve_checked("runner.test")
            .await;
        assert!(matches!(failed, Err(ResolveRefusal::Failed(_))));
    }

    #[test]
    fn v4_mapped_and_compatible_v6_forms_follow_the_v4_rule() {
        for raw in [
            "::ffff:127.0.0.1",
            "::ffff:7f00:1",
            "::127.0.0.1",
            "::10.0.0.1",
            "::ffff:192.168.1.1",
        ] {
            assert!(is_blocked_ip(raw.parse().expect("address")), "{raw} must be blocked");
        }
        for raw in ["::ffff:93.184.216.34", "2606:4700::1111"] {
            assert!(!is_blocked_ip(raw.parse().expect("address")), "{raw} must pass");
        }
    }

    #[tokio::test]
    async fn a_blocked_literal_is_refused_before_any_connection() {
        let server = serve(Mode::Ok).await;
        let pool = DeliveryPool::new(AddressPolicy::new("", false), settings(1, Duration::from_secs(5))).expect("pool");
        for url in [
            format!("http://127.0.0.1:{}/", server.port),
            format!("http://[::ffff:127.0.0.1]:{}/", server.port),
            format!("http://[::1]:{}/", server.port),
            "http://169.254.169.254/latest".to_string(),
            "http://10.0.0.1/".to_string(),
            "ftp://example.test/".to_string(),
            "http://user:pass@example.test/".to_string(),
        ] {
            assert!(matches!(get(&pool, &url).await, Err(PoolError::Rejected(_))), "{url}");
        }
        assert_eq!(server.accepted.load(Ordering::SeqCst), 0);
        assert_eq!(pool.stats().requests, 0);
    }

    #[tokio::test]
    async fn a_rebound_name_is_refused_on_the_next_connect_and_nothing_connects() {
        let server = serve(Mode::Ok).await;
        // Public (here: the test server's loopback) on the first lookup, private on the second.
        let lookup = Scripted::new(vec![Ok(ips(&["127.0.0.1"])), Ok(ips(&["10.0.0.9"]))]);
        let pool =
            DeliveryPool::with_lookup(test_policy(""), settings(1, Duration::from_millis(50)), lookup).expect("pool");
        let url = format!("http://runner.test:{}/", server.port);
        assert_eq!(get(&pool, &url).await.expect("first delivery"), 200);
        assert_eq!(server.accepted.load(Ordering::SeqCst), 1);
        // The idle connection expires, so the next request must connect, and resolve, again.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let refused = get(&pool, &url).await.expect_err("the rebound name is refused");
        assert!(
            matches!(refused, PoolError::Refused(ResolveRefusal::Blocked)),
            "the reason survives the transport error: {refused}"
        );
        assert!(refused.to_string().contains("blocked address"), "{refused}");
        assert_eq!(
            server.accepted.load(Ordering::SeqCst),
            1,
            "no connection to the private answer"
        );
        let stats = pool.stats();
        assert_eq!((stats.resolver_lookups, stats.resolver_refusals), (2, 1));
    }

    #[test]
    fn the_allow_list_is_keyed_by_host_and_port() {
        let pool = DeliveryPool::new(
            AddressPolicy::new("runner.test:8080, open.test", false),
            settings(1, Duration::from_secs(5)),
        )
        .expect("pool");
        let class = |raw: &str| pool.class_of(&reqwest::Url::parse(raw).expect("url"));
        assert_eq!(class("http://runner.test:8080/x"), ClientClass::Exempt);
        assert_eq!(class("http://RUNNER.test:8080/x"), ClientClass::Exempt);
        assert_eq!(class("http://runner.test:8081/x"), ClientClass::Checked);
        assert_eq!(class("http://runner.test/x"), ClientClass::Checked);
        assert_eq!(class("http://open.test:9999/x"), ClientClass::Exempt);
        assert_eq!(class("http://other.test:8080/x"), ClientClass::Checked);
        let private =
            DeliveryPool::new(AddressPolicy::new("", true), settings(1, Duration::from_secs(5))).expect("pool");
        assert_eq!(
            private.class_of(&reqwest::Url::parse("http://anything.test/").expect("url")),
            ClientClass::Exempt
        );
    }

    #[tokio::test]
    async fn an_allow_listed_literal_connects_and_the_same_literal_without_the_entry_does_not() {
        let server = serve(Mode::Ok).await;
        let url = format!("http://127.0.0.1:{}/", server.port);
        let listed = DeliveryPool::new(
            AddressPolicy::new(format!("127.0.0.1:{}", server.port), false),
            settings(1, Duration::from_secs(5)),
        )
        .expect("pool");
        assert_eq!(get(&listed, &url).await.expect("exempt delivery"), 200);
        let other_port = DeliveryPool::new(
            AddressPolicy::new(format!("127.0.0.1:{}", server.port.wrapping_add(1)), false),
            settings(1, Duration::from_secs(5)),
        )
        .expect("pool");
        assert!(matches!(get(&other_port, &url).await, Err(PoolError::Rejected(_))));
        let open = DeliveryPool::new(AddressPolicy::new("", true), settings(1, Duration::from_secs(5))).expect("pool");
        assert_eq!(get(&open, &url).await.expect("allow_private delivery"), 200);
    }

    #[tokio::test]
    async fn ten_sequential_requests_open_one_connection_and_one_lookup() {
        let server = serve(Mode::Ok).await;
        let lookup = Scripted::always(&["127.0.0.1"]);
        let pool =
            DeliveryPool::with_lookup(test_policy(""), settings(1, Duration::from_secs(30)), lookup).expect("pool");
        let url = format!("http://runner.test:{}/", server.port);
        for _ in 0..10 {
            assert_eq!(get(&pool, &url).await.expect("delivery"), 200);
        }
        assert_eq!(server.accepted.load(Ordering::SeqCst), 1);
        let stats = pool.stats();
        assert_eq!(
            (
                stats.requests,
                stats.resolver_lookups,
                stats.resolver_refusals,
                stats.connections_opened
            ),
            (10, 1, 0, 1)
        );
    }

    #[tokio::test]
    async fn the_target_limit_caps_requests_in_flight() {
        let server = serve(Mode::Slow).await;
        let lookup = Scripted::always(&["127.0.0.1"]);
        let pool = Arc::new(
            DeliveryPool::with_lookup(test_policy(""), settings(2, Duration::from_secs(30)), lookup).expect("pool"),
        );
        let url = format!("http://runner.test:{}/", server.port);
        let mut tasks = Vec::new();
        for _ in 0..6 {
            let (pool, url) = (pool.clone(), url.clone());
            tasks.push(tokio::spawn(async move { get(&pool, &url).await }));
        }
        for task in tasks {
            assert_eq!(task.await.expect("join").expect("delivery"), 200);
        }
        assert_eq!(server.max_in_flight.load(Ordering::SeqCst), 2);
        assert_eq!(pool.stats().permits_waiting, 0);
    }

    #[tokio::test]
    async fn an_idle_target_entry_is_evicted() {
        let a = serve(Mode::Ok).await;
        let b = serve(Mode::Ok).await;
        let lookup = Scripted::always(&["127.0.0.1"]);
        let pool =
            DeliveryPool::with_lookup(test_policy(""), settings(1, Duration::from_millis(50)), lookup).expect("pool");
        get(&pool, &format!("http://runner.test:{}/", a.port)).await.expect("a");
        assert_eq!(pool.stats().targets, 1);
        tokio::time::sleep(Duration::from_millis(150)).await;
        get(&pool, &format!("http://runner.test:{}/", b.port)).await.expect("b");
        assert_eq!(pool.stats().targets, 1, "the idle entry of the first target is gone");
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let target = serve(Mode::Ok).await;
        let redirecting = serve(Mode::Redirect(target.port)).await;
        let lookup = Scripted::always(&["127.0.0.1"]);
        let pool =
            DeliveryPool::with_lookup(test_policy(""), settings(1, Duration::from_secs(5)), lookup).expect("pool");
        let status = get(&pool, &format!("http://runner.test:{}/", redirecting.port))
            .await
            .expect("delivery");
        assert_eq!(status, 302);
        assert_eq!(target.accepted.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_fingerprint_follows_the_policy_and_for_policy_rebuilds_only_on_a_change() {
        let first = AddressPolicy::new("a.test:1, B.test", false);
        assert_eq!(
            first.fingerprint(),
            AddressPolicy::new("b.test,a.test:1", false).fingerprint()
        );
        assert_ne!(
            first.fingerprint(),
            AddressPolicy::new("a.test:1, B.test", true).fingerprint()
        );
        assert_ne!(first.fingerprint(), AddressPolicy::new("a.test:1", false).fingerprint());
        let pool = DeliveryPool::new(first.clone(), settings(1, Duration::from_secs(5))).expect("pool");
        assert_eq!(pool.policy_fingerprint(), first.fingerprint());
        assert!(
            !pool
                .for_policy(AddressPolicy::new("b.test, a.test:1", false))
                .expect("same")
        );
        let changed = AddressPolicy::new("", false);
        assert!(pool.for_policy(changed.clone()).expect("changed"));
        assert_eq!(pool.policy_fingerprint(), changed.fingerprint());
        pool.reset().expect("reset");
        assert_eq!(pool.policy_fingerprint(), changed.fingerprint());
    }

    const PROXY_CHILD_ENV: &str = "SYLVODE_OUTBOUND_PROXY_TEST_CHILD";

    /// Runs only inside the child process of `the_checked_client_ignores_the_proxy_environment`,
    /// whose environment names a proxy. The scripted lookup answers with the test server's
    /// loopback address, so a delivery that is answered went through the resolver.
    #[tokio::test]
    async fn proxy_environment_child() {
        let Some(port) = std::env::var(PROXY_CHILD_ENV)
            .ok()
            .and_then(|port| port.parse::<u16>().ok())
        else {
            return;
        };
        let lookup = Scripted::always(&["127.0.0.1"]);
        let pool =
            DeliveryPool::with_lookup(test_policy(""), settings(1, Duration::from_secs(5)), lookup).expect("pool");
        let url = format!("http://runner.test:{port}/hook");
        assert_eq!(get(&pool, &url).await.expect("delivered directly"), 200);
        let stats = pool.stats();
        assert_eq!((stats.resolver_lookups, stats.connections_opened), (1, 1));
    }

    /// Design O5: with `HTTP_PROXY` set the checked client still resolves and checks the name
    /// itself and connects to the target; the proxy is never contacted. The environment is set on
    /// a child process, not on this one.
    #[tokio::test]
    async fn the_checked_client_ignores_the_proxy_environment() {
        let server = serve(Mode::Ok).await;
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let proxy_port = proxy.local_addr().expect("addr").port();
        let exe = std::env::current_exe().expect("test binary");
        let target_port = server.port;
        let child = tokio::task::spawn_blocking(move || {
            std::process::Command::new(exe)
                .args([
                    "--exact",
                    "outbound::pool_tests::proxy_environment_child",
                    "--test-threads=1",
                ])
                .env(PROXY_CHILD_ENV, target_port.to_string())
                .env("HTTP_PROXY", format!("http://127.0.0.1:{proxy_port}"))
                .env("http_proxy", format!("http://127.0.0.1:{proxy_port}"))
                .env("ALL_PROXY", format!("http://127.0.0.1:{proxy_port}"))
                .env_remove("NO_PROXY")
                .env_remove("no_proxy")
                .output()
        });
        let output = child.await.expect("join").expect("child ran");
        assert!(
            output.status.success(),
            "the child test passed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(
            server.accepted.load(Ordering::SeqCst),
            1,
            "the target was reached directly"
        );
        let contacted = tokio::time::timeout(Duration::from_millis(200), proxy.accept()).await;
        assert!(contacted.is_err(), "the proxy was never contacted");
    }
}
