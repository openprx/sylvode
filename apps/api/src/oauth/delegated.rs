//! Delegated access: the third branch of the API's authentication middleware.
//!
//! A request that carries an `sva_` access token is authenticated here, after the checks that
//! identify the two older token kinds (`opr_` bot tokens and user JWTs) have failed to claim it, so
//! a delegated token can never reach another path and another token can never reach this one.
//!
//! The decision order, each step failing closed:
//!
//! 1. the feature must be configured, the token must come from the `Authorization` header, and no
//!    `access_token` query parameter may be present;
//! 2. [`validate_access_token`] re-reads the token, grant, client, user, membership and workspace
//!    setting from the database (so revocation, removal and disabling take effect on the next
//!    request);
//! 3. the per-grant rate limit (in process, checked before any further database work so a client
//!    that only collects refusals is bounded as well);
//! 4. the route: the request's *matched path pattern* and method must be in [`DELEGATED_ROUTES`]
//!    and the token's scopes must satisfy the entry. **A route that is not in the table is
//!    refused** whatever the user could do themselves: the table is the only way in, so a route
//!    added to the API later is denied until somebody lists it here;
//! 5. the target: the workspace and project of the object the request names are resolved with
//!    parameterized SQL **before the handler runs**; an object outside the grant is refused with
//!    the same answer as an object that does not exist, so a refused request has no side effect and
//!    reveals nothing;
//! 6. the context: the user's claims, the [`DelegatedAuthContext`] and, for the duration of the
//!    handler, the request scope that [`current`] reads;
//! 7. one `oauth_operation_logs` row after the handler, fail closed like the bot path.
//!
//! # Why a task local
//!
//! Handlers rebuild their own `Extensions` bag from the claims and the bot context only, and many
//! call `require_workspace_access*` with it. A delegated marker that travelled in extensions would
//! be dropped by every such handler, and the role cap would silently not apply. The request scope
//! is therefore a task local set around `next.run`: the central permission helpers and the few
//! handlers that filter or guard read it with [`current`], and a code path that does not run on the
//! request task (a spawned task) sees no delegation, which for the role cap and the dispatch
//! guard is the stricter reading of "not delegated" only where the spawn does no authorization.
//! Handlers that authorize do so before they spawn anything.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::rejection::ExtensionRejection;
use axum::extract::{ConnectInfo, FromRequestParts, MatchedPath, RawPathParams, Request, State};
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use parking_lot::Mutex;
use platform::app::AppState;
use platform::auth::{JwtClaims, TokenType};
use platform::config::OAuthConfig;
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

use super::audit::write_token_rejected_sampled;
use super::error::OAuthBearerError;
use super::grants::record_use;
use super::metadata::api_resource_metadata_url;
use super::rate::{GrantRateLimiter, client_address};
use super::scopes::{ProjectScope, Scope, ScopeSet, TokenResource};
use super::validate::{
    AudienceRule, BearerFailure, FORWARD_AUTH_HEADER, FORWARDED_BY_HEADER, ForwardCredential, TokenSource,
    ValidatedToken, check_source, query_carries_access_token, validate_access_token,
};
use crate::error::ApiError;
use crate::middleware::bot_auth::EventSurface;
use crate::response::{ApiResponse, OperationResponseMeta};
use crate::services::bot_identity::BOT_ENTITY_TYPE;

/// The message of the refusal for agent dispatch. The scope it names is reserved and not grantable
/// in this release.
pub const DISPATCH_REFUSED_MESSAGE: &str = "starting or assigning an agent requires the tasks:dispatch scope, which is not available to connected applications";

const ROUTE_REFUSED_MESSAGE: &str = "route not available to connected applications";
const TARGET_REFUSED_MESSAGE: &str = "the requested object is not available to this connection";
const INVALID_TOKEN_MESSAGE: &str = "the access token is invalid or has expired";
const QUERY_TOKEN_MESSAGE: &str = "access tokens are accepted only in the Authorization header";
const BAD_ATTRIBUTION_MESSAGE: &str = "conflicting attribution headers";

/// How often the use of one token is written back (`last_used_at` on the token and its grant).
///
/// The stamp is a write to two rows; once a minute per token keeps a busy client from turning
/// every read into a write while still telling the owner, to the minute, when a connection was
/// last used.
pub const USE_RECORD_INTERVAL: Duration = Duration::from_mins(1);

const MAX_TRACKED_TOKENS: usize = 10_000;

// ---------------------------------------------------------------------------------------------
// Configuration access
// ---------------------------------------------------------------------------------------------

