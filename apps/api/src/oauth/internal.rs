//! `GET /api/v1/oauth/internal/token-context`: what the MCP server asks about a caller's token.
//!
//! The MCP server holds neither a database nor a signing key, so it asks the API who an
//! MCP-audience token belongs to and what it covers. The call is not part of the public surface:
//!
//! - it answers `404` to anyone who does not present the internal forwarding credential
//!   (`X-Sylvode-Forwarded-By: mcp-server` and `X-Sylvode-Forward-Auth`, compared in constant time),
//!   whatever the method and whatever else the request carries, so the route does not exist for an
//!   ordinary client;
//! - with the credential it accepts MCP-audience tokens only, and runs the same validation as the
//!   delegated branch (steps 1 to 4): an API-audience token, a cookie, an expired or revoked token
//!   all fail with `401 invalid_token` and the bearer challenge;
//! - it returns identifiers and the scope set, never a secret, and is rate limited per grant.

use axum::Router;
use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use platform::app::AppState;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Instant;

use super::delegated::{CONTEXT_LIMITER, active_config, note_use};
use super::error::{OAuthBearerError, no_store_headers};
use super::metadata::api_resource_metadata_url;
use super::scopes::ProjectScope;
use super::tokens::TokenKind;
use super::validate::{
    AudienceRule, FORWARD_AUTH_HEADER, FORWARDED_BY_HEADER, ForwardCredential, TokenSource, check_source,
    query_carries_access_token, validate_access_token,
};
use crate::routes::auth::{extract_bearer_token, extract_cookie_token};

/// The route's path.
pub const TOKEN_CONTEXT_PATH: &str = "/api/v1/oauth/internal/token-context";

const INVALID_TOKEN_MESSAGE: &str = "the access token is invalid or has expired";

fn not_found() -> Response {
    let mut response = StatusCode::NOT_FOUND.into_response();
    for (name, value) in no_store_headers() {
        response.headers_mut().insert(name, value);
    }
    response
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

async fn token_context(axum::extract::State(state): axum::extract::State<AppState>, req: Request) -> Response {
    let Some(config) = active_config(req.extensions()) else {
        return not_found();
    };
    let credential = ForwardCredential::new(
        config.internal_forward_secret.as_ref(),
        header_text(req.headers(), FORWARDED_BY_HEADER),
        header_text(req.headers(), FORWARD_AUTH_HEADER),
    );
    if !credential.is_valid() {
        return not_found();
    }
    if req.method() != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let metadata_url = api_resource_metadata_url(&config);
    let refuse = |error: OAuthBearerError| {
        match metadata_url.as_deref() {
            Some(url) => error.with_resource_metadata(url),
            None => error,
        }
        .into_response()
    };

    if req.uri().query().is_some_and(query_carries_access_token) {
        return refuse(OAuthBearerError::invalid_request(
            "access tokens are accepted only in the Authorization header",
        ));
    }
    let (token, source) = match extract_bearer_token(req.headers()) {
        Some(token) => (token, TokenSource::Header),
        None => match extract_cookie_token(req.headers(), "access_token") {
            Some(token) => (token, TokenSource::Cookie),
            None => return refuse(OAuthBearerError::invalid_token("missing access token")),
        },
    };
    if let Err(failure) = check_source(source) {
        return refuse(failure.to_error(metadata_url.as_deref()));
    }
    if !token.starts_with(TokenKind::Access.prefix()) {
        return refuse(OAuthBearerError::invalid_token(INVALID_TOKEN_MESSAGE));
    }
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
    let validated = match validate_access_token(&state.db, &token, AudienceRule::McpForwarded(credential)).await {
        Ok(validated) => validated,
        Err(failure) => {
            if failure.reason.is_audited() {
                let mut record = failure
                    .audit_record()
                    .with_client_ip(super::rate::client_address(req.headers(), peer));
                if let Some(agent) = header_text(req.headers(), "user-agent") {
                    record = record.with_user_agent(agent);
                }
                if let Err(error) = super::audit::write_token_rejected_sampled(&state.db, &record).await {
                    tracing::warn!(error = %error, "could not write the token_rejected audit row");
                }
            }
            return refuse(failure.to_error(metadata_url.as_deref()));
        }
    };
    if let Err(limited) = CONTEXT_LIMITER.check(validated.grant_id, Instant::now()) {
        return refuse(OAuthBearerError::from(limited));
    }
    note_use(&state.db, validated.token_id, validated.grant_id);

    let (project_mode, project_ids) = match &validated.projects {
        ProjectScope::All => ("all", None),
        ProjectScope::Selected(ids) => ("selected", Some(ids.to_vec())),
    };
    let mut body = json!({
        "active": true,
        "client_id": validated.client_id.as_ref(),
        "sub": validated.user_id,
        "grant_id": validated.grant_id,
        "workspace_id": validated.workspace_id,
        "scope": validated.scopes.to_scope_string(),
        "project_mode": project_mode,
        "exp": validated.expires_at.unix_timestamp(),
    });
    // The project set is part of the answer for a grant limited to some projects, so the MCP
    // server can refuse a project argument outside it before calling the API.
    if let (Some(ids), Some(object)) = (project_ids, body.as_object_mut()) {
        object.insert("project_ids".to_string(), json!(ids));
    }
    json_ok(&body)
}

fn json_ok(value: &serde_json::Value) -> Response {
    let mut response = axum::Json(value).into_response();
    for (name, header_value) in no_store_headers() {
        response.headers_mut().insert(name, header_value);
    }
    response
}

/// The internal route. It is registered for every method so that an uncredentialed probe of any
/// method gets the same `404` as a path that does not exist.
pub fn routes() -> Router<AppState> {
    Router::new().route(TOKEN_CONTEXT_PATH, any(token_context))
}
