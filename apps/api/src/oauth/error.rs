//! Error types of the OAuth surface.
//!
//! The rest of the API answers every failure with HTTP 200 and a `{code, message}` envelope
//! (`crate::error`). OAuth clients depend on real status codes, on the RFC 6749 error body and on
//! the RFC 6750 `WWW-Authenticate` challenge, so the OAuth endpoints and the delegated bearer
//! branch have their own types.
//!
//! Descriptions are `&'static str` on purpose: a message can not carry a token, a code or any
//! other request value, because it can not be built from one.

use axum::{
    Json,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::scopes::ScopeSet;

/// The `realm` of every bearer challenge.
pub const REALM: &str = "sylvode";

/// Error codes of the protocol endpoints (OAuth 2.1 section 3.2.4, RFC 7009, RFC 8707).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthErrorKind {
    InvalidRequest,
    InvalidClient,
    InvalidGrant,
    UnauthorizedClient,
    UnsupportedGrantType,
    InvalidScope,
    InvalidTarget,
    UnsupportedTokenType,
    RateLimited,
    ServerError,
    /// The feature is not configured: the endpoint does not exist.
    NotFound,
}

impl OAuthErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidClient => "invalid_client",
            Self::InvalidGrant => "invalid_grant",
            Self::UnauthorizedClient => "unauthorized_client",
            Self::UnsupportedGrantType => "unsupported_grant_type",
            Self::InvalidScope => "invalid_scope",
            Self::InvalidTarget => "invalid_target",
            Self::UnsupportedTokenType => "unsupported_token_type",
            Self::RateLimited => "rate_limited",
            Self::ServerError => "server_error",
            Self::NotFound => "not_found",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::InvalidClient => StatusCode::UNAUTHORIZED,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::ServerError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::InvalidRequest
            | Self::InvalidGrant
            | Self::UnauthorizedClient
            | Self::UnsupportedGrantType
            | Self::InvalidScope
            | Self::InvalidTarget
            | Self::UnsupportedTokenType => StatusCode::BAD_REQUEST,
        }
    }
}

/// An error of a protocol endpoint: real status, `{"error", "error_description"}` body,
/// `Cache-Control: no-store`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthError {
    pub kind: OAuthErrorKind,
    pub description: &'static str,
    pub retry_after_secs: Option<u64>,
}

impl OAuthError {
    pub const fn new(kind: OAuthErrorKind, description: &'static str) -> Self {
        Self {
            kind,
            description,
            retry_after_secs: None,
        }
    }

    pub const fn rate_limited(retry_after_secs: u64) -> Self {
        Self {
            kind: OAuthErrorKind::RateLimited,
            description: "too many requests",
            retry_after_secs: Some(retry_after_secs),
        }
    }

    pub const fn server_error() -> Self {
        Self::new(OAuthErrorKind::ServerError, "the server could not complete the request")
    }

    pub const fn not_found() -> Self {
        Self::new(OAuthErrorKind::NotFound, "not found")
    }
}

/// Headers every OAuth response carries (RFC 6749 section 5.1).
pub const fn no_store_headers() -> [(HeaderName, HeaderValue); 2] {
    [
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (header::PRAGMA, HeaderValue::from_static("no-cache")),
    ]
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let mut response = (
            self.kind.status(),
            Json(json!({
                "error": self.kind.as_str(),
                "error_description": self.description,
            })),
        )
            .into_response();
        let headers = response.headers_mut();
        for (name, value) in no_store_headers() {
            headers.insert(name, value);
        }
        if let Some(seconds) = self.retry_after_secs
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            headers.insert(header::RETRY_AFTER, value);
        }
        response
    }
}

/// How a delegated bearer request failed (RFC 6750 section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BearerErrorKind {
    InvalidRequest,
    InvalidToken,
    InsufficientScope,
    /// A valid token used outside what the grant covers (another workspace, another project).
    /// RFC 6750 defines no error code for it, so no `WWW-Authenticate` challenge is sent.
    Forbidden,
    RateLimited,
    ServerError,
}

impl BearerErrorKind {
    pub const fn error_code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidToken => "invalid_token",
            Self::InsufficientScope => "insufficient_scope",
            Self::Forbidden => "forbidden",
            Self::RateLimited => "rate_limited",
            Self::ServerError => "server_error",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::InvalidToken => StatusCode::UNAUTHORIZED,
            Self::InsufficientScope | Self::Forbidden => StatusCode::FORBIDDEN,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::ServerError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    const fn has_challenge(self) -> bool {
        matches!(
            self,
            Self::InvalidRequest | Self::InvalidToken | Self::InsufficientScope
        )
    }
}

