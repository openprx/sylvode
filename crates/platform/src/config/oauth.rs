//! `[oauth]` — the authorization server for the API and the MCP server.
//!
//! The section is optional. Without `issuer` the whole feature is off: the metadata documents do
//! not exist, every endpoint answers 404 and no delegated token is accepted. The validated form,
//! [`OAuthConfig`], therefore only exists for a deployment that configured an issuer; every other
//! deployment carries `None` and cannot reach a half configured server.

use serde::Deserialize;

use super::raw::{Issues, is_placeholder, split_host_port, validate_port};
use super::secret::Secret;

/// Seconds an access token lives unless `[oauth]` overrides it.
pub const DEFAULT_ACCESS_TOKEN_TTL_SECONDS: i64 = 600;
/// Days a refresh token may stay unused unless `[oauth]` overrides it.
pub const DEFAULT_REFRESH_TOKEN_IDLE_DAYS: i64 = 30;
/// Days a grant lasts at most unless `[oauth]` overrides it.
pub const DEFAULT_GRANT_MAX_LIFETIME_DAYS: i64 = 365;
/// Seconds an authorization code lives unless `[oauth]` overrides it.
pub const DEFAULT_AUTHORIZATION_CODE_TTL_SECONDS: i64 = 60;
/// Active grants one user may hold for one client unless `[oauth]` overrides it.
pub const DEFAULT_MAX_ACTIVE_GRANTS_PER_USER_CLIENT: u32 = 10;
/// Days feed rows are retained unless `[oauth]` overrides it.
pub const DEFAULT_EVENT_FEED_RETENTION_DAYS: u32 = 7;
/// Events a resuming stream may be behind unless `[oauth]` overrides it.
pub const DEFAULT_EVENT_BACKFILL_MAX_EVENTS: u32 = 1_000;
/// Hours a resuming stream may be behind unless `[oauth]` overrides it.
pub const DEFAULT_EVENT_BACKFILL_MAX_AGE_HOURS: u32 = 72;
/// Concurrent event streams per API process unless `[oauth]` overrides it.
pub const DEFAULT_MAX_STREAMS_PER_PROCESS: u32 = 2_000;
/// Shortest accepted forwarding secret, in bytes (`oauth.internal_forward_secret` and
/// `mcp.oauth_forward_secret`).
pub const MIN_FORWARD_SECRET_LEN: usize = 32;

/// The validated `[oauth]` section of a deployment that enabled the authorization server.
///
/// `Debug` is safe: the forwarding secret is a [`Secret`].
#[derive(Clone, Debug)]
pub struct OAuthConfig {
    /// The issuer, byte for byte as it appears in the metadata and in every `iss` parameter.
    pub issuer: String,
    /// Identifier of the API as a protected resource. Defaults to `<issuer>/api`.
    pub api_resource: String,
    /// Identifier of the MCP server as a protected resource, when it serves delegated callers.
    pub mcp_resource: Option<String>,
    /// Credential the MCP server presents when it forwards a delegated token to the API.
    pub internal_forward_secret: Option<Secret>,
    pub allow_insecure_loopback_issuer: bool,
    pub access_token_ttl_seconds: i64,
    pub refresh_token_idle_days: i64,
    pub grant_max_lifetime_days: i64,
    pub authorization_code_ttl_seconds: i64,
    pub max_active_grants_per_user_client: u32,
    pub event_feed_retention_days: u32,
    pub event_backfill_max_events: u32,
    pub event_backfill_max_age_hours: u32,
    pub max_streams_per_process: u32,
}

