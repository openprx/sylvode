//! The types of the operation catalogue: one [`Entry`] per route of the router.

use crate::middleware::scoped_bot::Class;

/// An HTTP method of a catalogued route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    #[must_use]
    pub fn parse(method: &str) -> Option<Self> {
        match method {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// What a capability can do to the system, one value per capability id; the highest of its routes wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// No state change.
    Read,
    /// Any other state change.
    Write,
    /// Deletes or purges.
    Destructive,
    /// Changes permissions, membership, bots, webhooks or configuration; every route of scope
    /// `WorkspaceAdmin` or `InstanceAdmin`.
    Admin,
    /// Mints, reveals or rotates a credential or a capability URL.
    Secret,
}

impl Risk {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
            Self::Admin => "admin",
            Self::Secret => "secret",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stability {
    Stable,
    Experimental,
    Internal,
}

impl Stability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Experimental => "experimental",
            Self::Internal => "internal",
        }
    }
}

/// How the project of a request's target is found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find {
    /// The path parameter is the project id.
    Project(&'static str),
    Issue(&'static str),
    /// `PREFIX-N` within the bot's workspace.
    IssueIdentifier(&'static str),
    Comment(&'static str),
    /// A table with `id` and `project_id` columns; the path parameter is the row's id.
    Table(&'static str, &'static str),
    /// A governance proposal: the one project its linked issues belong to, or, when it is linked to
    /// none, itself when the bot authored it.
    Proposal(&'static str),
    ProposalComment(&'static str),
    Decision(&'static str),
    /// A Flow object: its project, or, outside any project, itself when it is in the bot's own
    /// personal space.
    FlowObject(&'static str),
    /// A Flow scope: allowed only when it is the bot's own personal scope.
    FlowScope(&'static str),
    /// A row of `table` (key column `id_col`) that names a Flow object in `object_col`.
    FlowVia(&'static str, &'static str, &'static str, &'static str),
    /// The project is named in the JSON body: `project_id`, else the parent object, else the scope.
    Body,
    /// A Flow conversion: the project of the source object (preview) or of the stored preview's source
    /// object (commit), and nothing else. A body that names both, or neither, resolves to nothing; a
    /// `project_id` in the body is not looked at, so it cannot choose the project the bot is judged by.
    Conversion,
    /// The project is the `project_id` query parameter; a request without it is refused.
    Query(&'static str),
}

/// How the target of a site route is found. Filled by the site lane (contract C3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteFind {}

/// What a route is scoped to. The bot class is a 1:1 image of it (see [`Scope::bot_class`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// No authentication.
    Public,
    /// No project; the handler limits the caller to what it owns or to its own workspace.
    Global,
    /// The resource belongs to the caller itself.
    Own,
    /// The target resolves to a project.
    Project(Find),
    /// A workspace-level read whose handler limits the rows to the readable projects.
    Filtered,
    /// Spans projects of a workspace.
    Workspace,
    /// Workspace administration.
    WorkspaceAdmin,
    /// The `admin_middleware` routes.
    InstanceAdmin,
    /// The public surface of a site.
    Site(SiteFind),
}

impl Scope {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Global => "global",
            Self::Own => "own",
            Self::Project(_) => "project",
            Self::Filtered => "filtered",
            Self::Workspace => "workspace",
            Self::WorkspaceAdmin => "workspace_admin",
            Self::InstanceAdmin => "instance_admin",
            Self::Site(_) => "site",
        }
    }

    /// The bot class of a scope; `None` for a scope a bot never reaches.
    #[must_use]
    pub const fn bot_class(self) -> Option<Class> {
        match self {
            Self::Global => Some(Class::NoProject),
            Self::Own => Some(Class::Own),
            Self::Project(find) => Some(Class::Project(find)),
            Self::Filtered => Some(Class::Filtered),
            Self::Workspace => Some(Class::Refused),
            Self::WorkspaceAdmin => Some(Class::Admin),
            Self::Public | Self::InstanceAdmin | Self::Site(_) => None,
        }
    }
}

/// The channels a route can be called through, as a bit set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exposure(u8);

impl Exposure {
    /// A user session: every route with `jwt`, bot-or-user or admin authentication, and public routes.
    pub const CONSOLE: Self = Self(1);
    /// Behind `bot_or_user_auth_middleware`: bot tokens, hence MCP.
    pub const BOT: Self = Self(2);
    /// In the OAuth allow-list `DELEGATED_ROUTES`.
    pub const OAUTH: Self = Self(4);
    /// Dispatchable by the plugin host for plugin and installation principals.
    pub const PLUGIN: Self = Self(8);
    /// Only routes under `/api/site/v1/`.
    pub const SITE: Self = Self(16);

    const NAMES: [(Self, &'static str); 5] = [
        (Self::CONSOLE, "console"),
        (Self::BOT, "bot"),
        (Self::OAUTH, "oauth"),
        (Self::PLUGIN, "plugin"),
        (Self::SITE, "site"),
    ];

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit of `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// The exposure called `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find_map(|(exposure, known)| (*known == name).then_some(*exposure))
    }

    /// The names of the set bits, in a fixed order.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(exposure, _)| self.contains(*exposure))
            .map(|(_, name)| *name)
            .collect()
    }
}

/// One route of the router.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Unique; the capability id is the part before the first `/`.
    pub op: &'static str,
    pub method: Method,
    /// The router's matched path template, verbatim.
    pub path: &'static str,
    pub scope: Scope,
    pub risk: Risk,
    /// A bot needs the `comment` level instead of `write`.
    pub comment: bool,
    pub exposure: Exposure,
    pub stability: Stability,
}

impl Entry {
    /// The capability id: the operation key up to the first `/`.
    #[must_use]
    pub fn capability(&self) -> &'static str {
        self.op.split_once('/').map_or(self.op, |(id, _)| id)
    }

    /// The facet of the operation key, if any.
    #[must_use]
    pub fn facet(&self) -> Option<&'static str> {
        self.op.split_once('/').map(|(_, facet)| facet)
    }

    /// The area, resource and verb of the capability id.
    #[must_use]
    pub fn id_parts(&self) -> Option<(&'static str, &'static str, &'static str)> {
        let mut parts = self.capability().split('.');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(area), Some(resource), Some(verb), None) => Some((area, resource, verb)),
            _ => None,
        }
    }

    /// A comment route: a bot needs the `comment` level in the project, not `write`.
    #[must_use]
    pub const fn comment(mut self) -> Self {
        self.comment = true;
        self
    }

    /// The route is in the OAuth allow-list.
    #[must_use]
    pub const fn oauth(mut self) -> Self {
        self.exposure = self.exposure.union(Exposure::OAUTH);
        self
    }

    /// The route is dispatchable for plugin and installation principals.
    #[must_use]
    pub const fn plugin(mut self) -> Self {
        self.exposure = self.exposure.union(Exposure::PLUGIN);
        self
    }

    #[must_use]
    pub const fn experimental(mut self) -> Self {
        self.stability = Stability::Experimental;
        self
    }
}

