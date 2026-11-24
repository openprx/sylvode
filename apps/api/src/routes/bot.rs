// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements)]

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    response::ApiResponse,
    services::bots::{self, ManagedBy, ManagedPolicy, NewBot, normalize_bot_permissions, normalize_transport_surface},
};

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
    /// Set when a project bot connection manages this token; it cannot be revoked here.
    pub managed_by: Option<ManagedBy>,
    /// `workspace` (the token permissions apply to every project) or `projects` (a level per
    /// project, see `project-access`).
    pub project_scope: String,
    /// The level in every project without a grant: `none`, `read` or `comment`.
    pub default_project_level: String,
    /// The number of projects with a grant.
    pub grant_count: i64,
}

/// One project's level of a project-scoped bot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectGrant {
    pub project_id: Uuid,
    /// `read`, `comment` or `write`.
    pub level: String,
}

/// Body and answer of `project-access`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectAccess {
    /// `workspace` or `projects`.
    pub project_scope: String,
    /// `none`, `read` or `comment`: the level in every project that has no grant.
    pub default_level: String,
    pub grants: Vec<ProjectGrant>,
}

// ============================================================================
// Helpers
// ============================================================================

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
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

    let tx = state.db.begin().await?;
    let created = bots::create(
        &tx,
        NewBot {
            workspace_id,
            created_by: user_id,
            name: &req.name,
            permissions: perms,
            transport_surface,
            expires_at,
        },
    )
    .await?;
    tx.commit().await?;
    if created.epoch_advanced {
        crate::flow::collab::permission_cache::invalidate_workspace_after_commit(&state, workspace_id);
    }

    Ok(ApiResponse::success(CreateBotResponse {
        id: created.id,
        workspace_id,
        name: created.name,
        token: created.token,
        token_prefix: created.token_prefix,
        permissions: created.permissions,
        transport_surface: created.transport_surface,
        expires_at: created.expires_at.map(platform::timestamp::Timestamp::to_rfc3339),
        created_at: created.created_at.to_rfc3339(),
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
        managed: bool,
        project_scope: String,
        default_project_level: String,
        grant_count: i64,
    }

    let rows = BotRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT b.id, b.workspace_id, b.name, b.token_prefix, b.permissions, b.transport_surface,
                  b.is_active, b.last_used_at, b.expires_at, b.created_at,
                  (r.bot_id IS NOT NULL) AS managed,
                  b.project_scope, b.default_project_level,
                  (SELECT COUNT(*) FROM bot_project_grants g WHERE g.bot_id = b.id) AS grant_count
           FROM workspace_bots b
           LEFT JOIN bot_runners r ON r.bot_id = b.id
           WHERE b.workspace_id = $1
           ORDER BY b.created_at DESC",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    let mut response: Vec<BotResponse> = Vec::with_capacity(rows.len());
    for b in rows {
        let managed_by = if b.managed {
            Some(bots::managed_projects(&state.db, b.id).await?)
        } else {
            None
        };
        let perms: Vec<String> = b
            .permissions
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        response.push(BotResponse {
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
            managed_by,
            project_scope: b.project_scope,
            default_project_level: b.default_project_level,
            grant_count: b.grant_count,
        });
    }

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

    bots::revoke(&state.db, workspace_id, bot_id, ManagedPolicy::Refuse).await?;

    Ok(ApiResponse::ok())
}

/// Owners and admins only; the bot must belong to the workspace and not be managed by a project bot
/// connection (its reach is the connection's).
async fn authorize_access_admin(
    state: &AppState,
    extensions: &axum::http::Extensions,
    workspace_id: Uuid,
    bot_id: Uuid,
) -> Result<(), ApiError> {
    let (_, role, _) = require_workspace_access(state, extensions, workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can change a bot's project access".to_string(),
        ));
    }
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
    bots::ensure_editable(&state.db, bot_id, ManagedPolicy::Refuse).await
}

async fn load_project_access(db: &impl ConnectionTrait, bot_id: Uuid) -> Result<ProjectAccess, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct Scope {
        project_scope: String,
        default_project_level: String,
    }
    #[derive(Debug, FromQueryResult)]
    struct Grant {
        project_id: Uuid,
        level: String,
    }
    let scope = Scope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT project_scope, default_project_level FROM workspace_bots WHERE id = $1",
        vec![bot_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("bot not found".to_string()))?;
    let grants = Grant::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT project_id, level FROM bot_project_grants WHERE bot_id = $1 ORDER BY project_id",
        vec![bot_id.into()],
    ))
    .all(db)
    .await?;
    Ok(ProjectAccess {
        project_scope: scope.project_scope,
        default_level: scope.default_project_level,
        grants: grants
            .into_iter()
            .map(|grant| ProjectGrant {
                project_id: grant.project_id,
                level: grant.level,
            })
            .collect(),
    })
}