impl OAuthConfig {
    /// A configuration with every default and the given issuer, without the checks the loader
    /// applies to a file. For callers that build a value in memory, such as tests.
    pub fn with_defaults(issuer: impl Into<String>) -> Self {
        let issuer = issuer.into();
        Self {
            api_resource: format!("{issuer}/api"),
            issuer,
            mcp_resource: None,
            internal_forward_secret: None,
            allow_insecure_loopback_issuer: false,
            access_token_ttl_seconds: DEFAULT_ACCESS_TOKEN_TTL_SECONDS,
            refresh_token_idle_days: DEFAULT_REFRESH_TOKEN_IDLE_DAYS,
            grant_max_lifetime_days: DEFAULT_GRANT_MAX_LIFETIME_DAYS,
            authorization_code_ttl_seconds: DEFAULT_AUTHORIZATION_CODE_TTL_SECONDS,
            max_active_grants_per_user_client: DEFAULT_MAX_ACTIVE_GRANTS_PER_USER_CLIENT,
            event_feed_retention_days: DEFAULT_EVENT_FEED_RETENTION_DAYS,
            event_backfill_max_events: DEFAULT_EVENT_BACKFILL_MAX_EVENTS,
            event_backfill_max_age_hours: DEFAULT_EVENT_BACKFILL_MAX_AGE_HOURS,
            max_streams_per_process: DEFAULT_MAX_STREAMS_PER_PROCESS,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawOAuth {
    issuer: Option<String>,
    api_resource: Option<String>,
    mcp_resource: Option<String>,
    internal_forward_secret: Option<String>,
    allow_insecure_loopback_issuer: Option<bool>,
    access_token_ttl_seconds: Option<i64>,
    refresh_token_idle_days: Option<i64>,
    grant_max_lifetime_days: Option<i64>,
    authorization_code_ttl_seconds: Option<i64>,
    max_active_grants_per_user_client: Option<u32>,
    event_feed_retention_days: Option<u32>,
    event_backfill_max_events: Option<u32>,
    event_backfill_max_age_hours: Option<u32>,
    max_streams_per_process: Option<u32>,
}

/// What a URL value is meant to be, which decides which parts it may carry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UrlRole {
    /// `scheme://authority` and nothing else.
    Issuer,
    /// An absolute URL with an optional path and query, but no fragment.
    Resource,
    /// `scheme://host[:port]`, an `Origin` value.
    Origin,
}

struct UrlParts<'a> {
    https: bool,
    host: &'a str,
    port: Option<&'a str>,
}

/// Splits an absolute `http` or `https` URL, refusing userinfo, whitespace and (for every role)
/// a fragment. Hand rolled for the same reason as `validate_http_url`: one shallow check does not
/// justify a URL parser dependency in this crate.
fn split_url<'a>(field: &str, value: &'a str, role: UrlRole) -> Result<UrlParts<'a>, String> {
    if is_placeholder(value) {
        return Err(format!("{field} must be a concrete URL, not a placeholder"));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{field} must not contain whitespace"));
    }
    let (https, rest) = if let Some(rest) = value.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = value.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(format!("{field} must be an absolute http or https URL"));
    };
    if rest.contains('#') {
        return Err(format!("{field} must not contain a fragment"));
    }
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.is_empty() {
        return Err(format!("{field} names no host"));
    }
    if authority.contains('@') {
        return Err(format!("{field} must not contain credentials"));
    }
    let (host, port) = match split_host_port(authority) {
        Some((host, port)) => {
            validate_port(field, port)?;
            (host, Some(port))
        }
        None => (authority, None),
    };
    if host.is_empty() {
        return Err(format!("{field} names no host"));
    }
    if role != UrlRole::Resource && !tail.is_empty() {
        return Err(format!(
            "{field} must be scheme://host[:port] without a path, query or fragment"
        ));
    }
    Ok(UrlParts { https, host, port })
}

