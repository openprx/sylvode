//! SQL access to `nav_configs` and `nav_user_prefs`.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, FromQueryResult)]
pub struct ConfigRow {
    pub doc: Value,
    pub version: i32,
    pub updated_at: Timestamp,
}

const CONFIG_COLUMNS: &str = "doc, version, updated_at";

pub async fn load_config(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<Option<ConfigRow>, ApiError> {
    Ok(ConfigRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {CONFIG_COLUMNS} FROM nav_configs \
             WHERE workspace_id = $1 AND project_id IS NOT DISTINCT FROM $2::uuid"
        ),
        vec![workspace_id.into(), project_id.into()],
    ))
    .one(db)
    .await?)
}

/// Who wrote: a person, or a bot.
#[derive(Debug, Clone, Copy)]
pub struct Writer {
    pub user: Option<Uuid>,
    pub bot: Option<Uuid>,
}

/// Creates the first version of a scope's document; `None` when one already exists.
pub async fn insert_config(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    doc: &Value,
    writer: Writer,
) -> Result<Option<ConfigRow>, ApiError> {
    Ok(ConfigRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "INSERT INTO nav_configs (workspace_id, project_id, doc, updated_by, updated_by_bot) \
             VALUES ($1, $2::uuid, $3, $4::uuid, $5::uuid) ON CONFLICT DO NOTHING RETURNING {CONFIG_COLUMNS}"
        ),
        vec![
            workspace_id.into(),
            project_id.into(),
            doc.clone().into(),
            writer.user.into(),
            writer.bot.into(),
        ],
    ))
    .one(db)
    .await?)
}

/// Replaces the document when `expected_version` is still the stored one.
pub async fn update_config(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    doc: &Value,
    expected_version: i32,
    writer: Writer,
) -> Result<Option<ConfigRow>, ApiError> {
    Ok(ConfigRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "UPDATE nav_configs SET doc = $3, version = version + 1, updated_at = now(), \
                    updated_by = $5::uuid, updated_by_bot = $6::uuid \
             WHERE workspace_id = $1 AND project_id IS NOT DISTINCT FROM $2::uuid AND version = $4 \
             RETURNING {CONFIG_COLUMNS}"
        ),
        vec![
            workspace_id.into(),
            project_id.into(),
            doc.clone().into(),
            expected_version.into(),
            writer.user.into(),
            writer.bot.into(),
        ],
    ))
    .one(db)
    .await?)
}

pub async fn delete_config(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM nav_configs WHERE workspace_id = $1 AND project_id IS NOT DISTINCT FROM $2::uuid",
        vec![workspace_id.into(), project_id.into()],
    ))
    .await?;
    Ok(())
}

#[derive(Debug, FromQueryResult)]
pub struct PrefsRow {
    pub prefs: Value,
    pub updated_at: Timestamp,
}

pub async fn load_prefs(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<Option<PrefsRow>, ApiError> {
    Ok(PrefsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT prefs, updated_at FROM nav_user_prefs \
         WHERE user_id = $1 AND workspace_id = $2 AND project_id IS NOT DISTINCT FROM $3::uuid",
        vec![user_id.into(), workspace_id.into(), project_id.into()],
    ))
    .one(db)
    .await?)
}

pub async fn save_prefs(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    prefs: &Value,
) -> Result<PrefsRow, ApiError> {
    PrefsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO nav_user_prefs (user_id, workspace_id, project_id, prefs) VALUES ($1, $2, $3::uuid, $4) \
         ON CONFLICT (user_id, workspace_id, COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
         DO UPDATE SET prefs = EXCLUDED.prefs, updated_at = now() RETURNING prefs, updated_at",
        vec![
            user_id.into(),
            workspace_id.into(),
            project_id.into(),
            prefs.clone().into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)
}

pub async fn delete_prefs(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM nav_user_prefs WHERE user_id = $1 AND workspace_id = $2 AND project_id IS NOT DISTINCT FROM $3::uuid",
        vec![user_id.into(), workspace_id.into(), project_id.into()],
    ))
    .await?;
    Ok(())
}

/// The type of a project, for the defaults of a project navigation.
pub async fn project_type(db: &impl ConnectionTrait, project_id: Uuid) -> Result<Option<String>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        type_key: String,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COALESCE(type_key, 'code_project') AS type_key FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.type_key))
}
