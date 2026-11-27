//! Routes of the `drafts` area: agents create and read, people approve and reject.

use api::{drafts, middleware};
use axum::{
    Router, middleware as axum_middleware,
    routing::{get, post},
};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/drafts",
            get(drafts::handlers::list_drafts)
                .post(drafts::handlers::create_draft)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/drafts/{id}",
            get(drafts::handlers::get_draft).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        // Review is for user sessions only: a bot token or a connected application cannot reach it.
        .route(
            "/api/v1/drafts/{id}/approve",
            post(drafts::handlers::approve_draft).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/drafts/{id}/reject",
            post(drafts::handlers::reject_draft).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
}