/// A failure of the delegated bearer branch.
///
/// The body keeps the fields of the API's usual
/// envelope (`code`, `message`, `error_code`) next to the OAuth `error`, and the status and the
/// challenge are the real ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthBearerError {
    pub kind: BearerErrorKind,
    pub description: &'static str,
    /// The full set a call needs, named in an `insufficient_scope` challenge.
    pub scope: Option<ScopeSet>,
    /// URL of the protected resource metadata document, named in the challenge.
    pub resource_metadata: Option<String>,
    pub retry_after_secs: Option<u64>,
}

impl OAuthBearerError {
    const fn of(kind: BearerErrorKind, description: &'static str) -> Self {
        Self {
            kind,
            description,
            scope: None,
            resource_metadata: None,
            retry_after_secs: None,
        }
    }

    pub const fn invalid_request(description: &'static str) -> Self {
        Self::of(BearerErrorKind::InvalidRequest, description)
    }

    pub const fn invalid_token(description: &'static str) -> Self {
        Self::of(BearerErrorKind::InvalidToken, description)
    }

    /// `required` is the full set the call needs, hierarchy included; `None` for a route that no
    /// scope opens.
    pub fn insufficient_scope(description: &'static str, required: Option<ScopeSet>) -> Self {
        Self {
            scope: required,
            ..Self::of(BearerErrorKind::InsufficientScope, description)
        }
    }

    pub const fn forbidden(description: &'static str) -> Self {
        Self::of(BearerErrorKind::Forbidden, description)
    }

    pub fn rate_limited(retry_after_secs: u64) -> Self {
        Self {
            retry_after_secs: Some(retry_after_secs),
            ..Self::of(BearerErrorKind::RateLimited, "too many requests")
        }
    }

    pub const fn server_error() -> Self {
        Self::of(
            BearerErrorKind::ServerError,
            "the server could not complete the request",
        )
    }

    #[must_use]
    pub fn with_resource_metadata(mut self, url: impl Into<String>) -> Self {
        self.resource_metadata = Some(url.into());
        self
    }

    /// The `WWW-Authenticate` value, for the kinds RFC 6750 defines a challenge for.
    pub fn www_authenticate(&self) -> Option<String> {
        if !self.kind.has_challenge() {
            return None;
        }
        let mut value = format!(
            "Bearer realm=\"{REALM}\", error=\"{}\", error_description=\"{}\"",
            self.kind.error_code(),
            quote(self.description)
        );
        if let Some(scope) = self.scope.filter(|scope| !scope.is_empty()) {
            value.push_str(", scope=\"");
            value.push_str(&scope.to_scope_string());
            value.push('"');
        }
        if let Some(url) = &self.resource_metadata {
            value.push_str(", resource_metadata=\"");
            value.push_str(&quote(url));
            value.push('"');
        }
        Some(value)
    }
}

/// Escapes the two characters that end or escape a quoted string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

