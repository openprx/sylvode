//! Work items, comments, labels, activity, personal views and the board.

use api::{middleware, routes};
use axum::{
    Router, middleware as axum_middleware,
    routing::{get, post},
};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        // Issue routes (protected)
        .route(
            "/api/v1/projects/{project_id}/issues",
            post(routes::issue::create_issue)
                .get(routes::issue::list_issues)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/issues/{id}",
            get(routes::issue::get_issue)
                .put(routes::issue::update_issue)
                .delete(routes::issue::delete_issue)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Comment routes (protected)
        .route(
            "/api/v1/issues/{issue_id}/comments",
            post(routes::comment::create_comment)
                .get(routes::comment::list_comments)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/comments/{id}",
            get(routes::comment::get_comment)
                .put(routes::comment::update_comment)
                .delete(routes::comment::delete_comment)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Label routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/labels",
            post(routes::label::create_label)
                .get(routes::label::list_labels)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/labels/{id}",
            axum::routing::put(routes::label::update_label)
                .delete(routes::label::delete_label)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // Issue label association routes (protected)
        .route(
            "/api/v1/issues/{issue_id}/labels/{label_id}",
            post(routes::label::add_label_to_issue)
                .delete(routes::label::remove_label_from_issue)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/issues/{issue_id}/labels",
            get(routes::label::get_issue_labels).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Bot-accessible routes (bot_or_user_auth_middleware)
        .route(
            "/api/v1/projects/{project_id}/labels",
            get(routes::label::list_project_labels).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/issues/by-identifier/{identifier}",
            get(routes::issue::get_issue_by_identifier).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/issues/{issue_id}/labels/batch",
            post(routes::issue::add_labels_to_issue).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Activity routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/activities",
            get(routes::activity::get_workspace_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/activities",
            get(routes::activity::get_project_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/issues/{issue_id}/activities",
            get(routes::activity::get_issue_activities).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // My routes (protected)
        .route(
            "/api/v1/my/issues",
            get(routes::my::get_my_issues).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/my/activities",
            get(routes::my::get_my_activities).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Board routes (protected)
        .route(
            "/api/v1/projects/{project_id}/board",
            get(routes::board::get_project_board).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/projects/{project_id}/workflow/effective",
            get(routes::workflow::get_effective_workflow_by_project).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
}
