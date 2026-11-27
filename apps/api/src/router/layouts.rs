//! Routes of the `layouts` area: layouts, slots, placements and per-user placement preferences.

use api::{layouts, middleware};
use axum::{
    Router, middleware as axum_middleware,
    routing::{get, post, put},
};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/layouts",
            get(layouts::handlers::list_layouts)
                .post(layouts::handlers::create_layout)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/layouts/validate",
            post(layouts::handlers::validate_layout).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/layouts/components",
            get(layouts::handlers::list_components).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/layouts/{id}",
            get(layouts::handlers::get_layout)
                .patch(layouts::handlers::update_layout)
                .delete(layouts::handlers::delete_layout)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/slots",
            get(layouts::handlers::list_slots).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/slots/{slot_id}/placements",
            get(layouts::placements::resolve_slot).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/placements",
            post(layouts::placements::create_placement).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/placements/{id}",
            put(layouts::placements::update_placement)
                .delete(layouts::placements::delete_placement)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/me/placements/{id}/prefs",
            put(layouts::placements::put_prefs).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
}
