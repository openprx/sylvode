use crate::migrate::db_tests::scratch;
use crate::migrate::run_migrations;
use crate::router::build_router;
use api::routes::flow_surface::FlowSurfaceResponse;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use platform::{
    app::AppState,
    auth::JwtManager,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

const JWT_SECRET: &str = "flow-surface-boundary-test-secret";

/// The legacy envelope of a database failure, as every route outside the Flow surface has
/// always sent it.
const LEGACY_DATABASE_ERROR_BODY: &str = r#"{"code":500,"message":"database error"}"#;

/// The legacy envelope of an internal failure, as the authentication middleware sends it for
/// a database failure on every route outside the Flow surface.
const LEGACY_INTERNAL_ERROR_BODY: &str = r#"{"code":500,"message":"internal server error"}"#;

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

/// A database that does not answer is HTTP 503 with code 503, so a probe that only looks at the
/// status line sees the failure.
#[tokio::test]
async fn ready_answers_503_when_the_database_is_unreachable() {
    let state = state_for(DatabaseConnection::Disconnected);
    let response = axum::response::IntoResponse::into_response(super::ready(axum::extract::State(state)).await);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json body");
    assert_eq!(body["code"], 503, "{body}");
}

#[test]
fn scope_native_and_legacy_adapter_router_constructs_without_panicking() {
    let state = state_for(DatabaseConnection::Disconnected);
    let _router = crate::router::build_router(state);
}

fn token_for(user_id: Uuid) -> String {
    JwtManager::new(JWT_SECRET, 900, 3600)
        .issue_access_token(&user_id.to_string(), &format!("{user_id}@boundary.test"))
        .expect("the token is issued")
}

struct Answer {
    status: StatusCode,
    content_type: Option<String>,
    behind_boundary: bool,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

async fn call(state: &AppState, method: &Method, uri: &str, token: Option<&str>, body: Option<&Value>) -> Answer {
    let mut request = Request::builder().method(method.clone()).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("the request is well formed");
    let response = build_router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let behind_boundary = response.extensions().get::<FlowSurfaceResponse>().is_some();
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("the response body is readable")
        .to_vec();
    Answer {
        status,
        content_type,
        behind_boundary,
        body,
    }
}

/// One `.route("<path>", ..)` call of the two router functions.
struct RegisteredRoute {
    path: String,
    /// The `routes` modules whose handlers the call names, in order of appearance.
    handler_modules: Vec<String>,
}

impl RegisteredRoute {
    /// Whether the path belongs to the Flow surface. Every Flow path has a path segment that
    /// is exactly `flow` or `collab`, and no other path has one (`workflow`, `workflows` and
    /// `workflow-states` are different segments), so there is no list of exceptions.
    fn flow_path(&self) -> bool {
        self.path
            .split('/')
            .any(|segment| segment == "flow" || segment == "collab")
    }

    /// Whether every handler the call names is a `routes::flow` / `routes::collab` handler,
    /// or none is. A call that names both kinds has no answer and fails the test.
    fn flow_handlers(&self) -> bool {
        let flow = |module: &String| module == "flow" || module == "collab";
        // The liveness and readiness probes are functions of this file, not of a `routes`
        // module, and the areas outside the Flow surface (drafts, layouts, nav, ...) name their
        // own modules; a Flow path names `routes::flow` / `routes::collab` handlers in full.
        assert!(
            !self.handler_modules.is_empty() || !self.flow_path(),
            "{} names no `routes::<module>::<handler>`",
            self.path
        );
        let all = !self.handler_modules.is_empty() && self.handler_modules.iter().all(flow);
        assert!(
            all || !self.handler_modules.iter().any(flow),
            "{} mixes Flow handlers with handlers of other modules: {:?}",
            self.path,
            self.handler_modules
        );
        all
    }
}

/// Every `.route("<path>", ..)` registered by the router area files.
fn registered_routes() -> Vec<RegisteredRoute> {
    let mut routes = Vec::new();
    let mut block: Option<String> = None;
    let mut finish = |block: &str| {
        if let Some(path) = block.split('"').nth(1) {
            let handler_modules = block
                .split("routes::")
                .skip(1)
                .filter_map(|rest| rest.split_once("::"))
                .map(|(module, _)| module.to_string())
                .collect();
            routes.push(RegisteredRoute {
                path: path.to_string(),
                handler_modules,
            });
        }
    };
    for line in crate::router::ROUTER_SOURCES
        .iter()
        .flat_map(|(_, source)| source.lines())
    {
        let starts_route = line.starts_with("        .route(");
        let ends_block = line.starts_with("        .") || line == "}";
        if (starts_route || ends_block)
            && let Some(previous) = block.take()
        {
            finish(&previous);
        }
        if starts_route {
            block = Some(String::new());
        }
        if let Some(current) = block.as_mut() {
            current.push_str(line);
            current.push('\n');
        }
    }
    routes
}

/// `path` with every `{parameter}` replaced by a UUID.
fn concrete(path: &str) -> String {
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

fn is_typed_database_failure(body: &Value) -> bool {
    let draining = body["error_code"] == "server_draining" && body["details"]["reason"] == "contention";
    let rejected =
        body["error_code"] == "server_rejected" && body["details"]["reason"] == "deterministic_database_refusal";
    draining || rejected
}

#[derive(Default)]
struct Tally {
    flow_answers: usize,
    other_answers: usize,
    typed_database_failures: usize,
    typed_bot_database_failures: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Caller {
    Anonymous,
    User,
    Bot,
}

/// A syntactically valid bot credential; the refusing database cannot look it up.
const BOT_TOKEN: &str = concat!("opr_", "0123456789abcdef0123456789abcdef");

/// Calls `path` with every method and every kind of caller, and checks each answer of a
/// method the route accepts.
async fn probe_route(state: &AppState, user_token: &str, path: &str, flow: bool, tally: &mut Tally) {
    let uri = concrete(path);
    let empty = serde_json::json!({});
    // Outside the Flow surface no session is used: the anonymous and the bot answers are
    // enough to see the boundary marker, and neither runs a handler (the bot credential
    // cannot be looked up).
    let callers: &[Caller] = if flow {
        &[Caller::Anonymous, Caller::User, Caller::Bot]
    } else {
        &[Caller::Anonymous, Caller::Bot]
    };
    let mut accepted = 0_usize;
    for method in [Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        let body = (method != Method::GET).then_some(&empty);
        for caller in callers {
            let token = match caller {
                Caller::Anonymous => None,
                Caller::User => Some(user_token),
                Caller::Bot => Some(BOT_TOKEN),
            };
            let answer = call(state, &method, &uri, token, body).await;
            // The catalogue guard sits outside the boundary and answers a method that has no
            // catalogue entry with a bare `405` (no body, no database access), so a refused
            // method of a Flow path carries no marker; every answer a handler gives does.
            let guard_refusal = answer.status == StatusCode::METHOD_NOT_ALLOWED && answer.body.is_empty();
            assert_eq!(
                answer.behind_boundary || (flow && guard_refusal),
                flow,
                "{method} {path}: a Flow path must answer behind the Flow surface boundary on every method \
                 (a bare 405 of the catalogue guard excepted), and no other path may"
            );
            if answer.status != StatusCode::METHOD_NOT_ALLOWED {
                accepted += 1;
                check_answer(&answer, &format!("{method} {path} as {caller:?}"), flow, *caller, tally);
            }
        }
    }
    assert!(accepted > 0, "{path} accepted none of the probed methods");
}

fn check_answer(answer: &Answer, route: &str, flow: bool, caller: Caller, tally: &mut Tally) {
    assert_ne!(answer.status, StatusCode::NOT_FOUND, "{route} is not routed");
    let typed = is_typed_database_failure(&answer.json());
    if flow {
        tally.flow_answers += 1;
        for legacy in [LEGACY_DATABASE_ERROR_BODY, LEGACY_INTERNAL_ERROR_BODY] {
            assert_ne!(
                answer.text(),
                legacy,
                "{route} answered a legacy envelope for a database failure"
            );
        }
        tally.typed_database_failures += usize::from(caller == Caller::User && typed);
        tally.typed_bot_database_failures += usize::from(caller == Caller::Bot && typed);
    } else {
        tally.other_answers += 1;
        assert!(
            !typed,
            "{route} is not a Flow route and answered a typed database failure"
        );
    }
}

/// Structural proof on the running router, with a database that refuses every statement.
///
/// A route is a Flow route by its path (a `flow` or `collab` segment), and that is the same
/// set as the routes whose handlers are `routes::flow` / `routes::collab` handlers: a Flow
/// path that names any other handler, and a Flow handler on any other path, fail here. Each
/// Flow path answers behind the boundary on every method, whoever calls (nobody, a user
/// session, a bot credential); no other path does; and no Flow route answers a database
/// failure with a legacy envelope, the authentication layer's included. A Flow route
/// registered outside `router::flow`, or after the boundary, fails the marker
/// assertion.
#[tokio::test]
async fn every_flow_route_answers_behind_the_boundary_and_no_other_route_does() {
    let state = state_for(DatabaseConnection::Disconnected);
    let token = token_for(Uuid::new_v4());
    let routes = registered_routes();
    for route in &routes {
        assert_eq!(
            route.flow_path(),
            route.flow_handlers(),
            "{}: a path with a `flow` / `collab` segment serves Flow handlers only, and Flow handlers are \
             served on such paths only (handlers of: {:?})",
            route.path,
            route.handler_modules
        );
    }
    let flow_routes = routes.iter().filter(|route| route.flow_path()).count();
    assert!(
        flow_routes >= 40 && routes.len() > flow_routes + 100,
        "the router parser found {flow_routes} Flow routes of {}; it no longer matches the router files",
        routes.len()
    );

    let mut tally = Tally::default();
    for route in &routes {
        probe_route(&state, &token, &route.path, route.flow_path(), &mut tally).await;
    }
    let Tally {
        flow_answers,
        other_answers,
        typed_database_failures,
        typed_bot_database_failures,
    } = tally;
    eprintln!(
        "flow surface boundary: {flow_routes} Flow routes, {flow_answers} Flow answers \
         ({typed_database_failures} typed database failures for a user session, \
         {typed_bot_database_failures} for a bot credential), {other_answers} other answers"
    );
    assert!(
        typed_database_failures >= 20,
        "only {typed_database_failures} Flow answers were typed database failures; the probe no longer \
         reaches the database"
    );
    assert!(
        typed_bot_database_failures >= 40,
        "only {typed_bot_database_failures} Flow answers to a bot credential were typed database failures; \
         the credential lookup no longer fails inside the boundary"
    );
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

/// Nothing the database or the driver said may travel in a typed envelope.
fn assert_no_driver_text(answer: &Answer, context: &str) {
    let text = answer.text().to_lowercase();
    for fragment in [
        "database error",
        "relation",
        "constraint",
        "violates",
        "does not exist",
        "sqlstate",
        "sqlx",
        "pool",
        "connection",
        "flow_objects",
        "flow_workspace_settings",
        "boundary_refusal",
        "serializ",
    ] {
        assert!(!text.contains(fragment), "{context}: `{fragment}` in {text}");
    }
}

fn assert_server_rejected(answer: &Answer, context: &str) {
    let body = answer.json();
    assert_eq!(answer.status, StatusCode::OK, "{context}: {body}");
    assert_eq!(body["code"], 500, "{context}: {body}");
    assert_eq!(body["error_code"], "server_rejected", "{context}: {body}");
    assert_eq!(
        body["details"],
        serde_json::json!({"reason": "deterministic_database_refusal"}),
        "{context}: {body}"
    );
    assert_eq!(body["message"], "server_rejected", "{context}: {body}");
    assert!(body["data"].is_null(), "{context}: {body}");
    assert_no_driver_text(answer, context);
}

/// The answer for an operation that ran and whose attribution record could not be written:
/// `server_rejected` with no `details`, and nothing that invites a retry.
fn assert_rejected_after_operation(answer: &Answer, context: &str) {
    let body = answer.json();
    assert!(answer.behind_boundary, "{context}");
    assert_eq!(answer.status, StatusCode::OK, "{context}: {body}");
    assert_eq!(body["code"], 500, "{context}: {body}");
    assert_eq!(body["error_code"], "server_rejected", "{context}: {body}");
    assert!(body["details"].is_null(), "{context}: {body}");
    assert!(body["data"].is_null(), "{context}: {body}");
    let text = answer.text();
    assert!(
        !text.contains("server_draining") && !text.contains("retry_after_ms"),
        "{context}: {text}"
    );
    assert_no_driver_text(answer, context);
}

fn assert_server_draining(answer: &Answer, context: &str) {
    let body = answer.json();
    assert_eq!(answer.status, StatusCode::OK, "{context}: {body}");
    assert_eq!(body["code"], 409, "{context}: {body}");
    assert_eq!(body["error_code"], "server_draining", "{context}: {body}");
    assert_eq!(
        body["details"],
        serde_json::json!({"reason": "contention", "retry_after_ms": 1000}),
        "{context}: {body}"
    );
    assert_eq!(body["message"], "server_draining", "{context}: {body}");
    assert!(body["data"].is_null(), "{context}: {body}");
    assert_no_driver_text(answer, context);
}

/// A workspace with Flow enabled and its owner.
/// The list route of the workspace's Flow scope: the scope is created with the workspace's first
/// Flow use, as the API does it.
async fn scope_objects_uri(db: &DatabaseConnection, workspace_id: Uuid) -> String {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT flow_ensure_legacy_scope($1) AS id",
            vec![workspace_id.into()],
        ))
        .await
        .expect("the scope query runs")
        .expect("the scope query answers");
    let scope: Uuid = row.try_get("", "id").expect("the scope id is a UUID");
    format!("/api/v2/flow/scopes/{scope}/objects")
}

async fn seed_flow_workspace(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) \
         VALUES ($1, $2, '!', 'boundary', 'user', true)",
        vec![owner_id.into(), format!("{owner_id}@boundary.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'boundary', $3)",
        vec![
            workspace_id.into(),
            format!("ws-{workspace_id}").into(),
            owner_id.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![workspace_id.into(), owner_id.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace_id.into()],
    )
    .await;
    (workspace_id, owner_id)
}

/// The legacy answer, byte for byte: status, content type and body, outside the boundary.
fn assert_legacy(answer: &Answer, body: &str, context: &str) {
    assert_eq!(answer.status, StatusCode::OK, "{context}");
    assert_eq!(answer.content_type.as_deref(), Some("application/json"), "{context}");
    assert_eq!(answer.text(), body, "{context}");
    assert!(!answer.behind_boundary, "{context}");
}

fn assert_legacy_database_error(answer: &Answer, context: &str) {
    assert_legacy(answer, LEGACY_DATABASE_ERROR_BODY, context);
}

/// Real database failures on one Flow read route (`GET .../scopes/{scope}/objects`), one Flow write
/// route (`PUT .../features/flow`) and one route outside the Flow surface
/// (`GET .../labels`), all through the application router:
///
/// * deterministic (a missing relation, `42P01`; a violated check constraint, `23514`):
///   Flow answers `server_rejected` / `deterministic_database_refusal`;
/// * not deterministic (a serialization failure raised by the database, `40001`; a closed
///   connection pool, no `SQLSTATE`): Flow answers `server_draining` / `contention`;
/// * the label route answers the legacy envelope byte for byte for both kinds.
#[tokio::test]
async fn real_database_failures_are_typed_on_flow_routes_and_unchanged_elsewhere() {
    let Some(scratch) = scratch("flow_surface_boundary").await else {
        eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
        return;
    };
    let db = scratch.connection().clone();
    run_migrations(&db).await.expect("migrations apply");
    let state = state_for(db.clone());
    let (workspace_id, owner_id) = seed_flow_workspace(&db).await;
    let token = token_for(owner_id);
    let read_uri = scope_objects_uri(&db, workspace_id).await;
    let write_uri = format!("/api/v1/workspaces/{workspace_id}/features/flow");
    let label_uri = format!("/api/v1/workspaces/{workspace_id}/labels");
    let read = || call(&state, &Method::GET, &read_uri, Some(&token), None);
    let write = |key: &'static str| {
        let body = serde_json::json!({"enabled": true, "default_member_level": "edit", "idempotency_key": key});
        let (state, uri, token) = (&state, &write_uri, &token);
        async move { call(state, &Method::PUT, uri, Some(token), Some(&body)).await }
    };
    let labels = || call(&state, &Method::GET, &label_uri, Some(&token), None);

    // Healthy: all three succeed, so every failure below is the injected one.
    for (name, answer) in [
        ("read", read().await),
        ("write", write("boundary-healthy").await),
        ("labels", labels().await),
    ] {
        assert_eq!(answer.json()["code"], 0, "{name}: {}", answer.text());
    }

    // ---- deterministic: the database refuses the statement for good ----
    db.execute_unprepared(
        "ALTER TABLE flow_objects RENAME TO flow_objects_renamed; \
         ALTER TABLE labels RENAME TO labels_renamed; \
         ALTER TABLE flow_workspace_settings \
           ADD CONSTRAINT boundary_refusal CHECK (default_member_level <> 'view') NOT VALID",
    )
    .await
    .expect("the deterministic failures are injected");
    assert_server_rejected(&read().await, "read route, missing relation");
    let refused = {
        let body = serde_json::json!({
            "enabled": true, "default_member_level": "view", "idempotency_key": "boundary-check"
        });
        call(&state, &Method::PUT, &write_uri, Some(&token), Some(&body)).await
    };
    assert_server_rejected(&refused, "write route, violated check constraint");
    assert_legacy_database_error(&labels().await, "label route, missing relation");
    db.execute_unprepared(
        "ALTER TABLE flow_objects_renamed RENAME TO flow_objects; \
         ALTER TABLE labels_renamed RENAME TO labels; \
         ALTER TABLE flow_workspace_settings DROP CONSTRAINT boundary_refusal",
    )
    .await
    .expect("the deterministic failures are removed");

    // ---- not deterministic, with an SQLSTATE: the database reports a serialization failure ----
    db.execute_unprepared(
        "CREATE FUNCTION boundary_serialization_failure() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'could not serialize access' USING ERRCODE = '40001'; END $$; \
         CREATE TRIGGER boundary_serialization_failure BEFORE INSERT OR UPDATE ON flow_workspace_settings \
           FOR EACH ROW EXECUTE FUNCTION boundary_serialization_failure()",
    )
    .await
    .expect("the serialization failure is injected");
    assert_server_draining(
        &write("boundary-serialization").await,
        "write route, serialization failure",
    );
    db.execute_unprepared("DROP TRIGGER boundary_serialization_failure ON flow_workspace_settings")
        .await
        .expect("the serialization failure is removed");

    // ---- not deterministic, without an SQLSTATE: the connection pool is closed ----
    db.clone().close().await.expect("the pool closes");
    assert_server_draining(&read().await, "read route, closed pool");
    assert_server_draining(&write("boundary-closed").await, "write route, closed pool");
    assert_legacy_database_error(&labels().await, "label route, closed pool");
    scratch.drop_self().await;
}

/// A bot credential (`opr_...`) and the two database statements the authentication
/// middleware runs for it, the credential lookup before the handler and the attribution
/// record after it. When the lookup fails, a Flow route (`GET .../scopes/{scope}/objects`) answers the
/// typed envelope of that failure, exactly as it does for a failure inside the handler. When
/// the attribution record fails the operation has already run, so the Flow route answers
/// `server_rejected` with no `details` whether the failure is deterministic or transient,
/// and never `server_draining`. A route outside the Flow surface (`GET .../labels`) answers
/// the legacy internal error byte for byte in every case. This also shows each Flow route's own authentication layer runs inside
/// the boundary.
#[tokio::test]
async fn bot_credential_database_failures_are_typed_on_flow_routes_and_unchanged_elsewhere() {
    use sha2::{Digest as _, Sha256};

    let Some(scratch) = scratch("flow_surface_bot").await else {
        eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
        return;
    };
    let db = scratch.connection().clone();
    run_migrations(&db).await.expect("migrations apply");
    let state = state_for(db.clone());
    let (workspace_id, owner_id) = seed_flow_workspace(&db).await;
    let token = format!("opr_{}", Uuid::new_v4().simple());
    // The bot as `bots::create` makes it: a bot user, its seat and its token row.
    let bot_id = Uuid::new_v4();
    exec(
        &db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
         VALUES ($1, $2, '!', 'boundary-bot', 'user', true, 'bot')",
        vec![bot_id.into(), format!("{bot_id}@bot.boundary.test").into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'admin')",
        vec![workspace_id.into(), bot_id.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by) \
         VALUES ($1, $2, 'boundary-bot', $3, 'opr_boun', '[\"read\",\"write\",\"admin\"]'::jsonb, $4)",
        vec![
            bot_id.into(),
            workspace_id.into(),
            hex::encode(Sha256::digest(token.as_bytes())).into(),
            owner_id.into(),
        ],
    )
    .await;
    let flow_uri = scope_objects_uri(&db, workspace_id).await;
    let label_uri = format!("/api/v1/workspaces/{workspace_id}/labels");
    let flow = || call(&state, &Method::GET, &flow_uri, Some(&token), None);
    let labels = || call(&state, &Method::GET, &label_uri, Some(&token), None);

    // Healthy: the credential is accepted on both routes, so every failure below is injected.
    for (name, answer) in [("flow", flow().await), ("labels", labels().await)] {
        assert_eq!(answer.json()["code"], 0, "{name}: {}", answer.text());
    }

    // ---- the credential lookup is refused for good (missing relation) ----
    db.execute_unprepared("ALTER TABLE workspace_bots RENAME TO workspace_bots_renamed")
        .await
        .expect("the lookup failure is injected");
    let answer = flow().await;
    assert!(
        answer.behind_boundary,
        "the authentication layer runs inside the boundary"
    );
    assert_server_rejected(&answer, "Flow route, credential lookup refused");
    assert_legacy(
        &labels().await,
        LEGACY_INTERNAL_ERROR_BODY,
        "label route, credential lookup refused",
    );
    db.execute_unprepared("ALTER TABLE workspace_bots_renamed RENAME TO workspace_bots")
        .await
        .expect("the lookup failure is removed");

    // ---- the attribution record of a successful operation is refused for good ----
    db.execute_unprepared("ALTER TABLE bot_operation_logs RENAME TO bot_operation_logs_renamed")
        .await
        .expect("the attribution failure is injected");
    assert_rejected_after_operation(&flow().await, "Flow route, attribution record refused");
    assert_legacy(
        &labels().await,
        LEGACY_INTERNAL_ERROR_BODY,
        "label route, attribution record refused",
    );
    db.execute_unprepared("ALTER TABLE bot_operation_logs_renamed RENAME TO bot_operation_logs")
        .await
        .expect("the attribution failure is removed");

    // ---- the attribution record fails for a transient reason (a serialization failure) ----
    db.execute_unprepared(
        "CREATE FUNCTION boundary_attribution_failure() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'could not serialize access' USING ERRCODE = '40001'; END $$; \
         CREATE TRIGGER boundary_attribution_failure BEFORE INSERT ON bot_operation_logs \
           FOR EACH ROW EXECUTE FUNCTION boundary_attribution_failure()",
    )
    .await
    .expect("the transient attribution failure is injected");
    assert_rejected_after_operation(&flow().await, "Flow route, attribution record transient failure");
    assert_legacy(
        &labels().await,
        LEGACY_INTERNAL_ERROR_BODY,
        "label route, attribution record transient failure",
    );
    db.execute_unprepared("DROP TRIGGER boundary_attribution_failure ON bot_operation_logs")
        .await
        .expect("the transient attribution failure is removed");
    for (name, answer) in [("flow", flow().await), ("labels", labels().await)] {
        assert_eq!(answer.json()["code"], 0, "{name} after the repairs: {}", answer.text());
    }

    // ---- not deterministic: the connection pool is closed ----
    db.clone().close().await.expect("the pool closes");
    assert_server_draining(&flow().await, "Flow route, closed pool");
    assert_legacy(&labels().await, LEGACY_INTERNAL_ERROR_BODY, "label route, closed pool");
    scratch.drop_self().await;
}

/// What a router with no Flow surface at all answers to a path it does not route: status,
/// content type and body.
async fn unrouted_baseline(method: &Method, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let request = Request::builder()
        .method(method.clone())
        .uri(uri)
        .body(Body::empty())
        .expect("the request is well formed");
    let response = axum::Router::new()
        .route("/health", axum::routing::get(async || "ok"))
        .oneshot(request)
        .await
        .expect("the baseline router answers");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("the baseline body is readable")
        .to_vec();
    (status, content_type, body)
}

/// A path no route matches is answered by the application's fallback, outside the boundary:
/// no marker, and the same status, content type and body as a router that has no Flow surface
/// at all, whether or not the path looks like a Flow path.
#[tokio::test]
async fn an_unrouted_path_answers_outside_the_boundary() {
    let state = state_for(DatabaseConnection::Disconnected);
    let token = token_for(Uuid::new_v4());
    let workspace_id = Uuid::new_v4();
    let mut probes = Vec::new();
    for uri in [
        "/api/v1/no-such-route".to_string(),
        "/api/v2/flow/no-such-route".to_string(),
        "/api/v2/flow/collab/no-such-route".to_string(),
        format!("/api/v2/flow/workspaces/{workspace_id}/no-such-route"),
    ] {
        for method in [Method::GET, Method::POST, Method::DELETE] {
            probes.push((method.clone(), uri.clone(), None));
            probes.push((method, uri.clone(), Some(token.as_str())));
        }
    }
    for (method, uri, caller) in probes {
        let context = format!("{method} {uri}");
        let answer = call(&state, &method, &uri, caller, None).await;
        let (status, content_type, body) = unrouted_baseline(&method, &uri).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{context}");
        assert_eq!(answer.status, status, "{context}");
        assert_eq!(answer.content_type, content_type, "{context}");
        assert_eq!(answer.body, body, "{context}");
        assert!(answer.body.is_empty(), "{context}");
        assert!(
            !answer.behind_boundary,
            "{context}: an unrouted path is answered behind the Flow surface boundary"
        );
    }
}
