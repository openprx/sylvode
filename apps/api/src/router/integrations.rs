//! Bots, webhooks, notifications, search, import, export and uploads.

use api::{middleware, routes};
use axum::{
    Router, middleware as axum_middleware,
    routing::{delete, get, patch, post},
};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        // Bot token management routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/bots",
            post(routes::bot::create_bot)
                .get(routes::bot::list_bots)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/bots/{bot_id}",
            axum::routing::delete(routes::bot::revoke_bot).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/bots/{bot_id}/project-access",
            get(routes::bot::get_project_access)
                .put(routes::bot::put_project_access)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/bot-operation-logs",
            get(routes::bot_operation_log::list_bot_operation_logs).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Webhook routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks",
            post(routes::webhook::create_webhook)
                .get(routes::webhook::list_webhooks)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}",
            get(routes::webhook::get_webhook)
                .patch(routes::webhook::update_webhook)
                .delete(routes::webhook::delete_webhook)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/rotate-secret",
            post(routes::webhook::rotate_webhook_secret).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries",
            get(routes::webhook::list_deliveries).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries/{delivery_id}",
            get(routes::webhook::get_delivery).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Notification routes (protected)
        .route(
            "/api/v1/notifications",
            get(routes::notification::list_notifications).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/unread-count",
            get(routes::notification::get_unread_count).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/notifications/{id}/read",
            patch(routes::notification::mark_notification_read)
                .put(routes::notification::mark_notification_read)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/notifications/read-all",
            patch(routes::notification::mark_all_read)
                .put(routes::notification::mark_all_read)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/notifications/{id}",
            delete(routes::notification::delete_notification).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Search route (protected)
        .route(
            "/api/v1/search",
            get(routes::search::search).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Export route (protected)
        .route(
            "/api/v1/projects/{project_id}/export",
            get(routes::export::export_project).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Import route (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/import/project",
            post(routes::import::import_project).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Upload route (protected)
        .route(
            "/api/v1/upload",
            post(routes::upload::upload_file).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
}
