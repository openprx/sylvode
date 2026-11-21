//! The alert chain's inbound side.
//!
//! A security product posts a small JSON envelope with a per-project source credential, Sylvode
//! stores it once, and a rule may turn it into an alert work item and one bot task.
//!
//! The pieces, in the order a request meets them:
//!
//! * [`auth`]: the one middleware of `POST /api/v1/inbound/events`. It accepts the `svi_`
//!   credential and nothing else, applies the failed-authentication and per-source rate limits,
//!   reads the body under a size limit and checks the optional HMAC signature;
//! * [`envelope`]: the strict `sylvode.inbound_event.v1` validation;
//! * [`ingest`]: the transaction that stores the event, deduplicates it into an alert and, under
//!   the safety caps, hands one task to a bot through `DispatchOrigin::InboundRule`;
//! * [`rules`], [`description`], [`notify`]: rule matching, the work item text (SV-built title,
//!   untrusted text inside a fenced block) and the notifications of the alert's milestones;
//! * [`credentials`]: minting of source tokens and signing secrets.
//!
//! The management endpoints for sources and rules live in `routes::inbound_admin`.

use axum::middleware as axum_middleware;
use axum::{Router, routing::post};
use platform::app::AppState;
use platform::config::InboundConfig;

pub mod audit;
pub mod auth;
pub mod credentials;
pub mod description;
pub mod envelope;
pub mod ingest;
pub mod notify;
pub mod response;
pub mod rules;
pub mod severity;

/// The path of the inbound endpoint.
pub const INBOUND_EVENTS_PATH: &str = "/api/v1/inbound/events";

/// The inbound route with the configuration installed for this process.
pub fn routes(state: &AppState) -> Router<AppState> {
    routes_with(state, crate::config::runtime().inbound)
}

/// The inbound route for an explicit `[inbound]` configuration, so a test can build exactly the
/// router the server runs without installing process-wide state.
///
/// The route carries its own authentication layer. It is deliberately not behind
/// `bot_or_user_auth_middleware`: the endpoint accepts a source credential only, never a session,
/// a bot token or a delegated token.
pub fn routes_with(state: &AppState, config: InboundConfig) -> Router<AppState> {
    let auth_state = auth::InboundAuthState {
        app: state.clone(),
        config,
    };
    Router::new().route(
        INBOUND_EVENTS_PATH,
        post(ingest::post_event).route_layer(axum_middleware::from_fn_with_state(
            auth_state,
            auth::inbound_source_auth,
        )),
    )
}

#[cfg(test)]
mod db_tests;
