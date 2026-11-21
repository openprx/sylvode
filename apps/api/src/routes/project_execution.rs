//! Read and write a project's execution settings.
//!
//! Reading is open to the project's members and to the bots of its workspace. Writing is for a
//! person who owns or administers the workspace: a bot is refused whatever permissions it holds,
//! because a bot that could repoint its own runner's working directory would turn "may call the
//! API" into "may choose where a command line agent runs".

use axum::{Extension, Json, extract::Path, extract::State, response::IntoResponse};
use platform::{app::AppState, auth::JwtClaims};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::ApiResponse,
    services::{
        execution_settings::{self, ExecutionSettingsInput, SaveOutcome},
        trust_score_service::{is_project_admin_or_owner, is_project_member},
    },
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

/// `PUT /api/v1/projects/{project_id}/execution-settings`: creates the settings, or replaces
/// them when `expected_version` names the stored version.
pub async fn put_execution_settings(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(input): Json<ExecutionSettingsInput>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    if bot.is_some() || crate::oauth::delegated::current().is_some() {
        return Err(ApiError::Forbidden("bots cannot change execution settings".to_string()));
    }
    let actor_id = user_id(&claims)?;
    let project = execution_settings::project_ref(&state.db, project_id).await?;
    if !is_project_admin_or_owner(&state.db, project_id, actor_id).await? {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can change execution settings".to_string(),
        ));
    }
    let valid = execution_settings::validate(&state, &project, &input).await?;
    match execution_settings::upsert(&state, &project, &valid, input.expected_version, actor_id).await? {
        SaveOutcome::Saved { after, .. } => Ok(ApiResponse::success(json!(after))),
        SaveOutcome::Conflict { current } => Ok(Json(ApiResponse {
            code: 409,
            message: "execution settings changed; reload".to_string(),
            data: Some(json!({
                "version": current.as_ref().map(|settings| settings.version),
                "current": current,
            })),
            error_code: None,
            details: None,
        })),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::too_many_lines, clippy::indexing_slicing)]
mod tests {
    use super::{get_execution_settings, put_execution_settings};
    use crate::error::ApiError;
    use crate::routes::context::tenant_fixture::{
        Tenant, bot_context_for, claims_for, count, exec, seed_bot_user, seed_label, seed_member, seed_tenant,
        state_for,
    };
    use crate::scratch_or_skip;
    use crate::services::execution_settings::ExecutionSettingsInput;
    use axum::{
        Extension, Json,
        extract::{Path, State},
        response::IntoResponse,
    };
    use sea_orm::{DatabaseConnection, FromQueryResult};
    use serde_json::{Value, json};
    use uuid::Uuid;

    #[derive(Debug, FromQueryResult)]
    struct Event {
        payload: Value,
        actor_id: Option<Uuid>,
        aggregate_id: String,
    }

    fn body(directory: &str, expected_version: Option<i32>) -> ExecutionSettingsInput {
        serde_json::from_value(json!({
            "enabled": true,
            "working_directory": directory,
            "expected_version": expected_version,
        }))
        .expect("a valid body")
    }

    fn body_with(extra: &Value) -> ExecutionSettingsInput {
        let mut value = json!({ "working_directory": "/work/a" });
        if let (Some(target), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
            for (key, field) in extra {
                target.insert(key.clone(), field.clone());
            }
        }
        serde_json::from_value(value).expect("a valid body")
    }

    async fn put(
        db: &DatabaseConnection,
        user: Uuid,
        project: Uuid,
        input: ExecutionSettingsInput,
    ) -> Result<Value, ApiError> {
        let answer = put_execution_settings(
            State(state_for(db.clone())),
            Extension(claims_for(user)),
            None,
            Path(project),
            Json(input),
        )
        .await?;
        Ok(serde_json::to_value(answer.0).expect("the envelope serialises"))
    }

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

    fn refused(result: &Result<Value, ApiError>, status: i32) -> bool {
        matches!(
            (result, status),
            (Err(ApiError::Forbidden(_)), 403)
                | (Err(ApiError::BadRequest(_)), 400)
                | (Err(ApiError::NotFound(_)), 404)
        )
    }

