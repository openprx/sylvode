//! The installation as the host sees it right now: loaded from the database at the start of an
//! invocation and again before every host call (contract C1 §3).
//!
//! Nothing here is cached, so disabling or revoking an installation, narrowing its grants, ending
//! the invoking user's sessions or removing the approver takes effect at the next call.

use std::collections::BTreeSet;

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::catalogue::{Grant, InstallationView, RefusalCode};

/// Why an installation could not be loaded for a call.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadError {
    /// The installation may not be used: `installation_not_active` or `installation_revoked`.
    Refused(RefusalCode),
    /// A database failure; the caller answers `unavailable`.
    Unavailable,
}

/// The active generation of an installation with its grants.
#[derive(Clone, Debug)]
pub struct Live {
    /// Requested capability ids and the grants whose digest is still current.
    pub view: InstallationView,
    pub generation_id: Uuid,
    pub package_row_id: Uuid,
    pub package_key: String,
    /// The administrator who approved the active generation.
    pub approver: Option<Uuid>,
}

#[derive(FromQueryResult)]
struct InstallationRow {
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    package_key: String,
    status: String,
    active_generation: Option<i32>,
    generation_id: Option<Uuid>,
    package_row_id: Option<Uuid>,
    requested_capabilities: Option<Vec<String>>,
    reviewed_by: Option<Uuid>,
}

#[derive(FromQueryResult)]
struct GrantRow {
    capability: String,
    selector: Value,
    plugin_ops_digest: String,
    current_digest: Option<String>,
}

fn uuid_key(selector: &Value, key: &str) -> Option<Uuid> {
    selector.get(key)?.as_str().and_then(|text| Uuid::parse_str(text).ok())
}

/// The grants a call may use: those approved against the capability's current `plugin_ops_digest`
/// (a grant approved against another digest is not honoured until it is approved again) and whose
/// selector keys parse (a selector that does not parse never matches, so the grant is dropped).
fn honoured_grants(rows: Vec<GrantRow>) -> Vec<Grant> {
    rows.into_iter()
        .filter(|grant| grant.current_digest.as_deref() == Some(grant.plugin_ops_digest.as_str()))
        .filter_map(|grant| {
            let project = grant
                .selector
                .get("project")
                .map(|_| uuid_key(&grant.selector, "project"));
            let form = grant.selector.get("form").map(|_| uuid_key(&grant.selector, "form"));
            if matches!(project, Some(None)) || matches!(form, Some(None)) {
                return None;
            }
            Some(Grant {
                capability: grant.capability,
                project: project.flatten(),
                form: form.flatten(),
                plugin_ops_digest: grant.plugin_ops_digest,
            })
        })
        .collect()
}

/// Loads the installation and its active generation.
///
/// # Errors
/// `installation_not_active` for a pending, disabled or unknown installation, `installation_revoked`
/// for a revoked one, [`LoadError::Unavailable`] for a database failure.
pub async fn load(db: &impl ConnectionTrait, installation_id: Uuid) -> Result<Live, LoadError> {
    let unavailable = |error: sea_orm::DbErr| {
        tracing::error!(target: "sv::extensions", %error, "the installation could not be loaded");
        LoadError::Unavailable
    };
    let row = InstallationRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT i.workspace_id, i.project_id, i.package_key, i.status, i.active_generation, \
                g.id AS generation_id, g.package_row_id, g.requested_capabilities, g.reviewed_by \
         FROM installations i \
         LEFT JOIN installation_generations g \
           ON g.installation_id = i.id AND g.generation = i.active_generation AND g.status = 'approved' \
         WHERE i.id = $1",
        vec![installation_id.into()],
    ))
    .one(db)
    .await
    .map_err(unavailable)?
    .ok_or(LoadError::Refused(RefusalCode::InstallationNotActive))?;
    if row.status == "revoked" {
        return Err(LoadError::Refused(RefusalCode::InstallationRevoked));
    }
    let (true, Some(generation), Some(generation_id), Some(package_row_id)) = (
        row.status == "active",
        row.active_generation,
        row.generation_id,
        row.package_row_id,
    ) else {
        return Err(LoadError::Refused(RefusalCode::InstallationNotActive));
    };
    let grants = GrantRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT gr.capability, gr.selector, gr.plugin_ops_digest, c.plugin_ops_digest AS current_digest \
         FROM installation_grants gr JOIN capability_ids c ON c.id = gr.capability \
         WHERE gr.generation_id = $1 ORDER BY gr.capability, gr.selector::text",
        vec![generation_id.into()],
    ))
    .all(db)
    .await
    .map_err(unavailable)?;
    let grants = honoured_grants(grants);
    Ok(Live {
        view: InstallationView {
            installation_id,
            generation,
            workspace_id: row.workspace_id,
            project_id: row.project_id,
            requested: row
                .requested_capabilities
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            grants,
        },
        generation_id,
        package_row_id,
        package_key: row.package_key,
        approver: row.reviewed_by,
    })
}

