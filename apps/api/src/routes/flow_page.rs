//! Page appearance, version restore, trash, and mention candidates.
//!
//! Included from `routes::flow` so the flow-surface gate still sees `routes::flow::`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use platform::app::AppState;
use platform::auth::JwtClaims;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::flow::appearance::{self, PageAppearance, PageCover, PageIcon};
use crate::flow::collab::authz::{self, PermissionLevel};
use crate::flow::comments;
use crate::flow::policy;
use crate::flow::restore::{self, RestoreVersionRequest};
use crate::flow::shares;
use crate::flow::trash;
use crate::middleware::bot_auth::BotAuthContext;
use crate::response::ApiResponse;

use super::{ShareClientIp, build_auth_extensions, request_origin};

#[derive(Debug, Deserialize)]
pub struct AssetPath {
    pub object_id: Uuid,
    pub asset_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct PublicAssetPath {
    pub token: String,
    pub asset_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct AppearanceBody {
    #[serde(default)]
    pub icon: Option<PageIcon>,
    #[serde(default)]
    pub cover: Option<PageCover>,
}

#[derive(Debug, Deserialize)]
pub struct UploadAppearanceBody {
    pub data_base64: String,
}

#[derive(Debug, Deserialize)]
pub struct RestoreVersionBody {
    pub seq: i64,
    pub idempotency_key: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TrashQuery {
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct MentionQuery {
    #[serde(default)]
    pub q: String,
}

pub async fn put_flow_appearance(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(body): Json<AppearanceBody>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    require_level(
        &state,
        &access,
        PermissionLevel::Edit,
        "edit access is required to change the page header",
    )
    .await?;
    let appearance = appearance::set_appearance(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        access.actor_id(),
        access.principal_kind_wire() == "bot",
        &PageAppearance {
            icon: body.icon,
            cover: body.cover,
        },
    )
    .await?;
    Ok(ApiResponse::success(appearance))
}

pub async fn post_flow_appearance_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(body): Json<UploadAppearanceBody>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    require_level(
        &state,
        &access,
        PermissionLevel::Edit,
        "edit access is required to upload a page image",
    )
    .await?;
    let created = appearance::upload_asset(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        access.actor_id(),
        access.principal_kind_wire() == "bot",
        &body.data_base64,
    )
    .await?;
    Ok(ApiResponse::success(created))
}

pub async fn get_flow_appearance_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(path): Path<AssetPath>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), path.object_id, PermissionLevel::View).await?;
    let asset = appearance::read_asset(&state.db, access.workspace_id(), access.object_id(), path.asset_id).await?;
    Ok(image_response(asset))
}

pub async fn get_public_flow_appearance_asset(
    State(state): State<AppState>,
    Path(path): Path<PublicAssetPath>,
    ShareClientIp(ip): ShareClientIp,
) -> Result<impl IntoResponse, ApiError> {
    shares::enforce_public_share_rate_limit(&state.db, ip).await?;
    let asset = shares::read_public_asset(&state.db, &path.token, path.asset_id).await?;
    Ok(image_response(asset))
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
        let Some(page) = trash::list_trash(&state, &access, query.cursor.as_deref(), query.limit).await? else {
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

pub async fn get_flow_mention_candidates(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Query(query): Query<MentionQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    require_level(
        &state,
        &access,
        PermissionLevel::Comment,
        "comment access is required to mention someone",
    )
    .await?;
    let listed =
        comments::list_mention_candidates(&state.db, access.workspace_id(), access.object_id(), &query.q).await?;
    Ok(ApiResponse::success(listed))
}

fn image_response(asset: appearance::AppearanceAssetBytes) -> axum::response::Response {
    let mut response = axum::response::Response::new(axum::body::Body::from(asset.bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
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