/// Whether `host` is a loopback IP literal (`127.0.0.0/8` or `[::1]`). `localhost` is not one:
/// OAuth 2.1 recommends the literal, and a name can be re-pointed.
fn is_loopback_literal(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Checks one URL value. `http` is accepted only for a loopback literal and only when
/// `allow_insecure` says the deployment opted into it.
pub(super) fn validate_url(field: &str, value: &str, role: UrlRole, allow_insecure: bool) -> Result<String, String> {
    let trimmed = value.trim();
    let parts = split_url(field, trimmed, role)?;
    if !parts.https {
        if !is_loopback_literal(parts.host) {
            return Err(format!(
                "{field} must use https, plain http is only allowed for 127.0.0.1 or [::1]"
            ));
        }
        if !allow_insecure {
            return Err(format!(
                "{field} uses plain http, which needs oauth.allow_insecure_loopback_issuer = true (local development only)"
            ));
        }
        if role == UrlRole::Issuer && parts.port.is_none() {
            return Err(format!("{field} must name a port when it is a loopback http issuer"));
        }
    }
    Ok(if role == UrlRole::Origin {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_string()
    })
}

/// Checks a forwarding secret: concrete and long enough to resist guessing.
pub(super) fn validate_forward_secret(field: &str, value: &str) -> Result<Secret, String> {
    if is_placeholder(value) {
        return Err(format!("{field} must be a concrete secret, not a placeholder"));
    }
    if value.len() < MIN_FORWARD_SECRET_LEN {
        return Err(format!("{field} must be at least {MIN_FORWARD_SECRET_LEN} bytes"));
    }
    Ok(Secret::new(value))
}

fn bounded<T: PartialOrd + std::fmt::Display + Copy>(
    issues: &mut Issues,
    field: &str,
    value: Option<T>,
    default: T,
    min: T,
    max: T,
) -> T {
    match value {
        None => default,
        Some(value) if value >= min && value <= max => value,
        Some(_) => {
            issues.push(format!("oauth.{field} must be between {min} and {max}"));
            default
        }
    }
}

impl RawOAuth {
    pub(super) fn validate(self, issues: &mut Issues) -> Option<OAuthConfig> {
        let allow_insecure = self.allow_insecure_loopback_issuer.unwrap_or(false);
        let Some(issuer_raw) = self.issuer else {
            if self.api_resource.is_some()
                || self.mcp_resource.is_some()
                || self.internal_forward_secret.is_some()
                || allow_insecure
            {
                issues.push(
                    "oauth.issuer is required when oauth.api_resource, oauth.mcp_resource, \
                     oauth.internal_forward_secret or oauth.allow_insecure_loopback_issuer is set",
                );
            }
            return None;
        };

        let issuer_url = issues.record(validate_url(
            "oauth.issuer",
            &issuer_raw,
            UrlRole::Issuer,
            allow_insecure,
        ));
        if allow_insecure && !issuer_url.as_deref().is_none_or(is_loopback_issuer) {
            issues.push(
                "oauth.allow_insecure_loopback_issuer = true requires oauth.issuer to name 127.0.0.1 or [::1]; \
                 remove the flag so production issuers stay https",
            );
        }
        let api_resource = self.api_resource.map_or_else(
            || issuer_url.as_ref().map(|issuer| format!("{issuer}/api")),
            |value| {
                issues.record(validate_url(
                    "oauth.api_resource",
                    &value,
                    UrlRole::Resource,
                    allow_insecure,
                ))
            },
        );
        let mcp_resource = self.mcp_resource.and_then(|value| {
            issues.record(validate_url(
                "oauth.mcp_resource",
                &value,
                UrlRole::Resource,
                allow_insecure,
            ))
        });
        let internal_forward_secret = self
            .internal_forward_secret
            .and_then(|value| issues.record(validate_forward_secret("oauth.internal_forward_secret", &value)));
        if mcp_resource.is_some() && internal_forward_secret.is_none() {
            issues.push("oauth.internal_forward_secret is required when oauth.mcp_resource is set");
        }
        if mcp_resource.is_none() && internal_forward_secret.is_some() {
            issues.push("oauth.mcp_resource is required when oauth.internal_forward_secret is set");
        }

        let access_token_ttl_seconds = bounded(
            issues,
            "access_token_ttl_seconds",
            self.access_token_ttl_seconds,
            DEFAULT_ACCESS_TOKEN_TTL_SECONDS,
            60,
            3_600,
        );
        let refresh_token_idle_days = bounded(
            issues,
            "refresh_token_idle_days",
            self.refresh_token_idle_days,
            DEFAULT_REFRESH_TOKEN_IDLE_DAYS,
            1,
            90,
        );
        let grant_max_lifetime_days = bounded(
            issues,
            "grant_max_lifetime_days",
            self.grant_max_lifetime_days,
            DEFAULT_GRANT_MAX_LIFETIME_DAYS,
            1,
            730,
        );
        let authorization_code_ttl_seconds = bounded(
            issues,
            "authorization_code_ttl_seconds",
            self.authorization_code_ttl_seconds,
            DEFAULT_AUTHORIZATION_CODE_TTL_SECONDS,
            10,
            600,
        );
        let max_active_grants_per_user_client = bounded(
            issues,
            "max_active_grants_per_user_client",
            self.max_active_grants_per_user_client,
            DEFAULT_MAX_ACTIVE_GRANTS_PER_USER_CLIENT,
            1,
            50,
        );
        let event_feed_retention_days = bounded(
            issues,
            "event_feed_retention_days",
            self.event_feed_retention_days,
            DEFAULT_EVENT_FEED_RETENTION_DAYS,
            1,
            30,
        );
        let event_backfill_max_events = bounded(
            issues,
            "event_backfill_max_events",
            self.event_backfill_max_events,
            DEFAULT_EVENT_BACKFILL_MAX_EVENTS,
            100,
            10_000,
        );
        let event_backfill_max_age_hours = bounded(
            issues,
            "event_backfill_max_age_hours",
            self.event_backfill_max_age_hours,
            DEFAULT_EVENT_BACKFILL_MAX_AGE_HOURS,
            1,
            168,
        );
        let max_streams_per_process = bounded(
            issues,
            "max_streams_per_process",
            self.max_streams_per_process,
            DEFAULT_MAX_STREAMS_PER_PROCESS,
            10,
            20_000,
        );
        Some(OAuthConfig {
            issuer: issuer_url?,
            api_resource: api_resource?,
            mcp_resource,
            internal_forward_secret,
            allow_insecure_loopback_issuer: allow_insecure,
            access_token_ttl_seconds,
            refresh_token_idle_days,
            grant_max_lifetime_days,
            authorization_code_ttl_seconds,
            max_active_grants_per_user_client,
            event_feed_retention_days,
            event_backfill_max_events,
            event_backfill_max_age_hours,
            max_streams_per_process,
        })
    }
}

/// Whether an already validated issuer names a loopback literal.
fn is_loopback_issuer(issuer: &str) -> bool {
    split_url("oauth.issuer", issuer, UrlRole::Issuer).is_ok_and(|parts| is_loopback_literal(parts.host))
}
