//! Rate limiting for the authorization server.
//!
//! Two primitives:
//!
//! * a database fixed window, shared by every API process, for the endpoints that face the
//!   internet (`authorize`, `decision`, `token_ip`, `token_grant`, `revoke`, `stream_connect`,
//!   `embed_ticket`). One statement per hit, keyed by the SHA-256 of an address or a grant or user
//!   id, so no address is stored in readable form;
//! * an in-process token bucket per grant for delegated API requests (600 per minute, burst 60),
//!   which needs no round trip.
//!
//! Client addresses come from [`client_address`], which is `security::client_ip` with the
//! configured trusted proxies. The raw peer address is never used: behind the bundled reverse
//! proxy it is the proxy for every caller, and a limiter keyed on it would throttle everybody
//! together.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use parking_lot::Mutex;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

use super::error::{OAuthBearerError, OAuthError};
use super::tokens::hash_secret;
use crate::security::client_ip;

/// Length of every database window, in seconds.
pub const WINDOW_SECS: u32 = 60;

/// The database-backed buckets and their limits per window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateBucket {
    /// Authorization requests, per client address.
    Authorize,
    /// Consent decisions, per user (per address for `no_account`).
    Decision,
    /// Token endpoint, per client address.
    TokenIp,
    /// Token endpoint, per grant.
    TokenGrant,
    /// Revocation endpoint, per client address.
    Revoke,
    /// Event stream connections, per grant.
    StreamConnect,
    /// Embed tickets, per grant.
    EmbedTicket,
}

impl RateBucket {
    pub const ALL: [Self; 7] = [
        Self::Authorize,
        Self::Decision,
        Self::TokenIp,
        Self::TokenGrant,
        Self::Revoke,
        Self::StreamConnect,
        Self::EmbedTicket,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authorize => "authorize",
            Self::Decision => "decision",
            Self::TokenIp => "token_ip",
            Self::TokenGrant => "token_grant",
            Self::Revoke => "revoke",
            Self::StreamConnect => "stream_connect",
            Self::EmbedTicket => "embed_ticket",
        }
    }

    /// Hits allowed per window.
    pub const fn limit(self) -> u32 {
        match self {
            Self::Authorize | Self::Revoke => 30,
            Self::Decision | Self::StreamConnect => 10,
            Self::TokenIp | Self::EmbedTicket => 60,
            Self::TokenGrant => 20,
        }
    }
}

/// The identity a window is counted for, stored only as a SHA-256 hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateKey(String);

impl RateKey {
    pub fn from_ip(ip: IpAddr) -> Self {
        Self(hash_secret(&ip.to_canonical().to_string()).as_hex().to_owned())
    }

    pub fn from_id(id: Uuid) -> Self {
        Self(hash_secret(&id.to_string()).as_hex().to_owned())
    }

    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

/// The client address a request is limited under: the right-most untrusted forwarding entry when
/// the peer is a configured trusted proxy, the peer itself otherwise.
pub fn client_address(headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    client_ip(headers, peer, &crate::config::runtime().trusted_proxies)
}

/// A hit over the limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimited {
    pub retry_after_secs: u64,
}

impl From<RateLimited> for OAuthError {
    fn from(limited: RateLimited) -> Self {
        Self::rate_limited(limited.retry_after_secs)
    }
}

impl From<RateLimited> for OAuthBearerError {
    fn from(limited: RateLimited) -> Self {
        Self::rate_limited(limited.retry_after_secs)
    }
}

