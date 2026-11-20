//! Validation of a delegated access token against the database.
//!
//! [`validate_access_token`] performs steps 1 to 4 of the delegated branch's decision order, each
//! failing closed:
//!
//! 1. the token must be well formed (and, through [`check_source`], must not come from a cookie);
//! 2. one query finds the token with its grant, client, user, membership and workspace setting;
//! 3. the token, the grant, the client, the user, the membership and the workspace setting must
//!    all be live. A failure caused by the user, the membership, the workspace or the client also
//!    revokes the grant, with the matching reason, so the next use fails without the join;
//! 4. the audience must match: the API accepts `api` tokens, and `mcp` tokens only when the
//!    request carries the MCP server's forwarding credential.
//!
//! The raw token is only hashed here; it is never stored in a result, an error or a log field.

use std::fmt;
use std::sync::Arc;

use platform::config::Secret;
use platform::timestamp::Timestamp;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use super::audit::{AuditEvent, AuditRecord, DetailValue};
use super::error::OAuthBearerError;
use super::grants::{Actor, RevokeReason, revoke_grant};
use super::scopes::{ProjectScope, ScopeSet, TokenResource};
use super::tokens::{TokenKind, hash_secret, is_well_formed, secrets_equal};

/// Value of `X-Sylvode-Forwarded-By` that names the MCP server.
pub const FORWARDED_BY_MCP: &str = "mcp-server";
/// Header naming the forwarding party.
pub const FORWARDED_BY_HEADER: &str = "x-sylvode-forwarded-by";
/// Header carrying the forwarding secret.
pub const FORWARD_AUTH_HEADER: &str = "x-sylvode-forward-auth";

/// Where a bearer value was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenSource {
    /// `Authorization: Bearer`.
    Header,
    /// The `access_token` cookie. Delegated tokens are never cookies.
    Cookie,
}

/// The MCP server's forwarding credential as configured and as presented on a request.
///
/// `Debug` shows neither value.
#[derive(Clone, Copy)]
pub struct ForwardCredential<'a> {
    configured: Option<&'a Secret>,
    forwarded_by: Option<&'a str>,
    presented: Option<&'a str>,
}

impl<'a> ForwardCredential<'a> {
    pub const fn new(
        configured: Option<&'a Secret>,
        forwarded_by: Option<&'a str>,
        presented: Option<&'a str>,
    ) -> Self {
        Self {
            configured,
            forwarded_by,
            presented,
        }
    }

    /// No credential: MCP-audience tokens will be refused.
    pub const fn absent() -> Self {
        Self::new(None, None, None)
    }

    /// Whether the request names the MCP server and carries the configured secret. The secrets
    /// are compared in constant time. With no configured secret nothing is ever valid.
    pub fn is_valid(&self) -> bool {
        match (self.configured, self.forwarded_by, self.presented) {
            (Some(configured), Some(FORWARDED_BY_MCP), Some(presented)) => {
                !configured.is_empty() && secrets_equal(configured.expose(), presented)
            }
            _ => false,
        }
    }
}

impl fmt::Debug for ForwardCredential<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ForwardCredential")
            .field("valid", &self.is_valid())
            .finish_non_exhaustive()
    }
}

/// Which audiences a call site accepts.
#[derive(Clone, Copy, Debug)]
pub enum AudienceRule<'a> {
    /// The API's authentication middleware: `api` tokens, and `mcp` tokens only with a valid
    /// forwarding credential.
    Api(ForwardCredential<'a>),
    /// The internal token-context call: `mcp` tokens only, and only with a valid credential.
    McpForwarded(ForwardCredential<'a>),
}

/// Why a token was refused. Kept apart from the response, which says as little as possible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    Malformed,
    CookieSource,
    UnknownToken,
    TokenRevoked,
    TokenExpired,
    GrantNotActive,
    GrantExpired,
    ClientDisabled,
    UserInactive,
    SessionVersionChanged,
    MembershipMissing,
    WorkspaceDisallows,
    AudienceMismatch,
    /// The database could not answer. The request is refused.
    Internal,
}

