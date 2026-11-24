//! Runner bot route classes of the `integrations` router area (`router/integrations.rs`).

use super::{RunnerRoute, r};
use crate::middleware::scoped_bot::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("GET", "/api/v1/notifications", Class::Own),
    r("PATCH", "/api/v1/notifications/read-all", Class::Own),
    r("PUT", "/api/v1/notifications/read-all", Class::Own),
    r("GET", "/api/v1/notifications/unread-count", Class::Own),
    r("DELETE", "/api/v1/notifications/{id}", Class::Own),
    r("PATCH", "/api/v1/notifications/{id}/read", Class::Own),
    r("PUT", "/api/v1/notifications/{id}/read", Class::Own),
    r("POST", "/api/v1/upload", Class::Own),
    r("GET", "/api/v1/workspaces/{workspace_id}/bot-operation-logs", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/bots", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/bots", Class::Admin),
    r("DELETE", "/api/v1/workspaces/{workspace_id}/bots/{bot_id}", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/bots/{bot_id}/project-access", Class::Admin),
    r("PUT", "/api/v1/workspaces/{workspace_id}/bots/{bot_id}/project-access", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/import/project", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/webhooks", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/webhooks", Class::Admin),
    r("DELETE", "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", Class::Admin),
    r("PATCH", "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries/{delivery_id}", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/rotate-secret", Class::Admin),
    r("GET", "/api/v1/projects/{project_id}/export", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/search", Class::Project(Find::Query("project_id"))),
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
            BTreeMap::from([("admin", 15), ("own resource", 8), ("project-resolved", 2)]),
            "the classes of the integrations routes changed"
        );
    }
}