/// A request extension that replaces the process configuration for one request.
///
/// Nothing in production sets it: the server reads the installed `[oauth]` section. A router
/// wrapped in `Extension(OAuthConfigOverride(..))` lets a test run the real middleware against a
/// chosen configuration without installing process-wide state.
#[derive(Clone, Debug)]
pub struct OAuthConfigOverride(pub Arc<OAuthConfig>);

/// The `[oauth]` configuration in effect for a request, or `None` when the feature is off.
pub fn active_config(extensions: &Extensions) -> Option<Arc<OAuthConfig>> {
    extensions
        .get::<OAuthConfigOverride>()
        .map(|config| Arc::clone(&config.0))
        .or_else(|| crate::config::runtime().oauth.clone())
}

// ---------------------------------------------------------------------------------------------
// The route allow-list
// ---------------------------------------------------------------------------------------------

/// What a route needs from the token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Required {
    /// This scope (or one that implies it).
    One(Scope),
    /// Any one of these.
    AnyOf(&'static [Scope]),
    /// Any scope at all: the route only describes the connection itself.
    AnyScope,
}

impl Required {
    pub fn satisfied_by(self, scopes: ScopeSet) -> bool {
        match self {
            Self::One(scope) => scopes.allows(scope),
            Self::AnyOf(any) => any.iter().any(|scope| scopes.allows(*scope)),
            Self::AnyScope => !scopes.is_empty(),
        }
    }

    /// The full set a challenge names, hierarchy included.
    pub fn challenge(self) -> Option<ScopeSet> {
        match self {
            Self::One(scope) => Some(ScopeSet::challenge_for(scope)),
            Self::AnyOf(any) => Some(
                any.iter()
                    .fold(ScopeSet::EMPTY, |set, scope| set.union(ScopeSet::challenge_for(*scope))),
            ),
            Self::AnyScope => None,
        }
    }
}

