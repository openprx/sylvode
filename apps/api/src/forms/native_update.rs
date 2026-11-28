//! Transaction-aware form schema update shared by the HTTP save path and the draft apply.
//!
//! The caller supplies the transaction and has already authorised the actor. Validation, the
//! optimistic version check, the version bump, the `form.updated` event and the schema-version
//! row are the same for every caller. No lock is taken here beyond the row lock of the UPDATE.

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    forms::{
        rules::SchemaScope,
        schema::{ensure_schema_field_ids, ensure_valid_schema},
    },
    routes::form::{FormResponse, find_form_with_conn, insert_form_event_with_idempotency, insert_schema_version},
};

pub struct NativeFormSchemaUpdate {
    pub form_id: Uuid,
    pub schema: Value,
    /// When set, the write is refused unless the form is still at this schema version.
    pub expected_schema_version: Option<i32>,
    pub actor_id: Option<Uuid>,
    pub source: Value,
    pub change_summary: String,
}

/// Validates and writes a new form schema, bumps the schema version, and records the event and
/// the schema-version row.
pub async fn update_form_schema_in_transaction(
    tx: &DatabaseTransaction,
    req: NativeFormSchemaUpdate,
) -> Result<FormResponse, ApiError> {
    let current = find_form_with_conn(tx, req.form_id).await?;
    let schema = ensure_schema_field_ids(req.schema).map_err(ApiError::BadRequest)?;
    ensure_valid_schema(
        &schema,
        Some(SchemaScope {
            workspace_id: current.workspace_id,
            project_id: current.project_id,
        }),
    )?;
    let mut sql = "UPDATE project_forms \
                   SET schema = $1, schema_version = schema_version + 1, \
                       updated_at = GREATEST(clock_timestamp(), updated_at + interval '1 microsecond') \
                   WHERE id = $2"
        .to_owned();
    let mut values = vec![schema.into(), req.form_id.into()];
    if let Some(expected) = req.expected_schema_version {
        sql.push_str(" AND schema_version = $3");
        values.push(expected.into());
    }
    let written = tx
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;
    if written.rows_affected() != 1 {
        return Err(ApiError::VersionConflict("form_schema_version_conflict"));
    }
    let form = find_form_with_conn(tx, req.form_id).await?;
    insert_form_event_with_idempotency(
        tx,
        &form,
        None,
        "form.updated",
        req.actor_id,
        req.source,
        json!({ "form_id": form.id, "key": form.key, "changed_fields": ["schema"] }),
        None,
    )
    .await?;
    insert_schema_version(tx, &form, req.actor_id, &req.change_summary).await?;
    Ok(form)
}
