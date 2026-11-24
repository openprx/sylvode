//! Authentication, user administration, workspaces and members.

use api::{middleware, routes};
use axum::{
    Router, middleware as axum_middleware,
    routing::{delete, get, patch, post, put},
};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        // Auth routes
        .route("/api/v1/auth/setup-status", get(routes::auth::setup_status))
        .route("/api/v1/auth/register", post(routes::auth::register))
        .route("/api/v1/auth/login", post(routes::auth::login))
        .route("/api/v1/auth/refresh", post(routes::auth::refresh))
        .route(
            "/api/v1/auth/logout",
            post(routes::auth::logout).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me",
            get(routes::auth::me).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me/profile",
            put(routes::auth::update_profile).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me/password",
            put(routes::auth::update_password).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/auth/me/preferences",
            get(routes::auth::get_preferences)
                .put(routes::auth::update_preferences)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Workspace routes (protected)
        .route(
            "/api/v1/admin/users",
            post(routes::admin::create_user)
                .get(routes::admin::list_users)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                )),
        )
        .route(
            "/api/v1/admin/users/{id}",
            get(routes::admin::get_user)
                .put(routes::admin::update_user)
                .delete(routes::admin::delete_user)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                )),
        )
        .route(
            "/api/v1/admin/users/{id}/status",
            patch(routes::admin::toggle_user_status).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/admin/users/{id}/password",
            axum::routing::put(routes::admin::reset_user_password).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::admin::admin_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/admin/stats",
            get(routes::admin::get_stats).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::admin::admin_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces",
            post(routes::workspace::create_workspace)
                .get(routes::workspace::list_workspaces)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{id}",
            get(routes::workspace::get_workspace)
                .put(routes::workspace::update_workspace)
                .delete(routes::workspace::delete_workspace)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Workspace member routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/members",
            post(routes::member::add_member)
                .get(routes::member::list_members)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{user_id}",
            delete(routes::member::remove_member)
                .patch(routes::member::update_member_role)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/users",
            get(routes::member::search_users).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
}
