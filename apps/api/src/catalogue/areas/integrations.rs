//! Catalogue entries of the `integrations` router area (`router/integrations.rs`).

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("integrations.bot_logs.read", Get, "/api/v1/workspaces/{workspace_id}/bot-operation-logs", WorkspaceAdmin, Admin),
    bot("integrations.bots.create", Post, "/api/v1/workspaces/{workspace_id}/bots", WorkspaceAdmin, Admin),
    bot("integrations.bots.delete", Delete, "/api/v1/workspaces/{workspace_id}/bots/{bot_id}", WorkspaceAdmin, Admin),
    bot("integrations.bots.read/list", Get, "/api/v1/workspaces/{workspace_id}/bots", WorkspaceAdmin, Admin),
    bot("integrations.bots.read/project_access", Get, "/api/v1/workspaces/{workspace_id}/bots/{bot_id}/project-access", WorkspaceAdmin, Admin),
    bot("integrations.bots.update/project_access", Put, "/api/v1/workspaces/{workspace_id}/bots/{bot_id}/project-access", WorkspaceAdmin, Admin),
    bot("integrations.notifications.delete", Delete, "/api/v1/notifications/{id}", Own, Destructive),
    bot("integrations.notifications.read/list", Get, "/api/v1/notifications", Own, Read).oauth(),
    bot("integrations.notifications.read/unread_count", Get, "/api/v1/notifications/unread-count", Own, Read).oauth(),
    bot("integrations.notifications.update/read", Patch, "/api/v1/notifications/{id}/read", Own, Write).oauth(),
    bot("integrations.notifications.update/read_all", Patch, "/api/v1/notifications/read-all", Own, Write).oauth(),
    bot("integrations.notifications.update/read_all_put", Put, "/api/v1/notifications/read-all", Own, Write).oauth(),
    bot("integrations.notifications.update/read_put", Put, "/api/v1/notifications/{id}/read", Own, Write).oauth(),
    bot("integrations.search.read", Get, "/api/v1/search", Filtered, Read),
    bot("integrations.transfer.export", Get, "/api/v1/projects/{project_id}/export", Project(Find::Project("project_id")), Write),
    bot("integrations.transfer.import", Post, "/api/v1/workspaces/{workspace_id}/import/project", WorkspaceAdmin, Admin),
    bot("integrations.uploads.create", Post, "/api/v1/upload", Own, Write),
    bot("integrations.webhooks.create", Post, "/api/v1/workspaces/{workspace_id}/webhooks", WorkspaceAdmin, Admin),
    bot("integrations.webhooks.delete", Delete, "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", WorkspaceAdmin, Admin),
    bot("integrations.webhooks.read", Get, "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", WorkspaceAdmin, Admin),
    bot("integrations.webhooks.read/deliveries", Get, "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries", WorkspaceAdmin, Admin),
    bot("integrations.webhooks.read/delivery", Get, "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries/{delivery_id}", WorkspaceAdmin, Admin),
    bot("integrations.webhooks.read/list", Get, "/api/v1/workspaces/{workspace_id}/webhooks", WorkspaceAdmin, Admin),
    bot("integrations.webhooks.rotate", Post, "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/rotate-secret", WorkspaceAdmin, Secret),
    bot("integrations.webhooks.update", Patch, "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", WorkspaceAdmin, Admin),
];

#[cfg(test)]
mod tests {
    use super::ENTRIES;
    use crate::catalogue::bot_class_counts;
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            bot_class_counts(ENTRIES),
            BTreeMap::from([
                ("admin", 15),
                ("filtered", 1),
                ("own resource", 8),
                ("project-resolved", 1)
            ]),
            "the classes of the integrations routes changed"
        );
    }
}
