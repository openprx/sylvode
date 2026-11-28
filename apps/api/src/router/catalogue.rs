//! Routes of the `catalogue` area: the capability ids and their operations, and the `OpenAPI` view of them.

use api::{
    catalogue::{handler, openapi},
    middleware,
};
use axum::{Router, middleware as axum_middleware, routing::get};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/catalogue",
            get(handler::capabilities).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/openapi.json",
            get(openapi::serve).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
}
