//! The scope vocabulary, scope sets, and what a grant can reach.
//!
//! The vocabulary is closed: a name that is not in [`Scope::ALL`] is refused, and three names are
//! reserved for later releases and refused with their own error so an audit row can tell the two
//! apart. A [`ScopeSet`] is a bit set, so intersection, union and subset tests are single
//! instructions and a set can not hold a name outside the vocabulary.
//!
//! Hierarchy: `work_items:write` implies `work_items:read` and `forms:write` implies
//! `forms:read`, as the first release's table states. No other write scope implies a read scope;
//! `notifications:write` marks notifications read and does not let a client list them.

use std::sync::Arc;

use thiserror::Error;
use uuid::Uuid;

/// Longest scope string accepted from a request, in bytes.
pub const MAX_SCOPE_STRING_LEN: usize = 1024;

/// Names reserved for later releases. Requests for them are refused as `invalid_scope`.
pub const RESERVED_SCOPES: [&str; 3] = ["flow:read", "flow:write", "tasks:dispatch"];

/// One scope of the first release's vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    ProjectsRead,
    WorkItemsRead,
    WorkItemsWrite,
    FormsRead,
    FormsWrite,
    NotificationsRead,
    NotificationsWrite,
    TasksRead,
    SurfacesEmbed,
    OfflineAccess,
}

impl Scope {
    /// The whole vocabulary, in the order metadata documents list it.
    pub const ALL: [Self; 10] = [
        Self::ProjectsRead,
        Self::WorkItemsRead,
        Self::WorkItemsWrite,
        Self::FormsRead,
        Self::FormsWrite,
        Self::NotificationsRead,
        Self::NotificationsWrite,
        Self::TasksRead,
        Self::SurfacesEmbed,
        Self::OfflineAccess,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectsRead => "projects:read",
            Self::WorkItemsRead => "work_items:read",
            Self::WorkItemsWrite => "work_items:write",
            Self::FormsRead => "forms:read",
            Self::FormsWrite => "forms:write",
            Self::NotificationsRead => "notifications:read",
            Self::NotificationsWrite => "notifications:write",
            Self::TasksRead => "tasks:read",
            Self::SurfacesEmbed => "surfaces:embed",
            Self::OfflineAccess => "offline_access",
        }
    }

    /// The scope with this exact name, or `None`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.as_str() == name)
    }

    const fn bit(self) -> u16 {
        1 << (self as u16)
    }

    /// The read scope this scope implies, if any.
    pub const fn implied_read(self) -> Option<Self> {
        match self {
            Self::WorkItemsWrite => Some(Self::WorkItemsRead),
            Self::FormsWrite => Some(Self::FormsRead),
            _ => None,
        }
    }

    /// Whether the user may untick the scope on the consent page. A scope that is not optional is
    /// shown fixed when the client asked for it.
    pub const fn optional_on_consent(self) -> bool {
        matches!(
            self,
            Self::WorkItemsWrite | Self::FormsWrite | Self::NotificationsWrite | Self::SurfacesEmbed
        )
    }
}

/// Why a scope string or list was refused.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ScopeError {
    #[error("the scope list is empty")]
    Empty,
    #[error("the scope list is longer than {MAX_SCOPE_STRING_LEN} bytes")]
    TooLong,
    #[error("the scope list is not a single-space separated list of scope tokens")]
    Malformed,
    #[error("a scope is not part of the vocabulary")]
    Unknown,
    #[error("a scope is reserved for a later release")]
    Reserved,
    #[error("the requested scopes are not a subset of the granted scopes")]
    NotSubset,
}

/// A set of scopes, as a bit set over [`Scope::ALL`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScopeSet(u16);

impl ScopeSet {
    pub const EMPTY: Self = Self(0);

    pub fn from_scopes(scopes: &[Scope]) -> Self {
        scopes.iter().fold(Self::EMPTY, |set, scope| set.with(*scope))
    }

    /// Every scope of the vocabulary.
    pub fn all() -> Self {
        Self::from_scopes(&Scope::ALL)
    }

    #[must_use]
    pub const fn with(self, scope: Scope) -> Self {
        Self(self.0 | scope.bit())
    }

    /// Parses an OAuth `scope` parameter: scope tokens separated by single spaces.
    ///
    /// Duplicates collapse. An empty string, an empty token, a token with characters outside
    /// RFC 6749 section 3.3, an unknown name and a reserved name are each refused.
    pub fn parse(text: &str) -> Result<Self, ScopeError> {
        if text.is_empty() {
            return Err(ScopeError::Empty);
        }
        if text.len() > MAX_SCOPE_STRING_LEN {
            return Err(ScopeError::TooLong);
        }
        Self::from_names(text.split(' '))
    }