    async fn set_role(db: &DatabaseConnection, tenant: &Tenant, user: Uuid, role: &str) {
        exec(
            db,
            "UPDATE workspace_members SET role = $3 WHERE workspace_id = $1 AND user_id = $2",
            vec![tenant.workspace_id.into(), user.into(), role.to_string().into()],
        )
        .await;
    }

    #[tokio::test]
    async fn owners_create_and_admins_update_with_the_stored_version() {
        let scratch = scratch_or_skip!("pes_roles_versions");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ev").await;
        let owner = tenant.member_id;
        set_role(db, &tenant, owner, "owner").await;
        let admin = seed_member(db, &tenant, "admin").await;
        let member = seed_member(db, &tenant, "member").await;

        // A member can read nothing before it exists and cannot write.
        assert_eq!(
            get(db, member, None, tenant.project_id).await.expect("read")["data"],
            Value::Null
        );
        assert!(refused(
            &put(db, member, tenant.project_id, body("/work/a", None)).await,
            403
        ));

        let created = put(db, owner, tenant.project_id, body("/work/a/", None))
            .await
            .expect("create");
        assert_eq!(created["code"], 0, "{created}");
        assert_eq!(created["data"]["version"], 1);
        assert_eq!(
            created["data"]["working_directory"], "/work/a",
            "one trailing slash is stripped"
        );
        assert_eq!(created["data"]["updated_by"], owner.to_string());

        let updated = put(db, admin, tenant.project_id, body("/work/b", Some(1)))
            .await
            .expect("update");
        assert_eq!(updated["data"]["version"], 2);
        assert_eq!(updated["data"]["working_directory"], "/work/b");
        assert_eq!(updated["data"]["updated_by"], admin.to_string());

        // A stale version is a 409 that names the current one and changes nothing.
        let stale = put(db, owner, tenant.project_id, body("/work/c", Some(1)))
            .await
            .expect("envelope");
        assert_eq!(stale["code"], 409, "{stale}");
        assert_eq!(stale["message"], "execution settings changed; reload");
        assert_eq!(stale["data"]["version"], 2);
        let missing = put(db, owner, tenant.project_id, body("/work/c", None))
            .await
            .expect("envelope");
        assert_eq!(
            missing["code"], 409,
            "a save without the version of an existing row conflicts"
        );
        let read = get(db, member, None, tenant.project_id).await.expect("read");
        assert_eq!(read["data"]["working_directory"], "/work/b");
        assert_eq!(read["data"]["version"], 2);

        // A member still cannot write once the row exists.
        assert!(refused(
            &put(db, member, tenant.project_id, body("/work/d", Some(2))).await,
            403
        ));

        // A version for a row that does not exist is a conflict too.
        let other = seed_tenant(db, "ev2").await;
        set_role(db, &other, other.member_id, "owner").await;
        let phantom = put(db, other.member_id, other.project_id, body("/work/a", Some(1)))
            .await
            .expect("envelope");
        assert_eq!(phantom["code"], 409);
        assert_eq!(phantom["data"]["version"], Value::Null);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_bot_never_writes_and_reads_only_its_own_workspace() {
        let scratch = scratch_or_skip!("pes_bots");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "bt").await;
        set_role(db, &tenant, tenant.member_id, "owner").await;
        let bot = seed_bot_user(db, &tenant).await;
        let outsider_tenant = seed_tenant(db, "bo").await;
        let outsider = seed_bot_user(db, &outsider_tenant).await;
        put(db, tenant.member_id, tenant.project_id, body("/work/a", None))
            .await
            .expect("create");

        // Even a bot that holds every permission, and that is an admin of the workspace.
        set_role(db, &tenant, bot, "admin").await;
        let write = put_execution_settings(
            State(state_for(db.clone())),
            Extension(claims_for(bot)),
            Some(Extension(bot_context_for(
                bot,
                tenant.workspace_id,
                &["read", "write", "admin"],
            ))),
            Path(tenant.project_id),
            Json(body("/work/evil", Some(1))),
        )
        .await;
        assert!(
            matches!(&write, Err(ApiError::Forbidden(message)) if message == "bots cannot change execution settings"),
            "a bot must be refused"
        );
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM project_execution_settings WHERE working_directory = '/work/a'",
                vec![],
            )
            .await,
            1
        );

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
        assert!(refused(&foreign, 403), "{foreign:?}");
        let stranger = get(db, outsider_tenant.member_id, None, tenant.project_id).await;
        assert!(refused(&stranger, 403), "a person outside the workspace is refused");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn invalid_fields_are_refused_by_name() {
        let scratch = scratch_or_skip!("pes_validation");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "vl").await;
        let other = seed_tenant(db, "vm").await;
        set_role(db, &tenant, tenant.member_id, "owner").await;
        let own_label = seed_label(db, &tenant, "failed").await;
        let foreign_label = seed_label(db, &other, "foreign").await;
        let owner = tenant.member_id;
        let project = tenant.project_id;

        for (extra, field) in [
            (json!({"working_directory": "relative"}), "working_directory"),
            (json!({"working_directory": "/a/../b"}), "working_directory"),
            (json!({"working_directory": "/a/./b"}), "working_directory"),
            (json!({"working_directory": "//a"}), "working_directory"),
            (json!({"working_directory": "/a\u{1}b"}), "working_directory"),
            (
                json!({"working_directory": format!("/{}", "a".repeat(1024))}),
                "working_directory",
            ),
            (json!({"working_directory": null}), "working_directory"),
            (json!({"cli": "x"}), "cli"),
            (json!({"channel": "x"}), "channel"),
            (json!({"approval_gate": "manual"}), "approval_gate"),
            (json!({"on_complete_state": "state:Bad"}), "on_complete_state"),
            (json!({"on_complete_state": "state:no_such_state"}), "on_complete_state"),
            (json!({"on_fail_label_id": foreign_label}), "on_fail_label_id"),
            (json!({"on_fail_label_id": Uuid::new_v4()}), "on_fail_label_id"),
        ] {
            let answer = put(db, owner, project, body_with(&extra)).await;
            let Err(ApiError::BadRequest(message)) = &answer else {
                panic!("{extra} must be refused, got {answer:?}");
            };
            assert!(message.starts_with(&format!("{field}:")), "{extra}: {message}");
        }
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM project_execution_settings",
                vec![]
            )
            .await,
            0,
            "a refused save writes nothing"
        );

        let saved = put(
            db,
            owner,
            project,
            body_with(&json!({
                "cli": "codex",
                "channel": "headless",
                "on_complete_state": "state:done",
                "on_fail_label_id": own_label,
            })),
        )
        .await
        .expect("a valid body");
        assert_eq!(saved["data"]["cli"], "codex");
        assert_eq!(saved["data"]["on_fail_label_id"], own_label.to_string());
        assert_eq!(saved["data"]["on_complete_state"], "state:done");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_save_writes_one_business_event_with_before_and_after() {
        let scratch = scratch_or_skip!("pes_event");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "be").await;
        set_role(db, &tenant, tenant.member_id, "owner").await;
        let owner = tenant.member_id;
        put(db, owner, tenant.project_id, body("/work/a", None))
            .await
            .expect("create");
        put(db, owner, tenant.project_id, body("/work/b", Some(1)))
            .await
            .expect("update");
        // A conflict records nothing.
        put(db, owner, tenant.project_id, body("/work/c", Some(1)))
            .await
            .expect("envelope");

        let events = Event::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT payload, actor_id, aggregate_id FROM business_events \
             WHERE event_type = 'project.execution_settings_updated' AND project_id = $1 ORDER BY created_at, id",
            vec![tenant.project_id.into()],
        ))
        .all(db)
        .await
        .expect("events load");
        assert_eq!(events.len(), 2);
        let create = events
            .iter()
            .find(|event| event.payload["before"].is_null())
            .expect("create event");
        assert_eq!(create.payload["after"]["working_directory"], "/work/a");
        assert_eq!(create.actor_id, Some(owner));
        assert_eq!(create.aggregate_id, tenant.project_id.to_string());
        let update = events
            .iter()
            .find(|event| !event.payload["before"].is_null())
            .expect("update event");
        assert_eq!(update.payload["before"]["working_directory"], "/work/a");
        assert_eq!(update.payload["after"]["working_directory"], "/work/b");
        assert_eq!(update.payload["after"]["version"], 2);
        scratch.drop_self().await;
    }
}
