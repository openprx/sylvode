//! Catalogue entries of the `planning` router area (`router/planning.rs`).

use crate::catalogue::types::dsl::{
    Admin, Delete, Destructive, Entry, Find, Get, Post, Project, Put, Read, Workspace, WorkspaceAdmin, Write, bot,
};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("planning.milestones.create", Post, "/api/v1/workspaces/{workspace_id}/milestones", Workspace, Write),
    bot("planning.milestones.delete", Delete, "/api/v1/milestones/{id}", Workspace, Destructive),
    bot("planning.milestones.read", Get, "/api/v1/milestones/{id}", Workspace, Read),
    bot("planning.milestones.read/list", Get, "/api/v1/workspaces/{workspace_id}/milestones", Workspace, Read),
    bot("planning.milestones.update", Put, "/api/v1/milestones/{id}", Workspace, Write),
    bot("planning.sprints.create", Post, "/api/v1/projects/{project_id}/sprints", Project(Find::Project("project_id")), Write),
    bot("planning.sprints.delete", Delete, "/api/v1/sprints/{id}", Project(Find::Table("sprints","id")), Destructive),
    bot("planning.sprints.read", Get, "/api/v1/sprints/{id}", Project(Find::Table("sprints","id")), Read),
    bot("planning.sprints.read/list", Get, "/api/v1/projects/{project_id}/sprints", Project(Find::Project("project_id")), Read),
    bot("planning.sprints.update", Put, "/api/v1/sprints/{id}", Project(Find::Table("sprints","id")), Write),
    bot("planning.workflows.assign", Put, "/api/v1/projects/{project_id}/workflow", Project(Find::Project("project_id")), Admin),
    bot("planning.workflows.create", Post, "/api/v1/workspaces/{workspace_id}/workflows", WorkspaceAdmin, Admin),
    bot("planning.workflows.delete", Delete, "/api/v1/workflows/{workflow_id}", WorkspaceAdmin, Admin),
    bot("planning.workflows.read", Get, "/api/v1/workflows/{workflow_id}", WorkspaceAdmin, Admin),
    bot("planning.workflows.read/list", Get, "/api/v1/workspaces/{workspace_id}/workflows", WorkspaceAdmin, Admin),
    bot("planning.workflows.read/states", Get, "/api/v1/workflows/{workflow_id}/states", WorkspaceAdmin, Admin),
    bot("planning.workflows.update", Put, "/api/v1/workflows/{workflow_id}", WorkspaceAdmin, Admin),
    bot("planning.workflows.update/reorder", Put, "/api/v1/workflows/{workflow_id}/states/reorder", WorkspaceAdmin, Admin),
    bot("planning.workflows.update/state", Put, "/api/v1/workflow-states/{state_id}", WorkspaceAdmin, Admin),
    bot("planning.workflows.update/state_add", Post, "/api/v1/workflows/{workflow_id}/states", WorkspaceAdmin, Admin),
    bot("planning.workflows.update/state_remove", Delete, "/api/v1/workflow-states/{state_id}", WorkspaceAdmin, Admin),
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
            BTreeMap::from([("admin", 10), ("project-resolved", 6), ("refused", 5)]),
            "the classes of the planning routes changed"
        );
    }
}
