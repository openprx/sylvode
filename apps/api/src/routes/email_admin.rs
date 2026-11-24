//! Instance administration of e-mail delivery.

use axum::{Extension, extract::State, response::IntoResponse};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::ConnectionTrait;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::ApiResponse,
    services::{email_text, notifications},
};

#[derive(Debug, Serialize)]
pub struct TestEmailResponse {
    queued: bool,
}

/// `POST /api/v1/admin/email/test`: queues a test e-mail to the caller's own address, in the
/// language saved with the caller's notification preferences.
///
/// The worker sends it like any other e-mail, so the answer says that the message is queued, not
/// that it arrived; a failure is recorded on the outbox row (see `docs/operations.md`).
///
/// # Errors
/// 409 `feature_disabled` when `[email]` is not enabled.
pub async fn send_test_email(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    if crate::config::runtime().email.is_none() {
        return Err(ApiError::conflict_coded(
            "feature_disabled",
            "e-mail delivery is not set up on this server",
            None,
        ));
    }
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid access token".to_string()))?;
    queue_test_email(&state.db, user_id).await?;
    Ok(ApiResponse::success(TestEmailResponse { queued: true }))
}

/// Queues the test e-mail for `user_id`, in that user's language.
pub async fn queue_test_email<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<(), ApiError> {
    let language = notifications::language_of(db, user_id).await?;
    notifications::queue_email(db, user_id, "test", None, &email_text::test_email(language)).await
}
