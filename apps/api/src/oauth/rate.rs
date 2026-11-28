//! Rate limiting for the authorization server.
//!
//! Two primitives:
//!
//! * a database fixed window, shared by every API process, for the endpoints that face the
//!   internet (`authorize`, `decision`, `token_ip`, `token_grant`, `revoke`, `stream_connect`).
//!   One statement per hit, keyed by the SHA-256 of an address or a grant or user
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
/// Length of the daily window, in seconds.
pub const DAY_WINDOW_SECS: u32 = 86_400;

/// Length of the failed-login window per account, in seconds (15 minutes).
pub const ACCOUNT_FAIL_WINDOW_SECS: u32 = 900;

/// How long a signed-in (account, address) pair stays known after its last sign-in, in seconds
/// (30 days).
pub const KNOWN_PAIR_WINDOW_SECS: u32 = 2_592_000;

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
    /// Failed authentications on the inbound event endpoint, per client address.
    InboundAuthFail,
    /// Events accepted from one inbound source per minute.
    InboundSourceMinute,
    /// Events accepted from one inbound source per day.
    InboundSourceDay,
    /// Self-reports of one runner bot.
    RunnerReport,
    /// Console login attempts, per client address.
    AuthLoginIp,
    /// Console login attempts, per account and client address together.
    AuthLoginPair,
    /// Failed console logins, per account. Successful logins are not counted.
    AuthLoginAccountFail,
    /// An (account, address) pair that has signed in: one row per pair, kept for 30 days from its
    /// last sign-in (each sign-in restarts the window, see [`touch_window`]). Not a limit: a row
    /// means "known", and the account's failure cap does not apply to a known pair.
    AuthLoginKnown,
    /// Failed console logins of known pairs, per account per day. A known pair is exempt from
    /// the account's failure cap, so this is the ceiling of that exemption: over it, known pairs
    /// are limited like unknown ones.
    AuthLoginKnownFail,
    /// Registration requests, per client address.
    AuthRegisterIp,
    /// Session refreshes, per client address.
    AuthRefreshIp,
    /// First-run status reads, per client address.
    AuthSetupStatusIp,
    /// Site operator login attempts, per client address. Counted apart from the console login so
    /// the two surfaces never share a counter; the figures are the console's.
    SiteLoginIp,
    /// Site operator login attempts, per account and client address together.
    SiteLoginPair,
    /// Failed site operator logins, per account. Successful logins are not counted.
    SiteLoginAccountFail,
    /// An (account, address) pair that has signed in to a site: one row per pair, kept for 30 days
    /// from its last site sign-in. Same meaning as [`Self::AuthLoginKnown`], separate rows.
    SiteLoginKnown,
    /// Failed site operator logins of known pairs, per account per day; the ceiling of the known
    /// pairs' exemption from the account cap, as [`Self::AuthLoginKnownFail`] is for the console.
    SiteLoginKnownFail,
}

