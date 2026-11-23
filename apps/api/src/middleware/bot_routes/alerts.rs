//! Runner bot route classes of the `alerts` router area (`router/alerts.rs`).

use super::{RunnerRoute, r};
use crate::middleware::runner_bot_scope::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("DELETE", "/api/v1/alert-rules/{id}", Class::Admin),
    r("GET", "/api/v1/alert-rules/{id}", Class::Admin),
    r("PUT", "/api/v1/alert-rules/{id}", Class::Admin),
    r("DELETE", "/api/v1/inbound-sources/{id}", Class::Admin),
    r("GET", "/api/v1/inbound-sources/{id}", Class::Admin),
    r("PATCH", "/api/v1/inbound-sources/{id}", Class::Admin),
    r("GET", "/api/v1/inbound-sources/{id}/events", Class::Admin),
    r("POST", "/api/v1/inbound-sources/{id}/rotate", Class::Admin),
    r("GET", "/api/v1/projects/{project_id}/alert-rules", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/alert-rules", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/alert-rules/reorder", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/alert-rules/test", Class::Admin),
    r("GET", "/api/v1/projects/{project_id}/inbound-sources", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/inbound-sources", Class::Admin),
    r("GET", "/api/v1/issues/{id}/alert", Class::Project(Find::Issue("id"))),
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
            BTreeMap::from([("admin", 14), ("project-resolved", 1)]),
            "the classes of the alerts routes changed"
        );
    }
}
