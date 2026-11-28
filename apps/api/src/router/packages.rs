//! Routes of the `packages` area: the package store and the workspace's publisher keys.
//!
//! Upload, delete and the publisher-key routes sit behind the JWT middleware only: a bot token or a
//! delegated token never reaches them. Listing and reading also admit a bot of the whole
//! workspace (a project-scoped bot is refused by its class in `bot_routes/packages.rs`).

use api::{middleware, packages};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware as axum_middleware,
    routing::{delete, get, post},
};
use platform::app::AppState;

/// Body limit of the upload route: the 32 MiB package plus multipart framing, not the global 200 MiB.
const UPLOAD_BODY_LIMIT: usize = 33 * 1024 * 1024;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/packages",
            post(packages::handlers::upload_package)
                .layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT))
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::jwt_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/packages",
            get(packages::handlers::list_packages).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/packages/{digest}",
            get(packages::handlers::get_package).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/packages/{digest}",
            delete(packages::handlers::delete_package).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/publisher-keys",
            get(packages::handlers::list_publisher_keys)
                .post(packages::handlers::register_publisher_key)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::jwt_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/publisher-keys/{key_id}/revoke",
            post(packages::handlers::revoke_publisher_key).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
}
