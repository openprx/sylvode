// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements)]

/// Bot Token authentication middleware.
///
/// Supports two authentication modes:
///   1. Bot Token (`opr_*`) — looks up `workspace_bots` table via SHA-256 hash.
///   2. JWT Bearer / cookie — falls back to the existing JWT path.
///
/// On success the middleware injects:
///   - bot token auth: `BotAuthContext` + synthetic `JwtClaims`
///   - JWT auth: `JwtClaims`
use axum::{
    extract::{Request, State},
    http::Extensions,
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use platform::{
    app::AppState,
    auth::{JwtClaims, JwtManager, TokenType},
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::time::Instant;
use uuid::Uuid;

use crate::{
    error::ApiError,
    flow::event_origin::EventSurface,
    response::OperationResponseMeta,
    routes::auth::{extract_bearer_token, extract_cookie_token},
};

/// Canonical MCP attribution headers (ADR-0020 D3).
const MCP_TOOL_HEADER: &str = "x-sylvode-mcp-tool";
const MCP_SURFACE_HEADER: &str = "x-sylvode-mcp-surface";
/// Legacy spellings, still sent by every 1.x `mcp-server` next to the canonical ones and by any
/// pre-1.0 client on its own. Accepted for the whole 1.x line.
const LEGACY_MCP_TOOL_HEADER: &str = "x-openpr-mcp-tool";
const LEGACY_MCP_SURFACE_HEADER: &str = "x-openpr-mcp-surface";

/// The MCP attribution a request presented, after the canonical and legacy spellings have been
/// reconciled. Values that are not visible ASCII are treated as absent, as before.
#[derive(Debug, Clone, Copy, Default)]
struct McpAttribution<'a> {
    surface: Option<&'a str>,
    tool: Option<&'a str>,
}

/// Reconciles every occurrence of one attribution field across its canonical and legacy header
/// names.
///
/// Fail-closed (ADR-0020 D3): every occurrence must carry byte-identical values, otherwise the
/// request is rejected. These headers become audit evidence, so silently preferring one spelling
/// over the other would let a caller record an attribution the other header contradicts.
fn reconcile_attribution_header<'a>(
    headers: &'a axum::http::HeaderMap,
    canonical: &str,
    legacy: &str,
) -> Result<Option<&'a str>, ApiError> {
    let mut values = headers.get_all(canonical).iter().chain(headers.get_all(legacy).iter());
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.any(|other| other.as_bytes() != first.as_bytes()) {
        return Err(ApiError::Unauthorized(format!(
            "conflicting MCP attribution headers: every `{canonical}` / `{legacy}` value must be identical"
        )));
    }
    Ok(first.to_str().ok())
}

/// The single reader of the MCP attribution headers; every consumer goes through it.
fn mcp_attribution(headers: &axum::http::HeaderMap) -> Result<McpAttribution<'_>, ApiError> {
    Ok(McpAttribution {
        surface: reconcile_attribution_header(headers, MCP_SURFACE_HEADER, LEGACY_MCP_SURFACE_HEADER)?,
        tool: reconcile_attribution_header(headers, MCP_TOOL_HEADER, LEGACY_MCP_TOOL_HEADER)?,
    })
}

struct BotOperationContext {
    bot_id: Uuid,
    workspace_id: Uuid,
    tool_name: Option<String>,
    surface: EventSurface,
    method: String,
    path: String,
    request_id: Uuid,
}

/// The transport this authenticated request arrived over, resolved **here at the boundary** and
/// nowhere else.
///
/// `events-v1.md` (2026-09-01) requires `source.surface` to be "由认证/传输边界推导" and carried
/// into the domain by the auth context; [`EventSurface::from_client_transport_label`] is the
/// allow-list, and an unrecognized or absent label falls back to [`EventSurface::Rest`]. Returning
/// the typed enum rather than a `&'static str` is what stops a second, drifting vocabulary from
/// appearing downstream: the bot-operation log spells it with `as_wire()`, and so does the event
/// envelope.
///
/// ⚠️ **Residual trust gap, stated plainly.** The label is an HTTP header, so it is still
/// *asserted* by the holder of a bot token rather than proven: a token may declare `mcp_http` on a
/// plain `curl`. The allow-list bounds the damage — `web`, `worker` and `system` are unreachable,
/// and a request with no bot credential never reaches this function at all (JWT-direct traffic is
/// `Rest` by construction) — but it does not close it. Closing it needs the *credential* to carry
/// its registered surface, which `workspace_bots` has no column for today; see the report's
/// contract-TODO list. This is the pre-existing trust model of these headers, which already drive
/// `bot_operation_logs.surface`; this change widens their blast radius from observability to
/// audit evidence, and that is worth saying out loud rather than burying.
#[cfg(test)]
fn operation_surface(attribution: McpAttribution<'_>) -> EventSurface {
    attribution
        .surface
        .and_then(EventSurface::from_client_transport_label)
        .unwrap_or(EventSurface::Rest)
}

