//! Management of connected applications and the revocation hooks (contract sections 6.5, 11.8
//! to 11.10).
//!
//! Three levels of endpoints, all for Sylvode's own signed-in people:
//!
//! * a person lists and disconnects their own connected applications,
//! * a workspace owner or admin lists and disconnects the connections that reach the workspace
//!   and switches connected applications on or off for it,
//! * an instance administrator lists the registered client and disables or re-enables it.
//!
//! None of them can create, extend or widen a grant. None accepts a bot token or a delegated
//! token, whatever its scopes: the [`Session`] extractor refuses both with 403 before any other
//! check. State-changing requests also refuse a cross-site origin.
//!
//! The second half of the module is the revocation hooks the handlers of other areas call inside
//! their own transaction (password change, account disabled, removal from a workspace). A hook
//! revokes with the caller's transaction, so the change and the revocation commit or roll back
//! together and the `sv_oauth_grant_changed` notification is delivered after that commit only.
//!
//! # Lock order
//!
//! A revocation first locks the unredeemed authorization codes and the live tokens of the grants
//! it ends, then the grant rows (inside `grants::revoke_*`). The token endpoint locks its code or
//! refresh token row first and touches the grant after, so both sides take their locks in the
//! same order and neither can wait for the other. Locking the codes and tokens also makes a
//! redemption that is in flight finish before the revocation reads the token rows, and one that
//! starts later find the grant revoked, so no token is issued after the revocation committed.
//! The consent decision locks the user row (`FOR SHARE`), then the user's grants; a hook that
//! changes the user row (password, `is_active`) holds that row exclusively before it locks
//! anything else, which is the same direction.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Extension, Router,
    body::Bytes,
    extract::{ConnectInfo, FromRequestParts, Path, State},
    http::{HeaderMap, Method, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use platform::app::AppState;
use platform::auth::JwtManager;
use platform::config::OAuthConfig;
use platform::timestamp::Timestamp;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::audit::{self, AuditEvent, AuditRecord, DetailValue};
use super::error::no_store_headers;
use super::grants::{self, Actor, RevokeReason};
use super::rate;
use super::token::{json_response, not_found, parse_form, user_agent};
use crate::error::{DbFailureClass, classify_db_failure};
use crate::routes::auth::{extract_bearer_token, extract_cookie_token};

pub const USER_APPS_PATH: &str = "/api/v1/me/connected-apps";
pub const USER_APP_PATH: &str = "/api/v1/me/connected-apps/{grant_id}";
pub const WORKSPACE_APPS_PATH: &str = "/api/v1/workspaces/{workspace_id}/connected-apps";
pub const WORKSPACE_APP_PATH: &str = "/api/v1/workspaces/{workspace_id}/connected-apps/{grant_id}";
pub const WORKSPACE_SETTINGS_PATH: &str = "/api/v1/workspaces/{workspace_id}/connected-apps/settings";
pub const ADMIN_CLIENTS_PATH: &str = "/api/v1/admin/oauth-clients";
pub const ADMIN_CLIENT_PATH: &str = "/api/v1/admin/oauth-clients/{client_id}";

const DEFAULT_PAGE_SIZE: i64 = 50;
const MAX_PAGE_SIZE: i64 = 100;
const MAX_OFFSET: i64 = 10_000;
/// Projects listed per grant; the full count is reported next to them.
const MAX_LISTED_PROJECTS: i64 = 100;
const MAX_CLIENTS_LISTED: i64 = 100;
const MAX_BODY_BYTES: usize = 1024;
const MAX_ATTEMPTS: u32 = 3;
const RETRY_PAUSE: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// A failed management call: a real status code and the API envelope fields. Messages are static
/// strings, so nothing a caller sent can come back in one.
#[derive(Debug)]
pub struct ManageError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    /// The database reported a failure a second attempt can get past (a deadlock, a lock timeout).
    transient: bool,
}

impl ManageError {
    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            transient: false,
        }
    }

    const fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "sign in to continue")
    }

    const fn forbidden(message: &'static str) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    const fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }

    const fn invalid(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    const fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal server error",
        )
    }

    #[cfg(test)]
    pub const fn status(&self) -> StatusCode {
        self.status
    }
}

