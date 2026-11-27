//! Routes of the `catalogue` area: the capability ids and their operations.

use api::{catalogue::handler, middleware};
use axum::{Router, middleware as axum_middleware, routing::get};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new().route(
        "/api/v1/catalogue",
        get(handler::capabilities).route_layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            middleware::bot_auth::bot_or_user_auth_middleware,
        )),
    )
}
