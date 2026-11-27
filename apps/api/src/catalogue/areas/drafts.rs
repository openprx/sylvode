//! Catalogue entries of the `drafts` router area (`router/drafts.rs`).
//!
//! Creating and reading are open to bots in the draft's project. Approving and rejecting are
//! console only: they sit behind the user-session layer, so a bot token or a connected
//! application never reaches them, and the handler asks for a workspace owner or administrator.
//! Both share the `approve` capability.

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("drafts.drafts.create", Post, "/api/v1/drafts", Project(Find::Body), Write),
    bot("drafts.drafts.read", Get, "/api/v1/drafts/{id}", Project(Find::Table("artefact_drafts", "id")), Read),
    bot("drafts.drafts.read/list", Get, "/api/v1/drafts", Filtered, Read),
    console("drafts.drafts.approve", Post, "/api/v1/drafts/{id}/approve", WorkspaceAdmin, Admin),
    console("drafts.drafts.approve/reject", Post, "/api/v1/drafts/{id}/reject", WorkspaceAdmin, Admin),
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
            BTreeMap::from([("filtered", 1), ("project-resolved", 2)]),
            "the classes of the drafts routes changed"
        );
    }
}
