//! Authentication of `POST /api/v1/inbound/events`.
//!
//! One credential type is accepted here and nowhere else: `Authorization: Bearer svi_...`. A
//! session, a bot token or a delegated token is refused exactly like an unknown value, and the
//! source token is refused by every other route because its prefix is none the other layers
//! claim. Order of the checks, each before the more expensive one:
//!
//! 1. the endpoint is enabled, and the credential is not in the query string;
//! 2. the client address has not used up its failed-authentication allowance;
//! 3. the token hashes to a live source whose project exists, and the person the source acts for
//!    is still a workspace owner or admin;
//! 4. the source's per-minute and per-day allowances (counted before the body is read, so a flood
//!    of invalid bodies is capped too);
//! 5. the body is read under the size limit, and the optional HMAC signature is verified.
//!
//! An unknown, disabled or rotated token and a wrong signature answer identically for every
//! caller; only a holder of a valid token learns that a source is suspended.

use std::net::{IpAddr, SocketAddr};

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use hmac::{
    Hmac, Mac,
    digest::{KeyInit, generic_array::GenericArray},
};
use platform::{app::AppState, config::InboundConfig, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{audit, credentials, notify, response::InboundError};
use crate::oauth::rate::{RateBucket, RateError, RateKey, client_address, enforce_with_limit, is_over_limit};

/// Header carrying the unix timestamp a signature covers.
pub const TIMESTAMP_HEADER: &str = "x-sylvode-timestamp";
/// Header carrying `v1=<hex>`.
pub const SIGNATURE_HEADER: &str = "x-sylvode-signature";

/// State of the authentication layer.
#[derive(Clone)]
pub struct InboundAuthState {
    pub app: AppState,
    pub config: InboundConfig,
}

/// The source a request authenticated as. Deliberately without `Debug`: it holds the signing
/// secret.
#[derive(Clone, FromQueryResult)]
pub struct SourceRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub signing_secret: Option<String>,
    pub require_signature: bool,
    pub acting_user_id: Option<Uuid>,
    pub enabled: bool,
    pub rate_per_minute: i32,
    pub daily_cap: i32,
    /// Role of the acting user in the source's workspace, `None` when they are no longer a member
    /// or their account is inactive.
    pub acting_role: Option<String>,
}

impl SourceRow {
    /// Whether the person the source acts for may still stand behind it (D7).
    pub fn acting_user_is_admin(&self) -> bool {
        self.acting_user_id.is_some() && matches!(self.acting_role.as_deref(), Some("owner" | "admin"))
    }
}

/// What the layer hands the handler.
#[derive(Clone)]
pub struct VerifiedRequest {
    pub source: SourceRow,
    pub body: Bytes,
    pub body_sha256: String,
    pub signed: bool,
    pub client: IpAddr,
    pub config: InboundConfig,
}

/// Outcome of checking the signature headers of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureCheck {
    /// Both headers present and correct.
    Valid,
    /// Neither header present.
    Absent,
    /// A header is missing or malformed, the timestamp is outside the window, or the MAC differs.
    Invalid,
}

/// `HMAC-SHA256(secret, "<timestamp>.<body>")` as lower case hex.
pub fn compute_signature(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = signing_mac(secret);
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// An HMAC-SHA256 instance. HMAC accepts a key of any length, so `new_from_slice` cannot fail; the
/// all-zero key fallback only keeps the function total without a panic path.
fn signing_mac(secret: &str) -> Hmac<Sha256> {
    <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| <Hmac<Sha256> as KeyInit>::new(&GenericArray::default()))
}

