//! `POST /api/v1/oauth/token`: authorization code (PKCE S256) and refresh token grants.
//!
//! The endpoint is public (`token_endpoint_auth_method: none`): the client is named by
//! `client_id` in the form body. Every request is counted against the client address before any
//! parsing, and against the grant once it is known. Failures answer with the standard error codes
//! and nothing more: an unknown code, a code of another client and an expired code all look the
//! same, and the reason (an enum of static strings) goes to the audit trail only.
//!
//! Transactions: a redemption runs in one transaction with the code or refresh row locked. A
//! failure that must still leave a trace in the database (replay or reuse revoking the grant, the
//! failed-attempt counter of a code) commits; every other failure rolls back and its audit row is
//! written on its own.
//!
//! Failed redemptions of one code: a wrong verifier or redirect URI does not consume the code (so
//! a stranger's failed attempt cannot turn the legitimate redemption into a replay revocation),
//! but the third failure invalidates the code. No token has been issued from it, so no grant is
//! revoked; the client restarts the flow.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use platform::app::AppState;
use platform::config::OAuthConfig;
use platform::timestamp::Timestamp;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::audit::{self, AuditEvent, AuditRecord, DetailValue};
use super::error::{OAuthError, OAuthErrorKind, no_store_headers};
use super::grants::{Actor, RevokeReason, revoke_grant};
use super::rate::{self, RateBucket, RateError, RateKey};
use super::scopes::{Scope, ScopeSet, TokenResource};
use super::tokens::{Lifetimes, MintedToken, TokenKind, is_well_formed, mint, secrets_equal};

/// Path of the token endpoint.
pub const TOKEN_PATH: &str = "/api/v1/oauth/token";
/// Largest request body the protocol endpoints read.
pub const MAX_BODY_BYTES: usize = 16 * 1024;
/// Failed redemptions of one code before it is invalidated.
pub const MAX_CODE_FAILURES: i32 = 3;

const CODE_FAILURE_BUCKET: &str = "code_failures";
const MAX_FORM_PARAMS: usize = 32;

// ---------------------------------------------------------------------------------------------
// Request plumbing shared with the revocation endpoint
// ---------------------------------------------------------------------------------------------

/// What a protocol endpoint needs from the request.
pub(super) struct Incoming {
    pub headers: HeaderMap,
    pub peer: Option<SocketAddr>,
    pub query: Option<String>,
    pub body: Vec<u8>,
}

pub(super) async fn read_request(req: Request) -> Result<Incoming, OAuthError> {
    let (parts, body) = req.into_parts();
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
    let body = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| OAuthError::new(OAuthErrorKind::InvalidRequest, "the request body is too large"))?;
    Ok(Incoming {
        headers: parts.headers,
        peer,
        query: parts.uri.query().map(str::to_owned),
        body: body.to_vec(),
    })
}

/// Parameters of a protocol endpoint travel in the form body only. A query string is refused so
/// that no code, token or verifier is ever put in a URL.
pub(super) fn refuse_query(incoming: &Incoming) -> Result<(), OAuthError> {
    if incoming.query.as_deref().is_some_and(|query| !query.is_empty()) {
        return Err(OAuthError::new(
            OAuthErrorKind::InvalidRequest,
            "parameters must be sent in the request body",
        ));
    }
    Ok(())
}

pub(super) fn require_form_content_type(headers: &HeaderMap) -> Result<(), OAuthError> {
    let is_form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/x-www-form-urlencoded"));
    if is_form {
        Ok(())
    } else {
        Err(OAuthError::new(
            OAuthErrorKind::InvalidRequest,
            "the content type must be application/x-www-form-urlencoded",
        ))
    }
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Form decoding of one name or value: `+` is a space, `%XX` is a byte, the result must be UTF-8.
fn form_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'+' => out.push(b' '),
            b'%' => {
                let high = hex_value(*bytes.get(index + 1)?)?;
                let low = hex_value(*bytes.get(index + 2)?)?;
                out.push(high << 4 | low);
                index += 2;
            }
            other => out.push(other),
        }
        index += 1;
    }
    String::from_utf8(out).ok()
}

/// The decoded parameters of a form body. `None` when the body is malformed or a parameter name
/// occurs twice (RFC 6749 section 3.2: parameters must not be repeated).
pub(super) fn parse_form(body: &[u8]) -> Option<Vec<(String, String)>> {
    let text = std::str::from_utf8(body).ok()?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    for segment in text.split('&').filter(|segment| !segment.is_empty()) {
        let (name, value) = segment.split_once('=').unwrap_or((segment, ""));
        let name = form_decode(name)?;
        let value = form_decode(value)?;
        if pairs.iter().any(|(seen, _)| *seen == name) || pairs.len() >= MAX_FORM_PARAMS {
            return None;
        }
        pairs.push((name, value));
    }
    Some(pairs)
}

pub(super) fn form_get<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

fn form_required<'a>(pairs: &'a [(String, String)], name: &str) -> Result<&'a str, OAuthError> {
    form_get(pairs, name)
        .filter(|value| !value.is_empty())
        .ok_or(OAuthError::new(
            OAuthErrorKind::InvalidRequest,
            "a required parameter is missing",
        ))
}

