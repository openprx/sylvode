//! The management endpoints and the revocation hooks through the real handlers against PostgreSQL.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::struct_field_names
)]

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    routing::{delete, patch, put},
};
use platform::app::{AppState, FlowPermissionCacheSlot};
use platform::auth::JwtManager;
use platform::config::{AppConfig, OAuthConfig, Secret};
use sea_orm::sqlx::postgres::PgListener;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{revoke_user_grants, routes};
use crate::oauth::authorize;
use crate::oauth::grants::{Actor, GRANT_CHANGED_CHANNEL, RevokeReason};
use crate::oauth::redirect::percent_encode;
use crate::oauth::test_support::{Scratch, Start, count, oauth_scratch_or_skip, seed_access_token, seed_grant};
use crate::oauth::token::s256_challenge;
use crate::oauth::tokens::{TokenKind, mint};
use crate::oauth::validate::{AudienceRule, ForwardCredential, validate_access_token};
use crate::routes::context::tenant_fixture::{Tenant, exec, seed_tenant};

const ISSUER: &str = "http://127.0.0.1:18097";
const API_RES: &str = "http://127.0.0.1:18097/api";
const MCP_RES: &str = "http://127.0.0.1:8090/mcp/rpc";
const REDIRECT: &str = "http://127.0.0.1:53682/oauth/sylvode/callback";
const SCOPES: &str = "projects:read work_items:read notifications:read tasks:read offline_access";
const STATE: &str = "state-value-0123456789abcdef";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const JWT_SECRET: &str = "wp7-manage-test-jwt-secret";
const READ_SCOPES: [&str; 3] = ["projects:read", "work_items:read", "offline_access"];

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

struct Person {
    id: Uuid,
    bearer: String,
}

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}

impl Reply {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("not json ({e}): {:?}", self.text()))
    }

    fn data(&self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.text());
        let body = self.json();
        assert_eq!(body["code"], 0, "{body}");
        body["data"].clone()
    }

    #[track_caller]
    fn assert_error(&self, status: u16, code: &str) {
        assert_eq!(self.status.as_u16(), status, "{}", self.text());
        assert_eq!(self.json()["error_code"], code, "{}", self.text());
    }

    /// A legacy handler failed: a non-200 status or a non-zero envelope code.
    #[track_caller]
    fn assert_failed(&self) {
        let failed = self.status != StatusCode::OK || self.json()["code"] != 0;
        assert!(failed, "the handler reported success: {}", self.text());
    }

    #[track_caller]
    fn assert_ok(&self) {
        assert_eq!(self.status, StatusCode::OK, "{}", self.text());
        assert_eq!(self.json()["code"], 0, "{}", self.text());
    }
}

fn config() -> OAuthConfig {
    let mut config = OAuthConfig::with_defaults(ISSUER);
    config.mcp_resource = Some(MCP_RES.to_owned());
    config.internal_forward_secret = Some(Secret::new("forward-secret-for-tests-0123456789abcdef"));
    config
}

fn app_state(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "api-test".to_owned(),
            bind_addr: "127.0.0.1:0".to_owned(),
            database_url: Secret::new("postgres://unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: FlowPermissionCacheSlot::default(),
    }
}

/// The management routes, the authorization server's routes, and the real handlers that carry a
/// revocation hook, registered the way `main.rs` registers them.
fn full_router(state: &AppState) -> Router {
    use crate::middleware::{admin::admin_middleware, bot_auth::bot_or_user_auth_middleware};
    use axum::middleware::from_fn_with_state;
    let cfg = Arc::new(config());
    Router::new()
        .merge(authorize::routes(Some(&cfg)))
        .merge(routes(Some(&cfg)))
        .route(
            "/api/v1/auth/me/password",
            put(crate::routes::auth::update_password)
                .route_layer(from_fn_with_state(state.clone(), bot_or_user_auth_middleware)),
        )
        .route(
            "/api/v1/admin/users/{id}",
            delete(crate::routes::admin::delete_user).route_layer(from_fn_with_state(state.clone(), admin_middleware)),
        )
        .route(
            "/api/v1/admin/users/{id}/status",
            patch(crate::routes::admin::toggle_user_status)
                .route_layer(from_fn_with_state(state.clone(), admin_middleware)),
        )
        .route(
            "/api/v1/admin/users/{id}/password",
            put(crate::routes::admin::reset_user_password)
                .route_layer(from_fn_with_state(state.clone(), admin_middleware)),
        )
        .route(
            "/api/v1/workspaces/{id}",
            delete(crate::routes::workspace::delete_workspace)
                .route_layer(from_fn_with_state(state.clone(), bot_or_user_auth_middleware)),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{user_id}",
            delete(crate::routes::member::remove_member)
                .route_layer(from_fn_with_state(state.clone(), bot_or_user_auth_middleware)),
        )
        .with_state(state.clone())
}

struct Harness {
    scratch: Scratch,
    router: Router,
    tenant: Tenant,
    other: Tenant,
    /// An admin of the first workspace.
    ws_admin: Person,
    /// An owner or admin of the second workspace only.
    other_admin: Person,
    /// An instance administrator (`users.role = 'admin'`).
    inst_admin: Person,
}

fn person_with(id: Uuid, session_version: i64) -> Person {
    let jwt = JwtManager::new(JWT_SECRET, 900, 3600);
    let bearer = jwt
        .issue_access_token_at_session(&id.to_string(), &format!("{id}@tenant.test"), session_version)
        .expect("jwt");
    Person { id, bearer }
}

fn person(id: Uuid) -> Person {
    person_with(id, 0)
}

async fn insert_user(db: &impl ConnectionTrait, role: &str, entity_type: &str) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
         VALUES ($1, $2, '!', 'test user', $3, true, $4)",
        vec![
            id.into(),
            format!("{id}@tenant.test").into(),
            role.into(),
            entity_type.into(),
        ],
    )
    .await;
    id
}

async fn add_member(db: &impl ConnectionTrait, workspace_id: Uuid, user_id: Uuid, role: &str) {
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
        vec![workspace_id.into(), user_id.into(), role.into()],
    )
    .await;
}

impl Harness {
    async fn new(scratch: Scratch) -> Self {
        let db = scratch.db.clone();
        let tenant = seed_tenant(&db, "wp7a").await;
        let other = seed_tenant(&db, "wp7b").await;
        let ws_admin_id = insert_user(&db, "user", "human").await;
        add_member(&db, tenant.workspace_id, ws_admin_id, "admin").await;
        let other_admin_id = insert_user(&db, "user", "human").await;
        add_member(&db, other.workspace_id, other_admin_id, "owner").await;
        let inst_admin_id = insert_user(&db, "admin", "human").await;
        let state = app_state(db);
        Self {
            router: full_router(&state),
            scratch,
            tenant,
            other,
            ws_admin: person(ws_admin_id),
            other_admin: person(other_admin_id),
            inst_admin: person(inst_admin_id),
        }
    }

    fn db(&self) -> &DatabaseConnection {
        &self.scratch.db
    }

    async fn finish(self) {
        self.scratch.drop_self().await;
    }

    fn member(&self) -> Person {
        person(self.tenant.member_id)
    }

    async fn n(&self, sql: &str) -> i64 {
        count(self.db(), sql, vec![]).await
    }

