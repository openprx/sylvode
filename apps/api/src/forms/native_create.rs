//! Transaction-aware Forms creation services shared by HTTP and cross-module bridges.
//!
//! Callers supply an existing transaction so a bridge can commit the native Forms record,
//! its native events, and its own lineage atomically. Validation and plugin hooks stay identical
//! to direct Forms creation; this module is deliberately independent of Axum extractors.

use std::collections::BTreeSet;

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    forms::{
        permissions::{denied_read_field_keys, ensure_field_write_policy_allows},
        projections::refresh_record_projection,
        rules::{
            SchemaScope, apply_defaults, ensure_read_only_untouched,
            external::{RuleCaller, Writer},
        },
        schema::{ensure_schema_field_ids, ensure_valid_schema, normalize_key},
        signature_media::{
            annotate_signature_audit_entries, append_signature_audit_source, materialize_signature_values_with_audit,
            without_client_signature_media,
        },
        validation::{caller, validate_create, write_context, writer_for_source},
    },
    plugins::hooks::{run_event_handler_hooks, run_field_validator_hooks, run_formula_hooks},
    routes::form::{
        FormResponse, RecordResponse, apply_autonumber_values, calculate_values, ensure_json_object,
        ensure_record_objects_claimable, find_form_with_conn, find_idempotent_record, find_record,
        insert_form_event_with_idempotency, insert_schema_version, recalculate_parent_records_for_child, render_title,
    },
};

pub struct NativeFormCreateRequest {
    pub form_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub title_template: Option<String>,
    pub schema: Value,
    pub detail_layout: Value,
    pub created_by: Option<Uuid>,
    pub source: Value,
    pub change_summary: String,
}

/// The 409 for a form key the project already uses (archived forms keep their key).
#[must_use]
pub fn form_key_taken() -> ApiError {
    ApiError::conflict_coded(
        "form_key_taken",
        "a form with this key already exists in the project (an archived one still holds its key)",
        None,
    )
}

/// Maps the unique violations of form keys and view keys to the 409 they stand for.
///
/// A check before the write answers the common case where one exists; this catches every create
/// path and two requests racing. Any other database error passes through unchanged.
#[must_use]
pub fn key_violation(err: sea_orm::DbErr) -> ApiError {
    let text = err.to_string();
    if text.contains("project_forms_project_id_key_key") {
        form_key_taken()
    } else if text.contains("form_views_form_id_key_key") {
        ApiError::conflict_coded(
            "form_view_key_taken",
            "a view with this key already exists on the form (an archived one still holds its key)",
            None,
        )
    } else {
        ApiError::from(err)
    }
}

/// Runs the native Forms form-create validation, insert, event, and schema-version pipeline.
pub async fn create_form_in_transaction(
    tx: &DatabaseTransaction,
    req: NativeFormCreateRequest,
) -> Result<FormResponse, ApiError> {
    let key = normalize_key(&req.key).map_err(ApiError::BadRequest)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }
    let schema = ensure_schema_field_ids(req.schema).map_err(ApiError::BadRequest)?;
    ensure_valid_schema(
        &schema,
        Some(SchemaScope {
            workspace_id: req.workspace_id,
            project_id: req.project_id,
        }),
    )?;
    let detail_layout = ensure_json_object(req.detail_layout, "detail_layout")?;
    let form_id = req.form_id.unwrap_or_else(Uuid::new_v4);
    let source = ensure_json_object(req.source, "source")?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO project_forms \
         (id, workspace_id, project_id, key, name, description, icon, color, title_template, schema, \
          detail_layout, created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        vec![
            form_id.into(),
            req.workspace_id.into(),
            req.project_id.into(),
            key.into(),
            name.to_string().into(),
            req.description.into(),
            req.icon.into(),
            req.color.into(),
            req.title_template.unwrap_or_else(|| "{id}".to_string()).into(),
            schema.into(),
            detail_layout.into(),
            req.created_by.into(),
        ],
    ))
    .await
    .map_err(key_violation)?;
    let form = find_form_with_conn(tx, form_id).await?;
    insert_form_event_with_idempotency(
        tx,
        &form,
        None,
        "form.created",
        req.created_by,
        source,
        json!({"form_id":form.id,"key":form.key,"name":form.name}),
        None,
    )
    .await?;
    insert_schema_version(tx, &form, req.created_by, &req.change_summary).await?;
    Ok(form)
}

