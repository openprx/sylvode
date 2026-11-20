//! The authorization request and the consent decision (contract sections 6.1, 11.3, 11.4).
//!
//! * `POST /api/v1/oauth/authorization-requests` validates what the consent page received in the
//!   browser's address bar, stores it as a short-lived request and tells the page what to show:
//!   an error page (nothing is ever redirected to an unvalidated client or redirect URI), a
//!   redirect back to the client with a standard error, a sign-in prompt, or the consent data.
//! * `POST /api/v1/oauth/authorization-requests/{id}/decision` records the user's decision. It
//!   needs a first-party session and the single-use consent token, refuses cross-site requests,
//!   and takes client, redirect URI, PKCE challenge and resources from the stored request, never
//!   from the decision body, so a decision can only complete the request it was shown for.
//!
//! Both answer HTTP 200 with the API envelope on success and a real status code on failure; the
//! failure body carries the envelope fields and the OAuth `error` fields side by side.

use std::net::IpAddr;
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use platform::app::AppState;
use platform::auth::JwtManager;
use platform::config::OAuthConfig;
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, FromQueryResult, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::audit::{self, AuditEvent, AuditRecord, DetailValue};
use super::error::{OAuthError, no_store_headers};
use super::grants::{Actor, RevokeReason, revoke_grant};
use super::rate::{self, RateBucket, RateKey};
use super::redirect::{display_host, matches_registered, with_query};
use super::revoke;
use super::scopes::{Scope, ScopeSet, TokenResource};
use super::token::{
    self, ClientRow, db_error, is_valid_code_challenge, json_response, load_active_client, load_active_client_shared,
    not_found, rate_error, read_request, refuse_query, resource_of, user_agent,
};
use super::tokens::{Lifetimes, TokenKind, hash_secret, is_well_formed, mint, secrets_equal};
use crate::routes::auth::{extract_bearer_token, extract_cookie_token};

pub const REQUESTS_PATH: &str = "/api/v1/oauth/authorization-requests";
pub const DECISION_PATH: &str = "/api/v1/oauth/authorization-requests/{request_id}/decision";
/// Header carrying the single-use consent token.
pub const CONSENT_HEADER: &str = "x-sylvode-consent";

/// How long a stored authorization request can be decided.
const REQUEST_LIFETIME_SECS: f64 = 600.0;
const MAX_STATE_LEN: usize = 512;
const MIN_STATE_LEN: usize = 16;
const MAX_DEVICE_NAME_CHARS: usize = 64;
const MAX_SELECTED_PROJECTS: usize = 500;

// ---------------------------------------------------------------------------------------------
// Errors of these two endpoints
// ---------------------------------------------------------------------------------------------

/// A failure of the consent page's two calls: a real status code, the envelope fields and the
/// OAuth `error` fields. Messages are static strings.
#[derive(Debug)]
struct FlowError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retry_after_secs: Option<u64>,
    details: Option<Value>,
}

impl FlowError {
    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retry_after_secs: None,
            details: None,
        }
    }

    const fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    const fn forbidden(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }
}

impl From<OAuthError> for FlowError {
    fn from(error: OAuthError) -> Self {
        let code = error.kind.as_str();
        Self {
            status: error.kind.status(),
            code,
            message: error.description,
            retry_after_secs: error.retry_after_secs,
            details: None,
        }
    }
}

impl From<DbErr> for FlowError {
    fn from(error: DbErr) -> Self {
        db_error(&error).into()
    }
}

impl IntoResponse for FlowError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "code": self.status.as_u16(),
            "message": self.message,
            "error_code": self.code,
            "error": self.code,
            "error_description": self.message,
        });
        if let (Some(details), Some(map)) = (self.details, body.as_object_mut()) {
            map.insert("details".to_owned(), details);
        }
        let mut response = (self.status, Json(body)).into_response();
        let headers = response.headers_mut();
        for (name, value) in no_store_headers() {
            headers.insert(name, value);
        }
        if let Some(seconds) = self.retry_after_secs
            && let Ok(value) = header::HeaderValue::from_str(&seconds.to_string())
        {
            headers.insert(header::RETRY_AFTER, value);
        }
        response
    }
}

fn success(data: &Value) -> Response {
    json_response(&json!({"code": 0, "message": "success", "data": data}))
}

// ---------------------------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Session {
    user_id: Uuid,
    name: String,
    email: String,
    version: i64,
}

enum SessionState {
    /// No usable session: nothing presented, or an expired or invalid one.
    Anonymous,
    /// A bot or delegated token was presented: these endpoints are for people.
    NotAPerson,
    Person(Session),
}

#[derive(Debug, FromQueryResult)]
struct UserRow {
    name: String,
    email: String,
    is_active: bool,
    entity_type: String,
    session_version: i64,
}

async fn resolve_session(state: &AppState, headers: &HeaderMap) -> Result<SessionState, FlowError> {
    let presented = extract_bearer_token(headers).or_else(|| extract_cookie_token(headers, "access_token"));
    let Some(token) = presented else {
        return Ok(SessionState::Anonymous);
    };
    if token.starts_with("opr_") || token.starts_with("sva_") {
        return Ok(SessionState::NotAPerson);
    }
    let jwt = JwtManager::new(
        state.cfg.jwt_secret.expose(),
        state.cfg.jwt_access_ttl_seconds,
        state.cfg.jwt_refresh_ttl_seconds,
    );
    let Ok(claims) = jwt.verify_access_token(&token) else {
        return Ok(SessionState::Anonymous);
    };
    let Ok(user_id) = Uuid::parse_str(&claims.sub) else {
        return Ok(SessionState::Anonymous);
    };
    let row = UserRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT name, email, is_active, entity_type, session_version FROM users WHERE id = $1",
        vec![user_id.into()],
    ))
    .one(&state.db)
    .await?;
    Ok(match row {
        Some(row) if row.is_active && row.entity_type == "human" && row.session_version == claims.sv => {
            SessionState::Person(Session {
                user_id,
                name: row.name,
                email: row.email,
                version: row.session_version,
            })
        }
        _ => SessionState::Anonymous,
    })
}

