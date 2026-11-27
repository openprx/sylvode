//! Catalogue entries of the `layouts` router area (`router/layouts.rs`).
//!
//! The layout and placement rows carry a polymorphic scope (workspace, project or form), not a
//! `project_id` column, so the middleware cannot resolve their project. Those handlers decide for
//! a scoped bot themselves (`layouts::access`): reads need `read` and writes `write` in the
//! scope's project through `scoped_bot::ensure_bot_level`, and a workspace-scoped row is reachable
//! only by a bot that is not limited to projects. The preference route is for people only.

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("layouts.components.read", Get, "/api/v1/layouts/components", Global, Read),
    bot("layouts.layouts.create", Post, "/api/v1/layouts", Global, Write),
    bot("layouts.layouts.delete", Delete, "/api/v1/layouts/{id}", Global, Destructive),
    bot("layouts.layouts.read", Get, "/api/v1/layouts/{id}", Global, Read),
    bot("layouts.layouts.read/list", Get, "/api/v1/layouts", Filtered, Read),
    bot("layouts.layouts.read/validate", Post, "/api/v1/layouts/validate", Global, Read),
    bot("layouts.layouts.update", Patch, "/api/v1/layouts/{id}", Global, Write),
    bot("layouts.placements.create", Post, "/api/v1/placements", Global, Write),
    bot("layouts.placements.delete", Delete, "/api/v1/placements/{id}", Global, Destructive),
    bot("layouts.placements.read/slot", Get, "/api/v1/slots/{slot_id}/placements", Project(Find::Query("project_id")), Read),
    bot("layouts.placements.update", Put, "/api/v1/placements/{id}", Global, Write),
    bot("layouts.preferences.update", Put, "/api/v1/me/placements/{id}/prefs", Workspace, Write),
    bot("layouts.slots.read", Get, "/api/v1/slots", Global, Read),
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
                ("filtered", 1),
                ("no project", 10),
                ("project-resolved", 1),
                ("refused", 1)
            ]),
            "the classes of the layouts routes changed"
        );
    }
}
