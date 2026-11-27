//! Page appearance, version restore, trash, and mention candidates.
//!
//! Included from `routes::flow` so the flow-surface gate still sees `routes::flow::`.

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use platform::app::AppState;
use platform::auth::JwtClaims;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::flow::collab::authz::{self, PermissionLevel};
use crate::flow::policy;
use crate::flow::restore::{self, RestoreVersionRequest};
use crate::flow::trash;
use crate::middleware::bot_auth::BotAuthContext;
use crate::response::ApiResponse;

use super::{build_auth_extensions, request_origin};

#[derive(Debug, Deserialize)]
pub struct RestoreVersionBody {
    pub seq: i64,
    pub idempotency_key: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TrashQuery {
    /// Limits the listing to the archived objects of this project.
    pub project_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

pub async fn post_flow_restore_version(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(body): Json<RestoreVersionBody>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    require_level(
        &state,
        &access,
        PermissionLevel::Edit,
        "edit access is required to restore a version",
    )
    .await?;
    let extensions = build_auth_extensions(claims, bot);
    let restored = restore::restore_page_version(
        &state,
        access.workspace_id(),
        RestoreVersionRequest {
            object_id: access.object_id(),
            actor_id: access.actor_id(),
            actor_is_bot: access.principal_kind_wire() == "bot",
            checked_epoch: access.authz_epoch(),
            target_seq: body.seq,
            idempotency_key: body.idempotency_key,
            message: body.message,
            origin_client_id: format!("rest:{}", access.actor_id()),
            origin: request_origin(&extensions),
        },
    )
    .await?;
    Ok(ApiResponse::success(restored))
}

pub async fn get_flow_trash(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<TrashQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let access = policy::begin_flow_read(&state, &extensions, workspace_id).await?;
        let Some(page) =
            trash::list_trash(&state, &access, query.project_id, query.cursor.as_deref(), query.limit).await?
        else {
            continue;
        };
        return Ok(ApiResponse::success(page));
    }
    Err(policy::authorization_read_unstable())
}

pub async fn post_flow_purge(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    let purged = trash::purge_archived(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        access.principal_kind_wire(),
        access.actor_id(),
        access.role(),
    )
    .await?;
    Ok(ApiResponse::success(purged))
}

async fn authorize(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&Extension<BotAuthContext>>,
    object_id: Uuid,
    minimum: PermissionLevel,
) -> Result<policy::AuthorizedFlowObject, ApiError> {
    let extensions = build_auth_extensions(claims.clone(), bot.cloned());
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::not_found("flow object not found".to_string()))?;
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        if let Some(access) =
            policy::require_flow_object_access(state, &extensions, workspace_id, object_id, minimum).await?
        {
            return Ok(access);
        }
    }
    Err(policy::authorization_read_unstable())
}

async fn require_level(
    state: &AppState,
    access: &policy::AuthorizedFlowObject,
    minimum: PermissionLevel,
    message: &str,
) -> Result<(), ApiError> {
    let level = authz::effective_permission(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        access.principal_kind_wire(),
        access.actor_id(),
        access.role(),
    )
    .await?;
    if level < minimum {
        return Err(ApiError::policy_rejected(message.to_string()));
    }
    Ok(())
}
