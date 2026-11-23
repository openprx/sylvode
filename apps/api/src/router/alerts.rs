//! Alert chain: the inbound event endpoint, sources and rules.

use api::{middleware, routes};
use axum::{
    Router, middleware as axum_middleware,
    routing::{get, post},
};
use platform::app::AppState;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        // Alert chain: the inbound event endpoint. It carries its own layer, which accepts a source
        // credential only (never a session, a bot token or a delegated token) and applies the
        // rate limits and the body limit; it is deliberately not behind the layers below.
        .route(
            api::inbound::INBOUND_EVENTS_PATH,
            post(routes::inbound_events::post_event).route_layer(axum_middleware::from_fn_with_state(
                api::inbound::auth_state(auth_state, api::config::runtime().inbound),
                api::inbound::auth::inbound_source_auth,
            )),
        )
        // Alert chain management: sources and rules, for workspace owners and admins only (each
        // handler checks the role and refuses bots; none of these routes is delegable).
        .route(
            "/api/v1/projects/{project_id}/inbound-sources",
            get(routes::inbound_admin::list_sources).post(routes::inbound_admin::create_source)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/inbound-sources/{id}",
            get(routes::inbound_admin::get_source)
                .patch(routes::inbound_admin::patch_source)
                .delete(routes::inbound_admin::delete_source)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/inbound-sources/{id}/rotate",
            post(routes::inbound_admin::rotate_source)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/inbound-sources/{id}/events",
            get(routes::inbound_admin::list_source_events)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/alert-rules",
            get(routes::inbound_admin::list_rules).post(routes::inbound_admin::create_rule)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/alert-rules/reorder",
            post(routes::inbound_admin::reorder_rules)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/projects/{project_id}/alert-rules/test",
            post(routes::inbound_admin::test_rules)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        // The read-only alert facts of a work item: any workspace member, not delegable.
        .route(
            "/api/v1/issues/{id}/alert",
            get(routes::inbound_events::get_issue_alert)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/alert-rules/{id}",
            get(routes::inbound_admin::get_rule)
                .put(routes::inbound_admin::replace_rule)
                .delete(routes::inbound_admin::delete_rule)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
}
