//! Runner bot route classes of the `issues` router area (`router/issues.rs`).

use super::{RunnerRoute, r, rc};
use crate::middleware::scoped_bot::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("DELETE", "/api/v1/labels/{id}", Class::Admin),
    r("PUT", "/api/v1/labels/{id}", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/labels", Class::Admin),
    r("GET", "/api/v1/my/activities", Class::Refused),
    r("GET", "/api/v1/my/issues", Class::Refused),
    r("GET", "/api/v1/workspaces/{workspace_id}/activities", Class::Refused),
    r("GET", "/api/v1/workspaces/{workspace_id}/labels", Class::Refused),
    rc("DELETE", "/api/v1/comments/{id}", Class::Project(Find::Comment("id"))),
    r("GET", "/api/v1/comments/{id}", Class::Project(Find::Comment("id"))),
    rc("PUT", "/api/v1/comments/{id}", Class::Project(Find::Comment("id"))),
    r("GET", "/api/v1/issues/by-identifier/{identifier}", Class::Project(Find::IssueIdentifier("identifier"))),
    r("DELETE", "/api/v1/issues/{id}", Class::Project(Find::Issue("id"))),
    r("GET", "/api/v1/issues/{id}", Class::Project(Find::Issue("id"))),
    r("PUT", "/api/v1/issues/{id}", Class::Project(Find::Issue("id"))),
    r("GET", "/api/v1/issues/{issue_id}/activities", Class::Project(Find::Issue("issue_id"))),
    r("GET", "/api/v1/issues/{issue_id}/comments", Class::Project(Find::Issue("issue_id"))),
    rc("POST", "/api/v1/issues/{issue_id}/comments", Class::Project(Find::Issue("issue_id"))),
    r("GET", "/api/v1/issues/{issue_id}/labels", Class::Project(Find::Issue("issue_id"))),
    r("POST", "/api/v1/issues/{issue_id}/labels/batch", Class::Project(Find::Issue("issue_id"))),
    r("DELETE", "/api/v1/issues/{issue_id}/labels/{label_id}", Class::Project(Find::Issue("issue_id"))),
    r("POST", "/api/v1/issues/{issue_id}/labels/{label_id}", Class::Project(Find::Issue("issue_id"))),
    r("GET", "/api/v1/projects/{project_id}/activities", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/board", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/issues", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/issues", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/labels", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/workflow/effective", Class::Project(Find::Project("project_id"))),
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
            BTreeMap::from([("admin", 3), ("project-resolved", 20), ("refused", 4)]),
            "the classes of the issues routes changed"
        );
    }
}
