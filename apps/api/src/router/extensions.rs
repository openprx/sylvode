//! Routes of the `extensions` area: installations of packages and their actions.
//!
//! Request, upgrade and preview accept a bot of the whole workspace as well as a person; every other
//! route sits behind the JWT middleware, which no bot or delegated token passes.

use api::{
    extensions::install::{handlers, invoke},
    middleware,
};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware as axum_middleware,
    routing::{get, post},
};
use platform::app::AppState;

/// Bodies of the installation routes are small JSON documents.
const BODY_LIMIT: usize = 256 * 1024;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/installations",
            post(handlers::request_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/installations",
            get(handlers::list_installations).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/installations/preview",
            post(handlers::preview_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}",
            get(handlers::get_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/generations",
            post(handlers::upgrade_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/review",
            get(handlers::review_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/approve",
            post(handlers::approve_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/reject",
            post(handlers::reject_generation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/disable",
            post(handlers::disable_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/enable",
            post(handlers::enable_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/revoke",
            post(handlers::revoke_installation).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/invocations",
            get(handlers::list_invocations).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/calls",
            get(handlers::list_calls).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/installations/{installation_id}/actions/{action}",
            post(invoke::invoke_action)
                .layer(DefaultBodyLimit::max(invoke::BODY_LIMIT))
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::jwt_auth_middleware,
                )),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}
