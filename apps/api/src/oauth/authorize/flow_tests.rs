//! The authorization server endpoints through the real router against PostgreSQL.
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

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use platform::app::{AppState, FlowPermissionCacheSlot};
use platform::auth::JwtManager;
use platform::config::{AppConfig, OAuthConfig, Secret};
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::routes;
use crate::oauth::grants::GRANT_CHANGED_CHANNEL;
use crate::oauth::rate::{RateBucket, RateKey};
use crate::oauth::redirect::percent_encode;
use crate::oauth::scopes::{ProjectScope, Scope, TokenResource};
use crate::oauth::test_support::{Scratch, Start, count, oauth_scratch_or_skip, seed_access_token, seed_grant};
use crate::oauth::token::{parse_form, s256_challenge};
use crate::oauth::tokens::hash_secret;
use crate::oauth::validate::{AudienceRule, ForwardCredential, RejectReason, validate_access_token};
use crate::routes::context::tenant_fixture::{Tenant, exec, seed_tenant};

const ISSUER: &str = "http://127.0.0.1:18097";
const API_RES: &str = "http://127.0.0.1:18097/api";
const MCP_RES: &str = "http://127.0.0.1:8090/mcp/rpc";
const REDIRECT: &str = "http://127.0.0.1:53682/oauth/sylvode/callback";
const SCOPES: &str = "projects:read work_items:read notifications:read tasks:read offline_access";
const STATE: &str = "state-value-0123456789abcdef";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const FORWARD: &str = "forward-secret-for-tests-0123456789abcdef";
const JWT_SECRET: &str = "wp2-flow-test-jwt-secret";
const TOKEN: &str = "/api/v1/oauth/token";
const REVOKE: &str = "/api/v1/oauth/revoke";
const REQUESTS: &str = "/api/v1/oauth/authorization-requests";

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

struct Harness {
    /// Clear the fixed windows before every request, so a test that makes many calls is not
    /// limited by accident. The rate tests switch it off.
    reset_windows: std::sync::atomic::AtomicBool,
    scratch: Scratch,
    state: AppState,
    router: Router,
    tenant: Tenant,
}

struct Person {
    id: Uuid,
    bearer: String,
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("not json ({e}): {:?}", self.text()))
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The `data` of a success envelope.
    fn data(&self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.text());
        self.json()["data"].clone()
    }

    #[track_caller]
    fn assert_oauth_error(&self, status: u16, error: &str) {
        assert_eq!(self.status.as_u16(), status, "{}", self.text());
        assert_eq!(self.json()["error"], error, "{}", self.text());
    }

    #[track_caller]
    fn assert_flow_error(&self, status: u16, code: &str) {
        assert_eq!(self.status.as_u16(), status, "{}", self.text());
        assert_eq!(self.json()["error_code"], code, "{}", self.text());
    }
}

fn config_with(tweak: impl FnOnce(&mut OAuthConfig)) -> OAuthConfig {
    let mut config = OAuthConfig::with_defaults(ISSUER);
    config.mcp_resource = Some(MCP_RES.to_owned());
    config.internal_forward_secret = Some(Secret::new(FORWARD));
    tweak(&mut config);
    config
}

impl Harness {
    async fn new(scratch: Scratch) -> Self {
        Self::with_config(scratch, |_| {}).await
    }

    async fn with_config(scratch: Scratch, tweak: impl FnOnce(&mut OAuthConfig)) -> Self {
        let tenant = seed_tenant(&scratch.db, "wp2").await;
        let state = AppState {
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
            db: scratch.db.clone(),
            flow_permission_cache: FlowPermissionCacheSlot::default(),
        };
        let router = routes(Some(&Arc::new(config_with(tweak)))).with_state(state.clone());
        Self {
            reset_windows: std::sync::atomic::AtomicBool::new(true),
            scratch,
            state,
            router,
            tenant,
        }
    }

    fn db(&self) -> &DatabaseConnection {
        &self.scratch.db
    }

    async fn finish(self) {
        self.scratch.drop_self().await;
    }

    fn person(id: Uuid) -> Person {
        let jwt = JwtManager::new(JWT_SECRET, 900, 3600);
        let bearer = jwt
            .issue_access_token_at_session(&id.to_string(), &format!("{id}@tenant.test"), 0)
            .expect("jwt");
        Person { id, bearer }
    }

    fn member(&self) -> Person {
        Self::person(self.tenant.member_id)
    }

    /// A second user who is a member of `workspace_id`.
    async fn add_user(&self, workspace_id: Uuid) -> Person {
        let id = Uuid::new_v4();
        exec(
            self.db(),
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'second user', 'user', true)",
            vec![id.into(), format!("{id}@tenant.test").into()],
        )
        .await;
        exec(
            self.db(),
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![workspace_id.into(), id.into()],
        )
        .await;
        Self::person(id)
    }

    fn limits_on(&self) {
        self.reset_windows.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    async fn send(&self, req: Request<Body>) -> Reply {
        if self.reset_windows.load(std::sync::atomic::Ordering::SeqCst) {
            exec(
                self.db(),
                "DELETE FROM oauth_rate_hits WHERE bucket IN ('authorize', 'decision', 'token_ip', 'token_grant', 'revoke')",
                vec![],
            )
            .await;
        }
        let path = req.uri().path().to_owned();
        let response = self.router.clone().oneshot(req).await.expect("router answers");
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body")
            .to_vec();
        if path.starts_with("/api/v1/oauth/") && status != StatusCode::METHOD_NOT_ALLOWED {
            assert_eq!(headers[header::CACHE_CONTROL], "no-store", "{path} must not be cached");
        }
        Reply { status, headers, body }
    }

    async fn token(&self, pairs: &[(&str, &str)]) -> Reply {
        self.send(form(TOKEN, pairs)).await
    }

    async fn revoke(&self, pairs: &[(&str, &str)]) -> Reply {
        self.send(form(REVOKE, pairs)).await
    }

    async fn begin(&self, person: Option<&Person>, body: &Value) -> Reply {
        self.send(json_post(REQUESTS, person, &[], body)).await
    }

    async fn decide(
        &self,
        person: Option<&Person>,
        request_id: &str,
        consent: Option<&str>,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Reply {
        let mut all: Vec<(&str, &str)> = headers.to_vec();
        if let Some(consent) = consent {
            all.push(("x-sylvode-consent", consent));
        }
        self.send(json_post(
            &format!("{REQUESTS}/{request_id}/decision"),
            person,
            &all,
            body,
        ))
        .await
    }

    /// Rows of a table or a condition.
    async fn n(&self, sql: &str) -> i64 {
        count(self.db(), sql, vec![]).await
    }
}

fn form(path: &str, pairs: &[(&str, &str)]) -> Request<Body> {
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

fn json_post(path: &str, person: Option<&Person>, headers: &[(&str, &str)], body: &Value) -> Request<Body> {
    let mut builder = Request::post(path).header(header::CONTENT_TYPE, "application/json");
    if let Some(person) = person {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", person.bearer));
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

fn authorize_body() -> Value {
    json!({
        "response_type": "code",
        "client_id": "ramflux",
        "redirect_uri": REDIRECT,
        "scope": SCOPES,
        "state": STATE,
        "code_challenge": s256_challenge(VERIFIER),
        "code_challenge_method": "S256",
        "resource": [API_RES, MCP_RES],
        "device_name": "Test laptop",
    })
}

fn with(mut body: Value, key: &str, value: Value) -> Value {
    body[key] = value;
    body
}

fn query_of(url: &str) -> HashMap<String, String> {
    let (_, query) = url.split_once('?').expect("a query");
    parse_form(query.as_bytes()).expect("form").into_iter().collect()
}

struct Consent {
    request_id: String,
    consent_token: String,
}

impl Harness {
    /// A stored request shown to `person`: the request id and the consent token.
    async fn consent_for(&self, person: &Person, body: &Value) -> Consent {
        let data = self.begin(Some(person), body).await.data();
        assert_eq!(data["kind"], "consent", "{data}");
        Consent {
            request_id: data["request_id"].as_str().expect("id").to_owned(),
            consent_token: data["consent_token"].as_str().expect("token").to_owned(),
        }
    }

    fn allow_body(&self, projects: &[Uuid]) -> Value {
        let mut body = json!({
            "decision": "allow",
            "workspace_id": self.tenant.workspace_id,
            "project_mode": "all",
            "scopes": SCOPES.split(' ').collect::<Vec<_>>(),
        });
        if !projects.is_empty() {
            body["project_mode"] = json!("selected");
            body["project_ids"] = json!(projects);
        }
        body
    }

    /// Runs request and decision, returns the authorization code.
    async fn code_for(&self, person: &Person, body: &Value, allow: &Value) -> String {
        let consent = self.consent_for(person, body).await;
        let reply = self
            .decide(
                Some(person),
                &consent.request_id,
                Some(&consent.consent_token),
                &[],
                allow,
            )
            .await;
        let data = reply.data();
        let target = data["redirect_to"].as_str().expect("redirect_to");
        let query = query_of(target);
        assert!(target.starts_with(REDIRECT), "{target}");
        assert_eq!(query["state"], STATE);
        assert_eq!(query["iss"], ISSUER);
        query["code"].clone()
    }

    async fn redeem(&self, code: &str, resource: &str) -> Reply {
        self.token(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT),
            ("client_id", "ramflux"),
            ("code_verifier", VERIFIER),
            ("resource", resource),
        ])
        .await
    }

    async fn refresh(&self, refresh_token: &str, resource: &str) -> Reply {
        self.token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", "ramflux"),
            ("resource", resource),
        ])
        .await
    }

    /// A complete connection for `person`: tokens for the MCP resource from the code.
    async fn connect(&self, person: &Person, allow: &Value) -> Connected {
        let code = self.code_for(person, &authorize_body(), allow).await;
        let reply = self.redeem(&code, MCP_RES).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        let body = reply.json();
        Connected {
            code,
            access: body["access_token"].as_str().expect("access").to_owned(),
            refresh: body["refresh_token"].as_str().expect("refresh").to_owned(),
            grant_id: Uuid::parse_str(body["grant_id"].as_str().expect("grant")).expect("uuid"),
        }
    }

    async fn grant_status(&self, grant: Uuid) -> (String, Option<String>) {
        #[derive(FromQueryResult)]
        struct Row {
            status: String,
            revoke_reason: Option<String>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, revoke_reason FROM oauth_grants WHERE id = $1",
            vec![grant.into()],
        ))
        .one(self.db())
        .await
        .expect("query")
        .expect("grant exists");
        (row.status, row.revoke_reason)
    }

    async fn audit_count(&self, event: &str) -> i64 {
        count(
            self.db(),
            "SELECT count(*) AS n FROM oauth_audit_events WHERE event = $1",
            vec![event.into()],
        )
        .await
    }

    async fn audit_detail(&self, event: &str, key: &str) -> Vec<String> {
        #[derive(FromQueryResult)]
        struct Row {
            v: Option<String>,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT detail ->> $2 AS v FROM oauth_audit_events WHERE event = $1 ORDER BY occurred_at, id",
            vec![event.into(), key.into()],
        ))
        .all(self.db())
        .await
        .expect("query")
        .into_iter()
        .filter_map(|row| row.v)
        .collect()
    }

    /// Counts that must not change when a request is refused.
    async fn side_effects(&self) -> [i64; 6] {
        [
            self.n("SELECT count(*) AS n FROM users").await,
            self.n("SELECT count(*) AS n FROM workspace_members").await,
            self.n("SELECT count(*) AS n FROM oauth_grants").await,
            self.n("SELECT count(*) AS n FROM oauth_tokens").await,
            self.n("SELECT count(*) AS n FROM oauth_authorization_codes").await,
            self.n("SELECT count(*) AS n FROM oauth_grant_projects").await,
        ]
    }

    async fn seconds_left(&self, table: &str, hash: &str) -> f64 {
        #[derive(FromQueryResult)]
        struct Row {
            v: f64,
        }
        let sql = match table {
            "tokens" => {
                "SELECT extract(epoch FROM (expires_at - now()))::float8 AS v FROM oauth_tokens WHERE token_hash = $1"
            }
            _ => {
                "SELECT extract(epoch FROM (expires_at - now()))::float8 AS v FROM oauth_authorization_codes WHERE code_hash = $1"
            }
        };
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![hash.into()],
        ))
        .one(self.db())
        .await
        .expect("query")
        .expect("row")
        .v
    }
}