    /// Builds a set from names, as read from a text array column or a parsed list.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self, ScopeError> {
        let mut set = Self::EMPTY;
        let mut any = false;
        for name in names {
            any = true;
            if name.is_empty() || !name.bytes().all(|b| matches!(b, 0x21 | 0x23..=0x5b | 0x5d..=0x7e)) {
                return Err(ScopeError::Malformed);
            }
            match Scope::from_name(name) {
                Some(scope) => set = set.with(scope),
                None if RESERVED_SCOPES.contains(&name) => return Err(ScopeError::Reserved),
                None => return Err(ScopeError::Unknown),
            }
        }
        if any { Ok(set) } else { Err(ScopeError::Empty) }
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether the set names `scope` itself, ignoring the hierarchy.
    pub const fn contains(self, scope: Scope) -> bool {
        self.0 & scope.bit() != 0
    }

    /// Whether the set authorises something that requires `required`: the scope itself, or a
    /// scope that implies it.
    pub fn allows(self, required: Scope) -> bool {
        self.contains(required)
            || Scope::ALL
                .into_iter()
                .any(|held| self.contains(held) && held.implied_read() == Some(required))
    }

    /// Whether every scope of `self` is in `other`.
    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    #[must_use]
    pub const fn without(self, scope: Scope) -> Self {
        Self(self.0 & !scope.bit())
    }

    /// The set plus every scope its members imply.
    #[must_use]
    pub fn with_implied(self) -> Self {
        Scope::ALL
            .into_iter()
            .filter(|scope| self.contains(*scope))
            .filter_map(Scope::implied_read)
            .fold(self, Self::with)
    }

    /// The scopes to name in an `insufficient_scope` challenge for a call that requires
    /// `required`: the scope and the read scope below it.
    pub fn challenge_for(required: Scope) -> Self {
        Self::EMPTY.with(required).with_implied()
    }

    /// The members, in vocabulary order.
    pub fn iter(self) -> impl Iterator<Item = Scope> {
        Scope::ALL.into_iter().filter(move |scope| self.contains(*scope))
    }

    /// The space separated form used in tokens responses and challenges.
    pub fn to_scope_string(self) -> String {
        let mut out = String::new();
        for scope in self.iter() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(scope.as_str());
        }
        out
    }

    /// The names, for a text array column.
    pub fn to_names(self) -> Vec<String> {
        self.iter().map(|scope| scope.as_str().to_owned()).collect()
    }

    /// The scope an access token for `resource` carries: the grant's scopes intersected with what
    /// the resource supports. `offline_access` is a property of the grant and never part of an
    /// access token.
    #[must_use]
    pub fn narrow_to_resource(self, resource: TokenResource) -> Self {
        self.intersection(resource.scopes_supported())
    }

    /// Checks a refresh request's `scope` against the grant: it may only narrow.
    pub const fn narrow_to_subset(self, requested: Self) -> Result<Self, ScopeError> {
        if requested.is_empty() {
            return Err(ScopeError::Empty);
        }
        if requested.is_subset_of(self) {
            Ok(requested)
        } else {
            Err(ScopeError::NotSubset)
        }
    }
}

/// The resource an access token is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenResource {
    Api,
    Mcp,
}

impl TokenResource {
    /// The symbolic name stored in `oauth_tokens.resource` and `oauth_clients.allowed_resources`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Mcp => "mcp",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "api" => Some(Self::Api),
            "mcp" => Some(Self::Mcp),
            _ => None,
        }
    }

    /// `scopes_supported` of the resource's protected resource metadata.
    pub fn scopes_supported(self) -> ScopeSet {
        match self {
            Self::Mcp => ScopeSet::from_scopes(&[
                Scope::ProjectsRead,
                Scope::WorkItemsRead,
                Scope::WorkItemsWrite,
                Scope::FormsRead,
                Scope::FormsWrite,
            ]),
            Self::Api => ScopeSet::all().without(Scope::OfflineAccess),
        }
    }
}

/// The projects a grant reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectScope {
    All,
    Selected(Arc<[Uuid]>),
}