    async fn send(&self, req: Request<Body>) -> Reply {
        send_on(self.router.clone(), req).await
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        headers: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Reply {
        self.send(request(method, path, token, headers, body)).await
    }

    async fn patch_client(&self, status: Value, client: &str) -> Reply {
        self.call(
            Method::PATCH,
            &format!("/api/v1/admin/oauth-clients/{client}"),
            Some(&self.inst_admin.bearer),
            &[],
            Some(&json!({"status": status})),
        )
        .await
    }

    async fn get(&self, path: &str, who: &Person) -> Reply {
        self.call(Method::GET, path, Some(&who.bearer), &[], None).await
    }

    async fn del(&self, path: &str, who: &Person) -> Reply {
        self.call(Method::DELETE, path, Some(&who.bearer), &[], None).await
    }

    /// A new user in the first workspace and (as `member`) in the second.
    async fn user_in_both(&self) -> Person {
        let id = insert_user(self.db(), "user", "human").await;
        add_member(self.db(), self.tenant.workspace_id, id, "member").await;
        add_member(self.db(), self.other.workspace_id, id, "member").await;
        person(id)
    }

    /// An active grant with one live access token and one live refresh token.
    async fn grant(&self, user: Uuid, workspace: Uuid, mode: &str) -> Uuid {
        let grant = seed_grant(self.db(), user, workspace, mode, &READ_SCOPES).await;
        seed_access_token(self.db(), grant, "api", &["projects:read"]).await;
        seed_refresh_token(self.db(), grant).await;
        grant
    }

    async fn listener(&self) -> PgListener {
        let mut listener = PgListener::connect_with(self.db().get_postgres_connection_pool())
            .await
            .expect("listener");
        listener.listen(GRANT_CHANGED_CHANNEL).await.expect("listen");
        listener
    }

    async fn grant_state(&self, grant: Uuid) -> (String, Option<String>, Option<Uuid>) {
        #[derive(FromQueryResult)]
        struct Row {
            status: String,
            revoke_reason: Option<String>,
            revoked_by: Option<Uuid>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, revoke_reason, revoked_by FROM oauth_grants WHERE id = $1",
            vec![grant.into()],
        ))
        .one(self.db())
        .await
        .expect("query")
        .expect("grant exists");
        (row.status, row.revoke_reason, row.revoked_by)
    }

    async fn live_tokens(&self, grant: Uuid) -> i64 {
        count(
            self.db(),
            "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1 AND revoked_at IS NULL",
            vec![grant.into()],
        )
        .await
    }

    async fn audit(&self, event: &str, grant: Uuid) -> Vec<(Option<Uuid>, Option<String>)> {
        #[derive(FromQueryResult)]
        struct Row {
            actor_id: Option<Uuid>,
            reason: Option<String>,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT actor_id, detail ->> 'reason' AS reason FROM oauth_audit_events \
             WHERE event = $1 AND grant_id = $2 ORDER BY occurred_at, id",
            vec![event.into(), grant.into()],
        ))
        .all(self.db())
        .await
        .expect("query")
        .into_iter()
        .map(|row| (row.actor_id, row.reason))
        .collect()
    }

    async fn assert_revoked(&self, grant: Uuid, reason: &str, by: Option<Uuid>) {
        let (status, found, revoked_by) = self.grant_state(grant).await;
        assert_eq!(status, "revoked", "grant {grant}");
        assert_eq!(found.as_deref(), Some(reason));
        assert_eq!(revoked_by, by);
        assert_eq!(self.live_tokens(grant).await, 0, "every token of the grant is revoked");
        assert_eq!(
            self.audit("grant_revoked", grant).await,
            vec![(by, Some(reason.to_owned()))],
            "one audit row with the actor and the reason"
        );
    }

    async fn assert_active(&self, grant: Uuid) {
        assert_eq!(self.grant_state(grant).await.0, "active", "grant {grant}");
        assert!(self.live_tokens(grant).await > 0);
        assert!(self.audit("grant_revoked", grant).await.is_empty());
    }
}

async fn seed_refresh_token(db: &impl ConnectionTrait, grant: Uuid) -> String {
    let minted = mint(TokenKind::Refresh).expect("mint");
    exec(
        db,
        "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, scopes, expires_at) \
         VALUES ($1, $2, 'refresh', $3, 'ramflux', ARRAY['projects:read','offline_access'], now() + interval '30 days')",
        vec![Uuid::new_v4().into(), minted.hash.as_hex().into(), grant.into()],
    )
    .await;
    minted.raw.expose().to_owned()
}

async fn seed_code(db: &impl ConnectionTrait, grant: Uuid) -> String {
    let minted = mint(TokenKind::AuthorizationCode).expect("mint");
    exec(
        db,
        "INSERT INTO oauth_authorization_codes (code_hash, grant_id, client_id, redirect_uri, code_challenge, \
         resources, expires_at) VALUES ($1, $2, 'ramflux', $3, $4, ARRAY['api','mcp'], now() + interval '60 seconds')",
        vec![
            minted.hash.as_hex().into(),
            grant.into(),
            REDIRECT.into(),
            s256_challenge(VERIFIER).into(),
        ],
    )
    .await;
    minted.raw.expose().to_owned()
}

fn request(
    method: Method,
    path: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request")
}

async fn send_on(router: Router, req: Request<Body>) -> Reply {
    let response = router.oneshot(req).await.expect("router answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body")
        .to_vec();
    Reply { status, body }
}

fn form_request(path: &str, pairs: &[(&str, &str)]) -> Request<Body> {
    let mut body = String::new();
    for (name, value) in pairs {
        if !body.is_empty() {
            body.push('&');
        }
        write!(body, "{}={}", percent_encode(name), percent_encode(value)).expect("write");
    }
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("request")
}

fn apps_path(grant: Uuid) -> String {
    format!("/api/v1/me/connected-apps/{grant}")
}

fn ws_apps_path(workspace: Uuid) -> String {
    format!("/api/v1/workspaces/{workspace}/connected-apps")
}

fn ws_app_path(workspace: Uuid, grant: Uuid) -> String {
    format!("/api/v1/workspaces/{workspace}/connected-apps/{grant}")
}

fn settings_path(workspace: Uuid) -> String {
    format!("/api/v1/workspaces/{workspace}/connected-apps/settings")
}

/// Payloads of `sv_oauth_grant_changed` that arrive within `wait`.
async fn notifications(listener: &mut PgListener, wait: Duration) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(Ok(notification)) = tokio::time::timeout(wait, listener.recv()).await {
        seen.push(notification.payload().to_owned());
    }
    seen.sort();
    seen
}

