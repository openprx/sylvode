//! Delegated access through the real router.
//!
//! Every test drives `build_router` (the router the server runs) with real requests against a
//! scratch PostgreSQL database that has every migration applied. Delegated tokens are minted the
//! way the token endpoint stores them: grant rows and token rows written directly, with the
//! token's hash. The configuration reaches the middleware through the request extension that
//! exists for that purpose (`OAuthConfigOverride`), so no process-wide state is installed.
//!
//! The cast: user `alice` is `owner` of workspace `ws1` and `admin` of `ws2` (and an instance
//! administrator); `bob` is a member of `ws1`; `carol` of `ws2`; `agent` is a bot in `ws1`.
//! `ws1` has projects `p1` and `p2`, `ws2` has `p3`, and each project has one work item.
#![allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::struct_field_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::migration_runner_database_tests::{Scratch, scratch};
use super::{MigrationOptions, build_router, run_migrations_with};
use api::oauth::delegated::{DELEGATED_ROUTES, OAuthConfigOverride, Required, Resolver, RouteEntry, lookup};
use api::oauth::scopes::{Scope, ScopeSet};
use api::oauth::tokens::{TokenKind, mint};
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use parking_lot::Mutex;
use platform::{
    app::AppState,
    auth::JwtManager,
    config::{AppConfig, OAuthConfig, Secret},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;
use tower::ServiceExt as _;
use uuid::Uuid;

const JWT_SECRET: &str = "delegated-router-test-secret";
const FORWARD_SECRET: &str = "delegated-forward-secret-0123456789abcdef";
const ISSUER: &str = "https://sv.test";
const METADATA_URL: &str = "https://sv.test/.well-known/oauth-protected-resource/api";
const ROUTE_REFUSED: &str = "route not available to connected applications";

/// Routes of the allow-list that another package registers. Remove an entry when that package is
/// merged; until then the table names a route this router does not have yet.
const NOT_REGISTERED_YET: &[(&str, &str)] = &[("GET", "/api/v1/oauth/events/stream")];

/// Every scope a read-and-write connection can hold in the first release's vocabulary, for tests
/// where the scope must not be what limits the request.
const ALL_API_SCOPES: &[&str] = &[
    "projects:read",
    "work_items:read",
    "work_items:write",
    "notifications:read",
    "notifications:write",
    "tasks:read",
];

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

fn oauth_config() -> Arc<OAuthConfig> {
    let mut config = OAuthConfig::with_defaults(ISSUER);
    config.mcp_resource = Some("https://sv.test/mcp".to_string());
    config.internal_forward_secret = Some(Secret::new(FORWARD_SECRET));
    Arc::new(config)
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

#[derive(Debug, FromQueryResult)]
struct Count {
    n: i64,
}

async fn count(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    Count::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(db)
        .await
        .expect("count query runs")
        .map_or(0, |row| row.n)
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }

    /// The OAuth error code of a bearer refusal, if this is one.
    fn bearer_error(&self) -> Option<String> {
        self.json().get("error").and_then(Value::as_str).map(str::to_owned)
    }

    /// The `code` of the API's own envelope (`0` is success).
    fn code(&self) -> Option<i64> {
        self.json().get("code").and_then(Value::as_i64)
    }

    /// Whether the middleware itself refused the request (as opposed to a handler answering).
    fn refused_by_delegation(&self) -> bool {
        self.bearer_error().is_some() || self.header("www-authenticate").is_some()
    }
}

struct Session {
    grant: Uuid,
    token_id: Uuid,
    token: String,
}

struct World {
    scratch: Scratch,
    db: DatabaseConnection,
    router: Router,
    ws1: Uuid,
    ws2: Uuid,
    alice: Uuid,
    bob: Uuid,
    agent: Uuid,
    p1: Uuid,
    p2: Uuid,
    p3: Uuid,
    i1: Uuid,
    i2: Uuid,
    i3: Uuid,
    ident1: String,
    ident2: String,
    ident3: String,
    label1: Uuid,
    n1: Uuid,
    n2: Uuid,
    n3: Uuid,
    n4: Uuid,
    n_without_workspace: Uuid,
    n_bob: Uuid,
}

impl World {
    async fn new(label: &str) -> Option<Self> {
        let Some(scratch) = scratch(label).await else {
            eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
            return None;
        };
        let db = scratch.connection().clone();
        run_migrations_with(&db, MigrationOptions::default())
            .await
            .expect("migrations apply");
        let state = state_for(db.clone());
        let router = build_router(state).layer(axum::Extension(OAuthConfigOverride(oauth_config())));

        let [ws1, ws2, alice, bob, carol, agent, p1, p2, p3, i1, i2, i3, label1] =
            std::array::from_fn(|_| Uuid::new_v4());
        for (id, name, role, entity) in [
            (alice, "alice", "admin", "human"),
            (bob, "bob", "user", "human"),
            (carol, "carol", "user", "human"),
            (agent, "agent", "user", "bot"),
        ] {
            exec(
                &db,
                "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
                 VALUES ($1, $2, '!', $3, $4, true, $5)",
                vec![
                    id.into(),
                    format!("{name}-{id}@router.test").into(),
                    name.to_string().into(),
                    role.into(),
                    entity.into(),
                ],
            )
            .await;
        }
        for (id, slug) in [(ws1, "one"), (ws2, "two")] {
            exec(
                &db,
                "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, $3, $4)",
                vec![
                    id.into(),
                    format!("{slug}-{id}").into(),
                    format!("workspace {slug}").into(),
                    alice.into(),
                ],
            )
            .await;
        }
        for (ws, user, role) in [
            (ws1, alice, "owner"),
            (ws2, alice, "admin"),
            (ws1, bob, "member"),
            (ws2, carol, "member"),
            (ws1, agent, "member"),
        ] {
            exec(
                &db,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
                vec![ws.into(), user.into(), role.into()],
            )
            .await;
        }
        for (id, ws, key) in [(p1, ws1, "AAA"), (p2, ws1, "BBB"), (p3, ws2, "CCC")] {
            exec(
                &db,
                "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
                 VALUES ($1, $2, $3, $4, $5, 'code_project')",
                vec![
                    id.into(),
                    ws.into(),
                    key.into(),
                    format!("project {key}").into(),
                    alice.into(),
                ],
            )
            .await;
        }
        for (id, project, title) in [(i1, p1, "item one"), (i2, p2, "item two"), (i3, p3, "item three")] {
            exec(
                &db,
                "INSERT INTO work_items (id, project_id, title, state, created_by) VALUES ($1, $2, $3, 'todo', $4)",
                vec![id.into(), project.into(), title.into(), alice.into()],
            )
            .await;
        }
        let identifier = |key: &str, db: DatabaseConnection, id: Uuid| {
            let key = key.to_string();
            async move {
                let row = Count::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT sequence_number::bigint AS n FROM work_items WHERE id = $1",
                    vec![id.into()],
                ))
                .one(&db)
                .await
                .expect("sequence number is readable")
                .expect("the work item exists");
                format!("{key}-{}", row.n)
            }
        };
        let ident1 = identifier("AAA", db.clone(), i1).await;
        let ident2 = identifier("BBB", db.clone(), i2).await;
        let ident3 = identifier("CCC", db.clone(), i3).await;
        exec(
            &db,
            "INSERT INTO labels (id, workspace_id, name) VALUES ($1, $2, 'bug')",
            vec![label1.into(), ws1.into()],
        )
        .await;
        for (issue, text) in [(i1, "first comment"), (i2, "second comment")] {
            exec(
                &db,
                "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, $4)",
                vec![Uuid::new_v4().into(), issue.into(), alice.into(), text.into()],
            )
            .await;
        }
        let [n1, n2, n3, n4, n_without_workspace, n_bob] = std::array::from_fn(|_| Uuid::new_v4());
        // (id, owner, workspace, related project, related issue)
        for (id, owner, ws, project, issue) in [
            (n1, alice, Some(ws1), Some(p1), None),
            (n2, alice, Some(ws1), None, Some(i2)),
            (n3, alice, Some(ws2), Some(p3), None),
            (n4, alice, Some(ws1), None, None),
            (n_without_workspace, alice, None, Some(p1), None),
            (n_bob, bob, Some(ws1), Some(p1), None),
        ] {
            exec(
                &db,
                "INSERT INTO notifications (id, user_id, workspace_id, type, kind, payload, title, content, \
                 related_project_id, related_issue_id, is_read) \
                 VALUES ($1, $2, $3, 'mentioned', 'mentioned', '{}'::jsonb, 'title', 'content', $4, $5, false)",
                vec![id.into(), owner.into(), ws.into(), project.into(), issue.into()],
            )
            .await;
        }
        for project in [p1, p2, p3] {
            exec(
                &db,
                "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status) \
                 VALUES ($1, $2, $3, 'issue_assigned', 'pending')",
                vec![Uuid::new_v4().into(), project.into(), agent.into()],
            )
            .await;
        }
        Some(Self {
            scratch,
            db,
            router,
            ws1,
            ws2,
            alice,
            bob,
            agent,
            p1,
            p2,
            p3,
            i1,
            i2,
            i3,
            ident1,
            ident2,
            ident3,
            label1,
            n1,
            n2,
            n3,
            n4,
            n_without_workspace,
            n_bob,
        })
    }

    async fn finish(self) {
        self.scratch.drop_self().await;
    }

    /// A member of `ws1` of its own, so a test can change the user without touching the cast.
    async fn new_member(&self, role: &str) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'extra', 'user', true)",
            vec![id.into(), format!("extra-{id}@router.test").into()],
        )
        .await;
        exec(
            &self.db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
            vec![self.ws1.into(), id.into(), role.into()],
        )
        .await;
        id
    }

    async fn grant(&self, user: Uuid, ws: Uuid, selected: &[Uuid], scopes: &[&str]) -> Uuid {
        let grant = Uuid::new_v4();
        let mode = if selected.is_empty() { "all" } else { "selected" };
        exec(
            &self.db,
            "INSERT INTO oauth_grants (id, client_id, user_id, workspace_id, scopes, resources, project_mode, \
             user_session_version, expires_at) \
             VALUES ($1, 'ramflux', $2, $3, $4, ARRAY['api','mcp'], $5, \
             (SELECT session_version FROM users WHERE id = $2), now() + interval '365 days')",
            vec![
                grant.into(),
                user.into(),
                ws.into(),
                scopes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>().into(),
                mode.into(),
            ],
        )
        .await;
        for project in selected {
            exec(
                &self.db,
                "INSERT INTO oauth_grant_projects (grant_id, project_id) VALUES ($1, $2)",
                vec![grant.into(), (*project).into()],
            )
            .await;
        }
        grant
    }

    async fn token(&self, grant: Uuid, resource: &str, scopes: &[&str]) -> Session {
        let minted = mint(TokenKind::Access).expect("a token is minted");
        let token_id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, expires_at) \
             VALUES ($1, $2, 'access', $3, 'ramflux', $4, $5, now() + interval '10 minutes')",
            vec![
                token_id.into(),
                minted.hash.as_hex().into(),
                grant.into(),
                resource.into(),
                scopes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>().into(),
            ],
        )
        .await;
        Session {
            grant,
            token_id,
            token: minted.raw.expose().to_owned(),
        }
    }

    /// A grant for `user` in `ws` and an API token carrying the same scopes.
    async fn session(&self, user: Uuid, ws: Uuid, selected: &[Uuid], scopes: &[&str]) -> Session {
        let grant = self.grant(user, ws, selected, scopes).await;
        self.token(grant, "api", scopes).await
    }

    async fn send(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        extra: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Reply {
        send_to(&self.router, method, uri, token, extra, body).await
    }

    async fn get(&self, session: &Session, uri: &str) -> Reply {
        self.send("GET", uri, Some(&session.token), &[], None).await
    }

    fn jwt(user: Uuid) -> String {
        JwtManager::new(JWT_SECRET, 900, 3600)
            .issue_access_token(&user.to_string(), &format!("{user}@router.test"))
            .expect("the token is issued")
    }

    fn forward_headers(tool: Option<&'static str>) -> Vec<(&'static str, &'static str)> {
        let mut headers = vec![
            ("x-sylvode-forwarded-by", "mcp-server"),
            ("x-sylvode-forward-auth", FORWARD_SECRET),
        ];
        if let Some(tool) = tool {
            headers.push(("x-sylvode-mcp-tool", tool));
        }
        headers
    }
}

