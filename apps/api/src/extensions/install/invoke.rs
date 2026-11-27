//! `POST /api/v1/installations/{installation_id}/actions/{action}` (contract C1 §6): a member runs an
//! action of an active installation from the console.
//!
//! The action runs with the member's rights narrowed to the grants and the manifest's requests; the
//! route only finds the component and hands over to the host, which loads the installation again
//! before every host call.

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Deserialize;
use serde_json::{Value, json};
use sv_package::ActionEntry;
use uuid::Uuid;

use super::access;
use super::live::{self, LoadError};
use crate::catalogue::{Invoker, Principal, Refusal, RefusalCode};
use crate::entities::workspace_member::WorkspaceRole;
use crate::error::ApiError;
use crate::extensions::host::{World, invoke};
use crate::packages::open;
use crate::response::ApiResponse;

/// The largest body of the route: the guest input limit (256 KiB) plus the envelope.
pub const BODY_LIMIT: usize = 320 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    #[serde(default)]
    project_id: Option<Uuid>,
    #[serde(default)]
    form_id: Option<Uuid>,
    #[serde(default)]
    record_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeBody {
    #[serde(default)]
    input: Value,
    #[serde(default)]
    context: Context,
}

fn action_name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Runs the action as the caller.
pub async fn invoke_action(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((installation_id, action)): Path<(Uuid, String)>,
    Json(body): Json<InvokeBody>,
) -> Result<impl IntoResponse, ApiError> {
    let (row, user_id, role) = access::installation_for_member(&state, &claims, installation_id).await?;
    let loaded = live::load(&state.db, installation_id)
        .await
        .map_err(|error| match error {
            LoadError::Refused(code) => ApiError::from(Refusal::new(code, None)),
            LoadError::Unavailable => ApiError::internal(),
        })?;
    check_context(&state.db, &row, &body.context).await?;
    let unknown = || ApiError::from(Refusal::new(RefusalCode::PluginActionUnknown, None));
    if !action_name_ok(&action) {
        return Err(unknown());
    }
    let declared = declared_action(&state.db, loaded.package_row_id, &action)
        .await?
        .ok_or_else(unknown)?;
    let stored = open::find_by_row(&state.db, row.workspace_id, loaded.package_row_id)
        .await?
        .ok_or_else(|| ApiError::not_found("package not found"))?;
    let bytes = open::component_bytes(&state.db, row.workspace_id, &stored, &declared.component).await?;
    let principal = Principal::Plugin {
        invoker: Invoker::User {
            user_id,
            workspace_id: row.workspace_id,
            role: WorkspaceRole::from_str(&role).unwrap_or(WorkspaceRole::Member),
        },
        installation: loaded.view,
    };
    let input = serde_json::to_string(&body.input)
        .map_err(|_| ApiError::bad_request_coded("input_invalid", "the input is not JSON"))?;
    let outcome = invoke(&state, &principal, &bytes, World::Action, &action, &input).await;
    match outcome.result {
        Ok(output) => Ok(ApiResponse::success(json!({
            "ok": true,
            "output": output,
            "invocation": outcome.record.id,
        }))),
        Err(error) => Err(error.into()),
    }
}

/// The ids of the context must lie inside the installation's scope: the project is the
/// installation's, the form is a form of that scope, the record belongs to the form.
async fn check_context(
    db: &impl ConnectionTrait,
    row: &super::store::InstallationRow,
    context: &Context,
) -> Result<(), ApiError> {
    let mismatch = || ApiError::from(Refusal::new(RefusalCode::CapabilityScopeMismatch, None));
    if context
        .project_id
        .is_some_and(|project| row.project_id.is_some_and(|own| own != project))
    {
        return Err(mismatch());
    }
    let project = context.project_id.or(row.project_id);
    if let Some(form_id) = context.form_id {
        let found = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM project_forms WHERE id = $1 AND workspace_id = $2 AND ($3::uuid IS NULL OR project_id = $3)",
                vec![form_id.into(), row.workspace_id.into(), project.into()],
            ))
            .await?;
        if found.is_none() {
            return Err(mismatch());
        }
    }
    if let Some(record_id) = context.record_id {
        let found = match context.form_id {
            Some(form_id) => {
                db.query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT 1 FROM form_records WHERE id = $1 AND form_id = $2",
                    vec![record_id.into(), form_id.into()],
                ))
                .await?
            }
            None => None,
        };
        if found.is_none() {
            return Err(mismatch());
        }
    }
    Ok(())
}

async fn declared_action(
    db: &impl ConnectionTrait,
    package_row_id: Uuid,
    name: &str,
) -> Result<Option<ActionEntry>, ApiError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COALESCE(manifest->'actions', '[]'::jsonb) AS actions FROM packages WHERE id = $1",
            vec![package_row_id.into()],
        ))
        .await?;
    let Some(row) = row else { return Ok(None) };
    let actions: Value = row.try_get("", "actions").map_err(|_| ApiError::internal())?;
    let actions: Vec<ActionEntry> = serde_json::from_value(actions).map_err(|_| ApiError::internal())?;
    Ok(actions.into_iter().find(|action| action.name == name))
}

#[cfg(test)]
mod tests {
    use super::action_name_ok;

    #[test]
    fn action_names_follow_the_manifest_grammar() {
        assert!(action_name_ok("close_request"));
        assert!(!action_name_ok(""));
        assert!(!action_name_ok("Close"));
        assert!(!action_name_ok("a-b"));
        assert!(!action_name_ok(&"a".repeat(65)));
    }
}