// ---------------------------------------------------------------------------------------------
// Request validation (contract section 6.1 step 3)
// ---------------------------------------------------------------------------------------------

/// A `resource` parameter given once or several times.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ResourceParam {
    One(String),
    Many(Vec<String>),
}

/// The authorization request parameters, as the consent page forwards them.
#[derive(Debug, Default, Deserialize)]
struct AuthorizeBody {
    request_id: Option<String>,
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    resource: Option<ResourceParam>,
    device_name: Option<String>,
    workspace_hint: Option<String>,
}

impl AuthorizeBody {
    fn resources(&self) -> Vec<&str> {
        match &self.resource {
            None => Vec::new(),
            Some(ResourceParam::One(one)) => vec![one.as_str()],
            Some(ResourceParam::Many(many)) => many.iter().map(String::as_str).collect(),
        }
    }
}

/// What a failed validation shows or does.
#[derive(Debug, PartialEq, Eq)]
enum Rejection {
    /// The page shows an error and the browser is not sent anywhere.
    ErrorPage { error: &'static str, check: &'static str },
    /// The browser is sent back to the (validated) redirect URI with a standard error.
    Redirect {
        error: &'static str,
        description: &'static str,
        check: &'static str,
        redirect_uri: String,
        state: Option<String>,
    },
}

#[derive(Debug)]
struct Validated {
    redirect_uri: String,
    state: String,
    code_challenge: String,
    scopes: ScopeSet,
    resources: Vec<TokenResource>,
    device_name: Option<String>,
    workspace_hint: Option<Uuid>,
}

fn count(n: usize) -> DetailValue {
    DetailValue::Count(i64::try_from(n).unwrap_or(i64::MAX))
}

