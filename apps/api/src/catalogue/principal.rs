//! Who calls an operation, and the resource it targets (contract C1 §2.3, §3).
//!
//! Every fact the checker needs is loaded into these values beforehand, so [`super::allowed`] is
//! pure and synchronous.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::entities::workspace_member::WorkspaceRole;
use crate::middleware::bot_auth::BotPermission;
use crate::middleware::scoped_bot::BotAccess;

/// One grant of an installation: a capability id and the selector that narrows it. An absent key
/// means any value within the installation's scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub capability: String,
    pub project: Option<Uuid>,
    pub form: Option<Uuid>,
}

/// An installation as the host loads it before every call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallationView {
    pub installation_id: Uuid,
    pub generation: i32,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    /// The capability ids the active generation's manifest requests.
    pub requested: BTreeSet<String>,
    pub grants: Vec<Grant>,
}

/// Who invoked a plugin action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invoker {
    User {
        user_id: Uuid,
        workspace_id: Uuid,
        role: WorkspaceRole,
    },
}

/// The caller of an operation.
#[derive(Clone, Debug)]
pub enum Principal {
    User {
        user_id: Uuid,
        workspace_id: Uuid,
        role: WorkspaceRole,
    },
    Bot {
        bot_id: Uuid,
        workspace_id: Uuid,
        permission: BotPermission,
        access: BotAccess,
    },
    Delegated {
        client_id: Uuid,
        user_id: Uuid,
        workspace_id: Uuid,
    },
    Plugin {
        invoker: Invoker,
        installation: InstallationView,
    },
    Installation {
        installation: InstallationView,
        approver_user_id: Uuid,
    },
}

/// The target of an operation, resolved by the caller of the checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub form_id: Option<Uuid>,
}