/// A JSON response with the headers every OAuth response carries.
pub(super) fn json_response(value: &Value) -> Response {
    let mut response = Json(value).into_response();
    for (name, header_value) in no_store_headers() {
        response.headers_mut().insert(name, header_value);
    }
    response
}

/// An empty 200 with the no-store headers.
pub(super) fn empty_ok() -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    for (name, header_value) in no_store_headers() {
        response.headers_mut().insert(name, header_value);
    }
    response
}

pub(super) fn db_error(error: &DbErr) -> OAuthError {
    tracing::error!(error = %error, "oauth database error");
    OAuthError::server_error()
}

pub(super) fn rate_error(error: RateError) -> OAuthError {
    match error {
        RateError::Limited(limited) => limited.into(),
        RateError::Store(error) => db_error(&error),
    }
}

/// A minted secret, or a logged `server_error` when the random source failed.
pub(super) fn mint_or_fail<T>(result: Result<T, super::tokens::TokenError>) -> Result<T, OAuthError> {
    result.map_err(|error| {
        tracing::error!(error = %error, "oauth token minting failed");
        OAuthError::server_error()
    })
}

pub(super) async fn not_found() -> Response {
    OAuthError::not_found().into_response()
}

pub(super) fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// The resource a presented `resource` URL names, by exact comparison with the configured ones.
pub(super) fn resource_of(config: &OAuthConfig, url: &str) -> Option<TokenResource> {
    if url == config.api_resource {
        Some(TokenResource::Api)
    } else if config.mcp_resource.as_deref() == Some(url) {
        Some(TokenResource::Mcp)
    } else {
        None
    }
}

/// The client row the endpoints need.
#[derive(Debug, FromQueryResult)]
pub(super) struct ClientRow {
    pub client_id: String,
    pub display_name: String,
    pub description: String,
    pub status: String,
    pub redirect_uris: Vec<String>,
    pub allowed_scopes: Vec<String>,
    pub allowed_resources: Vec<String>,
}

const CLIENT_SQL: &str = "SELECT client_id, display_name, description, status, redirect_uris, allowed_scopes, \
     allowed_resources FROM oauth_clients WHERE client_id = $1";
const CLIENT_SQL_SHARED: &str = "SELECT client_id, display_name, description, status, redirect_uris, allowed_scopes, \
     allowed_resources FROM oauth_clients WHERE client_id = $1 FOR SHARE";

async fn load_client<C: ConnectionTrait>(
    conn: &C,
    client_id: &str,
    sql: &'static str,
) -> Result<Option<ClientRow>, DbErr> {
    let row = ClientRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![client_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.filter(|row| row.status == "active"))
}

/// An active client by id, or `None` for an unknown or disabled one.
pub(super) async fn load_active_client<C: ConnectionTrait>(
    conn: &C,
    client_id: &str,
) -> Result<Option<ClientRow>, DbErr> {
    load_client(conn, client_id, CLIENT_SQL).await
}

/// Like [`load_active_client`], and keeps the client row share-locked until the transaction ends,
/// so disabling the client (which updates the row before it sweeps the client's grants) waits for
/// a consent in flight, and a consent that starts after it sees the client disabled.
pub(super) async fn load_active_client_shared<C: ConnectionTrait>(
    conn: &C,
    client_id: &str,
) -> Result<Option<ClientRow>, DbErr> {
    load_client(conn, client_id, CLIENT_SQL_SHARED).await
}

// ---------------------------------------------------------------------------------------------
// Transaction outcome handling
// ---------------------------------------------------------------------------------------------

/// A failed redemption: the answer, whether the transaction's writes must be kept, and the audit
/// row to write.
struct Failure {
    error: OAuthError,
    commit: bool,
    audit: Option<Box<AuditRecord>>,
    /// A grant to end in this transaction, which commits with it.
    revoke: Option<(Uuid, RevokeReason)>,
}

impl From<DbErr> for Failure {
    fn from(error: DbErr) -> Self {
        Self {
            error: db_error(&error),
            commit: false,
            audit: None,
            revoke: None,
        }
    }
}

async fn finish<T>(
    tx: DatabaseTransaction,
    db: &DatabaseConnection,
    result: Result<T, Failure>,
) -> Result<T, OAuthError> {
    match result {
        Ok(value) => {
            tx.commit().await.map_err(|error| db_error(&error))?;
            Ok(value)
        }
        Err(failure) if failure.revoke.is_some() => {
            // The grant row is already held (it is locked first), so ending the grant here is in
            // the lock order; the revocation and the audit row commit together.
            if let Some((grant_id, reason)) = failure.revoke {
                let ended = async {
                    revoke_grant(&tx, grant_id, reason, Actor::System).await?;
                    if let Some(record) = &failure.audit {
                        audit::write(&tx, record).await?;
                    }
                    Ok::<(), sea_orm::DbErr>(())
                }
                .await;
                if let Err(error) = ended {
                    tracing::error!(error = %error, "oauth grant revocation failed");
                    return Err(OAuthError::server_error());
                }
            }
            tx.commit().await.map_err(|error| db_error(&error))?;
            Err(failure.error)
        }
        Err(failure) if failure.commit => {
            if let Some(record) = &failure.audit
                && let Err(error) = audit::write(&tx, record).await
            {
                tracing::error!(error = %error, "oauth audit write failed");
                return Err(OAuthError::server_error());
            }
            tx.commit().await.map_err(|error| db_error(&error))?;
            Err(failure.error)
        }
        Err(failure) => {
            if let Err(error) = tx.rollback().await {
                tracing::warn!(error = %error, "oauth transaction rollback failed");
            }
            if let Some(record) = &failure.audit
                && let Err(error) = audit::write(db, record).await
            {
                tracing::error!(error = %error, "oauth audit write failed");
            }
            Err(failure.error)
        }
    }
}