fn is_visible_ascii(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

fn valid_state(state: &str) -> bool {
    (MIN_STATE_LEN..=MAX_STATE_LEN).contains(&state.len()) && is_visible_ascii(state)
}

/// Validates in the order of the contract and stops at the first failure.
fn validate(config: &OAuthConfig, client: &ClientRow, body: &AuthorizeBody) -> Result<Validated, Rejection> {
    // 1 and 2: client and redirect URI. Failures here are never redirected.
    let Some(redirect_uri) = body
        .redirect_uri
        .as_deref()
        .filter(|uri| matches_registered(&client.redirect_uris, uri))
    else {
        return Err(Rejection::ErrorPage {
            error: "redirect_uri_mismatch",
            check: "redirect_uri",
        });
    };
    // The state is echoed only when it is plain visible ASCII of bounded length: never anything
    // that was not validated, and never interpreted.
    let echo_state = body
        .state
        .as_deref()
        .filter(|state| state.len() <= MAX_STATE_LEN && is_visible_ascii(state))
        .map(str::to_owned);
    let redirect = |error, description, check| Rejection::Redirect {
        error,
        description,
        check,
        redirect_uri: redirect_uri.to_owned(),
        state: echo_state.clone(),
    };

    if body.response_type.as_deref() != Some("code") {
        return Err(redirect(
            "unsupported_response_type",
            "only the code response type is supported",
            "response_type",
        ));
    }
    let challenge = body.code_challenge.as_deref().unwrap_or_default();
    if !is_valid_code_challenge(challenge) {
        return Err(redirect(
            "invalid_request",
            "a valid code_challenge is required",
            "code_challenge",
        ));
    }
    if body.code_challenge_method.as_deref() != Some("S256") {
        return Err(redirect(
            "invalid_request",
            "code_challenge_method must be S256",
            "code_challenge_method",
        ));
    }
    let Some(state) = body.state.as_deref().filter(|state| valid_state(state)) else {
        return Err(redirect("invalid_request", "a valid state is required", "state"));
    };

    let mut resources: Vec<TokenResource> = Vec::new();
    for url in body.resources() {
        let known = resource_of(config, url).filter(|resource| {
            client
                .allowed_resources
                .iter()
                .any(|allowed| allowed == resource.as_str())
        });
        match known {
            Some(resource) if !resources.contains(&resource) => resources.push(resource),
            _ => return Err(redirect("invalid_target", "the resource is not valid", "resource")),
        }
    }
    if resources.is_empty() {
        return Err(redirect("invalid_target", "a resource is required", "resource"));
    }

    let scopes = body
        .scope
        .as_deref()
        .and_then(|text| ScopeSet::parse(text).ok())
        .filter(|scopes| {
            let allowed = ScopeSet::from_scopes(
                &client
                    .allowed_scopes
                    .iter()
                    .filter_map(|name| Scope::from_name(name))
                    .collect::<Vec<_>>(),
            );
            scopes.is_subset_of(allowed)
        });
    let Some(scopes) = scopes else {
        return Err(redirect("invalid_scope", "the scope is not valid", "scope"));
    };

    let device_name = match body.device_name.as_deref() {
        None => None,
        Some(name)
            if (1..=MAX_DEVICE_NAME_CHARS).contains(&name.chars().count()) && !name.chars().any(char::is_control) =>
        {
            Some(name.to_owned())
        }
        Some(_) => {
            return Err(redirect(
                "invalid_request",
                "the device name is not valid",
                "device_name",
            ));
        }
    };
    let workspace_hint = match body.workspace_hint.as_deref() {
        None => None,
        Some(text) => match Uuid::parse_str(text) {
            Ok(id) => Some(id),
            Err(_) => {
                return Err(redirect(
                    "invalid_request",
                    "the workspace hint is not valid",
                    "workspace_hint",
                ));
            }
        },
    };
    Ok(Validated {
        redirect_uri: redirect_uri.to_owned(),
        state: state.to_owned(),
        code_challenge: challenge.to_owned(),
        scopes,
        resources,
        device_name,
        workspace_hint,
    })
}

fn error_redirect(config: &OAuthConfig, rejection: &Rejection) -> Option<String> {
    match rejection {
        Rejection::ErrorPage { .. } => None,
        Rejection::Redirect {
            error,
            description,
            redirect_uri,
            state,
            ..
        } => Some(redirect_with_error(
            config,
            redirect_uri,
            error,
            description,
            state.as_deref(),
        )),
    }
}

fn redirect_with_error(
    config: &OAuthConfig,
    redirect_uri: &str,
    error: &str,
    description: &str,
    state: Option<&str>,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![("error", error), ("error_description", description)];
    if let Some(state) = state {
        params.push(("state", state));
    }
    params.push(("iss", config.issuer.as_str()));
    with_query(redirect_uri, &params)
}

// ---------------------------------------------------------------------------------------------
// Stored requests
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct StoredRequest {
    id: Uuid,
    client_id: String,
    redirect_uri: String,
    state: String,
    code_challenge: String,
    requested_scopes: Vec<String>,
    resources: Vec<String>,
    device_name: Option<String>,
    workspace_hint: Option<Uuid>,
    consent_token_hash: Option<String>,
    consent_user_id: Option<Uuid>,
    status: String,
    live: bool,
}

const STORED_COLUMNS: &str = "id, client_id, redirect_uri, state, code_challenge, requested_scopes, resources, \
     device_name, workspace_hint, consent_token_hash, consent_user_id, status, (expires_at > now()) AS live";

impl StoredRequest {
    fn scope_set(&self) -> ScopeSet {
        ScopeSet::from_names(self.requested_scopes.iter().map(String::as_str)).unwrap_or(ScopeSet::EMPTY)
    }

    fn resource_list(&self) -> Vec<TokenResource> {
        self.resources
            .iter()
            .filter_map(|name| TokenResource::from_name(name))
            .collect()
    }
}

async fn load_stored<C: ConnectionTrait>(conn: &C, id: Uuid, lock: bool) -> Result<Option<StoredRequest>, DbErr> {
    let sql = format!(
        "SELECT {STORED_COLUMNS} FROM oauth_authorization_requests WHERE id = $1{}",
        if lock { " FOR UPDATE" } else { "" }
    );
    StoredRequest::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![id.into()],
    ))
    .one(conn)
    .await
}

struct Origin<'a> {
    ip: IpAddr,
    user_agent: Option<&'a str>,
}

impl Origin<'_> {
    fn record(&self, event: AuditEvent) -> AuditRecord {
        let record = AuditRecord::new(event).with_client_ip(self.ip);
        match self.user_agent {
            Some(agent) => record.with_user_agent(agent),
            None => record,
        }
    }
}

async fn write_audit<C: ConnectionTrait>(conn: &C, record: &AuditRecord) -> Result<(), FlowError> {
    audit::write(conn, record).await.map_err(|error| {
        tracing::error!(error = %error, "oauth audit write failed");
        FlowError::from(OAuthError::server_error())
    })
}

// ---------------------------------------------------------------------------------------------
// POST /api/v1/oauth/authorization-requests
// ---------------------------------------------------------------------------------------------

fn require_json(headers: &HeaderMap) -> Result<(), FlowError> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"));
    if is_json {
        Ok(())
    } else {
        Err(FlowError::bad_request(
            "invalid_request",
            "the content type must be application/json",
        ))
    }
}

fn error_page(error: &'static str) -> Response {
    success(&json!({"kind": "error_page", "error": error}))
}

