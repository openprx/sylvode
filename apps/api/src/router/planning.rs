//! Workflows and sprints.

use api::{middleware, routes};
use axum::{
    Router, middleware as axum_middleware,
    routing::{get, post, put},
};
use platform::app::AppState;

pub(crate) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        // Workflow CRUD routes (protected)
        .route(
            "/api/v1/workspaces/{workspace_id}/workflows",
            post(routes::workflow::create_workflow)
                .get(routes::workflow::list_workflows)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workflows/{workflow_id}",
            get(routes::workflow::get_workflow)
                .put(routes::workflow::update_workflow)
                .delete(routes::workflow::delete_workflow)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workflows/{workflow_id}/states",
            get(routes::workflow::list_workflow_states)
                .post(routes::workflow::create_workflow_state)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/workflows/{workflow_id}/states/reorder",
            put(routes::workflow::reorder_workflow_states).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/workflow-states/{state_id}",
            put(routes::workflow::update_workflow_state)
                .delete(routes::workflow::delete_workflow_state)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/workflow",
            put(routes::workflow::set_project_workflow).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        // Sprint routes (protected)
        .route(
            "/api/v1/projects/{project_id}/sprints",
            post(routes::sprint::create_sprint)
                .get(routes::sprint::list_sprints)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/sprints/{id}",
            axum::routing::get(routes::sprint::get_sprint)
                .put(routes::sprint::update_sprint)
                .delete(routes::sprint::delete_sprint)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
}