impl RateBucket {
    pub const ALL: [Self; 23] = [
        Self::Authorize,
        Self::Decision,
        Self::TokenIp,
        Self::TokenGrant,
        Self::Revoke,
        Self::StreamConnect,
        Self::InboundAuthFail,
        Self::InboundSourceMinute,
        Self::InboundSourceDay,
        Self::RunnerReport,
        Self::AuthLoginIp,
        Self::AuthLoginPair,
        Self::AuthLoginAccountFail,
        Self::AuthLoginKnown,
        Self::AuthLoginKnownFail,
        Self::AuthRegisterIp,
        Self::AuthRefreshIp,
        Self::AuthSetupStatusIp,
        Self::SiteLoginIp,
        Self::SiteLoginPair,
        Self::SiteLoginAccountFail,
        Self::SiteLoginKnown,
        Self::SiteLoginKnownFail,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authorize => "authorize",
            Self::Decision => "decision",
            Self::TokenIp => "token_ip",
            Self::TokenGrant => "token_grant",
            Self::Revoke => "revoke",
            Self::StreamConnect => "stream_connect",
            Self::InboundAuthFail => "inbound_auth_fail",
            Self::InboundSourceMinute => "inbound_source_minute",
            Self::InboundSourceDay => "inbound_source_day",
            Self::RunnerReport => "runner_report",
            Self::AuthLoginIp => "auth_login_ip",
            Self::AuthLoginPair => "auth_login_pair",
            Self::AuthLoginAccountFail => "auth_login_account_fail",
            Self::AuthLoginKnown => "auth_login_known",
            Self::AuthLoginKnownFail => "auth_login_known_fail",
            Self::AuthRegisterIp => "auth_register_ip",
            Self::AuthRefreshIp => "auth_refresh_ip",
            Self::AuthSetupStatusIp => "auth_setup_status_ip",
            Self::SiteLoginIp => "site_login_ip",
            Self::SiteLoginPair => "site_login_pair",
            Self::SiteLoginAccountFail => "site_login_account_fail",
            Self::SiteLoginKnown => "site_login_known",
            Self::SiteLoginKnownFail => "site_login_known_fail",
        }
    }

    /// Length of this bucket's window, in seconds.
    pub const fn window_secs(self) -> u32 {
        match self {
            Self::InboundSourceDay | Self::AuthLoginKnownFail | Self::SiteLoginKnownFail => DAY_WINDOW_SECS,
            Self::AuthLoginAccountFail | Self::SiteLoginAccountFail => ACCOUNT_FAIL_WINDOW_SECS,
            Self::AuthLoginKnown | Self::SiteLoginKnown => KNOWN_PAIR_WINDOW_SECS,
            _ => WINDOW_SECS,
        }
    }

    /// Hits allowed per window.
    pub const fn limit(self) -> u32 {
        match self {
            // Console authentication (the `Auth*` buckets) shares the figures of the endpoints
            // that face the internet in the same way. A shared address (an office behind one NAT)
            // needs headroom, one account has one owner so its bucket is tight, and register is
            // the tightest per address because each accepted call costs a full password hash.
            Self::Authorize
            | Self::Revoke
            | Self::InboundAuthFail
            | Self::AuthLoginIp
            | Self::AuthSetupStatusIp
            | Self::SiteLoginIp => 30,
            Self::Decision | Self::StreamConnect | Self::AuthLoginPair | Self::AuthRegisterIp | Self::SiteLoginPair => {
                10
            }
            Self::AuthLoginAccountFail | Self::SiteLoginAccountFail => 50,
            // One hit makes a pair known.
            Self::AuthLoginKnown | Self::SiteLoginKnown => 1,
            Self::AuthLoginKnownFail | Self::SiteLoginKnownFail => 200,
            Self::TokenIp | Self::AuthRefreshIp => 60,
            Self::TokenGrant => 20,
            Self::InboundSourceMinute => 120,
            Self::InboundSourceDay => 20_000,
            Self::RunnerReport => 12,
        }
    }

    /// Whether `[auth] rate_limit_scale` multiplies this bucket: the console and site operator
    /// authentication buckets, except the known-pair marker (a hit there is a flag, not a limit).
    const fn scaled_by_auth_config(self) -> bool {
        matches!(
            self,
            Self::AuthLoginIp
                | Self::AuthLoginPair
                | Self::AuthLoginAccountFail
                | Self::AuthLoginKnownFail
                | Self::AuthRegisterIp
                | Self::AuthRefreshIp
                | Self::AuthSetupStatusIp
                | Self::SiteLoginIp
                | Self::SiteLoginPair
                | Self::SiteLoginAccountFail
                | Self::SiteLoginKnownFail
        )
    }

    /// The limit with `scale` applied (a scale of 0 counts as 1). Saturates instead of overflowing.
    pub const fn limit_scaled(self, scale: u32) -> u32 {
        if self.scaled_by_auth_config() {
            let scale = if scale == 0 { 1 } else { scale };
            self.limit().saturating_mul(scale)
        } else {
            self.limit()
        }
    }

    /// The limit this process enforces: [`Self::limit`] scaled by `[auth] rate_limit_scale`.
    pub fn effective_limit(self) -> u32 {
        self.limit_scaled(crate::config::runtime().auth_rate_limit_scale)
    }
}

/// The identity a window is counted for, stored only as a SHA-256 hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateKey(String);

impl RateKey {
    pub fn from_ip(ip: IpAddr) -> Self {
        Self(hash_secret(&ip.to_canonical().to_string()).as_hex().to_owned())
    }