impl RejectReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::CookieSource => "cookie_source",
            Self::UnknownToken => "unknown_token",
            Self::TokenRevoked => "token_revoked",
            Self::TokenExpired => "token_expired",
            Self::GrantNotActive => "grant_not_active",
            Self::GrantExpired => "grant_expired",
            Self::ClientDisabled => "client_disabled",
            Self::UserInactive => "user_inactive",
            Self::SessionVersionChanged => "session_version_changed",
            Self::MembershipMissing => "membership_missing",
            Self::WorkspaceDisallows => "workspace_disallows",
            Self::AudienceMismatch => "audience_mismatch",
            Self::Internal => "internal",
        }
    }

    /// The grant revocation a live-state failure triggers, if any.
    pub const fn revocation(self) -> Option<RevokeReason> {
        match self {
            Self::GrantExpired => Some(RevokeReason::Expired),
            Self::ClientDisabled => Some(RevokeReason::ClientDisabled),
            Self::UserInactive => Some(RevokeReason::AccountDisabled),
            Self::SessionVersionChanged => Some(RevokeReason::PasswordChanged),
            Self::MembershipMissing => Some(RevokeReason::MembershipRemoved),
            Self::WorkspaceDisallows => Some(RevokeReason::WorkspaceDisabled),
            _ => None,
        }
    }

    /// Whether the refusal belongs to steps 2 to 4 and so gets a (sampled) `token_rejected` row.
    pub const fn is_audited(self) -> bool {
        !matches!(self, Self::Malformed | Self::CookieSource | Self::Internal)
    }
}

/// A refused token, with the ids that are known for it. No secret is part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BearerFailure {
    pub reason: RejectReason,
    pub token_id: Option<Uuid>,
    pub grant_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
}

impl BearerFailure {
    pub(super) const fn bare(reason: RejectReason) -> Self {
        Self {
            reason,
            token_id: None,
            grant_id: None,
            user_id: None,
            workspace_id: None,
        }
    }

    /// The response for this failure: 401 `invalid_token` (the description names the audience
    /// only for an audience mismatch), or 500 when the database failed.
    pub fn to_error(&self, resource_metadata: Option<&str>) -> OAuthBearerError {
        let error = match self.reason {
            RejectReason::Internal => return OAuthBearerError::server_error(),
            RejectReason::AudienceMismatch => OAuthBearerError::invalid_token("token audience does not match"),
            RejectReason::CookieSource => {
                OAuthBearerError::invalid_token("access tokens are accepted only in the Authorization header")
            }
            _ => OAuthBearerError::invalid_token("the access token is invalid or has expired"),
        };
        match resource_metadata {
            Some(url) => error.with_resource_metadata(url),
            None => error,
        }
    }

    /// The `token_rejected` audit record for this failure (the caller adds address and agent).
    pub fn audit_record(&self) -> AuditRecord {
        let mut record =
            AuditRecord::new(AuditEvent::TokenRejected).with_detail("reason", DetailValue::Code(self.reason.as_str()));
        if let Some(id) = self.token_id {
            record = record.with_token(id);
        }
        if let Some(id) = self.grant_id {
            record = record.with_grant(id);
        }
        if let Some(id) = self.user_id {
            record = record.with_user(id);
        }
        if let Some(id) = self.workspace_id {
            record = record.with_workspace(id);
        }
        record
    }
}

/// An access token that passed steps 1 to 4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedToken {
    pub token_id: Uuid,
    pub grant_id: Uuid,
    pub client_id: Arc<str>,
    pub user_id: Uuid,
    pub user_email: Arc<str>,
    pub user_session_version: i64,
    pub workspace_id: Uuid,
    pub scopes: ScopeSet,
    pub projects: ProjectScope,
    pub resource: TokenResource,
    pub expires_at: Timestamp,
}

/// Step 1: a delegated token is accepted from the `Authorization` header only.
pub const fn check_source(source: TokenSource) -> Result<(), BearerFailure> {
    match source {
        TokenSource::Header => Ok(()),
        TokenSource::Cookie => Err(BearerFailure::bare(RejectReason::CookieSource)),
    }
}