impl From<DbErr> for ManageError {
    fn from(error: DbErr) -> Self {
        tracing::error!(error = %error, "oauth management database error");
        Self {
            transient: classify_db_failure(&error) == DbFailureClass::Transient,
            ..Self::internal()
        }
    }
}

impl IntoResponse for ManageError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            axum::Json(json!({
                "code": self.status.as_u16(),
                "message": self.message,
                "error_code": self.code,
            })),
        )
            .into_response();
        for (name, value) in no_store_headers() {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

fn success(data: &Value) -> Response {
    json_response(&json!({"code": 0, "message": "success", "data": data}))
}

// ---------------------------------------------------------------------------------------------
// Who is calling
// ---------------------------------------------------------------------------------------------

/// A signed-in person, established from the session cookie or a user JWT.
///
/// Extracting it is the whole authentication of the management endpoints. It refuses, in order:
/// a cross-site state-changing request, a bot token or delegated token in any place a session
/// token is read from (403), a missing or invalid session (401), and an inactive account, a
/// session that predates a password change, or an account that is not a person (401).
#[derive(Clone, Debug)]
pub struct Session {
    pub user_id: Uuid,
    /// `users.role`, normalised: the instance-level role, not a workspace role.
    instance_role: String,
    ip: IpAddr,
    user_agent: Option<String>,
}

impl Session {
    fn require_instance_admin(&self) -> Result<(), ManageError> {
        if matches!(self.instance_role.as_str(), "admin" | "owner") {
            Ok(())
        } else {
            Err(ManageError::forbidden("admin access required"))
        }
    }

    /// An audit record carrying the caller as actor, with the request's origin.
    fn record(&self, event: AuditEvent) -> AuditRecord {
        let record = AuditRecord::new(event).with_actor(self.user_id).with_client_ip(self.ip);
        match &self.user_agent {
            Some(agent) => record.with_user_agent(agent),
            None => record,
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct AccountRow {
    role: String,
    is_active: bool,
    entity_type: String,
    session_version: i64,
}

/// `Origin`, when present, must be the issuer, and `Sec-Fetch-Site`, when present, must be
/// `same-origin`. The same rule as the consent decision. The session cookie is `SameSite=Lax`, so
/// a browser does not attach it to a cross-site `DELETE`, `PUT` or `PATCH`; this refuses the
/// request outright when a browser says it came from elsewhere.
fn check_same_origin(headers: &HeaderMap, issuer: &str) -> Result<(), ManageError> {
    let refused = || {
        ManageError::new(
            StatusCode::FORBIDDEN,
            "csrf_origin_mismatch",
            "the request origin is not allowed",
        )
    };
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(issuer)
    {
        return Err(refused());
    }
    if let Some(site) = headers.get("sec-fetch-site")
        && site.to_str().ok() != Some("same-origin")
    {
        return Err(refused());
    }
    Ok(())
}

const fn is_state_changing(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn is_not_a_session_token(token: &str) -> bool {
    token.starts_with("opr_") || token.starts_with("sva_")
}

impl FromRequestParts<AppState> for Session {
    type Rejection = ManageError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let config = parts
            .extensions
            .get::<Arc<OAuthConfig>>()
            .cloned()
            .ok_or_else(ManageError::internal)?;
        if is_state_changing(&parts.method) {
            check_same_origin(&parts.headers, &config.issuer)?;
        }

        let bearer = extract_bearer_token(&parts.headers);
        let cookie = extract_cookie_token(&parts.headers, "access_token");
        // A bot or delegated token anywhere a session token could be read from is refused, so a
        // request can not carry one next to a valid session and be taken for the session.
        if bearer
            .iter()
            .chain(cookie.iter())
            .any(|token| is_not_a_session_token(token))
        {
            return Err(ManageError::forbidden("this endpoint is for signed-in people only"));
        }
        let token = bearer.or(cookie).ok_or_else(ManageError::unauthorized)?;

        let jwt = JwtManager::new(
            state.cfg.jwt_secret.expose(),
            state.cfg.jwt_access_ttl_seconds,
            state.cfg.jwt_refresh_ttl_seconds,
        );
        let claims = jwt
            .verify_access_token(&token)
            .map_err(|_| ManageError::unauthorized())?;
        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ManageError::unauthorized())?;
        let account = AccountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role, is_active, entity_type, session_version FROM users WHERE id = $1",
            vec![user_id.into()],
        ))
        .one(&state.db)
        .await?;
        let account = account
            .filter(|row| row.is_active && row.entity_type == "human" && row.session_version == claims.sv)
            .ok_or_else(ManageError::unauthorized)?;

        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
        Ok(Self {
            user_id,
            instance_role: account.role.trim().to_lowercase(),
            ip: rate::client_address(&parts.headers, peer),
            user_agent: user_agent(&parts.headers),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------------------------

/// Runs `work` in a transaction: committed on success, rolled back on any error.
async fn in_tx<T>(
    db: &DatabaseConnection,
    work: impl AsyncFnOnce(&DatabaseTransaction) -> Result<T, ManageError>,
) -> Result<T, ManageError> {
    let tx = db.begin().await?;
    match work(&tx).await {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            if let Err(rollback) = tx.rollback().await {
                tracing::warn!(error = %rollback, "oauth management rollback failed");
            }
            Err(error)
        }
    }
}

/// Runs a whole transaction again when the database aborted it as a deadlock victim or a lock
/// timeout. Every transaction run through here is safe to repeat: a revocation is idempotent and
/// a setting is written, not incremented.
async fn retrying<T, F, Fut>(mut attempt: F) -> Result<T, ManageError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ManageError>>,
{
    let mut tries = 1;
    loop {
        match attempt().await {
            Err(error) if error.transient && tries < MAX_ATTEMPTS => {
                tries += 1;
                tokio::time::sleep(RETRY_PAUSE).await;
            }
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Revocation with the lock order of the module comment
// ---------------------------------------------------------------------------------------------

/// What a revocation reaches.
enum Target<'a> {
    Grant(Uuid),
    User(Uuid),
    UserWorkspace(Uuid, Uuid),
    Workspace(Uuid),
    Client(&'a str),
}

/// The statements that lock the unredeemed codes and the live tokens of the active grants a
/// filter selects, in a stable order. The filter is a literal chosen below, never built from input.
macro_rules! lock_statements {
    ($filter:literal) => {
        (
            concat!(
                "SELECT c.code_hash FROM oauth_authorization_codes c WHERE c.consumed_at IS NULL AND c.grant_id IN \
                 (SELECT g.id FROM oauth_grants g WHERE g.status = 'active' AND ",
                $filter,
                ") ORDER BY c.code_hash FOR UPDATE"
            ),
            concat!(
                "SELECT t.id FROM oauth_tokens t WHERE t.revoked_at IS NULL AND t.grant_id IN \
                 (SELECT g.id FROM oauth_grants g WHERE g.status = 'active' AND ",
                $filter,
                ") ORDER BY t.id FOR UPDATE"
            ),
        )
    };
}

async fn lock_artifacts(tx: &DatabaseTransaction, target: &Target<'_>) -> Result<(), DbErr> {
    let (codes, tokens, values): (&str, &str, Vec<sea_orm::Value>) = match target {
        Target::Grant(id) => {
            let (codes, tokens) = lock_statements!("g.id = $1");
            (codes, tokens, vec![(*id).into()])
        }
        Target::User(user) => {
            let (codes, tokens) = lock_statements!("g.user_id = $1");
            (codes, tokens, vec![(*user).into()])
        }
        Target::UserWorkspace(user, workspace) => {
            let (codes, tokens) = lock_statements!("g.user_id = $1 AND g.workspace_id = $2");
            (codes, tokens, vec![(*user).into(), (*workspace).into()])
        }
        Target::Workspace(workspace) => {
            let (codes, tokens) = lock_statements!("g.workspace_id = $1");
            (codes, tokens, vec![(*workspace).into()])
        }
        Target::Client(client) => {
            let (codes, tokens) = lock_statements!("g.client_id = $1");
            (codes, tokens, vec![(*client).into()])
        }
    };
    for sql in [codes, tokens] {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values.clone()))
            .await?;
    }
    Ok(())
}

fn log_revoked(ids: &[Uuid], reason: RevokeReason) {
    for id in ids {
        tracing::info!(grant_id = %id, reason = reason.as_str(), "oauth grant revoked");
    }
}

/// Revokes one grant. `false` when it was already revoked.
async fn revoke_one(
    tx: &DatabaseTransaction,
    grant_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<bool, DbErr> {
    lock_artifacts(tx, &Target::Grant(grant_id)).await?;
    let revoked = grants::revoke_grant(tx, grant_id, reason, actor).await?;
    if revoked {
        log_revoked(&[grant_id], reason);
    }
    Ok(revoked)
}

/// Revocation hook: every active grant of a user, in every workspace. Call it in the transaction
/// that changes the password or deactivates the account, after that change.
pub async fn revoke_user_grants(
    tx: &DatabaseTransaction,
    user_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    lock_artifacts(tx, &Target::User(user_id)).await?;
    let ids = grants::revoke_all_user_grants(tx, user_id, reason, actor).await?;
    log_revoked(&ids, reason);
    Ok(ids)
}

/// Revocation hook: every active grant of one user in one workspace. Call it in the transaction
/// that removes the membership.
pub async fn revoke_user_workspace_grants(
    tx: &DatabaseTransaction,
    user_id: Uuid,
    workspace_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    lock_artifacts(tx, &Target::UserWorkspace(user_id, workspace_id)).await?;
    let ids = grants::revoke_user_workspace_grants(tx, user_id, workspace_id, reason, actor).await?;
    log_revoked(&ids, reason);
    Ok(ids)
}

/// Every active grant in a workspace (connected applications switched off).
pub async fn revoke_workspace_grants(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    lock_artifacts(tx, &Target::Workspace(workspace_id)).await?;
    let ids = grants::revoke_workspace_grants(tx, workspace_id, reason, actor).await?;
    log_revoked(&ids, reason);
    Ok(ids)
}

/// Every active grant of a client (client disabled).
pub async fn revoke_client_grants(
    tx: &DatabaseTransaction,
    client_id: &str,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    lock_artifacts(tx, &Target::Client(client_id)).await?;
    let ids = grants::revoke_client_grants(tx, client_id, reason, actor).await?;
    log_revoked(&ids, reason);
    Ok(ids)
}

// ---------------------------------------------------------------------------------------------
// Request parsing
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Page {
    limit: i64,
    offset: i64,
}

fn parse_bounded(value: &str, min: i64, max: i64) -> Option<i64> {
    value.parse::<i64>().ok().filter(|number| (min..=max).contains(number))
}

/// `limit` (1 to 100, default 50) and `offset` (0 to 10000, default 0). Anything else is a 400
/// rather than a silent clamp, so a client that asks for more than it can get is told.
fn page_of(uri: &Uri) -> Result<Page, ManageError> {
    let mut page = Page {
        limit: DEFAULT_PAGE_SIZE,
        offset: 0,
    };
    let Some(query) = uri.query().filter(|query| !query.is_empty()) else {
        return Ok(page);
    };
    let pairs = parse_form(query.as_bytes()).ok_or_else(|| ManageError::invalid("the query string is not valid"))?;
    for (name, value) in pairs {
        match name.as_str() {
            "limit" => {
                page.limit = parse_bounded(&value, 1, MAX_PAGE_SIZE)
                    .ok_or_else(|| ManageError::invalid("limit must be between 1 and 100"))?;
            }
            "offset" => {
                page.offset = parse_bounded(&value, 0, MAX_OFFSET)
                    .ok_or_else(|| ManageError::invalid("offset must be between 0 and 10000"))?;
            }
            _ => {}
        }
    }
    Ok(page)
}

/// A path value that must be a UUID. A malformed one answers exactly as an unknown one does.
fn uuid_or_not_found(text: &str) -> Result<Uuid, ManageError> {
    Uuid::parse_str(text).map_err(|_| ManageError::not_found())
}

fn json_body<T: for<'de> Deserialize<'de>>(headers: &HeaderMap, body: &Bytes) -> Result<T, ManageError> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return Err(ManageError::invalid("the request body must be application/json"));
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(ManageError::invalid("the request body is too large"));
    }
    serde_json::from_slice(body).map_err(|_| ManageError::invalid("the request body is not valid"))
}

// ---------------------------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the fields are the columns of the row")]
struct GrantRow {
    grant_id: Uuid,
    client_id: String,
    client_name: String,
    device_name: Option<String>,
    workspace_id: Uuid,
    workspace_name: String,
    user_id: Uuid,
    user_name: String,
    user_email: String,
    scopes: Vec<String>,
    project_mode: String,
    consented_at: Timestamp,
    last_used_at: Option<Timestamp>,
    expires_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the fields are the columns of the row")]
struct ProjectRow {
    grant_id: Uuid,
    project_id: Uuid,
    project_key: String,
    project_name: String,
    project_count: i64,
}

/// What a management screen needs and nothing from the token tables: the grant id, the client's
/// display data, the device label, the workspace, the scopes, the project selection and the
/// consent, last-use and expiry times.
macro_rules! grant_select {
    ($filter:literal) => {
        concat!(
            "SELECT g.id AS grant_id, g.client_id, c.display_name AS client_name, g.device_name, \
                    g.workspace_id, w.name AS workspace_name, g.user_id, u.name AS user_name, \
                    u.email AS user_email, g.scopes, g.project_mode, g.consented_at, g.last_used_at, g.expires_at \
             FROM oauth_grants g \
             JOIN oauth_clients c ON c.client_id = g.client_id \
             JOIN workspaces w ON w.id = g.workspace_id \
             JOIN users u ON u.id = g.user_id \
             WHERE g.status = 'active' AND g.expires_at > now() AND ",
            $filter,
            " ORDER BY g.consented_at DESC, g.id DESC LIMIT $2 OFFSET $3"
        )
    };
}

const USER_GRANTS_SQL: &str = grant_select!("g.user_id = $1");
const WORKSPACE_GRANTS_SQL: &str = grant_select!("g.workspace_id = $1");

const PROJECTS_SQL: &str = "SELECT t.grant_id, t.project_id, t.project_key, t.project_name, t.project_count \
     FROM (SELECT gp.grant_id, p.id AS project_id, p.key AS project_key, p.name AS project_name, \
                  row_number() OVER (PARTITION BY gp.grant_id ORDER BY p.key, p.id) AS position, \
                  count(*) OVER (PARTITION BY gp.grant_id) AS project_count \
           FROM oauth_grant_projects gp JOIN projects p ON p.id = gp.project_id \
           WHERE gp.grant_id = ANY($1)) t \
     WHERE t.position <= $2 ORDER BY t.grant_id, t.position";

#[derive(Clone, Copy)]
enum ListScope {
    /// The caller's own grants.
    Own(Uuid),
    /// Every grant that reaches a workspace, with the person who made it.
    Workspace(Uuid),
}

async fn list_grants(db: &DatabaseConnection, scope: ListScope, page: Page) -> Result<Value, ManageError> {
    let (sql, subject) = match scope {
        ListScope::Own(user) => (USER_GRANTS_SQL, user),
        ListScope::Workspace(workspace) => (WORKSPACE_GRANTS_SQL, workspace),
    };
    // One more than asked for tells whether another page exists.
    let mut rows = GrantRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![subject.into(), (page.limit + 1).into(), page.offset.into()],
    ))
    .all(db)
    .await?;
    let has_more = i64::try_from(rows.len()).is_ok_and(|len| len > page.limit);
    rows.truncate(usize::try_from(page.limit).unwrap_or(usize::MAX));

    let selected: Vec<Uuid> = rows
        .iter()
        .filter(|row| row.project_mode == "selected")
        .map(|row| row.grant_id)
        .collect();
    let projects = if selected.is_empty() {
        Vec::new()
    } else {
        ProjectRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            PROJECTS_SQL,
            vec![selected.into(), MAX_LISTED_PROJECTS.into()],
        ))
        .all(db)
        .await?
    };

    let with_user = matches!(scope, ListScope::Workspace(_));
    let items: Vec<Value> = rows.iter().map(|row| grant_item(row, &projects, with_user)).collect();
    Ok(json!({
        "items": items,
        "limit": page.limit,
        "offset": page.offset,
        "has_more": has_more,
        "next_offset": has_more.then_some(page.offset + page.limit),
    }))
}

fn grant_item(row: &GrantRow, projects: &[ProjectRow], with_user: bool) -> Value {
    let own: Vec<&ProjectRow> = projects
        .iter()
        .filter(|project| project.grant_id == row.grant_id)
        .collect();
    let mut item = json!({
        "grant_id": row.grant_id,
        "client": {"client_id": row.client_id, "display_name": row.client_name},
        "device_name": row.device_name,
        "workspace": {"id": row.workspace_id, "name": row.workspace_name},
        "scopes": row.scopes,
        "project_mode": row.project_mode,
        "projects": own.iter().map(|project| json!({
            "id": project.project_id,
            "key": project.project_key,
            "name": project.project_name,
        })).collect::<Vec<_>>(),
        "project_count": own.first().map(|project| project.project_count),
        "consented_at": row.consented_at,
        "last_used_at": row.last_used_at,
        "expires_at": row.expires_at,
    });
    if with_user && let Some(map) = item.as_object_mut() {
        map.insert(
            "user".to_owned(),
            json!({"id": row.user_id, "name": row.user_name, "email": row.user_email}),
        );
    }
    item
}

// ---------------------------------------------------------------------------------------------
// Level 1: the person's own connections
// ---------------------------------------------------------------------------------------------

async fn list_own(State(state): State<AppState>, session: Session, uri: Uri) -> Result<Response, ManageError> {
    let page = page_of(&uri)?;
    Ok(success(
        &list_grants(&state.db, ListScope::Own(session.user_id), page).await?,
    ))
}

async fn disconnect_own(
    State(state): State<AppState>,
    session: Session,
    Path(grant_id): Path<String>,
) -> Result<Response, ManageError> {
    let grant_id = uuid_or_not_found(&grant_id)?;
    retrying(|| {
        in_tx(&state.db, async |tx| {
            // Someone else's grant, a revoked one and an unknown id are all the same answer.
            let found = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM oauth_grants WHERE id = $1 AND user_id = $2 AND status = 'active' \
                 AND expires_at > now()",
                    vec![grant_id.into(), session.user_id.into()],
                ))
                .await?;
            if found.is_none() {
                return Err(ManageError::not_found());
            }
            if !revoke_one(tx, grant_id, RevokeReason::UserRevoked, Actor::User(session.user_id)).await? {
                return Err(ManageError::not_found());
            }
            Ok(())
        })
    })
    .await?;
    Ok(success(&json!({"grant_id": grant_id, "revoked": true})))
}