/// How the object a route names is found. The string is the path parameter that holds its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolver {
    /// The workspace is the path value.
    Workspace(&'static str),
    Project(&'static str),
    Issue(&'static str),
    /// `PREFIX-N` within the grant's workspace.
    IssueIdentifier(&'static str),
    /// A notification of the grant's user.
    Notification(&'static str),
    /// The grant's own workspace; the handler filters by the grant.
    SelfOnly,
    /// The grant itself.
    GrantSelf,
}

/// One allowed method and path pattern.
#[derive(Clone, Copy, Debug)]
pub struct RouteEntry {
    pub method: &'static str,
    /// The router's matched path template, exactly as registered.
    pub pattern: &'static str,
    pub required: Required,
    pub resolver: Resolver,
}

const READ_PROJECTS: Required = Required::One(Scope::ProjectsRead);
const READ_ITEMS: Required = Required::One(Scope::WorkItemsRead);
const WRITE_ITEMS: Required = Required::One(Scope::WorkItemsWrite);
const READ_NOTIFICATIONS: Required = Required::One(Scope::NotificationsRead);
const WRITE_NOTIFICATIONS: Required = Required::One(Scope::NotificationsWrite);
const READ_TASKS: Required = Required::One(Scope::TasksRead);
const STREAM_SCOPES: Required = Required::AnyOf(&[Scope::NotificationsRead, Scope::TasksRead]);

const fn entry(method: &'static str, pattern: &'static str, required: Required, resolver: Resolver) -> RouteEntry {
    RouteEntry {
        method,
        pattern,
        required,
        resolver,
    }
}

/// The allow-list. A method and path pattern that is not here is refused for a delegated
/// principal.
///
/// Patterns are copied from `main.rs`. Forms and the form-record routes, the batch label route, the
/// project label list and search are absent on purpose: their handlers either are bot-only or
/// decide permissions or results through paths that do not read the delegated grant, so they stay
/// denied until each is made delegation-aware.
pub static DELEGATED_ROUTES: &[RouteEntry] = &[
    entry(
        "GET",
        "/api/v1/workspaces/{workspace_id}/projects",
        READ_PROJECTS,
        Resolver::Workspace("workspace_id"),
    ),
    entry("GET", "/api/v1/projects/{id}", READ_PROJECTS, Resolver::Project("id")),
    entry(
        "GET",
        "/api/v1/projects/{project_id}/context",
        READ_PROJECTS,
        Resolver::Project("project_id"),
    ),
    entry(
        "GET",
        "/api/v1/projects/{project_id}/agent-policy",
        Required::AnyScope,
        Resolver::Project("project_id"),
    ),
    entry(
        "GET",
        "/api/v1/projects/{project_id}/issues",
        READ_ITEMS,
        Resolver::Project("project_id"),
    ),
    entry(
        "POST",
        "/api/v1/projects/{project_id}/issues",
        WRITE_ITEMS,
        Resolver::Project("project_id"),
    ),
    entry("GET", "/api/v1/issues/{id}", READ_ITEMS, Resolver::Issue("id")),
    entry("PUT", "/api/v1/issues/{id}", WRITE_ITEMS, Resolver::Issue("id")),
    entry(
        "GET",
        "/api/v1/issues/by-identifier/{identifier}",
        READ_ITEMS,
        Resolver::IssueIdentifier("identifier"),
    ),
    entry(
        "GET",
        "/api/v1/issues/{issue_id}/comments",
        READ_ITEMS,
        Resolver::Issue("issue_id"),
    ),
    entry(
        "POST",
        "/api/v1/issues/{issue_id}/comments",
        WRITE_ITEMS,
        Resolver::Issue("issue_id"),
    ),
    entry(
        "GET",
        "/api/v1/issues/{issue_id}/activities",
        READ_ITEMS,
        Resolver::Issue("issue_id"),
    ),
    entry(
        "GET",
        "/api/v1/issues/{issue_id}/labels",
        READ_ITEMS,
        Resolver::Issue("issue_id"),
    ),
    entry(
        "POST",
        "/api/v1/issues/{issue_id}/labels/{label_id}",
        WRITE_ITEMS,
        Resolver::Issue("issue_id"),
    ),
    entry(
        "DELETE",
        "/api/v1/issues/{issue_id}/labels/{label_id}",
        WRITE_ITEMS,
        Resolver::Issue("issue_id"),
    ),
    entry("GET", "/api/v1/notifications", READ_NOTIFICATIONS, Resolver::SelfOnly),
    entry(
        "GET",
        "/api/v1/notifications/unread-count",
        READ_NOTIFICATIONS,
        Resolver::SelfOnly,
    ),
    entry(
        "PATCH",
        "/api/v1/notifications/{id}/read",
        WRITE_NOTIFICATIONS,
        Resolver::Notification("id"),
    ),
    entry(
        "PUT",
        "/api/v1/notifications/{id}/read",
        WRITE_NOTIFICATIONS,
        Resolver::Notification("id"),
    ),
    entry(
        "PATCH",
        "/api/v1/notifications/read-all",
        WRITE_NOTIFICATIONS,
        Resolver::SelfOnly,
    ),
    entry(
        "PUT",
        "/api/v1/notifications/read-all",
        WRITE_NOTIFICATIONS,
        Resolver::SelfOnly,
    ),
    entry(
        "GET",
        "/api/v1/projects/{project_id}/ai/tasks",
        READ_TASKS,
        Resolver::Project("project_id"),
    ),
    entry(
        "GET",
        "/api/v1/oauth/connection",
        Required::AnyScope,
        Resolver::GrantSelf,
    ),
    entry("GET", "/api/v1/oauth/events/stream", STREAM_SCOPES, Resolver::GrantSelf),
];

/// The entry for a method and the router's matched path pattern.
///
/// The pattern is compared as a whole string: `/api/v1/issues/{id}` never matches the raw URL
/// `/api/v1/issues/123`, so a client cannot choose how it is matched by how it spells its path.
pub fn lookup(method: &Method, pattern: &str) -> Option<&'static RouteEntry> {
    DELEGATED_ROUTES
        .iter()
        .find(|route| route.method == method.as_str() && route.pattern == pattern)
}

// ---------------------------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------------------------

/// Who a delegated request acts for and what it may reach. Inserted into the request extensions
/// and, for the handler's duration, into the request scope read by [`current`].
#[derive(Clone, Debug)]
pub struct DelegatedAuthContext {
    pub grant_id: Uuid,
    pub token_id: Uuid,
    pub client_id: Arc<str>,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub scopes: ScopeSet,
    pub projects: ProjectScope,
    pub resource: TokenResource,
    /// `McpHttp` when the MCP server forwarded the request under its credential, else `Rest`.
    pub surface: EventSurface,
    /// Only when forwarded; validated like the bot path's tool name.
    pub tool_name: Option<String>,
    pub request_id: Uuid,
}

pub fn extract_delegated_context(extensions: &Extensions) -> Option<&DelegatedAuthContext> {
    extensions.get::<DelegatedAuthContext>()
}

struct RequestScope {
    context: Arc<DelegatedAuthContext>,
    dispatch_refused: AtomicBool,
}

tokio::task_local! {
    static ACTIVE: Arc<RequestScope>;
}

/// The delegated context of the request running on this task, if it is a delegated request.
pub fn current() -> Option<Arc<DelegatedAuthContext>> {
    ACTIVE.try_with(|scope| Arc::clone(&scope.context)).ok()
}

/// Runs `future` as a delegated request of `context` would run its handler. For tests of code that
/// reads [`current`] without going through the middleware.
#[cfg(test)]
pub(crate) async fn with_context<F: std::future::Future>(context: DelegatedAuthContext, future: F) -> F::Output {
    let scope = Arc::new(RequestScope {
        context: Arc::new(context),
        dispatch_refused: AtomicBool::new(false),
    });
    ACTIVE.scope(scope, future).await
}

/// Refuses a delegated request that would start, assign or re-dispatch an agent.
///
/// `user_ids` are the people a write names (the new assignee, the mentioned users). If any of them
/// is a bot the call fails before the caller has written anything, and the middleware turns the
/// response into `403 insufficient_scope`. Not delegated: always `Ok`.
pub async fn refuse_agent_dispatch(db: &impl ConnectionTrait, user_ids: &[Uuid]) -> Result<(), ApiError> {
    let Some(scope) = ACTIVE.try_with(Arc::clone).ok() else {
        return Ok(());
    };
    if user_ids.is_empty() {
        return Ok(());
    }
    let named_bot = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM users WHERE id = ANY($1) AND entity_type = $2 LIMIT 1",
            vec![user_ids.to_vec().into(), BOT_ENTITY_TYPE.into()],
        ))
        .await?
        .is_some();
    if named_bot {
        scope.dispatch_refused.store(true, Ordering::Release);
        return Err(ApiError::Forbidden(DISPATCH_REFUSED_MESSAGE.to_string()));
    }
    Ok(())
}

/// The workspace a bot-or-delegated request works in, for handlers that were bot-only.
///
/// A delegated request supplies the grant's workspace (its project set was enforced before the
/// handler); a bot supplies its own. A request with neither fails exactly as the plain
/// `Extension<BotAuthContext>` extractor did.
pub struct CallerWorkspace(pub Uuid);

impl<S: Send + Sync> FromRequestParts<S> for CallerWorkspace {
    type Rejection = ExtensionRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(context) = parts.extensions.get::<DelegatedAuthContext>() {
            return Ok(Self(context.workspace_id));
        }
        let axum::Extension(bot) =
            axum::Extension::<crate::middleware::bot_auth::BotAuthContext>::from_request_parts(parts, state).await?;
        Ok(Self(bot.workspace_id))
    }
}