fn sorted_ids(ids: &[Uuid]) -> Vec<String> {
    let mut out: Vec<String> = ids.iter().map(ToString::to_string).collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------------------------
// Level 1: the person's own connections
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_person_lists_only_their_own_active_grants_without_token_material() {
    let scratch = oauth_scratch_or_skip!("wp7_list_own", Start::Complete);
    let h = Harness::new(scratch).await;
    let me = h.member();
    let stranger = h.user_in_both().await;

    let all = h.grant(me.id, h.tenant.workspace_id, "all").await;
    let selected = h.grant(me.id, h.tenant.workspace_id, "selected").await;
    exec(
        h.db(),
        "INSERT INTO oauth_grant_projects (grant_id, project_id) VALUES ($1, $2)",
        vec![selected.into(), h.tenant.project_id.into()],
    )
    .await;
    exec(
        h.db(),
        "UPDATE oauth_grants SET device_name = 'MacBook', last_used_at = now() WHERE id = $1",
        vec![selected.into()],
    )
    .await;
    let revoked = h.grant(me.id, h.tenant.workspace_id, "all").await;
    exec(
        h.db(),
        "UPDATE oauth_grants SET status = 'revoked', revoked_at = now(), revoke_reason = 'user_revoked' WHERE id = $1",
        vec![revoked.into()],
    )
    .await;
    let expired = h.grant(me.id, h.tenant.workspace_id, "all").await;
    exec(
        h.db(),
        "UPDATE oauth_grants SET expires_at = now() - interval '1 day' WHERE id = $1",
        vec![expired.into()],
    )
    .await;
    let theirs = h.grant(stranger.id, h.tenant.workspace_id, "all").await;

    let reply = h.get("/api/v1/me/connected-apps", &me).await;
    let data = reply.data();
    let items = data["items"].as_array().expect("items");
    let ids: Vec<&str> = items.iter().map(|i| i["grant_id"].as_str().unwrap()).collect();
    assert_eq!(items.len(), 2, "{data}");
    assert!(ids.contains(&all.to_string().as_str()) && ids.contains(&selected.to_string().as_str()));
    assert!(!ids.contains(&theirs.to_string().as_str()));
    // Newest first.
    assert_eq!(ids[0], selected.to_string());

    let item = &items[0];
    assert_eq!(
        item["client"],
        json!({"client_id": "ramflux", "display_name": "Ramflux"})
    );
    assert_eq!(item["device_name"], "MacBook");
    assert_eq!(item["workspace"]["id"], h.tenant.workspace_id.to_string());
    assert_eq!(item["project_mode"], "selected");
    assert_eq!(item["projects"].as_array().unwrap().len(), 1);
    assert_eq!(item["projects"][0]["id"], h.tenant.project_id.to_string());
    assert_eq!(item["project_count"], 1);
    assert!(item["consented_at"].is_string() && item["last_used_at"].is_string() && item["expires_at"].is_string());
    assert!(
        item.get("user").is_none(),
        "a person's own list does not repeat the person"
    );
    assert_eq!(items[1]["projects"], json!([]));
    assert_eq!(items[1]["project_count"], Value::Null);
    assert_eq!(data["has_more"], false);

    assert_no_token_material(&h, &reply.text()).await;
    h.finish().await;
}

/// The response carries no token, hash or token row id.
async fn assert_no_token_material(h: &Harness, text: &str) {
    for needle in ["sva_", "svr_", "svc_", "token_hash", "code_hash", "consent_token"] {
        assert!(!text.contains(needle), "response contains {needle}: {text}");
    }
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        token_hash: String,
    }
    let rows = Row::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT id, token_hash FROM oauth_tokens".to_owned(),
    ))
    .all(h.db())
    .await
    .expect("tokens");
    assert!(!rows.is_empty());
    for row in rows {
        assert!(!text.contains(&row.id.to_string()), "a token row id leaked");
        assert!(!text.contains(&row.token_hash), "a token hash leaked");
    }
}