// ---------------------------------------------------------------------------------------------
// Level 2: a workspace owner or admin
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct RoleRow {
    role: String,
}

/// The caller must be an owner or admin of the workspace. A person who is not a member at all
/// gets the answer an unknown workspace gets, so the endpoint does not confirm which workspaces
/// exist; a member without the role is refused.
async fn require_workspace_admin<C: ConnectionTrait>(
    conn: &C,
    session: &Session,
    workspace_id: Uuid,
) -> Result<(), ManageError> {
    let row = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), session.user_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(ManageError::not_found)?;
    if matches!(row.role.trim().to_lowercase().as_str(), "owner" | "admin") {
        Ok(())
    } else {
        Err(ManageError::forbidden("workspace owner or admin access required"))
    }
}

async fn list_workspace(
    State(state): State<AppState>,
    session: Session,
    Path(workspace_id): Path<String>,
    uri: Uri,
) -> Result<Response, ManageError> {
    let workspace_id = uuid_or_not_found(&workspace_id)?;
    require_workspace_admin(&state.db, &session, workspace_id).await?;
    let page = page_of(&uri)?;
    Ok(success(
        &list_grants(&state.db, ListScope::Workspace(workspace_id), page).await?,
    ))
}

async fn disconnect_in_workspace(
    State(state): State<AppState>,
    session: Session,
    Path((workspace_id, grant_id)): Path<(String, String)>,
) -> Result<Response, ManageError> {
    let workspace_id = uuid_or_not_found(&workspace_id)?;
    require_workspace_admin(&state.db, &session, workspace_id).await?;
    let grant_id = uuid_or_not_found(&grant_id)?;
    retrying(|| {
        in_tx(&state.db, async |tx| {
            // A grant of another workspace is not found here, whoever owns it.
            let found = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM oauth_grants WHERE id = $1 AND workspace_id = $2 AND status = 'active' \
                 AND expires_at > now()",
                    vec![grant_id.into(), workspace_id.into()],
                ))
                .await?;
            if found.is_none() {
                return Err(ManageError::not_found());
            }
            if !revoke_one(
                tx,
                grant_id,
                RevokeReason::WorkspaceAdminRevoked,
                Actor::User(session.user_id),
            )
            .await?
            {
                return Err(ManageError::not_found());
            }
            Ok(())
        })
    })
    .await?;
    Ok(success(&json!({"grant_id": grant_id, "revoked": true})))
}