/// The `session_version` of an active person, or `None` when the user is gone, deactivated or not a
/// person (a bot or AI user never acts through a plugin).
///
/// # Errors
/// [`LoadError::Unavailable`] for a database failure.
pub async fn actor_session(db: &impl ConnectionTrait, user_id: Uuid) -> Result<Option<i64>, LoadError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT session_version FROM users WHERE id = $1 AND is_active AND entity_type = 'human'",
            vec![user_id.into()],
        ))
        .await
        .map_err(|_| LoadError::Unavailable)?;
    Ok(row.and_then(|row| row.try_get::<i64>("", "session_version").ok()))
}

/// Whether `user_id` is an active person who is owner or admin of the workspace.
///
/// # Errors
/// [`LoadError::Unavailable`] for a database failure.
pub async fn approver_is_admin(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, LoadError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_members m JOIN users u ON u.id = m.user_id \
             WHERE m.workspace_id = $1 AND m.user_id = $2 AND m.role IN ('owner','admin') \
               AND u.is_active AND u.entity_type = 'human'",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await
        .map_err(|_| LoadError::Unavailable)?;
    Ok(row.is_some())
}

/// An installation whose approver is gone stops serving until a current administrator approves
/// again (reason `approver_lost`).
///
/// # Errors
/// [`LoadError::Unavailable`] for a database failure.
pub async fn disable_for_lost_approver(db: &impl ConnectionTrait, installation_id: Uuid) -> Result<(), LoadError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE installations SET status = 'disabled', disabled_reason = 'approver_lost', updated_at = now() \
         WHERE id = $1 AND status = 'active'",
        vec![installation_id.into()],
    ))
    .await
    .map_err(|_| LoadError::Unavailable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{GrantRow, honoured_grants};

    fn row(selector: serde_json::Value, stored: &str, current: Option<&str>) -> GrantRow {
        GrantRow {
            capability: "forms.records.update".to_string(),
            selector,
            plugin_ops_digest: stored.to_string(),
            current_digest: current.map(str::to_owned),
        }
    }

    /// A grant approved against an older digest of its id, or whose id is gone, is not loaded; so is
    /// a grant whose selector does not parse. A valid selector becomes its project and form.
    #[test]
    fn only_current_grants_with_a_valid_selector_are_honoured() {
        let form = Uuid::from_u128(5);
        let honoured = honoured_grants(vec![
            row(json!({"form": form.to_string()}), "sha256:a", Some("sha256:a")),
            row(json!({}), "sha256:a", Some("sha256:b")),
            row(json!({}), "sha256:a", None),
            row(json!({"form": "not-a-uuid"}), "sha256:a", Some("sha256:a")),
            row(json!({"project": 3}), "sha256:a", Some("sha256:a")),
            row(json!({}), "sha256:a", Some("sha256:a")),
        ]);
        assert_eq!(honoured.len(), 2);
        assert_eq!(honoured[0].form, Some(form));
        assert_eq!(honoured[1].form, None);
        assert_eq!(honoured[1].project, None);
    }
}
