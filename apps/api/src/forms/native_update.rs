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
    /// The new schema. `None` keeps the stored schema and still bumps the version, for a save that
    /// only changes the layout stored beside it.
    pub schema: Option<Value>,
    /// Names reported in the `form.updated` event payload. Callers that wrote other columns in the
    /// same transaction list them here so one save emits exactly one event.
    pub changed_fields: Vec<&'static str>,
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
    let schema = match req.schema {
        Some(schema) => {
            let current = find_form_with_conn(tx, req.form_id).await?;
            let schema = ensure_schema_field_ids(schema).map_err(ApiError::BadRequest)?;
            ensure_valid_schema(
                &schema,
                Some(SchemaScope {
                    workspace_id: current.workspace_id,
                    project_id: current.project_id,
                }),
            )?;
            Some(schema)
        }
        None => None,
    };
    let mut set = String::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    if let Some(schema) = schema {
        values.push(schema.into());
        set.push_str("schema = $1, ");
    }
    set.push_str(
        "schema_version = schema_version + 1, \
         updated_at = GREATEST(clock_timestamp(), updated_at + interval '1 microsecond')",
    );
    values.push(req.form_id.into());
    let mut sql = format!("UPDATE project_forms SET {set} WHERE id = ${}", values.len());
    if let Some(expected) = req.expected_schema_version {
        values.push(expected.into());
        std::fmt::Write::write_fmt(&mut sql, format_args!(" AND schema_version = ${}", values.len()))
            .map_err(|_| ApiError::Internal)?;
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
        json!({ "form_id": form.id, "key": form.key, "changed_fields": req.changed_fields }),
        None,
    )
    .await?;
    insert_schema_version(tx, &form, req.actor_id, &req.change_summary).await?;
    Ok(form)
}