#[derive(Debug, FromQueryResult)]
struct AllowRow {
    allowed: bool,
}

async fn get_settings(
    State(state): State<AppState>,
    session: Session,
    Path(workspace_id): Path<String>,
) -> Result<Response, ManageError> {
    let workspace_id = uuid_or_not_found(&workspace_id)?;
    require_workspace_admin(&state.db, &session, workspace_id).await?;
    // No row means allowed.
    let row = AllowRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT allow_connected_apps AS allowed FROM workspace_delegation_settings WHERE workspace_id = $1",
        vec![workspace_id.into()],
    ))
    .one(&state.db)
    .await?;
    Ok(success(
        &json!({"allow_connected_apps": row.is_none_or(|row| row.allowed)}),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    allow_connected_apps: bool,
}

async fn put_settings(
    State(state): State<AppState>,
    session: Session,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ManageError> {
    let workspace_id = uuid_or_not_found(&workspace_id)?;
    require_workspace_admin(&state.db, &session, workspace_id).await?;
    let body: SettingsBody = json_body(&headers, &body)?;
    let allow = body.allow_connected_apps;
    let revoked = retrying(|| {
        in_tx(&state.db, async |tx| {
            // The role is checked again under the transaction, so a demotion that committed
            // after the first check is honoured.
            require_workspace_admin(tx, &session, workspace_id).await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps, updated_by, updated_at) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = EXCLUDED.allow_connected_apps, \
                   updated_by = EXCLUDED.updated_by, updated_at = now()",
                vec![workspace_id.into(), allow.into(), session.user_id.into()],
            ))
            .await?;
            let revoked = if allow {
                Vec::new()
            } else {
                revoke_workspace_grants(
                    tx,
                    workspace_id,
                    RevokeReason::WorkspaceDisabled,
                    Actor::User(session.user_id),
                )
                .await?
            };
            audit::write(
                tx,
                &session
                    .record(AuditEvent::WorkspaceSettingChanged)
                    .with_workspace(workspace_id)
                    .with_detail("allow_connected_apps", DetailValue::Flag(allow))
                    .with_detail("revoked_grants", DetailValue::Count(i64::try_from(revoked.len()).unwrap_or(i64::MAX))),
            )
            .await?;
            Ok(revoked.len())
        })
    })
    .await?;
    Ok(success(
        &json!({"allow_connected_apps": allow, "revoked_grants": revoked}),
    ))
}

