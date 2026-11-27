//! Routes of the `nav` area: the resolved menus, the navigation documents and the per-person
//! preferences.

use api::{middleware, nav};
use axum::{Router, middleware as axum_middleware, routing::get};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/nav",
            get(nav::handlers::get_workspace_menu).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/nav/config",
            get(nav::handlers::get_workspace_config)
                .put(nav::handlers::put_workspace_config)
                .delete(nav::handlers::delete_workspace_config)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/nav",
            get(nav::handlers::get_project_menu).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/projects/{project_id}/nav/config",
            get(nav::handlers::get_project_config)
                .put(nav::handlers::put_project_config)
                .delete(nav::handlers::delete_project_config)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/me/nav-prefs",
            get(nav::handlers::get_prefs)
                .put(nav::handlers::put_prefs)
                .delete(nav::handlers::delete_prefs)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::jwt_auth_middleware,
                )),
        )
}