    /// A key for an identity tried from an address: IPv4 by the address, IPv6 by its /64 prefix
    /// (privacy addresses rotate inside one), an IPv4-mapped IPv6 address as the IPv4 one.
    pub fn from_text_at(text: &str, ip: IpAddr) -> Self {
        Self::from_text(&format!("{text}\n{}", address_prefix(ip)))
    }

    pub fn from_id(id: Uuid) -> Self {
        Self(hash_secret(&id.to_string()).as_hex().to_owned())
    }

    /// A key for a text identity such as a normalized e-mail address. Only the hash is stored.
    pub fn from_text(text: &str) -> Self {
        Self(hash_secret(text).as_hex().to_owned())
    }

    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

/// The address a pair is keyed by: the IPv4 address, or the /64 prefix of an IPv6 address.
fn address_prefix(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            octets[8..].fill(0);
            format!("{}/64", std::net::Ipv6Addr::from(octets))
        }
    }
}

/// Counts a hit for `key` in `bucket` and restarts the bucket's window at now, so the row stays
/// for a full window after the last hit. The count is set to 1.
pub(crate) async fn touch_window<C: ConnectionTrait>(conn: &C, bucket: RateBucket, key: &RateKey) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO oauth_rate_hits AS hit (bucket, key_hash, window_started, hit_count) \
         VALUES ($1, $2, now(), 1) \
         ON CONFLICT (bucket, key_hash) DO UPDATE SET window_started = now(), hit_count = 1",
        vec![bucket.as_str().into(), key.as_hex().into()],
    ))
    .await?;
    Ok(())
}

/// The client address a request is limited under: the right-most untrusted forwarding entry when
/// the peer is a configured trusted proxy, the peer itself otherwise.
pub fn client_address(headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    client_ip(headers, peer, &crate::config::runtime().trusted_proxies)
}

/// The `429` answer of a console route: the standard envelope with a `Retry-After` header.
pub fn too_many_requests(limited: RateLimited) -> axum::response::Response {
    use axum::response::IntoResponse;
    let mut response = (
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        crate::response::ApiResponse::<()>::error(429, "too many requests; wait before trying again"),
    )
        .into_response();
    if let Ok(value) = limited.retry_after_secs.to_string().parse() {
        response.headers_mut().insert(axum::http::header::RETRY_AFTER, value);
    }
    response
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
    enforce_with_limit(db, bucket, key, bucket.effective_limit()).await
}

pub(crate) async fn enforce_with_limit<C: ConnectionTrait>(
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
            f64::from(bucket.window_secs()).into(),
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
                "DELETE FROM oauth_rate_hits WHERE bucket = $1 \
                 AND window_started < now() - make_interval(secs => $2)",
                vec![bucket.as_str().into(), f64::from(bucket.window_secs() + 3_600).into()],
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

/// Whether `key` has already used up `limit` hits in `bucket`'s running window, without counting a
/// hit. Lets a caller refuse before doing work whose failures are what the bucket counts.
pub(crate) async fn is_over_limit<C: ConnectionTrait>(
    conn: &C,
    bucket: RateBucket,
    key: &RateKey,
    limit: u32,
) -> Result<Option<RateLimited>, DbErr> {
    let row = HitRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT hit_count, \
           GREATEST(1, CEIL(EXTRACT(EPOCH FROM (window_started + make_interval(secs => $3) - now()))))::bigint \
             AS retry_after \
         FROM oauth_rate_hits \
         WHERE bucket = $1 AND key_hash = $2 AND window_started > now() - make_interval(secs => $3)",
        vec![
            bucket.as_str().into(),
            key.as_hex().into(),
            f64::from(bucket.window_secs()).into(),
        ],
    ))
    .one(conn)
    .await?;
    Ok(row.and_then(|row| {
        (u32::try_from(row.hit_count).unwrap_or(u32::MAX) >= limit).then(|| RateLimited {
            retry_after_secs: u64::try_from(row.retry_after).unwrap_or(1).max(1),
        })
    }))
}

