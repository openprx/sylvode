//! Catalogue entries of the `extensions` router area (`router/extensions.rs`).
//!
//! Request, upgrade and preview are behind the bot-or-user authentication: a workspace owner or
//! admin, or a bot of the whole workspace with `write` (the class `Admin` refuses every
//! project-scoped bot; the handler checks the rest). Review, approve, reject, enable, disable,
//! revoke, the reads and the invocation log are for people with a console session only. The invoke
//! route is for a console session too: bots do not invoke actions yet (lead decision C1-2).

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("extensions.installations.request", Post, "/api/v1/workspaces/{workspace_id}/installations", WorkspaceAdmin, Admin),
    bot("extensions.installations.request/upgrade", Post, "/api/v1/installations/{installation_id}/generations", WorkspaceAdmin, Admin),
    bot("extensions.installations.read/preview", Post, "/api/v1/workspaces/{workspace_id}/installations/preview", WorkspaceAdmin, Read),
    console("extensions.installations.read/list", Get, "/api/v1/workspaces/{workspace_id}/installations", WorkspaceAdmin, Read),
    console("extensions.installations.read", Get, "/api/v1/installations/{installation_id}", WorkspaceAdmin, Read),
    console("extensions.installations.read/review", Get, "/api/v1/installations/{installation_id}/review", WorkspaceAdmin, Read),
    console("extensions.installations.approve", Post, "/api/v1/installations/{installation_id}/approve", WorkspaceAdmin, Admin),
    console("extensions.installations.approve/reject", Post, "/api/v1/installations/{installation_id}/reject", WorkspaceAdmin, Admin),
    console("extensions.installations.disable", Post, "/api/v1/installations/{installation_id}/disable", WorkspaceAdmin, Admin),
    console("extensions.installations.disable/enable", Post, "/api/v1/installations/{installation_id}/enable", WorkspaceAdmin, Admin),
    console("extensions.installations.revoke", Post, "/api/v1/installations/{installation_id}/revoke", WorkspaceAdmin, Admin),
    console("extensions.invocations.read", Get, "/api/v1/installations/{installation_id}/invocations", WorkspaceAdmin, Read),
    console("extensions.actions.invoke", Post, "/api/v1/installations/{installation_id}/actions/{action}", Own, Write),
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
            BTreeMap::from([("admin", 3)]),
            "the classes of the extensions routes changed"
        );
    }
}
