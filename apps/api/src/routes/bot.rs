// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements)]

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::timestamp::Timestamp;
use platform::{app::AppState, auth::JwtClaims};
use rand::Rng;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::middleware::bot_auth::{BotAuthContext, BotPermission, bot_permissions_allow, require_workspace_access};
use crate::{
    error::ApiError, flow::event_origin::EventSurface, response::ApiResponse, services::bot_identity::BOT_ENTITY_TYPE,
};

/// `users.agent_type` of a bot created through this API.
///
/// Such a bot acts through its `opr_` token, most often from an MCP client. Its entity type is the ordinary [`BOT_ENTITY_TYPE`], so it can be
/// assigned work, named as a webhook's bot user and registered as an AI participant like any
/// other bot.
pub const TOKEN_BOT_AGENT_TYPE: &str = "mcp";

// ============================================================================
// Request / Response types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateBotRequest {
    pub name: String,
    /// Permissions array, defaults to `["read"]`.
    ///
    /// `read` reaches every safe HTTP method, `write` adds the unsafe ones, `admin` additionally
    /// makes the token count as a workspace administrator. Each implies the ones before it, so
    /// `["write"]` and `["read","write"]` are the same token. An empty array is rejected rather
    /// than issued as a token that can do nothing.
    pub permissions: Option<Vec<String>>,
    /// Transport identity cryptographically bound to this token row.
    pub transport_surface: Option<String>,
    /// RFC3339 expiry, None = never
    pub expires_at: Option<String>,
}

/// Response for create — includes raw token (one-time only).
#[derive(Debug, Serialize)]
pub struct CreateBotResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub token: String, // raw token, returned ONCE
    pub token_prefix: String,
    pub permissions: Vec<String>,
    pub transport_surface: String,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// Response for list — never includes token or hash.
#[derive(Debug, Serialize)]
pub struct BotResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub permissions: Vec<String>,
    pub transport_surface: String,
    pub is_active: bool,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

// ============================================================================
// Helpers
// ============================================================================

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen::<u8>()).collect();
    format!("opr_{}", hex::encode(&bytes))
}

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

fn workspace_role_from_permissions(perms: &[String]) -> &'static str {
    if bot_permissions_allow(perms, BotPermission::Admin) {
        "admin"
    } else {
        "member"
    }
}

/// Validate and canonicalize the permission list a bot token will carry.
///
/// The stored array is what every request is judged against, so it has to be exactly the vocabulary
/// the enforcement side understands: unknown names grant nothing there and would silently produce an
/// inert token here. Duplicates are collapsed so the stored value reads the same as it behaves.
fn normalize_bot_permissions(permissions: Vec<String>) -> Result<Vec<String>, ApiError> {
    const VALID_PERMISSIONS: [&str; 3] = ["read", "write", "admin"];
    let mut normalized: Vec<String> = Vec::with_capacity(permissions.len());
    for permission in permissions {
        let permission = permission.trim();
        if !VALID_PERMISSIONS.contains(&permission) {
            return Err(ApiError::BadRequest(format!(
                "invalid permission '{permission}', must be one of: read, write, admin"
            )));
        }
        if !normalized.iter().any(|kept| kept == permission) {
            normalized.push(permission.to_string());
        }
    }
    if normalized.is_empty() {
        return Err(ApiError::BadRequest(
            "permissions must contain at least one of: read, write, admin".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_transport_surface(value: Option<String>) -> Result<String, ApiError> {
    let value = value.unwrap_or_else(|| "rest".to_string());
    let value = value.trim();
    EventSurface::from_bot_credential_label(value)
        .map(|surface| surface.as_wire().to_string())
        .ok_or_else(|| ApiError::BadRequest(format!("transport_surface must be {}", accepted_transport_surfaces())))
}

/// `rest, mcp_http, ..., or cli_tools_call`, spelled from the one accepted-surface list.
fn accepted_transport_surfaces() -> String {
    let mut text = String::new();
    let last = EventSurface::BOT_CREDENTIAL_SURFACES.len().saturating_sub(1);
    for (index, surface) in EventSurface::BOT_CREDENTIAL_SURFACES.iter().enumerate() {
        if index > 0 {
            text.push_str(if index == last { ", or " } else { ", " });
        }
        text.push_str(surface.as_wire());
    }
    text
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /`api/v1/workspaces/:workspace_id/bots`
pub async fn create_bot(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateBotRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }

    // Only owners/admins can create bots
    let (_, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can create bots".to_string(),
        ));
    }

    let perms = normalize_bot_permissions(req.permissions.unwrap_or_else(|| vec!["read".to_string()]))?;
    let transport_surface = normalize_transport_surface(req.transport_surface)?;

    let expires_at: Option<platform::timestamp::Timestamp> = match req.expires_at {
        Some(ref s) => Some(
            platform::timestamp::Timestamp::parse_rfc3339(s)
                .map_err(|_| ApiError::BadRequest("invalid expires_at format".to_string()))?,
        ),
        None => None,
    };

    let raw_token = generate_token();
    let token_hash = sha256_hex(&raw_token);
    // token_prefix: first 8 chars, e.g. "opr_a1b2"
    let token_prefix: String = raw_token.chars().take(8).collect();

    let bot_id = Uuid::new_v4();
    let now = Timestamp::now();
    let perms_json = serde_json::to_value(&perms).unwrap_or_else(|_| serde_json::json!(["read"]));
    let workspace_role = workspace_role_from_permissions(&perms);
    let bot_email = format!("{bot_id}@bot.openpr.local");
    let bot_name = req.name.clone();

    let tx = state.db.begin().await?;
    let advanced_epoch = crate::flow::collab::authz::advance_epoch_if_present(&tx, workspace_id).await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO users
           (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $8)",
        vec![
            bot_id.into(),
            bot_email.into(),
            "!".into(),
            bot_name.clone().into(),
            "user".into(),
            BOT_ENTITY_TYPE.into(),
            TOKEN_BOT_AGENT_TYPE.into(),
            now.into(),
        ],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
           VALUES ($1, $2, $3, $4)",
        vec![workspace_id.into(), bot_id.into(), workspace_role.into(), now.into()],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO workspace_bots
           (id, workspace_id, name, token_hash, token_prefix, permissions, transport_surface,
            created_by, expires_at, is_active, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, true, $10, $10)",
        vec![
            bot_id.into(),
            workspace_id.into(),
            bot_name.clone().into(),
            token_hash.into(),
            token_prefix.clone().into(),
            perms_json.into(),
            transport_surface.clone().into(),
            user_id.into(),
            expires_at.into(),
            now.into(),
        ],
    ))
    .await?;

    tx.commit().await?;
    if advanced_epoch.is_some() {
        crate::flow::collab::permission_cache::invalidate_workspace_after_commit(&state, workspace_id);
    }

    Ok(ApiResponse::success(CreateBotResponse {
        id: bot_id,
        workspace_id,
        name: bot_name,
        token: raw_token,
        token_prefix,
        permissions: perms,
        transport_surface,
        expires_at: expires_at.map(platform::timestamp::Timestamp::to_rfc3339),
        created_at: now.to_rfc3339(),
    }))
}