/// The five buckets of one password-login surface. The console and the site operator login use
/// the same model with separate counters.
#[derive(Clone, Copy, Debug)]
pub struct LoginBuckets {
    /// Attempts per client address.
    pub ip: RateBucket,
    /// Attempts per (account, address) pair.
    pub pair: RateBucket,
    /// Failures per account.
    pub account_fail: RateBucket,
    /// Marker of a pair that has signed in.
    pub known: RateBucket,
    /// Failures of known pairs per account per day.
    pub known_fail: RateBucket,
}

impl LoginBuckets {
    /// `POST /api/v1/auth/login`.
    pub const CONSOLE: Self = Self {
        ip: RateBucket::AuthLoginIp,
        pair: RateBucket::AuthLoginPair,
        account_fail: RateBucket::AuthLoginAccountFail,
        known: RateBucket::AuthLoginKnown,
        known_fail: RateBucket::AuthLoginKnownFail,
    };
    /// `POST /api/site/v1/{slug}/auth/operator/login`.
    pub const SITE: Self = Self {
        ip: RateBucket::SiteLoginIp,
        pair: RateBucket::SiteLoginPair,
        account_fail: RateBucket::SiteLoginAccountFail,
        known: RateBucket::SiteLoginKnown,
        known_fail: RateBucket::SiteLoginKnownFail,
    };
}

/// The keys one login attempt is counted under.
#[derive(Clone, Debug)]
pub struct LoginKeys {
    /// The client address.
    pub ip: RateKey,
    /// The account tried from the address.
    pub pair: RateKey,
    /// The account alone (the normalized e-mail).
    pub account: RateKey,
}

/// Counts a login attempt and decides whether it may go on. Returns whether the (account,
/// address) pair is known.
///
/// An attempt from one address at one account is limited tightly; the account as a whole is
/// limited only by its failures, with a higher ceiling and never by successes, so a stranger who
/// knows an e-mail cannot lock the owner out from the owner's own address. A known pair is not
/// stopped by the account cap, until the account's daily ceiling of known-pair failures is used up.
pub async fn login_admit(db: &DatabaseConnection, buckets: LoginBuckets, keys: &LoginKeys) -> Result<bool, RateError> {
    enforce(db, buckets.ip, &keys.ip).await?;
    enforce(db, buckets.pair, &keys.pair).await?;
    let mut known = is_over_limit(db, buckets.known, &keys.pair, 1).await?.is_some();
    if known {
        known = is_over_limit(
            db,
            buckets.known_fail,
            &keys.account,
            buckets.known_fail.effective_limit(),
        )
        .await?
        .is_none();
    }
    if let Some(limited) = is_over_limit(
        db,
        buckets.account_fail,
        &keys.account,
        buckets.account_fail.effective_limit(),
    )
    .await?
        && !known
    {
        return Err(RateError::Limited(limited));
    }
    Ok(known)
}

/// Counts a failed credential check. A known pair is never refused by the account's cap, even
/// while its failures count; they count against the known pairs' daily ceiling instead.
pub async fn login_failed(
    db: &DatabaseConnection,
    buckets: LoginBuckets,
    keys: &LoginKeys,
    known: bool,
) -> Result<(), RateError> {
    match enforce(db, buckets.account_fail, &keys.account).await {
        Err(RateError::Limited(limited)) if !known => return Err(RateError::Limited(limited)),
        Err(RateError::Store(error)) => return Err(RateError::Store(error)),
        Ok(()) | Err(RateError::Limited(_)) => {}
    }
    if known {
        enforce(db, buckets.known_fail, &keys.account).await?;
    }
    Ok(())
}

