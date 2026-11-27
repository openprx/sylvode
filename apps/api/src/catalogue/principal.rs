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
    /// The `plugin_ops_digest` of the capability id when the grant was approved. A grant whose
    /// digest differs from the id's current one is not loaded (see `extensions::install::live`).
    pub plugin_ops_digest: String,
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
    /// An anonymous visitor of a public site; `visitor_id` once the visitor cookie exists.
    SiteVisitor {
        site_id: Uuid,
        workspace_id: Uuid,
        project_id: Uuid,
        visitor_id: Option<Uuid>,
    },
    /// A workspace member signed in to a public site with a site role.
    SiteOperator {
        site_id: Uuid,
        workspace_id: Uuid,
        project_id: Uuid,
        user_id: Uuid,
        role: SiteRole,
    },
}

/// The role of a site operator (`site_roles.role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteRole {
    Operator,
    Editor,
}

impl SiteRole {
    /// The stored name.
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Operator => "site_operator",
            Self::Editor => "site_editor",
        }
    }

    /// The name on the site API.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Editor => "editor",
        }
    }

    /// The role of a stored name, or of its site API name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "site_operator" | "operator" => Some(Self::Operator),
            "site_editor" | "editor" => Some(Self::Editor),
            _ => None,
        }
    }
}

/// The target of an operation, resolved by the caller of the checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub form_id: Option<Uuid>,
}