impl ProjectScope {
    pub fn contains(&self, project_id: Uuid) -> bool {
        match self {
            Self::All => true,
            Self::Selected(ids) => ids.contains(&project_id),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_the_contract_table() {
        let names: Vec<_> = Scope::ALL.iter().map(|scope| scope.as_str()).collect();
        assert_eq!(
            names,
            [
                "projects:read",
                "work_items:read",
                "work_items:write",
                "forms:read",
                "forms:write",
                "notifications:read",
                "notifications:write",
                "tasks:read",
                "surfaces:embed",
                "offline_access"
            ]
        );
        for scope in Scope::ALL {
            assert_eq!(Scope::from_name(scope.as_str()), Some(scope));
        }
        let optional: Vec<_> = Scope::ALL
            .iter()
            .filter(|s| s.optional_on_consent())
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            optional,
            [
                "work_items:write",
                "forms:write",
                "notifications:write",
                "surfaces:embed"
            ]
        );
    }

    #[test]
    fn parsing_accepts_single_space_lists_and_collapses_duplicates() {
        let set = ScopeSet::parse("projects:read work_items:read projects:read offline_access").unwrap();
        assert_eq!(set.to_scope_string(), "projects:read work_items:read offline_access");
        assert_eq!(set.to_names(), ["projects:read", "work_items:read", "offline_access"]);
        assert_eq!(
            ScopeSet::parse("tasks:read").unwrap(),
            ScopeSet::from_scopes(&[Scope::TasksRead])
        );
    }

    #[test]
    fn parsing_refuses_every_malformed_input_with_its_reason() {
        let cases = [
            ("", ScopeError::Empty),
            (" ", ScopeError::Malformed),
            ("projects:read ", ScopeError::Malformed),
            (" projects:read", ScopeError::Malformed),
            ("projects:read  work_items:read", ScopeError::Malformed),
            ("projects:read\twork_items:read", ScopeError::Malformed),
            ("projects:read\nwork_items:read", ScopeError::Malformed),
            ("projects:\"read", ScopeError::Malformed),
            ("projects:\\read", ScopeError::Malformed),
            ("projects:réad", ScopeError::Malformed),
            ("Projects:Read", ScopeError::Unknown),
            ("projects:delete", ScopeError::Unknown),
            ("projects", ScopeError::Unknown),
            ("projects:read admin", ScopeError::Unknown),
            ("flow:read", ScopeError::Reserved),
            ("flow:write", ScopeError::Reserved),
            ("tasks:dispatch", ScopeError::Reserved),
            ("projects:read tasks:dispatch", ScopeError::Reserved),
        ];
        for (text, expected) in cases {
            assert_eq!(ScopeSet::parse(text), Err(expected), "{text:?}");
        }
        let long = vec!["projects:read"; 100].join(" ");
        assert_eq!(ScopeSet::parse(&long), Err(ScopeError::TooLong));
        assert_eq!(ScopeSet::from_names(std::iter::empty()), Err(ScopeError::Empty));
    }

    #[test]
    fn the_hierarchy_covers_exactly_the_documented_pairs() {
        let write = ScopeSet::from_scopes(&[Scope::WorkItemsWrite]);
        assert!(write.allows(Scope::WorkItemsWrite) && write.allows(Scope::WorkItemsRead));
        assert!(!write.contains(Scope::WorkItemsRead), "the set itself is not widened");
        assert!(!write.allows(Scope::FormsRead) && !write.allows(Scope::ProjectsRead));
        let read = ScopeSet::from_scopes(&[Scope::WorkItemsRead]);
        assert!(read.allows(Scope::WorkItemsRead) && !read.allows(Scope::WorkItemsWrite));
        let forms = ScopeSet::from_scopes(&[Scope::FormsWrite]);
        assert!(forms.allows(Scope::FormsRead));
        // No other write scope implies a read scope.
        let notifications = ScopeSet::from_scopes(&[Scope::NotificationsWrite]);
        assert!(!notifications.allows(Scope::NotificationsRead));
        for scope in Scope::ALL {
            let expected = matches!(
                (scope, scope.implied_read()),
                (Scope::WorkItemsWrite, Some(Scope::WorkItemsRead)) | (Scope::FormsWrite, Some(Scope::FormsRead))
            ) || scope.implied_read().is_none();
            assert!(expected, "{scope:?}");
        }
        assert_eq!(
            write.with_implied(),
            ScopeSet::from_scopes(&[Scope::WorkItemsWrite, Scope::WorkItemsRead])
        );
    }

    #[test]
    fn challenges_name_the_scope_and_the_read_scope_below_it() {
        assert_eq!(
            ScopeSet::challenge_for(Scope::WorkItemsWrite).to_scope_string(),
            "work_items:read work_items:write"
        );
        assert_eq!(
            ScopeSet::challenge_for(Scope::FormsWrite).to_scope_string(),
            "forms:read forms:write"
        );
        assert_eq!(
            ScopeSet::challenge_for(Scope::TasksRead).to_scope_string(),
            "tasks:read"
        );
    }

    #[test]
    fn set_operations() {
        let a = ScopeSet::from_scopes(&[Scope::ProjectsRead, Scope::TasksRead]);
        let b = ScopeSet::from_scopes(&[Scope::TasksRead, Scope::FormsRead]);
        assert_eq!(a.union(b).to_names().len(), 3);
        assert_eq!(a.intersection(b), ScopeSet::from_scopes(&[Scope::TasksRead]));
        assert!(a.intersection(b).is_subset_of(a) && a.intersection(b).is_subset_of(b));
        assert!(!a.is_subset_of(b));
        assert!(ScopeSet::EMPTY.is_subset_of(a) && ScopeSet::EMPTY.is_empty());
        assert!(a.is_subset_of(a));
        assert_eq!(
            a.without(Scope::TasksRead),
            ScopeSet::from_scopes(&[Scope::ProjectsRead])
        );
        assert_eq!(ScopeSet::all().iter().count(), Scope::ALL.len());
        assert_eq!(
            ScopeSet::from_names(a.to_names().iter().map(String::as_str)).unwrap(),
            a
        );
    }

    #[test]
    fn access_token_scopes_are_the_grant_narrowed_to_the_resource() {
        let grant =
            ScopeSet::parse("projects:read work_items:read notifications:read tasks:read forms:write offline_access")
                .unwrap();
        let api = grant.narrow_to_resource(TokenResource::Api);
        assert_eq!(
            api.to_scope_string(),
            "projects:read work_items:read forms:write notifications:read tasks:read"
        );
        assert!(!api.contains(Scope::OfflineAccess));
        let mcp = grant.narrow_to_resource(TokenResource::Mcp);
        assert_eq!(mcp.to_scope_string(), "projects:read work_items:read forms:write");
        // Nothing in common leaves an empty set, which the token endpoint answers `invalid_scope`.
        let notifications_only = ScopeSet::parse("notifications:read offline_access").unwrap();
        assert!(notifications_only.narrow_to_resource(TokenResource::Mcp).is_empty());
        assert_eq!(
            TokenResource::Mcp.scopes_supported().to_scope_string(),
            "projects:read work_items:read work_items:write forms:read forms:write"
        );
        assert_eq!(TokenResource::Api.scopes_supported().iter().count(), 9);
        assert!(!TokenResource::Api.scopes_supported().contains(Scope::OfflineAccess));
    }

    #[test]
    fn a_refresh_request_may_only_narrow() {
        let grant = ScopeSet::parse("projects:read work_items:read tasks:read").unwrap();
        let narrower = ScopeSet::parse("tasks:read").unwrap();
        assert_eq!(grant.narrow_to_subset(narrower), Ok(narrower));
        assert_eq!(grant.narrow_to_subset(grant), Ok(grant));
        assert_eq!(
            grant.narrow_to_subset(ScopeSet::parse("tasks:read forms:read").unwrap()),
            Err(ScopeError::NotSubset)
        );
        assert_eq!(grant.narrow_to_subset(ScopeSet::EMPTY), Err(ScopeError::Empty));
        // A write scope is not covered by the read scope it implies, in this direction either.
        let read_only = ScopeSet::parse("work_items:read").unwrap();
        assert_eq!(
            read_only.narrow_to_subset(ScopeSet::parse("work_items:write").unwrap()),
            Err(ScopeError::NotSubset)
        );
    }

    #[test]
    fn resources_and_project_scopes() {
        assert_eq!(TokenResource::from_name("api"), Some(TokenResource::Api));
        assert_eq!(TokenResource::from_name("mcp"), Some(TokenResource::Mcp));
        assert_eq!(TokenResource::from_name("flow"), None);
        assert_eq!(TokenResource::Mcp.as_str(), "mcp");
        let id = Uuid::new_v4();
        assert!(ProjectScope::All.contains(id));
        let selected = ProjectScope::Selected(Arc::from([id].as_slice()));
        assert!(selected.contains(id) && !selected.contains(Uuid::new_v4()));
        assert!(!ProjectScope::Selected(Arc::from(Vec::<Uuid>::new())).contains(id));
    }
}