/// The id of the new record is always generated here: no caller supplies one, so a create can't collide with an existing record.
pub struct NativeRecordCreateRequest {
    pub values: Value,
    pub title: Option<String>,
    pub source: Value,
    pub idempotency_key: Option<String>,
}

pub struct NativeRecordCreation {
    pub record_id: Uuid,
    pub values: Value,
    pub created_by: Option<Uuid>,
    pub event_payload: Value,
    pub was_created: bool,
}

/// Who a record create runs for.
pub struct CreateActor<'a> {
    /// The rule caller: CEL `user` and the principal plugin rules are told about.
    pub caller: RuleCaller,
    /// The workspace role whose form field policy applies (stage 1, read-denied keys); `None` when
    /// the caller has already limited the fields to its own allow-list (a site visitor's
    /// projection, contract C3 section 6.1).
    pub policy_role: Option<&'a str>,
    /// The role legacy plugin hooks run under.
    pub hook_role: &'a str,
    /// `form_records.created_by`; `None` for a bot or a site visitor.
    pub created_by: Option<Uuid>,
    /// The actor kind and id recorded in signature audit entries.
    pub audit_kind: &'static str,
    pub audit_id: Uuid,
}

/// Runs the complete native Forms record-create pipeline inside the caller's transaction.
pub async fn create_record_in_transaction(
    state: &AppState,
    tx: &DatabaseTransaction,
    form: &FormResponse,
    actor_id: Uuid,
    role: &str,
    is_bot: bool,
    req: NativeRecordCreateRequest,
) -> Result<NativeRecordCreation, ApiError> {
    let actor = CreateActor {
        caller: caller(actor_id, role, is_bot),
        policy_role: Some(role),
        hook_role: role,
        created_by: (!is_bot).then_some(actor_id),
        audit_kind: if is_bot { "bot" } else { "user" },
        audit_id: actor_id,
    };
    create_record_as(state, tx, form, actor, req).await
}