/// Why a hit was not admitted.
#[derive(Debug, thiserror::Error)]
pub enum RateError {
    #[error("rate limit exceeded")]
    Limited(RateLimited),
    /// The counter could not be read or written. The caller decides; the endpoints that face the
    /// internet refuse the request rather than run unlimited.
    #[error("rate limit store unavailable")]
    Store(#[from] DbErr),
}

#[derive(Debug, FromQueryResult)]
struct HitRow {
    hit_count: i32,
    retry_after: i64,
}

/// The next count in a fixed window: `window_open` says whether the previous window is still
/// running, and `allowed` is true while the count is within `limit`.
pub const fn advance_window(previous_count: Option<u32>, window_open: bool, limit: u32) -> (u32, bool) {
    let count = if window_open {
        match previous_count {
            Some(previous) => previous.saturating_add(1),
            None => 1,
        }
    } else {
        1
    };
    (count, count <= limit)
}

/// Counts one hit for `key` in `bucket`'s current window and admits or refuses it.
pub async fn enforce(db: &DatabaseConnection, bucket: RateBucket, key: &RateKey) -> Result<(), RateError> {
    enforce_with_limit(db, bucket, key, bucket.limit()).await
}

pub(super) async fn enforce_with_limit<C: ConnectionTrait>(
    conn: &C,
    bucket: RateBucket,
    key: &RateKey,
    limit: u32,
) -> Result<(), RateError> {
    // The window arithmetic runs on the database clock, so processes with skewed clocks agree.
    let row = HitRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO oauth_rate_hits AS hit (bucket, key_hash, window_started, hit_count) \
         VALUES ($1, $2, now(), 1) \
         ON CONFLICT (bucket, key_hash) DO UPDATE \
         SET window_started = CASE WHEN hit.window_started <= now() - make_interval(secs => $3) \
                                   THEN now() ELSE hit.window_started END, \
             hit_count = CASE WHEN hit.window_started <= now() - make_interval(secs => $3) \
                              THEN 1 ELSE hit.hit_count + 1 END \
         RETURNING hit_count, \
           GREATEST(1, CEIL(EXTRACT(EPOCH FROM (window_started + make_interval(secs => $3) - now()))))::bigint \
             AS retry_after",
        vec![
            bucket.as_str().into(),
            key.as_hex().into(),
            f64::from(WINDOW_SECS).into(),
        ],
    ))
    .one(conn)
    .await?
    .ok_or(DbErr::RecordNotFound("rate window row".to_owned()))?;

    // About one first hit in sixteen sweeps the bucket's stale rows, so the table stays small
    // without a scan on every request.
    if row.hit_count == 1
        && key.as_hex().starts_with('0')
        && let Err(error) = conn
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM oauth_rate_hits WHERE bucket = $1 AND window_started < now() - interval '1 hour'",
                vec![bucket.as_str().into()],
            ))
            .await
    {
        tracing::warn!(bucket = bucket.as_str(), error = %error, "oauth rate window prune failed");
    }

    let observed = u32::try_from(row.hit_count).unwrap_or(u32::MAX);
    let (_, allowed) = advance_window(Some(observed.saturating_sub(1)), true, limit);
    if allowed {
        Ok(())
    } else {
        Err(RateError::Limited(RateLimited {
            retry_after_secs: u64::try_from(row.retry_after).unwrap_or(1).max(1),
        }))
    }
}

/// Capacity of the per-grant bucket: the burst.
pub const GRANT_BURST: u32 = 60;
/// Sustained rate of the per-grant bucket, per minute.
pub const GRANT_PER_MINUTE: u32 = 600;
const MAX_TRACKED_GRANTS: usize = 10_000;
const IDLE_EVICTION: Duration = Duration::from_mins(2);
const MILLI: u64 = 1_000;

struct Bucket {
    milli_tokens: u64,
    last: Instant,
}

/// In-process token bucket per grant. The clock is a parameter so behaviour is testable.
pub struct GrantRateLimiter {
    burst: u32,
    per_minute: u32,
    buckets: Mutex<HashMap<Uuid, Bucket>>,
}