async fn create_request(
    State(state): State<AppState>,
    Extension(config): Extension<Arc<OAuthConfig>>,
    req: Request,
) -> Response {
    match handle_create(&state, &config, req).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn handle_create(state: &AppState, config: &OAuthConfig, req: Request) -> Result<Response, FlowError> {
    let incoming = read_request(req).await?;
    let ip = rate::client_address(&incoming.headers, incoming.peer);
    rate::enforce(&state.db, RateBucket::Authorize, &RateKey::from_ip(ip))
        .await
        .map_err(rate_error)?;
    refuse_query(&incoming)?;
    require_json(&incoming.headers)?;
    let body: AuthorizeBody = serde_json::from_slice(&incoming.body)
        .map_err(|_| FlowError::bad_request("invalid_request", "the request body is not valid"))?;
    let agent = user_agent(&incoming.headers);
    let origin = Origin {
        ip,
        user_agent: agent.as_deref(),
    };
    let session = match resolve_session(state, &incoming.headers).await? {
        SessionState::NotAPerson => {
            return Err(FlowError::forbidden(
                "forbidden",
                "this endpoint is for signed-in people only",
            ));
        }
        SessionState::Anonymous => None,
        SessionState::Person(session) => Some(session),
    };

    let (request_id, client, stored) = match &body.request_id {
        Some(text) => match reload(state, text).await? {
            Ok(found) => found,
            Err(page) => return Ok(page),
        },
        None => match start(state, config, &origin, &body).await? {
            Ok(found) => found,
            Err(page) => return Ok(page),
        },
    };
    let redirect_host = display_host(&stored.redirect_uri).unwrap_or_default();
    let Some(session) = session else {
        return Ok(success(&json!({
            "kind": "login_required",
            "request_id": request_id,
            "client": {
                "client_id": client.client_id,
                "display_name": client.display_name,
                "redirect_host": redirect_host,
            },
        })));
    };

    let consent = token::mint_or_fail(mint(TokenKind::Consent))?;
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE oauth_authorization_requests SET consent_token_hash = $2, consent_user_id = $3 \
             WHERE id = $1 AND status = 'pending'",
            vec![request_id.into(), consent.hash.as_hex().into(), session.user_id.into()],
        ))
        .await?;
    let workspaces = load_workspaces(&state.db, session.user_id).await?;
    let eligible: Vec<Uuid> = workspaces.iter().filter(|w| w.allowed).map(|w| w.id).collect();
    let preselected = stored
        .workspace_hint
        .filter(|hint| eligible.contains(hint))
        .or(match eligible.as_slice() {
            [only] => Some(*only),
            _ => None,
        });
    let active = count_active_grants(&state.db, session.user_id, &client.client_id).await?;
    let requested = stored.scope_set();
    let scopes: Vec<Value> = requested
        .iter()
        .map(|scope| json!({"name": scope.as_str(), "required": !scope.optional_on_consent()}))
        .collect();
    Ok(success(&json!({
        "kind": "consent",
        "request_id": request_id,
        "consent_token": consent.raw.expose(),
        "client": {
            "client_id": client.client_id,
            "display_name": client.display_name,
            "description": client.description,
            "redirect_host": redirect_host,
            "device_name": stored.device_name,
        },
        "account": {"user_id": session.user_id, "name": session.name, "email": session.email},
        "workspaces": workspaces.iter().map(|w| json!({
            "id": w.id,
            "name": w.name,
            "eligible": w.allowed,
            "reason": if w.allowed { Value::Null } else { Value::String("workspace_disallows".to_owned()) },
        })).collect::<Vec<_>>(),
        "preselected_workspace_id": preselected,
        "scopes": scopes,
        "resources": stored.resources,
        "active_grant_count": active,
        "max_active_grants": config.max_active_grants_per_user_client,
    })))
}

/// A stored request ready to answer for, or the response that ends the call early.
type Staged = Result<(Uuid, ClientRow, StoredRequest), Response>;

/// Loads a stored request by id for a page reload.
async fn reload(state: &AppState, text: &str) -> Result<Staged, FlowError> {
    let Ok(id) = Uuid::parse_str(text) else {
        return Ok(Err(error_page("request_expired")));
    };
    let Some(stored) = load_stored(&state.db, id, false)
        .await?
        .filter(|s| s.live && s.status == "pending")
    else {
        return Ok(Err(error_page("request_expired")));
    };
    let Some(client) = load_active_client(&state.db, &stored.client_id).await? else {
        return Ok(Err(error_page("unknown_client")));
    };
    Ok(Ok((stored.id, client, stored)))
}

/// Validates new parameters and stores the request. `Err` is the response to send.
async fn start(
    state: &AppState,
    config: &OAuthConfig,
    origin: &Origin<'_>,
    body: &AuthorizeBody,
) -> Result<Staged, FlowError> {
    let client = match body.client_id.as_deref() {
        Some(client_id) => load_active_client(&state.db, client_id).await?,
        None => None,
    };
    let Some(client) = client else {
        write_audit(
            &state.db,
            &origin
                .record(AuditEvent::AuthorizeRejected)
                .with_detail("check", DetailValue::Code("client_id")),
        )
        .await?;
        return Ok(Err(error_page("unknown_client")));
    };
    match validate(config, &client, body) {
        Err(rejection) => {
            let (check, page) = match &rejection {
                Rejection::ErrorPage { error, check } => (*check, Some(*error)),
                Rejection::Redirect { check, .. } => (*check, None),
            };
            write_audit(
                &state.db,
                &origin
                    .record(AuditEvent::AuthorizeRejected)
                    .with_client(&client.client_id)
                    .with_detail("check", DetailValue::Code(check)),
            )
            .await?;
            Ok(Err(match (page, error_redirect(config, &rejection)) {
                (Some(error), _) => error_page(error),
                (None, Some(redirect_to)) => success(&json!({"kind": "redirect", "redirect_to": redirect_to})),
                (None, None) => error_page("unknown_client"),
            }))
        }
        Ok(valid) => {
            let id = Uuid::new_v4();
            let tx = state.db.begin().await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO oauth_authorization_requests \
                 (id, client_id, redirect_uri, state, code_challenge, requested_scopes, resources, device_name, \
                  workspace_hint, expires_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now() + make_interval(secs => $10))",
                vec![
                    id.into(),
                    client.client_id.clone().into(),
                    valid.redirect_uri.clone().into(),
                    valid.state.clone().into(),
                    valid.code_challenge.clone().into(),
                    valid.scopes.to_names().into(),
                    valid
                        .resources
                        .iter()
                        .map(|r| r.as_str().to_owned())
                        .collect::<Vec<_>>()
                        .into(),
                    valid.device_name.clone().into(),
                    valid.workspace_hint.into(),
                    REQUEST_LIFETIME_SECS.into(),
                ],
            ))
            .await?;
            write_audit(
                &tx,
                &origin
                    .record(AuditEvent::AuthorizeRequested)
                    .with_client(&client.client_id)
                    .with_detail("scopes", count(valid.scopes.iter().count())),
            )
            .await?;
            tx.commit().await?;
            let stored = load_stored(&state.db, id, false)
                .await?
                .ok_or_else(|| FlowError::from(OAuthError::server_error()))?;
            Ok(Ok((id, client, stored)))
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
    allowed: bool,
}