#[tokio::test]
async fn a_person_disconnects_their_own_grant_and_nobody_elses() {
    let scratch = oauth_scratch_or_skip!("wp7_disconnect_own", Start::Complete);
    let h = Harness::new(scratch).await;
    let me = h.member();
    let stranger = h.user_in_both().await;
    let mine = h.grant(me.id, h.tenant.workspace_id, "all").await;
    let theirs = h.grant(stranger.id, h.tenant.workspace_id, "all").await;
    let mut listener = h.listener().await;

    // Another person's grant answers exactly as one that does not exist.
    let foreign = h.del(&apps_path(theirs), &me).await;
    foreign.assert_error(404, "not_found");
    let unknown = h.del(&apps_path(Uuid::new_v4()), &me).await;
    assert_eq!(foreign.body, unknown.body);
    h.del("/api/v1/me/connected-apps/not-a-uuid", &me)
        .await
        .assert_error(404, "not_found");
    h.assert_active(theirs).await;
    h.assert_active(mine).await;
    assert!(
        notifications(&mut listener, Duration::from_millis(300))
            .await
            .is_empty()
    );

    let reply = h.del(&apps_path(mine), &me).await;
    assert_eq!(reply.data(), json!({"grant_id": mine, "revoked": true}));
    h.assert_revoked(mine, "user_revoked", Some(me.id)).await;
    h.assert_active(theirs).await;
    assert_eq!(
        notifications(&mut listener, Duration::from_millis(500)).await,
        vec![mine.to_string()]
    );

    // Twice is harmless: not found, no second audit row, no second notification.
    h.del(&apps_path(mine), &me).await.assert_error(404, "not_found");
    assert_eq!(h.audit("grant_revoked", mine).await.len(), 1);
    assert!(
        notifications(&mut listener, Duration::from_millis(300))
            .await
            .is_empty()
    );
    assert!(
        h.get("/api/v1/me/connected-apps", &me).await.data()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Level 2: workspace administration
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_workspace_admin_sees_and_revokes_only_their_workspace_grants() {
    let scratch = oauth_scratch_or_skip!("wp7_ws_admin", Start::Complete);
    let h = Harness::new(scratch).await;
    let user = h.user_in_both().await;
    let here = h.grant(user.id, h.tenant.workspace_id, "all").await;
    let there = h.grant(user.id, h.other.workspace_id, "all").await;
    let ws = h.tenant.workspace_id;

    let reply = h.get(&ws_apps_path(ws), &h.ws_admin).await;
    let data = reply.data();
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{data}");
    assert_eq!(items[0]["grant_id"], here.to_string());
    assert_eq!(items[0]["user"]["id"], user.id.to_string());
    assert!(items[0]["user"]["name"].is_string() && items[0]["user"]["email"].is_string());
    assert_eq!(items[0]["client"]["display_name"], "Ramflux");
    assert_eq!(items[0]["scopes"].as_array().unwrap().len(), READ_SCOPES.len());
    assert_no_token_material(&h, &reply.text()).await;

    // A plain member is refused, a non-member is told nothing, an admin of another workspace
    // sees and can do nothing here.
    h.get(&ws_apps_path(ws), &h.member())
        .await
        .assert_error(403, "forbidden");
    h.get(&ws_apps_path(ws), &h.other_admin)
        .await
        .assert_error(404, "not_found");
    h.get(&ws_apps_path(Uuid::new_v4()), &h.ws_admin)
        .await
        .assert_error(404, "not_found");
    h.get(&ws_apps_path(ws), &h.inst_admin)
        .await
        .assert_error(404, "not_found");
    h.del(&ws_app_path(ws, here), &h.member())
        .await
        .assert_error(403, "forbidden");
    h.del(&ws_app_path(ws, here), &h.other_admin)
        .await
        .assert_error(404, "not_found");
    // The admin of this workspace can not reach a grant of another workspace through their own.
    h.del(&ws_app_path(ws, there), &h.ws_admin)
        .await
        .assert_error(404, "not_found");
    h.del(&ws_app_path(h.other.workspace_id, there), &h.ws_admin)
        .await
        .assert_error(404, "not_found");
    h.assert_active(here).await;
    h.assert_active(there).await;

    let mut listener = h.listener().await;
    h.del(&ws_app_path(ws, here), &h.ws_admin).await.data();
    h.assert_revoked(here, "workspace_admin_revoked", Some(h.ws_admin.id))
        .await;
    h.assert_active(there).await;
    assert_eq!(
        notifications(&mut listener, Duration::from_millis(500)).await,
        vec![here.to_string()]
    );
    h.del(&ws_app_path(ws, here), &h.ws_admin)
        .await
        .assert_error(404, "not_found");
    assert_eq!(h.audit("grant_revoked", here).await.len(), 1);
    h.finish().await;
}

/// A consent attempt up to and including the decision. Returns the decision reply.
async fn try_consent(h: &Harness, who: &Person, workspace: Uuid) -> Reply {
    let begin = json!({
        "response_type": "code",
        "client_id": "ramflux",
        "redirect_uri": REDIRECT,
        "scope": SCOPES,
        "state": STATE,
        "code_challenge": s256_challenge(VERIFIER),
        "code_challenge_method": "S256",
        "resource": [API_RES, MCP_RES],
    });
    let data = h
        .call(
            Method::POST,
            "/api/v1/oauth/authorization-requests",
            Some(&who.bearer),
            &[],
            Some(&begin),
        )
        .await
        .data();
    assert_eq!(data["kind"], "consent", "{data}");
    let request_id = data["request_id"].as_str().unwrap().to_owned();
    let consent = data["consent_token"].as_str().unwrap().to_owned();
    h.call(
        Method::POST,
        &format!("/api/v1/oauth/authorization-requests/{request_id}/decision"),
        Some(&who.bearer),
        &[("x-sylvode-consent", &consent)],
        Some(&json!({
            "decision": "allow",
            "workspace_id": workspace,
            "project_mode": "all",
            "scopes": SCOPES.split(' ').collect::<Vec<_>>(),
        })),
    )
    .await
}

#[tokio::test]
async fn switching_connected_applications_off_revokes_the_workspace_and_blocks_consent() {
    let scratch = oauth_scratch_or_skip!("wp7_settings", Start::Complete);
    let h = Harness::new(scratch).await;
    let user = h.user_in_both().await;
    let ws = h.tenant.workspace_id;
    let a = h.grant(user.id, ws, "all").await;
    let b = h.grant(h.tenant.member_id, ws, "selected").await;
    let elsewhere = h.grant(user.id, h.other.workspace_id, "all").await;
    let path = settings_path(ws);

    assert_eq!(
        h.get(&path, &h.ws_admin).await.data(),
        json!({"allow_connected_apps": true})
    );
    // Wrong callers.
    h.get(&path, &h.member()).await.assert_error(403, "forbidden");
    h.get(&path, &h.other_admin).await.assert_error(404, "not_found");
    for who in [&h.member(), &h.other_admin, &h.inst_admin] {
        let reply = h
            .call(
                Method::PUT,
                &path,
                Some(&who.bearer),
                &[],
                Some(&json!({"allow_connected_apps": false})),
            )
            .await;
        assert!(
            reply.status == StatusCode::FORBIDDEN || reply.status == StatusCode::NOT_FOUND,
            "{}",
            reply.text()
        );
    }
    h.assert_active(a).await;
    assert_eq!(h.n("SELECT count(*) AS n FROM workspace_delegation_settings").await, 0);

    // Bad bodies.
    let bearer = Some(h.ws_admin.bearer.as_str());
    h.call(
        Method::PUT,
        &path,
        bearer,
        &[],
        Some(&json!({"allow_connected_apps": "no"})),
    )
    .await
    .assert_error(400, "invalid_request");
    h.call(
        Method::PUT,
        &path,
        bearer,
        &[],
        Some(&json!({"allow_connected_apps": false, "extra": 1})),
    )
    .await
    .assert_error(400, "invalid_request");
    let wrong_type = Request::put(&path)
        .header(header::AUTHORIZATION, format!("Bearer {}", h.ws_admin.bearer))
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(r#"{"allow_connected_apps":false}"#))
        .unwrap();
    h.send(wrong_type).await.assert_error(400, "invalid_request");

    let mut listener = h.listener().await;
    let reply = h
        .call(
            Method::PUT,
            &path,
            bearer,
            &[],
            Some(&json!({"allow_connected_apps": false})),
        )
        .await;
    assert_eq!(
        reply.data(),
        json!({"allow_connected_apps": false, "revoked_grants": 2})
    );
    h.assert_revoked(a, "workspace_disabled", Some(h.ws_admin.id)).await;
    h.assert_revoked(b, "workspace_disabled", Some(h.ws_admin.id)).await;
    h.assert_active(elsewhere).await;
    assert_eq!(
        notifications(&mut listener, Duration::from_millis(500)).await,
        sorted_ids(&[a, b])
    );
    assert_eq!(
        h.get(&path, &h.ws_admin).await.data(),
        json!({"allow_connected_apps": false})
    );
    #[derive(FromQueryResult)]
    struct Setting {
        actor_id: Option<Uuid>,
        flag: Option<String>,
        revoked: Option<String>,
    }
    let rows = Setting::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT actor_id, detail ->> 'allow_connected_apps' AS flag, detail ->> 'revoked_grants' AS revoked \
         FROM oauth_audit_events WHERE event = 'workspace_setting_changed'"
            .to_owned(),
    ))
    .all(h.db())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(h.ws_admin.id));
    assert_eq!(rows[0].flag.as_deref(), Some("false"));
    assert_eq!(rows[0].revoked.as_deref(), Some("2"));

    // New consent for the workspace is refused with the documented error; the other workspace is not.
    try_consent(&h, &user, ws)
        .await
        .assert_error(403, "workspace_not_eligible");
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'active'")
            .await,
        1
    );
    try_consent(&h, &user, h.other.workspace_id).await.data();

    // Switching off again is harmless, switching on allows consent again.
    let again = h
        .call(
            Method::PUT,
            &path,
            bearer,
            &[],
            Some(&json!({"allow_connected_apps": false})),
        )
        .await;
    assert_eq!(again.data()["revoked_grants"], 0);
    let on = h
        .call(
            Method::PUT,
            &path,
            bearer,
            &[],
            Some(&json!({"allow_connected_apps": true})),
        )
        .await;
    assert_eq!(on.data(), json!({"allow_connected_apps": true, "revoked_grants": 0}));
    h.assert_revoked(a, "workspace_disabled", Some(h.ws_admin.id)).await;
    try_consent(&h, &user, ws).await.data();
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Level 3: the instance administrator
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_instance_admin_lists_disables_and_re_enables_the_client() {
    let scratch = oauth_scratch_or_skip!("wp7_client", Start::Complete);
    let h = Harness::new(scratch).await;
    let user = h.user_in_both().await;
    let a = h.grant(user.id, h.tenant.workspace_id, "all").await;
    let b = h.grant(user.id, h.other.workspace_id, "all").await;
    let refresh = seed_refresh_token(h.db(), a).await;

    for who in [&h.member(), &h.ws_admin, &h.other_admin] {
        h.get("/api/v1/admin/oauth-clients", who)
            .await
            .assert_error(403, "forbidden");
        h.call(
            Method::PATCH,
            "/api/v1/admin/oauth-clients/ramflux",
            Some(&who.bearer),
            &[],
            Some(&json!({"status": "disabled"})),
        )
        .await
        .assert_error(403, "forbidden");
    }
    h.assert_active(a).await;

    let list = h.get("/api/v1/admin/oauth-clients", &h.inst_admin).await;
    let data = list.data();
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["client_id"], "ramflux");
    assert_eq!(items[0]["status"], "active");
    assert_eq!(items[0]["active_grants"], 2);
    for key in [
        "token_endpoint_auth_method",
        "embed_origins",
        "status_changed_by",
        "updated_at",
    ] {
        assert!(items[0].get(key).is_none(), "{key} is internal");
    }

    h.patch_client(json!("deleted"), "ramflux")
        .await
        .assert_error(400, "invalid_request");
    h.patch_client(json!("disabled"), "nobody")
        .await
        .assert_error(404, "not_found");
    let bad = h
        .call(
            Method::PATCH,
            "/api/v1/admin/oauth-clients/ramflux",
            Some(&h.inst_admin.bearer),
            &[],
            Some(&json!({"status": "disabled", "redirect_uris": ["http://evil"]})),
        )
        .await;
    bad.assert_error(400, "invalid_request");
    h.assert_active(a).await;

    let mut listener = h.listener().await;
    let off = h.patch_client(json!("disabled"), "ramflux").await;
    assert_eq!(
        off.data(),
        json!({"client_id": "ramflux", "status": "disabled", "revoked_grants": 2})
    );
    h.assert_revoked(a, "client_disabled", Some(h.inst_admin.id)).await;
    h.assert_revoked(b, "client_disabled", Some(h.inst_admin.id)).await;
    assert_eq!(
        notifications(&mut listener, Duration::from_millis(500)).await,
        sorted_ids(&[a, b])
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_audit_events WHERE event = 'client_status_changed' AND actor_id IS NOT NULL")
            .await,
        1
    );

    // The token endpoint refuses the revoked refresh token, and new consent finds no client.
    let refreshed = h
        .send(form_request(
            "/api/v1/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", "ramflux"),
                ("resource", API_RES),
            ],
        ))
        .await;
    assert!(refreshed.status.is_client_error(), "{}", refreshed.text());
    let begin = json!({
        "response_type": "code", "client_id": "ramflux", "redirect_uri": REDIRECT, "scope": SCOPES,
        "state": STATE, "code_challenge": s256_challenge(VERIFIER), "code_challenge_method": "S256",
        "resource": [API_RES],
    });
    let page = h
        .call(
            Method::POST,
            "/api/v1/oauth/authorization-requests",
            Some(&user.bearer),
            &[],
            Some(&begin),
        )
        .await
        .data();
    assert_eq!(page["kind"], "error_page");
    assert_eq!(
        h.get("/api/v1/admin/oauth-clients", &h.inst_admin).await.data()["items"][0]["status"],
        "disabled"
    );

    // Disabling again is harmless; re-enabling resurrects nothing but allows new consent.
    assert_eq!(
        h.patch_client(json!("disabled"), "ramflux").await.data()["revoked_grants"],
        0
    );
    assert_eq!(
        h.patch_client(json!("active"), "ramflux").await.data()["status"],
        "active"
    );
    h.assert_revoked(a, "client_disabled", Some(h.inst_admin.id)).await;
    try_consent(&h, &user, h.tenant.workspace_id).await.data();
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Callers that are refused everywhere
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn bot_and_delegated_tokens_are_refused_on_every_management_endpoint() {
    let scratch = oauth_scratch_or_skip!("wp7_wrong_callers", Start::Complete);
    let h = Harness::new(scratch).await;
    let ws = h.tenant.workspace_id;
    // The same people who would be allowed with a session own the grant behind the delegated token.
    let grant = h.grant(h.ws_admin.id, ws, "all").await;
    let (_, delegated) = seed_access_token(h.db(), grant, "api", &["projects:read", "work_items:write"]).await;
    let bot = format!("opr_{}", "a".repeat(40));
    let allow_off = json!({"allow_connected_apps": false});
    let disable = json!({"status": "disabled"});
    let endpoints: Vec<(Method, String, Option<&Value>)> = vec![
        (Method::GET, "/api/v1/me/connected-apps".to_owned(), None),
        (Method::DELETE, apps_path(grant), None),
        (Method::GET, ws_apps_path(ws), None),
        (Method::DELETE, ws_app_path(ws, grant), None),
        (Method::GET, settings_path(ws), None),
        (Method::PUT, settings_path(ws), Some(&allow_off)),
        (Method::GET, "/api/v1/admin/oauth-clients".to_owned(), None),
        (
            Method::PATCH,
            "/api/v1/admin/oauth-clients/ramflux".to_owned(),
            Some(&disable),
        ),
    ];
    for (method, path, body) in &endpoints {
        for token in [&delegated, &bot] {
            let reply = h.call(method.clone(), path, Some(token), &[], *body).await;
            reply.assert_error(403, "forbidden");
            // A valid session next to it does not rescue the request.
            let mixed = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::COOKIE, format!("access_token={}", h.inst_admin.bearer))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.map(ToString::to_string).unwrap_or_default()))
                .unwrap();
            h.send(mixed).await.assert_error(403, "forbidden");
            // Nor does the token travelling as the session cookie.
            let cookie = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::COOKIE, format!("access_token={token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.map(ToString::to_string).unwrap_or_default()))
                .unwrap();
            h.send(cookie).await.assert_error(403, "forbidden");
        }
        // No credentials at all, and a malformed one: 401.
        h.call(method.clone(), path, None, &[], *body)
            .await
            .assert_error(401, "unauthorized");
        h.call(method.clone(), path, Some("not-a-jwt"), &[], *body)
            .await
            .assert_error(401, "unauthorized");
    }
    h.assert_active(grant).await;
    assert_eq!(h.n("SELECT count(*) AS n FROM workspace_delegation_settings").await, 0);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_clients WHERE status = 'disabled'")
            .await,
        0
    );

    // The delegated token itself still works where it belongs: it was never revoked.
    assert!(
        validate_access_token(h.db(), &delegated, AudienceRule::Api(ForwardCredential::absent()))
            .await
            .is_ok()
    );
    h.finish().await;
}