const fn entry(
    op: &'static str,
    method: Method,
    path: &'static str,
    scope: Scope,
    risk: Risk,
    exposure: Exposure,
) -> Entry {
    Entry {
        op,
        method,
        path,
        scope,
        risk,
        comment: false,
        exposure,
        stability: Stability::Stable,
    }
}

/// A route behind the bot-or-user authentication: console and bot.
#[must_use]
pub const fn bot(op: &'static str, method: Method, path: &'static str, scope: Scope, risk: Risk) -> Entry {
    entry(op, method, path, scope, risk, Exposure::CONSOLE.union(Exposure::BOT))
}

/// A route a bot never reaches: user session, admin or public.
#[must_use]
pub const fn console(op: &'static str, method: Method, path: &'static str, scope: Scope, risk: Risk) -> Entry {
    entry(op, method, path, scope, risk, Exposure::CONSOLE)
}

/// What a table file needs: the builders and every variant name.
pub mod dsl {
    pub use super::Method::{Delete, Get, Patch, Post, Put};
    pub use super::Risk::{Admin, Destructive, Read, Secret, Write};
    pub use super::Scope::{Filtered, Global, InstanceAdmin, Own, Project, Public, Workspace, WorkspaceAdmin};
    pub use super::{Entry, Find, bot, console};
}