// ---------------------------------------------------------------------------------------------
// Level 3: the instance administrator
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct ClientRow {
    client_id: String,
    display_name: String,
    description: String,
    homepage_url: Option<String>,
    client_type: String,
    redirect_uris: Vec<String>,
    allowed_scopes: Vec<String>,
    allowed_resources: Vec<String>,
    status: String,
    status_changed_at: Option<Timestamp>,
    created_at: Timestamp,
    active_grants: i64,
}

async fn list_clients(State(state): State<AppState>, session: Session) -> Result<Response, ManageError> {
    session.require_instance_admin()?;
    let rows = ClientRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT c.client_id, c.display_name, c.description, c.homepage_url, c.client_type, c.redirect_uris, \
                c.allowed_scopes, c.allowed_resources, c.status, c.status_changed_at, c.created_at, \
                (SELECT count(*) FROM oauth_grants g WHERE g.client_id = c.client_id AND g.status = 'active' \
                   AND g.expires_at > now()) AS active_grants \
         FROM oauth_clients c ORDER BY c.client_id LIMIT $1",
        vec![MAX_CLIENTS_LISTED.into()],
    ))
    .all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "client_id": row.client_id,
                "display_name": row.display_name,
                "description": row.description,
                "homepage_url": row.homepage_url,
                "client_type": row.client_type,
                "redirect_uris": row.redirect_uris,
                "allowed_scopes": row.allowed_scopes,
                "allowed_resources": row.allowed_resources,
                "status": row.status,
                "status_changed_at": row.status_changed_at,
                "created_at": row.created_at,
                "active_grants": row.active_grants,
            })
        })
        .collect();
    Ok(success(&json!({"items": items})))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientBody {
    status: String,
}