/// What every step of a redemption knows about the request.
struct Ctx<'a> {
    db: &'a DatabaseConnection,
    life: Lifetimes,
    ip: IpAddr,
    user_agent: Option<String>,
    client_id: &'a str,
}

impl Ctx<'_> {
    fn record(&self, event: AuditEvent) -> AuditRecord {
        let record = AuditRecord::new(event)
            .with_client(self.client_id)
            .with_client_ip(self.ip);
        match &self.user_agent {
            Some(agent) => record.with_user_agent(agent),
            None => record,
        }
    }

    /// A refused redemption: `TokenRejected` with the reason code.
    fn rejected(&self, reason: &'static str) -> AuditRecord {
        self.record(AuditEvent::TokenRejected)
            .with_detail("endpoint", DetailValue::Code("token"))
            .with_detail("reason", DetailValue::Code(reason))
    }

    fn fail(&self, error: OAuthError, reason: &'static str) -> Failure {
        Failure {
            error,
            commit: false,
            audit: boxed(self.rejected(reason)),
            revoke: None,
        }
    }

    fn fail_for_grant(&self, error: OAuthError, reason: &'static str, grant: &GrantState) -> Failure {
        Failure {
            error,
            commit: false,
            audit: boxed(grant.annotate(self.rejected(reason))),
            revoke: None,
        }
    }
}

#[allow(clippy::unnecessary_wraps, reason = "fills the optional audit field of a failure")]
fn boxed(record: AuditRecord) -> Option<Box<AuditRecord>> {
    Some(Box::new(record))
}

const fn invalid_grant() -> OAuthError {
    OAuthError::new(OAuthErrorKind::InvalidGrant, "the grant is invalid, expired or revoked")
}

const fn invalid_target() -> OAuthError {
    OAuthError::new(
        OAuthErrorKind::InvalidTarget,
        "the resource is not valid for this grant",
    )
}

const fn invalid_scope() -> OAuthError {
    OAuthError::new(
        OAuthErrorKind::InvalidScope,
        "the scope is not valid for this grant or resource",
    )
}

// ---------------------------------------------------------------------------------------------
// Grant state
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the fields are the columns of the row")]
#[allow(clippy::struct_excessive_bools, reason = "the fields are the columns of the row")]
struct GrantState {
    grant_id: Uuid,
    user_id: Uuid,
    workspace_id: Uuid,
    grant_scopes: Vec<String>,
    grant_resources: Vec<String>,
    grant_status: String,
    grant_live: bool,
    grant_expires_at: Timestamp,
    grant_session_version: i64,
    client_active: bool,
    user_active: bool,
    user_session_version: i64,
    is_member: bool,
    allows_apps: bool,
}

impl GrantState {
    const fn annotate(&self, record: AuditRecord) -> AuditRecord {
        record
            .with_grant(self.grant_id)
            .with_user(self.user_id)
            .with_workspace(self.workspace_id)
    }

    /// The first failed live condition, as the revocation to apply (when the grant is still
    /// active) and the audit reason code.
    fn failure(&self) -> Option<(Option<RevokeReason>, &'static str)> {
        if self.grant_status != "active" {
            Some((None, "grant_not_active"))
        } else if !self.grant_live {
            Some((Some(RevokeReason::Expired), "grant_expired"))
        } else if !self.client_active {
            Some((Some(RevokeReason::ClientDisabled), "client_disabled"))
        } else if !self.user_active {
            Some((Some(RevokeReason::AccountDisabled), "user_inactive"))
        } else if self.user_session_version != self.grant_session_version {
            Some((Some(RevokeReason::PasswordChanged), "session_version_changed"))
        } else if !self.is_member {
            Some((Some(RevokeReason::MembershipRemoved), "membership_missing"))
        } else if !self.allows_apps {
            Some((Some(RevokeReason::WorkspaceDisabled), "workspace_disallows"))
        } else {
            None
        }
    }

    fn scopes(&self) -> Result<ScopeSet, Failure> {
        ScopeSet::from_names(self.grant_scopes.iter().map(String::as_str)).map_err(|_| Failure {
            error: OAuthError::server_error(),
            commit: false,
            audit: None,
            revoke: None,
        })
    }
}