impl IntoResponse for OAuthBearerError {
    fn into_response(self) -> Response {
        let status = self.kind.status();
        let mut response = (
            status,
            Json(json!({
                "code": status.as_u16(),
                "message": self.description,
                "error_code": self.kind.error_code(),
                "error": self.kind.error_code(),
            })),
        )
            .into_response();
        let headers: &mut HeaderMap = response.headers_mut();
        for (name, value) in no_store_headers() {
            headers.insert(name, value);
        }
        if let Some(challenge) = self.www_authenticate()
            && let Ok(value) = HeaderValue::from_str(&challenge)
        {
            headers.insert(header::WWW_AUTHENTICATE, value);
        }
        if let Some(seconds) = self.retry_after_secs
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            headers.insert(header::RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::oauth::scopes::Scope;
    use axum::body::to_bytes;

    async fn body(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[test]
    fn protocol_errors_use_the_status_codes_of_the_standards() {
        let table = [
            (OAuthErrorKind::InvalidRequest, "invalid_request", 400),
            (OAuthErrorKind::InvalidClient, "invalid_client", 401),
            (OAuthErrorKind::InvalidGrant, "invalid_grant", 400),
            (OAuthErrorKind::UnauthorizedClient, "unauthorized_client", 400),
            (OAuthErrorKind::UnsupportedGrantType, "unsupported_grant_type", 400),
            (OAuthErrorKind::InvalidScope, "invalid_scope", 400),
            (OAuthErrorKind::InvalidTarget, "invalid_target", 400),
            (OAuthErrorKind::UnsupportedTokenType, "unsupported_token_type", 400),
            (OAuthErrorKind::RateLimited, "rate_limited", 429),
            (OAuthErrorKind::ServerError, "server_error", 500),
            (OAuthErrorKind::NotFound, "not_found", 404),
        ];
        for (kind, code, status) in table {
            assert_eq!((kind.as_str(), kind.status().as_u16()), (code, status));
        }
    }

    #[tokio::test]
    async fn a_protocol_error_response_has_the_rfc_body_and_no_store_headers() {
        let response = OAuthError::new(OAuthErrorKind::InvalidGrant, "the grant is invalid").into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::PRAGMA], "no-cache");
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
        assert_eq!(
            body(response).await,
            serde_json::json!({"error": "invalid_grant", "error_description": "the grant is invalid"})
        );

        let limited = OAuthError::rate_limited(42).into_response();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()[header::RETRY_AFTER], "42");
        assert_eq!(limited.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            OAuthError::server_error().into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(OAuthError::not_found().into_response().status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_invalid_token_challenge_carries_realm_error_and_resource_metadata() {
        let url = "https://sv.example.test/.well-known/oauth-protected-resource/api";
        let response = OAuthBearerError::invalid_token("the access token is invalid or has expired")
            .with_resource_metadata(url)
            .into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[header::WWW_AUTHENTICATE],
            format!(
                "Bearer realm=\"sylvode\", error=\"invalid_token\", \
                 error_description=\"the access token is invalid or has expired\", resource_metadata=\"{url}\""
            )
            .as_str()
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            body(response).await,
            serde_json::json!({
                "code": 401,
                "message": "the access token is invalid or has expired",
                "error_code": "invalid_token",
                "error": "invalid_token"
            })
        );
    }

    #[tokio::test]
    async fn an_insufficient_scope_challenge_names_the_full_scope_set() {
        let required = ScopeSet::challenge_for(Scope::WorkItemsWrite);
        let response = OAuthBearerError::insufficient_scope("the grant does not cover this call", Some(required))
            .with_resource_metadata("https://sv.example.test/.well-known/oauth-protected-resource/api")
            .into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let challenge = response.headers()[header::WWW_AUTHENTICATE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(
            challenge.starts_with("Bearer realm=\"sylvode\", error=\"insufficient_scope\""),
            "{challenge}"
        );
        assert!(
            challenge.contains("scope=\"work_items:read work_items:write\""),
            "{challenge}"
        );
        assert!(challenge.contains("resource_metadata=\""), "{challenge}");
        assert_eq!(body(response).await["error_code"], "insufficient_scope");

        // A route that no scope opens has no scope attribute.
        let none = OAuthBearerError::insufficient_scope("route not available to connected applications", None);
        assert!(!none.www_authenticate().unwrap().contains("scope="));
        let empty = OAuthBearerError::insufficient_scope("x", Some(ScopeSet::EMPTY));
        assert!(!empty.www_authenticate().unwrap().contains("scope="));
    }

    #[tokio::test]
    async fn other_bearer_failures() {
        let invalid_request = OAuthBearerError::invalid_request("access tokens are not accepted in the query string");
        let response = invalid_request.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            response.headers()[header::WWW_AUTHENTICATE]
                .to_str()
                .unwrap()
                .contains("error=\"invalid_request\"")
        );

        // A valid token outside the grant: 403 without a Bearer challenge.
        let forbidden = OAuthBearerError::forbidden("outside the connected workspace").into_response();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        assert!(forbidden.headers().get(header::WWW_AUTHENTICATE).is_none());
        assert_eq!(body(forbidden).await["error_code"], "forbidden");

        let limited = OAuthBearerError::rate_limited(9).into_response();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()[header::RETRY_AFTER], "9");
        assert!(limited.headers().get(header::WWW_AUTHENTICATE).is_none());

        let internal = OAuthBearerError::server_error().into_response();
        assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(internal.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    #[test]
    fn challenge_values_are_escaped() {
        assert_eq!(quote("a\"b\\c\nd"), "a\\\"b\\\\cd");
        let error = OAuthBearerError::invalid_token("x").with_resource_metadata("https://h/\"quoted\"");
        let challenge = error.www_authenticate().expect("challenge");
        assert!(
            challenge.contains("resource_metadata=\"https://h/\\\"quoted\\\"\""),
            "{challenge}"
        );
    }
}