/// [`create_record_in_transaction`] for an explicit actor (the public site's visitors and
/// operators).
pub async fn create_record_as(
    state: &AppState,
    tx: &DatabaseTransaction,
    form: &FormResponse,
    actor: CreateActor<'_>,
    req: NativeRecordCreateRequest,
) -> Result<NativeRecordCreation, ApiError> {
    if let Some(record) =
        find_idempotent_record(state, form, req.idempotency_key.as_deref(), "form.record.created").await?
    {
        return Ok(NativeRecordCreation {
            record_id: record.id,
            values: record.values.clone(),
            created_by: record.created_by,
            event_payload: json!({"record_id":record.id,"values":record.values}),
            was_created: false,
        });
    }

    if let Some(role) = actor.policy_role {
        ensure_field_write_policy_allows(state, form.id, role, &req.values).await?;
    }
    ensure_read_only_untouched(&form.schema, &req.values, None, &BTreeSet::new())?;
    let writer = writer_for_source(&req.source, Writer::Create);
    let (with_defaults, defaulted) = apply_defaults(&form.schema, req.values, &actor.caller)?;
    let values_with_formula = run_formula_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        actor.hook_role,
        with_defaults,
    )
    .await?;
    let record_id = Uuid::new_v4();
    let created_by = actor.created_by;
    // `signature_media` is server-owned: a client's copy is dropped, the server adds its own below.
    let source = without_client_signature_media(ensure_json_object(req.source, "source")?);
    let calculated = calculate_values(state, form, None, values_with_formula).await?;
    let with_autonumber = apply_autonumber_values(tx, form, None, calculated).await?;
    let denied_read_fields = match actor.policy_role {
        Some(role) => denied_read_field_keys(state, form.id, role).await?,
        None => BTreeSet::new(),
    };
    let ctx = write_context(form, Some(record_id), actor.caller, writer, denied_read_fields);
    let normalized = validate_create(&form.schema, &ctx, with_autonumber, &defaulted).await?;
    let signature_materialization = materialize_signature_values_with_audit(&form.schema, normalized).await?;
    let normalized = signature_materialization.values;
    ensure_record_objects_claimable(state, form.workspace_id, &normalized, None).await?;
    let signature_audit_entries = annotate_signature_audit_entries(
        signature_materialization.audit_entries,
        actor.audit_kind,
        actor.audit_id,
        "record.create",
        form.id,
        record_id,
        form.schema_version,
    );
    let source = append_signature_audit_source(source, signature_audit_entries)?;
    run_field_validator_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        &form.schema,
        &normalized,
    )
    .await?;
    let title = req.title.filter(|value| !value.trim().is_empty()).map_or_else(
        || render_title(&form.title_template, record_id, &normalized),
        crate::forms::read_policy::cap_title,
    );

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO form_records \
         (id, workspace_id, project_id, form_id, title, values, source, schema_version, created_by, updated_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)",
        vec![
            record_id.into(),
            form.workspace_id.into(),
            form.project_id.into(),
            form.id.into(),
            title.into(),
            normalized.clone().into(),
            source.clone().into(),
            form.schema_version.into(),
            created_by.into(),
        ],
    ))
    .await
    .map_err(key_violation)?;
    refresh_record_projection(tx, form.project_id, form.id, record_id, &form.schema, &normalized).await?;
    let event_payload = json!({"record_id":record_id,"values":normalized});
    insert_form_event_with_idempotency(
        tx,
        form,
        Some(record_id),
        "form.record.created",
        created_by,
        source,
        event_payload.clone(),
        req.idempotency_key,
    )
    .await?;

    Ok(NativeRecordCreation {
        record_id,
        values: normalized,
        created_by,
        event_payload,
        was_created: true,
    })
}

/// Runs native post-commit hooks only after the caller atomically commits its transaction.
pub async fn finish_record_creation(
    state: &AppState,
    form: &FormResponse,
    creation: &NativeRecordCreation,
) -> Result<RecordResponse, ApiError> {
    if creation.was_created {
        run_event_handler_hooks(
            state,
            form.workspace_id,
            form.project_id,
            form.id,
            &form.key,
            Some(creation.record_id),
            "form.record.created",
            creation.event_payload.clone(),
        )
        .await;
        recalculate_parent_records_for_child(state, creation.record_id, creation.created_by).await?;
    }
    find_record(state, creation.record_id).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use sea_orm::DbErr;

    use super::key_violation;
    use crate::error::ApiError;

    fn unique(constraint: &str) -> DbErr {
        DbErr::Custom(format!(
            "duplicate key value violates unique constraint \"{constraint}\""
        ))
    }

    fn coded(err: &ApiError) -> Option<(i32, &'static str)> {
        match err {
            ApiError::Coded { status, code, .. } => Some((*status, *code)),
            _ => None,
        }
    }

    #[test]
    fn a_taken_form_key_is_a_409_form_key_taken() {
        let err = key_violation(unique("project_forms_project_id_key_key"));
        assert_eq!(coded(&err), Some((409, "form_key_taken")));
    }

    #[test]
    fn a_taken_view_key_is_a_409_form_view_key_taken() {
        let err = key_violation(unique("form_views_form_id_key_key"));
        assert_eq!(coded(&err), Some((409, "form_view_key_taken")));
    }

    #[test]
    fn any_other_database_error_stays_a_database_error() {
        let err = key_violation(unique("form_records_other_key"));
        assert!(matches!(err, ApiError::Database(_)));
    }
}
