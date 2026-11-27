//! Who may call the installation routes (contract C1 §6, SF-11).
//!
//! A caller who is not a member of the installation's workspace gets the ordinary `not_found` for
//! every route, so nothing confirms that an installation exists. Members who are not owner or admin
//! get `installation_admin_only` on the admin routes. Approve, reject, enable, disable, revoke and
//! the reads sit behind the JWT middleware, which no bot or delegated token passes; request, upgrade
//! and preview also admit a bot of the whole workspace with the `write` permission.

use axum::Extension;
use platform::{app::AppState, auth::JwtClaims};
use uuid::Uuid;

use super::store::{InstallationRow, load_installation};
use crate::catalogue::{Refusal, RefusalCode};
use crate::error::ApiError;
use crate::middleware::{
    bot_auth::{BotAuthContext, BotPermission, ensure_bot_permission, require_workspace_access_from_auth},
    scoped_bot::{ProjectFilter, readable_projects},
};

/// Who asks for an installation: recorded as `requested_by`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requester {
    pub kind: &'static str,
    pub id: Uuid,
}

fn admin_only() -> ApiError {
    Refusal::new(RefusalCode::InstallationAdminOnly, None).into()
}

fn not_found() -> ApiError {
    ApiError::not_found("installation not found")
}

/// A workspace owner or admin who is a person (a console session); returns the user id.
///
/// # Errors
/// `not_found` for a non-member, `installation_admin_only` for a member who is not an administrator.
pub async fn admin_user(state: &AppState, claims: &JwtClaims, workspace_id: Uuid) -> Result<Uuid, ApiError> {
    let (user_id, role, is_bot) = require_workspace_access_from_auth(state, claims, None, workspace_id)
        .await
        .map_err(|_| not_found())?;
    // A bot's seat in the workspace is a member row too: the identity decides, not the role.
    if is_bot
        || !matches!(role.as_str(), "owner" | "admin")
        || crate::services::bot_identity::is_bot_user(&state.db, user_id)
            .await
            .map_err(|_| ApiError::internal())?
    {
        return Err(admin_only());
    }
    Ok(user_id)
}

/// A workspace member who is a person; returns the user id and the role.
///
/// # Errors
/// `not_found` for a non-member.
pub async fn member(state: &AppState, claims: &JwtClaims, workspace_id: Uuid) -> Result<(Uuid, String), ApiError> {
    let (user_id, role, _) = require_workspace_access_from_auth(state, claims, None, workspace_id)
        .await
        .map_err(|_| not_found())?;
    Ok((user_id, role))
}

/// The caller of request, upgrade or preview: an owner or admin person, or a bot of the whole
/// workspace with the `write` permission.
///
/// Every other caller is refused (a project-scoped bot is stopped earlier by its route class; the
/// check here is the second line).
///
/// # Errors
/// `not_found` for a non-member, `installation_admin_only` for anyone else who may not.
pub async fn requester(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    workspace_id: Uuid,
) -> Result<Requester, ApiError> {
    let bot = bot.map(|Extension(context)| context);
    let (id, role, is_bot) = require_workspace_access_from_auth(state, claims, bot.as_ref(), workspace_id)
        .await
        .map_err(|_| not_found())?;
    if is_bot {
        ensure_bot_permission(bot.as_ref().ok_or_else(ApiError::internal)?, BotPermission::Write)
            .map_err(|_| admin_only())?;
        if !matches!(readable_projects(&state.db, id).await?, ProjectFilter::All) {
            return Err(admin_only());
        }
        return Ok(Requester { kind: "bot", id });
    }
    if matches!(role.as_str(), "owner" | "admin") {
        Ok(Requester { kind: "user", id })
    } else {
        Err(admin_only())
    }
}

/// Loads an installation for a caller who must be a member of its workspace.
///
/// # Errors
/// `not_found` when it does not exist or the caller is not a member.
pub async fn installation_for_member(
    state: &AppState,
    claims: &JwtClaims,
    installation_id: Uuid,
) -> Result<(InstallationRow, Uuid, String), ApiError> {
    let row = load_installation(&state.db, installation_id)
        .await?
        .ok_or_else(not_found)?;
    let (user_id, role) = member(state, claims, row.workspace_id).await?;
    Ok((row, user_id, role))
}

/// Loads an installation for an owner or admin person of its workspace.
///
/// # Errors
/// `not_found` when it does not exist or the caller is not a member, `installation_admin_only`
/// for a member who is not an administrator.
pub async fn installation_for_admin(
    state: &AppState,
    claims: &JwtClaims,
    installation_id: Uuid,
) -> Result<(InstallationRow, Uuid), ApiError> {
    let row = load_installation(&state.db, installation_id)
        .await?
        .ok_or_else(not_found)?;
    let user_id = admin_user(state, claims, row.workspace_id).await?;
    Ok((row, user_id))
}
