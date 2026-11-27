//! Routes of the `site` area: the console administration of sites (inside the global layers) and
//! the public site API `/api/site/v1/{slug}` with its own layer stack (merged by `mod.rs` after
//! the global layers, so neither the permissive CORS layer nor the 200 MiB body limit applies).

use api::{catalogue, middleware, site};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware as axum_middleware,
    routing::{get, patch, post, put},
};
use platform::app::AppState;
use tower_http::trace::TraceLayer;

pub(super) fn routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/sites",
            get(site::admin::list_sites).post(site::admin::create_site).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/sites/{id}",
            get(site::admin::get_site).patch(site::admin::update_site).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/sites/{id}",
            axum::routing::delete(site::admin::delete_site).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/sites/{id}/publish",
            post(site::admin::publish_site).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/sites/{id}/unpublish",
            post(site::admin::unpublish_site).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/sites/{id}/disable",
            post(site::admin::disable_site).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/sites/{id}/settings",
            put(site::admin::put_settings).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::auth::jwt_auth_middleware,
            )),
        )
        .route(
            "/api/v1/sites/{id}/pages",
            get(site::admin::list_pages).post(site::admin::create_page).route_layer(
                axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                ),
            ),
        )
        .route(
            "/api/v1/site-pages/{page_id}",
            patch(site::admin::update_page)
                .delete(site::admin::delete_page)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::bot_auth::bot_or_user_auth_middleware,
                )),
        )
        .route(
            "/api/v1/sites/{id}/bindings",
            get(site::admin::list_bindings).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/sites/{id}/bindings/{key}",
            put(site::admin::put_binding)
                .delete(site::admin::delete_binding)
                .route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    middleware::auth::jwt_auth_middleware,
                )),
        )
        .route(
            "/api/v1/sites/{id}/roles",
            get(site::admin::list_roles).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .route(
            "/api/v1/sites/{id}/roles/{user_id}",
            put(site::admin::put_role).delete(site::admin::delete_role).route_layer(
                axum_middleware::from_fn_with_state(auth_state.clone(), middleware::auth::jwt_auth_middleware),
            ),
        )
}

/// The public site API. No authentication layer: the site guard refuses every `Authorization`
/// header and the handlers read only the site cookies.
pub(super) fn public_routes(auth_state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/api/site/v1/{slug}/manifest", get(site::public::manifest))
        .route("/api/site/v1/{slug}/pages", get(site::public::page))
        .route(
            "/api/site/v1/{slug}/bindings/{binding}/fields",
            get(site::public::fields),
        )
        .route(
            "/api/site/v1/{slug}/bindings/{binding}/records",
            get(site::public::list_records).post(site::public::submit_record),
        )
        .route(
            "/api/site/v1/{slug}/bindings/{binding}/records/{id}",
            get(site::public::get_record).patch(site::public::update_record),
        )
        .route("/api/site/v1/{slug}/auth/operator/login", post(site::public::login))
        .route("/api/site/v1/{slug}/auth/logout", post(site::public::logout))
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            site::http::guard,
        ))
        .layer(axum_middleware::from_fn(catalogue::guard::guard))
        .layer(DefaultBodyLimit::max(site::http::MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http().make_span_with(api::routes::collab::trace_span))
}