async fn load_workspaces<C: ConnectionTrait>(conn: &C, user_id: Uuid) -> Result<Vec<WorkspaceRow>, DbErr> {
    WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT w.id, w.name, COALESCE(s.allow_connected_apps, true) AS allowed \
         FROM workspace_members m \
         JOIN workspaces w ON w.id = m.workspace_id \
         LEFT JOIN workspace_delegation_settings s ON s.workspace_id = w.id \
         WHERE m.user_id = $1 ORDER BY w.name, w.id",
        vec![user_id.into()],
    ))
    .all(conn)
    .await
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    n: i64,
}

async fn count_active_grants<C: ConnectionTrait>(conn: &C, user_id: Uuid, client_id: &str) -> Result<i64, DbErr> {
    Ok(CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS n FROM oauth_grants \
         WHERE user_id = $1 AND client_id = $2 AND status = 'active' AND expires_at > now()",
        vec![user_id.into(), client_id.into()],
    ))
    .one(conn)
    .await?
    .map_or(0, |row| row.n))
}

// ---------------------------------------------------------------------------------------------
// POST /api/v1/oauth/authorization-requests/{id}/decision
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionBody {
    decision: String,
    workspace_id: Option<Uuid>,
    project_mode: Option<String>,
    project_ids: Option<Vec<Uuid>>,
    scopes: Option<Vec<String>>,
}

/// Cross-site request refusal: `Origin`, when present, must be the issuer and `Sec-Fetch-Site`,
/// when present, must be `same-origin`.
fn check_same_origin(headers: &HeaderMap, issuer: &str) -> Result<(), FlowError> {
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(issuer)
    {
        return Err(FlowError::forbidden(
            "csrf_origin_mismatch",
            "the request origin is not allowed",
        ));
    }
    if let Some(site) = headers.get("sec-fetch-site")
        && site.to_str().ok() != Some("same-origin")
    {
        return Err(FlowError::forbidden(
            "csrf_origin_mismatch",
            "the request origin is not allowed",
        ));
    }
    Ok(())
}

/// A failure inside the decision transaction and whether its writes are kept.
struct TxFail {
    error: FlowError,
    commit: bool,
}

impl From<OAuthError> for TxFail {
    fn from(error: OAuthError) -> Self {
        FlowError::from(error).into()
    }
}

impl From<FlowError> for TxFail {
    fn from(error: FlowError) -> Self {
        Self { error, commit: false }
    }
}

impl From<DbErr> for TxFail {
    fn from(error: DbErr) -> Self {
        FlowError::from(error).into()
    }
}

async fn decide(
    State(state): State<AppState>,
    Extension(config): Extension<Arc<OAuthConfig>>,
    Path(request_id): Path<String>,
    req: Request,
) -> Response {
    match handle_decide(&state, &config, &request_id, req).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

const fn consent_failure() -> FlowError {
    FlowError::forbidden("consent_token_invalid", "the consent token is missing or not valid")
}

async fn handle_decide(
    state: &AppState,
    config: &OAuthConfig,
    request_id: &str,
    req: Request,
) -> Result<Response, FlowError> {
    let incoming = read_request(req).await?;
    check_same_origin(&incoming.headers, &config.issuer)?;
    refuse_query(&incoming)?;
    require_json(&incoming.headers)?;
    let ip = rate::client_address(&incoming.headers, incoming.peer);
    let session = match resolve_session(state, &incoming.headers).await? {
        SessionState::NotAPerson => {
            return Err(FlowError::forbidden(
                "forbidden",
                "this endpoint is for signed-in people only",
            ));
        }
        SessionState::Anonymous => None,
        SessionState::Person(session) => Some(session),
    };
    let key = session
        .as_ref()
        .map_or_else(|| RateKey::from_ip(ip), |session| RateKey::from_id(session.user_id));
    rate::enforce(&state.db, RateBucket::Decision, &key)
        .await
        .map_err(rate_error)?;
    let body: DecisionBody = serde_json::from_slice(&incoming.body)
        .map_err(|_| FlowError::bad_request("invalid_request", "the request body is not valid"))?;
    let request_id = Uuid::parse_str(request_id)
        .map_err(|_| FlowError::new(StatusCode::NOT_FOUND, "request_not_found", "the request does not exist"))?;
    let agent = user_agent(&incoming.headers);
    let origin = Origin {
        ip,
        user_agent: agent.as_deref(),
    };
    let consent = incoming
        .headers
        .get(CONSENT_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let tx = state.db.begin().await?;
    let result = match body.decision.as_str() {
        "no_account" => decide_no_account(&tx, config, &origin, request_id, session.as_ref()).await,
        "deny" | "allow" => match session {
            None => Err(FlowError::new(StatusCode::UNAUTHORIZED, "session_required", "sign in to continue").into()),
            Some(session) => {
                decide_signed_in(
                    &tx,
                    state,
                    config,
                    &origin,
                    request_id,
                    &session,
                    consent.as_deref(),
                    &body,
                )
                .await
            }
        },
        _ => Err(FlowError::bad_request("invalid_request", "the decision is not valid").into()),
    };
    match result {
        Ok(redirect_to) => {
            tx.commit().await?;
            Ok(success(&json!({"redirect_to": redirect_to})))
        }
        Err(fail) => {
            if fail.commit {
                tx.commit().await?;
            } else if let Err(error) = tx.rollback().await {
                tracing::warn!(error = %error, "oauth transaction rollback failed");
            }
            Err(fail.error)
        }
    }
}

/// Locks the stored request and requires it to be pending and unexpired.
async fn lock_pending(tx: &DatabaseTransaction, request_id: Uuid) -> Result<StoredRequest, TxFail> {
    let stored = load_stored(tx, request_id, true)
        .await?
        .ok_or_else(|| FlowError::new(StatusCode::NOT_FOUND, "request_not_found", "the request does not exist"))?;
    if stored.status != "pending" {
        return Err(FlowError::new(
            StatusCode::CONFLICT,
            "already_decided",
            "the request was already decided",
        )
        .into());
    }
    if !stored.live {
        return Err(FlowError::new(StatusCode::NOT_FOUND, "request_not_found", "the request does not exist").into());
    }
    Ok(stored)
}

async fn mark_decided(tx: &DatabaseTransaction, request_id: Uuid) -> Result<(), DbErr> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE oauth_authorization_requests SET status = 'decided', consent_token_hash = NULL WHERE id = $1",
        vec![request_id.into()],
    ))
    .await?;
    Ok(())
}

