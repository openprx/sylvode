//! The application router, assembled from one module per area.
//!
//! Each area module exposes `routes`, a method chain of `.route(..)` calls; this module merges
//! them and applies the global layers. The Flow surface is the exception: it is merged through
//! `routes::flow_surface::seal` and through nothing else.

use api::routes;
use axum::{Router, extract::DefaultBodyLimit, routing::get};
use platform::app::AppState;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

mod alerts;
mod auth_workspace;
mod flow;
mod forms;
mod governance;
mod integrations;
mod issues;
mod planning;
mod projects;
mod uploads;

const MAX_HTTP_REQUEST_BYTES: usize = 200 * 1024 * 1024;

/// The source text of every router file, for the tests that check the route registrations
/// (authentication layers, Flow boundary, bot route classes) against what the router declares.
#[cfg(test)]
pub(crate) const ROUTER_SOURCES: &[(&str, &str)] = &[
    ("mod.rs", include_str!("mod.rs")),
    ("alerts.rs", include_str!("alerts.rs")),
    ("auth_workspace.rs", include_str!("auth_workspace.rs")),
    ("flow.rs", include_str!("flow.rs")),
    ("forms.rs", include_str!("forms.rs")),
    ("governance.rs", include_str!("governance.rs")),
    ("integrations.rs", include_str!("integrations.rs")),
    ("issues.rs", include_str!("issues.rs")),
    ("planning.rs", include_str!("planning.rs")),
    ("projects.rs", include_str!("projects.rs")),
    ("uploads.rs", include_str!("uploads.rs")),
];

#[cfg(test)]
mod tests {
    use super::ROUTER_SOURCES;

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

/// The application router: every REST route, with the global layers applied.
pub(crate) fn build_router(state: AppState) -> Router {
    build_router_with(state, api::config::runtime().oauth.as_ref())
}

/// [`build_router`] with an explicit `[oauth]` configuration (tests; the server passes the
/// installed one).
pub(crate) fn build_router_with(
    state: AppState,
    oauth: Option<&std::sync::Arc<platform::config::OAuthConfig>>,
) -> Router {
    let auth_state = state.clone();

    Router::new()
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
        .layer(DefaultBodyLimit::max(MAX_HTTP_REQUEST_BYTES))
        // The custom span preserves the tower-http fields but records only the URI path. Query
        // strings carry both secrets (collab tickets) and user content (search/filter terms), so
        // sanitization is global rather than an allowlist of routes that will drift.
        .layer(TraceLayer::new_for_http().make_span_with(routes::collab::trace_span))
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .with_state(state)
}