/// Checks the signature headers of a request against `secret`. The comparison of the two MACs is
/// constant time.
pub fn check_signature(
    secret: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now_unix: i64,
    tolerance_secs: i64,
) -> SignatureCheck {
    let (timestamp, signature) = match (timestamp, signature) {
        (None, None) => return SignatureCheck::Absent,
        (Some(timestamp), Some(signature)) => (timestamp, signature),
        _ => return SignatureCheck::Invalid,
    };
    let Ok(sent_at) = timestamp.trim().parse::<i64>() else {
        return SignatureCheck::Invalid;
    };
    if sent_at.abs_diff(now_unix) > tolerance_secs.unsigned_abs() {
        return SignatureCheck::Invalid;
    }
    let Some(hex_mac) = signature.trim().strip_prefix("v1=") else {
        return SignatureCheck::Invalid;
    };
    let Ok(expected) = hex::decode(hex_mac) else {
        return SignatureCheck::Invalid;
    };
    let mut mac = signing_mac(secret);
    mac.update(timestamp.trim().as_bytes());
    mac.update(b".");
    mac.update(body);
    if mac.verify_slice(&expected).is_ok() {
        SignatureCheck::Valid
    } else {
        SignatureCheck::Invalid
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

fn query_has_token(query: Option<&str>) -> bool {
    query.is_some_and(|query| {
        query.split('&').any(|pair| {
            pair.split('=')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("token"))
        })
    })
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

/// Counts a failed authentication of `client` and answers `refusal`, or 429 once the client has
/// used up its allowance.
async fn count_failure<C: ConnectionTrait>(
    db: &C,
    config: &InboundConfig,
    client: IpAddr,
    refusal: InboundError,
) -> InboundError {
    match enforce_with_limit(
        db,
        RateBucket::InboundAuthFail,
        &RateKey::from_ip(client),
        config.auth_failures_per_minute_per_ip,
    )
    .await
    {
        Ok(()) => refusal,
        Err(RateError::Limited(limited)) => InboundError::rate_limited(limited.retry_after_secs),
        Err(RateError::Store(error)) => {
            tracing::warn!(error = %error, "inbound failed-authentication counter unavailable");
            InboundError::store_unavailable()
        }
    }
}

async fn find_source<C: ConnectionTrait>(db: &C, token: &str) -> Result<Option<SourceRow>, sea_orm::DbErr> {
    SourceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT s.id, s.workspace_id, s.project_id, s.name, s.signing_secret, s.require_signature, \
                s.acting_user_id, s.enabled, s.rate_per_minute, s.daily_cap, \
                (SELECT m.role FROM workspace_members m \
                   JOIN users u ON u.id = m.user_id AND u.is_active \
                  WHERE m.workspace_id = s.workspace_id AND m.user_id = s.acting_user_id) AS acting_role \
           FROM inbound_sources s \
           JOIN projects p ON p.id = s.project_id AND p.workspace_id = s.workspace_id \
          WHERE s.token_hash = $1",
        vec![credentials::hash_token(token).into()],
    ))
    .one(db)
    .await
}

/// Writes the rejection audit row for a known source; a failure is logged and never changes the
/// answer.
async fn audit_rejection<C: ConnectionTrait>(db: &C, source: &SourceRow, reason: &str, client: IpAddr) {
    if let Err(error) =
        audit::record_rejection(db, source.workspace_id, source.project_id, source.id, reason, client).await
    {
        tracing::warn!(error = %error, "inbound rejection audit failed");
    }
}