// ---------------------------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------------------------

/// The project of a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectRef {
    /// The target is not bound to a project; a handler filter limits what it returns.
    NotProjectBound,
    Project(Uuid),
    /// The object belongs to a workspace but no project can be found for it. Only a grant for all
    /// projects reaches it.
    Unresolvable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub workspace_id: Uuid,
    pub project: ProjectRef,
}

impl Target {
    /// Whether `token` reaches the target: its workspace, and its project when the grant is
    /// limited to some.
    pub fn is_reachable_by(&self, workspace_id: Uuid, projects: &ProjectScope) -> bool {
        self.workspace_id == workspace_id
            && match self.project {
                ProjectRef::NotProjectBound => true,
                ProjectRef::Project(id) => projects.contains(id),
                ProjectRef::Unresolvable => matches!(projects, ProjectScope::All),
            }
    }
}

/// The grant facts a resolver needs.
#[derive(Clone, Copy, Debug)]
pub struct Grantee {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn uuid_param(params: &[(String, String)], name: &str) -> Option<Uuid> {
    Uuid::parse_str(param(params, name)?).ok()
}

#[derive(Debug, FromQueryResult)]
struct WorkspaceProject {
    workspace_id: Uuid,
    project_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct NotificationOwner {
    owner: Uuid,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
}

/// Splits `PREFIX-N` the way the handler does.
pub fn parse_identifier(identifier: &str) -> Option<(&str, i64)> {
    let (prefix, sequence) = identifier.rsplit_once('-')?;
    if prefix.is_empty() {
        return None;
    }
    Some((prefix, sequence.parse().ok()?))
}

/// Finds the workspace and project the request names. `Ok(None)` means there is no such object
/// (or the path value is malformed); the caller answers it exactly like an object outside the
/// grant.
pub async fn resolve_target(
    db: &DatabaseConnection,
    resolver: Resolver,
    params: &[(String, String)],
    grantee: Grantee,
) -> Result<Option<Target>, DbErr> {
    let target = |row: WorkspaceProject| Target {
        workspace_id: row.workspace_id,
        project: row.project_id.map_or(ProjectRef::Unresolvable, ProjectRef::Project),
    };
    match resolver {
        Resolver::Workspace(name) => Ok(uuid_param(params, name).map(|workspace_id| Target {
            workspace_id,
            project: ProjectRef::NotProjectBound,
        })),
        Resolver::SelfOnly | Resolver::GrantSelf => Ok(Some(Target {
            workspace_id: grantee.workspace_id,
            project: ProjectRef::NotProjectBound,
        })),
        Resolver::Project(name) => {
            let Some(project_id) = uuid_param(params, name) else {
                return Ok(None);
            };
            Ok(WorkspaceProject::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT p.workspace_id, p.id AS project_id FROM projects p WHERE p.id = $1",
                vec![project_id.into()],
            ))
            .one(db)
            .await?
            .map(target))
        }
        Resolver::Issue(name) => {
            let Some(issue_id) = uuid_param(params, name) else {
                return Ok(None);
            };
            Ok(WorkspaceProject::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT p.workspace_id, w.project_id FROM work_items w \
                 JOIN projects p ON p.id = w.project_id WHERE w.id = $1",
                vec![issue_id.into()],
            ))
            .one(db)
            .await?
            .map(target))
        }
        Resolver::IssueIdentifier(name) => {
            let Some((prefix, sequence)) = param(params, name).and_then(parse_identifier) else {
                return Ok(None);
            };
            Ok(WorkspaceProject::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT p.workspace_id, wi.project_id FROM work_items wi \
                 JOIN projects p ON p.id = wi.project_id \
                 WHERE p.workspace_id = $1 AND UPPER(p.key) = UPPER($2) AND wi.sequence_number = $3",
                vec![grantee.workspace_id.into(), prefix.into(), sequence.into()],
            ))
            .one(db)
            .await?
            .map(target))
        }
        Resolver::Notification(name) => {
            let Some(notification_id) = uuid_param(params, name) else {
                return Ok(None);
            };
            let row = NotificationOwner::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT n.user_id AS owner, n.workspace_id, \
                        COALESCE(n.related_project_id, \
                                 (SELECT w.project_id FROM work_items w WHERE w.id = n.related_issue_id)) AS project_id \
                 FROM notifications n WHERE n.id = $1",
                vec![notification_id.into()],
            ))
            .one(db)
            .await?;
            // Another user's notification and a notification without a workspace are both "not
            // available"; neither is distinguishable from a missing one.
            Ok(row.filter(|row| row.owner == grantee.user_id).and_then(|row| {
                Some(Target {
                    workspace_id: row.workspace_id?,
                    project: row.project_id.map_or(ProjectRef::Unresolvable, ProjectRef::Project),
                })
            }))
        }
    }
}

