//! Routes of the `extensions` area.

use axum::Router;
use platform::app::AppState;

pub(super) fn routes(_auth_state: &AppState) -> Router<AppState> {
    Router::new()
}
