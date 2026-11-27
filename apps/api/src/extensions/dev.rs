//! A temporary instance-admin route that exercises the host before installations exist
//! (`POST /api/v1/admin/extensions/dev-invoke`, op `extensions.dev.invoke`).
//!
//! It runs a component, passed in the request, as an invoked action of an ad-hoc installation with
//! the grants of the request, **acting as the user named in the request**. It exists for the real
//! tests of the host (RT2 P3-P5) and is deleted when installations (L2.P2m) land.

use axum::{Extension, Json, extract::State};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::host::{GrantSet, World, invoke};
use crate::catalogue::{Grant, InstallationView, Invoker, Principal};
use crate::entities::workspace_member::WorkspaceRole;
use crate::error::ApiError;
use crate::response::ApiResponse;

/// One grant as a request writes it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantInput {
    pub capability: String,
    #[serde(default)]
    pub project: Option<Uuid>,
    #[serde(default)]
    pub form: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevInvokeRequest {
    pub workspace_id: Uuid,
    /// The user the plugin acts as; must be a member of the workspace.
    pub as_user_id: Uuid,
    /// `action` or `field-rule`.
    pub world: String,
    /// The action name, or the rule stage.
    pub entry: String,
    /// The JSON text handed to the export.
    pub input: String,
    /// The component, base64.
    pub component_b64: String,
    /// Narrows the ad-hoc installation to one project.
    #[serde(default)]
    pub project_id: Option<Uuid>,
    /// The capabilities the ad-hoc manifest requests; defaults to the granted ones.
    #[serde(default)]
    pub requested: Option<Vec<String>>,
    #[serde(default)]
    pub grants: Vec<GrantInput>,
    /// Reuse an installation id to exercise the per-installation limits.
    #[serde(default)]
    pub installation_id: Option<Uuid>,
}

/// Runs the component and answers `{ok, output | error, record}`; a refused or failed plugin is
/// still a successful answer of this route, so the log record is always visible.
///
/// # Errors
/// A bad request, or a database failure.
pub async fn dev_invoke(
    State(state): State<AppState>,
    Extension(admin): Extension<JwtClaims>,
    Json(request): Json<DevInvokeRequest>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let world = match request.world.as_str() {
        "action" => World::Action,
        "field-rule" => World::FieldRule,
        _ => return Err(ApiError::BadRequest("world must be action or field-rule".to_string())),
    };
    let component = STANDARD
        .decode(request.component_b64.as_bytes())
        .map_err(|_| ApiError::BadRequest("component_b64 is not base64".to_string()))?;
    let role = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT m.role FROM workspace_members m JOIN users u ON u.id = m.user_id \
             WHERE m.workspace_id = $1 AND m.user_id = $2 AND u.is_active AND u.entity_type = 'human'",
            vec![request.workspace_id.into(), request.as_user_id.into()],
        ))
        .await
        .map_err(ApiError::internal_database_failure)?
        .and_then(|row| row.try_get::<String>("", "role").ok())
        .and_then(|role| WorkspaceRole::from_str(&role))
        .ok_or_else(|| {
            ApiError::BadRequest("as_user_id is not an active person who is a member of the workspace".to_string())
        })?;
    let grants: Vec<Grant> = request
        .grants
        .into_iter()
        .map(|grant| Grant {
            capability: grant.capability,
            project: grant.project,
            form: grant.form,
        })
        .collect();
    let requested = request.requested.map_or_else(
        || grants.iter().map(|grant| grant.capability.clone()).collect(),
        |ids| ids.into_iter().collect(),
    );
    let installation_id = request.installation_id.unwrap_or_else(Uuid::new_v4);
    let installation = InstallationView {
        installation_id,
        generation: 1,
        workspace_id: request.workspace_id,
        project_id: request.project_id,
        requested: std::collections::BTreeSet::new(),
        grants: Vec::new(),
    };
    let principal = Principal::Plugin {
        invoker: Invoker::User {
            user_id: request.as_user_id,
            workspace_id: request.workspace_id,
            role,
        },
        installation,
    };
    // The records the plugin writes carry `as_user_id` as their actor; this line names the admin.
    tracing::warn!(
        target: "sv::extensions",
        admin = %admin.sub,
        as_user = %request.as_user_id,
        workspace = %request.workspace_id,
        installation = %installation_id,
        world = world.name(),
        "dev invoke"
    );
    let mut outcome = invoke(
        &state,
        &principal,
        &component,
        world,
        &request.entry,
        &request.input,
        &GrantSet { requested, grants },
    )
    .await;
    outcome.record.dev_admin = Some(admin.sub.clone());
    let data = match outcome.result {
        Ok(output) => json!({"ok": true, "output": output, "record": outcome.record}),
        Err(error) => json!({
            "ok": false,
            "error": {"code": error.code.as_str(), "guest_code": error.guest_code},
            "record": outcome.record,
        }),
    };
    Ok(ApiResponse::success(data))
}
