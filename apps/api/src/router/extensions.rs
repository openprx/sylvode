//! Routes of the `extensions` area.

use api::{extensions::dev, middleware};
use axum::{Router, middleware as axum_middleware, routing::post};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    // Temporary: exercises the host before installations exist; removed by the installations batch.
    Router::new().route(
        "/api/v1/admin/extensions/dev-invoke",
        post(dev::dev_invoke).route_layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            middleware::admin::admin_middleware,
        )),
    )
}