fn registered_surface(value: &str) -> Result<EventSurface, ApiError> {
    EventSurface::from_bot_credential_label(value)
        .ok_or_else(|| ApiError::Unauthorized("bot credential has an invalid registered transport".to_string()))
}

/// Resolves the request surface only when it equals the surface stored with the credential.
/// The caller-controlled header is now merely a presented transport label; it cannot promote a
/// token into another allowed transport (ADR-0018 G8's v0.7 closure).
fn credential_bound_surface(attribution: McpAttribution<'_>, registered: &str) -> Result<EventSurface, ApiError> {
    let registered = registered_surface(registered)?;
    let declared = attribution.surface.and_then(EventSurface::from_client_transport_label);
    match (registered, declared) {
        (EventSurface::Rest, None) => Ok(EventSurface::Rest),
        (expected, Some(actual)) if expected == actual => Ok(expected),
        _ => Err(ApiError::Unauthorized(
            "bot credential is not valid for the presented transport".to_string(),
        )),
    }
}

fn operation_tool_name(attribution: McpAttribution<'_>) -> Option<String> {
    let value = attribution.tool?.trim();
    let mut segments = value.split('.');
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 64
            && segment.starts_with(|character: char| character.is_ascii_lowercase())
            && segment
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_')
    };
    let first = segments.next()?;
    let second = segments.next()?;
    if value.len() <= 128 && valid_segment(first) && valid_segment(second) && segments.all(valid_segment) {
        Some(value.to_string())
    } else {
        None
    }
}

/// Writes the attribution row of one bot-authenticated operation to `bot_operation_logs`.
///
/// Awaited by the middleware rather than spawned: the row is the record that attributes the call
/// to its bot, surface and tool, so the middleware has to know whether it exists before it
/// reports the outcome (see [`fail_closed_on_audit_failure`]).
async fn record_operation<C: ConnectionTrait>(
    db: &C,
    context: BotOperationContext,
    business_code: i32,
    error_message: Option<&'static str>,
    duration_ms: i64,
) -> Result<(), sea_orm::DbErr> {
    let outcome = if business_code == 0 { "ok" } else { "error" };
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO bot_operation_logs
           (id, workspace_id, bot_id, tool_name, surface, method, path,
            business_code, outcome, error_message, duration_ms, request_id, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        vec![
            Uuid::new_v4().into(),
            context.workspace_id.into(),
            context.bot_id.into(),
            context.tool_name.into(),
            context.surface.as_wire().into(),
            context.method.into(),
            context.path.into(),
            business_code.into(),
            outcome.into(),
            error_message.map(str::to_string).into(),
            duration_ms.into(),
            context.request_id.into(),
            Utc::now().into(),
        ],
    ))
    .await
    .map(|_| ())
}

/// Fail-closed outcome of a bot operation whose attribution row could not be written.
///
/// A bot operation never reports success without its `bot_operation_logs` row: when the write
/// fails, a successful response is replaced by the internal-error response and the failure is
/// logged at ERROR with the database's reason. A response that already reports a failure is
/// returned unchanged (it reports no success), and the failure is logged the same way. The
/// operation itself has already run by then, so a caller that sees the internal error retries
/// with its idempotency key where the command takes one.
fn fail_closed_on_audit_failure(
    response: Response,
    business_code: i32,
    request_id: Uuid,
    surface: EventSurface,
    audit: Result<(), sea_orm::DbErr>,
) -> Result<Response, ApiError> {
    let Err(error) = audit else {
        return Ok(response);
    };
    tracing::error!(
        request_id = %request_id,
        surface = surface.as_wire(),
        business_code,
        error = %error,
        "bot operation log write failed; the operation is not reported as successful without its attribution record"
    );
    if business_code == 0 {
        Err(ApiError::Internal)
    } else {
        Ok(response)
    }
}

/// Auth context injected when a bot token is used.
#[derive(Debug, Clone, Serialize)]
pub struct BotAuthContext {
    pub bot_id: Uuid,
    pub workspace_id: Uuid,
    pub permissions: Vec<String>,
    /// The transport this request arrived over, as resolved by [`operation_surface`].
    ///
    /// `events-v1.md` (2026-09-01) 判据 (c): "认证上下文（如 bot auth）**必须承载** transport 与
    /// tool，否则中间件解析出的信息到不了 domain". Before this field existed the middleware parsed
    /// the transport, spent it on the bot-operation log, and dropped it — so a real
    /// `flow.feature_set` MCP tool call, which does send the header, was still recorded as
    /// `surface: "rest"` in `business_events`. The parse was never the missing part; the wire from
    /// the parse to the domain was.
    pub surface: EventSurface,
    /// The exact registered tool name from the MCP client, validated by [`operation_tool_name`].
    /// `None` for a bot request that is not a tool call.
    pub tool_name: Option<String>,
    /// The server-minted id for this request, shared verbatim with the bot-operation log row.
    ///
    /// Reused as the event envelope's `source.request` so that an audit event and the
    /// `bot_operation_logs` row describing the same call join on one value. 判据 (b) — "`source
    /// .request` 必须每请求不同（同一请求的多条事件相同）" — is satisfied structurally: it is minted
    /// once per request here, and every event the request writes copies that one value.
    pub request_id: Uuid,
}

