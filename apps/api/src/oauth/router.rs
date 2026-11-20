//! The OAuth routes of the API, merged into `build_router` with one line.
//!
//! Each later package adds one `.merge(<module>::routes(state))` line here and one `pub mod` line
//! in `mod.rs`.

use axum::Router;
use platform::app::AppState;
use platform::config::OAuthConfig;
use std::sync::Arc;

use super::{authorize, delegated, manage, metadata, stream};

/// Routes that exist for every deployment. Without `[oauth] issuer` they answer 404.
pub fn routes(state: &AppState) -> Router<AppState> {
    routes_with(state, crate::config::runtime().oauth.as_ref())
}

/// The same routes for an explicit `[oauth]` configuration, so a test can build the router the
/// server runs without installing process-wide state.
pub fn routes_with(state: &AppState, config: Option<&Arc<OAuthConfig>>) -> Router<AppState> {
    Router::new()
        .merge(metadata::routes(config.cloned()))
        .merge(authorize::routes(config))
        .merge(delegated::routes(state))
        .merge(manage::routes(config))
        .merge(stream::routes(config))
}