/// The authentication layer of the inbound route.
pub async fn inbound_source_auth(State(state): State<InboundAuthState>, request: Request, next: Next) -> Response {
    match authenticate(&state, request).await {
        Ok(request) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}

async fn authenticate(state: &InboundAuthState, request: Request) -> Result<Request, InboundError> {
    let config = state.config;
    let db = &state.app.db;
    if !config.enabled {
        return Err(InboundError::disabled());
    }
    if query_has_token(request.uri().query()) {
        return Err(InboundError::new(StatusCode::BAD_REQUEST, "credential_in_query"));
    }
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
    let client = client_address(request.headers(), peer);

    match is_over_limit(
        db,
        RateBucket::InboundAuthFail,
        &RateKey::from_ip(client),
        config.auth_failures_per_minute_per_ip,
    )
    .await
    {
        Ok(Some(limited)) => return Err(InboundError::rate_limited(limited.retry_after_secs)),
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(error = %error, "inbound failed-authentication counter unavailable");
            return Err(InboundError::store_unavailable());
        }
    }

    let Some(token) = bearer_token(request.headers()).filter(|token| credentials::is_token_shape(token)) else {
        return Err(count_failure(db, &config, client, InboundError::invalid_credentials()).await);
    };
    let source = match find_source(db, token).await {
        Ok(Some(source)) if source.enabled => source,
        Ok(_) => {
            // Unknown, rotated or disabled: counted in the log only, since there is no source to
            // attribute it to.
            tracing::info!(client = %client, "inbound credential refused");
            return Err(count_failure(db, &config, client, InboundError::invalid_credentials()).await);
        }
        Err(error) => {
            tracing::warn!(error = %error, "inbound source lookup failed");
            return Err(InboundError::store_unavailable());
        }
    };
    if !source.acting_user_is_admin() {
        audit_rejection(db, &source, "source_suspended", client).await;
        return Err(InboundError::source_suspended());
    }

    enforce_source_rate(state, &source, client).await?;

    if !is_json_content_type(request.headers()) {
        return Err(InboundError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ));
    }
    let max_body = usize::try_from(config.max_body_bytes).unwrap_or(usize::MAX);
    let declared_too_large = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > max_body);
    if declared_too_large {
        return Err(InboundError::new(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large"));
    }

    let (mut parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, max_body)
        .await
        .map_err(|_| InboundError::new(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large"))?;

    let signed = match source.signing_secret.as_deref() {
        Some(secret) => {
            let header_text = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());
            match check_signature(
                secret,
                header_text(TIMESTAMP_HEADER),
                header_text(SIGNATURE_HEADER),
                &bytes,
                Timestamp::now().unix_timestamp(),
                i64::from(config.signature_tolerance_secs),
            ) {
                SignatureCheck::Valid => true,
                SignatureCheck::Absent if !source.require_signature => false,
                SignatureCheck::Absent | SignatureCheck::Invalid => {
                    audit_rejection(db, &source, "invalid_signature", client).await;
                    return Err(count_failure(db, &config, client, InboundError::invalid_signature()).await);
                }
            }
        }
        None => false,
    };

    touch_last_used(db, source.id, client).await;
    let verified = VerifiedRequest {
        body_sha256: hex::encode(Sha256::digest(&bytes)),
        source,
        body: bytes,
        signed,
        client,
        config,
    };
    parts.extensions.insert(verified);
    Ok(Request::from_parts(parts, Body::empty()))
}

/// Counts the event against the source's minute and day allowances.
async fn enforce_source_rate(state: &InboundAuthState, source: &SourceRow, client: IpAddr) -> Result<(), InboundError> {
    let db = &state.app.db;
    let key = RateKey::from_id(source.id);
    let checks = [
        (RateBucket::InboundSourceMinute, source.rate_per_minute),
        (RateBucket::InboundSourceDay, source.daily_cap),
    ];
    for (bucket, limit) in checks {
        match enforce_with_limit(db, bucket, &key, u32::try_from(limit).unwrap_or(1)).await {
            Ok(()) => {}
            Err(RateError::Limited(limited)) => {
                audit_rejection(db, source, "rate_limited", client).await;
                if let Err(error) = notify::source_throttled(db, source).await {
                    tracing::warn!(error = %error, "inbound throttle notice failed");
                }
                return Err(InboundError::rate_limited(limited.retry_after_secs));
            }
            Err(RateError::Store(error)) => {
                tracing::warn!(error = %error, "inbound rate counter unavailable");
                return Err(InboundError::store_unavailable());
            }
        }
    }
    Ok(())
}

async fn touch_last_used<C: ConnectionTrait>(db: &C, source_id: Uuid, client: IpAddr) {
    if let Err(error) = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE inbound_sources SET last_used_at = now(), last_used_ip = $2::inet WHERE id = $1",
            vec![source_id.into(), client.to_string().into()],
        ))
        .await
    {
        tracing::warn!(error = %error, "inbound last-used update failed");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = "svs_test-secret-built-at-runtime";
    const NOW: i64 = 1_800_000_000;

    fn signed_headers(timestamp: i64, body: &[u8]) -> (String, String) {
        let ts = timestamp.to_string();
        let mac = compute_signature(SECRET, &ts, body);
        (ts, format!("v1={mac}"))
    }

    #[test]
    fn a_correct_signature_inside_the_window_is_valid() {
        let body = br#"{"a":1}"#;
        let (ts, sig) = signed_headers(NOW - 10, body);
        assert_eq!(
            check_signature(SECRET, Some(&ts), Some(&sig), body, NOW, 300),
            SignatureCheck::Valid
        );
    }

    #[test]
    fn the_window_is_inclusive_and_symmetric() {
        let body = b"{}";
        for (offset, expected) in [
            (-300, SignatureCheck::Valid),
            (300, SignatureCheck::Valid),
            (-301, SignatureCheck::Invalid),
            (301, SignatureCheck::Invalid),
        ] {
            let (ts, sig) = signed_headers(NOW + offset, body);
            assert_eq!(
                check_signature(SECRET, Some(&ts), Some(&sig), body, NOW, 300),
                expected,
                "offset {offset}"
            );
        }
    }

    #[test]
    fn a_changed_body_a_wrong_secret_or_a_bad_format_is_invalid() {
        let body = b"{}";
        let (ts, sig) = signed_headers(NOW, body);
        assert_eq!(
            check_signature(SECRET, Some(&ts), Some(&sig), b"{ }", NOW, 300),
            SignatureCheck::Invalid
        );
        assert_eq!(
            check_signature("svs_other", Some(&ts), Some(&sig), body, NOW, 300),
            SignatureCheck::Invalid
        );
        let bare = sig.trim_start_matches("v1=");
        for bad in [bare, "v2=00", "v1=zz", "v1=", ""] {
            assert_eq!(
                check_signature(SECRET, Some(&ts), Some(bad), body, NOW, 300),
                SignatureCheck::Invalid,
                "{bad}"
            );
        }
        assert_eq!(
            check_signature(SECRET, Some("not-a-number"), Some(&sig), body, NOW, 300),
            SignatureCheck::Invalid
        );
        // The timestamp is part of the signed text: moving it invalidates the MAC.
        let later = (NOW + 1).to_string();
        assert_eq!(
            check_signature(SECRET, Some(&later), Some(&sig), body, NOW, 300),
            SignatureCheck::Invalid
        );
    }

    #[test]
    fn one_header_without_the_other_is_invalid_and_none_is_absent() {
        let (ts, sig) = signed_headers(NOW, b"{}");
        assert_eq!(
            check_signature(SECRET, Some(&ts), None, b"{}", NOW, 300),
            SignatureCheck::Invalid
        );
        assert_eq!(
            check_signature(SECRET, None, Some(&sig), b"{}", NOW, 300),
            SignatureCheck::Invalid
        );
        assert_eq!(
            check_signature(SECRET, None, None, b"{}", NOW, 300),
            SignatureCheck::Absent
        );
    }

    #[test]
    fn the_signature_matches_the_documented_construction() {
        // HMAC-SHA256 over "<timestamp>.<body>" keyed with the secret, lower case hex: the same
        // bytes `openssl dgst -sha256 -hmac` produces for that input.
        let mac = compute_signature("key", "1700000000", b"{}");
        assert_eq!(mac.len(), 64);
        assert!(
            mac.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        let mut manual = <Hmac<Sha256> as Mac>::new_from_slice(b"key").unwrap();
        manual.update(b"1700000000.{}");
        assert_eq!(mac, hex::encode(manual.finalize().into_bytes()));
    }

    #[test]
    fn only_a_bearer_header_carries_the_credential() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer_token(&headers), None);
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer svi_abc"));
        assert_eq!(bearer_token(&headers), Some("svi_abc"));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("bearer svi_abc"));
        assert_eq!(bearer_token(&headers), Some("svi_abc"));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic svi_abc"));
        assert_eq!(bearer_token(&headers), None);
        headers.clear();
        headers.insert("x-waf-secret", HeaderValue::from_static("svi_abc"));
        assert_eq!(
            bearer_token(&headers),
            None,
            "the prx-waf header is not accepted by this stage"
        );
    }

    #[test]
    fn a_token_query_parameter_is_detected() {
        assert!(!query_has_token(None));
        assert!(!query_has_token(Some("a=1&b=2")));
        assert!(query_has_token(Some("token=x")));
        assert!(query_has_token(Some("a=1&Token=x")));
        assert!(!query_has_token(Some("tokens=x")));
    }

    #[test]
    fn content_type_must_be_json() {
        let mut headers = HeaderMap::new();
        assert!(!is_json_content_type(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(is_json_content_type(&headers));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(!is_json_content_type(&headers));
    }

    #[test]
    fn the_acting_user_must_still_be_an_owner_or_admin() {
        let row = |role: Option<&str>, acting: bool| SourceRow {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            project_id: Uuid::nil(),
            name: String::new(),
            signing_secret: None,
            require_signature: false,
            acting_user_id: acting.then(Uuid::new_v4),
            enabled: true,
            rate_per_minute: 1,
            daily_cap: 1,
            acting_role: role.map(str::to_owned),
        };
        assert!(row(Some("owner"), true).acting_user_is_admin());
        assert!(row(Some("admin"), true).acting_user_is_admin());
        assert!(!row(Some("member"), true).acting_user_is_admin());
        assert!(!row(None, true).acting_user_is_admin());
        assert!(!row(Some("admin"), false).acting_user_is_admin());
    }
}
