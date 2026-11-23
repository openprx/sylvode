//! Runner bot route classes of the `auth_workspace` router area (`router/auth_workspace.rs`).

use super::{RunnerRoute, r};
use crate::middleware::runner_bot_scope::Class;

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("GET", "/api/v1/auth/me", Class::Own),
    r("GET", "/api/v1/auth/me/preferences", Class::Own),
    r("PUT", "/api/v1/auth/me/preferences", Class::Own),
    r("GET", "/api/v1/workspaces", Class::Admin),
    r("POST", "/api/v1/workspaces", Class::Admin),
    r("DELETE", "/api/v1/workspaces/{id}", Class::Admin),
    r("GET", "/api/v1/workspaces/{id}", Class::Admin),
    r("PUT", "/api/v1/workspaces/{id}", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/members", Class::Admin),
    r("POST", "/api/v1/workspaces/{workspace_id}/members", Class::Admin),
    r("DELETE", "/api/v1/workspaces/{workspace_id}/members/{user_id}", Class::Admin),
    r("PATCH", "/api/v1/workspaces/{workspace_id}/members/{user_id}", Class::Admin),
    r("GET", "/api/v1/workspaces/{workspace_id}/users", Class::Admin),
    r("POST", "/api/v1/auth/logout", Class::Refused),
    r("PUT", "/api/v1/auth/me/password", Class::Refused),
    r("PUT", "/api/v1/auth/me/profile", Class::Refused),
];

#[cfg(test)]
mod tests {
    use super::ROUTES;
    use crate::middleware::bot_routes::class_counts;
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            class_counts(ROUTES),
            BTreeMap::from([("admin", 10), ("own resource", 3), ("refused", 3)]),
            "the classes of the auth_workspace routes changed"
        );
    }
}
