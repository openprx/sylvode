//! Catalogue entries of the `packages` router area (`router/packages.rs`).
//!
//! The two read routes and the upload submission are behind the bot authentication. The class `Refused` still admits a
//! project-scoped bot with a default level of `read` or more, so the handlers refuse every bot that
//! is not a bot of the whole workspace (`packages::access`, packages are workspace-level); a bot of
//! the whole workspace passes and the handler checks its workspace and `read` permission. The
//! submission route (`package-uploads`) is for bots only and stores nothing as a package: it keeps
//! a verified upload as a `package_upload` draft that a person approves (contract C3 section 4.4,
//! LEAD-DECISIONS M11-UPLOAD); the handler also demands `write` and a bot of the whole workspace.
//! The search route (`market/search`) follows the same rule as the package list. The index read routes
//! (`market/index`, contract M15) follow the same rule as the package reads; their image route is for a
//! console session, as are import, source refresh, hide and the event log (owner or admin, decided in the
//! handlers). Upload, delete and the publisher-key routes are behind the JWT middleware, which no bot token
//! passes.

use crate::catalogue::types::dsl::{
    Delete, Destructive, Entry, Get, Post, Put, Read, Workspace, WorkspaceAdmin, Write, bot, console,
};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("packages.packages.read", Get, "/api/v1/workspaces/{workspace_id}/packages/{digest}", Workspace, Read),
    bot("packages.packages.read/list", Get, "/api/v1/workspaces/{workspace_id}/packages", Workspace, Read),
    bot("market.packages.read/search", Get, "/api/v1/workspaces/{workspace_id}/market/search", Workspace, Read),
    bot("packages.package_uploads.create", Post, "/api/v1/workspaces/{workspace_id}/package-uploads", Workspace, Write),
    console("packages.packages.create", Post, "/api/v1/workspaces/{workspace_id}/packages", WorkspaceAdmin, Write),
    console("packages.packages.delete", Delete, "/api/v1/workspaces/{workspace_id}/packages/{digest}", WorkspaceAdmin, Destructive),
    bot("packages.index_sources.read/list", Get, "/api/v1/workspaces/{workspace_id}/market/index/sources", Workspace, Read),
    bot("packages.index_plugins.read/list", Get, "/api/v1/workspaces/{workspace_id}/market/index/plugins", Workspace, Read),
    bot("packages.index_plugins.read", Get, "/api/v1/workspaces/{workspace_id}/market/index/plugins/{source}/{publisher}/{name}", Workspace, Read),
    console("packages.index_assets.read", Get, "/api/v1/workspaces/{workspace_id}/market/index/assets/{source}/{publisher}/{name}/{*asset_path}", Workspace, Read).no_plugin(),
    console("packages.index_import.create", Post, "/api/v1/workspaces/{workspace_id}/market/index/import", WorkspaceAdmin, Write),
    console("packages.index_sources.refresh", Post, "/api/v1/workspaces/{workspace_id}/market/index/sources/{source}/refresh", WorkspaceAdmin, Write),
    console("packages.index_sources.update", Put, "/api/v1/workspaces/{workspace_id}/market/index/sources/{source}/hidden", WorkspaceAdmin, Write),
    console("packages.index_events.read/list", Get, "/api/v1/workspaces/{workspace_id}/market/index/events", WorkspaceAdmin, Read),
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
            BTreeMap::from([("refused", 7)]),
            "the classes of the packages routes changed"
        );
    }
}
