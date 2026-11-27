//! Catalogue entries of the `nav` router area (`router/nav.rs`).
//!
//! The menus are presentation: each handler hides what the viewer cannot read and a target page
//! keeps its own check. The workspace routes name no project, so they are `Global`: the handler
//! refuses a bot limited to projects, and writes are for people only. The project routes resolve
//! the project from the path; a bot writes a project navigation like the project's other content,
//! except external links and lock flags (`nav::bot`). The preference routes are for people only.

use crate::catalogue::types::dsl::{
    Delete, Destructive, Entry, Find, Get, Global, Project, Put, Read, Workspace, Write, bot, console,
};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("nav.workspace_menu.read", Get, "/api/v1/workspaces/{workspace_id}/nav", Global, Read),
    bot("nav.workspace_config.read", Get, "/api/v1/workspaces/{workspace_id}/nav/config", Global, Read),
    bot("nav.workspace_config.update", Put, "/api/v1/workspaces/{workspace_id}/nav/config", Global, Write),
    bot("nav.workspace_config.delete", Delete, "/api/v1/workspaces/{workspace_id}/nav/config", Global, Destructive),
    bot("nav.project_menu.read", Get, "/api/v1/projects/{project_id}/nav", Project(Find::Project("project_id")), Read),
    bot("nav.project_config.read", Get, "/api/v1/projects/{project_id}/nav/config", Project(Find::Project("project_id")), Read),
    bot("nav.project_config.update", Put, "/api/v1/projects/{project_id}/nav/config", Project(Find::Project("project_id")), Write),
    bot("nav.project_config.delete", Delete, "/api/v1/projects/{project_id}/nav/config", Project(Find::Project("project_id")), Destructive),
    console("nav.prefs.read", Get, "/api/v1/me/nav-prefs", Workspace, Read),
    console("nav.prefs.update", Put, "/api/v1/me/nav-prefs", Workspace, Write),
    console("nav.prefs.delete", Delete, "/api/v1/me/nav-prefs", Workspace, Destructive),
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
            BTreeMap::from([("no project", 4), ("project-resolved", 4)]),
            "the classes of the nav routes changed"
        );
    }
}