#[derive(Debug, FromQueryResult)]
struct StatusRow {
    status: String,
}

async fn patch_client(
    State(state): State<AppState>,
    session: Session,
    Path(client_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ManageError> {
    session.require_instance_admin()?;
    let body: ClientBody = json_body(&headers, &body)?;
    let (status, disabling) = match body.status.as_str() {
        "active" => ("active", false),
        "disabled" => ("disabled", true),
        _ => return Err(ManageError::invalid("status must be active or disabled")),
    };
    let revoked = retrying(|| {
        in_tx(&state.db, async |tx| {
            let current = StatusRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status FROM oauth_clients WHERE client_id = $1 FOR NO KEY UPDATE",
                vec![client_id.clone().into()],
            ))
            .one(tx)
            .await?
            .ok_or_else(ManageError::not_found)?;
            let changed = current.status != status;
            if changed {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE oauth_clients SET status = $2, status_changed_by = $3, status_changed_at = now(), \
                     updated_at = now() WHERE client_id = $1",
                    vec![client_id.clone().into(), status.into(), session.user_id.into()],
                ))
                .await?;
            }
            // Disabling always sweeps, also when the client was already disabled: a connection
            // that raced the first sweep is ended by the second.
            let revoked = if disabling {
                revoke_client_grants(
                    tx,
                    &client_id,
                    RevokeReason::ClientDisabled,
                    Actor::User(session.user_id),
                )
                .await?
            } else {
                Vec::new()
            };
            audit::write(
                tx,
                &session
                    .record(AuditEvent::ClientStatusChanged)
                    .with_client(&client_id)
                    .with_detail("status", DetailValue::Code(status))
                    .with_detail("changed", DetailValue::Flag(changed))
                    .with_detail(
                        "revoked_grants",
                        DetailValue::Count(i64::try_from(revoked.len()).unwrap_or(i64::MAX)),
                    ),
            )
            .await?;
            Ok(revoked.len())
        })
    })
    .await?;
    Ok(success(
        &json!({"client_id": client_id, "status": status, "revoked_grants": revoked}),
    ))
}