/// A project a grant can see, with its display data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReachableProject {
    pub key: String,
    pub name: String,
}

/// Which of `project_ids` a grant can reach right now, in one query.
///
/// This is the single definition of project visibility for a grant at the time something is
/// delivered or listed outside the request path (the event stream asks it per batch). It states
/// the same rules as the request path: [`Target::is_reachable_by`] (the project belongs to the
/// grant's workspace and is in its project set) and the membership the token validation requires
/// (the user is still a member of that workspace). In Sylvode a project is visible to every
/// member of its workspace and the role cap (an owner or admin acts as `member`) does not change
/// what a member may read, so no role enters the question. A project that does not exist is
/// absent from the answer, exactly as an unresolvable target is refused.
pub async fn reachable_projects(
    db: &DatabaseConnection,
    grantee: Grantee,
    scope: &ProjectScope,
    project_ids: &[Uuid],
) -> Result<HashMap<Uuid, ReachableProject>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        key: String,
        name: String,
    }
    if project_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let (all, selected): (bool, Vec<Uuid>) = match scope {
        ProjectScope::All => (true, Vec::new()),
        ProjectScope::Selected(ids) => (false, ids.to_vec()),
    };
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.id, p.key, p.name FROM projects p \
         WHERE p.id = ANY($1) AND p.workspace_id = $2 \
           AND ($3 OR p.id = ANY($4)) \
           AND EXISTS (SELECT 1 FROM workspace_members m WHERE m.workspace_id = p.workspace_id AND m.user_id = $5)",
        vec![
            project_ids.to_vec().into(),
            grantee.workspace_id.into(),
            all.into(),
            selected.into(),
            grantee.user_id.into(),
        ],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.id,
                ReachableProject {
                    key: row.key,
                    name: row.name,
                },
            )
        })
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Process-wide state: rate limits and the use stamp
// ---------------------------------------------------------------------------------------------

static GRANT_LIMITER: LazyLock<GrantRateLimiter> = LazyLock::new(GrantRateLimiter::delegated_default);
/// The internal token-context call has its own budget, so the lookups the MCP server makes per
/// tool call do not spend the grant's request budget.
pub(super) static CONTEXT_LIMITER: LazyLock<GrantRateLimiter> = LazyLock::new(GrantRateLimiter::delegated_default);

/// Remembers when a token's use was last written back.
pub struct UseThrottle {
    last: Mutex<HashMap<Uuid, Instant>>,
}

