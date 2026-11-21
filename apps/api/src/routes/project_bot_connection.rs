//! A project's bot connection: connect, read, update, disable, enable, rotate credentials, remove.
//!
//! Owners and admins of the project's workspace manage the connection; members read a reduced
//! view. A bot is refused whatever permissions it holds, and so is an OAuth delegated caller:
//! the connection decides where a command line agent runs and issues the credentials it runs
//! with, so "may call the API" must never mean "may connect a bot". None of these routes is on
//! the delegated allow-list.
//!
//! The answers that carry a configuration block set `Cache-Control: no-store`.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use platform::{app::AppState, auth::JwtClaims};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::ApiResponse,
    services::{
        bot_connections::{self, ConnectRequest, ConnectionView, MemberView, Outcome, UpdateRequest, VERSION_CONFLICT},
        execution_settings::{self, ProjectRef},
        runner_config_block::ConfigBlock,
        trust_score_service::{is_project_admin_or_owner, is_project_member},
    },
};

fn user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

/// Bots and delegated callers never reach a connection, for reading or writing.
fn refuse_machines(bot: Option<&Extension<BotAuthContext>>) -> Result<(), ApiError> {
    if bot.is_some() || crate::oauth::delegated::current().is_some() {
        return Err(ApiError::Forbidden("bots cannot manage bot connections".to_string()));
    }
    Ok(())
}

/// The person and the project of a write: an owner or admin of the project's workspace.
async fn require_manager(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&Extension<BotAuthContext>>,
    project_id: Uuid,
) -> Result<(Uuid, ProjectRef), ApiError> {
    refuse_machines(bot)?;
    let actor = user_id(claims)?;
    let project = execution_settings::project_ref(&state.db, project_id).await?;
    if !is_project_admin_or_owner(&state.db, project_id, actor).await? {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can manage bot connections".to_string(),
        ));
    }
    Ok((actor, project))
}

/// The request's `Origin`, the fallback for the block's `api_url`.
fn origin_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::ORIGIN).and_then(|value| value.to_str().ok())
}

/// A body that carries a secret must not be stored by any cache.
fn no_store(body: impl IntoResponse) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The envelope of a stale version: business code 409 with the current connection.
fn version_conflict(current: &ConnectionView) -> Response {
    Json(ApiResponse {
        code: 409,
        message: VERSION_CONFLICT.to_string(),
        data: Some(json!({ "version": current.version, "current": current })),
        error_code: None,
        details: None,
    })
    .into_response()
}

#[derive(Serialize)]
struct ConnectionAnswer<'a> {
    connection: &'a ConnectionView,
}

#[derive(Serialize)]
struct BlockAnswer<'a> {
    connection: &'a ConnectionView,
    config_block: Option<&'a ConfigBlock>,
}

/// `GET /api/v1/projects/{project_id}/bot-connection`
pub async fn get_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    refuse_machines(bot.as_ref())?;
    let actor = user_id(&claims)?;
    execution_settings::project_ref(&state.db, project_id).await?;
    if is_project_admin_or_owner(&state.db, project_id, actor).await? {
        let connection = bot_connections::load_view(&state.db, project_id).await?;
        return Ok(ApiResponse::success(json!({ "connection": connection })).into_response());
    }
    if !is_project_member(&state.db, project_id, actor).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    let connection: Option<MemberView> = bot_connections::load_member_view(&state.db, project_id).await?;
    Ok(ApiResponse::success(json!({ "connection": connection })).into_response())
}

/// `POST /api/v1/projects/{project_id}/bot-connection`
pub async fn connect_bot(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<ConnectRequest>,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    let connected = bot_connections::connect(&state, &project, actor, req, origin_of(&headers)).await?;
    Ok(no_store(ApiResponse::success(BlockAnswer {
        connection: &connected.connection,
        config_block: connected.config_block.as_ref(),
    })))
}

/// `PATCH /api/v1/projects/{project_id}/bot-connection`
pub async fn update_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<UpdateRequest>,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::update(&state, &project, actor, req).await? {
        Outcome::Done(connection) => Ok(ApiResponse::success(ConnectionAnswer {
            connection: &connection,
        })
        .into_response()),
        Outcome::Conflict { current } => Ok(version_conflict(&current)),
    }
}

/// The body of disable, enable and rotate.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionBody {
    pub expected_version: i32,
}

/// `POST /api/v1/projects/{project_id}/bot-connection/disable`
pub async fn disable_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<VersionBody>,
) -> Result<Response, ApiError> {
    set_enabled(state, claims, bot, project_id, body, false).await
}

/// `POST /api/v1/projects/{project_id}/bot-connection/enable`
pub async fn enable_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<VersionBody>,
) -> Result<Response, ApiError> {
    set_enabled(state, claims, bot, project_id, body, true).await
}

async fn set_enabled(
    state: AppState,
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    project_id: Uuid,
    body: VersionBody,
    enabled: bool,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::set_enabled(&state, &project, actor, body.expected_version, enabled).await? {
        Outcome::Done(connection) => Ok(ApiResponse::success(ConnectionAnswer {
            connection: &connection,
        })
        .into_response()),
        Outcome::Conflict { current } => Ok(version_conflict(&current)),
    }
}

/// `POST /api/v1/projects/{project_id}/bot-connection/rotate`
pub async fn rotate_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<VersionBody>,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::rotate(&state, &project, actor, body.expected_version, origin_of(&headers)).await? {
        Outcome::Done(rotated) => Ok(no_store(ApiResponse::success(BlockAnswer {
            connection: &rotated.connection,
            config_block: Some(&rotated.config_block),
        }))),
        Outcome::Conflict { current } => Ok(version_conflict(&current)),
    }
}

#[derive(Debug, Deserialize)]
pub struct RemoveQuery {
    pub expected_version: i32,
}

/// `DELETE /api/v1/projects/{project_id}/bot-connection?expected_version=N`
pub async fn remove_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<RemoveQuery>,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::remove(&state, &project, actor, query.expected_version).await? {
        Outcome::Done(removed) => Ok(ApiResponse::success(removed).into_response()),
        Outcome::Conflict { current } => Ok(version_conflict(&current)),
    }
}
