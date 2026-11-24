//! Runner bot route classes of the `planning` router area (`router/planning.rs`).

use super::{RunnerRoute, r};
use crate::middleware::runner_bot_scope::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("DELETE", "/api/v1/workflow-states/{state_id}", Class::Admin),
    r("PUT", "/api/v1/workflow-states/{state_id}", Class::Admin),
    r("DELETE", "/api/v1/workflows/{workflow_id}", Class::Admin),
    r("GET", "/api/v1/workflows/{workflow_id}", Class::Admin),
    r("PUT", "/api/v1/workflows/{workflow_id}", Class::Admin),
    r("GET", "/api/v1/workflows/{workflow_id}/states", Class::Admin),
    r("POST", "/api/v1/workflows/{workflow_id}/states", Class::Admin),
    r("PUT", "/api/v1/workflows/{workflow_id}/states/reorder", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/workflows", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/workflows", Class::Admin),
    r("GET", "/api/v1/projects/{project_id}/sprints", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/sprints", Class::Project(Find::Project("project_id"))),
    r("PUT", "/api/v1/projects/{project_id}/workflow", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/sprints/{id}", Class::Project(Find::Table("sprints","id"))),
    r("DELETE", "/api/v1/sprints/{id}", Class::Project(Find::Table("sprints","id"))),
    r("PUT", "/api/v1/sprints/{id}", Class::Project(Find::Table("sprints","id"))),
    r("GET", "/api/v1/workspaces/{workspace_id}/milestones", Class::Refused),
    r("POST", "/api/v1/workspaces/{workspace_id}/milestones", Class::Refused),
    r("DELETE", "/api/v1/milestones/{id}", Class::Refused),
    r("GET", "/api/v1/milestones/{id}", Class::Refused),
    r("PUT", "/api/v1/milestones/{id}", Class::Refused),
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
            BTreeMap::from([("admin", 10), ("project-resolved", 6), ("refused", 5)]),
            "the classes of the planning routes changed"
        );
    }
}