async fn send_to(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    extra: &[(&str, &str)],
    body: Option<&Value>,
) -> Reply {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("the request is well formed");
    let response = router.clone().oneshot(request).await.expect("the router answers");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("the body is readable")
        .to_vec();
    Reply { status, headers, body }
}

fn ids(reply: &Reply, key: &str) -> BTreeSet<Uuid> {
    reply.json()["data"][key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()))
                .collect()
        })
        .unwrap_or_default()
}

fn assert_ok(reply: &Reply, context: &str) {
    assert!(
        !reply.refused_by_delegation(),
        "{context}: refused by delegation: {}",
        reply.text()
    );
    assert_eq!(reply.status, StatusCode::OK, "{context}: {}", reply.text());
    assert_eq!(reply.code(), Some(0), "{context}: {}", reply.text());
}

/// The one answer for a target outside the grant: 403, `forbidden`, no hint of why.
fn assert_target_refused(reply: &Reply, context: &str) {
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{context}: {}", reply.text());
    assert_eq!(
        reply.bearer_error().as_deref(),
        Some("forbidden"),
        "{context}: {}",
        reply.text()
    );
    assert_eq!(
        reply.json()["message"],
        "the requested object is not available to this connection",
        "{context}"
    );
}

// ---------------------------------------------------------------------------------------------
// Route enumeration
// ---------------------------------------------------------------------------------------------

struct RegisteredRoute {
    path: String,
    methods: Vec<&'static str>,
}

/// Every route of the router, read from the router itself: its `Debug` output lists each path
/// (`RouteId(n): "<path>"`) and, per route, which methods carry a handler. A route registered with
/// `any` or as a plain service accepts every method. If axum changes that output the sanity checks
/// below fail loudly instead of the enumeration silently shrinking.
fn registered_routes(router: &Router) -> Vec<RegisteredRoute> {
    let text = format!("{router:?}");
    let routes_marker = "path_router: PathRouter { routes: {";
    let node_marker = "}, node: Node { paths: {";
    let routes_start = text.find(routes_marker).expect("the router lists its routes") + routes_marker.len();
    let routes_end = routes_start
        + text[routes_start..]
            .find(node_marker)
            .expect("the router lists its paths");
    let paths_start = routes_end + node_marker.len();
    let paths_end = paths_start
        + text[paths_start..]
            .find("fallback_router")
            .expect("the router lists its fallback");

    let mut paths = std::collections::BTreeMap::new();
    for piece in text[paths_start..paths_end].split("RouteId(").skip(1) {
        let (id, rest) = piece.split_once("): \"").expect("a path entry");
        let (path, _) = rest.split_once('"').expect("a path ends");
        paths.insert(id.to_owned(), path.to_owned());
    }
    let mut routes = Vec::new();
    for piece in text[routes_start..routes_end].split("RouteId(").skip(1) {
        let (id, entry) = piece.split_once("): ").expect("a route entry");
        let any = !entry.starts_with("MethodRouter(") || entry.contains("fallback: Service");
        let methods: Vec<&'static str> = if any {
            vec!["GET", "POST", "PUT", "PATCH", "DELETE"]
        } else {
            [
                ("get: Route", "GET"),
                ("post: Route", "POST"),
                ("put: Route", "PUT"),
                ("patch: Route", "PATCH"),
                ("delete: Route", "DELETE"),
            ]
            .into_iter()
            .filter(|(needle, _)| entry.contains(needle))
            .map(|(_, method)| method)
            .collect()
        };
        let path = paths.get(id).expect("every route has a path").clone();
        routes.push(RegisteredRoute { path, methods });
    }
    routes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// On the allow-list: the middleware lets a connection with the scope through to the handler.
    Allowed,
    /// Behind the delegated branch and not on the allow-list: refused with `insufficient_scope`.
    RouteDenied,
    /// Behind another authentication middleware (user JWT, administrator): the token is no
    /// credential there and the request is refused.
    OtherAuthDenied,
    /// Needs no bearer credential at all: a delegated token adds nothing to what anyone can do.
    Public,
}

/// Routes that need no bearer credential. Every other route must be allowed or refused, so a new
/// public route fails the enumeration until it is listed here, with the reason.
const PUBLIC_ROUTES: &[(&str, &str)] = &[
    // Liveness and readiness probes.
    ("GET", "/health"),
    ("GET", "/ready"),
    // Sign-in and registration: they take credentials in the body and give no bearer access.
    ("POST", "/api/v1/auth/login"),
    ("POST", "/api/v1/auth/register"),
    // Metadata documents: public by design (RFC 8414, RFC 9728); 404 when `[oauth]` is not set.
    ("GET", "/.well-known/oauth-authorization-server"),
    ("GET", "/.well-known/oauth-protected-resource"),
    ("GET", "/.well-known/oauth-protected-resource/{*resource_path}"),
    // The MCP server's token lookup: answers 404 to everyone without the forwarding credential
    // (see `the_token_context_endpoint_exists_only_for_the_mcp_server`).
    ("GET", "/api/v1/oauth/internal/token-context"),
    ("POST", "/api/v1/oauth/internal/token-context"),
    ("PUT", "/api/v1/oauth/internal/token-context"),
    ("PATCH", "/api/v1/oauth/internal/token-context"),
    ("DELETE", "/api/v1/oauth/internal/token-context"),
    // Signed downloads: authorised by a signature in the query, not by a bearer token.
    ("GET", "/api/v1/uploads/signatures/{file_name}/download"),
    ("GET", "/api/v1/form-attachments/{attachment_id}/download"),
    // Public Flow shares: authorised by the share token in the path.
    ("GET", "/api/v1/flow/public-shares/{token}"),
    ("GET", "/api/v1/flow/public-shares/{token}/assets/{asset_id}"),
    ("GET", "/api/v2/flow/public-shares/{token}"),
    ("GET", "/api/v2/flow/public-shares/{token}/assets/{asset_id}"),
    ("GET", "/api/v2/flow/public-shares/{token}/body-assets/{asset_id}"),
    // WebSocket upgrades: the handshake is authorised by a single-use ticket, not a bearer token.
    ("GET", "/api/v1/collab/ws"),
    ("GET", "/api/v2/flow/collab/ws"),
];