#[tokio::test]
async fn sessions_that_are_not_live_people_are_refused() {
    let scratch = oauth_scratch_or_skip!("wp7_sessions", Start::Complete);
    let h = Harness::new(scratch).await;
    let me = h.member();
    // Disabled account, stale session version, a bot user's JWT.
    exec(
        h.db(),
        "UPDATE users SET is_active = false WHERE id = $1",
        vec![me.id.into()],
    )
    .await;
    h.get("/api/v1/me/connected-apps", &me)
        .await
        .assert_error(401, "unauthorized");
    exec(
        h.db(),
        "UPDATE users SET is_active = true, session_version = 5 WHERE id = $1",
        vec![me.id.into()],
    )
    .await;
    h.get("/api/v1/me/connected-apps", &me)
        .await
        .assert_error(401, "unauthorized");
    h.get("/api/v1/me/connected-apps", &person_with(me.id, 5)).await.data();
    let bot_user = person(insert_user(h.db(), "user", "bot").await);
    h.get("/api/v1/me/connected-apps", &bot_user)
        .await
        .assert_error(401, "unauthorized");
    h.finish().await;
}

#[tokio::test]
async fn management_routes_do_not_exist_without_an_issuer() {
    let scratch = oauth_scratch_or_skip!("wp7_unconfigured", Start::Complete);
    let h = Harness::new(scratch).await;
    let router = routes(None).with_state(app_state(h.db().clone()));
    let me = h.member();
    for path in [
        "/api/v1/me/connected-apps".to_owned(),
        apps_path(Uuid::new_v4()),
        ws_apps_path(h.tenant.workspace_id),
        settings_path(h.tenant.workspace_id),
        "/api/v1/admin/oauth-clients".to_owned(),
    ] {
        let reply = send_on(router.clone(), request(Method::GET, &path, Some(&me.bearer), &[], None)).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
    }
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Cross-site request forgery
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn state_changing_requests_from_another_origin_are_refused() {
    let scratch = oauth_scratch_or_skip!("wp7_csrf", Start::Complete);
    let h = Harness::new(scratch).await;
    let me = h.member();
    let ws = h.tenant.workspace_id;
    let mine = h.grant(me.id, ws, "all").await;
    let off = json!({"allow_connected_apps": false});
    let disable = json!({"status": "disabled"});
    let cases: Vec<(Method, String, &Person, Option<&Value>)> = vec![
        (Method::DELETE, apps_path(mine), &me, None),
        (Method::DELETE, ws_app_path(ws, mine), &h.ws_admin, None),
        (Method::PUT, settings_path(ws), &h.ws_admin, Some(&off)),
        (
            Method::PATCH,
            "/api/v1/admin/oauth-clients/ramflux".to_owned(),
            &h.inst_admin,
            Some(&disable),
        ),
    ];
    for (method, path, who, body) in &cases {
        // The session as a cookie, which is how a browser sends it.
        for extra in [
            ("origin", "https://evil.example"),
            ("origin", "null"),
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-site", "same-site"),
        ] {
            let req = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::COOKIE, format!("access_token={}", who.bearer))
                .header(header::CONTENT_TYPE, "application/json")
                .header(extra.0, extra.1)
                .body(Body::from(body.map(ToString::to_string).unwrap_or_default()))
                .unwrap();
            h.send(req).await.assert_error(403, "csrf_origin_mismatch");
        }
    }
    h.assert_active(mine).await;
    assert_eq!(h.n("SELECT count(*) AS n FROM workspace_delegation_settings").await, 0);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_clients WHERE status = 'disabled'")
            .await,
        0
    );

    // Reads are not state changing; a same-origin write goes through, with the cookie.
    let read = Request::builder()
        .uri("/api/v1/me/connected-apps")
        .header(header::COOKIE, format!("access_token={}", me.bearer))
        .header("origin", "https://evil.example")
        .body(Body::empty())
        .unwrap();
    h.send(read).await.data();
    let same = Request::builder()
        .method(Method::DELETE)
        .uri(apps_path(mine))
        .header(header::COOKIE, format!("access_token={}", me.bearer))
        .header("origin", ISSUER)
        .header("sec-fetch-site", "same-origin")
        .body(Body::empty())
        .unwrap();
    h.send(same).await.data();
    h.assert_revoked(mine, "user_revoked", Some(me.id)).await;
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn lists_are_bounded_and_paginated() {
    let scratch = oauth_scratch_or_skip!("wp7_pages", Start::Complete);
    let h = Harness::new(scratch).await;
    let me = h.member();
    let mut made = Vec::new();
    for _ in 0..7 {
        made.push(h.grant(me.id, h.tenant.workspace_id, "all").await);
    }
    let first = h.get("/api/v1/me/connected-apps?limit=3", &me).await.data();
    assert_eq!(first["items"].as_array().unwrap().len(), 3);
    assert_eq!(first["has_more"], true);
    assert_eq!(first["next_offset"], 3);
    let mut seen: Vec<String> = Vec::new();
    let mut offset = 0;
    loop {
        let page = h
            .get(&format!("/api/v1/me/connected-apps?limit=3&offset={offset}"), &me)
            .await
            .data();
        seen.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["grant_id"].as_str().unwrap().to_owned()),
        );
        match page["next_offset"].as_i64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    let mut expected: Vec<String> = made.iter().map(ToString::to_string).collect();
    expected.sort();
    seen.sort();
    assert_eq!(seen, expected, "the pages together are every grant once");
    let last = h.get("/api/v1/me/connected-apps?limit=3&offset=6", &me).await.data();
    assert_eq!(last["items"].as_array().unwrap().len(), 1);
    assert_eq!(last["has_more"], false);
    assert_eq!(last["next_offset"], Value::Null);
    assert_eq!(h.get("/api/v1/me/connected-apps", &me).await.data()["limit"], 50);
    assert_eq!(
        h.get("/api/v1/me/connected-apps?limit=100", &me).await.data()["limit"],
        100
    );
    for bad in [
        "limit=0",
        "limit=101",
        "limit=abc",
        "limit=-1",
        "offset=-1",
        "offset=10001",
        "limit=1&limit=2",
    ] {
        h.get(&format!("/api/v1/me/connected-apps?{bad}"), &me)
            .await
            .assert_error(400, "invalid_request");
    }
    h.get(
        &format!("{}?limit=101", ws_apps_path(h.tenant.workspace_id)),
        &h.ws_admin,
    )
    .await
    .assert_error(400, "invalid_request");
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// The hooks, through the real handlers
// ---------------------------------------------------------------------------------------------

