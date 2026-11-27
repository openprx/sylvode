//! Routes of the `extensions` area.

use api::{extensions::dev, middleware};
use axum::{Router, extract::DefaultBodyLimit, middleware as axum_middleware, routing::post};
use platform::app::AppState;

/// The largest body of the dev route: a component of at most 8 MiB, base64 coded, plus the request.
const DEV_INVOKE_BODY_LIMIT: usize = 12 * 1024 * 1024;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    let router = Router::new();
    // Temporary: exercises the host before installations exist; removed by the installations
    // batch. Registered only when `[extensions] dev_invoke_enabled` is true (default false).
    if api::config::runtime().extensions.dev_invoke_enabled {
        router.route(
            "/api/v1/admin/extensions/dev-invoke",
            post(dev::dev_invoke)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                ))
                .layer(DefaultBodyLimit::max(DEV_INVOKE_BODY_LIMIT)),
        )
    } else {
        router
    }
}
