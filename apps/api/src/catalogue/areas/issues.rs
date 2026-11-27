//! Catalogue entries of the `issues` router area (`router/issues.rs`).

use crate::catalogue::types::dsl::{
    Admin, Delete, Destructive, Entry, Filtered, Find, Get, Post, Project, Put, Read, Workspace, WorkspaceAdmin, Write,
    bot,
};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("work_items.comments.create", Post, "/api/v1/issues/{issue_id}/comments", Project(Find::Issue("issue_id")), Write).comment().oauth().plugin(),
    bot("work_items.comments.delete", Delete, "/api/v1/comments/{id}", Project(Find::Comment("id")), Destructive).comment(),
    bot("work_items.comments.read", Get, "/api/v1/comments/{id}", Project(Find::Comment("id")), Read).plugin(),
    bot("work_items.comments.read/list", Get, "/api/v1/issues/{issue_id}/comments", Project(Find::Issue("issue_id")), Read).oauth().plugin(),
    bot("work_items.comments.update", Put, "/api/v1/comments/{id}", Project(Find::Comment("id")), Write).comment(),
    bot("work_items.items.create", Post, "/api/v1/projects/{project_id}/issues", Project(Find::Project("project_id")), Write).oauth().plugin(),
    bot("work_items.items.delete", Delete, "/api/v1/issues/{id}", Project(Find::Issue("id")), Destructive),
    bot("work_items.items.read", Get, "/api/v1/issues/{id}", Project(Find::Issue("id")), Read).oauth().plugin(),
    bot("work_items.items.read/activities", Get, "/api/v1/issues/{issue_id}/activities", Project(Find::Issue("issue_id")), Read).oauth(),
    bot("work_items.items.read/board", Get, "/api/v1/projects/{project_id}/board", Project(Find::Project("project_id")), Read),
    bot("work_items.items.read/by_identifier", Get, "/api/v1/issues/by-identifier/{identifier}", Project(Find::IssueIdentifier("identifier")), Read).oauth(),
    bot("work_items.items.read/effective_workflow", Get, "/api/v1/projects/{project_id}/workflow/effective", Project(Find::Project("project_id")), Read),
    bot("work_items.items.read/list", Get, "/api/v1/projects/{project_id}/issues", Project(Find::Project("project_id")), Read).oauth().plugin(),
    bot("work_items.items.read/mine", Get, "/api/v1/my/issues", Filtered, Read),
    bot("work_items.items.read/my_activities", Get, "/api/v1/my/activities", Filtered, Read),
    bot("work_items.items.read/pages", Get, "/api/v1/issues/{issue_id}/pages", Project(Find::Issue("issue_id")), Read).oauth(),
    bot("work_items.items.read/project_activities", Get, "/api/v1/projects/{project_id}/activities", Project(Find::Project("project_id")), Read),
    bot("work_items.items.read/workspace_activities", Get, "/api/v1/workspaces/{workspace_id}/activities", Workspace, Read),
    bot("work_items.items.update", Put, "/api/v1/issues/{id}", Project(Find::Issue("id")), Write).oauth().plugin(),
    bot("work_items.items.update/label_add", Post, "/api/v1/issues/{issue_id}/labels/{label_id}", Project(Find::Issue("issue_id")), Write).oauth(),
    bot("work_items.items.update/label_remove", Delete, "/api/v1/issues/{issue_id}/labels/{label_id}", Project(Find::Issue("issue_id")), Write).oauth(),
    bot("work_items.items.update/labels_batch", Post, "/api/v1/issues/{issue_id}/labels/batch", Project(Find::Issue("issue_id")), Write),
    bot("work_items.items.update/page_add", Post, "/api/v1/issues/{issue_id}/pages", Project(Find::Issue("issue_id")), Write),
    bot("work_items.items.update/page_remove", Delete, "/api/v1/issues/{issue_id}/pages/{object_id}", Project(Find::Issue("issue_id")), Write),
    bot("work_items.labels.create", Post, "/api/v1/workspaces/{workspace_id}/labels", WorkspaceAdmin, Admin),
    bot("work_items.labels.delete", Delete, "/api/v1/labels/{id}", WorkspaceAdmin, Admin),
    bot("work_items.labels.read/issue", Get, "/api/v1/issues/{issue_id}/labels", Project(Find::Issue("issue_id")), Read).oauth(),
    bot("work_items.labels.read/list", Get, "/api/v1/projects/{project_id}/labels", Project(Find::Project("project_id")), Read),
    bot("work_items.labels.read/workspace", Get, "/api/v1/workspaces/{workspace_id}/labels", Workspace, Read),
    bot("work_items.labels.update", Put, "/api/v1/labels/{id}", WorkspaceAdmin, Admin),
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
            BTreeMap::from([("admin", 3), ("filtered", 2), ("project-resolved", 23), ("refused", 2)]),
            "the classes of the issues routes changed"
        );
    }
}