/// Builds the context the domain sees from what the transport boundary resolved.
///
/// Extracted from the middleware body so the wire that `events-v1.md` 判据 (c) is about —
/// "认证上下文（如 bot auth）**必须承载** transport 与 tool，否则中间件解析出的信息到不了 domain" —
/// is a function that can be called from a test and broken by a mutation. Inline in the
/// middleware it was neither: reaching it needed a live bot token row, an axum `Router` and a
/// real request and the surface registered with its credential, so any attempted header-only
/// promotion is rejected before a domain handler sees the request.
fn bot_auth_context(
    bot_id: Uuid,
    workspace_id: Uuid,
    permissions: Vec<String>,
    registered_transport: &str,
    headers: &axum::http::HeaderMap,
) -> Result<BotAuthContext, ApiError> {
    let attribution = mcp_attribution(headers)?;
    Ok(BotAuthContext {
        bot_id,
        workspace_id,
        permissions,
        surface: credential_bound_surface(attribution, registered_transport)?,
        tool_name: operation_tool_name(attribution),
        // Minted once per request, here, and copied by everything that describes this request.
        request_id: Uuid::new_v4(),
    })
}

pub fn extract_bot_context(extensions: &Extensions) -> Option<&BotAuthContext> {
    extensions.get::<BotAuthContext>()
}

/// What a bot token is allowed to do, as stored in `workspace_bots.permissions`.
///
/// The three names are the ones `POST /workspaces/{id}/bots` accepts and the ones the column
/// comment documents. Until now none of them was ever consulted: every bot was mapped to a
/// workspace role and the `read` / `write` distinction existed only on paper, so a token issued as
/// read-only could create, update and delete anything its workspace contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotPermission {
    /// Retrieve data. Implied by `write` and `admin` — a write returns what it wrote.
    Read,
    /// Change data. Implied by `admin`.
    Write,
    /// Act as a workspace administrator: bypasses form field-level policies and record scoping
    /// exactly like a human `admin` member, and nothing beyond that.
    Admin,
}

impl BotPermission {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }
}

/// Whether `permissions` grants `required`, honouring `admin` > `write` > `read`.
///
/// Unknown entries grant nothing: the issuing endpoint rejects them, so a token carrying one is
/// either forged or predates a permission rename, and neither deserves the benefit of the doubt.
pub fn bot_permissions_allow(permissions: &[String], required: BotPermission) -> bool {
    let granted = |name: &str| permissions.iter().any(|permission| permission == name);
    match required {
        BotPermission::Read => granted("read") || granted("write") || granted("admin"),
        BotPermission::Write => granted("write") || granted("admin"),
        BotPermission::Admin => granted("admin"),
    }
}

/// Reject a bot token that does not carry `required`.
pub fn ensure_bot_permission(bot: &BotAuthContext, required: BotPermission) -> Result<(), ApiError> {
    if bot_permissions_allow(&bot.permissions, required) {
        return Ok(());
    }
    Err(ApiError::Forbidden(format!(
        "bot token lacks the '{}' permission",
        required.as_str()
    )))
}

/// `pub(crate)` so `ADR-0012` §4.1 point 5's second required direction can be asserted directly:
/// the narrowing that stops an admin bot from crossing an object authorization boundary must
/// **not** take away its workspace-level admin role, and the only honest witness for "the role is
/// still synthesized" is this function itself.
pub(crate) fn bot_role_from_permissions(permissions: &[String]) -> String {
    if bot_permissions_allow(permissions, BotPermission::Admin) {
        "admin".to_string()
    } else {
        "member".to_string()
    }
}

/// Unified workspace access check for both bot-token and JWT auth paths.
///
/// Returns `(actor_id, role, is_bot)`:
/// - `actor_id`: user id (JWT) or bot id (bot token)
/// - `role`: workspace role for user, or a synthesized role from bot permissions
/// - `is_bot`: whether the request used a bot token
pub async fn require_workspace_access(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
) -> Result<(Uuid, String, bool), ApiError> {
    let claims = extensions
        .get::<JwtClaims>()
        .ok_or_else(|| ApiError::Unauthorized("missing auth context".to_string()))?;
    let bot = extract_bot_context(extensions);

    require_workspace_access_from_auth(state, claims, bot, workspace_id).await
}