/// Locks the grant row (`FOR NO KEY UPDATE`) and reads its live state. This is the first lock a
/// redemption takes (lock order: grant, then the code or refresh token row): a revocation that
/// committed before the lock was granted is seen as `revoked`, and one that starts later waits
/// for this transaction and then revokes whatever it issued.
async fn load_grant_state(tx: &DatabaseTransaction, grant_id: Uuid) -> Result<Option<GrantState>, DbErr> {
    GrantState::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT g.id AS grant_id, g.user_id, g.workspace_id, g.scopes AS grant_scopes, \
                g.resources AS grant_resources, g.status AS grant_status, \
                (g.expires_at > now()) AS grant_live, g.expires_at AS grant_expires_at, \
                g.user_session_version AS grant_session_version, \
                (cl.status = 'active') AS client_active, \
                (u.is_active AND u.entity_type = 'human') AS user_active, \
                u.session_version AS user_session_version, \
                (m.user_id IS NOT NULL) AS is_member, \
                COALESCE(s.allow_connected_apps, true) AS allows_apps \
         FROM oauth_grants g \
         JOIN oauth_clients cl ON cl.client_id = g.client_id \
         JOIN users u ON u.id = g.user_id \
         LEFT JOIN workspace_members m ON m.workspace_id = g.workspace_id AND m.user_id = g.user_id \
         LEFT JOIN workspace_delegation_settings s ON s.workspace_id = g.workspace_id \
         WHERE g.id = $1 \
         FOR NO KEY UPDATE OF g",
        vec![grant_id.into()],
    ))
    .one(tx)
    .await
}

/// Applies a live-state failure: revokes the grant when it is still active (committing that) and
/// answers `invalid_grant`.
fn state_failure(ctx: &Ctx<'_>, grant: &GrantState, revoke: Option<RevokeReason>, reason: &'static str) -> Failure {
    Failure {
        error: invalid_grant(),
        commit: false,
        audit: boxed(grant.annotate(ctx.rejected(reason))),
        revoke: revoke.map(|reason| (grant.grant_id, reason)),
    }
}

// ---------------------------------------------------------------------------------------------
// Issuing tokens
// ---------------------------------------------------------------------------------------------

struct Issued {
    access: MintedToken,
    access_expires_in: i64,
    refresh: Option<MintedToken>,
    scope: ScopeSet,
    grant_id: Uuid,
    workspace_id: Uuid,
}

fn mint_failure<T>(result: Result<T, super::tokens::TokenError>) -> Result<T, Failure> {
    result.map_err(|error| {
        tracing::error!(error = %error, "oauth token minting failed");
        Failure {
            error: OAuthError::server_error(),
            commit: false,
            audit: None,
            revoke: None,
        }
    })
}

/// Inserts an access token for `resource` and, when `with_refresh`, a rotated refresh token.
async fn issue(
    tx: &DatabaseTransaction,
    ctx: &Ctx<'_>,
    grant: &GrantState,
    resource: TokenResource,
    access_scope: ScopeSet,
    parent_refresh: Option<Uuid>,
    with_refresh: bool,
) -> Result<(Issued, Uuid), Failure> {
    let now = Timestamp::now();
    let access = mint_failure(mint(TokenKind::Access))?;
    let access_id = Uuid::new_v4();
    let access_expires_at = ctx.life.access_expires_at(now, grant.grant_expires_at);
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, expires_at) \
         VALUES ($1, $2, 'access', $3, $4, $5, $6, $7)",
        vec![
            access_id.into(),
            access.hash.as_hex().into(),
            grant.grant_id.into(),
            ctx.client_id.into(),
            resource.as_str().into(),
            access_scope.to_names().into(),
            access_expires_at.into(),
        ],
    ))
    .await?;
    let refresh = if with_refresh {
        let refresh = mint_failure(mint(TokenKind::Refresh))?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, parent_id, \
             expires_at) VALUES ($1, $2, 'refresh', $3, $4, NULL, $5, $6, $7)",
            vec![
                Uuid::new_v4().into(),
                refresh.hash.as_hex().into(),
                grant.grant_id.into(),
                ctx.client_id.into(),
                grant.scopes()?.to_names().into(),
                parent_refresh.into(),
                ctx.life.refresh_expires_at(now, grant.grant_expires_at).into(),
            ],
        ))
        .await?;
        Some(refresh)
    } else {
        None
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE oauth_grants SET last_used_at = now() WHERE id = $1",
        vec![grant.grant_id.into()],
    ))
    .await?;
    let expires_in = (access_expires_at - now).whole_seconds().max(0);
    Ok((
        Issued {
            access,
            access_expires_in: expires_in,
            refresh,
            scope: access_scope,
            grant_id: grant.grant_id,
            workspace_id: grant.workspace_id,
        },
        access_id,
    ))
}

fn issued_response(issued: &Issued) -> Response {
    let mut body = json!({
        "access_token": issued.access.raw.expose(),
        "token_type": "Bearer",
        "expires_in": issued.access_expires_in,
        "scope": issued.scope.to_scope_string(),
        "grant_id": issued.grant_id,
        "workspace_id": issued.workspace_id,
    });
    if let (Some(refresh), Some(map)) = (&issued.refresh, body.as_object_mut()) {
        map.insert(
            "refresh_token".to_owned(),
            Value::String(refresh.raw.expose().to_owned()),
        );
    }
    json_response(&body)
}