/// Whether a query string carries an `access_token` parameter.
///
/// Every route refuses one with `400 invalid_request`, because tokens never travel in URLs. The key is compared after percent
/// decoding, so `access%5Ftoken` does not slip through.
pub fn query_carries_access_token(query: &str) -> bool {
    query.split('&').any(|pair| {
        let key = pair.split_once('=').map_or(pair, |(key, _)| key);
        percent_decode(key).as_deref() == Some("access_token")
    })
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'%' => {
                let pair = text.get(at + 1..at + 3)?;
                out.push(u8::from_str_radix(pair, 16).ok()?);
                at += 3;
            }
            b'+' => {
                out.push(b' ');
                at += 1;
            }
            other => {
                out.push(other);
                at += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[derive(Debug, FromQueryResult)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the fields are the columns of the lookup row"
)]
struct TokenRow {
    token_id: Uuid,
    grant_id: Uuid,
    client_id: String,
    resource: Option<String>,
    token_scopes: Vec<String>,
    token_expires_at: Timestamp,
    token_live: bool,
    token_revoked: bool,
    user_id: Uuid,
    workspace_id: Uuid,
    project_mode: String,
    grant_status: String,
    grant_live: bool,
    grant_session_version: i64,
    client_active: bool,
    user_active: bool,
    user_session_version: i64,
    user_email: String,
    is_member: bool,
    allows_apps: bool,
    project_ids: Option<Vec<Uuid>>,
}

/// The one lookup of step 2. Time is the database's (`now()`), so expiry does not depend on the
/// clock of the API process.
const LOOKUP_SQL: &str = "\
SELECT t.id AS token_id, t.grant_id, t.client_id, t.resource, t.scopes AS token_scopes, \
       t.expires_at AS token_expires_at, (t.expires_at > now()) AS token_live, \
       (t.revoked_at IS NOT NULL) AS token_revoked, \
       g.user_id, g.workspace_id, g.project_mode, g.status AS grant_status, \
       (g.expires_at > now()) AS grant_live, g.user_session_version AS grant_session_version, \
       (c.status = 'active') AS client_active, \
       u.is_active AS user_active, u.session_version AS user_session_version, u.email AS user_email, \
       (m.user_id IS NOT NULL) AS is_member, \
       COALESCE(s.allow_connected_apps, true) AS allows_apps, \
       CASE WHEN g.project_mode = 'selected' \
            THEN ARRAY(SELECT gp.project_id FROM oauth_grant_projects gp \
                       WHERE gp.grant_id = g.id ORDER BY gp.project_id) END AS project_ids \
FROM oauth_tokens t \
JOIN oauth_grants g ON g.id = t.grant_id \
JOIN oauth_clients c ON c.client_id = g.client_id \
JOIN users u ON u.id = g.user_id \
LEFT JOIN workspace_members m ON m.workspace_id = g.workspace_id AND m.user_id = g.user_id \
LEFT JOIN workspace_delegation_settings s ON s.workspace_id = g.workspace_id \
WHERE t.token_hash = $1 AND t.kind = 'access'";

/// The first failed live condition of step 3, in the order the checks are made.
fn live_state_failure(row: &TokenRow) -> Option<RejectReason> {
    if row.token_revoked {
        Some(RejectReason::TokenRevoked)
    } else if !row.token_live {
        Some(RejectReason::TokenExpired)
    } else if row.grant_status != "active" {
        Some(RejectReason::GrantNotActive)
    } else if !row.grant_live {
        Some(RejectReason::GrantExpired)
    } else if !row.client_active {
        Some(RejectReason::ClientDisabled)
    } else if !row.user_active {
        Some(RejectReason::UserInactive)
    } else if row.user_session_version != row.grant_session_version {
        Some(RejectReason::SessionVersionChanged)
    } else if !row.is_member {
        Some(RejectReason::MembershipMissing)
    } else if !row.allows_apps {
        Some(RejectReason::WorkspaceDisallows)
    } else {
        None
    }
}

/// Step 4: whether `resource` is acceptable to `rule`.
fn audience_accepted(rule: AudienceRule<'_>, resource: TokenResource) -> bool {
    match (rule, resource) {
        (AudienceRule::Api(_), TokenResource::Api) => true,
        (AudienceRule::Api(forward) | AudienceRule::McpForwarded(forward), TokenResource::Mcp) => forward.is_valid(),
        (AudienceRule::McpForwarded(_), TokenResource::Api) => false,
    }
}

const fn failure_for(row: &TokenRow, reason: RejectReason) -> BearerFailure {
    BearerFailure {
        reason,
        token_id: Some(row.token_id),
        grant_id: Some(row.grant_id),
        user_id: Some(row.user_id),
        workspace_id: Some(row.workspace_id),
    }
}

/// Ends the grant behind a token whose live state failed, on its own short transaction. A failure
/// here is logged and does not change the answer: the request is refused either way.
async fn revoke_for_state(db: &DatabaseConnection, row: &TokenRow, reason: RevokeReason) {
    let result = async {
        let tx = db.begin().await?;
        revoke_grant(&tx, row.grant_id, reason, Actor::System).await?;
        tx.commit().await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(
            grant_id = %row.grant_id,
            reason = reason.as_str(),
            error = %error,
            "could not revoke a grant whose live state failed; the token is still refused"
        );
    }
}

/// Validates a presented `sva_` access token. See the module documentation for the steps.
pub async fn validate_access_token(
    db: &DatabaseConnection,
    raw: &str,
    rule: AudienceRule<'_>,
) -> Result<ValidatedToken, BearerFailure> {
    if !is_well_formed(TokenKind::Access, raw) {
        return Err(BearerFailure::bare(RejectReason::Malformed));
    }
    let hash = hash_secret(raw);
    let row = TokenRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        LOOKUP_SQL,
        vec![hash.as_hex().into()],
    ))
    .one(db)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "delegated token lookup failed");
        BearerFailure::bare(RejectReason::Internal)
    })?
    .ok_or(BearerFailure::bare(RejectReason::UnknownToken))?;

    if let Some(reason) = live_state_failure(&row) {
        if let Some(revoke) = reason.revocation() {
            revoke_for_state(db, &row, revoke).await;
        }
        return Err(failure_for(&row, reason));
    }

    let internal = || failure_for(&row, RejectReason::Internal);
    let resource = row
        .resource
        .as_deref()
        .and_then(TokenResource::from_name)
        .ok_or_else(internal)?;
    if !audience_accepted(rule, resource) {
        return Err(failure_for(&row, RejectReason::AudienceMismatch));
    }
    let scopes = ScopeSet::from_names(row.token_scopes.iter().map(String::as_str)).map_err(|_| internal())?;
    let projects = match (row.project_mode.as_str(), row.project_ids.as_deref()) {
        ("all", _) => ProjectScope::All,
        ("selected", Some(ids)) => ProjectScope::Selected(Arc::from(ids)),
        _ => return Err(internal()),
    };
    Ok(ValidatedToken {
        token_id: row.token_id,
        grant_id: row.grant_id,
        client_id: Arc::from(row.client_id.as_str()),
        user_id: row.user_id,
        user_email: Arc::from(row.user_email.as_str()),
        user_session_version: row.user_session_version,
        workspace_id: row.workspace_id,
        scopes,
        projects,
        resource,
        expires_at: row.token_expires_at,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn secret() -> Secret {
        Secret::new("forward-secret-for-tests-0123456789abcdef")
    }

    #[test]
    fn forward_credential_requires_name_secret_and_configuration() {
        let configured = secret();
        let good = configured.expose().to_owned();
        let cred = |by, presented| ForwardCredential::new(Some(&configured), by, presented);
        assert!(cred(Some("mcp-server"), Some(&good)).is_valid());
        assert!(!cred(Some("mcp-server"), Some("wrong")).is_valid());
        assert!(!cred(Some("someone-else"), Some(&good)).is_valid());
        assert!(!cred(None, Some(&good)).is_valid());
        assert!(!cred(Some("mcp-server"), None).is_valid());
        assert!(!ForwardCredential::absent().is_valid());
        assert!(!ForwardCredential::new(None, Some("mcp-server"), Some(&good)).is_valid());
        let empty = Secret::new("");
        assert!(!ForwardCredential::new(Some(&empty), Some("mcp-server"), Some("")).is_valid());
    }

    #[test]
    fn forward_credential_debug_hides_both_values() {
        let configured = secret();
        let cred = ForwardCredential::new(Some(&configured), Some("mcp-server"), Some(configured.expose()));
        let shown = format!("{cred:?} {:?}", AudienceRule::Api(cred));
        assert!(!shown.contains(configured.expose()), "{shown}");
        assert!(shown.contains("valid: true"));
    }

    #[test]
    fn audience_matrix() {
        let configured = secret();
        let valid = ForwardCredential::new(Some(&configured), Some("mcp-server"), Some(configured.expose()));
        let invalid = ForwardCredential::absent();
        let table = [
            (AudienceRule::Api(invalid), TokenResource::Api, true),
            (AudienceRule::Api(valid), TokenResource::Api, true),
            (AudienceRule::Api(invalid), TokenResource::Mcp, false),
            (AudienceRule::Api(valid), TokenResource::Mcp, true),
            (AudienceRule::McpForwarded(valid), TokenResource::Mcp, true),
            (AudienceRule::McpForwarded(invalid), TokenResource::Mcp, false),
            (AudienceRule::McpForwarded(valid), TokenResource::Api, false),
            (AudienceRule::McpForwarded(invalid), TokenResource::Api, false),
        ];
        for (rule, resource, accepted) in table {
            assert_eq!(audience_accepted(rule, resource), accepted, "{rule:?} {resource:?}");
        }
    }

    #[test]
    fn cookies_are_refused_and_headers_pass() {
        assert!(check_source(TokenSource::Header).is_ok());
        let failure = check_source(TokenSource::Cookie).unwrap_err();
        assert_eq!(failure.reason, RejectReason::CookieSource);
        assert_eq!(
            failure.to_error(None).kind.status(),
            axum::http::StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn access_token_query_parameters_are_detected_after_decoding() {
        for query in [
            "access_token=x",
            "a=1&access_token=x",
            "access_token",
            "access%5Ftoken=x",
            "b=2&access%5ftoken=x",
        ] {
            assert!(query_carries_access_token(query), "{query}");
        }
        for query in [
            "",
            "a=1",
            "my_access_token=x",
            "access_tokens=x",
            "a=access_token",
            "access%zz=1",
        ] {
            assert!(!query_carries_access_token(query), "{query}");
        }
    }

    #[test]
    fn only_state_failures_that_name_a_cause_revoke_the_grant() {
        let expected = [
            (RejectReason::GrantExpired, Some(RevokeReason::Expired)),
            (RejectReason::ClientDisabled, Some(RevokeReason::ClientDisabled)),
            (RejectReason::UserInactive, Some(RevokeReason::AccountDisabled)),
            (RejectReason::SessionVersionChanged, Some(RevokeReason::PasswordChanged)),
            (RejectReason::MembershipMissing, Some(RevokeReason::MembershipRemoved)),
            (RejectReason::WorkspaceDisallows, Some(RevokeReason::WorkspaceDisabled)),
            (RejectReason::Malformed, None),
            (RejectReason::CookieSource, None),
            (RejectReason::UnknownToken, None),
            (RejectReason::TokenRevoked, None),
            (RejectReason::TokenExpired, None),
            (RejectReason::GrantNotActive, None),
            (RejectReason::AudienceMismatch, None),
            (RejectReason::Internal, None),
        ];
        for (reason, revoke) in expected {
            assert_eq!(reason.revocation(), revoke, "{reason:?}");
        }
    }

    #[test]
    fn responses_say_little() {
        let metadata = "https://sv.example.test/.well-known/oauth-protected-resource/api";
        for reason in [
            RejectReason::Malformed,
            RejectReason::UnknownToken,
            RejectReason::TokenRevoked,
            RejectReason::TokenExpired,
            RejectReason::GrantNotActive,
            RejectReason::GrantExpired,
            RejectReason::ClientDisabled,
            RejectReason::UserInactive,
            RejectReason::SessionVersionChanged,
            RejectReason::MembershipMissing,
            RejectReason::WorkspaceDisallows,
        ] {
            let error = BearerFailure::bare(reason).to_error(Some(metadata));
            assert_eq!(
                error.description, "the access token is invalid or has expired",
                "{reason:?}"
            );
            assert_eq!(error.kind, crate::oauth::error::BearerErrorKind::InvalidToken);
        }
        let audience = BearerFailure::bare(RejectReason::AudienceMismatch).to_error(None);
        assert_eq!(audience.description, "token audience does not match");
        let internal = BearerFailure::bare(RejectReason::Internal).to_error(Some(metadata));
        assert_eq!(internal.kind.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn rejection_audit_records_carry_ids_and_a_reason_only() {
        let (token, grant) = (Uuid::new_v4(), Uuid::new_v4());
        let failure = BearerFailure {
            reason: RejectReason::TokenExpired,
            token_id: Some(token),
            grant_id: Some(grant),
            user_id: None,
            workspace_id: None,
        };
        let record = failure.audit_record();
        assert_eq!(record.event, AuditEvent::TokenRejected);
        assert_eq!(record.token_id, Some(token));
        assert_eq!(record.grant_id, Some(grant));
        assert_eq!(
            record.detail().get("reason").and_then(|v| v.as_str()),
            Some("token_expired")
        );
        assert_eq!(record.detail().len(), 1);
        assert!(RejectReason::TokenExpired.is_audited());
        assert!(!RejectReason::Malformed.is_audited());
    }
}
