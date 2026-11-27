//! Runner bot route classes of the `projects` router area (`router/projects.rs`).

use super::{RunnerRoute, r};
use crate::middleware::scoped_bot::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("GET", "/api/v1/ai/protocol", Class::NoProject),
    r("PUT", "/api/v1/ai/runner/report", Class::NoProject),
    r("GET", "/api/v1/bot/change-hints", Class::Own),
    r("GET", "/api/v1/ai/tasks/active", Class::NoProject),
    r("GET", "/api/v1/ai/tasks/{task_id}", Class::NoProject),
    r("POST", "/api/v1/ai/tasks/{task_id}/ack", Class::NoProject),
    r("POST", "/api/v1/ai/tasks/{task_id}/complete", Class::NoProject),
    r("POST", "/api/v1/ai/tasks/{task_id}/fail", Class::NoProject),
    r("POST", "/api/v1/ai/tasks/{task_id}/progress", Class::NoProject),
    r("GET", "/api/v1/project-types", Class::NoProject),
    r("GET", "/api/v1/project-types/{key}", Class::NoProject),
    r("GET", "/api/v1/scenario-templates", Class::NoProject),
    r("GET", "/api/v1/scenario-templates/{key}", Class::NoProject),
    r("PATCH", "/api/v1/project-types/{key}", Class::Admin),
    r("DELETE", "/api/v1/projects/{id}", Class::Admin),
    r("GET", "/api/v1/projects/{project_id}/bot-connections", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/bot-connections", Class::Admin),
    r("DELETE", "/api/v1/projects/{project_id}/bot-connections/{bot_id}", Class::Admin),
    r("GET", "/api/v1/projects/{project_id}/bot-connections/{bot_id}", Class::Admin),
    r("PATCH", "/api/v1/projects/{project_id}/bot-connections/{bot_id}", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/bot-connections/{bot_id}/disable", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/bot-connections/{bot_id}/enable", Class::Admin),
    r("POST", "/api/v1/projects/{project_id}/bot-connections/{bot_id}/rotate", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/project-types", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/project-types", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/projects", Class::Admin),
    r("GET", "/api/v1/decision-domains", Class::Refused),
    r("GET", "/api/v1/trust-scores", Class::Refused),
    r("GET", "/api/v1/trust-scores/{user_id}", Class::Refused),
    r("GET", "/api/v1/trust-scores/{user_id}/history", Class::Refused),
    r("GET", "/api/v1/trust-scores/{user_id}/{domain}", Class::Refused),
    r("GET", "/api/v1/users/{id}/trust", Class::Refused),
    r("GET", "/api/v1/workspaces/{workspace_id}/projects", Class::Filtered),
    r("GET", "/api/v1/check-results/{check_result_id}", Class::Project(Find::Table("check_results","check_result_id"))),
    r("POST", "/api/v1/check-results/{check_result_id}/proposal", Class::Project(Find::Table("check_results","check_result_id"))),
    r("GET", "/api/v1/issues/{issue_id}/ai-tasks", Class::Project(Find::Issue("issue_id"))),
    r("GET", "/api/v1/plugins/{plugin_id}", Class::Project(Find::Table("plugins","plugin_id"))),
    r("PATCH", "/api/v1/plugins/{plugin_id}", Class::Project(Find::Table("plugins","plugin_id"))),
    r("GET", "/api/v1/plugins/{plugin_id}/invocations", Class::Project(Find::Table("plugins","plugin_id"))),
    r("POST", "/api/v1/plugins/{plugin_id}/invoke", Class::Project(Find::Table("plugins","plugin_id"))),
    r("GET", "/api/v1/projects/{id}", Class::Project(Find::Project("id"))),
    r("PUT", "/api/v1/projects/{id}", Class::Project(Find::Project("id"))),
    r("GET", "/api/v1/projects/{project_id}/agent-policy", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/ai/tasks", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/ai/tasks", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/check-results", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/check-results", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/context", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/domains", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/domains", Class::Project(Find::Project("project_id"))),
    r("DELETE", "/api/v1/projects/{project_id}/domains/{id}", Class::Project(Find::Project("project_id"))),
    r("PATCH", "/api/v1/projects/{project_id}/domains/{id}", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/domains/{id}/members", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/governance-context", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/plugins", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/plugins", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/release-readiness", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/resources", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/resources", Class::Project(Find::Project("project_id"))),
    r("DELETE", "/api/v1/projects/{project_id}/resources/{resource_id}", Class::Project(Find::Project("project_id"))),
    r("PATCH", "/api/v1/projects/{project_id}/resources/{resource_id}", Class::Project(Find::Project("project_id"))),
    r("POST", "/api/v1/projects/{project_id}/scenario-templates/{template_key}/install", Class::Project(Find::Project("project_id"))),
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
                ("admin", 13),
                ("no project", 12),
                ("own resource", 1),
                ("filtered", 1),
                ("project-resolved", 29),
                ("refused", 6)
            ]),
            "the classes of the projects routes changed"
        );
    }
}