impl UseThrottle {
    pub fn new() -> Self {
        Self {
            last: Mutex::new(HashMap::new()),
        }
    }

    /// Whether the use of `token_id` should be written now. At most once per
    /// [`USE_RECORD_INTERVAL`] per token.
    pub fn should_record(&self, token_id: Uuid, now: Instant) -> bool {
        let mut last = self.last.lock();
        if let Some(previous) = last.get(&token_id)
            && now.saturating_duration_since(*previous) < USE_RECORD_INTERVAL
        {
            return false;
        }
        if last.len() >= MAX_TRACKED_TOKENS {
            last.retain(|_, seen| now.saturating_duration_since(*seen) < USE_RECORD_INTERVAL);
        }
        if last.len() < MAX_TRACKED_TOKENS {
            last.insert(token_id, now);
        }
        true
    }
}

impl Default for UseThrottle {
    fn default() -> Self {
        Self::new()
    }
}

static USE_THROTTLE: LazyLock<UseThrottle> = LazyLock::new(UseThrottle::new);

/// Stamps the use of a token, at most once a minute per token, without delaying the request. A
/// failure is logged and never changes the answer.
pub(super) fn note_use(db: &DatabaseConnection, token_id: Uuid, grant_id: Uuid) {
    if !USE_THROTTLE.should_record(token_id, Instant::now()) {
        return;
    }
    let db = db.clone();
    tokio::spawn(async move {
        if let Err(error) = record_use(&db, token_id, grant_id).await {
            tracing::warn!(
                token_id = %token_id,
                grant_id = %grant_id,
                error = %error,
                "could not record the use of a delegated token"
            );
        }
    });
}

// ---------------------------------------------------------------------------------------------
// The operation log
// ---------------------------------------------------------------------------------------------

struct LogRow {
    grant_id: Uuid,
    client_id: Arc<str>,
    user_id: Uuid,
    workspace_id: Uuid,
    surface: EventSurface,
    tool_name: Option<String>,
    method: String,
    path: String,
    request_id: Uuid,
}

async fn write_operation_log(
    db: &DatabaseConnection,
    row: &LogRow,
    business_code: i32,
    error_message: Option<&'static str>,
    duration_ms: i64,
) -> Result<(), DbErr> {
    let outcome = if business_code == 0 { "ok" } else { "error" };
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO oauth_operation_logs \
         (id, grant_id, client_id, user_id, workspace_id, surface, tool_name, method, path, \
          business_code, outcome, error_message, duration_ms, request_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        vec![
            Uuid::new_v4().into(),
            row.grant_id.into(),
            row.client_id.to_string().into(),
            row.user_id.into(),
            row.workspace_id.into(),
            row.surface.as_wire().into(),
            row.tool_name.clone().into(),
            row.method.clone().into(),
            row.path.clone().into(),
            business_code.into(),
            outcome.into(),
            error_message.map(str::to_owned).into(),
            duration_ms.into(),
            row.request_id.into(),
        ],
    ))
    .await
    .map(|_| ())
}

fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------------------------------
// The middleware branch
// ---------------------------------------------------------------------------------------------

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// A refusal with the challenge's resource metadata URL attached.
fn refusal(error: OAuthBearerError, metadata_url: Option<&str>) -> Response {
    match metadata_url {
        Some(url) => error.with_resource_metadata(url),
        None => error,
    }
    .into_response()
}

/// Writes the sampled `token_rejected` audit row for a refused token. Observation only: a failure
/// is logged and the refusal stands.
async fn audit_rejection(
    db: &DatabaseConnection,
    failure: &BearerFailure,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) {
    if !failure.reason.is_audited() {
        return;
    }
    let mut record = failure.audit_record().with_client_ip(client_address(headers, peer));
    if let Some(agent) = header_text(headers, "user-agent") {
        record = record.with_user_agent(agent);
    }
    if let Err(error) = write_token_rejected_sampled(db, &record).await {
        tracing::warn!(error = %error, "could not write the token_rejected audit row");
    }
}

