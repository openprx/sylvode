//! Catalogue entries of the `auth_workspace` router area (`router/auth_workspace.rs`).

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("account.me.read", Get, "/api/v1/auth/me", Own, Read),
    bot("account.me.read/preferences", Get, "/api/v1/auth/me/preferences", Own, Read),
    bot("account.me.update", Put, "/api/v1/auth/me/profile", Workspace, Write),
    bot("account.me.update/preferences", Put, "/api/v1/auth/me/preferences", Own, Write),
    bot("account.password.update", Put, "/api/v1/auth/me/password", Workspace, Secret),
    console("account.registrations.create", Post, "/api/v1/auth/register", Public, Write),
    console("account.sessions.create", Post, "/api/v1/auth/login", Public, Secret),
    bot("account.sessions.delete", Post, "/api/v1/auth/logout", Workspace, Destructive),
    console("account.sessions.refresh", Post, "/api/v1/auth/refresh", Public, Secret),
    console("account.setup.read", Get, "/api/v1/auth/setup-status", Public, Read),
    console("system.email.test", Post, "/api/v1/admin/email/test", InstanceAdmin, Admin),
    console("system.stats.read", Get, "/api/v1/admin/stats", InstanceAdmin, Admin),
    console("system.users.create", Post, "/api/v1/admin/users", InstanceAdmin, Admin),
    console("system.users.delete", Delete, "/api/v1/admin/users/{id}", InstanceAdmin, Admin),
    console("system.users.read", Get, "/api/v1/admin/users/{id}", InstanceAdmin, Admin),
    console("system.users.read/list", Get, "/api/v1/admin/users", InstanceAdmin, Admin),
    console("system.users.rotate", Put, "/api/v1/admin/users/{id}/password", InstanceAdmin, Secret),
    console("system.users.update", Put, "/api/v1/admin/users/{id}", InstanceAdmin, Admin),
    console("system.users.update/status", Patch, "/api/v1/admin/users/{id}/status", InstanceAdmin, Admin),
    bot("workspace.members.create", Post, "/api/v1/workspaces/{workspace_id}/members", WorkspaceAdmin, Admin),
    bot("workspace.members.delete", Delete, "/api/v1/workspaces/{workspace_id}/members/{user_id}", WorkspaceAdmin, Admin),
    bot("workspace.members.read/list", Get, "/api/v1/workspaces/{workspace_id}/members", WorkspaceAdmin, Admin),
    bot("workspace.members.read/users", Get, "/api/v1/workspaces/{workspace_id}/users", WorkspaceAdmin, Admin),
    bot("workspace.members.update", Patch, "/api/v1/workspaces/{workspace_id}/members/{user_id}", WorkspaceAdmin, Admin),
    bot("workspace.workspaces.create", Post, "/api/v1/workspaces", WorkspaceAdmin, Admin),
    bot("workspace.workspaces.delete", Delete, "/api/v1/workspaces/{id}", WorkspaceAdmin, Admin),
    bot("workspace.workspaces.read", Get, "/api/v1/workspaces/{id}", WorkspaceAdmin, Admin),
    bot("workspace.workspaces.read/list", Get, "/api/v1/workspaces", WorkspaceAdmin, Admin),
    bot("workspace.workspaces.update", Put, "/api/v1/workspaces/{id}", WorkspaceAdmin, Admin),
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
            BTreeMap::from([("admin", 10), ("own resource", 3), ("refused", 3)]),
            "the classes of the auth_workspace routes changed"
        );
    }
}