async fn decide_no_account(
    tx: &DatabaseTransaction,
    config: &OAuthConfig,
    origin: &Origin<'_>,
    request_id: Uuid,
    session: Option<&Session>,
) -> Result<String, TxFail> {
    if session.is_some() {
        return Err(FlowError::bad_request("invalid_request", "a signed-in user has an account").into());
    }
    let stored = lock_pending(tx, request_id).await?;
    mark_decided(tx, request_id).await?;
    write_audit(
        tx,
        &origin
            .record(AuditEvent::AuthorizeNoAccount)
            .with_client(&stored.client_id),
    )
    .await?;
    Ok(redirect_with_error(
        config,
        &stored.redirect_uri,
        "access_denied",
        "account_required",
        Some(&stored.state),
    ))
}

/// The consent header must hash to the stored value and the request must be the one shown to
/// this user.
fn consent_matches(stored: &StoredRequest, presented: Option<&str>, session: &Session) -> bool {
    let (Some(presented), Some(expected), Some(user)) = (
        presented.filter(|text| is_well_formed(TokenKind::Consent, text)),
        stored.consent_token_hash.as_deref(),
        stored.consent_user_id,
    ) else {
        return false;
    };
    let matches_token = secrets_equal(hash_secret(presented).as_hex(), expected);
    matches_token && user == session.user_id
}

#[allow(clippy::too_many_arguments, reason = "one decision needs the whole request context")]
async fn decide_signed_in(
    tx: &DatabaseTransaction,
    state: &AppState,
    config: &OAuthConfig,
    origin: &Origin<'_>,
    request_id: Uuid,
    session: &Session,
    consent: Option<&str>,
    body: &DecisionBody,
) -> Result<String, TxFail> {
    let stored = lock_pending(tx, request_id).await?;
    if !consent_matches(&stored, consent, session) {
        // Nothing is consumed: the right token can still decide the request. The refusal is
        // recorded outside the transaction, which is rolled back.
        if let Err(error) = write_audit(
            &state.db,
            &origin
                .record(AuditEvent::AuthorizeRejected)
                .with_client(&stored.client_id)
                .with_user(session.user_id)
                .with_detail("check", DetailValue::Code("consent_token")),
        )
        .await
        {
            return Err(error.into());
        }
        return Err(consent_failure().into());
    }
    let client = load_active_client_shared(tx, &stored.client_id)
        .await?
        .ok_or_else(|| FlowError::forbidden("client_not_active", "the application is not available"))?;

    if body.decision == "deny" {
        let eligible = load_workspaces(tx, session.user_id)
            .await?
            .iter()
            .any(|workspace| workspace.allowed);
        let reason = if eligible {
            "consent_denied"
        } else {
            "no_eligible_workspace"
        };
        mark_decided(tx, request_id).await?;
        write_audit(
            tx,
            &origin
                .record(AuditEvent::ConsentDenied)
                .with_client(&client.client_id)
                .with_user(session.user_id)
                .with_detail("reason", DetailValue::Code(reason)),
        )
        .await?;
        return Ok(redirect_with_error(
            config,
            &stored.redirect_uri,
            "access_denied",
            reason,
            Some(&stored.state),
        ));
    }
    allow(tx, config, origin, &stored, &client, session, body).await
}

/// The selection the user made, checked against the stored request.
struct Selection {
    workspace_id: Uuid,
    all_projects: bool,
    project_ids: Vec<Uuid>,
    scopes: ScopeSet,
}

