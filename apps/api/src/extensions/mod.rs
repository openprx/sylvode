//! Extension host v4: plugin invocation (contract C1 §3-§5).
//!
//! * [`host`]: the WebAssembly host. Runs a component under fuel, epoch, memory, host-call and
//!   process-wide limits and links only the interfaces a grant set can use.
//! * [`dispatch`]: a host call becomes an in-process request to the API router, so a plugin reaches
//!   exactly the handlers a user reaches, under the same checks.
//! * [`resource`]: resolves the target of an operation for the catalogue checker.
//! * This file: the marker that tells handlers a request comes from a plugin ([`current`]), the
//!   [`InternalCall`] request extension and its authentication ([`internal_claims`]).
//!
//! Installations and persistent grants arrive with the next batch; until then the caller of
//! [`host::invoke`] passes the grants in.

use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, MatchedPath, RawPathParams, Request},
    middleware::Next,
    response::Response,
};
use platform::{
    app::AppState,
    auth::{JwtClaims, TokenType},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

use crate::catalogue::{Principal, Refusal, RefusalCode, allowed_pre, allowed_target, by_route};
use crate::error::ApiError;

pub mod dispatch;
pub mod host;
pub mod install;
pub mod resource;
pub mod rules;

/// What a handler may learn about a plugin call; set by the internal-call branch of the
/// authentication middleware around the handler, and read through [`current`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginCall {
    pub installation_id: Uuid,
    pub generation: i32,
    /// `user` in the MVP.
    pub actor_kind: &'static str,
    pub actor_id: Uuid,
}

tokio::task_local! {
    static CURRENT: PluginCall;
}

/// The plugin call the current request belongs to, if any. A task-local: handlers rebuild their
/// own request extensions, so an extension marker would be lost on the way.
#[must_use]
pub fn current() -> Option<PluginCall> {
    CURRENT.try_with(|call| *call).ok()
}

/// Runs `future` as a request of the plugin call `call`.
pub async fn scope<F: std::future::Future>(call: PluginCall, future: F) -> F::Output {
    CURRENT.scope(call, future).await
}

/// The request extension the host puts on a request it dispatches. Extensions are set by server
/// code only and never arrive from the wire, so a request from outside never carries it.
#[derive(Clone, Debug)]
pub struct InternalCall {
    /// The operation the host checked; the guard refuses a route whose entry has another `op`.
    pub op: &'static str,
    /// The principal as the host built it; the authentication branch re-checks it.
    pub principal: Arc<Principal>,
    /// The invoking user.
    pub actor: Uuid,
    /// `users.session_version` of the actor when the invocation started (contract §3, N-2).
    pub session_version: i64,
    pub installation_id: Uuid,
    pub generation: i32,
    /// 1 for a call from a guest; a call made while handling one is refused.
    pub depth: u8,
    pub request_id: Uuid,
}

impl InternalCall {
    #[must_use]
    pub const fn marker(&self) -> PluginCall {
        PluginCall {
            installation_id: self.installation_id,
            generation: self.generation,
            actor_kind: "user",
            actor_id: self.actor,
        }
    }
}

/// The claims of an internal call: the invoking user, live. A user who is gone or deactivated, or
/// whose sessions were revoked since the invocation started (logout-all, password reset), or who is
/// not a person (a bot or AI user), is refused `403 forbidden` and the invocation ends.
///
/// # Errors
/// `Forbidden` as above; a database failure.
pub async fn internal_claims(state: &AppState, call: &InternalCall) -> Result<JwtClaims, ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT email, is_active, session_version, entity_type FROM users WHERE id = $1",
            vec![call.actor.into()],
        ))
        .await
        .map_err(ApiError::internal_database_failure)?;
    let forbidden = || ApiError::Forbidden("forbidden".to_string());
    let row = row.ok_or_else(forbidden)?;
    let email: String = row.try_get("", "email").map_err(|_| ApiError::Internal)?;
    let active: bool = row.try_get("", "is_active").map_err(|_| ApiError::Internal)?;
    let version: i64 = row.try_get("", "session_version").map_err(|_| ApiError::Internal)?;
    let entity_type: String = row.try_get("", "entity_type").map_err(|_| ApiError::Internal)?;
    // Only a person acts through a plugin: a bot or AI user would run with plain user claims and
    // skip the bot context's project scoping.
    if !active || entity_type != "human" || version != call.session_version {
        return Err(forbidden());
    }
    Ok(JwtClaims {
        sub: call.actor.to_string(),
        email,
        token_type: TokenType::Access,
        iat: 0,
        exp: 0,
        sv: version,
    })
}

/// The authentication branch of an internal call, shared by the two authentication middlewares: no
/// token is parsed. The call is checked once more against the entry of the matched route and the
/// resolved target, the invoking user is loaded live, and the handler runs with that user's claims
/// and the plugin marker ([`current`]).
///
/// # Errors
/// `plugin_reentry` for a depth other than 1, a catalogue refusal when the principal is not
/// allowed, `Forbidden` when the user is gone or the session changed.
pub async fn handle_internal(
    state: &AppState,
    request: Request,
    next: Next,
    call: InternalCall,
) -> Result<Response, ApiError> {
    if call.depth != 1 {
        return Err(Refusal::new(RefusalCode::PluginReentry, None).into());
    }
    let (mut parts, body) = request.into_parts();
    let not_exposed = || ApiError::from(Refusal::new(RefusalCode::CapabilityNotExposed, None));
    let entry = parts
        .extensions
        .get::<MatchedPath>()
        .and_then(|matched| by_route(parts.method.as_str(), matched.as_str()))
        .filter(|entry| entry.op == call.op)
        .ok_or_else(not_exposed)?;
    allowed_pre(&call.principal, entry)?;
    let raw = RawPathParams::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| not_exposed())?;
    let params: Vec<(String, String)> = raw
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let resource = resource::resolve_resource(&state.db, entry, &params).await?;
    let Some(resource) = resource else {
        return Err(Refusal::new(RefusalCode::CapabilityScopeMismatch, Some(entry.capability())).into());
    };
    allowed_target(&call.principal, entry, &resource)?;
    let claims = internal_claims(state, &call).await?;
    let mut request = Request::from_parts(parts, body);
    request.extensions_mut().insert(claims);
    Ok(scope(call.marker(), next.run(request)).await)
}