/// One user with a grant in each workspace, and a control user with a grant that must survive.
struct Subject {
    user: Person,
    here: Uuid,
    there: Uuid,
    control: Uuid,
}

async fn subject(h: &Harness) -> Subject {
    let user = h.user_in_both().await;
    let control_user = h.user_in_both().await;
    Subject {
        here: h.grant(user.id, h.tenant.workspace_id, "all").await,
        there: h.grant(user.id, h.other.workspace_id, "selected").await,
        control: h.grant(control_user.id, h.tenant.workspace_id, "all").await,
        user,
    }
}

async fn set_password(h: &Harness, user: Uuid, password: &str) {
    let hash = crate::routes::auth::hash_password(password.to_owned())
        .await
        .expect("hash");
    exec(
        h.db(),
        "UPDATE users SET password_hash = $1 WHERE id = $2",
        vec![hash.into(), user.into()],
    )
    .await;
}

/// The names of the real handlers that carry a hook.
#[derive(Clone, Copy, Debug)]
enum Hook {
    PasswordChange,
    AdminReset,
    AdminDisable,
    AdminDelete,
    MemberRemoval,
}

const ALL_HOOKS: [Hook; 5] = [
    Hook::PasswordChange,
    Hook::AdminReset,
    Hook::AdminDisable,
    Hook::AdminDelete,
    Hook::MemberRemoval,
];

impl Hook {
    fn reason(self) -> &'static str {
        match self {
            Self::PasswordChange | Self::AdminReset => "password_changed",
            Self::AdminDisable | Self::AdminDelete => "account_disabled",
            Self::MemberRemoval => "membership_removed",
        }
    }

    /// Whether the grant in the second workspace ends too.
    const fn ends_every_grant(self) -> bool {
        !matches!(self, Self::MemberRemoval)
    }

    /// The person recorded as the actor, if any (admin handlers do not receive the caller).
    fn actor(self, h: &Harness, s: &Subject) -> Option<Uuid> {
        match self {
            Self::PasswordChange => Some(s.user.id),
            Self::MemberRemoval => Some(h.ws_admin.id),
            _ => None,
        }
    }

    /// What the handler needs in place before it is called.
    async fn prepare(self, h: &Harness, s: &Subject) {
        if matches!(self, Self::PasswordChange) {
            set_password(h, s.user.id, "old-password-1").await;
        }
    }

    async fn call(self, h: &Harness, s: &Subject) -> Reply {
        match self {
            Self::PasswordChange => {
                h.call(
                    Method::PUT,
                    "/api/v1/auth/me/password",
                    Some(&s.user.bearer),
                    &[],
                    Some(&json!({"current_password": "old-password-1", "new_password": "new-password-2"})),
                )
                .await
            }
            Self::AdminReset => {
                h.call(
                    Method::PUT,
                    &format!("/api/v1/admin/users/{}/password", s.user.id),
                    Some(&h.inst_admin.bearer),
                    &[],
                    Some(&json!({"new_password": "reset-password-3"})),
                )
                .await
            }
            Self::AdminDisable => {
                h.call(
                    Method::PATCH,
                    &format!("/api/v1/admin/users/{}/status", s.user.id),
                    Some(&h.inst_admin.bearer),
                    &[],
                    Some(&json!({"is_active": false})),
                )
                .await
            }
            Self::AdminDelete => {
                h.del(&format!("/api/v1/admin/users/{}", s.user.id), &h.inst_admin)
                    .await
            }
            Self::MemberRemoval => {
                h.del(
                    &format!("/api/v1/workspaces/{}/members/{}", h.tenant.workspace_id, s.user.id),
                    &h.ws_admin,
                )
                .await
            }
        }
    }

    /// The change the handler makes on its own, as a number that differs before and after.
    async fn marker(self, h: &Harness, s: &Subject) -> String {
        #[derive(FromQueryResult)]
        struct Row {
            v: String,
        }
        let real = match self {
            Self::MemberRemoval => Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*)::text AS v FROM workspace_members WHERE user_id = $1 AND workspace_id = $2",
                vec![s.user.id.into(), h.tenant.workspace_id.into()],
            ),
            Self::PasswordChange | Self::AdminReset => Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT password_hash || session_version::text AS v FROM users WHERE id = $1",
                vec![s.user.id.into()],
            ),
            Self::AdminDisable | Self::AdminDelete => Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT is_active::text || session_version::text AS v FROM users WHERE id = $1",
                vec![s.user.id.into()],
            ),
        };
        Row::find_by_statement(real).one(h.db()).await.unwrap().unwrap().v
    }
}

