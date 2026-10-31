//! Comment threads and public share links for Flow objects.
//!
//! Handlers stay in `routes::flow` via `#[path]` so the flow-surface gate still sees
//! `routes::flow::`. The anonymous share read hashes the token, hides revoked and expired links,
//! and rate-limits per client address. It is registered without auth middleware on purpose.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use platform::app::AppState;
use platform::auth::JwtClaims;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::flow::collab::authz::{self, PermissionLevel};
use crate::flow::comments::{self, CommentActor, NewComment};
use crate::flow::policy;
use crate::flow::shares;
use crate::middleware::bot_auth::BotAuthContext;
use crate::response::ApiResponse;

use super::build_auth_extensions;

#[derive(Clone, Copy)]
/// Client address for the public-share limiter.
///
/// `axum::serve(...into_make_service_with_connect_info)` inserts [`ConnectInfo`]. A caller that
/// reaches the handler without it (direct tests) shares the unspecified address.
pub struct ShareClientIp(pub IpAddr);

impl<S> FromRequestParts<S> for ShareClientIp
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |info| info.0.ip());
        Ok(Self(ip))
    }
}

#[derive(Debug, Deserialize)]
pub struct CommentPath {
    pub object_id: Uuid,
    pub comment_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ShareLinkPath {
    pub object_id: Uuid,
    pub link_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct TokenPath {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateCommentBody {
    body: String,
    #[serde(default)]
    block_id: Option<String>,
    #[serde(default)]
    parent_id: Option<Uuid>,
    #[serde(default)]
    mentions: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct PatchCommentBody {
    body: String,
    #[serde(default)]
    mentions: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveCommentBody {
    resolved: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateShareLinkBody {
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

pub async fn get_flow_comments(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    let listed = comments::list_comments(&state.db, access.object_id()).await?;
    Ok(ApiResponse::success(listed))
}

pub async fn post_flow_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(body): Json<CreateCommentBody>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::Comment).await?;
    let actor = actor_of(&state, &access).await?;
    let created = comments::create_comment(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        &actor,
        NewComment {
            body: body.body,
            block_id: body.block_id,
            parent_id: body.parent_id,
            mentions: body.mentions,
        },
    )
    .await?;
    Ok(ApiResponse::success(created))
}

pub async fn patch_flow_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(path): Path<CommentPath>,
    Json(body): Json<PatchCommentBody>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), path.object_id, PermissionLevel::Comment).await?;
    let actor = actor_of(&state, &access).await?;
    let updated = comments::update_comment(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        path.comment_id,
        &actor,
        &body.body,
        body.mentions,
    )
    .await?;
    Ok(ApiResponse::success(updated))
}

pub async fn delete_flow_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(path): Path<CommentPath>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), path.object_id, PermissionLevel::Comment).await?;
    let actor = actor_of(&state, &access).await?;
    comments::delete_comment(&state.db, access.object_id(), path.comment_id, &actor).await?;
    Ok(ApiResponse::success(serde_json::json!({ "deleted": true })))
}

pub async fn resolve_flow_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(path): Path<CommentPath>,
    Json(body): Json<ResolveCommentBody>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), path.object_id, PermissionLevel::Comment).await?;
    let actor = actor_of(&state, &access).await?;
    let updated = comments::set_resolved(&state.db, access.object_id(), path.comment_id, &actor, body.resolved).await?;
    Ok(ApiResponse::success(updated))
}

pub async fn list_flow_share_links(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    let level = permission_level(&state, &access).await?;
    let listed = shares::list_links(&state.db, access.object_id(), level).await?;
    Ok(ApiResponse::success(listed))
}

pub async fn post_flow_share_link(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(body): Json<CreateShareLinkBody>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), object_id, PermissionLevel::View).await?;
    let level = permission_level(&state, &access).await?;
    let created = shares::create_link(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        access.actor_id(),
        level,
        body.expires_at,
    )
    .await?;
    Ok(ApiResponse::success(created))
}

pub async fn revoke_flow_share_link(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(path): Path<ShareLinkPath>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorize(&state, &claims, bot.as_ref(), path.object_id, PermissionLevel::View).await?;
    let level = permission_level(&state, &access).await?;
    let revoked = shares::revoke_link(&state.db, access.object_id(), path.link_id, level).await?;
    Ok(ApiResponse::success(revoked))
}

/// Anonymous read of one page. Missing, revoked, expired, and disabled links all answer `not_found`.
pub async fn get_public_flow_share(
    State(state): State<AppState>,
    Path(path): Path<TokenPath>,
    ShareClientIp(ip): ShareClientIp,
) -> Result<impl IntoResponse, ApiError> {
    shares::enforce_public_share_rate_limit(ip)?;
    let view = shares::read_public(&state.db, &path.token).await?;
    Ok(ApiResponse::success(view))
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

async fn permission_level(
    state: &AppState,
    access: &policy::AuthorizedFlowObject,
) -> Result<PermissionLevel, ApiError> {
    authz::effective_permission(
        &state.db,
        access.workspace_id(),
        access.object_id(),
        access.principal_kind_wire(),
        access.actor_id(),
        access.role(),
    )
    .await
}

async fn actor_of(state: &AppState, access: &policy::AuthorizedFlowObject) -> Result<CommentActor, ApiError> {
    Ok(CommentActor {
        user_id: access.actor_id(),
        level: permission_level(state, access).await?,
    })
}