struct Connected {
    code: String,
    access: String,
    refresh: String,
    grant_id: Uuid,
}

fn forwarded() -> (Secret, &'static str) {
    (Secret::new(FORWARD), "mcp-server")
}

// ---------------------------------------------------------------------------------------------
// The whole flow
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn full_flow_request_decision_redemption_refresh_and_revocation() {
    let scratch = oauth_scratch_or_skip!("wp2_happy", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();

    // Anonymous: the request is stored and the page is told to sign in.
    let login = h.begin(None, &authorize_body()).await.data();
    assert_eq!(login["kind"], "login_required");
    assert_eq!(login["client"]["client_id"], "ramflux");
    assert_eq!(login["client"]["redirect_host"], "127.0.0.1");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_authorization_requests").await, 1);
    // After sign-in the page reloads the stored request by id.
    let reloaded = h
        .begin(Some(&person), &json!({"request_id": login["request_id"]}))
        .await
        .data();
    assert_eq!(reloaded["kind"], "consent");
    assert_eq!(reloaded["request_id"], login["request_id"]);
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_authorization_requests").await, 1);
    assert_eq!(reloaded["client"]["display_name"], "Ramflux");
    assert_eq!(reloaded["client"]["device_name"], "Test laptop");
    assert_eq!(reloaded["account"]["user_id"], person.id.to_string());
    assert_eq!(reloaded["workspaces"][0]["id"], h.tenant.workspace_id.to_string());
    assert_eq!(reloaded["workspaces"][0]["eligible"], true);
    assert_eq!(reloaded["preselected_workspace_id"], h.tenant.workspace_id.to_string());
    assert_eq!(reloaded["resources"], json!(["api", "mcp"]));
    assert_eq!(reloaded["active_grant_count"], 0);
    assert_eq!(reloaded["max_active_grants"], 10);
    let scopes: HashMap<String, bool> = reloaded["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .map(|s| (s["name"].as_str().unwrap().to_owned(), s["required"].as_bool().unwrap()))
        .collect();
    assert_eq!(scopes.len(), 5);
    assert!(
        scopes.values().all(|required| *required),
        "all five requested scopes are required ones"
    );
    let consent_token = reloaded["consent_token"].as_str().expect("token").to_owned();
    assert_eq!(consent_token.len(), 64);

    // Decision: allow for the selected project.
    let request_id = login["request_id"].as_str().unwrap();
    let allow = h.allow_body(&[h.tenant.project_id]);
    let decided = h
        .decide(Some(&person), request_id, Some(&consent_token), &[], &allow)
        .await;
    let redirect = decided.data()["redirect_to"].as_str().expect("redirect").to_owned();
    assert!(redirect.starts_with(&format!("{REDIRECT}?")), "{redirect}");
    let query = query_of(&redirect);
    assert_eq!(query["state"], STATE, "state is returned untouched");
    assert_eq!(query["iss"], ISSUER);
    let code = query["code"].clone();
    assert!(code.starts_with("svc_") && code.len() == 68);
    assert!(!query.contains_key("error"));

    // Stored: the grant binds user, client, workspace, scopes, projects; the code only as a hash.
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'active' AND project_mode = 'selected'")
            .await,
        1
    );
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_grant_projects").await, 1);
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_authorization_codes").await, 1);
    assert_eq!(
        count(
            h.db(),
            "SELECT count(*) AS n FROM oauth_authorization_codes WHERE code_hash = $1 AND redirect_uri = $2",
            vec![hash_secret(&code).as_hex().into(), REDIRECT.into()]
        )
        .await,
        1
    );
    let code_ttl = h.seconds_left("codes", hash_secret(&code).as_hex()).await;
    assert!(
        (50.0..=60.5).contains(&code_ttl),
        "the code lives 60 seconds, has {code_ttl}"
    );
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_authorization_requests WHERE status = 'decided' AND consent_token_hash IS NULL").await, 1);
    for event in ["authorize_requested", "consent_granted", "code_issued"] {
        assert_eq!(h.audit_count(event).await, 1, "{event}");
    }

    // Redemption for the MCP resource.
    let reply = h.redeem(&code, MCP_RES).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(reply.headers[header::PRAGMA], "no-cache");
    let issued = reply.json();
    let access = issued["access_token"].as_str().expect("access").to_owned();
    let refresh = issued["refresh_token"].as_str().expect("refresh").to_owned();
    assert!(access.starts_with("sva_") && access.len() == 68);
    assert!(refresh.starts_with("svr_") && refresh.len() == 68);
    assert_eq!(issued["token_type"], "Bearer");
    assert!((590..=600).contains(&issued["expires_in"].as_i64().unwrap()));
    assert_eq!(
        issued["scope"], "projects:read work_items:read",
        "MCP supports only these of the grant"
    );
    assert_eq!(issued["workspace_id"], h.tenant.workspace_id.to_string());
    let grant_id = Uuid::parse_str(issued["grant_id"].as_str().unwrap()).expect("uuid");

    // The access token is usable where the contract says: MCP audience, with the forwarding credential.
    let (secret, by) = forwarded();
    let forward = ForwardCredential::new(Some(&secret), Some(by), Some(FORWARD));
    let validated = validate_access_token(h.db(), &access, AudienceRule::McpForwarded(forward))
        .await
        .expect("valid for the MCP server");
    assert_eq!(validated.grant_id, grant_id);
    assert_eq!(validated.user_id, person.id);
    assert_eq!(validated.resource, TokenResource::Mcp);
    assert!(validated.scopes.contains(Scope::ProjectsRead) && !validated.scopes.contains(Scope::OfflineAccess));
    assert!(validated.projects.contains(h.tenant.project_id));
    assert_eq!(
        validate_access_token(h.db(), &access, AudienceRule::Api(ForwardCredential::absent()))
            .await
            .expect_err("not for the API")
            .reason,
        RejectReason::AudienceMismatch
    );
    assert_eq!(h.audit_count("code_redeemed").await, 1);

    // Refresh for the API resource: new access token, rotated refresh token.
    let reply = h.refresh(&refresh, API_RES).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let second = reply.json();
    let api_access = second["access_token"].as_str().unwrap().to_owned();
    let new_refresh = second["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(new_refresh, refresh);
    assert_eq!(second["grant_id"], issued["grant_id"]);
    assert_eq!(
        second["scope"], "projects:read work_items:read notifications:read tasks:read",
        "the API supports everything but offline_access"
    );
    let api = validate_access_token(h.db(), &api_access, AudienceRule::Api(ForwardCredential::absent()))
        .await
        .expect("valid for the API");
    assert_eq!(api.resource, TokenResource::Api);
    assert_eq!(api.projects, ProjectScope::Selected(Arc::from([h.tenant.project_id])));
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE kind = 'refresh' AND consumed_at IS NOT NULL")
            .await,
        1,
        "the old refresh token is consumed"
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens c JOIN oauth_tokens p ON p.id = c.parent_id WHERE c.kind = 'refresh'").await,
        1,
        "the new one names its parent"
    );
    assert_eq!(h.audit_count("token_refreshed").await, 1);

    // Revocation with the refresh token ends the grant and every token of it.
    let reply = h
        .revoke(&[
            ("token", &new_refresh),
            ("token_type_hint", "refresh_token"),
            ("client_id", "ramflux"),
        ])
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.body.is_empty());
    assert_eq!(
        h.grant_status(grant_id).await,
        ("revoked".to_owned(), Some("client_revoked".to_owned()))
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE revoked_at IS NULL")
            .await,
        0
    );
    assert_eq!(
        validate_access_token(h.db(), &api_access, AudienceRule::Api(ForwardCredential::absent()))
            .await
            .expect_err("revoked")
            .reason,
        RejectReason::TokenRevoked
    );
    h.refresh(&new_refresh, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(h.audit_count("grant_revoked").await, 1);
    assert_eq!(h.audit_count("token_revoked").await, 1);
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Authorization request
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn authorization_request_errors_and_what_they_create() {
    let scratch = oauth_scratch_or_skip!("wp2_authreq", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let before = h.side_effects().await;

    // Never redirected: unknown client, disabled client, unregistered redirect URI.
    let data = h
        .begin(None, &with(authorize_body(), "client_id", json!("nobody")))
        .await
        .data();
    assert_eq!(data, json!({"kind": "error_page", "error": "unknown_client"}));
    let data = h
        .begin(
            None,
            &with(
                authorize_body(),
                "redirect_uri",
                json!("http://localhost:53682/oauth/sylvode/callback"),
            ),
        )
        .await
        .data();
    assert_eq!(data, json!({"kind": "error_page", "error": "redirect_uri_mismatch"}));
    let data = h
        .begin(
            None,
            &with(authorize_body(), "redirect_uri", json!("https://evil.example/cb")),
        )
        .await
        .data();
    assert_eq!(data["kind"], "error_page");
    let data = h
        .begin(
            None,
            &with(
                authorize_body(),
                "redirect_uri",
                json!("http://127.0.0.1:80/oauth/sylvode/callback"),
            ),
        )
        .await
        .data();
    assert_eq!(data["error"], "redirect_uri_mismatch");
    let data = h.begin(None, &json!({})).await.data();
    assert_eq!(data["error"], "unknown_client");
    exec(
        h.db(),
        "UPDATE oauth_clients SET status = 'disabled' WHERE client_id = 'ramflux'",
        vec![],
    )
    .await;
    let data = h.begin(None, &authorize_body()).await.data();
    assert_eq!(data["error"], "unknown_client");
    exec(
        h.db(),
        "UPDATE oauth_clients SET status = 'active' WHERE client_id = 'ramflux'",
        vec![],
    )
    .await;

    // Redirected with a standard error, state and iss.
    let cases = [
        (
            with(authorize_body(), "response_type", json!("token")),
            "unsupported_response_type",
        ),
        (
            with(authorize_body(), "code_challenge_method", json!("plain")),
            "invalid_request",
        ),
        (
            with(authorize_body(), "code_challenge", json!("short")),
            "invalid_request",
        ),
        (
            with(
                authorize_body(),
                "resource",
                json!(["http://127.0.0.1:18097/elsewhere"]),
            ),
            "invalid_target",
        ),
        (
            with(authorize_body(), "scope", json!("projects:read tasks:dispatch")),
            "invalid_scope",
        ),
        (
            with(authorize_body(), "scope", json!("projects:read forms:write")),
            "invalid_scope",
        ),
    ];
    for (body, error) in cases {
        let data = h.begin(Some(&person), &body).await.data();
        assert_eq!(data["kind"], "redirect", "{data}");
        let target = data["redirect_to"].as_str().unwrap();
        assert!(target.starts_with(REDIRECT), "{target}");
        let query = query_of(target);
        assert_eq!(query["error"], error);
        assert_eq!(query["state"], STATE);
        assert_eq!(query["iss"], ISSUER);
        assert!(!query.contains_key("code"));
    }
    // A missing state is reported without one.
    let mut no_state = authorize_body();
    no_state.as_object_mut().unwrap().remove("state");
    let data = h.begin(Some(&person), &no_state).await.data();
    let query = query_of(data["redirect_to"].as_str().unwrap());
    assert_eq!(query["error"], "invalid_request");
    assert!(!query.contains_key("state"));
    assert_eq!(query["iss"], ISSUER);

    assert_eq!(h.side_effects().await, before, "nothing but audit rows was created");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_authorization_requests").await, 0);
    assert_eq!(h.audit_count("authorize_requested").await, 0);
    assert_eq!(h.audit_count("authorize_rejected").await, 13);
    let checks = h.audit_detail("authorize_rejected", "check").await;
    assert!(checks.contains(&"redirect_uri".to_owned()) && checks.contains(&"scope".to_owned()));

    // Transport errors.
    let reply = h
        .send(
            Request::post(REQUESTS)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;
    reply.assert_flow_error(400, "invalid_request");
    let reply = h
        .send(
            Request::post(REQUESTS)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await;
    reply.assert_flow_error(400, "invalid_request");
    let reply = h
        .send(
            Request::post(format!("{REQUESTS}?access_token=x"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    reply.assert_flow_error(400, "invalid_request");
    // Bot and delegated tokens are not people.
    let bot = Person {
        id: Uuid::new_v4(),
        bearer: format!("opr_{}", "a".repeat(64)),
    };
    h.begin(Some(&bot), &authorize_body())
        .await
        .assert_flow_error(403, "forbidden");
    let delegated = Person {
        id: Uuid::new_v4(),
        bearer: format!("sva_{}", "a".repeat(64)),
    };
    h.begin(Some(&delegated), &authorize_body())
        .await
        .assert_flow_error(403, "forbidden");
    // An expired or invalid session is treated as signed out.
    let stale = Person {
        id: Uuid::new_v4(),
        bearer: "not.a.jwt".to_owned(),
    };
    assert_eq!(
        h.begin(Some(&stale), &authorize_body()).await.data()["kind"],
        "login_required"
    );
    // A reload of an unknown or expired request shows the expiry page.
    let data = h
        .begin(Some(&person), &json!({"request_id": Uuid::new_v4()}))
        .await
        .data();
    assert_eq!(data, json!({"kind": "error_page", "error": "request_expired"}));
    let data = h.begin(Some(&person), &json!({"request_id": "garbage"})).await.data();
    assert_eq!(data["error"], "request_expired");
    let consent = h.consent_for(&person, &authorize_body()).await;
    exec(
        h.db(),
        "UPDATE oauth_authorization_requests SET expires_at = now() - interval '1 second'",
        vec![],
    )
    .await;
    let data = h
        .begin(Some(&person), &json!({"request_id": consent.request_id}))
        .await
        .data();
    assert_eq!(data["error"], "request_expired");
    h.finish().await;
}

#[tokio::test]
async fn a_user_who_is_not_eligible_still_gets_the_consent_data_with_the_reason() {
    let scratch = oauth_scratch_or_skip!("wp2_ineligible", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    exec(
        h.db(),
        "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) VALUES ($1, false) \
         ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = false",
        vec![h.tenant.workspace_id.into()],
    )
    .await;
    let data = h.begin(Some(&person), &authorize_body()).await.data();
    assert_eq!(data["kind"], "consent");
    assert_eq!(data["workspaces"][0]["eligible"], false);
    assert_eq!(data["workspaces"][0]["reason"], "workspace_disallows");
    assert_eq!(data["preselected_workspace_id"], Value::Null);
    // "Return to Ramflux" is the deny decision; with nothing eligible it says why.
    let reply = h
        .decide(
            Some(&person),
            data["request_id"].as_str().unwrap(),
            data["consent_token"].as_str(),
            &[],
            &json!({"decision": "deny"}),
        )
        .await;
    let query = query_of(reply.data()["redirect_to"].as_str().unwrap());
    assert_eq!(
        (query["error"].as_str(), query["error_description"].as_str()),
        ("access_denied", "no_eligible_workspace")
    );
    assert_eq!(query["state"], STATE);
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_decision_is_protected_and_bound_to_its_request() {
    let scratch = oauth_scratch_or_skip!("wp2_decision", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let other = h.add_user(h.tenant.workspace_id).await;
    let allow = h.allow_body(&[]);
    let consent = h.consent_for(&person, &authorize_body()).await;
    let id = consent.request_id.as_str();
    let token = consent.consent_token.as_str();
    let before = h.side_effects().await;

    // No session.
    h.decide(None, id, Some(token), &[], &allow)
        .await
        .assert_flow_error(401, "session_required");
    // No consent token, a wrong one, a malformed one, another user's session with the right one.
    h.decide(Some(&person), id, None, &[], &allow)
        .await
        .assert_flow_error(403, "consent_token_invalid");
    h.decide(Some(&person), id, Some(&"0".repeat(64)), &[], &allow)
        .await
        .assert_flow_error(403, "consent_token_invalid");
    h.decide(Some(&person), id, Some("short"), &[], &allow)
        .await
        .assert_flow_error(403, "consent_token_invalid");
    h.decide(Some(&other), id, Some(token), &[], &allow)
        .await
        .assert_flow_error(403, "consent_token_invalid");
    // Cross-site and foreign origins, even with everything else right.
    for headers in [
        vec![("origin", "https://evil.example")],
        vec![("origin", "null")],
        vec![("sec-fetch-site", "cross-site")],
        vec![("sec-fetch-site", "same-site")],
        vec![("origin", ISSUER), ("sec-fetch-site", "cross-site")],
    ] {
        h.decide(Some(&person), id, Some(token), &headers, &allow)
            .await
            .assert_flow_error(403, "csrf_origin_mismatch");
    }
    // Wrong content type and unknown fields.
    let reply = h
        .send(
            Request::post(format!("{REQUESTS}/{id}/decision"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::AUTHORIZATION, format!("Bearer {}", person.bearer))
                .header("x-sylvode-consent", token)
                .body(Body::from("decision=allow"))
                .unwrap(),
        )
        .await;
    reply.assert_flow_error(400, "invalid_request");
    h.decide(
        Some(&person),
        id,
        Some(token),
        &[],
        &json!({"decision": "allow", "client_id": "other"}),
    )
    .await
    .assert_flow_error(400, "invalid_request");
    h.decide(Some(&person), id, Some(token), &[], &json!({"decision": "maybe"}))
        .await
        .assert_flow_error(400, "invalid_request");
    // Unknown request, garbage id.
    h.decide(Some(&person), &Uuid::new_v4().to_string(), Some(token), &[], &allow)
        .await
        .assert_flow_error(404, "request_not_found");
    h.decide(Some(&person), "garbage", Some(token), &[], &allow)
        .await
        .assert_flow_error(404, "request_not_found");
    // A signed-in user cannot claim to have no account.
    h.decide(Some(&person), id, None, &[], &json!({"decision": "no_account"}))
        .await
        .assert_flow_error(400, "invalid_request");

    assert_eq!(
        h.side_effects().await,
        before,
        "every refusal left the database untouched"
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_authorization_requests WHERE status = 'pending'")
            .await,
        1,
        "and the request is still open"
    );

    // The right token on the right session still works, once.
    let reply = h
        .decide(
            Some(&person),
            id,
            Some(token),
            &[("origin", ISSUER), ("sec-fetch-site", "same-origin")],
            &allow,
        )
        .await;
    assert!(reply.data()["redirect_to"].as_str().unwrap().contains("code=svc_"));
    h.decide(Some(&person), id, Some(token), &[], &allow)
        .await
        .assert_flow_error(409, "already_decided");
    h.decide(None, id, None, &[], &json!({"decision": "no_account"}))
        .await
        .assert_flow_error(409, "already_decided");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_grants").await, 1);

    // The decision completes the stored request: client, redirect URI, PKCE challenge and
    // resources come from it, not from the body.
    assert_eq!(
        h.n(&format!(
            "SELECT count(*) AS n FROM oauth_authorization_codes c JOIN oauth_grants g ON g.id = c.grant_id \
             WHERE c.code_challenge = '{}' AND c.redirect_uri = '{REDIRECT}' AND c.client_id = 'ramflux' \
             AND g.user_id = '{}' AND g.workspace_id = '{}' AND c.resources = ARRAY['api','mcp']",
            s256_challenge(VERIFIER),
            person.id,
            h.tenant.workspace_id
        ))
        .await,
        1
    );

    // A request past its lifetime cannot be decided.
    let late = h.consent_for(&person, &authorize_body()).await;
    exec(
        h.db(),
        "UPDATE oauth_authorization_requests SET expires_at = now() - interval '1 second' WHERE status = 'pending'",
        vec![],
    )
    .await;
    h.decide(Some(&person), &late.request_id, Some(&late.consent_token), &[], &allow)
        .await
        .assert_flow_error(404, "request_not_found");
    // A consent token is replaced when the page reloads: the old one stops working.
    let again = h.consent_for(&person, &authorize_body()).await;
    let reloaded = h
        .begin(Some(&person), &json!({"request_id": again.request_id}))
        .await
        .data();
    assert_ne!(reloaded["consent_token"], again.consent_token);
    h.decide(
        Some(&person),
        &again.request_id,
        Some(&again.consent_token),
        &[],
        &allow,
    )
    .await
    .assert_flow_error(403, "consent_token_invalid");
    h.decide(
        Some(&person),
        &again.request_id,
        reloaded["consent_token"].as_str(),
        &[],
        &allow,
    )
    .await
    .data();
    h.finish().await;
}

#[tokio::test]
async fn deny_and_no_account_create_nothing_and_redirect_with_the_standard_errors() {
    let scratch = oauth_scratch_or_skip!("wp2_deny", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let before = h.side_effects().await;

    let consent = h.consent_for(&person, &authorize_body()).await;
    let reply = h
        .decide(
            Some(&person),
            &consent.request_id,
            Some(&consent.consent_token),
            &[],
            &json!({"decision": "deny"}),
        )
        .await;
    let query = query_of(reply.data()["redirect_to"].as_str().unwrap());
    assert_eq!(
        (query["error"].as_str(), query["error_description"].as_str()),
        ("access_denied", "consent_denied")
    );
    assert_eq!(query["state"], STATE);
    assert_eq!(query["iss"], ISSUER);
    assert!(!query.contains_key("code"));
    assert_eq!(h.audit_count("consent_denied").await, 1);
    assert_eq!(h.side_effects().await, before);

    // No account: no session, no consent token needed, nothing created.
    let login = h.begin(None, &authorize_body()).await.data();
    let reply = h
        .decide(
            None,
            login["request_id"].as_str().unwrap(),
            None,
            &[],
            &json!({"decision": "no_account"}),
        )
        .await;
    let query = query_of(reply.data()["redirect_to"].as_str().unwrap());
    assert_eq!(
        (query["error"].as_str(), query["error_description"].as_str()),
        ("access_denied", "account_required")
    );
    assert_eq!(query["state"], STATE);
    assert_eq!(query["iss"], ISSUER);
    assert_eq!(
        h.side_effects().await,
        before,
        "no user, grant, code or token was created"
    );
    assert_eq!(h.audit_count("authorize_no_account").await, 1);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_authorization_requests WHERE status = 'decided'")
            .await,
        2
    );
    // It cannot be decided again.
    h.decide(
        None,
        login["request_id"].as_str().unwrap(),
        None,
        &[],
        &json!({"decision": "no_account"}),
    )
    .await
    .assert_flow_error(409, "already_decided");
    // Origin rules apply to it as well.
    let again = h.begin(None, &authorize_body()).await.data();
    h.decide(
        None,
        again["request_id"].as_str().unwrap(),
        None,
        &[("origin", "https://evil.example")],
        &json!({"decision": "no_account"}),
    )
    .await
    .assert_flow_error(403, "csrf_origin_mismatch");
    h.finish().await;
}

#[tokio::test]
async fn invalid_selections_are_refused_and_create_nothing() {
    let scratch = oauth_scratch_or_skip!("wp2_selection", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let foreign = seed_tenant(h.db(), "other").await;
    let consent = h.consent_for(&person, &authorize_body()).await;
    let before = h.side_effects().await;
    let ws = h.tenant.workspace_id;
    let all_scopes: Vec<&str> = SCOPES.split(' ').collect();
    let extra_scopes: Vec<&str> = SCOPES.split(' ').chain(["forms:read"]).collect();
    let decide = |body: Value| {
        let (h, person, consent) = (&h, &person, &consent);
        async move {
            h.decide(
                Some(person),
                &consent.request_id,
                Some(&consent.consent_token),
                &[],
                &body,
            )
            .await
        }
    };
    let allow = |extra: Value| {
        let mut body = json!({"decision": "allow", "workspace_id": ws, "project_mode": "all", "scopes": all_scopes});
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        body
    };
    for (label, body) in [
        ("no workspace", allow(json!({"workspace_id": null}))),
        (
            "selected without projects",
            allow(json!({"project_mode": "selected", "project_ids": []})),
        ),
        ("no mode", allow(json!({"project_mode": null}))),
        (
            "a project of another workspace",
            allow(json!({"project_mode": "selected", "project_ids": [foreign.project_id]})),
        ),
        (
            "a project that does not exist",
            allow(json!({"project_mode": "selected", "project_ids": [Uuid::new_v4()]})),
        ),
        ("a required scope unticked", allow(json!({"scopes": ["projects:read"]}))),
        ("a scope that was not requested", allow(json!({"scopes": extra_scopes}))),
        ("an unknown scope", allow(json!({"scopes": ["projects:read", "bogus"]}))),
    ] {
        decide(body).await.assert_flow_error(400, "invalid_selection");
        assert_eq!(h.side_effects().await, before, "{label}");
    }
    // A workspace the user is not in, and one that is switched off.
    decide(allow(json!({"workspace_id": foreign.workspace_id})))
        .await
        .assert_flow_error(403, "workspace_not_eligible");
    decide(allow(json!({"workspace_id": Uuid::new_v4()})))
        .await
        .assert_flow_error(403, "workspace_not_eligible");
    exec(
        h.db(),
        "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) VALUES ($1, false) \
         ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = false",
        vec![ws.into()],
    )
    .await;
    decide(allow(json!({})))
        .await
        .assert_flow_error(403, "workspace_not_eligible");
    assert_eq!(h.side_effects().await, before);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_authorization_requests WHERE status = 'pending'")
            .await,
        1
    );
    // After switching the workspace back on, the same request can still be decided.
    exec(h.db(), "DELETE FROM workspace_delegation_settings", vec![]).await;
    decide(allow(json!({}))).await.data();
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_grants").await, 1);
    h.finish().await;
}

#[tokio::test]
async fn a_user_in_several_workspaces_chooses_one() {
    let scratch = oauth_scratch_or_skip!("wp2_workspaces", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let second = seed_tenant(h.db(), "second").await;
    exec(
        h.db(),
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        vec![second.workspace_id.into(), person.id.into()],
    )
    .await;

    let data = h.begin(Some(&person), &authorize_body()).await.data();
    assert_eq!(data["workspaces"].as_array().unwrap().len(), 2);
    assert_eq!(
        data["preselected_workspace_id"],
        Value::Null,
        "two to choose from, no hint"
    );
    let hinted = h
        .begin(
            Some(&person),
            &with(authorize_body(), "workspace_hint", json!(second.workspace_id)),
        )
        .await
        .data();
    assert_eq!(hinted["preselected_workspace_id"], second.workspace_id.to_string());
    let stranger = h
        .begin(
            Some(&person),
            &with(authorize_body(), "workspace_hint", json!(Uuid::new_v4())),
        )
        .await
        .data();
    assert_eq!(
        stranger["preselected_workspace_id"],
        Value::Null,
        "a hint only preselects what is eligible"
    );

    let allow = json!({
        "decision": "allow", "workspace_id": second.workspace_id, "project_mode": "selected",
        "project_ids": [second.project_id], "scopes": SCOPES.split(' ').collect::<Vec<_>>(),
    });
    let code = h.code_for(&person, &authorize_body(), &allow).await;
    let reply = h.redeem(&code, API_RES).await;
    assert_eq!(reply.json()["workspace_id"], second.workspace_id.to_string());
    assert_eq!(
        h.n(&format!(
            "SELECT count(*) AS n FROM oauth_grants WHERE workspace_id = '{}'",
            second.workspace_id
        ))
        .await,
        1
    );
    assert_eq!(
        h.n(&format!(
            "SELECT count(*) AS n FROM oauth_grants WHERE workspace_id = '{}'",
            h.tenant.workspace_id
        ))
        .await,
        0
    );
    // A project of the first workspace cannot be named for the second.
    let wrong = json!({
        "decision": "allow", "workspace_id": second.workspace_id, "project_mode": "selected",
        "project_ids": [h.tenant.project_id], "scopes": SCOPES.split(' ').collect::<Vec<_>>(),
    });
    let consent = h.consent_for(&person, &authorize_body()).await;
    h.decide(
        Some(&person),
        &consent.request_id,
        Some(&consent.consent_token),
        &[],
        &wrong,
    )
    .await
    .assert_flow_error(400, "invalid_selection");
    h.finish().await;
}

#[tokio::test]
async fn at_most_ten_grants_and_unredeemed_ones_do_not_count() {
    let scratch = oauth_scratch_or_skip!("wp2_limit", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let all = ["projects:read", "work_items:read"];

    // Ten connected grants (each has a token) fill the limit.
    for _ in 0..10 {
        let grant = seed_grant(h.db(), person.id, h.tenant.workspace_id, "all", &all).await;
        seed_access_token(h.db(), grant, "api", &all).await;
    }
    let consent = h.consent_for(&person, &authorize_body()).await;
    let reply = h
        .decide(
            Some(&person),
            &consent.request_id,
            Some(&consent.consent_token),
            &[],
            &h.allow_body(&[]),
        )
        .await;
    reply.assert_flow_error(409, "too_many_grants");
    let body = reply.json();
    assert_eq!(body["details"]["max_active_grants"], 10);
    assert_eq!(body["details"]["grants"].as_array().unwrap().len(), 10);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants").await,
        10,
        "no eleventh grant"
    );
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_authorization_codes").await, 0);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_authorization_requests WHERE status = 'pending'")
            .await,
        1
    );
    let data = h
        .begin(Some(&person), &json!({"request_id": consent.request_id}))
        .await
        .data();
    assert_eq!(data["active_grant_count"], 10);
    // The reload replaced the consent token.
    let token = data["consent_token"].as_str().expect("token").to_owned();
    // Another user is not affected.
    let other = h.add_user(h.tenant.workspace_id).await;
    h.code_for(&other, &authorize_body(), &h.allow_body(&[])).await;

    // Disconnecting one makes room.
    let victim: Uuid = Uuid::parse_str(body["details"]["grants"][0]["grant_id"].as_str().unwrap()).unwrap();
    exec(
        h.db(),
        "UPDATE oauth_grants SET status = 'revoked', revoked_at = now(), revoke_reason = 'user_revoked' WHERE id = $1",
        vec![victim.into()],
    )
    .await;
    h.decide(
        Some(&person),
        &consent.request_id,
        Some(&token),
        &[],
        &h.allow_body(&[]),
    )
    .await
    .data();
    assert_eq!(
        h.n(&format!(
            "SELECT count(*) AS n FROM oauth_grants WHERE user_id = '{}' AND status = 'active'",
            person.id
        ))
        .await,
        10
    );
    h.finish().await;
}

#[tokio::test]
async fn grants_that_never_received_a_token_and_whose_code_expired_are_revoked_before_counting() {
    let scratch = oauth_scratch_or_skip!("wp2_sweep", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();

    // Ten consents that were never redeemed.
    let mut abandoned = Vec::new();
    for _ in 0..10 {
        let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
        abandoned.push(code);
    }
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'active'")
            .await,
        10
    );
    // While their codes are alive they count: the eleventh is refused.
    let consent = h.consent_for(&person, &authorize_body()).await;
    h.decide(
        Some(&person),
        &consent.request_id,
        Some(&consent.consent_token),
        &[],
        &h.allow_body(&[]),
    )
    .await
    .assert_flow_error(409, "too_many_grants");
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'active'")
            .await,
        10
    );
    // One of them is redeemed: it has tokens, so it stays.
    let kept = h.redeem(&abandoned[0], API_RES).await;
    assert_eq!(kept.status, StatusCode::OK);
    // Their codes run out.
    exec(
        h.db(),
        "UPDATE oauth_authorization_codes SET expires_at = now() - interval '1 second' WHERE consumed_at IS NULL",
        vec![],
    )
    .await;
    h.decide(
        Some(&person),
        &consent.request_id,
        Some(&consent.consent_token),
        &[],
        &h.allow_body(&[]),
    )
    .await
    .data();
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'revoked' AND revoke_reason = 'expired'")
            .await,
        9,
        "the nine unredeemed grants were ended"
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'active'")
            .await,
        2,
        "the redeemed one and the new one"
    );
    assert_eq!(h.audit_count("grant_revoked").await, 9);
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Token endpoint
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn token_endpoint_request_errors() {
    let scratch = oauth_scratch_or_skip!("wp2_tokerr", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    let before = h.side_effects().await;
    let good = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", REDIRECT),
        ("client_id", "ramflux"),
        ("code_verifier", VERIFIER),
        ("resource", MCP_RES),
    ];
    let zero_code = format!("svc_{}", "0".repeat(64));
    let without = |name: &str| -> Vec<(&str, &str)> { good.iter().copied().filter(|(n, _)| *n != name).collect() };
    let replaced = |name: &str, value: &'static str| -> Vec<(&str, &str)> {
        good.iter()
            .map(|(n, v)| if *n == name { (*n, value) } else { (*n, *v) })
            .collect()
    };
    let with_code = |value: &str| -> Vec<(&str, String)> {
        good.iter()
            .map(|(n, v)| {
                (
                    *n,
                    if *n == "code" {
                        value.to_owned()
                    } else {
                        (*v).to_owned()
                    },
                )
            })
            .collect()
    };

    // Transport.
    let reply = h
        .send(
            Request::post(TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    reply.assert_oauth_error(400, "invalid_request");
    let reply = h.send(Request::post(TOKEN).body(Body::empty()).unwrap()).await;
    reply.assert_oauth_error(400, "invalid_request");
    let mut with_query = form(TOKEN, &good);
    *with_query.uri_mut() = format!("{TOKEN}?code=x").parse().unwrap();
    h.send(with_query).await.assert_oauth_error(400, "invalid_request");
    let mut duplicated = good.to_vec();
    duplicated.push(("resource", API_RES));
    h.token(&duplicated).await.assert_oauth_error(400, "invalid_request");
    let bytes = Request::post(TOKEN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("grant_type=%zz"))
        .unwrap();
    h.send(bytes).await.assert_oauth_error(400, "invalid_request");
    let huge = Request::post(TOKEN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("a={}", "x".repeat(20_000))))
        .unwrap();
    h.send(huge).await.assert_oauth_error(400, "invalid_request");
    let reply = h.send(Request::get(TOKEN).body(Body::empty()).unwrap()).await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);

    // Parameters.
    for name in [
        "grant_type",
        "client_id",
        "code",
        "redirect_uri",
        "code_verifier",
        "resource",
    ] {
        h.token(&without(name)).await.assert_oauth_error(400, "invalid_request");
    }
    h.token(&replaced("code_verifier", "too-short"))
        .await
        .assert_oauth_error(400, "invalid_request");
    h.token(&replaced("client_id", "nobody"))
        .await
        .assert_oauth_error(401, "invalid_client");
    h.token(&replaced("grant_type", "password"))
        .await
        .assert_oauth_error(400, "unsupported_grant_type");
    h.token(&replaced("grant_type", "client_credentials"))
        .await
        .assert_oauth_error(400, "unsupported_grant_type");
    h.token(&replaced("resource", "http://127.0.0.1:18097/other"))
        .await
        .assert_oauth_error(400, "invalid_target");
    h.token(&replaced("code", "svc_nothex"))
        .await
        .assert_oauth_error(400, "invalid_grant");
    let owned = with_code(&zero_code);
    let borrowed: Vec<(&str, &str)> = owned.iter().map(|(n, v)| (*n, v.as_str())).collect();
    h.token(&borrowed).await.assert_oauth_error(400, "invalid_grant");
    // A disabled client looks like an unknown one.
    exec(
        h.db(),
        "UPDATE oauth_clients SET status = 'disabled' WHERE client_id = 'ramflux'",
        vec![],
    )
    .await;
    h.token(&good).await.assert_oauth_error(401, "invalid_client");
    exec(
        h.db(),
        "UPDATE oauth_clients SET status = 'active' WHERE client_id = 'ramflux'",
        vec![],
    )
    .await;

    assert_eq!(h.side_effects().await, before, "no request created anything");
    // The code is untouched by all of it.
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_authorization_codes WHERE consumed_at IS NULL")
            .await,
        1
    );
    assert_eq!(h.redeem(&code, MCP_RES).await.status, StatusCode::OK);
    h.finish().await;
}

#[tokio::test]
async fn code_failures_are_bounded_and_do_not_revoke() {
    let scratch = oauth_scratch_or_skip!("wp2_codefail", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let wrong_verifier = "w".repeat(50);
    let wrong_redirect = "http://127.0.0.1:53683/oauth/sylvode/callback";
    let hr = &h;
    let attempt = move |code: &str, verifier: &str, redirect: &str| {
        let pairs = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
            ("client_id", "ramflux"),
            ("code_verifier", verifier),
            ("resource", MCP_RES),
        ];
        let request = form(TOKEN, &pairs);
        async move { hr.send(request).await }
    };

    // One mismatch, then the right redemption succeeds.
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    attempt(&code, &wrong_verifier, REDIRECT)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_tokens").await, 0);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_authorization_codes WHERE consumed_at IS NULL")
            .await,
        1
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'active'")
            .await,
        1,
        "the grant is not revoked"
    );
    attempt(&code, VERIFIER, REDIRECT).await.json();
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_tokens").await, 2);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_rate_hits WHERE bucket = 'code_failures'")
            .await,
        0,
        "the counter is gone"
    );

    // Redirect mismatch between the request and the redemption, twice, then success.
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    attempt(&code, VERIFIER, wrong_redirect)
        .await
        .assert_oauth_error(400, "invalid_grant");
    attempt(&code, VERIFIER, "http://127.0.0.1:53682/oauth/sylvode/other")
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(attempt(&code, VERIFIER, REDIRECT).await.status, StatusCode::OK);

    // Three mismatches invalidate the code: the right redemption then fails with the standard
    // error, nothing is issued and the grant stays (no token was ever issued from the code).
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    let tokens_before = h.n("SELECT count(*) AS n FROM oauth_tokens").await;
    attempt(&code, &wrong_verifier, REDIRECT)
        .await
        .assert_oauth_error(400, "invalid_grant");
    attempt(&code, VERIFIER, wrong_redirect)
        .await
        .assert_oauth_error(400, "invalid_grant");
    attempt(&code, &"x".repeat(60), REDIRECT)
        .await
        .assert_oauth_error(400, "invalid_grant");
    attempt(&code, VERIFIER, REDIRECT)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_tokens").await, tokens_before);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_authorization_codes WHERE consumed_at IS NULL")
            .await,
        1,
        "never consumed"
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'revoked' AND revoke_reason = 'code_replay'")
            .await,
        0
    );
    let reasons = h.audit_detail("token_rejected", "reason").await;
    assert_eq!(
        reasons.iter().filter(|r| *r == "verifier_mismatch").count(),
        2,
        "one before the sequence, one in it"
    );
    assert_eq!(reasons.iter().filter(|r| *r == "redirect_mismatch").count(), 3);
    assert_eq!(reasons.iter().filter(|r| *r == "code_invalidated").count(), 1);
    assert!(
        reasons.contains(&"code_expired".to_owned()),
        "the fourth attempt met an invalidated code"
    );
    // Its grant has no token and no live code: the next consent sweeps it as expired.
    let consent = h.consent_for(&person, &authorize_body()).await;
    h.decide(
        Some(&person),
        &consent.request_id,
        Some(&consent.consent_token),
        &[],
        &h.allow_body(&[]),
    )
    .await
    .data();
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE revoke_reason = 'expired'")
            .await,
        1
    );
    h.finish().await;
}

#[tokio::test]
async fn code_reuse_revokes_the_grant_and_every_token_of_it() {
    let scratch = oauth_scratch_or_skip!("wp2_replay", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let connected = h.connect(&person, &h.allow_body(&[])).await;
    let mut listener = sea_orm::sqlx::postgres::PgListener::connect_with(h.db().get_postgres_connection_pool())
        .await
        .expect("listener");
    listener.listen(GRANT_CHANGED_CHANNEL).await.expect("listen");

    // Any second redemption is the replay case, even with a different resource.
    h.redeem(&connected.code, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(
        h.grant_status(connected.grant_id).await,
        ("revoked".to_owned(), Some("code_replay".to_owned()))
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE revoked_at IS NULL")
            .await,
        0
    );
    assert_eq!(h.audit_count("code_replay").await, 1);
    assert_eq!(h.audit_count("grant_revoked").await, 1);
    let notification = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("a notification arrives")
        .expect("listener works");
    assert_eq!(notification.payload(), connected.grant_id.to_string());
    // The tokens issued from the code no longer work.
    assert_eq!(
        validate_access_token(
            h.db(),
            &connected.access,
            AudienceRule::Api(ForwardCredential::absent())
        )
        .await
        .expect_err("revoked")
        .reason,
        RejectReason::TokenRevoked
    );
    h.refresh(&connected.refresh, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    h.finish().await;
}

#[tokio::test]
async fn code_state_checks() {
    let scratch = oauth_scratch_or_skip!("wp2_codestate", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();

    // Expired: invalid_grant, nothing issued, the grant is not revoked by it.
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    exec(
        h.db(),
        "UPDATE oauth_authorization_codes SET expires_at = now() - interval '1 second'",
        vec![],
    )
    .await;
    h.redeem(&code, MCP_RES).await.assert_oauth_error(400, "invalid_grant");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_tokens").await, 0);
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'active'")
            .await,
        1
    );

    // The code of one resource list cannot be redeemed for another.
    let only_api = with(authorize_body(), "resource", json!([API_RES]));
    let code = h.code_for(&person, &only_api, &h.allow_body(&[])).await;
    h.redeem(&code, MCP_RES).await.assert_oauth_error(400, "invalid_target");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_tokens").await, 0);
    assert_eq!(
        h.redeem(&code, API_RES).await.status,
        StatusCode::OK,
        "and the code was not spent"
    );

    // A second client cannot redeem the first one's code.
    exec(
        h.db(),
        "INSERT INTO oauth_clients (client_id, display_name, client_type, token_endpoint_auth_method, redirect_uris, \
         allowed_scopes, allowed_resources) VALUES ('otherapp', 'Other', 'public', 'none', \
         ARRAY['http://127.0.0.1/cb'], ARRAY['projects:read'], ARRAY['api'])",
        vec![],
    )
    .await;
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    let tokens = h.n("SELECT count(*) AS n FROM oauth_tokens").await;
    h.token(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", REDIRECT),
        ("client_id", "otherapp"),
        ("code_verifier", VERIFIER),
        ("resource", API_RES),
    ])
    .await
    .assert_oauth_error(400, "invalid_grant");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_tokens").await, tokens);
    assert_eq!(h.redeem(&code, API_RES).await.status, StatusCode::OK);

    // Live state: the user is gone, the client disabled, membership removed.
    for (label, break_it, reason) in [
        (
            "user disabled",
            "UPDATE users SET is_active = false",
            "account_disabled",
        ),
        (
            "password changed",
            "UPDATE users SET session_version = session_version + 1",
            "password_changed",
        ),
        (
            "membership removed",
            "DELETE FROM workspace_members",
            "membership_removed",
        ),
        (
            "client disabled",
            "UPDATE oauth_clients SET status = 'disabled'",
            "client_disabled",
        ),
        (
            "workspace off",
            "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) \
             SELECT id, false FROM workspaces \
             ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = false",
            "workspace_disabled",
        ),
    ] {
        let scratch = oauth_scratch_or_skip!("wp2_codestate_live", Start::Complete);
        let live = Harness::new(scratch).await;
        let person = live.member();
        let code = live.code_for(&person, &authorize_body(), &live.allow_body(&[])).await;
        exec(live.db(), break_it, vec![]).await;
        let reply = live.redeem(&code, API_RES).await;
        if reason == "client_disabled" {
            reply.assert_oauth_error(401, "invalid_client");
        } else {
            reply.assert_oauth_error(400, "invalid_grant");
            assert_eq!(
                live.n(&format!(
                    "SELECT count(*) AS n FROM oauth_grants WHERE status = 'revoked' AND revoke_reason = '{reason}'"
                ))
                .await,
                1,
                "{label}"
            );
        }
        assert_eq!(live.n("SELECT count(*) AS n FROM oauth_tokens").await, 0, "{label}");
        live.finish().await;
    }
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn refresh_replay_revokes_the_grant_and_notifies() {
    let scratch = oauth_scratch_or_skip!("wp2_reuse", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let connected = h.connect(&person, &h.allow_body(&[])).await;
    let first = h.refresh(&connected.refresh, API_RES).await.json();
    let newest = first["refresh_token"].as_str().unwrap().to_owned();
    let second = h.refresh(&newest, API_RES).await.json();
    let latest = second["refresh_token"].as_str().unwrap().to_owned();
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE kind = 'refresh'")
            .await,
        3
    );
    let mut listener = sea_orm::sqlx::postgres::PgListener::connect_with(h.db().get_postgres_connection_pool())
        .await
        .expect("listener");
    listener.listen(GRANT_CHANGED_CHANNEL).await.expect("listen");

    // The first token, rotated twice ago, comes back.
    h.refresh(&connected.refresh, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(
        h.grant_status(connected.grant_id).await,
        ("revoked".to_owned(), Some("refresh_reuse".to_owned()))
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE revoked_at IS NULL")
            .await,
        0,
        "every token of the grant"
    );
    assert_eq!(h.audit_count("refresh_reuse_detected").await, 1);
    assert_eq!(h.audit_count("grant_revoked").await, 1);
    let notification = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("a notification arrives")
        .expect("listener works");
    assert_eq!(notification.payload(), connected.grant_id.to_string());
    // The newest refresh token is dead as well.
    h.refresh(&latest, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    h.finish().await;
}

/// A refresh request for the API resource with `extra` replacing or (when empty) removing fields.
async fn refresh_with(h: &Harness, refresh_token: &str, extra: &[(&str, &str)]) -> Reply {
    let mut all: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", "ramflux"),
        ("resource", API_RES),
    ];
    for (name, value) in extra {
        all.retain(|(n, _)| n != name);
        if !value.is_empty() {
            all.push((*name, *value));
        }
    }
    h.send(form(TOKEN, &all)).await
}

#[tokio::test]
async fn refresh_errors_scope_and_resource_rules() {
    let scratch = oauth_scratch_or_skip!("wp2_refresh", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let connected = h.connect(&person, &h.allow_body(&[])).await;
    let before = h.side_effects().await;
    refresh_with(&h, &connected.refresh, &[("refresh_token", "")])
        .await
        .assert_oauth_error(400, "invalid_request");
    refresh_with(&h, &connected.refresh, &[("resource", "")])
        .await
        .assert_oauth_error(400, "invalid_request");
    refresh_with(&h, &connected.refresh, &[("resource", "http://127.0.0.1:18097/other")])
        .await
        .assert_oauth_error(400, "invalid_target");
    refresh_with(&h, &connected.refresh, &[("refresh_token", "svr_zz")])
        .await
        .assert_oauth_error(400, "invalid_grant");
    refresh_with(&h, &connected.refresh, &[("refresh_token", "sva_zz")])
        .await
        .assert_oauth_error(400, "invalid_grant");
    refresh_with(&h, &connected.refresh, &[("client_id", "nobody")])
        .await
        .assert_oauth_error(401, "invalid_client");
    refresh_with(&h, &connected.refresh, &[("scope", "bogus")])
        .await
        .assert_oauth_error(400, "invalid_scope");
    refresh_with(&h, &connected.refresh, &[("scope", "tasks:dispatch")])
        .await
        .assert_oauth_error(400, "invalid_scope");
    refresh_with(&h, &connected.refresh, &[("scope", "forms:read")])
        .await
        .assert_oauth_error(400, "invalid_scope");
    refresh_with(&h, &connected.refresh, &[("scope", "work_items:write")])
        .await
        .assert_oauth_error(400, "invalid_scope");
    // In the grant but empty for the resource (offline_access alone for the API).
    refresh_with(&h, &connected.refresh, &[("scope", "offline_access")])
        .await
        .assert_oauth_error(400, "invalid_scope");
    assert_eq!(h.side_effects().await, before, "no refusal rotated anything");
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE consumed_at IS NOT NULL")
            .await,
        0
    );
    // The access token an unrelated access token as a refresh token is no refresh token.
    refresh_with(&h, &connected.refresh, &[("refresh_token", connected.access.as_str())])
        .await
        .assert_oauth_error(400, "invalid_grant");

    // A narrower scope narrows the access token, not the grant.
    let narrow = refresh_with(&h, &connected.refresh, &[("scope", "projects:read")])
        .await
        .json();
    assert_eq!(narrow["scope"], "projects:read");
    let again = h
        .refresh(narrow["refresh_token"].as_str().unwrap(), MCP_RES)
        .await
        .json();
    assert_eq!(
        again["scope"], "projects:read work_items:read",
        "the next refresh starts from the grant again"
    );

    // A grant that never had the MCP resource.
    let api_only = with(authorize_body(), "resource", json!([API_RES]));
    let code = h.code_for(&person, &api_only, &h.allow_body(&[])).await;
    let tokens = h.redeem(&code, API_RES).await.json();
    h.refresh(tokens["refresh_token"].as_str().unwrap(), MCP_RES)
        .await
        .assert_oauth_error(400, "invalid_target");
    // A refusal for the resource does not consume the token.
    assert_eq!(
        h.refresh(tokens["refresh_token"].as_str().unwrap(), API_RES)
            .await
            .status,
        StatusCode::OK
    );

    // A grant without offline_access has no refresh token.
    let no_offline = json!({
        "decision": "allow", "workspace_id": h.tenant.workspace_id, "project_mode": "all",
        "scopes": ["projects:read", "work_items:read", "notifications:read", "tasks:read"],
    });
    let body = with(
        authorize_body(),
        "scope",
        json!("projects:read work_items:read notifications:read tasks:read"),
    );
    let code = h.code_for(&person, &body, &no_offline).await;
    let issued = h.redeem(&code, API_RES).await.json();
    assert!(issued.get("refresh_token").is_none());
    h.finish().await;
}

#[tokio::test]
async fn expiry_of_codes_tokens_and_grants_at_the_configured_limits() {
    let scratch = oauth_scratch_or_skip!("wp2_expiry", Start::Complete);
    let h = Harness::with_config(scratch, |config| {
        config.access_token_ttl_seconds = 120;
        config.refresh_token_idle_days = 7;
        config.grant_max_lifetime_days = 10;
        config.authorization_code_ttl_seconds = 30;
    })
    .await;
    let person = h.member();
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    let code_left = h.seconds_left("codes", hash_secret(&code).as_hex()).await;
    assert!((20.0..=30.5).contains(&code_left), "code ttl 30 s, has {code_left}");
    assert!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE expires_at BETWEEN now() + interval '9 days 23 hours' AND now() + interval '10 days'").await == 1,
        "the grant lasts the configured 10 days"
    );
    let issued = h.redeem(&code, API_RES).await.json();
    assert!((110..=120).contains(&issued["expires_in"].as_i64().unwrap()));
    let access_left = h
        .seconds_left("tokens", hash_secret(issued["access_token"].as_str().unwrap()).as_hex())
        .await;
    assert!(
        (110.0..=120.5).contains(&access_left),
        "access ttl 120 s, has {access_left}"
    );
    let refresh_left = h
        .seconds_left(
            "tokens",
            hash_secret(issued["refresh_token"].as_str().unwrap()).as_hex(),
        )
        .await;
    let seven_days = 7.0 * 86_400.0;
    assert!(
        (seven_days - 60.0..=seven_days + 0.5).contains(&refresh_left),
        "refresh idle 7 days, has {refresh_left}"
    );

    // Near the end of the grant nothing issued outlives it.
    exec(
        h.db(),
        "UPDATE oauth_grants SET expires_at = now() + interval '45 seconds'",
        vec![],
    )
    .await;
    let refreshed = h
        .refresh(issued["refresh_token"].as_str().unwrap(), API_RES)
        .await
        .json();
    assert!(
        (35..=45).contains(&refreshed["expires_in"].as_i64().unwrap()),
        "{refreshed}"
    );
    let refresh_left = h
        .seconds_left(
            "tokens",
            hash_secret(refreshed["refresh_token"].as_str().unwrap()).as_hex(),
        )
        .await;
    assert!(
        refresh_left <= 45.5,
        "the refresh token ends with the grant, has {refresh_left}"
    );

    // The refresh token's idle time is up: the grant is ended as expired.
    exec(h.db(), "UPDATE oauth_tokens SET expires_at = now() - interval '1 second' WHERE kind = 'refresh' AND consumed_at IS NULL", vec![]).await;
    h.refresh(refreshed["refresh_token"].as_str().unwrap(), API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE status = 'revoked' AND revoke_reason = 'expired'")
            .await,
        1
    );

    // The grant's own end, on a fresh connection.
    let person = h.member();
    let connected = h.connect(&person, &h.allow_body(&[])).await;
    exec(
        h.db(),
        "UPDATE oauth_grants SET expires_at = now() - interval '1 second' WHERE status = 'active'",
        vec![],
    )
    .await;
    assert_eq!(
        validate_access_token(
            h.db(),
            &connected.access,
            AudienceRule::Api(ForwardCredential::absent())
        )
        .await
        .map(|_| ())
        .unwrap_err()
        .reason,
        RejectReason::GrantExpired
    );
    h.refresh(&connected.refresh, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_grants WHERE revoke_reason = 'expired'")
            .await,
        2
    );

    // An expired access token is refused by validation (WP1) and a code of an ended grant is refused here.
    let code = h.code_for(&person, &authorize_body(), &h.allow_body(&[])).await;
    exec(
        h.db(),
        "UPDATE oauth_grants SET expires_at = now() - interval '1 second' WHERE status = 'active'",
        vec![],
    )
    .await;
    h.redeem(&code, API_RES).await.assert_oauth_error(400, "invalid_grant");
    h.finish().await;
}

#[tokio::test]
async fn refresh_stops_when_the_live_state_of_the_grant_fails() {
    for (label, break_it, reason) in [
        (
            "user disabled",
            "UPDATE users SET is_active = false",
            "account_disabled",
        ),
        (
            "password changed",
            "UPDATE users SET session_version = session_version + 1",
            "password_changed",
        ),
        (
            "membership removed",
            "DELETE FROM workspace_members",
            "membership_removed",
        ),
        (
            "workspace off",
            "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) SELECT id, false FROM workspaces \
             ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = false",
            "workspace_disabled",
        ),
        (
            "client disabled",
            "UPDATE oauth_clients SET status = 'disabled'",
            "client_disabled",
        ),
    ] {
        let scratch = oauth_scratch_or_skip!("wp2_refresh_live", Start::Complete);
        let h = Harness::new(scratch).await;
        let person = h.member();
        let connected = h.connect(&person, &h.allow_body(&[])).await;
        exec(h.db(), break_it, vec![]).await;
        let reply = h.refresh(&connected.refresh, API_RES).await;
        if reason == "client_disabled" {
            reply.assert_oauth_error(401, "invalid_client");
        } else {
            reply.assert_oauth_error(400, "invalid_grant");
            assert_eq!(
                h.grant_status(connected.grant_id).await,
                ("revoked".to_owned(), Some(reason.to_owned())),
                "{label}"
            );
        }
        assert_eq!(
            h.n("SELECT count(*) AS n FROM oauth_tokens WHERE kind = 'access'")
                .await,
            1,
            "{label}: nothing new issued"
        );
        h.finish().await;
    }
}

// ---------------------------------------------------------------------------------------------
// Revocation
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn revocation_rules() {
    let scratch = oauth_scratch_or_skip!("wp2_revoke", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let connected = h.connect(&person, &h.allow_body(&[])).await;
    let ramflux = |token: &str| [("token", token.to_owned()), ("client_id", "ramflux".to_owned())];
    let hr = &h;
    let send = move |pairs: Vec<(&str, String)>| {
        let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let request = form(REVOKE, &borrowed);
        async move { hr.send(request).await }
    };

    // Errors.
    send(vec![("client_id", "ramflux".to_owned())])
        .await
        .assert_oauth_error(400, "invalid_request");
    send(vec![("token", String::new()), ("client_id", "ramflux".to_owned())])
        .await
        .assert_oauth_error(400, "invalid_request");
    send(vec![("token", connected.access.clone())])
        .await
        .assert_oauth_error(400, "invalid_request");
    send(vec![
        ("token", connected.access.clone()),
        ("client_id", "nobody".to_owned()),
    ])
    .await
    .assert_oauth_error(401, "invalid_client");
    let reply = h
        .send(
            Request::post(REVOKE)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    reply.assert_oauth_error(400, "invalid_request");
    let mut with_query = form(REVOKE, &[("token", &connected.access), ("client_id", "ramflux")]);
    *with_query.uri_mut() = format!("{REVOKE}?token=x").parse().unwrap();
    h.send(with_query).await.assert_oauth_error(400, "invalid_request");
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE revoked_at IS NOT NULL")
            .await,
        0
    );

    // Unknown, malformed, wrong-kind and other-client tokens answer 200 and change nothing.
    for token in [
        "garbage".to_owned(),
        format!("sva_{}", "0".repeat(64)),
        format!("svr_{}", "0".repeat(64)),
        connected.code.clone(),
        "sva_short".to_owned(),
    ] {
        let reply = send(ramflux(&token).to_vec()).await;
        assert_eq!(reply.status, StatusCode::OK, "{token}");
        assert!(reply.body.is_empty());
    }
    exec(
        h.db(),
        "INSERT INTO oauth_clients (client_id, display_name, client_type, token_endpoint_auth_method, redirect_uris, \
         allowed_scopes, allowed_resources) VALUES ('otherapp', 'Other', 'public', 'none', \
         ARRAY['http://127.0.0.1/cb'], ARRAY['projects:read'], ARRAY['api'])",
        vec![],
    )
    .await;
    for token in [&connected.access, &connected.refresh] {
        let reply = send(vec![("token", token.clone()), ("client_id", "otherapp".to_owned())]).await;
        assert_eq!(reply.status, StatusCode::OK);
    }
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE revoked_at IS NOT NULL")
            .await,
        0,
        "another client revokes nothing"
    );
    assert_eq!(h.audit_count("token_revoked").await, 0);

    // An access token revokes itself only; the hint is only a hint.
    let mut listener = sea_orm::sqlx::postgres::PgListener::connect_with(h.db().get_postgres_connection_pool())
        .await
        .expect("listener");
    listener.listen(GRANT_CHANGED_CHANNEL).await.expect("listen");
    let reply = send(vec![
        ("token", connected.access.clone()),
        ("client_id", "ramflux".to_owned()),
        ("token_type_hint", "refresh_token".to_owned()),
    ])
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(h.grant_status(connected.grant_id).await.0, "active");
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE revoked_at IS NOT NULL AND kind = 'access'")
            .await,
        1
    );
    assert_eq!(
        validate_access_token(
            h.db(),
            &connected.access,
            AudienceRule::Api(ForwardCredential::absent())
        )
        .await
        .map(|_| ())
        .unwrap_err()
        .reason,
        RejectReason::TokenRevoked
    );
    let woken = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("streams are woken")
        .expect("listener works");
    assert_eq!(woken.payload(), connected.grant_id.to_string());
    // Revoking it again changes nothing and writes no second row.
    send(ramflux(&connected.access).to_vec()).await;
    assert_eq!(h.audit_count("token_revoked").await, 1);
    // The refresh token still works.
    assert_eq!(h.refresh(&connected.refresh, API_RES).await.status, StatusCode::OK);

    h.finish().await;
}

#[tokio::test]
async fn revoking_a_refresh_token_with_an_access_hint_ends_the_grant() {
    let scratch = oauth_scratch_or_skip!("wp2_revoke_refresh", Start::Complete);
    let h = Harness::new(scratch).await;
    let person = h.member();
    let connected = h.connect(&person, &h.allow_body(&[])).await;
    let reply = h
        .revoke(&[
            ("token", &connected.refresh),
            ("client_id", "ramflux"),
            ("token_type_hint", "access_token"),
        ])
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        h.grant_status(connected.grant_id).await,
        ("revoked".to_owned(), Some("client_revoked".to_owned()))
    );
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE revoked_at IS NULL")
            .await,
        0
    );
    assert_eq!(h.audit_count("grant_revoked").await, 1);
    // Repeating it answers 200 again and adds nothing.
    let reply = h
        .revoke(&[("token", &connected.refresh), ("client_id", "ramflux")])
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(h.audit_count("grant_revoked").await, 1);
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Rate limits
// ---------------------------------------------------------------------------------------------

async fn fill_window(h: &Harness, bucket: RateBucket, key: &RateKey, hits: u32) {
    exec(
        h.db(),
        "INSERT INTO oauth_rate_hits (bucket, key_hash, window_started, hit_count) VALUES ($1, $2, now(), $3) \
         ON CONFLICT (bucket, key_hash) DO UPDATE SET hit_count = $3, window_started = now()",
        vec![
            bucket.as_str().into(),
            key.as_hex().into(),
            i32::try_from(hits).expect("small").into(),
        ],
    )
    .await;
}

fn local_key() -> RateKey {
    // Requests without a peer address (these tests) are counted under the unspecified address.
    RateKey::from_ip(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
}

#[tokio::test]
async fn the_endpoints_are_rate_limited_and_say_when_to_retry() {
    let scratch = oauth_scratch_or_skip!("wp2_rate", Start::Complete);
    let h = Harness::new(scratch).await;
    h.limits_on();
    let person = h.member();

    // Every request counts, failures included: sixty bad token requests, then the limit.
    for _ in 0..RateBucket::TokenIp.limit() {
        h.token(&[("grant_type", "password"), ("client_id", "ramflux")])
            .await
            .assert_oauth_error(400, "unsupported_grant_type");
    }
    let limited = h.token(&[("grant_type", "password"), ("client_id", "ramflux")]).await;
    limited.assert_oauth_error(429, "rate_limited");
    let retry: u64 = limited.headers[header::RETRY_AFTER].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry));
    assert_eq!(limited.headers[header::CACHE_CONTROL], "no-store");
    // Even a good request is refused while the window is full.
    let code = {
        let consent = h.consent_for(&person, &authorize_body()).await;
        let reply = h
            .decide(
                Some(&person),
                &consent.request_id,
                Some(&consent.consent_token),
                &[],
                &h.allow_body(&[]),
            )
            .await;
        query_of(reply.data()["redirect_to"].as_str().unwrap())["code"].clone()
    };
    h.redeem(&code, API_RES).await.assert_oauth_error(429, "rate_limited");
    assert_eq!(h.n("SELECT count(*) AS n FROM oauth_tokens").await, 0);

    // The other buckets.
    fill_window(&h, RateBucket::Revoke, &local_key(), RateBucket::Revoke.limit()).await;
    let reply = h.revoke(&[("token", "x"), ("client_id", "ramflux")]).await;
    reply.assert_oauth_error(429, "rate_limited");
    assert!(reply.headers.contains_key(header::RETRY_AFTER));
    fill_window(&h, RateBucket::Authorize, &local_key(), RateBucket::Authorize.limit()).await;
    let reply = h.begin(None, &authorize_body()).await;
    reply.assert_flow_error(429, "rate_limited");
    assert!(reply.headers.contains_key(header::RETRY_AFTER));
    fill_window(
        &h,
        RateBucket::Decision,
        &RateKey::from_id(person.id),
        RateBucket::Decision.limit(),
    )
    .await;
    let reply = h
        .decide(
            Some(&person),
            &Uuid::new_v4().to_string(),
            None,
            &[],
            &json!({"decision": "deny"}),
        )
        .await;
    reply.assert_flow_error(429, "rate_limited");
    assert!(reply.headers.contains_key(header::RETRY_AFTER));
    // The no-account decision is limited per address.
    fill_window(&h, RateBucket::Decision, &local_key(), RateBucket::Decision.limit()).await;
    h.decide(
        None,
        &Uuid::new_v4().to_string(),
        None,
        &[],
        &json!({"decision": "no_account"}),
    )
    .await
    .assert_flow_error(429, "rate_limited");
    assert_eq!(h.side_effects().await[3], 0);
    h.finish().await;
}

#[tokio::test]
async fn refresh_is_limited_per_grant() {
    let scratch = oauth_scratch_or_skip!("wp2_rate_grant", Start::Complete);
    let h = Harness::new(scratch).await;
    h.limits_on();
    let person = h.member();
    let connected = h.connect(&person, &h.allow_body(&[])).await;
    fill_window(
        &h,
        RateBucket::TokenGrant,
        &RateKey::from_id(connected.grant_id),
        RateBucket::TokenGrant.limit(),
    )
    .await;
    let reply = h.refresh(&connected.refresh, API_RES).await;
    reply.assert_oauth_error(429, "rate_limited");
    assert!(reply.headers.contains_key(header::RETRY_AFTER));
    assert_eq!(
        h.n("SELECT count(*) AS n FROM oauth_tokens WHERE consumed_at IS NOT NULL")
            .await,
        0,
        "nothing was rotated"
    );
    // The limit is the grant's: a replay is still detected first.
    exec(
        h.db(),
        "DELETE FROM oauth_rate_hits WHERE bucket = 'token_grant'",
        vec![],
    )
    .await;
    let first = h.refresh(&connected.refresh, API_RES).await;
    assert_eq!(first.status, StatusCode::OK);
    fill_window(
        &h,
        RateBucket::TokenGrant,
        &RateKey::from_id(connected.grant_id),
        RateBucket::TokenGrant.limit(),
    )
    .await;
    h.refresh(&connected.refresh, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    assert_eq!(
        h.grant_status(connected.grant_id).await.1.as_deref(),
        Some("refresh_reuse")
    );
    h.finish().await;
}

#[tokio::test]
async fn without_configuration_every_endpoint_answers_404() {
    let scratch = oauth_scratch_or_skip!("wp2_off", Start::Complete);
    let h = Harness::new(scratch).await;
    let off = routes(None).with_state(h.state.clone());
    for path in [
        REQUESTS.to_owned(),
        format!("{REQUESTS}/{}/decision", Uuid::new_v4()),
        TOKEN.to_owned(),
        REVOKE.to_owned(),
    ] {
        let response = off
            .clone()
            .oneshot(Request::post(&path).body(Body::empty()).unwrap())
            .await
            .expect("answers");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Secrets stay out of logs, audit rows and storage
// ---------------------------------------------------------------------------------------------

thread_local! {
    /// Lines captured on this thread while a test has armed the capture.
    static CAPTURED: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

fn push_line(line: String) {
    CAPTURED.with(|captured| {
        if let Some(lines) = captured.borrow_mut().as_mut() {
            lines.push(line);
        }
    });
}

/// Renders the fields of an event or span.
struct Render(String);

impl tracing::field::Visit for Render {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

/// A process-wide subscriber that records only on threads that armed the capture. Being the
/// global default (instead of a scoped one per test) keeps the callsite interest the same on
/// every thread, so parallel tests cannot hide events from it.
struct Capture;

impl tracing::Subscriber for Capture {
    /// Without this the default answer follows `enabled` on the thread that happens to register
    /// the callsite first, and an unarmed thread would switch the callsite off for everybody.
    fn register_callsite(&self, _: &'static tracing::Metadata<'static>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        CAPTURED.with(|captured| captured.borrow().is_some())
    }

    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut render = Render(format!("span {} ", attrs.metadata().name()));
        attrs.record(&mut render);
        push_line(render.0);
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut render = Render(format!("{} ", event.metadata().target()));
        event.record(&mut render);
        push_line(render.0);
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

fn start_capture() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = tracing::subscriber::set_global_default(Capture);
    });
    CAPTURED.with(|captured| *captured.borrow_mut() = Some(Vec::new()));
}

fn captured_text() -> String {
    CAPTURED.with(|captured| {
        captured
            .borrow()
            .as_ref()
            .map(|lines| lines.join("\n"))
            .unwrap_or_default()
    })
}

#[tokio::test]
async fn no_log_line_audit_row_or_stored_value_contains_a_raw_secret() {
    let scratch = oauth_scratch_or_skip!("wp2_secrets", Start::Complete);
    let h = Harness::new(scratch).await;
    start_capture();
    let person = h.member();

    // The state is the client's public correlation value, not a secret: it is stored as given and
    // so appears in the database statement spans. Everything else listed here is a secret.
    let mut secrets: Vec<String> = vec![VERIFIER.to_owned()];
    let consent = h.consent_for(&person, &authorize_body()).await;
    secrets.push(consent.consent_token.clone());
    let reply = h
        .decide(
            Some(&person),
            &consent.request_id,
            Some(&consent.consent_token),
            &[],
            &h.allow_body(&[]),
        )
        .await;
    let code = query_of(reply.data()["redirect_to"].as_str().unwrap())["code"].clone();
    secrets.push(code.clone());
    // A failed redemption, a good one, a refresh, a replayed code, a reused refresh token.
    h.token(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", REDIRECT),
        ("client_id", "ramflux"),
        ("code_verifier", &"w".repeat(50)),
        ("resource", MCP_RES),
    ])
    .await
    .assert_oauth_error(400, "invalid_grant");
    let issued = h.redeem(&code, MCP_RES).await.json();
    let refresh = issued["refresh_token"].as_str().unwrap().to_owned();
    secrets.push(issued["access_token"].as_str().unwrap().to_owned());
    secrets.push(refresh.clone());
    let second = h.refresh(&refresh, API_RES).await.json();
    secrets.push(second["access_token"].as_str().unwrap().to_owned());
    secrets.push(second["refresh_token"].as_str().unwrap().to_owned());
    h.redeem(&code, MCP_RES).await.assert_oauth_error(400, "invalid_grant");
    h.refresh(&refresh, API_RES)
        .await
        .assert_oauth_error(400, "invalid_grant");
    h.revoke(&[
        ("token", second["refresh_token"].as_str().unwrap()),
        ("client_id", "ramflux"),
    ])
    .await;
    h.token(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", &format!("svr_{}", "1".repeat(64))),
        ("client_id", "ramflux"),
        ("resource", API_RES),
    ])
    .await;

    for secret in &secrets {
        assert!(secret.len() >= 20);
    }
    let logs = captured_text();
    let grant_id: String = {
        #[derive(FromQueryResult)]
        struct Row {
            id: Uuid,
        }
        Row::find_by_statement(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id FROM oauth_grants".to_owned(),
        ))
        .one(h.db())
        .await
        .expect("query")
        .expect("one grant")
        .id
        .to_string()
    };
    assert!(
        logs.lines()
            .any(|line| line.contains("authorization code replay") && line.contains(&grant_id)),
        "the replay warning, with the grant id, was captured ({} lines)",
        logs.lines().count()
    );
    assert!(
        logs.lines()
            .any(|line| line.contains("refresh token reuse detected") && line.contains(&grant_id)),
        "the reuse warning was captured"
    );
    assert!(logs.lines().count() > 20, "database activity was captured too");
    for secret in &secrets {
        assert!(
            !logs.contains(secret.as_str()),
            "a log line contains a secret starting {}: {:?}",
            &secret[..4],
            logs.lines()
                .find(|line| line.contains(secret.as_str()))
                .map(|l| l.chars().take(300).collect::<String>())
        );
    }
    for table in [
        "oauth_audit_events",
        "oauth_tokens",
        "oauth_authorization_codes",
        "oauth_authorization_requests",
        "oauth_grants",
        "oauth_rate_hits",
    ] {
        for secret in &secrets {
            assert_eq!(
                count(
                    h.db(),
                    &format!("SELECT count(*) AS n FROM {table} t WHERE t::text LIKE '%' || $1 || '%'"),
                    vec![secret.clone().into()]
                )
                .await,
                0,
                "{table} contains a raw secret"
            );
        }
    }
    // Audit rows hold ids and reason codes only: not even the state.
    assert_eq!(
        count(
            h.db(),
            "SELECT count(*) AS n FROM oauth_audit_events t WHERE t::text LIKE '%' || $1 || '%'",
            vec![STATE.into()]
        )
        .await,
        0
    );
    assert!(h.audit_count("token_rejected").await >= 2);
    assert_eq!(h.audit_count("code_replay").await, 1);
    assert_eq!(h.audit_count("refresh_reuse_detected").await, 1);
    h.finish().await;
}