pub async fn require_workspace_access_from_auth(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    workspace_id: Uuid,
) -> Result<(Uuid, String, bool), ApiError> {
    if let Some(bot_ctx) = bot {
        if bot_ctx.workspace_id != workspace_id {
            return Err(ApiError::Forbidden("bot not authorized for this workspace".to_string()));
        }
        // A token with no usable permission at all reaches nothing, the same way a user with no
        // workspace membership row does.
        ensure_bot_permission(bot_ctx, BotPermission::Read)?;
        let role = bot_role_from_permissions(&bot_ctx.permissions);
        return Ok((bot_ctx.bot_id, role, true));
    }

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    #[derive(Debug, FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let row = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workspace not found or access denied".to_string()))?;

    Ok((user_id, row.role, false))
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// The permission an HTTP method demands of a bot token.
///
/// Safe methods only read, everything else can change state. Kept separate from the middleware so
/// the mapping is testable without standing up a router.
fn required_bot_permission(method: &axum::http::Method) -> BotPermission {
    if method.is_safe() {
        BotPermission::Read
    } else {
        BotPermission::Write
    }
}

/// Middleware: authenticate as bot (opr_ token) or fall through to JWT.
///
/// Injects `JwtClaims` for both paths to keep existing handlers compatible.
/// For bot tokens, also injects `BotAuthContext`.
pub async fn bot_or_user_auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let started = Instant::now();
    let mut operation_context = None;
    let token = extract_bearer_token(req.headers())
        .or_else(|| extract_cookie_token(req.headers(), "access_token"))
        .ok_or_else(|| ApiError::Unauthorized("missing access token".to_string()))?;

    if token.starts_with("opr_") {
        // ── Bot Token path ──
        let token_hash = sha256_hex(&token);

        #[derive(Debug, FromQueryResult)]
        struct BotRow {
            id: Uuid,
            workspace_id: Uuid,
            permissions: serde_json::Value,
            transport_surface: String,
            is_active: bool,
            expires_at: Option<chrono::DateTime<Utc>>,
        }

        let bot = BotRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"SELECT id, workspace_id, permissions, transport_surface, is_active, expires_at
               FROM workspace_bots
               WHERE token_hash = $1",
            vec![token_hash.into()],
        ))
        .one(&state.db)
        .await
        .map_err(|_| ApiError::Internal)?
        .ok_or_else(|| ApiError::Unauthorized("invalid bot token".to_string()))?;

        if !bot.is_active {
            return Err(ApiError::Unauthorized("bot token is disabled".to_string()));
        }
        if let Some(expires_at) = bot.expires_at
            && expires_at < Utc::now()
        {
            return Err(ApiError::Unauthorized("bot token has expired".to_string()));
        }

        // Update last_used_at asynchronously (best-effort, don't block request)
        let db = state.db.clone();
        let bot_id = bot.id;
        tokio::spawn(async move {
            let _ = db
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE workspace_bots SET last_used_at = $1 WHERE id = $2",
                    vec![Utc::now().into(), bot_id.into()],
                ))
                .await;
        });

        let permissions: Vec<String> = bot
            .permissions
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let context = bot_auth_context(
            bot.id,
            bot.workspace_id,
            permissions,
            &bot.transport_surface,
            req.headers(),
        )?;
        // Every bot-reachable route passes through here, which makes this the one place the
        // `read` / `write` split can be enforced without trusting each handler to remember. The
        // HTTP method is the authority: safe methods (GET/HEAD/OPTIONS/TRACE) need `read`,
        // everything else needs `write`. Handlers that mutate behind a POST are covered; the price
        // is that a read-shaped POST (preview, signed-url) also needs `write`, which is the
        // direction to fail in.
        let authenticated_operation = BotOperationContext {
            bot_id: context.bot_id,
            workspace_id: context.workspace_id,
            // Copied from the auth context rather than resolved a second time, so the surface,
            // tool and request id an audit event carries are the *same values* this log records
            // for the same call — not a second, independently derived answer that can disagree.
            tool_name: context.tool_name.clone(),
            surface: context.surface,
            method: req.method().as_str().to_string(),
            path: req.uri().path().to_string(),
            request_id: context.request_id,
        };
        if let Err(error) = ensure_bot_permission(&context, required_bot_permission(req.method())) {
            let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
            let request_id = authenticated_operation.request_id;
            let surface = authenticated_operation.surface;
            if let Err(audit_error) =
                record_operation(&state.db, authenticated_operation, 403, Some("forbidden"), duration_ms).await
            {
                // The request is refused either way; the refusal stays the answer.
                tracing::error!(
                    request_id = %request_id,
                    surface = surface.as_wire(),
                    business_code = 403,
                    error = %audit_error,
                    "bot operation log write failed for a refused bot request"
                );
            }
            return Err(error);
        }
        operation_context = Some(authenticated_operation);

        req.extensions_mut().insert(context);
        req.extensions_mut().insert(JwtClaims {
            sub: bot.id.to_string(),
            email: format!("bot+{}@openpr.local", bot.id),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        });
    } else {
        // ── JWT path (unchanged behaviour) ──
        let jwt = JwtManager::new(
            state.cfg.jwt_secret.expose(),
            state.cfg.jwt_access_ttl_seconds,
            state.cfg.jwt_refresh_ttl_seconds,
        );
        let claims: JwtClaims = jwt
            .verify_access_token(&token)
            .map_err(|_| ApiError::Unauthorized("invalid access token".to_string()))?;

        req.extensions_mut().insert(claims);
    }

    let response = next.run(req).await;
    if let Some(context) = operation_context {
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
        let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        let request_id = context.request_id;
        let surface = context.surface;
        let audit = record_operation(&state.db, context, business_code, error_message, duration_ms).await;
        return fail_closed_on_audit_failure(response, business_code, request_id, surface, audit);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{
        BotAuthContext, BotPermission, EventSurface, bot_permissions_allow, bot_role_from_permissions,
        credential_bound_surface, ensure_bot_permission, mcp_attribution, operation_surface, operation_tool_name,
        required_bot_permission,
    };
    use crate::error::ApiError;
    use axum::http::{HeaderMap, HeaderValue, Method};
    use uuid::Uuid;

    fn bot(permissions: &[&str]) -> BotAuthContext {
        BotAuthContext {
            bot_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            permissions: permissions.iter().map(|value| (*value).to_string()).collect(),
            surface: EventSurface::Rest,
            tool_name: None,
            request_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn a_read_only_bot_cannot_perform_write_operations() {
        let read_only = bot(&["read"]);

        assert!(ensure_bot_permission(&read_only, BotPermission::Read).is_ok());
        assert!(ensure_bot_permission(&read_only, BotPermission::Write).is_err());
        assert!(ensure_bot_permission(&read_only, BotPermission::Admin).is_err());
    }

    #[test]
    fn write_and_admin_imply_the_weaker_permissions() {
        assert!(bot_permissions_allow(&["write".to_string()], BotPermission::Read));
        assert!(bot_permissions_allow(&["write".to_string()], BotPermission::Write));
        assert!(!bot_permissions_allow(&["write".to_string()], BotPermission::Admin));

        assert!(bot_permissions_allow(&["admin".to_string()], BotPermission::Read));
        assert!(bot_permissions_allow(&["admin".to_string()], BotPermission::Write));
        assert!(bot_permissions_allow(&["admin".to_string()], BotPermission::Admin));
    }

    #[test]
    fn unknown_and_empty_permissions_grant_nothing() {
        assert!(!bot_permissions_allow(&[], BotPermission::Read));
        assert!(!bot_permissions_allow(&["readonly".to_string()], BotPermission::Read));
        assert!(!bot_permissions_allow(&["ADMIN".to_string()], BotPermission::Admin));
        assert!(ensure_bot_permission(&bot(&[]), BotPermission::Read).is_err());
    }

    #[test]
    fn only_the_admin_permission_synthesizes_the_admin_workspace_role() {
        assert_eq!(bot_role_from_permissions(&["admin".to_string()]), "admin");
        assert_eq!(
            bot_role_from_permissions(&["read".to_string(), "write".to_string()]),
            "member"
        );
        assert_eq!(bot_role_from_permissions(&[]), "member");
    }

    #[test]
    fn unsafe_methods_require_the_write_permission() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert_eq!(required_bot_permission(&method), BotPermission::Read);
        }
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert_eq!(required_bot_permission(&method), BotPermission::Write);
        }
    }

    /// The transport labels a client may declare, and — the half that carries the security
    /// weight — the ones it may not.
    ///
    /// These headers stopped being "bounded observability labels" the moment `BotAuthContext`
    /// began carrying them into `business_events.source` (`events-v1.md` 判据 (c)). What bounds
    /// them now is `EventSurface::from_client_transport_label`'s allow-list: a client can move
    /// itself among the *client* transports, and cannot reach `web` (proven by the WebSocket
    /// ticket handshake, never by a header), `worker`/`system` (background work has no remote
    /// caller), or spell `rest` into anything but the default.
    /// `events-v1.md` 判据 (c), at the seam it names.
    ///
    /// The middleware parsed the transport and the tool long before this work package existed —
    /// and then spent them on the bot-operation log and dropped them, so a real `flow.feature_set`
    /// MCP call still reached the domain looking exactly like a REST call. The parse was never the
    /// missing piece; carrying it into the context the handlers read was.
    #[test]
    fn the_auth_context_carries_the_transport_and_tool_the_boundary_resolved() {
        let workspace_id = Uuid::new_v4();
        let bot_id = Uuid::new_v4();

        for (label, expected) in [
            ("mcp_http", EventSurface::McpHttp),
            ("mcp_sse", EventSurface::McpSse),
            ("mcp_stdio", EventSurface::McpStdio),
            ("cli_tools_call", EventSurface::CliToolsCall),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-sylvode-mcp-surface",
                HeaderValue::from_str(label).expect("static label is a valid header value"),
            );
            headers.insert("x-sylvode-mcp-tool", HeaderValue::from_static("flow.feature_set"));
            let context = super::bot_auth_context(bot_id, workspace_id, vec!["write".to_string()], label, &headers)
                .expect("registered and presented transports match");
            assert_eq!(
                context.surface, expected,
                "a `{label}` call must reach the domain as `{label}`, not as REST"
            );
            assert_eq!(
                context.tool_name.as_deref(),
                Some("flow.feature_set"),
                "the exact registered tool must reach the domain too"
            );
        }

        // No headers at all: a plain bot-token REST call, and a request id all the same.
        let context = super::bot_auth_context(
            bot_id,
            workspace_id,
            vec!["read".to_string()],
            "rest",
            &HeaderMap::new(),
        )
        .expect("REST credentials do not require a transport header");
        assert_eq!(context.surface, EventSurface::Rest);
        assert_eq!(context.tool_name, None);
        assert_ne!(
            context.request_id,
            super::bot_auth_context(
                bot_id,
                workspace_id,
                vec!["read".to_string()],
                "rest",
                &HeaderMap::new(),
            )
            .expect("REST credentials do not require a transport header")
            .request_id,
            "each request must mint its own id — `source.request` is per request, not per process"
        );
    }

    #[test]
    fn a_bot_cannot_forge_a_transport_different_from_its_credential() {
        let mut headers = HeaderMap::new();
        headers.insert("x-sylvode-mcp-surface", HeaderValue::from_static("mcp_sse"));
        let presented = mcp_attribution(&headers).expect("one surface header is not a conflict");
        let empty = HeaderMap::new();
        let absent = mcp_attribution(&empty).expect("no headers is not a conflict");

        assert!(credential_bound_surface(presented, "mcp_http").is_err());
        assert!(credential_bound_surface(presented, "rest").is_err());
        assert_eq!(
            credential_bound_surface(presented, "mcp_sse").expect("matching credential"),
            EventSurface::McpSse
        );
        assert!(credential_bound_surface(absent, "mcp_sse").is_err());
        assert_eq!(
            credential_bound_surface(absent, "rest").expect("plain REST credential"),
            EventSurface::Rest
        );
    }

    #[test]
    fn a_client_may_declare_only_the_client_transports_and_nothing_else() {
        let resolve = |headers: &HeaderMap| -> EventSurface {
            operation_surface(mcp_attribution(headers).expect("a single surface header is not a conflict"))
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-sylvode-mcp-surface", HeaderValue::from_static("mcp_http"));
        headers.insert("x-sylvode-mcp-tool", HeaderValue::from_static("form_records.list"));
        assert_eq!(resolve(&headers), EventSurface::McpHttp);
        assert_eq!(
            operation_tool_name(mcp_attribution(&headers).expect("no conflict")).as_deref(),
            Some("form_records.list")
        );

        for label in ["mcp_sse", "mcp_stdio", "cli", "cli_tools_call"] {
            headers.insert(
                "x-sylvode-mcp-surface",
                HeaderValue::from_str(label).expect("static label is a valid header value"),
            );
            assert_eq!(
                resolve(&headers).as_wire(),
                label,
                "`{label}` is a client transport and must round-trip through the boundary"
            );
        }

        // The refusals. `web`/`worker`/`system` are the ones that would let a bot token mint
        // events indistinguishable from a browser session or from the server talking to itself.
        for forged in ["web", "worker", "system", "admin", "REST", "mcp_http "] {
            headers.insert(
                "x-sylvode-mcp-surface",
                HeaderValue::from_str(forged).expect("static label is a valid header value"),
            );
            assert_eq!(
                resolve(&headers),
                EventSurface::Rest,
                "`{forged}` must not be declarable by a client; the boundary falls back to REST"
            );
        }

        headers.insert("x-sylvode-mcp-tool", HeaderValue::from_static("invalid"));
        assert!(operation_tool_name(mcp_attribution(&headers).expect("no conflict")).is_none());
    }

    fn headers_of(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    fn assert_conflict(result: Result<super::McpAttribution<'_>, ApiError>, case: &str) {
        match result {
            Err(ApiError::Unauthorized(message)) => assert!(
                message.contains("conflicting MCP attribution headers"),
                "{case}: unexpected message {message}"
            ),
            Err(other) => panic!("{case}: wrong error family {other:?}"),
            Ok(attribution) => panic!("{case}: a conflict must be rejected, resolved to {attribution:?}"),
        }
    }

    /// ADR-0020 D3: canonical `X-Sylvode-MCP-*`, legacy `X-OpenPR-MCP-*`, either alone or both
    /// with equal values is accepted; any disagreement is refused rather than resolved.
    #[test]
    fn attribution_accepts_either_spelling_and_refuses_disagreement() {
        for (case, pairs) in [
            (
                "canonical only",
                &[
                    ("x-sylvode-mcp-surface", "mcp_http"),
                    ("x-sylvode-mcp-tool", "projects.list"),
                ][..],
            ),
            (
                "legacy only",
                &[
                    ("x-openpr-mcp-surface", "mcp_http"),
                    ("x-openpr-mcp-tool", "projects.list"),
                ][..],
            ),
            (
                "both equal",
                &[
                    ("x-sylvode-mcp-surface", "mcp_http"),
                    ("x-openpr-mcp-surface", "mcp_http"),
                    ("x-sylvode-mcp-tool", "projects.list"),
                    ("x-openpr-mcp-tool", "projects.list"),
                ][..],
            ),
            (
                "duplicate equal",
                &[
                    ("x-sylvode-mcp-surface", "mcp_http"),
                    ("x-sylvode-mcp-surface", "mcp_http"),
                    ("x-openpr-mcp-tool", "projects.list"),
                ][..],
            ),
        ] {
            let headers = headers_of(pairs);
            let attribution = mcp_attribution(&headers).unwrap_or_else(|error| panic!("{case}: {error:?}"));
            assert_eq!(attribution.surface, Some("mcp_http"), "{case}");
            assert_eq!(attribution.tool, Some("projects.list"), "{case}");
        }

        for (case, pairs) in [
            (
                "surface canonical vs legacy",
                &[
                    ("x-sylvode-mcp-surface", "mcp_http"),
                    ("x-openpr-mcp-surface", "mcp_sse"),
                ][..],
            ),
            (
                "tool canonical vs legacy",
                &[
                    ("x-sylvode-mcp-tool", "projects.list"),
                    ("x-openpr-mcp-tool", "projects.delete"),
                ][..],
            ),
            (
                "duplicate canonical surface",
                &[
                    ("x-sylvode-mcp-surface", "mcp_http"),
                    ("x-sylvode-mcp-surface", "mcp_sse"),
                ][..],
            ),
            (
                "duplicate legacy tool",
                &[
                    ("x-openpr-mcp-tool", "projects.list"),
                    ("x-openpr-mcp-tool", "projects.delete"),
                ][..],
            ),
            (
                "case differs",
                &[
                    ("x-sylvode-mcp-surface", "mcp_http"),
                    ("x-openpr-mcp-surface", "MCP_HTTP"),
                ][..],
            ),
        ] {
            assert_conflict(mcp_attribution(&headers_of(pairs)), case);
        }
    }
}

/// The fail-closed rule for a bot operation whose attribution row could not be written.
#[cfg(test)]
mod audit_failure_tests {
    use super::{EventSurface, fail_closed_on_audit_failure};
    use crate::error::ApiError;
    use axum::{http::StatusCode, response::Response};
    use uuid::Uuid;

    fn teapot() -> Response {
        let mut response = Response::new(axum::body::Body::empty());
        *response.status_mut() = StatusCode::IM_A_TEAPOT;
        response
    }

    #[test]
    fn a_written_audit_row_leaves_the_response_untouched() {
        let response = fail_closed_on_audit_failure(teapot(), 0, Uuid::new_v4(), EventSurface::Cli, Ok(()))
            .expect("an audited operation keeps its response");
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    }

    #[test]
    fn a_successful_operation_without_its_audit_row_reports_an_internal_error() {
        let outcome = fail_closed_on_audit_failure(
            teapot(),
            0,
            Uuid::new_v4(),
            EventSurface::CliToolsCall,
            Err(sea_orm::DbErr::Custom("check constraint violated".to_string())),
        );
        assert!(
            matches!(outcome, Err(ApiError::Internal)),
            "success must not be reported when the attribution record is missing"
        );
    }

    #[test]
    fn a_failed_operation_without_its_audit_row_keeps_its_own_failure() {
        let response = fail_closed_on_audit_failure(
            teapot(),
            404,
            Uuid::new_v4(),
            EventSurface::McpHttp,
            Err(sea_orm::DbErr::Custom("connection reset".to_string())),
        )
        .expect("a response that already reports failure is returned as it is");
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    }
}

/// `bot_operation_logs` and `workspace_bots` against a real `PostgreSQL` server, opt-in via
/// `OPENPR_TEST_DATABASE_URL` like the other scratch-database tests.
///
/// The API accepts a bot surface from one list, [`EventSurface::BOT_CREDENTIAL_SURFACES`], while
/// the tables restrict it with CHECK constraints written in SQL. The two drifted once: a
/// `cli_tools_call` credential could be issued and used, and every one of its audit rows was
/// rejected by `bot_operation_logs_surface_check`. This walks the list through the real migrations
/// and the real write path so they cannot drift again.
#[cfg(test)]
#[allow(clippy::print_stderr)]
mod database_tests {
    use super::{BotOperationContext, EventSurface, record_operation};
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use uuid::Uuid;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));
        let name = format!("sylvode_bot_auth_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));
        let (prefix, _) = admin_url.rsplit_once('/')?;
        let db = Database::connect(&format!("{prefix}/{name}"))
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
        Some(Scratch { db, name, admin_url })
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@bot-auth.test").into()],
        ))
        .await
        .expect("user insert succeeds");
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'bot auth test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        ))
        .await
        .expect("workspace insert succeeds");
        workspace_id
    }

    async fn seed_bot(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        surface: EventSurface,
    ) -> Result<Uuid, sea_orm::DbErr> {
        let bot_id = Uuid::new_v4();
        let token_hash = format!("{:0>64}", bot_id.simple());
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, transport_surface) \
             VALUES ($1, $2, $3, $4, 'opr_test', $5)",
            vec![
                bot_id.into(),
                workspace_id.into(),
                format!("bot-{}", surface.as_wire()).into(),
                token_hash.into(),
                surface.as_wire().into(),
            ],
        ))
        .await
        .map(|_| bot_id)
    }

    fn operation(workspace_id: Uuid, bot_id: Uuid, surface: EventSurface) -> BotOperationContext {
        BotOperationContext {
            bot_id,
            workspace_id,
            tool_name: (surface != EventSurface::Rest).then(|| "labels.list".to_string()),
            surface,
            method: "GET".to_string(),
            path: format!("/api/v1/workspaces/{workspace_id}/labels"),
            request_id: Uuid::new_v4(),
        }
    }

    #[derive(FromQueryResult)]
    struct SurfaceRow {
        surface: String,
    }

    #[tokio::test]
    async fn every_surface_the_api_accepts_can_be_registered_and_audited() {
        let scratch = scratch_or_skip!("every_accepted_surface");
        let workspace_id = seed_workspace(&scratch.db).await;

        for surface in EventSurface::BOT_CREDENTIAL_SURFACES {
            let bot_id = seed_bot(&scratch.db, workspace_id, surface)
                .await
                .unwrap_or_else(|err| panic!("a `{}` credential must be storable: {err}", surface.as_wire()));
            record_operation(&scratch.db, operation(workspace_id, bot_id, surface), 0, None, 3)
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "the audit row of a `{}` operation must be writable: {err}",
                        surface.as_wire()
                    )
                });
        }

        let mut recorded: Vec<String> = SurfaceRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT surface FROM bot_operation_logs WHERE workspace_id = $1",
            vec![workspace_id.into()],
        ))
        .all(&scratch.db)
        .await
        .expect("operation rows are readable")
        .into_iter()
        .map(|row| row.surface)
        .collect();
        recorded.sort();
        let mut expected: Vec<String> = EventSurface::BOT_CREDENTIAL_SURFACES
            .iter()
            .map(|surface| surface.as_wire().to_string())
            .collect();
        expected.sort();
        assert_eq!(recorded, expected, "exactly one audit row per accepted surface");

        scratch.drop_self().await;
    }

    /// The other direction: the constraints must not admit surfaces no bot credential can carry,
    /// which would let a row claim to come from a browser session or from the server itself.
    #[tokio::test]
    async fn surfaces_no_bot_can_carry_are_refused_by_both_tables() {
        let scratch = scratch_or_skip!("refused_surfaces");
        let workspace_id = seed_workspace(&scratch.db).await;

        for surface in [EventSurface::Web, EventSurface::Worker, EventSurface::System] {
            assert!(
                seed_bot(&scratch.db, workspace_id, surface).await.is_err(),
                "`{}` must not be storable as a bot credential surface",
                surface.as_wire()
            );
            assert!(
                record_operation(
                    &scratch.db,
                    operation(workspace_id, Uuid::new_v4(), surface),
                    0,
                    None,
                    3
                )
                .await
                .is_err(),
                "`{}` must not be recordable as a bot operation surface",
                surface.as_wire()
            );
        }

        scratch.drop_self().await;
    }
}
