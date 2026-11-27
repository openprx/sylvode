//! Catalogue entries of the `alerts` router area (`router/alerts.rs`).

use crate::catalogue::types::dsl::{
    Admin, Delete, Entry, Find, Get, Patch, Post, Project, Put, Read, Secret, WorkspaceAdmin, bot,
};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("alerts.issue_alerts.read", Get, "/api/v1/issues/{id}/alert", Project(Find::Issue("id")), Read),
    bot("alerts.rules.create", Post, "/api/v1/projects/{project_id}/alert-rules", WorkspaceAdmin, Admin),
    bot("alerts.rules.delete", Delete, "/api/v1/alert-rules/{id}", WorkspaceAdmin, Admin),
    bot("alerts.rules.read", Get, "/api/v1/alert-rules/{id}", WorkspaceAdmin, Admin),
    bot("alerts.rules.read/list", Get, "/api/v1/projects/{project_id}/alert-rules", WorkspaceAdmin, Admin),
    bot("alerts.rules.test", Post, "/api/v1/projects/{project_id}/alert-rules/test", WorkspaceAdmin, Admin),
    bot("alerts.rules.update", Put, "/api/v1/alert-rules/{id}", WorkspaceAdmin, Admin),
    bot("alerts.rules.update/reorder", Post, "/api/v1/projects/{project_id}/alert-rules/reorder", WorkspaceAdmin, Admin),
    bot("alerts.sources.create", Post, "/api/v1/projects/{project_id}/inbound-sources", WorkspaceAdmin, Secret),
    bot("alerts.sources.delete", Delete, "/api/v1/inbound-sources/{id}", WorkspaceAdmin, Admin),
    bot("alerts.sources.read", Get, "/api/v1/inbound-sources/{id}", WorkspaceAdmin, Admin),
    bot("alerts.sources.read/events", Get, "/api/v1/inbound-sources/{id}/events", WorkspaceAdmin, Admin),
    bot("alerts.sources.read/list", Get, "/api/v1/projects/{project_id}/inbound-sources", WorkspaceAdmin, Admin),
    bot("alerts.sources.rotate", Post, "/api/v1/inbound-sources/{id}/rotate", WorkspaceAdmin, Secret),
    bot("alerts.sources.update", Patch, "/api/v1/inbound-sources/{id}", WorkspaceAdmin, Admin),
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
            BTreeMap::from([("admin", 14), ("project-resolved", 1)]),
            "the classes of the alerts routes changed"
        );
    }
}
