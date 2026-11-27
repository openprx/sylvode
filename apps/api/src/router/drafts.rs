//! Routes of the `drafts` area. Scaffolding only; the owning lane adds the routes.

use axum::Router;
use platform::app::AppState;

pub(super) fn routes(_auth_state: &AppState) -> Router<AppState> {
    Router::new()
}