// ---------------------------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------------------------

/// A code verifier is 43 to 128 characters of `[A-Za-z0-9-._~]` (RFC 7636 section 4.1).
pub fn is_valid_code_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// Whether `challenge` has the shape the authorization request accepts.
///
/// A code challenge for S256 is the unpadded base64url of a SHA-256 digest: exactly 43
/// characters. The authorization request accepts the wider range of the contract (43 to 128 characters of the
/// unreserved set); the stored value is compared as text.
pub fn is_valid_code_challenge(challenge: &str) -> bool {
    is_valid_code_verifier(challenge)
}

/// `BASE64URL(SHA-256(verifier))`.
pub fn s256_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Whether `verifier` answers `challenge`, in constant time. A verifier of the wrong shape never
/// verifies. `plain` does not exist: only S256 is ever computed.
pub fn pkce_verifies(verifier: &str, challenge: &str) -> bool {
    is_valid_code_verifier(verifier) && secrets_equal(&s256_challenge(verifier), challenge)
}

// ---------------------------------------------------------------------------------------------
// Authorization code grant
// ---------------------------------------------------------------------------------------------

struct CodeParams<'a> {
    code_hash: &'a str,
    redirect_uri: &'a str,
    verifier: &'a str,
    resource: TokenResource,
}

#[derive(Debug, FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the fields are the columns of the row")]
struct CodeRow {
    code_client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_resources: Vec<String>,
    code_live: bool,
    code_consumed: bool,
}

#[derive(Debug, FromQueryResult)]
struct FailureCount {
    hit_count: i32,
}

/// Counts a failed redemption of the code and invalidates it at the third. The counter lives in
/// `oauth_rate_hits` under its own bucket, keyed by the code's hash.
async fn count_code_failure(
    tx: &DatabaseTransaction,
    ctx: &Ctx<'_>,
    params: &CodeParams<'_>,
    grant: &GrantState,
    reason: &'static str,
) -> Failure {
    let counted = FailureCount::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO oauth_rate_hits AS hit (bucket, key_hash, window_started, hit_count) \
         VALUES ($1, $2, now(), 1) \
         ON CONFLICT (bucket, key_hash) DO UPDATE SET hit_count = hit.hit_count + 1 \
         RETURNING hit_count",
        vec![CODE_FAILURE_BUCKET.into(), params.code_hash.into()],
    ))
    .one(tx)
    .await;
    let count = match counted {
        Ok(Some(row)) => row.hit_count,
        Ok(None) => MAX_CODE_FAILURES,
        Err(error) => return Failure::from(error),
    };
    let mut reason = reason;
    if count >= MAX_CODE_FAILURES {
        let invalidated = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE oauth_authorization_codes SET expires_at = now() WHERE code_hash = $1",
                vec![params.code_hash.into()],
            ))
            .await;
        if let Err(error) = invalidated {
            return Failure::from(error);
        }
        reason = "code_invalidated";
    } else if count == 1 {
        // Stale counters of codes that were never tried again are swept as new ones start.
        let swept = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM oauth_rate_hits WHERE bucket = $1 AND window_started < now() - interval '1 hour'",
                vec![CODE_FAILURE_BUCKET.into()],
            ))
            .await;
        if let Err(error) = swept {
            return Failure::from(error);
        }
    }
    Failure {
        error: invalid_grant(),
        commit: true,
        audit: boxed(
            grant
                .annotate(ctx.rejected(reason))
                .with_detail("failures", DetailValue::Count(i64::from(count))),
        ),
        revoke: None,
    }
}

/// The grant a code or refresh token belongs to, read without a lock so the grant can be locked
/// first.
#[derive(Debug, FromQueryResult)]
struct GrantRef {
    grant_id: Uuid,
}

