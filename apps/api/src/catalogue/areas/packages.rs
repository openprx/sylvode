//! Catalogue entries of the `packages` router area (`router/packages.rs`).
//!
//! Only the two read routes are behind the bot authentication. The class `Refused` still admits a
//! project-scoped bot with a default level of `read` or more, so the handlers refuse every bot that
//! is not a bot of the whole workspace (`packages::access`, packages are workspace-level); a bot of
//! the whole workspace passes and the handler checks its workspace and `read` permission. Upload and
//! the publisher-key routes are behind the JWT middleware, which no bot token passes.

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("packages.packages.read", Get, "/api/v1/workspaces/{workspace_id}/packages/{digest}", Workspace, Read),
    bot("packages.packages.read/list", Get, "/api/v1/workspaces/{workspace_id}/packages", Workspace, Read),
    console("packages.packages.create", Post, "/api/v1/workspaces/{workspace_id}/packages", WorkspaceAdmin, Write),
    console("packages.publisher_keys.create", Post, "/api/v1/workspaces/{workspace_id}/publisher-keys", WorkspaceAdmin, Write),
    console("packages.publisher_keys.read", Get, "/api/v1/workspaces/{workspace_id}/publisher-keys", WorkspaceAdmin, Read),
    console("packages.publisher_keys.revoke", Post, "/api/v1/workspaces/{workspace_id}/publisher-keys/{key_id}/revoke", WorkspaceAdmin, Write),
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
            BTreeMap::from([("refused", 2)]),
            "the classes of the packages routes changed"
        );
    }
}