/// GET /`api/v1/workspaces/:workspace_id/bots`
pub async fn list_bots(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can list bots".to_string(),
        ));
    }

    #[derive(Debug, FromQueryResult)]
    struct BotRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        token_prefix: String,
        permissions: serde_json::Value,
        transport_surface: String,
        is_active: bool,
        last_used_at: Option<platform::timestamp::Timestamp>,
        expires_at: Option<platform::timestamp::Timestamp>,
        created_at: platform::timestamp::Timestamp,
    }

    let bots = BotRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, name, token_prefix, permissions, transport_surface,
                  is_active, last_used_at, expires_at, created_at
           FROM workspace_bots
           WHERE workspace_id = $1
           ORDER BY created_at DESC",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<BotResponse> = bots
        .into_iter()
        .map(|b| {
            let perms: Vec<String> = b
                .permissions
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            BotResponse {
                id: b.id,
                workspace_id: b.workspace_id,
                name: b.name,
                token_prefix: b.token_prefix,
                permissions: perms,
                transport_surface: b.transport_surface,
                is_active: b.is_active,
                last_used_at: b.last_used_at.map(platform::timestamp::Timestamp::to_rfc3339),
                expires_at: b.expires_at.map(platform::timestamp::Timestamp::to_rfc3339),
                created_at: b.created_at.to_rfc3339(),
            }
        })
        .collect();

    Ok(ApiResponse::success(response))
}