async fn redeem_code(tx: &DatabaseTransaction, ctx: &Ctx<'_>, params: &CodeParams<'_>) -> Result<Issued, Failure> {
    let grant_ref = GrantRef::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT grant_id FROM oauth_authorization_codes WHERE code_hash = $1",
        vec![params.code_hash.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(|| ctx.fail(invalid_grant(), "unknown_code"))?;
    let grant = load_grant_state(tx, grant_ref.grant_id)
        .await?
        .ok_or_else(|| ctx.fail(invalid_grant(), "unknown_code"))?;
    let row = CodeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT client_id AS code_client_id, redirect_uri, code_challenge, resources AS code_resources, \
                (expires_at > now()) AS code_live, (consumed_at IS NOT NULL) AS code_consumed \
         FROM oauth_authorization_codes WHERE code_hash = $1 FOR UPDATE",
        vec![params.code_hash.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(|| ctx.fail(invalid_grant(), "unknown_code"))?;

    if row.code_client_id != ctx.client_id {
        return Err(ctx.fail_for_grant(invalid_grant(), "client_mismatch", &grant));
    }
    if row.code_consumed {
        // A second redemption of a code: whatever was issued from it is no longer safe.
        tracing::warn!(grant_id = %grant.grant_id, "authorization code replay; grant revoked");
        return Err(Failure {
            error: invalid_grant(),
            commit: false,
            audit: boxed(grant.annotate(ctx.record(AuditEvent::CodeReplay))),
            revoke: Some((grant.grant_id, RevokeReason::CodeReplay)),
        });
    }
    if !row.code_live {
        return Err(ctx.fail_for_grant(invalid_grant(), "code_expired", &grant));
    }
    if let Some((revoke, reason)) = grant.failure() {
        return Err(state_failure(ctx, &grant, revoke, reason));
    }
    if row.redirect_uri != params.redirect_uri {
        return Err(count_code_failure(tx, ctx, params, &grant, "redirect_mismatch").await);
    }
    if !pkce_verifies(params.verifier, &row.code_challenge) {
        return Err(count_code_failure(tx, ctx, params, &grant, "verifier_mismatch").await);
    }
    if !row.code_resources.iter().any(|name| name == params.resource.as_str()) {
        return Err(ctx.fail_for_grant(invalid_target(), "resource_not_in_code", &grant));
    }
    let granted = grant.scopes()?;
    let access_scope = granted.narrow_to_resource(params.resource);
    if access_scope.is_empty() {
        return Err(ctx.fail_for_grant(invalid_scope(), "empty_scope_for_resource", &grant));
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE oauth_authorization_codes SET consumed_at = now() WHERE code_hash = $1",
        vec![params.code_hash.into()],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM oauth_rate_hits WHERE bucket = $1 AND key_hash = $2",
        vec![CODE_FAILURE_BUCKET.into(), params.code_hash.into()],
    ))
    .await?;
    let (issued, access_id) = issue(
        tx,
        ctx,
        &grant,
        params.resource,
        access_scope,
        None,
        granted.contains(Scope::OfflineAccess),
    )
    .await?;
    audit::write(
        tx,
        &grant
            .annotate(ctx.record(AuditEvent::CodeRedeemed))
            .with_token(access_id)
            .with_detail("resource", DetailValue::Code(params.resource.as_str())),
    )
    .await?;
    Ok(issued)
}

// ---------------------------------------------------------------------------------------------
// Refresh token grant
// ---------------------------------------------------------------------------------------------

struct RefreshParams<'a> {
    token_hash: &'a str,
    resource: TokenResource,
    scope: Option<ScopeSet>,
}

#[derive(Debug, FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the fields are the columns of the row")]
struct RefreshRow {
    token_id: Uuid,
    token_client_id: String,
    token_consumed: bool,
    token_revoked: bool,
    token_live: bool,
}

