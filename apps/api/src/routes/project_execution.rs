//! Read a project's execution settings.
//!
//! Reading is open to the project's members and to the bots of its workspace. The settings are
//! written only by the project's bot connection (`routes::project_bot_connection`): a bot that
//! could repoint its own runner's working directory would turn "may call the API" into "may
//! choose where a command line agent runs".

use axum::{Extension, extract::Path, extract::State, response::IntoResponse};
use platform::{app::AppState, auth::JwtClaims};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::ApiResponse,
    services::{execution_settings, trust_score_service::is_project_member},
};

fn user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

/// `GET /api/v1/projects/{project_id}/execution-settings`: the settings, or `null` when the
/// project has none.
pub async fn get_execution_settings(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let project = execution_settings::project_ref(&state.db, project_id).await?;
    if let Some(Extension(bot)) = bot {
        if bot.workspace_id != project.workspace_id {
            return Err(ApiError::Forbidden("bot not authorized for this project".to_string()));
        }
    } else if !is_project_member(&state.db, project_id, user_id(&claims)?).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    let settings = execution_settings::load(&state.db, project_id).await?;
    Ok(ApiResponse::success(json!(settings)))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::get_execution_settings;
    use crate::error::ApiError;
    use crate::routes::context::tenant_fixture::{
        bot_context_for, claims_for, exec, seed_bot_user, seed_member, seed_tenant, state_for,
    };
    use crate::scratch_or_skip;
    use axum::{
        Extension,
        extract::{Path, State},
        response::IntoResponse,
    };
    use sea_orm::DatabaseConnection;
    use serde_json::Value;
    use uuid::Uuid;

    async fn get(
        db: &DatabaseConnection,
        user: Uuid,
        bot: Option<crate::middleware::bot_auth::BotAuthContext>,
        project: Uuid,
    ) -> Result<Value, ApiError> {
        let response = get_execution_settings(
            State(state_for(db.clone())),
            Extension(claims_for(user)),
            bot.map(Extension),
            Path(project),
        )
        .await?
        .into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads");
        Ok(serde_json::from_slice(&bytes).expect("the body is JSON"))
    }

    /// Settings are inserted directly: the connection service is their only writer.
    #[tokio::test]
    async fn members_and_workspace_bots_read_and_nobody_else_does() {
        let scratch = scratch_or_skip!("pes_reads");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "rd").await;
        let member = seed_member(db, &tenant, "member").await;
        let bot = seed_bot_user(db, &tenant).await;
        let outsider_tenant = seed_tenant(db, "ro").await;
        let outsider = seed_bot_user(db, &outsider_tenant).await;

        // Before the project has settings the answer is null.
        assert_eq!(
            get(db, member, None, tenant.project_id).await.expect("read")["data"],
            Value::Null
        );
        exec(
            db,
            "INSERT INTO project_execution_settings (project_id, enabled, working_directory) VALUES ($1, true, '/work/a')",
            vec![tenant.project_id.into()],
        )
        .await;

        let read = get(db, member, None, tenant.project_id).await.expect("read");
        assert_eq!(read["data"]["working_directory"], "/work/a");
        assert_eq!(read["data"]["version"], 1);
        let own = get(
            db,
            bot,
            Some(bot_context_for(bot, tenant.workspace_id, &["read"])),
            tenant.project_id,
        )
        .await
        .expect("a bot of the workspace reads");
        assert_eq!(own["data"]["working_directory"], "/work/a");
        let foreign = get(
            db,
            outsider,
            Some(bot_context_for(outsider, outsider_tenant.workspace_id, &["read"])),
            tenant.project_id,
        )
        .await;
        assert!(matches!(foreign, Err(ApiError::Forbidden(_))), "{foreign:?}");
        let stranger = get(db, outsider_tenant.member_id, None, tenant.project_id).await;
        assert!(
            matches!(stranger, Err(ApiError::Forbidden(_))),
            "a person outside the workspace is refused"
        );
        let missing = get(db, member, None, Uuid::new_v4()).await;
        assert!(matches!(missing, Err(ApiError::NotFound(_))));
        scratch.drop_self().await;
    }
}
