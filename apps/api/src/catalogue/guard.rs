//! Defence in depth: a request whose route has no catalogue entry is refused.
//!
//! After a clean boot check this cannot happen; the layer closes the gap if a route is ever
//! registered by a path the check does not see.

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::refusal::{Refusal, RefusalCode};
use super::uncatalogued::is_uncatalogued;
use crate::error::ApiError;

/// A request that matched a route with no entry for any method, and that is not one of the
/// `UNCATALOGUED` OAuth routes, is refused `capability_unclassified`. A request that matched no
/// route (the fallback answers 404, so no refusal reveals which routes exist) and a method the path
/// does not carry (answered `405` by the router) pass through. `HEAD` is the router's own answer
/// to a `GET` route and needs no entry of its own.
///
/// The re-check of an internal call against the entry of its operation arrives with the plugin host.
pub async fn guard(request: Request, next: Next) -> Response {
    let classified = request
        .extensions()
        .get::<MatchedPath>()
        .is_none_or(|matched| super::knows_path(matched.as_str()) || is_uncatalogued(matched.as_str()));
    if classified {
        next.run(request).await
    } else {
        ApiError::from(Refusal::new(RefusalCode::CapabilityUnclassified, None)).into_response()
    }
}
