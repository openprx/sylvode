//! A project's bot connections: list, connect, read, update, disable, enable, rotate credentials
//! and remove, one connection per (project, bot).
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
        bot_connection_status::{self, ConnectionStatus},
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
    // Authorization first: for a project that does not exist the role check is false as well, so a
    // caller who may not manage connections cannot tell a missing project from one that exists.
    if !is_project_admin_or_owner(&state.db, project_id, actor).await? {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can manage bot connections".to_string(),
        ));
    }
    let project = execution_settings::project_ref(&state.db, project_id).await?;
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

/// The connection and its status, computed on read.
#[derive(Serialize)]
struct ConnectionAnswer<'a> {
    connection: &'a ConnectionView,
    status: ConnectionStatus,
}

#[derive(Serialize)]
struct BlockAnswer<'a> {
    connection: &'a ConnectionView,
    status: ConnectionStatus,
    config_block: Option<&'a ConfigBlock>,
}

async fn status_of(
    state: &AppState,
    project_key: &str,
    connection: &ConnectionView,
) -> Result<ConnectionStatus, ApiError> {
    bot_connection_status::compute(
        &state.db,
        connection.bot.id,
        project_key,
        &connection.execution.cli,
        &connection.execution.channel,
    )
    .await
}

async fn answer<'a>(
    state: &AppState,
    project_key: &str,
    connection: &'a ConnectionView,
) -> Result<ConnectionAnswer<'a>, ApiError> {
    Ok(ConnectionAnswer {
        status: status_of(state, project_key, connection).await?,
        connection,
    })
}

/// Who may read: owners and admins get the full view, other members the reduced one, everyone
/// else is refused. No existence check of its own: a project that does not exist has no members,
/// so the caller gets the same refusal as for a project they cannot see.
async fn reader(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&Extension<BotAuthContext>>,
    project_id: Uuid,
) -> Result<bool, ApiError> {
    refuse_machines(bot)?;
    let actor = user_id(claims)?;
    if is_project_admin_or_owner(&state.db, project_id, actor).await? {
        return Ok(true);
    }
    if !is_project_member(&state.db, project_id, actor).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    Ok(false)
}

/// `GET /api/v1/projects/{project_id}/bot-connections`: every connection, ordered by bot name.
///
/// Owners and admins get `{"connections": [{connection, status}]}`, other members
/// `{"connections": [MemberView]}`.
pub async fn list_bot_connections(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !reader(&state, &claims, bot.as_ref(), project_id).await? {
        let connections = bot_connections::list_member_views(&state.db, project_id).await?;
        return Ok(ApiResponse::success(json!({ "connections": connections })).into_response());
    }
    let project = execution_settings::project_ref(&state.db, project_id).await?;
    let views = bot_connections::list_views(&state.db, project_id).await?;
    let mut connections = Vec::with_capacity(views.len());
    for connection in &views {
        connections.push(json!({
            "connection": connection,
            "status": status_of(&state, &project.key, connection).await?,
        }));
    }
    Ok(ApiResponse::success(json!({ "connections": connections })).into_response())
}

/// `GET /api/v1/projects/{project_id}/bot-connections/{bot_id}`: one connection.
///
/// 404 when the project has none for the bot. Owners and admins get `{connection, status}`, other members
/// `{connection: MemberView}`.
pub async fn get_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, bot_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let missing = || ApiError::NotFound(bot_connections::NO_CONNECTION.to_string());
    if !reader(&state, &claims, bot.as_ref(), project_id).await? {
        let connection: MemberView = bot_connections::load_member_view(&state.db, project_id, bot_id)
            .await?
            .ok_or_else(missing)?;
        return Ok(ApiResponse::success(json!({ "connection": connection })).into_response());
    }
    let project = execution_settings::project_ref(&state.db, project_id).await?;
    let connection = bot_connections::load_view(&state.db, project_id, bot_id)
        .await?
        .ok_or_else(missing)?;
    Ok(ApiResponse::success(answer(&state, &project.key, &connection).await?).into_response())
}

/// `POST /api/v1/projects/{project_id}/bot-connections`
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
        status: status_of(&state, &project.key, &connected.connection).await?,
        connection: &connected.connection,
        config_block: connected.config_block.as_ref(),
    })))
}

/// `PATCH /api/v1/projects/{project_id}/bot-connections/{bot_id}`
pub async fn update_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, bot_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateRequest>,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::update(&state, &project, bot_id, actor, req).await? {
        Outcome::Done(connection) => {
            Ok(ApiResponse::success(answer(&state, &project.key, &connection).await?).into_response())
        }
        Outcome::Conflict { current } => Ok(version_conflict(&current)),
    }
}

/// The body of disable, enable and rotate.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionBody {
    pub expected_version: i32,
}

/// `POST /api/v1/projects/{project_id}/bot-connections/{bot_id}/disable`
pub async fn disable_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, bot_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<VersionBody>,
) -> Result<Response, ApiError> {
    set_enabled(state, claims, bot, (project_id, bot_id), body, false).await
}

/// `POST /api/v1/projects/{project_id}/bot-connections/{bot_id}/enable`
pub async fn enable_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, bot_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<VersionBody>,
) -> Result<Response, ApiError> {
    set_enabled(state, claims, bot, (project_id, bot_id), body, true).await
}

async fn set_enabled(
    state: AppState,
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    (project_id, bot_id): (Uuid, Uuid),
    body: VersionBody,
    enabled: bool,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::set_enabled(&state, &project, bot_id, actor, body.expected_version, enabled).await? {
        Outcome::Done(connection) => {
            Ok(ApiResponse::success(answer(&state, &project.key, &connection).await?).into_response())
        }
        Outcome::Conflict { current } => Ok(version_conflict(&current)),
    }
}

/// `POST /api/v1/projects/{project_id}/bot-connections/{bot_id}/rotate`
pub async fn rotate_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, bot_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<VersionBody>,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::rotate(
        &state,
        &project,
        bot_id,
        actor,
        body.expected_version,
        origin_of(&headers),
    )
    .await?
    {
        Outcome::Done(rotated) => Ok(no_store(ApiResponse::success(BlockAnswer {
            status: status_of(&state, &project.key, &rotated.connection).await?,
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

/// `DELETE /api/v1/projects/{project_id}/bot-connections/{bot_id}?expected_version=N`
pub async fn remove_bot_connection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, bot_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<RemoveQuery>,
) -> Result<Response, ApiError> {
    let (actor, project) = require_manager(&state, &claims, bot.as_ref(), project_id).await?;
    match bot_connections::remove(&state, &project, bot_id, actor, query.expected_version).await? {
        Outcome::Done(removed) => Ok(ApiResponse::success(removed).into_response()),
        Outcome::Conflict { current } => Ok(version_conflict(&current)),
    }
}