fn concrete_unlisted(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                Uuid::new_v4().to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn request_body(method: &str, pattern: &str) -> Option<Value> {
    match (method, pattern) {
        ("POST", "/api/v1/projects/{project_id}/issues") => Some(json!({"title": "created through a connection"})),
        ("PUT", "/api/v1/issues/{id}") => Some(json!({"title": "renamed through a connection"})),
        ("POST", "/api/v1/issues/{issue_id}/comments") => Some(json!({"content": "a comment"})),
        ("GET", _) => None,
        _ => Some(json!({})),
    }
}

impl World {
    /// The URI of an allow-listed route with the cast's real ids filled in.
    fn concrete(&self, entry: &RouteEntry) -> String {
        entry
            .pattern
            .split('/')
            .map(|segment| {
                let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
                    return segment.to_string();
                };
                match entry.resolver {
                    Resolver::Workspace(n) if n == name => self.ws1.to_string(),
                    Resolver::Project(n) if n == name => self.p1.to_string(),
                    Resolver::Issue(n) if n == name => self.i1.to_string(),
                    Resolver::IssueIdentifier(n) if n == name => self.ident1.clone(),
                    Resolver::Notification(n) if n == name => {
                        // Marking read twice is a 404 the second time; use a notification per method.
                        if entry.method == "PATCH" { self.n1 } else { self.n4 }.to_string()
                    }
                    _ => self.label1.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[tokio::test]
async fn every_route_is_on_the_allow_list_with_a_scope_or_denied() {
    let Some(w) = World::new("deleg_enumeration").await else {
        return;
    };
    let routes = registered_routes(&w.router);
    assert!(
        routes.len() >= 300 && routes.iter().all(|route| !route.methods.is_empty()),
        "the router parser found {} routes; it no longer matches the router's output",
        routes.len()
    );
    // The allow-list names only routes that exist (the two routes of other packages excepted).
    let registered: BTreeSet<(String, String)> = routes
        .iter()
        .flat_map(|route| route.methods.iter().map(|m| ((*m).to_string(), route.path.clone())))
        .collect();
    for entry in DELEGATED_ROUTES {
        let known = registered.contains(&(entry.method.to_string(), entry.pattern.to_string()));
        let pending = NOT_REGISTERED_YET.contains(&(entry.method, entry.pattern));
        assert!(
            known != pending,
            "{} {}: {}",
            entry.method,
            entry.pattern,
            if known {
                "is registered, so it must leave NOT_REGISTERED_YET"
            } else {
                "is on the allow-list but is not a route of the router"
            }
        );
    }

    let mut session = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
    let mut used = 0_u32;
    let mut tally = std::collections::BTreeMap::<&'static str, u32>::new();
    let mut public_seen = BTreeSet::new();
    let mut undecided: Vec<String> = Vec::new();
    let mut allowed_seen = BTreeSet::new();
    let mut pairs = 0_u32;
    for route in &routes {
        for method in &route.methods {
            pairs += 1;
            // A grant's request budget is 60 in a burst; a fresh grant per few dozen probes keeps
            // the enumeration from measuring the rate limit.
            used += 1;
            if used > 40 {
                session = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
                used = 1;
            }
            let entry = lookup(&Method::from_bytes(method.as_bytes()).expect("a method"), &route.path);
            let uri = entry.map_or_else(|| concrete_unlisted(&route.path), |entry| w.concrete(entry));
            let body = request_body(method, &route.path);
            let delegated = w.send(method, &uri, Some(&session.token), &[], body.as_ref()).await;
            let anonymous = w.send(method, &uri, None, &[], body.as_ref()).await;
            let label = format!("{method} {}", route.path);

            let class = if let Some(entry) = entry {
                assert!(
                    !delegated.refused_by_delegation(),
                    "{label}: on the allow-list with every scope, but refused: {}",
                    delegated.text()
                );
                assert_ne!(delegated.status, StatusCode::FORBIDDEN, "{label}: {}", delegated.text());
                assert!(!matches!(entry.required, Required::AnyOf(&[])), "{label}: empty AnyOf");
                allowed_seen.insert((entry.method, entry.pattern));
                Class::Allowed
            } else if delegated.status == StatusCode::FORBIDDEN
                && delegated.bearer_error().as_deref() == Some("insufficient_scope")
                && delegated.json()["message"] == ROUTE_REFUSED
            {
                let challenge = delegated.header("www-authenticate").unwrap_or_default();
                assert!(
                    challenge.contains(&format!("resource_metadata=\"{METADATA_URL}\"")),
                    "{label}: the refusal carries no resource metadata: {challenge}"
                );
                assert!(
                    !challenge.contains("scope="),
                    "{label}: a route refusal names no scope: {challenge}"
                );
                Class::RouteDenied
            } else if matches!(delegated.code(), Some(401 | 403)) || matches!(delegated.status.as_u16(), 401 | 403) {
                // Anything that is not the delegated branch must also have refused the anonymous call.
                assert!(
                    matches!(anonymous.code(), Some(401 | 403)) || matches!(anonymous.status.as_u16(), 401 | 403),
                    "{label}: refused a delegated token but not an anonymous caller"
                );
                Class::OtherAuthDenied
            } else {
                // Not refused: it must be a route that needs no credential, decided in the list.
                if !PUBLIC_ROUTES.contains(&(method, route.path.as_str())) {
                    undecided.push(format!(
                        "{label} -> {} {}",
                        delegated.status,
                        delegated.text().chars().take(80).collect::<String>()
                    ));
                }
                assert_eq!(
                    delegated.status, anonymous.status,
                    "{label}: a delegated token changed the answer of a public route"
                );
                public_seen.insert((*method, route.path.clone()));
                Class::Public
            };
            *tally
                .entry(match class {
                    Class::Allowed => "allowed",
                    Class::RouteDenied => "denied by route",
                    Class::OtherAuthDenied => "denied by other authentication",
                    Class::Public => "public",
                })
                .or_default() += 1;
        }
    }
    assert!(
        undecided.is_empty(),
        "routes answered a delegated token without being on the allow-list and without being decided as public \
         (add each to PUBLIC_ROUTES with the reason, or protect it):\n{}",
        undecided.join("\n")
    );
    for (method, path) in PUBLIC_ROUTES {
        assert!(
            public_seen.contains(&(*method, (*path).to_string())),
            "{method} {path} is in PUBLIC_ROUTES but is no longer a public route"
        );
    }
    let table_registered = DELEGATED_ROUTES.len() - NOT_REGISTERED_YET.len();
    assert_eq!(
        allowed_seen.len(),
        table_registered,
        "every registered table entry was probed"
    );
    eprintln!(
        "route enumeration: {} routes, {pairs} method/route pairs: {tally:?}; allow-list has {} entries ({} not registered yet)",
        routes.len(),
        DELEGATED_ROUTES.len(),
        NOT_REGISTERED_YET.len()
    );
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// The allow-list: scopes
// ---------------------------------------------------------------------------------------------

fn scope_names(required: Required) -> Vec<&'static str> {
    match required {
        Required::One(scope) => vec![scope.as_str()],
        Required::AnyOf(any) => any.iter().map(|s| s.as_str()).collect(),
        Required::AnyScope => vec!["projects:read"],
    }
}

#[tokio::test]
async fn each_allowed_route_works_with_its_scope_and_is_refused_without_it() {
    let Some(w) = World::new("deleg_scopes").await else {
        return;
    };
    let mut checked = 0;
    for entry in DELEGATED_ROUTES {
        if NOT_REGISTERED_YET.contains(&(entry.method, entry.pattern)) {
            continue;
        }
        let uri = w.concrete(entry);
        let body = request_body(entry.method, entry.pattern);
        let label = format!("{} {}", entry.method, entry.pattern);

        // With exactly the scope it needs (and no other), the handler answers.
        let allowed_scope = scope_names(entry.required);
        let session = w.session(w.alice, w.ws1, &[], &allowed_scope).await;
        let reply = w
            .send(entry.method, &uri, Some(&session.token), &[], body.as_ref())
            .await;
        assert_ok(&reply, &label);

        // Without it: 403 insufficient_scope, naming the full set the route needs, with the
        // protected-resource metadata in the challenge. Nothing was written.
        if let Required::One(required) = entry.required {
            let other = if required == Scope::TasksRead {
                "projects:read"
            } else {
                "tasks:read"
            };
            let session = w.session(w.alice, w.ws1, &[], &[other]).await;
            let reply = w
                .send(entry.method, &uri, Some(&session.token), &[], body.as_ref())
                .await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{label}: {}", reply.text());
            assert_eq!(reply.bearer_error().as_deref(), Some("insufficient_scope"), "{label}");
            let challenge = reply.header("www-authenticate").expect("a challenge");
            let named = ScopeSet::challenge_for(required).to_scope_string();
            assert!(
                challenge.contains(&format!("scope=\"{named}\"")),
                "{label}: challenge {challenge} does not name `{named}`"
            );
            assert!(
                challenge.contains(&format!("resource_metadata=\"{METADATA_URL}\"")),
                "{label}: {challenge}"
            );
            assert!(
                challenge.starts_with("Bearer realm=\"sylvode\""),
                "{label}: {challenge}"
            );
            assert_eq!(reply.header("cache-control").as_deref(), Some("no-store"), "{label}");
        }
        checked += 1;
    }
    assert_eq!(checked, DELEGATED_ROUTES.len() - NOT_REGISTERED_YET.len());

    // Hierarchy: write implies its read (work items), and nothing else is implied.
    let write_only = w.session(w.alice, w.ws1, &[], &["work_items:write"]).await;
    assert_ok(
        &w.get(&write_only, &format!("/api/v1/projects/{}/issues", w.p1)).await,
        "write implies read",
    );
    assert_ok(
        &w.get(&write_only, &format!("/api/v1/issues/{}", w.i1)).await,
        "write implies read (item)",
    );
    let read_only = w.session(w.alice, w.ws1, &[], &["work_items:read"]).await;
    let reply = w
        .send(
            "POST",
            &format!("/api/v1/projects/{}/issues", w.p1),
            Some(&read_only.token),
            &[],
            Some(&json!({"title": "must not exist"})),
        )
        .await;
    assert_eq!(reply.bearer_error().as_deref(), Some("insufficient_scope"));
    assert_eq!(
        count(
            &w.db,
            "SELECT COUNT(*) AS n FROM work_items WHERE title = 'must not exist'",
            vec![]
        )
        .await,
        0
    );
    let notifications_write = w.session(w.alice, w.ws1, &[], &["notifications:write"]).await;
    let reply = w.get(&notifications_write, "/api/v1/notifications").await;
    assert_eq!(
        reply.bearer_error().as_deref(),
        Some("insufficient_scope"),
        "notifications:write does not imply notifications:read: {}",
        reply.text()
    );
    // A project scope is not an item scope.
    let projects_only = w.session(w.alice, w.ws1, &[], &["projects:read"]).await;
    assert_eq!(
        w.get(&projects_only, &format!("/api/v1/issues/{}", w.i1))
            .await
            .bearer_error()
            .as_deref(),
        Some("insufficient_scope")
    );
    w.finish().await;
}

#[tokio::test]
async fn unlisted_routes_are_refused_even_when_the_user_could_call_them() {
    let Some(w) = World::new("deleg_unlisted").await else {
        return;
    };
    let session = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
    let bots_before = count(&w.db, "SELECT COUNT(*) AS n FROM workspace_bots", vec![]).await;
    let items_before = count(&w.db, "SELECT COUNT(*) AS n FROM work_items", vec![]).await;
    let tasks_before = count(&w.db, "SELECT COUNT(*) AS n FROM ai_tasks", vec![]).await;
    let cases: Vec<(&str, String, Option<Value>)> = vec![
        ("DELETE", format!("/api/v1/issues/{}", w.i1), None),
        ("DELETE", format!("/api/v1/projects/{}", w.p1), None),
        (
            "PUT",
            format!("/api/v1/projects/{}", w.p1),
            Some(json!({"name": "renamed"})),
        ),
        ("GET", "/api/v1/admin/stats".to_string(), None),
        ("GET", "/api/v1/admin/users".to_string(), None),
        (
            "POST",
            format!("/api/v1/workspaces/{}/bots", w.ws1),
            Some(json!({"name": "x", "permissions": ["admin"]})),
        ),
        ("GET", format!("/api/v1/workspaces/{}/bots", w.ws1), None),
        ("GET", format!("/api/v1/workspaces/{}/webhooks", w.ws1), None),
        ("POST", format!("/api/v1/projects/{}/ai/tasks", w.p1), Some(json!({}))),
        ("DELETE", format!("/api/v1/notifications/{}", w.n1), None),
        ("GET", format!("/api/v1/workspaces/{}/members", w.ws1), None),
        ("GET", "/api/v1/auth/me".to_string(), None),
        ("PUT", "/api/v1/auth/me/password".to_string(), Some(json!({}))),
        (
            "POST",
            format!("/api/v1/issues/{}/labels/batch", w.i1),
            Some(json!({"label_ids": [w.label1]})),
        ),
        ("GET", format!("/api/v1/projects/{}/forms", w.p1), None),
        ("GET", "/api/v1/search?q=item".to_string(), None),
    ];
    for (method, uri, body) in cases {
        let reply = w.send(method, &uri, Some(&session.token), &[], body.as_ref()).await;
        let refused =
            reply.bearer_error().as_deref() == Some("insufficient_scope") || matches!(reply.code(), Some(401 | 403));
        assert!(
            refused,
            "{method} {uri} was not refused: {} {}",
            reply.status,
            reply.text()
        );
    }
    assert_eq!(
        count(&w.db, "SELECT COUNT(*) AS n FROM workspace_bots", vec![]).await,
        bots_before
    );
    assert_eq!(
        count(&w.db, "SELECT COUNT(*) AS n FROM work_items", vec![]).await,
        items_before
    );
    assert_eq!(
        count(&w.db, "SELECT COUNT(*) AS n FROM ai_tasks", vec![]).await,
        tasks_before
    );
    // The same user, with their own session, can call what the connection cannot.
    let own = w
        .send("GET", "/api/v1/admin/users", Some(&World::jwt(w.alice)), &[], None)
        .await;
    assert_eq!(
        own.code(),
        Some(0),
        "the user's own session is unchanged: {}",
        own.text()
    );
    // A matched pattern is the key, not the raw URL: a path that merely looks like an allowed one
    // does not match.
    let sneaky = w
        .send(
            "GET",
            &format!("/api/v1/issues/{}/../{}", w.i1, w.i1),
            Some(&session.token),
            &[],
            None,
        )
        .await;
    assert!(
        sneaky.bearer_error().is_some() || sneaky.status == StatusCode::NOT_FOUND || sneaky.code() != Some(0),
        "{}",
        sneaky.text()
    );
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// No confused deputy
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_grant_for_one_workspace_and_one_project_reaches_nothing_else() {
    let Some(w) = World::new("deleg_isolation").await else {
        return;
    };
    // alice belongs to two workspaces; the grant is ws1 with p1 only.
    let selected = w.session(w.alice, w.ws1, &[w.p1], ALL_API_SCOPES).await;

    // Lists never return rows outside the grant.
    let list = w
        .get(&selected, &format!("/api/v1/workspaces/{}/projects", w.ws1))
        .await;
    assert_ok(&list, "project list");
    assert_eq!(ids(&list, "items"), BTreeSet::from([w.p1]), "{}", list.text());
    let paged = w
        .get(
            &selected,
            &format!("/api/v1/workspaces/{}/projects?page=1&per_page=10", w.ws1),
        )
        .await;
    assert_eq!(ids(&paged, "items"), BTreeSet::from([w.p1]));
    assert_eq!(
        paged.json()["data"]["total"],
        1,
        "the page total counts the grant's projects: {}",
        paged.text()
    );
    assert_target_refused(
        &w.get(&selected, &format!("/api/v1/workspaces/{}/projects", w.ws2))
            .await,
        "other workspace's project list",
    );

    // Gets: the project and everything below it.
    let project_routes = |project: Uuid| {
        vec![
            format!("/api/v1/projects/{project}"),
            format!("/api/v1/projects/{project}/context"),
            format!("/api/v1/projects/{project}/agent-policy"),
            format!("/api/v1/projects/{project}/issues"),
            format!("/api/v1/projects/{project}/ai/tasks"),
        ]
    };
    for uri in project_routes(w.p1) {
        assert_ok(&w.get(&selected, &uri).await, &uri);
    }
    let unknown = Uuid::new_v4();
    let mut refusals = Vec::new();
    for project in [w.p2, w.p3, unknown] {
        for uri in project_routes(project) {
            let reply = w.get(&selected, &uri).await;
            assert_target_refused(&reply, &uri);
            refusals.push((reply.status, reply.json(), reply.header("www-authenticate")));
        }
    }
    // Existing and absent objects are answered identically.
    assert!(
        refusals.windows(2).all(|pair| pair[0] == pair[1]),
        "refusals differ: {refusals:?}"
    );

    let issue_routes = |issue: Uuid| {
        vec![
            format!("/api/v1/issues/{issue}"),
            format!("/api/v1/issues/{issue}/comments"),
            format!("/api/v1/issues/{issue}/activities"),
            format!("/api/v1/issues/{issue}/labels"),
        ]
    };
    for uri in issue_routes(w.i1) {
        assert_ok(&w.get(&selected, &uri).await, &uri);
    }
    for issue in [w.i2, w.i3, unknown] {
        for uri in issue_routes(issue) {
            assert_target_refused(&w.get(&selected, &uri).await, &uri);
        }
    }
    assert_ok(
        &w.get(&selected, &format!("/api/v1/issues/by-identifier/{}", w.ident1))
            .await,
        "identifier in grant",
    );
    let by_id = w
        .get(
            &selected,
            &format!("/api/v1/issues/by-identifier/{}", w.ident1.to_lowercase()),
        )
        .await;
    assert_ok(&by_id, "identifier is case-insensitive like the handler");
    for identifier in [
        w.ident2.as_str(),
        w.ident3.as_str(),
        "ZZZ-999",
        "no-identifier",
        "AAA-0",
    ] {
        assert_target_refused(
            &w.get(&selected, &format!("/api/v1/issues/by-identifier/{identifier}"))
                .await,
            identifier,
        );
    }

    // Writes outside the grant are refused before the handler: nothing changes.
    let comments_before = count(&w.db, "SELECT COUNT(*) AS n FROM comments", vec![]).await;
    let items_before = count(&w.db, "SELECT COUNT(*) AS n FROM work_items", vec![]).await;
    let labels_before = count(&w.db, "SELECT COUNT(*) AS n FROM work_item_labels", vec![]).await;
    for issue in [w.i2, w.i3] {
        let reply = w
            .send(
                "POST",
                &format!("/api/v1/issues/{issue}/comments"),
                Some(&selected.token),
                &[],
                Some(&json!({"content": "x"})),
            )
            .await;
        assert_target_refused(&reply, "comment outside the grant");
        let reply = w
            .send(
                "PUT",
                &format!("/api/v1/issues/{issue}"),
                Some(&selected.token),
                &[],
                Some(&json!({"title": "taken"})),
            )
            .await;
        assert_target_refused(&reply, "update outside the grant");
        let reply = w
            .send(
                "POST",
                &format!("/api/v1/issues/{issue}/labels/{}", w.label1),
                Some(&selected.token),
                &[],
                None,
            )
            .await;
        assert_target_refused(&reply, "label outside the grant");
    }
    for project in [w.p2, w.p3] {
        let reply = w
            .send(
                "POST",
                &format!("/api/v1/projects/{project}/issues"),
                Some(&selected.token),
                &[],
                Some(&json!({"title": "taken"})),
            )
            .await;
        assert_target_refused(&reply, "create outside the grant");
    }
    assert_eq!(
        count(&w.db, "SELECT COUNT(*) AS n FROM comments", vec![]).await,
        comments_before
    );
    assert_eq!(
        count(&w.db, "SELECT COUNT(*) AS n FROM work_items", vec![]).await,
        items_before
    );
    assert_eq!(
        count(&w.db, "SELECT COUNT(*) AS n FROM work_item_labels", vec![]).await,
        labels_before
    );
    assert_eq!(
        count(
            &w.db,
            "SELECT COUNT(*) AS n FROM work_items WHERE title = 'taken'",
            vec![]
        )
        .await,
        0
    );
    // Inside the grant the same writes work.
    let created = w
        .send(
            "POST",
            &format!("/api/v1/projects/{}/issues", w.p1),
            Some(&selected.token),
            &[],
            Some(&json!({"title": "inside"})),
        )
        .await;
    assert_ok(&created, "create inside the grant");

    // Notifications: only the user's own, of the grant's workspace, of the grant's projects.
    let notifications = w.get(&selected, "/api/v1/notifications").await;
    assert_ok(&notifications, "notifications");
    assert_eq!(
        ids(&notifications, "items"),
        BTreeSet::from([w.n1]),
        "{}",
        notifications.text()
    );
    assert_eq!(notifications.json()["data"]["total"], 1);
    assert_eq!(notifications.json()["data"]["unread_count"], 1);
    assert_eq!(
        w.get(&selected, "/api/v1/notifications/unread-count").await.json()["data"]["count"],
        1
    );
    for foreign in [w.n2, w.n3, w.n4, w.n_without_workspace, w.n_bob] {
        let reply = w
            .send(
                "PATCH",
                &format!("/api/v1/notifications/{foreign}/read"),
                Some(&selected.token),
                &[],
                Some(&json!({})),
            )
            .await;
        assert_target_refused(&reply, "mark a notification outside the grant");
        assert_eq!(
            count(
                &w.db,
                "SELECT COUNT(*) AS n FROM notifications WHERE id = $1 AND is_read",
                vec![foreign.into()]
            )
            .await,
            0,
            "a refused request must not mark anything read"
        );
    }
    let read_all = w
        .send(
            "PUT",
            "/api/v1/notifications/read-all",
            Some(&selected.token),
            &[],
            Some(&json!({})),
        )
        .await;
    assert_ok(&read_all, "read-all");
    let read: BTreeSet<Uuid> = {
        #[derive(Debug, FromQueryResult)]
        struct Id {
            id: Uuid,
        }
        Id::find_by_statement(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id FROM notifications WHERE is_read".to_string(),
        ))
        .all(&w.db)
        .await
        .expect("read ids")
        .into_iter()
        .map(|row| row.id)
        .collect()
    };
    assert_eq!(
        read,
        BTreeSet::from([w.n1]),
        "read-all through the grant reaches the grant's notifications only"
    );

    // A grant for all projects of ws1 still stops at the workspace.
    let all = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
    let list = w.get(&all, &format!("/api/v1/workspaces/{}/projects", w.ws1)).await;
    assert_eq!(ids(&list, "items"), BTreeSet::from([w.p1, w.p2]), "{}", list.text());
    for uri in [
        format!("/api/v1/projects/{}", w.p3),
        format!("/api/v1/issues/{}", w.i3),
        format!("/api/v1/workspaces/{}/projects", w.ws2),
        format!("/api/v1/issues/by-identifier/{}", w.ident3),
    ] {
        assert_target_refused(&w.get(&all, &uri).await, &uri);
    }
    let notifications = w.get(&all, "/api/v1/notifications").await;
    // n1 is already read; a grant for all projects sees the workspace's notifications of its user:
    // the project-less one included, the one of another workspace and the unattributed one not.
    assert_eq!(
        ids(&notifications, "items"),
        BTreeSet::from([w.n1, w.n2, w.n4]),
        "{}",
        notifications.text()
    );
    assert_eq!(
        w.get(&all, "/api/v1/notifications/unread-count").await.json()["data"]["count"],
        2
    );
    w.finish().await;
}

#[tokio::test]
async fn a_grant_follows_the_users_live_state() {
    let Some(w) = World::new("deleg_state").await else {
        return;
    };
    let uri = format!("/api/v1/issues/{}", w.i1);
    let scopes = &["work_items:read"];
    let mut bodies = Vec::new();
    // Each case changes one fact and the next request with the same token fails; the response says
    // nothing about which fact it was.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Box<dyn Fn(&World, &Session, Uuid) -> String>)> = vec![
        (
            "grant revoked",
            Box::new(|_, s, _| {
                format!(
                    "UPDATE oauth_grants SET status = 'revoked', revoked_at = now(), revoke_reason = 'user_revoked' WHERE id = '{}'",
                    s.grant
                )
            }),
        ),
        (
            "token revoked",
            Box::new(|_, s, _| format!("UPDATE oauth_tokens SET revoked_at = now() WHERE id = '{}'", s.token_id)),
        ),
        (
            "token expired",
            Box::new(|_, s, _| {
                format!(
                    "UPDATE oauth_tokens SET expires_at = now() - interval '1 second' WHERE id = '{}'",
                    s.token_id
                )
            }),
        ),
        (
            "grant expired",
            Box::new(|_, s, _| {
                format!(
                    "UPDATE oauth_grants SET expires_at = now() - interval '1 second' WHERE id = '{}'",
                    s.grant
                )
            }),
        ),
        (
            "removed from the workspace",
            Box::new(|w, _, user| {
                format!(
                    "DELETE FROM workspace_members WHERE workspace_id = '{}' AND user_id = '{user}'",
                    w.ws1
                )
            }),
        ),
        (
            "account disabled",
            Box::new(|_, _, user| format!("UPDATE users SET is_active = false WHERE id = '{user}'")),
        ),
        (
            "password changed",
            Box::new(|_, _, user| {
                format!("UPDATE users SET session_version = session_version + 1 WHERE id = '{user}'")
            }),
        ),
        (
            "workspace turned connections off",
            Box::new(|w, _, _| {
                format!(
                    "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) VALUES ('{}', false)",
                    w.ws1
                )
            }),
        ),
        (
            "client disabled",
            Box::new(|_, _, _| "UPDATE oauth_clients SET status = 'disabled' WHERE client_id = 'ramflux'".to_string()),
        ),
    ];
    for (name, change) in cases {
        let member = w.new_member("member").await;
        let session = w.session(member, w.ws1, &[], scopes).await;
        assert_ok(&w.get(&session, &uri).await, &format!("{name}: before"));
        w.db.execute_unprepared(&change(&w, &session, member))
            .await
            .expect("the change applies");
        let reply = w.get(&session, &uri).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{name}: {}", reply.text());
        assert_eq!(reply.bearer_error().as_deref(), Some("invalid_token"), "{name}");
        let challenge = reply.header("www-authenticate").expect("a challenge");
        assert!(challenge.contains("error=\"invalid_token\""), "{name}: {challenge}");
        assert!(
            challenge.contains(&format!("resource_metadata=\"{METADATA_URL}\"")),
            "{name}: {challenge}"
        );
        bodies.push((name, reply.json()));
        // Undo what other cases must not inherit.
        w.db.execute_unprepared("UPDATE oauth_clients SET status = 'active' WHERE client_id = 'ramflux'")
            .await
            .expect("client restored");
        w.db.execute_unprepared("DELETE FROM workspace_delegation_settings")
            .await
            .expect("setting restored");
    }
    assert!(
        bodies.windows(2).all(|pair| pair[0].1 == pair[1].1),
        "the answer reveals which condition failed: {bodies:?}"
    );
    // Removal from the workspace also ends the grant, for good.
    assert!(
        count(
            &w.db,
            "SELECT COUNT(*) AS n FROM oauth_grants WHERE revoke_reason = 'membership_removed'",
            vec![]
        )
        .await
            >= 1
    );
    // An unknown, a malformed and a foreign-prefix value fail the same way.
    for token in [
        format!("sva_{}", "0".repeat(64)),
        "sva_short".to_string(),
        format!("sva_{}", "Z".repeat(64)),
    ] {
        let reply = w.send("GET", &uri, Some(&token), &[], None).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(reply.bearer_error().as_deref(), Some("invalid_token"));
    }

    // Audience.
    let mcp = w.grant(w.alice, w.ws1, &[], scopes).await;
    let mcp_token = w.token(mcp, "mcp", scopes).await;
    let direct = w.get(&mcp_token, &uri).await;
    assert_eq!(direct.status, StatusCode::UNAUTHORIZED, "{}", direct.text());
    assert_eq!(direct.json()["message"], "token audience does not match");
    let wrong_secret = w
        .send(
            "GET",
            &uri,
            Some(&mcp_token.token),
            &[
                ("x-sylvode-forwarded-by", "mcp-server"),
                ("x-sylvode-forward-auth", "not-the-secret-not-the-secret-0000"),
            ],
            None,
        )
        .await;
    assert_eq!(wrong_secret.status, StatusCode::UNAUTHORIZED);
    let wrong_name = w
        .send(
            "GET",
            &uri,
            Some(&mcp_token.token),
            &[
                ("x-sylvode-forwarded-by", "someone"),
                ("x-sylvode-forward-auth", FORWARD_SECRET),
            ],
            None,
        )
        .await;
    assert_eq!(wrong_name.status, StatusCode::UNAUTHORIZED);
    let forwarded = w
        .send(
            "GET",
            &uri,
            Some(&mcp_token.token),
            &World::forward_headers(Some("work_items.get")),
            None,
        )
        .await;
    assert_ok(&forwarded, "an MCP token forwarded by the MCP server");
    let row = {
        #[derive(Debug, FromQueryResult)]
        struct Log {
            surface: String,
            tool_name: Option<String>,
            outcome: String,
        }
        Log::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT surface, tool_name, outcome FROM oauth_operation_logs WHERE grant_id = $1 \
             ORDER BY created_at DESC LIMIT 1",
            vec![mcp.into()],
        ))
        .one(&w.db)
        .await
        .expect("log readable")
        .expect("a forwarded request is logged")
    };
    assert_eq!(
        (row.surface.as_str(), row.tool_name.as_deref(), row.outcome.as_str()),
        ("mcp_http", Some("work_items.get"), "ok")
    );
    // The API audience token with the credential present: the credential adds nothing, and a
    // tool name it carries is not taken as attribution.
    let api = w.session(w.alice, w.ws1, &[], scopes).await;
    let reply = w
        .send(
            "GET",
            &uri,
            Some(&api.token),
            &World::forward_headers(Some("work_items.get")),
            None,
        )
        .await;
    assert_ok(&reply, "an API token with forwarding headers");
    let (surface, tool): (String, Option<String>) = {
        #[derive(Debug, FromQueryResult)]
        struct Log {
            surface: String,
            tool_name: Option<String>,
        }
        let row = Log::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT surface, tool_name FROM oauth_operation_logs WHERE grant_id = $1 LIMIT 1",
            vec![api.grant.into()],
        ))
        .one(&w.db)
        .await
        .expect("log readable")
        .expect("logged");
        (row.surface, row.tool_name)
    };
    assert_eq!((surface.as_str(), tool), ("rest", None));
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Agent dispatch
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_connection_can_never_start_or_assign_an_agent() {
    let Some(w) = World::new("deleg_dispatch").await else {
        return;
    };
    let session = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
    let items = || count(&w.db, "SELECT COUNT(*) AS n FROM work_items", vec![]);
    let comments = || count(&w.db, "SELECT COUNT(*) AS n FROM comments", vec![]);
    let tasks = || count(&w.db, "SELECT COUNT(*) AS n FROM ai_tasks", vec![]);
    let (items_before, comments_before, tasks_before) = (items().await, comments().await, tasks().await);

    let create = w
        .send(
            "POST",
            &format!("/api/v1/projects/{}/issues", w.p1),
            Some(&session.token),
            &[],
            Some(&json!({"title": "for the agent", "assignee_id": w.agent})),
        )
        .await;
    let update = w
        .send(
            "PUT",
            &format!("/api/v1/issues/{}", w.i1),
            Some(&session.token),
            &[],
            Some(&json!({"assignee_id": w.agent})),
        )
        .await;
    let mention = w
        .send(
            "POST",
            &format!("/api/v1/issues/{}/comments", w.i1),
            Some(&session.token),
            &[],
            Some(&json!({"content": "please look", "mentions": [w.agent]})),
        )
        .await;
    for (name, reply) in [("create", &create), ("update", &update), ("mention", &mention)] {
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{name}: {}", reply.text());
        assert_eq!(reply.bearer_error().as_deref(), Some("insufficient_scope"), "{name}");
        assert!(
            reply.json()["message"]
                .as_str()
                .is_some_and(|m| m.contains("tasks:dispatch")),
            "{name}"
        );
        assert!(reply.header("www-authenticate").is_some(), "{name}");
    }
    assert_eq!(
        (items().await, comments().await, tasks().await),
        (items_before, comments_before, tasks_before),
        "a refused dispatch wrote nothing"
    );
    assert_eq!(
        count(
            &w.db,
            "SELECT COUNT(*) AS n FROM work_items WHERE assignee_id IS NOT NULL",
            vec![]
        )
        .await,
        0
    );
    // A mention of a person next to a mention of an agent is refused as a whole.
    let mixed = w
        .send(
            "POST",
            &format!("/api/v1/issues/{}/comments", w.i1),
            Some(&session.token),
            &[],
            Some(&json!({"content": "both", "mentions": [w.bob, w.agent]})),
        )
        .await;
    assert_eq!(mixed.status, StatusCode::FORBIDDEN);
    assert_eq!(comments().await, comments_before);
    // The route that creates tasks directly is not on the allow-list.
    let direct = w
        .send(
            "POST",
            &format!("/api/v1/projects/{}/ai/tasks", w.p1),
            Some(&session.token),
            &[],
            Some(&json!({"ai_participant_id": w.agent, "task_type": "issue_assigned"})),
        )
        .await;
    assert_eq!(direct.bearer_error().as_deref(), Some("insufficient_scope"));
    assert_eq!(tasks().await, tasks_before);

    // People are fine to assign and mention: only agents are guarded.
    let person = w
        .send(
            "POST",
            &format!("/api/v1/projects/{}/issues", w.p1),
            Some(&session.token),
            &[],
            Some(&json!({"title": "for bob", "assignee_id": w.bob})),
        )
        .await;
    assert_ok(&person, "assign a person");
    let comment = w
        .send(
            "POST",
            &format!("/api/v1/issues/{}/comments", w.i1),
            Some(&session.token),
            &[],
            Some(&json!({"content": "hello bob", "mentions": [w.bob]})),
        )
        .await;
    assert_ok(&comment, "mention a person");
    assert_eq!(tasks().await, tasks_before);

    // The user's own session is not narrowed: the same request still starts the agent.
    let own = w
        .send(
            "POST",
            &format!("/api/v1/projects/{}/issues", w.p1),
            Some(&World::jwt(w.alice)),
            &[],
            Some(&json!({"title": "own assignment", "assignee_id": w.agent})),
        )
        .await;
    assert_eq!(own.code(), Some(0), "{}", own.text());
    assert_eq!(
        tasks().await,
        tasks_before + 1,
        "the user's own request queues the task as before"
    );
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn tokens_travel_in_the_authorization_header_only() {
    let Some(w) = World::new("deleg_transport").await else {
        return;
    };
    let session = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
    let uri = format!("/api/v1/issues/{}", w.i1);

    // A cookie never carries a delegated token.
    let cookie = Request::builder()
        .method("GET")
        .uri(&uri)
        .header(header::COOKIE, format!("access_token={}", session.token))
        .body(Body::empty())
        .expect("request");
    let response = w.router.clone().oneshot(cookie).await.expect("answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get(header::WWW_AUTHENTICATE).is_some());
    let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.expect("body");
    assert!(String::from_utf8_lossy(&body).contains("Authorization header"));

    // A token in the query string is refused, whatever else the request carries, and so is its
    // percent-encoded spelling.
    for query in [
        format!("access_token={}", session.token),
        format!("access%5Ftoken={}", session.token),
        "x=1&access_token=anything".to_string(),
    ] {
        let reply = w
            .send("GET", &format!("{uri}?{query}"), Some(&session.token), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{query}: {}", reply.text());
        assert_eq!(reply.bearer_error().as_deref(), Some("invalid_request"));
        assert!(reply.header("www-authenticate").is_some());
    }
    // The header is the way in.
    assert_ok(&w.get(&session, &uri).await, "header");
    // Responses to delegated requests are not cacheable.
    assert_eq!(
        w.send("GET", &format!("{uri}?access_token=x"), Some(&session.token), &[], None)
            .await
            .header("cache-control")
            .as_deref(),
        Some("no-store")
    );

    // Without `[oauth]` configured no delegated token is valid.
    let state = state_for(w.db.clone());
    let unconfigured = build_router(state);
    let reply = send_to(&unconfigured, "GET", &uri, Some(&session.token), &[], None).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    assert_eq!(reply.bearer_error().as_deref(), Some("invalid_token"));

    // A grant is held to its request budget; other grants are not affected.
    let busy = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
    let mut limited = None;
    let mut served = 0;
    for _ in 0..80 {
        let reply = w.get(&busy, "/api/v1/oauth/connection").await;
        if reply.status == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(reply);
            break;
        }
        served += 1;
    }
    let limited = limited.expect("a grant is rate limited");
    assert!(
        (55..=70).contains(&served),
        "the burst is about 60 requests, got {served}"
    );
    assert_eq!(limited.bearer_error().as_deref(), Some("rate_limited"));
    assert!(limited.header("retry-after").is_some());
    assert_ok(&w.get(&session, &uri).await, "another grant is unaffected");

    // Use is stamped on the token, but not on every request.
    #[derive(Debug, FromQueryResult)]
    struct Used {
        used: bool,
    }
    let stamped = || {
        Used::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT last_used_at IS NOT NULL AS used FROM oauth_tokens WHERE id = $1",
            vec![session.token_id.into()],
        ))
        .one(&w.db)
    };
    let mut seen = false;
    for _ in 0..40 {
        if stamped().await.expect("readable").is_some_and(|row| row.used) {
            seen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(seen, "the first use of a token is stamped");
    w.db.execute_unprepared(&format!(
        "UPDATE oauth_tokens SET last_used_at = '2001-01-01T00:00:00Z' WHERE id = '{}'",
        session.token_id
    ))
    .await
    .expect("sentinel written");
    for _ in 0..5 {
        assert_ok(&w.get(&session, &uri).await, "repeat request");
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        count(
            &w.db,
            "SELECT COUNT(*) AS n FROM oauth_tokens WHERE id = $1 AND last_used_at = '2001-01-01T00:00:00Z'",
            vec![session.token_id.into()]
        )
        .await,
        1,
        "requests within the interval do not write the use stamp again"
    );
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// The internal token-context endpoint
// ---------------------------------------------------------------------------------------------

const CONTEXT: &str = "/api/v1/oauth/internal/token-context";

#[tokio::test]
async fn the_token_context_endpoint_exists_only_for_the_mcp_server() {
    let Some(w) = World::new("deleg_context").await else {
        return;
    };
    let scopes = &["projects:read", "work_items:read"];
    let grant = w.grant(w.alice, w.ws1, &[w.p1], scopes).await;
    let mcp = w.token(grant, "mcp", scopes).await;
    let api = w.token(grant, "api", scopes).await;

    // Without the credential the route does not exist: same answer for every caller and method.
    let wrong = [
        ("x-sylvode-forwarded-by", "mcp-server"),
        ("x-sylvode-forward-auth", "wrong-secret-wrong-secret-000000"),
    ];
    let mut absent = Vec::new();
    for method in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
        for (token, headers) in [
            (Some(mcp.token.as_str()), &[][..]),
            (Some(api.token.as_str()), &[][..]),
            (None, &[][..]),
            (Some(mcp.token.as_str()), &wrong[..]),
            (
                Some(mcp.token.as_str()),
                &[("x-sylvode-forward-auth", FORWARD_SECRET)][..],
            ),
            (
                Some(mcp.token.as_str()),
                &[("x-sylvode-forwarded-by", "mcp-server")][..],
            ),
        ] {
            let reply = w.send(method, CONTEXT, token, headers, None).await;
            assert_eq!(
                reply.status,
                StatusCode::NOT_FOUND,
                "{method} {headers:?}: {}",
                reply.text()
            );
            absent.push((reply.body.clone(), reply.header("www-authenticate")));
        }
    }
    assert!(absent.windows(2).all(|pair| pair[0] == pair[1]));
    assert!(absent[0].0.is_empty() && absent[0].1.is_none());
    // Other users' own sessions get the same nothing.
    assert_eq!(
        w.send("GET", CONTEXT, Some(&World::jwt(w.alice)), &[], None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // With the credential and an MCP token: the context, and nothing secret.
    let headers = World::forward_headers(None);
    let reply = w.send("GET", CONTEXT, Some(&mcp.token), &headers, None).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let body = reply.json();
    let keys: BTreeSet<&str> = body
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "active",
            "client_id",
            "exp",
            "grant_id",
            "project_ids",
            "project_mode",
            "scope",
            "sub",
            "workspace_id"
        ])
    );
    assert_eq!(body["project_ids"], json!([w.p1.to_string()]));
    assert_eq!(body["active"], true);
    assert_eq!(body["client_id"], "ramflux");
    assert_eq!(body["sub"], w.alice.to_string());
    assert_eq!(body["grant_id"], grant.to_string());
    assert_eq!(body["workspace_id"], w.ws1.to_string());
    assert_eq!(body["scope"], "projects:read work_items:read");
    assert_eq!(body["project_mode"], "selected");
    assert!(body["exp"].as_i64().is_some_and(|exp| exp > 0));
    assert_eq!(reply.header("cache-control").as_deref(), Some("no-store"));
    let text = reply.text();
    for secret in [mcp.token.as_str(), api.token.as_str(), FORWARD_SECRET, "token_hash"] {
        assert!(!text.contains(secret), "the context leaks a secret");
    }

    // A grant for all projects carries no project list.
    let all_grant = w.grant(w.alice, w.ws1, &[], scopes).await;
    let all_mcp = w.token(all_grant, "mcp", scopes).await;
    let all_body = w
        .send(
            "GET",
            CONTEXT,
            Some(&all_mcp.token),
            &World::forward_headers(None),
            None,
        )
        .await
        .json();
    assert_eq!(all_body["project_mode"], "all");
    assert!(all_body.get("project_ids").is_none(), "{all_body}");

    // Only MCP-audience tokens, only from the header, no query tokens.
    for (what, reply) in [
        (
            "api-audience token",
            w.send("GET", CONTEXT, Some(&api.token), &headers, None).await,
        ),
        ("no token", w.send("GET", CONTEXT, None, &headers, None).await),
        (
            "not a delegated token",
            w.send("GET", CONTEXT, Some(&World::jwt(w.alice)), &headers, None).await,
        ),
    ] {
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{what}: {}", reply.text());
        assert_eq!(reply.bearer_error().as_deref(), Some("invalid_token"), "{what}");
        assert!(reply.header("www-authenticate").is_some(), "{what}");
    }
    let query = w
        .send(
            "GET",
            &format!("{CONTEXT}?access_token={}", mcp.token),
            Some(&mcp.token),
            &headers,
            None,
        )
        .await;
    assert_eq!(query.status, StatusCode::BAD_REQUEST);
    let cookie = Request::builder()
        .method("GET")
        .uri(CONTEXT)
        .header(header::COOKIE, format!("access_token={}", mcp.token))
        .header("x-sylvode-forwarded-by", "mcp-server")
        .header("x-sylvode-forward-auth", FORWARD_SECRET)
        .body(Body::empty())
        .expect("request");
    assert_eq!(
        w.router.clone().oneshot(cookie).await.expect("answers").status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        w.send("POST", CONTEXT, Some(&mcp.token), &headers, None).await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    // The grant ends: the next lookup fails.
    w.db.execute_unprepared(&format!(
        "UPDATE oauth_tokens SET revoked_at = now() WHERE id = '{}'",
        mcp.token_id
    ))
    .await
    .expect("revoked");
    assert_eq!(
        w.send("GET", CONTEXT, Some(&mcp.token), &headers, None).await.status,
        StatusCode::UNAUTHORIZED
    );

    // Rate limited per grant.
    let busy_grant = w.grant(w.alice, w.ws1, &[], scopes).await;
    let busy = w.token(busy_grant, "mcp", scopes).await;
    let mut statuses = Vec::new();
    for _ in 0..80 {
        statuses.push(w.send("GET", CONTEXT, Some(&busy.token), &headers, None).await.status);
    }
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "the endpoint is rate limited"
    );
    assert_eq!(statuses[0], StatusCode::OK);
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// The connection description
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_connection_describes_the_grant() {
    let Some(w) = World::new("deleg_connection").await else {
        return;
    };
    let session = w.session(w.alice, w.ws1, &[w.p1], &["projects:read"]).await;
    let reply = w.get(&session, "/api/v1/oauth/connection").await;
    assert_ok(&reply, "connection");
    let data = &reply.json()["data"];
    assert_eq!(data["grant_id"], session.grant.to_string());
    assert_eq!(data["client_id"], "ramflux");
    assert_eq!(data["workspace"]["id"], w.ws1.to_string());
    assert_eq!(data["workspace"]["name"], "workspace one");
    assert_eq!(data["account"]["user_id"], w.alice.to_string());
    assert_eq!(data["scopes"], json!(["projects:read"]));
    assert_eq!(data["project_mode"], "selected");
    assert_eq!(
        data["projects"],
        json!([{"id": w.p1.to_string(), "key": "AAA", "name": "project AAA"}])
    );
    // It is for connected applications: the user's own session and a bot get nothing from it.
    let own = w
        .send("GET", "/api/v1/oauth/connection", Some(&World::jwt(w.alice)), &[], None)
        .await;
    assert_eq!(own.code(), Some(403), "{}", own.text());
    w.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Operation log, audit and logging hygiene
// ---------------------------------------------------------------------------------------------

struct Capture(Arc<Mutex<String>>);

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Visitor<'a>(&'a mut String);
        impl tracing::field::Visit for Visitor<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let _ = write!(self.0, "{}={value:?} ", field.name());
            }
        }
        let mut line = String::new();
        event.record(&mut Visitor(&mut line));
        let mut sink = self.0.lock();
        sink.push_str(&line);
        sink.push('\n');
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn every_delegated_request_is_logged_and_no_secret_reaches_a_log_or_an_audit_row() {
    let Some(w) = World::new("deleg_logs").await else {
        return;
    };
    let captured = Arc::new(Mutex::new(String::new()));
    let _guard = tracing::subscriber::set_default(Capture(Arc::clone(&captured)));

    let session = w.session(w.alice, w.ws1, &[w.p1], ALL_API_SCOPES).await;
    let ok = w.get(&session, &format!("/api/v1/issues/{}", w.i1)).await;
    assert_ok(&ok, "ok");
    let refused_route = w
        .send(
            "DELETE",
            &format!("/api/v1/issues/{}", w.i1),
            Some(&session.token),
            &[],
            None,
        )
        .await;
    assert_eq!(refused_route.status, StatusCode::FORBIDDEN);
    let refused_target = w.get(&session, &format!("/api/v1/issues/{}", w.i2)).await;
    assert_eq!(refused_target.status, StatusCode::FORBIDDEN);
    let bad = format!("sva_{}", "a".repeat(64));
    w.send(
        "GET",
        "/api/v1/notifications",
        Some(&bad),
        &[("user-agent", "tester/1.0")],
        None,
    )
    .await;
    // A revoked token leaves a sampled audit row.
    w.db.execute_unprepared(&format!(
        "UPDATE oauth_tokens SET revoked_at = now() WHERE id = '{}'",
        session.token_id
    ))
    .await
    .expect("revoked");
    w.get(&session, "/api/v1/notifications").await;
    w.get(&session, "/api/v1/notifications").await;

    #[derive(Debug, FromQueryResult)]
    struct Log {
        method: String,
        path: String,
        business_code: i32,
        outcome: String,
        surface: String,
        client_id: String,
    }
    let rows = Log::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT method, path, business_code, outcome, surface, client_id FROM oauth_operation_logs \
         WHERE grant_id = $1 ORDER BY created_at",
        vec![session.grant.into()],
    ))
    .all(&w.db)
    .await
    .expect("log readable");
    assert_eq!(
        rows.len(),
        3,
        "{rows:?}: the handled request and the two refusals are logged, rejected tokens are audited instead"
    );
    assert_eq!(
        (rows[0].method.as_str(), rows[0].business_code, rows[0].outcome.as_str()),
        ("GET", 0, "ok")
    );
    assert_eq!(rows[0].path, format!("/api/v1/issues/{}", w.i1));
    assert_eq!(
        (rows[1].method.as_str(), rows[1].business_code, rows[1].outcome.as_str()),
        ("DELETE", 403, "error")
    );
    assert_eq!((rows[2].business_code, rows[2].outcome.as_str()), (403, "error"));
    assert!(
        rows.iter()
            .all(|row| row.surface == "rest" && row.client_id == "ramflux")
    );

    let rejected = count(
        &w.db,
        "SELECT COUNT(*) AS n FROM oauth_audit_events WHERE event = 'token_rejected'",
        vec![],
    )
    .await;
    assert!(rejected >= 1, "a refused token leaves an audit row");
    assert!(
        rejected <= 3,
        "refusals are sampled, not written per request: {rejected}"
    );

    // The log write is part of the answer: when it cannot be written the success is not reported.
    w.db.execute_unprepared("ALTER TABLE oauth_operation_logs RENAME TO oauth_operation_logs_away")
        .await
        .expect("renamed");
    let fresh = w.session(w.alice, w.ws1, &[], ALL_API_SCOPES).await;
    let reply = w.get(&fresh, &format!("/api/v1/issues/{}", w.i1)).await;
    assert_eq!(
        reply.code(),
        Some(500),
        "an unlogged delegated request is not a success: {}",
        reply.text()
    );
    w.db.execute_unprepared("ALTER TABLE oauth_operation_logs_away RENAME TO oauth_operation_logs")
        .await
        .expect("restored");

    // Nothing secret anywhere: not in the rows, not in what was logged.
    let secrets = [
        session.token.as_str(),
        fresh.token.as_str(),
        bad.as_str(),
        FORWARD_SECRET,
    ];
    for secret in secrets {
        let rows_with_secret = count(
            &w.db,
            "SELECT COUNT(*) AS n FROM (SELECT row_to_json(t)::text AS j FROM oauth_audit_events t \
             UNION ALL SELECT row_to_json(t)::text FROM oauth_operation_logs t) x WHERE position($1 in j) > 0",
            vec![secret.to_string().into()],
        )
        .await;
        assert_eq!(rows_with_secret, 0, "a secret was written to a log table");
        assert!(!captured.lock().contains(secret), "a secret reached a log line");
    }
    let hash_in_rows = count(
        &w.db,
        "SELECT COUNT(*) AS n FROM (SELECT row_to_json(t)::text AS j FROM oauth_audit_events t \
         UNION ALL SELECT row_to_json(t)::text FROM oauth_operation_logs t) x WHERE position($1 in j) > 0",
        vec![
            api::oauth::tokens::hash_secret(&session.token)
                .as_hex()
                .to_string()
                .into(),
        ],
    )
    .await;
    assert_eq!(hash_in_rows, 0, "a token hash was written to a log table");
    let log_text = captured.lock().clone();
    assert!(
        !log_text.contains("sva_"),
        "a delegated token prefix reached a log line: {log_text}"
    );
    w.finish().await;
}
