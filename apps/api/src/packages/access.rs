//! Who may use the package routes (contract C3 section 4.4).
//!
//! Uploads and publisher keys are for people with a console session only: those routes sit behind
//! the JWT middleware, which no bot or delegated token passes. Reads also admit a bot of the
//! whole workspace (a project-scoped bot is refused by its route class).

use axum::Extension;
use platform::{app::AppState, auth::JwtClaims};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::{BotAuthContext, require_workspace_access_from_auth},
};

/// The caller of a package route, known to be a member of the path's workspace (a bot of that
/// workspace for reads). A non-member gets `not_found`.
pub(super) struct Caller {
    pub(super) user_id: Uuid,
    role: String,
    is_bot: bool,
}

impl Caller {
    pub(super) async fn resolve(
        state: &AppState,
        claims: &JwtClaims,
        bot: Option<Extension<BotAuthContext>>,
        workspace_id: Uuid,
    ) -> Result<Self, ApiError> {
        let bot = bot.map(|Extension(ctx)| ctx);
        let (user_id, role, is_bot) =
            require_workspace_access_from_auth(state, claims, bot.as_ref(), workspace_id).await?;
        Ok(Self { user_id, role, is_bot })
    }

    /// A workspace owner or admin who is a person.
    pub(super) fn require_admin(&self) -> Result<(), ApiError> {
        if !self.is_bot && matches!(self.role.as_str(), "owner" | "admin") {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "only workspace owners and admins can manage packages",
            ))
        }
    }

    /// The workspace owner, a person.
    pub(super) fn require_owner(&self) -> Result<(), ApiError> {
        if !self.is_bot && self.role == "owner" {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "only the workspace owner can manage publisher keys",
            ))
        }
    }
}
