//! Runner bot route classes of the `layouts` router area (`router/layouts.rs`).
//!
//! The layout and placement rows carry a polymorphic scope (workspace, project or form), not a
//! `project_id` column, so the middleware cannot resolve their project. Those handlers decide for
//! a scoped bot themselves (`layouts::access`): reads need `read` and writes `write` in the
//! scope's project through `scoped_bot::ensure_bot_level`, and a workspace-scoped row is reachable
//! only by a bot that is not limited to projects. The preference route is for people only.

use super::{RunnerRoute, r};
use crate::middleware::scoped_bot::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("GET", "/api/v1/layouts", Class::Filtered),
    r("POST", "/api/v1/layouts", Class::NoProject),
    r("POST", "/api/v1/layouts/validate", Class::NoProject),
    r("GET", "/api/v1/layouts/components", Class::NoProject),
    r("DELETE", "/api/v1/layouts/{id}", Class::NoProject),
    r("GET", "/api/v1/layouts/{id}", Class::NoProject),
    r("PATCH", "/api/v1/layouts/{id}", Class::NoProject),
    r("GET", "/api/v1/slots", Class::NoProject),
    r("GET", "/api/v1/slots/{slot_id}/placements", Class::Project(Find::Query("project_id"))),
    r("POST", "/api/v1/placements", Class::NoProject),
    r("DELETE", "/api/v1/placements/{id}", Class::NoProject),
    r("PUT", "/api/v1/placements/{id}", Class::NoProject),
    r("PUT", "/api/v1/me/placements/{id}/prefs", Class::Refused),
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
            BTreeMap::from([
                ("filtered", 1),
                ("no project", 10),
                ("project-resolved", 1),
                ("refused", 1)
            ]),
            "the classes of the layouts routes changed"
        );
    }
}