// ---------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------

/// The management routes. Without `[oauth] issuer` all of them answer 404.
pub fn routes(config: Option<&Arc<OAuthConfig>>) -> Router<AppState> {
    let Some(config) = config else {
        return Router::new()
            .route(USER_APPS_PATH, any(not_found))
            .route(USER_APP_PATH, any(not_found))
            .route(WORKSPACE_APPS_PATH, any(not_found))
            .route(WORKSPACE_APP_PATH, any(not_found))
            .route(WORKSPACE_SETTINGS_PATH, any(not_found))
            .route(ADMIN_CLIENTS_PATH, any(not_found))
            .route(ADMIN_CLIENT_PATH, any(not_found));
    };
    Router::new()
        .route(USER_APPS_PATH, get(list_own))
        .route(USER_APP_PATH, axum::routing::delete(disconnect_own))
        .route(WORKSPACE_APPS_PATH, get(list_workspace))
        .route(WORKSPACE_APP_PATH, axum::routing::delete(disconnect_in_workspace))
        .route(WORKSPACE_SETTINGS_PATH, get(get_settings).put(put_settings))
        .route(ADMIN_CLIENTS_PATH, get(list_clients))
        .route(ADMIN_CLIENT_PATH, axum::routing::patch(patch_client))
        .layer(Extension(Arc::clone(config)))
}

#[cfg(test)]
mod tests;