async fn path_params(parts: &mut Parts) -> Vec<(String, String)> {
    RawPathParams::from_request_parts(parts, &())
        .await
        .map(|raw| {
            raw.iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Refuses a request before its handler and leaves a best-effort log row: the request is refused
/// either way, so a failed write is logged and the refusal stands.
async fn refuse_logged(
    db: &DatabaseConnection,
    row: &LogRow,
    started: Instant,
    error: OAuthBearerError,
    code: i32,
    summary: &'static str,
    metadata_url: Option<&str>,
) -> Response {
    if let Err(log_error) = write_operation_log(db, row, code, Some(summary), elapsed_ms(started)).await {
        tracing::error!(
            request_id = %row.request_id,
            error = %log_error,
            "delegated operation log write failed for a refused request"
        );
    }
    refusal(error, metadata_url)
}

/// Authenticates and runs a request that carries an `sva_` token. See the module documentation.
#[allow(clippy::too_many_lines, reason = "the decision order is one straight sequence")]
pub async fn handle(state: &AppState, req: Request, next: Next, token: &str, source: TokenSource) -> Response {
    let started = Instant::now();
    let Some(config) = active_config(req.extensions()) else {
        // The feature is not configured: no delegated token can be valid.
        return refusal(OAuthBearerError::invalid_token(INVALID_TOKEN_MESSAGE), None);
    };
    let metadata_url = api_resource_metadata_url(&config);
    let metadata_url = metadata_url.as_deref();

    if let Err(failure) = check_source(source) {
        return refusal(failure.to_error(metadata_url), metadata_url);
    }
    if req.uri().query().is_some_and(query_carries_access_token) {
        return refusal(OAuthBearerError::invalid_request(QUERY_TOKEN_MESSAGE), metadata_url);
    }

    let (mut parts, body) = req.into_parts();
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
    let credential = ForwardCredential::new(
        config.internal_forward_secret.as_ref(),
        header_text(&parts.headers, FORWARDED_BY_HEADER),
        header_text(&parts.headers, FORWARD_AUTH_HEADER),
    );
    let forwarded = credential.is_valid();

    let validated: ValidatedToken = match validate_access_token(&state.db, token, AudienceRule::Api(credential)).await {
        Ok(validated) => validated,
        Err(failure) => {
            audit_rejection(&state.db, &failure, &parts.headers, peer).await;
            return refusal(failure.to_error(metadata_url), metadata_url);
        }
    };

    let request_id = Uuid::new_v4();
    let method = parts.method.clone();
    let mut log_row = LogRow {
        grant_id: validated.grant_id,
        client_id: Arc::clone(&validated.client_id),
        user_id: validated.user_id,
        workspace_id: validated.workspace_id,
        surface: EventSurface::Rest,
        tool_name: None,
        method: method.as_str().to_owned(),
        path: parts.uri.path().to_owned(),
        request_id,
    };

    if let Err(limited) = GRANT_LIMITER.check(validated.grant_id, Instant::now()) {
        return refusal(OAuthBearerError::from(limited), metadata_url);
    }

    // Route: the matched pattern and the method, nothing else.
    let matched = parts
        .extensions
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let Some(route) = matched.as_deref().and_then(|pattern| lookup(&method, pattern)) else {
        return refuse_logged(
            &state.db,
            &log_row,
            started,
            OAuthBearerError::insufficient_scope(ROUTE_REFUSED_MESSAGE, None),
            403,
            "route not available",
            metadata_url,
        )
        .await;
    };
    if !route.required.satisfied_by(validated.scopes) {
        return refuse_logged(
            &state.db,
            &log_row,
            started,
            OAuthBearerError::insufficient_scope(
                "the connection does not include the required scope",
                route.required.challenge(),
            ),
            403,
            "insufficient scope",
            metadata_url,
        )
        .await;
    }

    // Target: resolved before the handler, refused identically whether absent or outside the grant.
    let params = path_params(&mut parts).await;
    let grantee = Grantee {
        user_id: validated.user_id,
        workspace_id: validated.workspace_id,
    };
    let target = match resolve_target(&state.db, route.resolver, &params, grantee).await {
        Ok(target) => target,
        Err(error) => {
            tracing::error!(request_id = %request_id, error = %error, "delegated target lookup failed");
            return refusal(OAuthBearerError::server_error(), metadata_url);
        }
    };
    if !target.is_some_and(|target| target.is_reachable_by(validated.workspace_id, &validated.projects)) {
        return refuse_logged(
            &state.db,
            &log_row,
            started,
            OAuthBearerError::forbidden(TARGET_REFUSED_MESSAGE),
            403,
            "target not available",
            metadata_url,
        )
        .await;
    }

    // Attribution: only a request that arrived under the MCP server's credential may name a tool.
    let mut tool_name = None;
    if forwarded && validated.resource == TokenResource::Mcp {
        match crate::middleware::bot_auth::forwarded_tool_name(&parts.headers) {
            Ok(tool) => {
                log_row.surface = EventSurface::McpHttp;
                tool_name = tool;
            }
            Err(_) => {
                return refuse_logged(
                    &state.db,
                    &log_row,
                    started,
                    OAuthBearerError::invalid_request(BAD_ATTRIBUTION_MESSAGE),
                    400,
                    "bad request",
                    metadata_url,
                )
                .await;
            }
        }
    }
    log_row.tool_name.clone_from(&tool_name);

    let context = Arc::new(DelegatedAuthContext {
        grant_id: validated.grant_id,
        token_id: validated.token_id,
        client_id: Arc::clone(&validated.client_id),
        user_id: validated.user_id,
        workspace_id: validated.workspace_id,
        scopes: validated.scopes,
        projects: validated.projects.clone(),
        resource: validated.resource,
        surface: log_row.surface,
        tool_name,
        request_id,
    });
    parts.extensions.insert(JwtClaims {
        sub: validated.user_id.to_string(),
        email: validated.user_email.to_string(),
        token_type: TokenType::Access,
        iat: 0,
        exp: usize::try_from(validated.expires_at.unix_timestamp()).unwrap_or(0),
        sv: validated.user_session_version,
    });
    parts.extensions.insert((*context).clone());
    note_use(&state.db, validated.token_id, validated.grant_id);

    let scope = Arc::new(RequestScope {
        context,
        dispatch_refused: AtomicBool::new(false),
    });
    let response = ACTIVE
        .scope(Arc::clone(&scope), next.run(Request::from_parts(parts, body)))
        .await;

    // A guard refused an agent dispatch: the answer is the standard scope refusal.
    let response = if scope.dispatch_refused.load(Ordering::Acquire) {
        refusal(
            OAuthBearerError::insufficient_scope(DISPATCH_REFUSED_MESSAGE, None),
            metadata_url,
        )
    } else {
        response
    };

    let meta = response.extensions().get::<OperationResponseMeta>().copied();
    let (business_code, error_message) = meta.map_or_else(
        || {
            if response.status().is_success() {
                (0, None)
            } else {
                (i32::from(response.status().as_u16()), Some("http request rejected"))
            }
        },
        |value| (value.business_code, value.error_summary),
    );
    match write_operation_log(&state.db, &log_row, business_code, error_message, elapsed_ms(started)).await {
        Ok(()) => response,
        Err(error) => {
            tracing::error!(
                request_id = %request_id,
                business_code,
                error = %error,
                "delegated operation log write failed; the operation is not reported as successful without its record"
            );
            if business_code == 0 {
                ApiError::internal_after_operation().into_response()
            } else {
                response
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// GET /api/v1/oauth/connection
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct ConnectionRow {
    workspace_name: String,
    user_name: String,
    user_email: String,
    scopes: Vec<String>,
    project_mode: String,
    consented_at: Timestamp,
    expires_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct ConnectionProject {
    id: Uuid,
    key: String,
    name: String,
}

async fn connection(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let Some(context) = current() else {
        return Err(ApiError::Forbidden(
            "this endpoint is available to connected applications only".to_string(),
        ));
    };
    let row = ConnectionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT w.name AS workspace_name, u.name AS user_name, u.email AS user_email, \
                g.scopes, g.project_mode, g.consented_at, g.expires_at \
         FROM oauth_grants g \
         JOIN workspaces w ON w.id = g.workspace_id \
         JOIN users u ON u.id = g.user_id \
         WHERE g.id = $1",
        vec![context.grant_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("connection not found".to_string()))?;
    let projects = if row.project_mode == "selected" {
        ConnectionProject::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT p.id, p.key, p.name FROM oauth_grant_projects gp \
             JOIN projects p ON p.id = gp.project_id \
             WHERE gp.grant_id = $1 AND p.workspace_id = $2 ORDER BY p.key",
            vec![context.grant_id.into(), context.workspace_id.into()],
        ))
        .all(&state.db)
        .await?
    } else {
        Vec::new()
    };
    Ok(ApiResponse::success(json!({
        "grant_id": context.grant_id,
        "client_id": context.client_id.as_ref(),
        "workspace": {"id": context.workspace_id, "name": row.workspace_name},
        "account": {"user_id": context.user_id, "name": row.user_name, "email": row.user_email},
        "scopes": row.scopes,
        "project_mode": row.project_mode,
        "projects": projects
            .into_iter()
            .map(|p| json!({"id": p.id, "key": p.key, "name": p.name}))
            .collect::<Vec<_>>(),
        "consented_at": row.consented_at.to_rfc3339(),
        "expires_at": row.expires_at.to_rfc3339(),
    })))
}

/// The routes of this package.
///
/// The connection description sits behind the same authentication middleware as every other
/// delegated route and is on the allow-list; the internal token-context call authenticates itself
/// (see [`super::internal`]).
pub fn routes(state: &AppState) -> Router<AppState> {
    Router::new().merge(super::internal::routes()).route(
        "/api/v1/oauth/connection",
        get(connection).route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::bot_auth::bot_or_user_auth_middleware,
        )),
    )
}