async fn redeem_refresh(
    tx: &DatabaseTransaction,
    ctx: &Ctx<'_>,
    params: &RefreshParams<'_>,
) -> Result<Issued, Failure> {
    let grant_ref = GrantRef::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT grant_id FROM oauth_tokens WHERE token_hash = $1 AND kind = 'refresh'",
        vec![params.token_hash.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(|| ctx.fail(invalid_grant(), "unknown_refresh_token"))?;
    let grant = load_grant_state(tx, grant_ref.grant_id)
        .await?
        .ok_or_else(|| ctx.fail(invalid_grant(), "unknown_refresh_token"))?;
    let row = RefreshRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id AS token_id, client_id AS token_client_id, (consumed_at IS NOT NULL) AS token_consumed, \
                (revoked_at IS NOT NULL) AS token_revoked, (expires_at > now()) AS token_live \
         FROM oauth_tokens WHERE token_hash = $1 AND kind = 'refresh' FOR UPDATE",
        vec![params.token_hash.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(|| ctx.fail(invalid_grant(), "unknown_refresh_token"))?;

    if row.token_client_id != ctx.client_id {
        return Err(ctx.fail_for_grant(invalid_grant(), "client_mismatch", &grant));
    }
    if row.token_consumed {
        // A rotated refresh token came back: someone holds a copy. End the whole grant.
        tracing::warn!(grant_id = %grant.grant_id, "refresh token reuse detected; grant revoked");
        return Err(Failure {
            error: invalid_grant(),
            commit: false,
            audit: boxed(
                grant
                    .annotate(ctx.record(AuditEvent::RefreshReuseDetected))
                    .with_token(row.token_id),
            ),
            revoke: Some((grant.grant_id, RevokeReason::RefreshReuse)),
        });
    }
    if let Err(error) = rate::enforce(ctx.db, RateBucket::TokenGrant, &RateKey::from_id(grant.grant_id)).await {
        return Err(Failure {
            error: rate_error(error),
            commit: false,
            audit: boxed(grant.annotate(ctx.rejected("rate_limited"))),
            revoke: None,
        });
    }
    if row.token_revoked {
        return Err(ctx.fail_for_grant(invalid_grant(), "token_revoked", &grant));
    }
    if !row.token_live {
        return Err(state_failure(
            ctx,
            &grant,
            Some(RevokeReason::Expired),
            "refresh_token_expired",
        ));
    }
    if let Some((revoke, reason)) = grant.failure() {
        return Err(state_failure(ctx, &grant, revoke, reason));
    }
    if !grant
        .grant_resources
        .iter()
        .any(|name| name == params.resource.as_str())
    {
        return Err(ctx.fail_for_grant(invalid_target(), "resource_not_granted", &grant));
    }
    let granted = grant.scopes()?;
    let requested = match params.scope {
        Some(requested) => granted
            .narrow_to_subset(requested)
            .map_err(|_| ctx.fail_for_grant(invalid_scope(), "scope_not_subset", &grant))?,
        None => granted,
    };
    let access_scope = requested.narrow_to_resource(params.resource);
    if access_scope.is_empty() {
        return Err(ctx.fail_for_grant(invalid_scope(), "empty_scope_for_resource", &grant));
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE oauth_tokens SET consumed_at = now(), last_used_at = now() WHERE id = $1",
        vec![row.token_id.into()],
    ))
    .await?;
    let (issued, access_id) = issue(tx, ctx, &grant, params.resource, access_scope, Some(row.token_id), true).await?;
    audit::write(
        tx,
        &grant
            .annotate(ctx.record(AuditEvent::TokenRefreshed))
            .with_token(access_id)
            .with_detail("resource", DetailValue::Code(params.resource.as_str())),
    )
    .await?;
    Ok(issued)
}

// ---------------------------------------------------------------------------------------------
// The handler
// ---------------------------------------------------------------------------------------------

async fn token_endpoint(
    State(state): State<AppState>,
    Extension(config): Extension<Arc<OAuthConfig>>,
    req: Request,
) -> Response {
    match handle(&state, &config, req).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn handle(state: &AppState, config: &OAuthConfig, req: Request) -> Result<Response, OAuthError> {
    let incoming = read_request(req).await?;
    let ip = rate::client_address(&incoming.headers, incoming.peer);
    rate::enforce(&state.db, RateBucket::TokenIp, &RateKey::from_ip(ip))
        .await
        .map_err(rate_error)?;
    refuse_query(&incoming)?;
    require_form_content_type(&incoming.headers)?;
    let pairs = parse_form(&incoming.body).ok_or(OAuthError::new(
        OAuthErrorKind::InvalidRequest,
        "the request body is not a valid form",
    ))?;

    let client_id = form_required(&pairs, "client_id")?;
    let client = load_active_client(&state.db, client_id)
        .await
        .map_err(|error| db_error(&error))?
        .ok_or(OAuthError::new(
            OAuthErrorKind::InvalidClient,
            "the client is not recognised",
        ))?;
    let ctx = Ctx {
        db: &state.db,
        life: Lifetimes::from_config(config),
        ip,
        user_agent: user_agent(&incoming.headers),
        client_id: &client.client_id,
    };

    match form_required(&pairs, "grant_type")? {
        "authorization_code" => authorization_code_grant(state, config, &ctx, &pairs).await,
        "refresh_token" => refresh_token_grant(state, config, &ctx, &pairs).await,
        _ => Err(OAuthError::new(
            OAuthErrorKind::UnsupportedGrantType,
            "the grant type is not supported",
        )),
    }
}

fn requested_resource(config: &OAuthConfig, pairs: &[(String, String)]) -> Result<TokenResource, OAuthError> {
    let url = form_required(pairs, "resource")?;
    resource_of(config, url).ok_or_else(invalid_target)
}

async fn authorization_code_grant(
    state: &AppState,
    config: &OAuthConfig,
    ctx: &Ctx<'_>,
    pairs: &[(String, String)],
) -> Result<Response, OAuthError> {
    let code = form_required(pairs, "code")?;
    let redirect_uri = form_required(pairs, "redirect_uri")?;
    let verifier = form_required(pairs, "code_verifier")?;
    if !is_valid_code_verifier(verifier) {
        return Err(OAuthError::new(
            OAuthErrorKind::InvalidRequest,
            "the code verifier is not valid",
        ));
    }
    let resource = requested_resource(config, pairs)?;
    if !is_well_formed(TokenKind::AuthorizationCode, code) {
        let _ = audit::write(ctx.db, &ctx.rejected("malformed_code"))
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "oauth audit write failed");
            });
        return Err(invalid_grant());
    }
    let code_hash = super::tokens::hash_secret(code);
    let params = CodeParams {
        code_hash: code_hash.as_hex(),
        redirect_uri,
        verifier,
        resource,
    };
    let tx = state.db.begin().await.map_err(|error| db_error(&error))?;
    let result = redeem_code(&tx, ctx, &params).await;
    let issued = finish(tx, &state.db, result).await?;
    Ok(issued_response(&issued))
}

async fn refresh_token_grant(
    state: &AppState,
    config: &OAuthConfig,
    ctx: &Ctx<'_>,
    pairs: &[(String, String)],
) -> Result<Response, OAuthError> {
    let refresh_token = form_required(pairs, "refresh_token")?;
    let resource = requested_resource(config, pairs)?;
    let scope = match form_get(pairs, "scope") {
        Some(text) => Some(ScopeSet::parse(text).map_err(|_| invalid_scope())?),
        None => None,
    };
    if !is_well_formed(TokenKind::Refresh, refresh_token) {
        let _ = audit::write(ctx.db, &ctx.rejected("malformed_refresh_token"))
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "oauth audit write failed");
            });
        return Err(invalid_grant());
    }
    let token_hash = super::tokens::hash_secret(refresh_token);
    let params = RefreshParams {
        token_hash: token_hash.as_hex(),
        resource,
        scope,
    };
    let tx = state.db.begin().await.map_err(|error| db_error(&error))?;
    let result = redeem_refresh(&tx, ctx, &params).await;
    let issued = finish(tx, &state.db, result).await?;
    Ok(issued_response(&issued))
}

