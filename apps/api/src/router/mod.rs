//! The application router, assembled from one module per area.
//!
//! Each area module exposes `routes`, a method chain of `.route(..)` calls; this module merges
//! them and applies the global layers. The Flow surface is the exception: it is merged through
//! `routes::flow_surface::seal` and through nothing else.

use api::routes;
use axum::{Router, extract::DefaultBodyLimit, middleware as axum_middleware, routing::get};
use platform::app::AppState;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

mod alerts;
mod auth_workspace;
mod catalogue;
mod drafts;
mod extensions;
mod flow;
mod forms;
mod governance;
mod integrations;
mod issues;
mod layouts;
mod nav;
mod packages;
mod planning;
mod projects;
mod site;
mod uploads;

const MAX_HTTP_REQUEST_BYTES: usize = 200 * 1024 * 1024;

/// The source text of every router file: the boot check of the catalogue scans it for the route
/// registrations, and tests check the Flow boundary against it.
pub const ROUTER_SOURCES: &[(&str, &str)] = &[
    ("mod.rs", include_str!("mod.rs")),
    ("alerts.rs", include_str!("alerts.rs")),
    ("auth_workspace.rs", include_str!("auth_workspace.rs")),
    ("catalogue.rs", include_str!("catalogue.rs")),
    ("drafts.rs", include_str!("drafts.rs")),
    ("extensions.rs", include_str!("extensions.rs")),
    ("flow.rs", include_str!("flow.rs")),
    ("forms.rs", include_str!("forms.rs")),
    ("governance.rs", include_str!("governance.rs")),
    ("integrations.rs", include_str!("integrations.rs")),
    ("issues.rs", include_str!("issues.rs")),
    ("layouts.rs", include_str!("layouts.rs")),
    ("nav.rs", include_str!("nav.rs")),
    ("packages.rs", include_str!("packages.rs")),
    ("planning.rs", include_str!("planning.rs")),
    ("projects.rs", include_str!("projects.rs")),
    ("site.rs", include_str!("site.rs")),
    ("uploads.rs", include_str!("uploads.rs")),
];

/// The application router: every REST route, with the global layers applied.
pub fn build_router(state: AppState) -> Router {
    build_router_with(state, api::config::runtime().oauth.as_ref())
}

/// [`build_router`] with an explicit `[oauth]` configuration (tests; the server passes the
/// installed one).
pub fn build_router_with(state: AppState, oauth: Option<&std::sync::Arc<platform::config::OAuthConfig>>) -> Router {
    let auth_state = state.clone();

    let router = Router::new()
        .route("/health", get(crate::health))
        .route("/ready", get(crate::ready))
        .merge(uploads::routes(&auth_state))
        .merge(auth_workspace::routes(&auth_state))
        .merge(projects::routes(&auth_state))
        .merge(forms::routes(&auth_state))
        .merge(issues::routes(&auth_state))
        .merge(governance::routes(&auth_state))
        // The Flow surface: every `routes::flow` / `routes::collab` route, behind the one boundary
        // that renders its database failures as typed envelopes.
        .merge(routes::flow_surface::seal(flow::routes(&auth_state)))
        // OAuth authorization server: metadata documents and, with later packages, the endpoints.
        .merge(api::oauth::router::routes_with(&auth_state, oauth))
        .merge(alerts::routes(&auth_state))
        .merge(planning::routes(&auth_state))
        .merge(integrations::routes(&auth_state))
        .merge(catalogue::routes(&auth_state))
        .merge(drafts::routes(&auth_state))
        .merge(extensions::routes(&auth_state))
        .merge(layouts::routes(&auth_state))
        .merge(nav::routes(&auth_state))
        .merge(packages::routes(&auth_state))
        .merge(site::routes(&auth_state))
        // Defence in depth for the catalogue: a matched route with no entry is refused.
        .layer(axum_middleware::from_fn(api::catalogue::guard::guard))
        .layer(DefaultBodyLimit::max(MAX_HTTP_REQUEST_BYTES))
        // The custom span preserves the tower-http fields but records only the URI path. Query
        // strings carry both secrets (collab tickets) and user content (search/filter terms), so
        // sanitization is global rather than an allowlist of routes that will drift.
        .layer(TraceLayer::new_for_http().make_span_with(routes::collab::trace_span))
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        // The public site API brings its own layer stack (contract C3 section 6.4): merged after
        // the global layers, so neither the permissive CORS nor the 200 MiB limit reaches it.
        .merge(site::public_routes(&auth_state));
    let router = match site::static_routes() {
        Some(static_routes) => router.merge(static_routes),
        None => router,
    };
    router.with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use tower::ServiceExt;

    use platform::{
        app::{AppState, FlowPermissionCacheSlot},
        config::{AppConfig, Secret},
    };
    use sea_orm::DatabaseConnection;

    use super::{ROUTER_SOURCES, build_router};

    /// A state without a database: the requests below never reach one.
    fn disconnected_state() -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "router-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("a-router-test-signing-secret-of-some-length"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db: DatabaseConnection::Disconnected,
            flow_permission_cache: FlowPermissionCacheSlot::default(),
        }
    }

    /// The site guard runs only under `/api/site/v1/{slug}/...`: an unknown console path with a
    /// bearer token is the standard 404, never `site_credentials_refused`.
    #[tokio::test]
    async fn an_unknown_console_path_is_not_answered_by_the_site_guard() {
        let router = build_router(disconnected_state());
        for (method, path) in [
            (
                Method::POST,
                "/api/v1/forms/00000000-0000-0000-0000-000000000001/archive",
            ),
            (Method::GET, "/api/v1/no-such-route"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(path)
                        .header(header::AUTHORIZATION, "Bearer not-a-token")
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("router answers");
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("body reads");
            let text = String::from_utf8_lossy(&body);
            assert!(!text.contains("site_credentials_refused"), "{method} {path}: {text}");
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {text}");
        }
    }

    /// A router file left out of [`ROUTER_SOURCES`] would escape every source-level route check.
    #[test]
    fn every_router_file_is_scanned() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/router");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("router directory is readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = ROUTER_SOURCES.iter().map(|(name, _)| (*name).to_string()).collect();
        listed.sort();
        assert_eq!(listed, on_disk, "ROUTER_SOURCES must list every file of src/router");
    }
}