/// Records that the pair signed in. Signing in does not depend on remembering the pair, so a
/// store error is logged and dropped.
pub async fn login_succeeded(db: &DatabaseConnection, buckets: LoginBuckets, keys: &LoginKeys) {
    if let Err(error) = touch_window(db, buckets.known, &keys.pair).await {
        tracing::warn!(bucket = buckets.known.as_str(), error = %error, "auth known-pair record failed");
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
            (RateBucket::InboundAuthFail, "inbound_auth_fail", 30),
            (RateBucket::InboundSourceMinute, "inbound_source_minute", 120),
            (RateBucket::InboundSourceDay, "inbound_source_day", 20_000),
            (RateBucket::RunnerReport, "runner_report", 12),
            (RateBucket::AuthLoginIp, "auth_login_ip", 30),
            (RateBucket::AuthLoginPair, "auth_login_pair", 10),
            (RateBucket::AuthLoginAccountFail, "auth_login_account_fail", 50),
            (RateBucket::AuthLoginKnown, "auth_login_known", 1),
            (RateBucket::AuthLoginKnownFail, "auth_login_known_fail", 200),
            (RateBucket::AuthRegisterIp, "auth_register_ip", 10),
            (RateBucket::AuthRefreshIp, "auth_refresh_ip", 60),
            (RateBucket::AuthSetupStatusIp, "auth_setup_status_ip", 30),
            (RateBucket::SiteLoginIp, "site_login_ip", 30),
            (RateBucket::SiteLoginPair, "site_login_pair", 10),
            (RateBucket::SiteLoginAccountFail, "site_login_account_fail", 50),
            (RateBucket::SiteLoginKnown, "site_login_known", 1),
            (RateBucket::SiteLoginKnownFail, "site_login_known_fail", 200),
        ];
        assert_eq!(table.len(), RateBucket::ALL.len());
        for (bucket, name, limit) in table {
            assert_eq!((bucket.as_str(), bucket.limit()), (name, limit));
        }
    }

    #[test]
    fn the_auth_scale_multiplies_only_the_console_buckets() {
        let scaled = [
            (RateBucket::AuthLoginIp, 30),
            (RateBucket::AuthLoginPair, 10),
            (RateBucket::AuthLoginAccountFail, 50),
            (RateBucket::AuthLoginKnownFail, 200),
            (RateBucket::AuthRegisterIp, 10),
            (RateBucket::AuthRefreshIp, 60),
            (RateBucket::AuthSetupStatusIp, 30),
            (RateBucket::SiteLoginIp, 30),
            (RateBucket::SiteLoginPair, 10),
            (RateBucket::SiteLoginAccountFail, 50),
            (RateBucket::SiteLoginKnownFail, 200),
        ];
        for (bucket, base) in scaled {
            assert_eq!(bucket.limit_scaled(1), base, "{bucket:?}");
            assert_eq!(bucket.limit_scaled(0), base, "{bucket:?} treats 0 as 1");
            assert_eq!(bucket.limit_scaled(1000), base * 1000, "{bucket:?}");
            assert_eq!(bucket.limit_scaled(u32::MAX), u32::MAX, "{bucket:?} saturates");
        }
        // The known-pair marker and every non-console bucket keep their limit.
        for bucket in RateBucket::ALL {
            if !scaled.iter().any(|(scaled_bucket, _)| *scaled_bucket == bucket) {
                assert_eq!(bucket.limit_scaled(1000), bucket.limit(), "{bucket:?}");
            }
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
    fn pair_keys_use_ipv4_whole_and_ipv6_by_the_64_prefix() {
        let at = |ip: &str| RateKey::from_text_at("a@example.com", ip.parse().unwrap());
        assert_eq!(at("192.0.2.10"), at("192.0.2.10"));
        assert_ne!(at("192.0.2.10"), at("192.0.2.11"));
        assert_eq!(at("2001:db8:1:2::1"), at("2001:db8:1:2:aaaa:bbbb:cccc:dddd"));
        assert_ne!(at("2001:db8:1:2::1"), at("2001:db8:1:3::1"));
        assert_eq!(at("192.0.2.10"), at("::ffff:192.0.2.10"));
        assert_ne!(
            at("2001:db8:1:2::1"),
            RateKey::from_text_at("b@example.com", "2001:db8:1:2::1".parse().unwrap())
        );
    }

    #[test]
    fn text_keys_are_hashes_and_differ_per_identity() {
        let key = RateKey::from_text("alice@example.com");
        assert_eq!(key.as_hex().len(), 64);
        assert!(!key.as_hex().contains("alice"));
        assert_eq!(key, RateKey::from_text("alice@example.com"));
        assert_ne!(key, RateKey::from_text("bob@example.com"));
    }

    #[test]
    fn the_console_answer_is_429_with_retry_after() {
        let response = too_many_requests(RateLimited { retry_after_secs: 23 });
        assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[axum::http::header::RETRY_AFTER], "23");
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