fn invalid_selection(message: &'static str) -> TxFail {
    FlowError::bad_request("invalid_selection", message).into()
}

fn parse_selection(stored: &StoredRequest, body: &DecisionBody) -> Result<Selection, TxFail> {
    let workspace_id = body
        .workspace_id
        .ok_or_else(|| invalid_selection("choose a workspace"))?;
    let (all_projects, project_ids) = match (body.project_mode.as_deref(), body.project_ids.as_deref()) {
        (Some("all"), None | Some([])) => (true, Vec::new()),
        (Some("all"), Some(_)) => return Err(invalid_selection("a project list needs the selected mode")),
        (Some("selected"), Some(ids)) if !ids.is_empty() && ids.len() <= MAX_SELECTED_PROJECTS => {
            let mut unique = ids.to_vec();
            unique.sort_unstable();
            unique.dedup();
            (false, unique)
        }
        (Some("selected"), _) => return Err(invalid_selection("choose at least one project")),
        _ => return Err(invalid_selection("choose which projects to share")),
    };
    let names = body.scopes.as_deref().unwrap_or_default();
    let scopes = ScopeSet::from_names(names.iter().map(String::as_str))
        .map_err(|_| invalid_selection("the scopes are not valid"))?;
    let requested = stored.scope_set();
    if !scopes.is_subset_of(requested) {
        return Err(invalid_selection("the scopes were not requested"));
    }
    let missing_required = requested
        .iter()
        .filter(|scope| !scope.optional_on_consent())
        .any(|scope| !scopes.contains(scope));
    if missing_required {
        return Err(invalid_selection("a required scope is missing"));
    }
    if stored
        .resource_list()
        .iter()
        .any(|resource| scopes.narrow_to_resource(*resource).is_empty())
    {
        return Err(invalid_selection("the scopes do not cover a requested resource"));
    }
    Ok(Selection {
        workspace_id,
        all_projects,
        project_ids,
        scopes,
    })
}

#[derive(Debug, FromQueryResult)]
struct MembershipRow {
    allowed: bool,
}

#[derive(Debug, FromQueryResult)]
struct IdRow {
    id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct VersionRow {
    session_version: i64,
}

#[derive(Debug, FromQueryResult)]
struct GrantSummary {
    id: Uuid,
    device_name: Option<String>,
    workspace_id: Uuid,
    consented_at: Timestamp,
}

async fn allow(
    tx: &DatabaseTransaction,
    config: &OAuthConfig,
    origin: &Origin<'_>,
    stored: &StoredRequest,
    client: &ClientRow,
    session: &Session,
    body: &DecisionBody,
) -> Result<String, TxFail> {
    let selection = parse_selection(stored, body)?;

    // The user's standing, now, from live data. The row is held so a password change cannot slip
    // between this check and the grant: its revocation hook runs after this transaction.
    let standing = VersionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT session_version FROM users WHERE id = $1 AND is_active AND entity_type = 'human' FOR SHARE",
        vec![session.user_id.into()],
    ))
    .one(tx)
    .await?;
    if standing.is_none_or(|row| row.session_version != session.version) {
        return Err(FlowError::new(StatusCode::UNAUTHORIZED, "session_required", "sign in to continue").into());
    }
    // The membership row and the workspace setting are share-locked until this transaction ends:
    // removing the member or switching connected applications off updates its row first and
    // sweeps the workspace's grants after, so it either waits for this consent and then revokes
    // the grant created here, or commits first and this consent finds it refused.
    // The workspace row comes first among these three: removing a member advances the
    // workspace's authorization epoch (a `FOR UPDATE` on this row) before it locks the member
    // row, and the grant insert below needs a key share of it.
    IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM workspaces WHERE id = $1 FOR KEY SHARE",
        vec![selection.workspace_id.into()],
    ))
    .one(tx)
    .await?;
    let member = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT user_id AS id FROM workspace_members WHERE workspace_id = $1 AND user_id = $2 FOR SHARE",
        vec![selection.workspace_id.into(), session.user_id.into()],
    ))
    .one(tx)
    .await?;
    if member.is_none() {
        return Err(FlowError::forbidden("workspace_not_eligible", "the workspace cannot be connected").into());
    }
    // No settings row means allowed, and there is nothing to lock then: write the default row
    // (what "no row" means) so that a concurrent switch-off has to wait for this transaction.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_delegation_settings (workspace_id) VALUES ($1) ON CONFLICT (workspace_id) DO NOTHING",
        vec![selection.workspace_id.into()],
    ))
    .await?;
    let setting = MembershipRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT allow_connected_apps AS allowed FROM workspace_delegation_settings \
         WHERE workspace_id = $1 FOR SHARE",
        vec![selection.workspace_id.into()],
    ))
    .one(tx)
    .await?;
    if !setting.is_some_and(|row| row.allowed) {
        return Err(FlowError::forbidden("workspace_not_eligible", "the workspace cannot be connected").into());
    }
    if !selection.all_projects {
        let found = IdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM projects WHERE workspace_id = $1 AND id = ANY($2)",
            vec![selection.workspace_id.into(), selection.project_ids.clone().into()],
        ))
        .all(tx)
        .await?;
        if found.len() != selection.project_ids.len() {
            return Err(invalid_selection("a project does not belong to the workspace"));
        }
    }

    // One decision at a time per user and client, so the grant limit cannot be passed by racing.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        vec![format!("oauth_grants:{}:{}", session.user_id, client.client_id).into()],
    ))
    .await?;
    sweep_unredeemed(tx, session.user_id, &client.client_id).await?;
    let active = GrantSummary::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, device_name, workspace_id, consented_at FROM oauth_grants \
         WHERE user_id = $1 AND client_id = $2 AND status = 'active' AND expires_at > now() \
         ORDER BY consented_at, id",
        vec![session.user_id.into(), client.client_id.clone().into()],
    ))
    .all(tx)
    .await?;
    if active.len() >= usize::try_from(config.max_active_grants_per_user_client).unwrap_or(usize::MAX) {
        let mut error = FlowError::new(
            StatusCode::CONFLICT,
            "too_many_grants",
            "disconnect an older connection first",
        );
        error.details = Some(json!({
            "max_active_grants": config.max_active_grants_per_user_client,
            "grants": active.iter().map(|grant| json!({
                "grant_id": grant.id,
                "device_name": grant.device_name,
                "workspace_id": grant.workspace_id,
                "consented_at": grant.consented_at,
            })).collect::<Vec<_>>(),
        }));
        // The sweep above is kept.
        return Err(TxFail { error, commit: true });
    }

    let life = Lifetimes::from_config(config);
    let now = Timestamp::now();
    let grant_id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO oauth_grants (id, client_id, user_id, workspace_id, scopes, resources, project_mode, \
         device_name, user_session_version, status, consented_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'active', $10, $11)",
        vec![
            grant_id.into(),
            client.client_id.clone().into(),
            session.user_id.into(),
            selection.workspace_id.into(),
            selection.scopes.to_names().into(),
            stored.resources.clone().into(),
            (if selection.all_projects { "all" } else { "selected" }).into(),
            stored.device_name.clone().into(),
            session.version.into(),
            now.into(),
            life.grant_expires_at(now).into(),
        ],
    ))
    .await?;
    if !selection.all_projects {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO oauth_grant_projects (grant_id, project_id) SELECT $1, unnest($2::uuid[])",
            vec![grant_id.into(), selection.project_ids.clone().into()],
        ))
        .await?;
    }
    let code = token::mint_or_fail(mint(TokenKind::AuthorizationCode))?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO oauth_authorization_codes (code_hash, grant_id, client_id, redirect_uri, code_challenge, \
         resources, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        vec![
            code.hash.as_hex().into(),
            grant_id.into(),
            client.client_id.clone().into(),
            stored.redirect_uri.clone().into(),
            stored.code_challenge.clone().into(),
            stored.resources.clone().into(),
            life.code_expires_at(now).into(),
        ],
    ))
    .await?;
    mark_decided(tx, stored.id).await?;
    let base = |event| {
        origin
            .record(event)
            .with_client(&client.client_id)
            .with_grant(grant_id)
            .with_user(session.user_id)
            .with_workspace(selection.workspace_id)
    };
    write_audit(
        tx,
        &base(AuditEvent::ConsentGranted)
            .with_detail(
                "project_mode",
                DetailValue::Code(if selection.all_projects { "all" } else { "selected" }),
            )
            .with_detail("projects", count(selection.project_ids.len())),
    )
    .await?;
    write_audit(tx, &base(AuditEvent::CodeIssued)).await?;
    tracing::info!(grant_id = %grant_id, "consent granted");
    Ok(with_query(
        &stored.redirect_uri,
        &[
            ("code", code.raw.expose()),
            ("state", &stored.state),
            ("iss", &config.issuer),
        ],
    ))
}

