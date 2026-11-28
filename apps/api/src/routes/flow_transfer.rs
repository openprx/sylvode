//! Authenticated candidate transport for transfers between editable Pages.
use crate::middleware::bot_auth::BotAuthContext;
use crate::{error::ApiError, flow::page_transfers, response::ApiResponse};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use uuid::Uuid;

use crate::flow::page_transfer_contract::{CommitInput, PrepareInput, UndoInput};

async fn actor(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&Extension<BotAuthContext>>,
) -> Result<Uuid, ApiError> {
    let actor = super::flow_v2::user(claims, bot)?;
    if !crate::flow::scope::capabilities(&state.db)
        .await?
        .block_transfers_enabled
    {
        return Err(ApiError::unsupported_protocol("Page transfers are disabled"));
    }
    Ok(actor)
}

fn key(headers: &HeaderMap) -> Result<&str, ApiError> {
    let mut values = headers.get_all("idempotency-key").iter();
    let latest = values.next().and_then(|value| value.to_str().ok());
    match (latest, values.next()) {
        (Some(value), None) => Ok(value),
        _ => Err(ApiError::invalid_update("Expected one Idempotency-Key header")),
    }
}

pub async fn prepare(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<PrepareInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        page_transfers::prepare(
            &state.db,
            actor(&state, &claims, bot.as_ref()).await?,
            id,
            key(&headers)?,
            input,
        )
        .await?,
    ))
}

pub async fn commit(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<CommitInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        page_transfers::commit(
            &state.db,
            actor(&state, &claims, bot.as_ref()).await?,
            id,
            key(&headers)?,
            input,
        )
        .await?,
    ))
}

pub async fn status(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = actor(&state, &claims, bot.as_ref()).await?;
    // Prepare and status have distinct response shapes, even for the same prepared operation.
    Ok(ApiResponse::success(
        page_transfers::get_status(&state.db, actor, id).await?,
    ))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {}

pub async fn cancel(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(_): Json<CancelInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        page_transfers::cancel(
            &state.db,
            actor(&state, &claims, bot.as_ref()).await?,
            id,
            key(&headers)?,
        )
        .await?,
    ))
}

pub async fn undo(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<UndoInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        page_transfers::undo(
            &state.db,
            actor(&state, &claims, bot.as_ref()).await?,
            id,
            key(&headers)?,
            input,
        )
        .await?,
    ))
}
