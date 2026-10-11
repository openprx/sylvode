//! The response boundary of the Flow REST surface.
//!
//! The Flow surface is every REST route served by a `routes::flow` or `routes::collab` handler,
//! the admin Flow routes included. `error-mapping-v1.md` requires each of its error responses to
//! carry a stable `error_code`, and a database failure propagated with `?` has none: it renders
//! as the legacy envelope (`code` 500, `message` "database error"). [`seal`] is the one place
//! that changes this, and it changes it for the Flow surface only.

use axum::{Router, extract::Request, middleware, middleware::Next, response::Response};
use platform::app::AppState;

use crate::error::with_typed_database_failures;

/// Marks a response that was produced behind the Flow surface boundary.
///
/// Inserted into the extensions of every response [`seal`] lets out, whatever its status, so a
/// test can tell from a response alone whether its route is behind the boundary. Never serialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowSurfaceResponse;

/// Puts `routes` behind the Flow surface boundary.
///
/// While one of these routes serves a request, a database failure that reaches the response is
/// rendered as a typed envelope: `server_rejected` with
/// `details.reason = "deterministic_database_refusal"` when the database refused the statement
/// for good, `server_draining` with `reason = "contention"` and `retry_after_ms` otherwise. The
/// driver's text is logged and never sent. Routes that are not passed through here keep the
/// legacy envelope byte for byte.
///
/// The layer wraps every route of `routes`, including each route's own authentication layer, so
/// a handler added to the router that is sealed cannot answer outside it. Nothing inside a
/// handler takes part: retry loops still see the driver error and classify it themselves.
pub fn seal(routes: Router<AppState>) -> Router<AppState> {
    routes.layer(middleware::from_fn(boundary))
}

async fn boundary(request: Request, next: Next) -> Response {
    let mut response = with_typed_database_failures(next.run(request)).await;
    response.extensions_mut().insert(FlowSurfaceResponse);
    response
}