impl GrantRateLimiter {
    pub fn new(burst: u32, per_minute: u32) -> Self {
        Self {
            burst: burst.max(1),
            per_minute: per_minute.max(1),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// 600 requests per minute with a burst of 60.
    pub fn delegated_default() -> Self {
        Self::new(GRANT_BURST, GRANT_PER_MINUTE)
    }

    /// Takes one token for `grant_id`, or says how many seconds until one is available.
    pub fn check(&self, grant_id: Uuid, now: Instant) -> Result<(), RateLimited> {
        let capacity = u64::from(self.burst) * MILLI;
        let rate = u128::from(self.per_minute);
        let deficit = {
            let mut buckets = self.buckets.lock();
            if !buckets.contains_key(&grant_id) && buckets.len() >= MAX_TRACKED_GRANTS {
                evict(&mut buckets, now);
            }
            let bucket = buckets.entry(grant_id).or_insert(Bucket {
                milli_tokens: capacity,
                last: now,
            });
            let elapsed_us = now.saturating_duration_since(bucket.last).as_micros();
            let refill = u64::try_from(elapsed_us.saturating_mul(rate) / 60_000).unwrap_or(u64::MAX);
            bucket.milli_tokens = bucket.milli_tokens.saturating_add(refill).min(capacity);
            bucket.last = now;
            if bucket.milli_tokens >= MILLI {
                bucket.milli_tokens -= MILLI;
                return Ok(());
            }
            let remaining = MILLI - bucket.milli_tokens;
            drop(buckets);
            u128::from(remaining)
        };
        let wait_us = deficit.saturating_mul(60_000) / rate;
        let retry_after_secs = u64::try_from(wait_us.div_ceil(1_000_000)).unwrap_or(1).max(1);
        Err(RateLimited { retry_after_secs })
    }

    /// Number of grants currently tracked.
    pub fn tracked(&self) -> usize {
        self.buckets.lock().len()
    }
}

/// Makes room: drops buckets idle for two minutes (they would be full again anyway), and if that
/// frees nothing, the least recently used one.
fn evict(buckets: &mut HashMap<Uuid, Bucket>, now: Instant) {
    buckets.retain(|_, bucket| now.saturating_duration_since(bucket.last) < IDLE_EVICTION);
    if buckets.len() >= MAX_TRACKED_GRANTS
        && let Some(oldest) = buckets.iter().min_by_key(|(_, bucket)| bucket.last).map(|(id, _)| *id)
    {
        buckets.remove(&oldest);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn limits_match_the_contract_table() {
        let table = [
            (RateBucket::Authorize, "authorize", 30),
            (RateBucket::Decision, "decision", 10),
            (RateBucket::TokenIp, "token_ip", 60),
            (RateBucket::TokenGrant, "token_grant", 20),
            (RateBucket::Revoke, "revoke", 30),
            (RateBucket::StreamConnect, "stream_connect", 10),
            (RateBucket::EmbedTicket, "embed_ticket", 60),
        ];
        assert_eq!(table.len(), RateBucket::ALL.len());
        for (bucket, name, limit) in table {
            assert_eq!((bucket.as_str(), bucket.limit()), (name, limit));
        }
    }

    #[test]
    fn window_advances_and_resets() {
        assert_eq!(advance_window(None, false, 3), (1, true));
        assert_eq!(advance_window(Some(2), true, 3), (3, true));
        assert_eq!(advance_window(Some(3), true, 3), (4, false));
        assert_eq!(advance_window(Some(99), false, 3), (1, true));
        assert_eq!(advance_window(Some(u32::MAX), true, 3), (u32::MAX, false));
    }

    #[test]
    fn keys_are_hashes_not_addresses() {
        let ip: IpAddr = "192.0.2.10".parse().unwrap();
        let key = RateKey::from_ip(ip);
        assert_eq!(key.as_hex().len(), 64);
        assert!(!key.as_hex().contains("192"));
        assert_eq!(key, RateKey::from_ip(ip));
        assert_ne!(key, RateKey::from_ip("192.0.2.11".parse().unwrap()));
        let mapped: IpAddr = "::ffff:192.0.2.10".parse().unwrap();
        assert_eq!(
            key,
            RateKey::from_ip(mapped),
            "an IPv4-mapped address counts as the IPv4 one"
        );
        let id = Uuid::new_v4();
        assert_eq!(RateKey::from_id(id).as_hex().len(), 64);
        assert_ne!(RateKey::from_id(id), RateKey::from_id(Uuid::new_v4()));
    }

    #[test]
    fn rate_limited_becomes_both_error_kinds_with_retry_after() {
        let limited = RateLimited { retry_after_secs: 17 };
        let oauth: OAuthError = limited.into();
        assert_eq!(oauth.retry_after_secs, Some(17));
        assert_eq!(oauth.kind.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        let bearer: OAuthBearerError = limited.into();
        assert_eq!(bearer.retry_after_secs, Some(17));
    }

    #[test]
    fn burst_then_refusal_then_refill() {
        let limiter = GrantRateLimiter::delegated_default();
        let grant = Uuid::new_v4();
        let start = Instant::now();
        for _ in 0..60 {
            limiter.check(grant, start).expect("the burst is admitted");
        }
        let refused = limiter
            .check(grant, start)
            .expect_err("the 61st request in the same instant is refused");
        assert_eq!(refused.retry_after_secs, 1);
        // 600 per minute is one token every 100 ms.
        assert!(limiter.check(grant, start + Duration::from_millis(99)).is_err());
        limiter
            .check(grant, start + Duration::from_millis(100))
            .expect("one token is back after 100 ms");
        assert!(limiter.check(grant, start + Duration::from_millis(100)).is_err());
        // A long pause refills to the burst and no further.
        let later = start + Duration::from_hours(1);
        for _ in 0..60 {
            limiter.check(grant, later).expect("full burst after a pause");
        }
        assert!(limiter.check(grant, later).is_err());
    }

    #[test]
    fn grants_are_limited_independently() {
        let limiter = GrantRateLimiter::new(2, 60);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let now = Instant::now();
        limiter.check(a, now).unwrap();
        limiter.check(a, now).unwrap();
        assert!(limiter.check(a, now).is_err());
        limiter.check(b, now).expect("another grant has its own bucket");
        let wait = limiter.check(a, now).unwrap_err();
        assert_eq!(wait.retry_after_secs, 1, "60 per minute is one token a second");
        assert_eq!(limiter.tracked(), 2);
    }

    #[test]
    fn the_tracked_set_is_bounded() {
        let limiter = GrantRateLimiter::delegated_default();
        let now = Instant::now();
        for _ in 0..MAX_TRACKED_GRANTS + 5 {
            limiter.check(Uuid::new_v4(), now).unwrap();
        }
        assert!(limiter.tracked() <= MAX_TRACKED_GRANTS);
        // Idle entries are dropped first.
        let later = now + IDLE_EVICTION + Duration::from_secs(1);
        limiter.check(Uuid::new_v4(), later).unwrap();
        assert!(limiter.tracked() < MAX_TRACKED_GRANTS);
    }

    #[test]
    fn the_address_comes_from_the_trusted_proxy_chain_not_the_peer() {
        use axum::http::HeaderValue;
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));
        let proxy: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        assert_eq!(
            client_address(&headers, Some(proxy)),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        // An untrusted peer can not vouch for anything.
        let outsider: SocketAddr = "198.51.100.4:9000".parse().unwrap();
        assert_eq!(
            client_address(&headers, Some(outsider)),
            "198.51.100.4".parse::<IpAddr>().unwrap()
        );
    }
}