/// GET /`api/v1/workspaces/:workspace_id/bots/:bot_id/project-access`
pub async fn get_project_access(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, bot_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    authorize_access_admin(&state, &extensions, workspace_id, bot_id).await?;
    Ok(ApiResponse::success(load_project_access(&state.db, bot_id).await?))
}

/// Checks a body of `project-access` that needs no database: the names, the duplicates, and that a
/// workspace-wide bot carries no project level.
fn validate_project_access(request: &ProjectAccess) -> Result<(), ApiError> {
    let bad = |message: &str| ApiError::BadRequest(message.to_string());
    match request.project_scope.as_str() {
        "workspace" => {
            if request.default_level != "none" || !request.grants.is_empty() {
                return Err(bad(
                    "a workspace-wide bot has no project levels: default_level must be none and grants empty",
                ));
            }
        }
        "projects" => {
            if !matches!(request.default_level.as_str(), "none" | "read" | "comment") {
                return Err(bad("default_level must be none, read or comment"));
            }
        }
        _ => return Err(bad("project_scope must be workspace or projects")),
    }
    let mut seen = std::collections::BTreeSet::new();
    for grant in &request.grants {
        if !matches!(grant.level.as_str(), "read" | "comment" | "write") {
            return Err(bad("a grant level must be read, comment or write"));
        }
        if !seen.insert(grant.project_id) {
            return Err(bad("a project may appear only once in grants"));
        }
    }
    Ok(())
}

/// PUT /`api/v1/workspaces/:workspace_id/bots/:bot_id/project-access`
///
/// Replaces the scope, the default level and every grant in one transaction.
pub async fn put_project_access(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, bot_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ProjectAccess>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);
    authorize_access_admin(&state, &extensions, workspace_id, bot_id).await?;
    validate_project_access(&request)?;

    let tx = state.db.begin().await?;
    // Serialises two replacements of one bot and re-checks the bot inside the transaction.
    let locked = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
            vec![bot_id.into(), workspace_id.into()],
        ))
        .await?;
    if locked.is_none() {
        return Err(ApiError::NotFound("bot not found".to_string()));
    }
    bots::ensure_editable(&tx, bot_id, ManagedPolicy::Refuse).await?;

    if !request.grants.is_empty() {
        let ids: Vec<Uuid> = request.grants.iter().map(|grant| grant.project_id).collect();
        let known = tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM projects WHERE workspace_id = $1 AND id = ANY($2)",
                vec![workspace_id.into(), ids.clone().into()],
            ))
            .await?;
        if known.len() != ids.len() {
            return Err(ApiError::BadRequest(
                "grants name a project that is not in this workspace".to_string(),
            ));
        }
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE workspace_bots SET project_scope = $1, default_project_level = $2, updated_at = now() WHERE id = $3",
        vec![
            request.project_scope.clone().into(),
            request.default_level.clone().into(),
            bot_id.into(),
        ],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM bot_project_grants WHERE bot_id = $1",
        vec![bot_id.into()],
    ))
    .await?;
    for grant in &request.grants {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO bot_project_grants (bot_id, project_id, level, granted_by) VALUES ($1, $2, $3, $4)",
            vec![
                bot_id.into(),
                grant.project_id.into(),
                grant.level.clone().into(),
                actor.into(),
            ],
        ))
        .await?;
    }
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: "bot.project_access_changed".to_string(),
            aggregate_type: "bot".to_string(),
            aggregate_id: bot_id.to_string(),
            actor_id: Some(actor),
            source: serde_json::json!({ "type": "user", "actor_id": actor }),
            payload: serde_json::json!({
                "bot_id": bot_id,
                "project_scope": request.project_scope,
                "default_level": request.default_level,
                "grants": request.grants,
            }),
            metadata: serde_json::json!({}),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    let saved = load_project_access(&tx, bot_id).await?;
    tx.commit().await?;
    Ok(ApiResponse::success(saved))
}

/// Bots created through this API against a migrated scratch database.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod token_bot_identity_database_tests {
    use super::{CreateBotRequest, create_bot};
    use crate::routes::context::tenant_fixture::{count, exec, seed_tenant};
    use crate::scratch_or_skip;
    use crate::services::bots::TOKEN_BOT_AGENT_TYPE;
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
            "INSERT INTO ai_participants (id, project_id, name, registered_by) \
             VALUES ($1, $2, 'loop bot', $3)",
            vec![
                bot_id.to_string().into(),
                tenant.project_id.into(),
                tenant.member_id.into(),
            ],
        )
        .await;
        let queued = queue_vote_requested_tasks_for_project(
            db,
            crate::services::ai_task_service::DispatchOrigin::System,
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
                format!("{legacy}@bot.sylvode.local").into(),
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
