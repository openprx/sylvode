//! Narrow Record-only context transport; identity is the same authenticated v2 principal.
use crate::{error::ApiError, middleware::bot_auth::BotAuthContext, response::ApiResponse};
use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use uuid::Uuid;
pub async fn get_flow_v2_record_context(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(record): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user = super::flow_v2::user(&claims, bot.as_ref())?;
    Ok(ApiResponse::success(
        crate::flow::scope_record_context::read(&state, record, user).await?,
    ))
}
