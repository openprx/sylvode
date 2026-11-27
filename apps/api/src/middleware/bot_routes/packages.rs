//! Runner bot route classes of the `packages` router area (`router/packages.rs`).
//!
//! Only the two read routes are behind the bot authentication. A project-scoped bot belongs to
//! projects, not to the workspace-wide package store, so both are `Refused` for it; a bot of the
//! whole workspace passes and the handler checks its workspace and `read` permission. Upload and
//! the publisher-key routes are behind the JWT middleware, which no bot token passes, so they need
//! no class.

use super::{RunnerRoute, r};
use crate::middleware::scoped_bot::Class;

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("GET", "/api/v1/workspaces/{workspace_id}/packages", Class::Refused),
    r("GET", "/api/v1/workspaces/{workspace_id}/packages/{digest}", Class::Refused),
];

#[cfg(test)]
mod tests {
    use super::ROUTES;
    use crate::middleware::bot_routes::class_counts;
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            class_counts(ROUTES),
            BTreeMap::from([("refused", 2)]),
            "the classes of the packages routes changed"
        );
    }
}
