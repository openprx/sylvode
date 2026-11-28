//! Routes of the `packages` area: the package store and the workspace's publisher keys.
//!
//! Upload, delete and the publisher-key routes sit behind the JWT middleware only: a bot token or a
//! delegated token never reaches them. A bot's package submission (`package-uploads`) creates a
//! draft, never a package. Listing and reading also admit a bot of the whole
//! workspace (a project-scoped bot is refused by its class in `bot_routes/packages.rs`).

use api::{middleware, packages, packages::index};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware as axum_middleware,
    routing::{delete, get, post, put},
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
        // A bot submits a signed package as a draft for a person to approve; it creates no package.
        .route(
            "/api/v1/workspaces/{workspace_id}/package-uploads",
            post(packages::handlers::submit_package_upload)
                .layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT))
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
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
        // The search of the package store (contract C3 4.4), for members and bots of the whole workspace.
        .route(
            "/api/v1/workspaces/{workspace_id}/market/search",
            get(packages::search::search_packages).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // The public plugin index (contract M15): reads from the cache, for members and bots of the
        // whole workspace; the image route is for a console session only.
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/sources",
            get(index::handlers::list_sources).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/plugins",
            get(index::handlers::list_plugins).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/plugins/{source}/{publisher}/{name}",
            get(index::handlers::get_plugin).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/assets/{source}/{publisher}/{name}/{*asset_path}",
            get(index::handlers::get_asset).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        // Import, trust, refresh, hide and the audit log: people with a console session only.
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/import",
            post(index::import::import_package).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/sources/{source}/refresh",
            post(index::admin::refresh_now).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/sources/{source}/hidden",
            put(index::admin::set_hidden).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/market/index/events",
            get(index::admin::list_events).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
}
