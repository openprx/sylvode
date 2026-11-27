//! SQL access to `installations`, `installation_generations` and `installation_grants`.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value as SqlValue};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;

fn stmt(sql: impl Into<String>, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql.into(), values)
}

pub const INSTALLATION_COLUMNS: &str = "id, workspace_id, scope_kind, project_id, package_key, status, \
     disabled_reason, active_generation, created_by_kind, created_by, created_at, revoked_at";

#[derive(Debug, Clone, FromQueryResult)]
pub struct InstallationRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub scope_kind: String,
    pub project_id: Option<Uuid>,
    pub package_key: String,
    pub status: String,
    pub disabled_reason: Option<String>,
    pub active_generation: Option<i32>,
    pub created_by_kind: String,
    pub created_by: Uuid,
    pub created_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
}

pub async fn load_installation(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<InstallationRow>, ApiError> {
    Ok(InstallationRow::find_by_statement(stmt(
        format!("SELECT {INSTALLATION_COLUMNS} FROM installations WHERE id = $1"),
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

/// Takes the row lock that serialises approve, disable, enable, revoke and upgrade of one installation.
pub async fn lock_installation(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<InstallationRow>, ApiError> {
    Ok(InstallationRow::find_by_statement(stmt(
        format!("SELECT {INSTALLATION_COLUMNS} FROM installations WHERE id = $1 FOR UPDATE"),
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

pub async fn list_installations(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<InstallationRow>, ApiError> {
    Ok(InstallationRow::find_by_statement(stmt(
        format!(
            "SELECT {INSTALLATION_COLUMNS} FROM installations WHERE workspace_id = $1 ORDER BY created_at DESC, id"
        ),
        vec![workspace_id.into()],
    ))
    .all(db)
    .await?)
}

/// A live installation of this package in this scope, if any.
pub async fn find_live(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    package_key: &str,
) -> Result<Option<InstallationRow>, ApiError> {
    Ok(InstallationRow::find_by_statement(stmt(
        format!(
            "SELECT {INSTALLATION_COLUMNS} FROM installations \
             WHERE workspace_id = $1 AND project_id IS NOT DISTINCT FROM $2 AND package_key = $3 AND status <> 'revoked'"
        ),
        vec![workspace_id.into(), project_id.into(), package_key.to_owned().into()],
    ))
    .one(db)
    .await?)
}

pub async fn insert_installation(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    package_key: &str,
    created_by_kind: &str,
    created_by: Uuid,
) -> Result<InstallationRow, ApiError> {
    InstallationRow::find_by_statement(stmt(
        format!(
            "INSERT INTO installations (workspace_id, scope_kind, project_id, package_key, status, created_by_kind, created_by) \
             VALUES ($1, $2, $3, $4, 'pending', $5, $6) RETURNING {INSTALLATION_COLUMNS}"
        ),
        vec![
            workspace_id.into(),
            if project_id.is_some() { "project" } else { "workspace" }.to_owned().into(),
            project_id.into(),
            package_key.to_owned().into(),
            created_by_kind.to_owned().into(),
            created_by.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(|error| {
        if error.to_string().contains("installations_live_per_scope") {
            crate::catalogue::Refusal::new(crate::catalogue::RefusalCode::InstallationExists, None).into()
        } else {
            ApiError::from(error)
        }
    })?
    .ok_or_else(ApiError::internal)
}

pub const GENERATION_COLUMNS: &str = "g.id, g.installation_id, g.generation, g.package_row_id, g.status, \
     g.requested_capabilities, g.bind_map, g.requested_by_kind, g.requested_by, g.requested_at, g.reviewed_by, \
     g.reviewed_at, p.digest AS package_digest, p.version AS package_version";

#[derive(Debug, Clone, FromQueryResult)]
pub struct GenerationRow {
    pub id: Uuid,
    pub installation_id: Uuid,
    pub generation: i32,
    pub package_row_id: Uuid,
    pub status: String,
    pub requested_capabilities: Vec<String>,
    pub bind_map: Value,
    pub requested_by_kind: String,
    pub requested_by: Uuid,
    pub requested_at: Timestamp,
    pub reviewed_by: Option<Uuid>,
    pub reviewed_at: Option<Timestamp>,
    pub package_digest: String,
    pub package_version: String,
}

impl GenerationRow {
    /// The generation as the `Installation` shape lists it.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "generation": self.generation,
            "status": self.status,
            "package": {"digest": self.package_digest, "version": self.package_version},
            "requested_capabilities": self.requested_capabilities,
            "bind_map": self.bind_map,
            "requested_by": {"kind": self.requested_by_kind, "id": self.requested_by},
            "requested_at": self.requested_at,
            "reviewed_by": self.reviewed_by,
            "reviewed_at": self.reviewed_at,
        })
    }
}

pub async fn generations_of(db: &impl ConnectionTrait, installation_id: Uuid) -> Result<Vec<GenerationRow>, ApiError> {
    Ok(GenerationRow::find_by_statement(stmt(
        format!(
            "SELECT {GENERATION_COLUMNS} FROM installation_generations g JOIN packages p ON p.id = g.package_row_id \
             WHERE g.installation_id = $1 ORDER BY g.generation"
        ),
        vec![installation_id.into()],
    ))
    .all(db)
    .await?)
}

pub async fn generation(
    db: &impl ConnectionTrait,
    installation_id: Uuid,
    generation: i32,
) -> Result<Option<GenerationRow>, ApiError> {
    Ok(GenerationRow::find_by_statement(stmt(
        format!(
            "SELECT {GENERATION_COLUMNS} FROM installation_generations g JOIN packages p ON p.id = g.package_row_id \
             WHERE g.installation_id = $1 AND g.generation = $2"
        ),
        vec![installation_id.into(), generation.into()],
    ))
    .one(db)
    .await?)
}

#[allow(clippy::too_many_arguments, reason = "one argument per column of the INSERT")]
pub async fn insert_generation(
    db: &impl ConnectionTrait,
    installation_id: Uuid,
    generation: i32,
    package_row_id: Uuid,
    requested: &[String],
    bind_map: &Value,
    requested_by_kind: &str,
    requested_by: Uuid,
) -> Result<(), ApiError> {
    db.execute(stmt(
        "INSERT INTO installation_generations \
           (installation_id, generation, package_row_id, status, requested_capabilities, bind_map, \
            requested_by_kind, requested_by) \
         VALUES ($1, $2, $3, 'pending', $4, $5, $6, $7)",
        vec![
            installation_id.into(),
            generation.into(),
            package_row_id.into(),
            requested.to_vec().into(),
            bind_map.clone().into(),
            requested_by_kind.to_owned().into(),
            requested_by.into(),
        ],
    ))
    .await
    .map_err(|error| {
        if error.to_string().contains("installation_generations_one_pending") {
            ApiError::conflict_coded(
                "generation_pending",
                "this installation already has a pending generation; approve or reject it first",
                None,
            )
        } else {
            ApiError::from(error)
        }
    })?;
    Ok(())
}

pub async fn next_generation(db: &impl ConnectionTrait, installation_id: Uuid) -> Result<i32, ApiError> {
    #[derive(FromQueryResult)]
    struct Max {
        next: i32,
    }
    Ok(Max::find_by_statement(stmt(
        "SELECT COALESCE(MAX(generation), 0) + 1 AS next FROM installation_generations WHERE installation_id = $1",
        vec![installation_id.into()],
    ))
    .one(db)
    .await?
    .map_or(1, |row| row.next))
}

/// A grant as stored.
#[derive(Debug, Clone, FromQueryResult, Serialize)]
pub struct GrantRow {
    pub capability: String,
    pub selector: Value,
    pub plugin_ops_digest: String,
    /// The id's digest now, `None` for an id that is gone from the catalogue.
    pub current_digest: Option<String>,
}

pub async fn grants_of(db: &impl ConnectionTrait, generation_id: Uuid) -> Result<Vec<GrantRow>, ApiError> {
    Ok(GrantRow::find_by_statement(stmt(
        "SELECT gr.capability, gr.selector, gr.plugin_ops_digest, c.plugin_ops_digest AS current_digest \
         FROM installation_grants gr JOIN capability_ids c ON c.id = gr.capability \
         WHERE gr.generation_id = $1 ORDER BY gr.capability, gr.selector::text",
        vec![generation_id.into()],
    ))
    .all(db)
    .await?)
}

/// The current `plugin_ops_digest` of a capability id.
pub async fn current_digest(db: &impl ConnectionTrait, capability: &str) -> Result<Option<String>, ApiError> {
    let row = db
        .query_one(stmt(
            "SELECT plugin_ops_digest FROM capability_ids WHERE id = $1",
            vec![capability.to_owned().into()],
        ))
        .await?;
    Ok(row.and_then(|row| row.try_get::<Option<String>>("", "plugin_ops_digest").ok().flatten()))
}

pub async fn replace_grants(
    db: &impl ConnectionTrait,
    generation_id: Uuid,
    grants: &[(String, Value, String)],
    granted_by: Uuid,
) -> Result<(), ApiError> {
    db.execute(stmt(
        "DELETE FROM installation_grants WHERE generation_id = $1",
        vec![generation_id.into()],
    ))
    .await?;
    for (capability, selector, digest) in grants {
        db.execute(stmt(
            "INSERT INTO installation_grants (generation_id, capability, selector, plugin_ops_digest, granted_by) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
            vec![
                generation_id.into(),
                capability.clone().into(),
                selector.clone().into(),
                digest.clone().into(),
                granted_by.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

/// Marks a generation `approved` or `rejected` by `reviewer`.
pub async fn review_generation(
    db: &impl ConnectionTrait,
    generation_id: Uuid,
    status: &str,
    reviewer: Uuid,
) -> Result<(), ApiError> {
    db.execute(stmt(
        "UPDATE installation_generations SET status = $2, reviewed_by = $3, reviewed_at = now() WHERE id = $1",
        vec![generation_id.into(), status.to_owned().into(), reviewer.into()],
    ))
    .await?;
    Ok(())
}

/// Disables every live installation whose active generation uses a package signed by the key row:
/// `disabled` with reason `key_revoked` (contract C3-7). Returns the ids.
///
/// # Errors
/// A database failure.
pub async fn disable_for_revoked_key(db: &impl ConnectionTrait, key_row_id: Uuid) -> Result<Vec<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Id {
        id: Uuid,
    }
    Ok(Id::find_by_statement(stmt(
        "UPDATE installations i SET status = 'disabled', disabled_reason = 'key_revoked', updated_at = now() \
         FROM installation_generations g JOIN packages p ON p.id = g.package_row_id \
         WHERE g.installation_id = i.id AND g.generation = i.active_generation AND i.status = 'active' \
           AND p.publisher_key_id = $1 \
         RETURNING i.id",
        vec![key_row_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.id)
    .collect())
}