/// Ends, with reason `expired`, the grants of this user and client that never received a token
/// and whose code has run out, or whose own lifetime is over, so abandoned consents do not count
/// against the limit. The candidates are read without a lock and each is then ended through
/// `revoke_grant`, which takes the locks in the order the token endpoint takes them (codes,
/// tokens, grant); locking the grant rows here first would run against that order. A candidate
/// has no token and no live code, so nothing can start redeeming it in between.
async fn sweep_unredeemed(tx: &DatabaseTransaction, user_id: Uuid, client_id: &str) -> Result<(), DbErr> {
    let stale = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT g.id FROM oauth_grants g \
         WHERE g.user_id = $1 AND g.client_id = $2 AND g.status = 'active' \
           AND (g.expires_at <= now() OR ( \
                NOT EXISTS (SELECT 1 FROM oauth_tokens t WHERE t.grant_id = g.id) \
                AND NOT EXISTS (SELECT 1 FROM oauth_authorization_codes c \
                                WHERE c.grant_id = g.id AND c.expires_at > now() AND c.consumed_at IS NULL))) \
         ORDER BY g.id",
        vec![user_id.into(), client_id.into()],
    ))
    .all(tx)
    .await?;
    for IdRow { id } in stale {
        revoke_grant(tx, id, RevokeReason::Expired, Actor::System).await?;
    }
    Ok(())
}

/// Every route of this package: the two consent endpoints, the token endpoint and the revocation
/// endpoint. Without configuration all of them answer 404.
pub fn routes(config: Option<&Arc<OAuthConfig>>) -> Router<AppState> {
    let own = config.map_or_else(
        || {
            Router::new()
                .route(REQUESTS_PATH, any(not_found))
                .route(DECISION_PATH, any(not_found))
        },
        |config| {
            Router::new()
                .route(REQUESTS_PATH, post(create_request))
                .route(DECISION_PATH, post(decide))
                .layer(Extension(Arc::clone(config)))
        },
    );
    own.merge(token::routes(config.cloned())).merge(revoke::routes(config))
}

#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod tests;