/// The token endpoint's route. Without configuration the path answers 404.
pub fn routes(config: Option<Arc<OAuthConfig>>) -> Router<AppState> {
    config.map_or_else(
        || Router::new().route(TOKEN_PATH, any(not_found)),
        |config| {
            Router::new()
                .route(TOKEN_PATH, post(token_endpoint))
                .layer(Extension(config))
        },
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn form_parsing_decodes_and_rejects_duplicates_and_garbage() {
        let pairs = parse_form(b"a=1&b=x+y%20z&c=").expect("parses");
        assert_eq!(form_get(&pairs, "a"), Some("1"));
        assert_eq!(form_get(&pairs, "b"), Some("x y z"));
        assert_eq!(form_get(&pairs, "c"), Some(""));
        assert_eq!(form_get(&pairs, "d"), None);
        assert!(parse_form(b"a=1&a=2").is_none(), "repeated parameter");
        assert!(parse_form(b"a=%zz").is_none(), "bad escape");
        assert!(parse_form(b"a=%2").is_none(), "short escape");
        assert!(parse_form(b"a=%ff").is_none(), "not utf-8");
        assert!(parse_form(&[0xff, 0xfe]).is_none());
        assert!(parse_form(b"").expect("empty form").is_empty());
        let many: String = (0..40).map(|i| format!("p{i}=1")).collect::<Vec<_>>().join("&");
        assert!(parse_form(many.as_bytes()).is_none(), "too many parameters");
    }

    #[test]
    fn content_type_must_be_the_form_type() {
        let mut headers = HeaderMap::new();
        assert!(require_form_content_type(&headers).is_err());
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(require_form_content_type(&headers).is_err());
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("Application/X-WWW-Form-Urlencoded; charset=UTF-8"),
        );
        assert!(require_form_content_type(&headers).is_ok());
    }

    #[test]
    fn pkce_s256_vector_from_rfc_7636() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(s256_challenge(verifier), challenge);
        assert!(pkce_verifies(verifier, challenge));
    }

    #[test]
    fn pkce_refuses_wrong_short_long_and_plain_verifiers() {
        let verifier = "a".repeat(64);
        let challenge = s256_challenge(&verifier);
        assert!(pkce_verifies(&verifier, &challenge));
        assert!(!pkce_verifies(&"b".repeat(64), &challenge), "wrong verifier");
        assert!(
            !pkce_verifies(&challenge, &challenge),
            "plain: the challenge as the verifier"
        );
        assert!(
            !pkce_verifies(&verifier, &verifier),
            "plain: challenge equal to verifier"
        );
        assert!(
            !pkce_verifies(&"a".repeat(42), &s256_challenge(&"a".repeat(42))),
            "too short"
        );
        assert!(
            !pkce_verifies(&"a".repeat(129), &s256_challenge(&"a".repeat(129))),
            "too long"
        );
        assert!(pkce_verifies(&"a".repeat(43), &s256_challenge(&"a".repeat(43))));
        assert!(pkce_verifies(&"a".repeat(128), &s256_challenge(&"a".repeat(128))));
        assert!(
            !pkce_verifies(&format!("{}!", "a".repeat(50)), &challenge),
            "character outside the set"
        );
        assert!(!pkce_verifies("", &challenge));
    }

    #[test]
    fn verifier_and_challenge_shapes() {
        assert!(is_valid_code_verifier(&"Az09-._~".repeat(6)));
        assert!(!is_valid_code_verifier(&"a".repeat(42)));
        assert!(!is_valid_code_verifier(&"a".repeat(129)));
        assert!(!is_valid_code_verifier(&format!("{} ", "a".repeat(50))));
        assert!(!is_valid_code_verifier(&format!("{}/", "a".repeat(50))));
        assert!(is_valid_code_challenge(&s256_challenge("x")));
        assert!(!is_valid_code_challenge("short"));
    }

    #[test]
    fn resources_map_by_exact_comparison() {
        let mut config = OAuthConfig::with_defaults("https://sv.example.test");
        config.mcp_resource = Some("https://sv.example.test/mcp".to_owned());
        assert_eq!(
            resource_of(&config, "https://sv.example.test/api"),
            Some(TokenResource::Api)
        );
        assert_eq!(
            resource_of(&config, "https://sv.example.test/mcp"),
            Some(TokenResource::Mcp)
        );
        assert_eq!(resource_of(&config, "https://sv.example.test/api/"), None);
        assert_eq!(resource_of(&config, "https://sv.example.test/other"), None);
        config.mcp_resource = None;
        assert_eq!(resource_of(&config, "https://sv.example.test/mcp"), None);
    }
}