#[tokio::test]
async fn each_hook_revokes_in_the_transaction_of_its_own_handler() {
    for hook in ALL_HOOKS {
        let scratch = oauth_scratch_or_skip!("wp7_hooks", Start::Complete);
        let h = Harness::new(scratch).await;
        let s = subject(&h).await;
        let mut listener = h.listener().await;
        hook.prepare(&h, &s).await;
        let before = hook.marker(&h, &s).await;

        let reply = hook.call(&h, &s).await;
        reply.assert_ok();
        assert_ne!(
            hook.marker(&h, &s).await,
            before,
            "{hook:?}: the handler made its own change"
        );

        let actor = hook.actor(&h, &s);
        h.assert_revoked(s.here, hook.reason(), actor).await;
        let mut ended = vec![s.here];
        if hook.ends_every_grant() {
            h.assert_revoked(s.there, hook.reason(), actor).await;
            ended.push(s.there);
        } else {
            h.assert_active(s.there).await;
        }
        h.assert_active(s.control).await;
        assert_eq!(
            notifications(&mut listener, Duration::from_millis(500)).await,
            sorted_ids(&ended),
            "{hook:?}: one notification per revoked grant, after the commit"
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn re_enabling_an_account_revokes_nothing() {
    let scratch = oauth_scratch_or_skip!("wp7_reenable", Start::Complete);
    let h = Harness::new(scratch).await;
    let s = subject(&h).await;
    let path = format!("/api/v1/admin/users/{}/status", s.user.id);
    let enable = h
        .call(
            Method::PATCH,
            &path,
            Some(&h.inst_admin.bearer),
            &[],
            Some(&json!({"is_active": true})),
        )
        .await;
    enable.assert_ok();
    h.assert_active(s.here).await;
    h.assert_active(s.there).await;
    h.finish().await;
}

/// Makes any update of a grant fail, as a revocation that can not be written would.
async fn break_revocation(h: &Harness) {
    h.db()
        .execute_unprepared(
            "CREATE FUNCTION wp7_fail() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN RAISE EXCEPTION 'forced revocation failure'; END $$; \
             CREATE TRIGGER wp7_fail_grants BEFORE UPDATE ON oauth_grants FOR EACH ROW EXECUTE FUNCTION wp7_fail();",
        )
        .await
        .expect("trigger");
}

#[tokio::test]
async fn when_the_revocation_fails_the_whole_change_fails() {
    for hook in ALL_HOOKS {
        let scratch = oauth_scratch_or_skip!("wp7_atomic", Start::Complete);
        let h = Harness::new(scratch).await;
        let s = subject(&h).await;
        hook.prepare(&h, &s).await;
        let mut listener = h.listener().await;
        let before = hook.marker(&h, &s).await;
        break_revocation(&h).await;

        let reply = hook.call(&h, &s).await;
        reply.assert_failed();

        assert_eq!(
            hook.marker(&h, &s).await,
            before,
            "{hook:?}: the handler's own change was rolled back"
        );
        for grant in [s.here, s.there, s.control] {
            h.assert_active(grant).await;
        }
        assert_eq!(
            h.n("SELECT count(*) AS n FROM oauth_audit_events WHERE event = 'grant_revoked'")
                .await,
            0
        );
        assert!(
            notifications(&mut listener, Duration::from_millis(400))
                .await
                .is_empty(),
            "{hook:?}: no notification for a change that did not commit"
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn a_rolled_back_hook_leaves_the_grant_active_and_notifies_nobody() {
    let scratch = oauth_scratch_or_skip!("wp7_rollback", Start::Complete);
    let h = Harness::new(scratch).await;
    let s = subject(&h).await;
    let mut listener = h.listener().await;
    {
        use sea_orm::TransactionTrait;
        let tx = h.db().begin().await.unwrap();
        let ids = revoke_user_grants(&tx, s.user.id, RevokeReason::PasswordChanged, Actor::System)
            .await
            .unwrap();
        assert_eq!(ids.len(), 2);
        tx.rollback().await.unwrap();
    }
    h.assert_active(s.here).await;
    h.assert_active(s.there).await;
    assert!(
        notifications(&mut listener, Duration::from_millis(400))
            .await
            .is_empty()
    );

    // Committed, then repeated: the second call finds nothing and writes nothing.
    {
        use sea_orm::TransactionTrait;
        let tx = h.db().begin().await.unwrap();
        assert_eq!(
            revoke_user_grants(&tx, s.user.id, RevokeReason::PasswordChanged, Actor::System)
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(
            revoke_user_grants(&tx, s.user.id, RevokeReason::AccountDisabled, Actor::System)
                .await
                .unwrap()
                .is_empty()
        );
        tx.commit().await.unwrap();
    }
    h.assert_revoked(s.here, "password_changed", None).await;
    assert_eq!(
        notifications(&mut listener, Duration::from_millis(500)).await,
        sorted_ids(&[s.here, s.there])
    );
    h.finish().await;
}

mod races;

// ---------------------------------------------------------------------------------------------
// Races with the token endpoint
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redemption_racing_a_revocation_never_leaves_a_usable_token() {
    let scratch = oauth_scratch_or_skip!("wp7_race", Start::Complete);
    let h = Harness::new(scratch).await;
    let me = h.member();
    let mut issued_total = 0_u32;
    for round in 0..40 {
        let grant = seed_grant(h.db(), me.id, h.tenant.workspace_id, "all", &READ_SCOPES).await;
        let code = seed_code(h.db(), grant).await;
        let refresh = seed_refresh_token(h.db(), grant).await;
        exec(
            h.db(),
            "DELETE FROM oauth_rate_hits WHERE bucket IN ('token_ip', 'token_grant', 'revoke')",
            vec![],
        )
        .await;

        let redeem = form_request(
            "/api/v1/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", REDIRECT),
                ("client_id", "ramflux"),
                ("code_verifier", VERIFIER),
                ("resource", API_RES),
            ],
        );
        let rotate = form_request(
            "/api/v1/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", "ramflux"),
                ("resource", API_RES),
            ],
        );
        let disconnect = request(Method::DELETE, &apps_path(grant), Some(&me.bearer), &[], None);
        let tasks = [redeem, rotate, disconnect].map(|req| {
            let router = h.router.clone();
            tokio::spawn(async move {
                if round % 2 == 0 {
                    tokio::task::yield_now().await;
                }
                send_on(router, req).await
            })
        });
        let mut replies = Vec::new();
        for task in tasks {
            replies.push(task.await.expect("task"));
        }
        let (redeemed, rotated, disconnected) = (&replies[0], &replies[1], &replies[2]);
        assert_eq!(
            disconnected.status,
            StatusCode::OK,
            "round {round}: {}",
            disconnected.text()
        );

        h.assert_revoked(grant, "user_revoked", Some(me.id)).await;
        // Whatever the token endpoint answered before the revocation, none of it works now.
        for reply in [redeemed, rotated] {
            assert!(
                reply.status == StatusCode::OK || reply.status.is_client_error(),
                "round {round}: {}",
                reply.text()
            );
            if reply.status == StatusCode::OK {
                issued_total += 1;
                let body = reply.json();
                let access = body["access_token"].as_str().expect("access token");
                assert!(
                    validate_access_token(h.db(), access, AudienceRule::Api(ForwardCredential::absent()))
                        .await
                        .is_err(),
                    "round {round}: an access token issued around the revocation still validates"
                );
                if let Some(next) = body["refresh_token"].as_str() {
                    let again = h
                        .send(form_request(
                            "/api/v1/oauth/token",
                            &[
                                ("grant_type", "refresh_token"),
                                ("refresh_token", next),
                                ("client_id", "ramflux"),
                                ("resource", API_RES),
                            ],
                        ))
                        .await;
                    assert!(
                        again.status.is_client_error(),
                        "round {round}: a refresh token survived: {}",
                        again.text()
                    );
                }
            }
        }
        exec(
            h.db(),
            "DELETE FROM oauth_rate_hits WHERE bucket IN ('token_ip', 'token_grant', 'revoke')",
            vec![],
        )
        .await;
    }
    eprintln!("race rounds finished; {issued_total} token responses were issued before a revocation");
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_password_change_racing_a_refresh_commits_and_ends_the_grant() {
    let scratch = oauth_scratch_or_skip!("wp7_race_hook", Start::Complete);
    let h = Harness::new(scratch).await;
    for round in 0..15 {
        let user = h.user_in_both().await;
        set_password(&h, user.id, "old-password-1").await;
        let grant = seed_grant(h.db(), user.id, h.tenant.workspace_id, "all", &READ_SCOPES).await;
        let refresh = seed_refresh_token(h.db(), grant).await;
        exec(h.db(), "DELETE FROM oauth_rate_hits", vec![]).await;
        let change = request(
            Method::PUT,
            "/api/v1/auth/me/password",
            Some(&user.bearer),
            &[],
            Some(&json!({"current_password": "old-password-1", "new_password": "new-password-2"})),
        );
        let rotate = form_request(
            "/api/v1/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", "ramflux"),
                ("resource", API_RES),
            ],
        );
        let tasks = [change, rotate].map(|req| {
            let router = h.router.clone();
            tokio::spawn(async move { send_on(router, req).await })
        });
        let mut replies = Vec::new();
        for task in tasks {
            replies.push(task.await.expect("task"));
        }
        // The handler never fails for the race, and nothing is left live.
        replies[0].assert_ok();
        h.assert_revoked(grant, "password_changed", Some(user.id)).await;
        assert_eq!(h.audit("grant_revoked", grant).await.len(), 1, "round {round}");
    }
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Secrets stay out of logs and audit rows
// ---------------------------------------------------------------------------------------------

thread_local! {
    static CAPTURED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

struct Render(String);

impl tracing::field::Visit for Render {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

/// A subscriber installed for one test thread (the tests run on a current-thread runtime).
struct Capture;

impl tracing::Subscriber for Capture {
    /// Without this a callsite first reached on a thread with no subscriber stays switched off.
    fn register_callsite(&self, _: &'static tracing::Metadata<'static>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut render = Render(format!("span {} ", attrs.metadata().name()));
        attrs.record(&mut render);
        CAPTURED.with(|lines| lines.borrow_mut().push(render.0));
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut render = Render(format!("{} ", event.metadata().target()));
        event.record(&mut render);
        CAPTURED.with(|lines| lines.borrow_mut().push(render.0));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// Scoped subscribers only see a callsite that another thread reached first when some global
/// subscriber exists to keep its interest at "sometimes". This one records nothing.
struct Idle;

impl tracing::Subscriber for Idle {
    fn register_callsite(&self, _: &'static tracing::Metadata<'static>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        false
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, _: &tracing::Event<'_>) {}

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// Installs [`Idle`] unless another test module already installed a global subscriber, which
/// serves the same purpose.
fn install_idle_global_subscriber() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        // An error means a global default exists already, which is just as good.
        let _ = tracing::subscriber::set_global_default(Idle);
    });
}

#[tokio::test]
async fn no_log_line_audit_row_or_response_contains_a_token() {
    let scratch = oauth_scratch_or_skip!("wp7_secrets", Start::Complete);
    let h = Harness::new(scratch).await;
    install_idle_global_subscriber();
    let guard = tracing::subscriber::set_default(Capture);
    let me = h.member();
    let s = subject(&h).await;
    let mut secrets: Vec<String> = Vec::new();
    for grant in [s.here, s.there] {
        secrets.push(seed_access_token(h.db(), grant, "api", &["projects:read"]).await.1);
        secrets.push(seed_refresh_token(h.db(), grant).await);
        secrets.push(seed_code(h.db(), grant).await);
    }
    let mine = h.grant(me.id, h.tenant.workspace_id, "all").await;
    let (_, delegated) = seed_access_token(h.db(), mine, "api", &["projects:read"]).await;
    secrets.push(delegated.clone());

    let mut responses = String::new();
    for reply in [
        h.get("/api/v1/me/connected-apps", &me).await,
        h.call(Method::GET, "/api/v1/me/connected-apps", Some(&delegated), &[], None)
            .await,
        h.get(&ws_apps_path(h.tenant.workspace_id), &h.ws_admin).await,
        h.del(&ws_app_path(h.tenant.workspace_id, s.control), &h.ws_admin).await,
        h.del(&apps_path(mine), &me).await,
        h.call(
            Method::PUT,
            &settings_path(h.other.workspace_id),
            Some(&h.other_admin.bearer),
            &[],
            Some(&json!({"allow_connected_apps": false})),
        )
        .await,
        h.call(
            Method::PATCH,
            "/api/v1/admin/oauth-clients/ramflux",
            Some(&h.inst_admin.bearer),
            &[],
            Some(&json!({"status": "disabled"})),
        )
        .await,
        Hook::AdminDisable.call(&h, &s).await,
    ] {
        responses.push_str(&reply.text());
    }
    let logs = CAPTURED.with(|lines| lines.borrow().join("\n"));
    assert!(logs.contains("oauth grant revoked"), "the revocation was logged");
    assert!(logs.lines().count() > 20, "database activity was captured too");
    for secret in &secrets {
        assert!(secret.len() >= 20);
        assert!(!logs.contains(secret.as_str()), "a log line contains a token");
        assert!(!responses.contains(secret.as_str()), "a response contains a token");
        assert_eq!(
            count(
                h.db(),
                "SELECT count(*) AS n FROM oauth_audit_events t WHERE t::text LIKE '%' || $1 || '%'",
                vec![secret.clone().into()]
            )
            .await,
            0,
            "an audit row contains a token"
        );
    }
    assert!(!logs.contains(&h.member().bearer), "the session token was not logged");
    drop(guard);
    h.finish().await;
}
