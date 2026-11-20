//! The OAuth routes of the API, merged into `build_router` with one line.
//!
//! Each later package adds one `.merge(<module>::routes(state))` line here and one `pub mod` line
//! in `mod.rs`.

use axum::Router;
use platform::app::AppState;

use super::{authorize, manage, metadata};

/// Routes that exist for every deployment. Without `[oauth] issuer` they answer 404.
pub fn routes(_state: &AppState) -> Router<AppState> {
    Router::new()
        .merge(metadata::routes(crate::config::runtime().oauth.clone()))
        .merge(authorize::routes(crate::config::runtime().oauth.as_ref()))
        .merge(manage::routes(crate::config::runtime().oauth.as_ref()))
}