/// DELETE /`api/v1/workspaces/:workspace_id/bots/:bot_id`
pub async fn revoke_bot(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, bot_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can revoke bots".to_string(),
        ));
    }

    // Verify bot belongs to this workspace
    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND workspace_id = $2",
            vec![bot_id.into(), workspace_id.into()],
        ))
        .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound("bot not found".to_string()));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE workspace_bots SET is_active = false, updated_at = $1 WHERE id = $2",
            vec![Timestamp::now().into(), bot_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

#[cfg(test)]
mod tests {
    use super::{normalize_bot_permissions, normalize_transport_surface, workspace_role_from_permissions};
    use crate::{error::ApiError, flow::event_origin::EventSurface};

    #[test]
    fn transport_surface_accepts_exactly_the_bot_credential_surfaces() {
        assert_eq!(normalize_transport_surface(None).expect("default"), "rest");
        for surface in EventSurface::BOT_CREDENTIAL_SURFACES {
            assert_eq!(
                normalize_transport_surface(Some(format!(" {} ", surface.as_wire()))).expect("accepted surface"),
                surface.as_wire()
            );
        }
        for refused in ["web", "worker", "system", "MCP_HTTP", ""] {
            match normalize_transport_surface(Some(refused.to_string())) {
                Err(ApiError::BadRequest(message)) => assert_eq!(
                    message,
                    "transport_surface must be rest, mcp_http, mcp_sse, mcp_stdio, cli, or cli_tools_call"
                ),
                other => panic!("`{refused}` must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn permissions_are_canonicalized_and_deduplicated() {
        let normalized = normalize_bot_permissions(vec![" read ".to_string(), "write".to_string(), "read".to_string()])
            .expect("known permissions are accepted");

        assert_eq!(normalized, vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn a_token_that_could_do_nothing_is_not_issued() {
        assert!(normalize_bot_permissions(Vec::new()).is_err());
        assert!(normalize_bot_permissions(vec!["Read".to_string()]).is_err());
        assert!(normalize_bot_permissions(vec!["owner".to_string()]).is_err());
    }

    #[test]
    fn only_the_admin_permission_seats_the_bot_as_a_workspace_admin() {
        assert_eq!(workspace_role_from_permissions(&["admin".to_string()]), "admin");
        assert_eq!(
            workspace_role_from_permissions(&["read".to_string(), "write".to_string()]),
            "member"
        );
    }
}

/// Bots created through this API against a migrated scratch database.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod token_bot_identity_database_tests {
    use super::{CreateBotRequest, TOKEN_BOT_AGENT_TYPE, create_bot};
    use crate::routes::context::tenant_fixture::{count, exec, seed_tenant};
    use crate::scratch_or_skip;
    use crate::services::{ai_task_service::queue_vote_requested_tasks_for_project, bot_identity::is_bot_user};
    use axum::extract::{Extension, Json, Path, State};
    use axum::response::IntoResponse;
    use platform::auth::{JwtClaims, TokenType};
    use uuid::Uuid;

    fn state_for(db: sea_orm::DatabaseConnection) -> platform::app::AppState {
        use platform::config::{AppConfig, Secret};
        platform::app::AppState {
            cfg: AppConfig {
                app_name: "token-bot-identity-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("token-bot-identity-test-secret"),
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

    /// A bot made with a token is an ordinary bot: it can be assigned work, named as a
    /// webhook's bot user and act as a project's AI participant, with no administrator edit.
    #[tokio::test]
    async fn a_token_bot_is_a_bot_identity() {
        let scratch = scratch_or_skip!("token_bot_identity");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "tb").await;
        exec(
            db,
            "UPDATE workspace_members SET role = 'owner' WHERE workspace_id = $1 AND user_id = $2",
            vec![tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let claims = JwtClaims {
            sub: tenant.member_id.to_string(),
            email: format!("{}@tenant.test", tenant.member_id),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
            sv: 0,
        };
        let response = create_bot(
            State(state_for(db.clone())),
            Extension(claims),
            None,
            Path(tenant.workspace_id),
            Json(CreateBotRequest {
                name: "loop bot".to_string(),
                permissions: Some(vec!["write".to_string()]),
                transport_surface: Some("mcp_stdio".to_string()),
                expires_at: None,
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("create_bot failed: {err:?}"))
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let bot_id = body
            .pointer("/data/id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .expect("the response names the bot");

        assert!(
            is_bot_user(db, bot_id).await.expect("lookup"),
            "assignable and usable as bot_user_id"
        );
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM users WHERE id = $1 AND entity_type = 'bot' AND agent_type = $2",
                vec![bot_id.into(), TOKEN_BOT_AGENT_TYPE.into()],
            )
            .await,
            1
        );

        exec(
            db,
            "INSERT INTO ai_participants (id, project_id, name, model, provider, capabilities, registered_by) \
             VALUES ($1, $2, 'loop bot', 'claude-code', 'anthropic', '{}'::jsonb, $3)",
            vec![
                bot_id.to_string().into(),
                tenant.project_id.into(),
                tenant.member_id.into(),
            ],
        )
        .await;
        let queued = queue_vote_requested_tasks_for_project(
            db,
            tenant.project_id,
            &Uuid::new_v4().to_string(),
            serde_json::json!({}),
        )
        .await
        .expect("vote tasks queue");
        assert_eq!(queued, 1, "the token bot is an AI participant of the project");
        scratch.drop_self().await;
    }

    /// 0105 moves users created by the old bots API to `bot` and keeps everything else.
    #[tokio::test]
    async fn the_migration_turns_existing_token_users_into_bots() {
        let scratch = scratch_or_skip!("token_bot_migration");
        let db = &scratch.db;
        let legacy = Uuid::new_v4();
        let human = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type) \
             VALUES ($1, $2, '!', 'legacy', 'user', true, 'bot_mcp', 'mcp'), \
                    ($3, $4, '!', 'person', 'user', true, 'human', NULL)",
            vec![
                legacy.into(),
                format!("{legacy}@bot.openpr.local").into(),
                human.into(),
                format!("{human}@tenant.test").into(),
            ],
        )
        .await;
        let sql = include_str!("../../../../migrations/0105_token_bot_entity_type.sql");
        for _ in 0..2 {
            sea_orm::ConnectionTrait::execute_unprepared(db, sql)
                .await
                .expect("the migration runs, and runs again as a no-op");
        }
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM users WHERE id = $1 AND entity_type = 'bot' AND agent_type = 'mcp'",
                vec![legacy.into()],
            )
            .await,
            1
        );
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM users WHERE id = $1 AND entity_type = 'human'",
                vec![human.into()],
            )
            .await,
            1
        );
        scratch.drop_self().await;
    }
}
