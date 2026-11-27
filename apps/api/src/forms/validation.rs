//! Record value validation as every writer runs it: type normalisation, then the rule pipeline
//! of the schema ([`super::rules`]). Each entry point names its writer's policy.

use std::collections::BTreeSet;

use serde_json::Value;
use uuid::Uuid;

use super::{
    rules::{
        FieldError, FieldErrors, PipelineInput, PipelineOutput, RulePolicy, WriteContext,
        external::{RuleCaller, Writer},
        run,
    },
    values::NormalizedValues,
};
use crate::{error::ApiError, routes::form::FormResponse};

/// The write context of a form write.
#[must_use]
pub fn write_context(
    form: &FormResponse,
    record_id: Option<Uuid>,
    caller: RuleCaller,
    writer: Writer,
    denied_read_fields: BTreeSet<String>,
) -> WriteContext {
    WriteContext {
        writer,
        workspace_id: form.workspace_id,
        project_id: form.project_id,
        form_id: form.id,
        record_id,
        caller,
        now: WriteContext::now_seconds(),
        denied_read_fields,
    }
}

/// The caller of a user or bot write.
#[must_use]
pub fn caller(actor_id: Uuid, role: &str, is_bot: bool) -> RuleCaller {
    RuleCaller {
        actor_id: Some(actor_id),
        role: role.to_string(),
        is_bot,
    }
}

/// The writer a write's `source` names (plugin rule calls are told who writes): a public site
/// (`kind: site`), a Flow conversion, an import, else `default`.
#[must_use]
pub fn writer_for_source(source: &Value, default: Writer) -> Writer {
    if source.get("kind").and_then(Value::as_str) == Some("site") {
        Writer::Site
    } else if source.get("type").and_then(Value::as_str) == Some("flow_conversion") {
        Writer::FlowConversion
    } else if source.get("origin").and_then(Value::as_str) == Some("import") {
        Writer::Import
    } else {
        default
    }
}

/// Create (and every create-shaped writer): normalises and checks every field; refuses on any error.
/// `defaulted` lists the keys filled from defaults.
pub async fn validate_create(
    schema: &Value,
    ctx: &WriteContext,
    values: Value,
    defaulted: &BTreeSet<String>,
) -> Result<Value, ApiError> {
    validate_create_report(schema, ctx, values, defaulted, RulePolicy::Refuse)
        .await?
        .map(|output| output.values)
        .map_err(FieldErrors::into_api_error)
}

/// Create without refusing: the field errors come back for per-row reports (import preview), the
/// validate endpoint and the recalculation preview.
pub async fn validate_create_report(
    schema: &Value,
    ctx: &WriteContext,
    values: Value,
    defaulted: &BTreeSet<String>,
    policy: RulePolicy,
) -> Result<Result<PipelineOutput, FieldErrors>, ApiError> {
    let none = BTreeSet::new();
    run(
        schema,
        ctx,
        PipelineInput {
            values,
            existing: None,
            always_check: &none,
            defaulted,
            policy,
            report_untouched: false,
        },
    )
    .await
}

/// Update: normalises against the stored values and checks every value this write sets or changes.
///
/// Also reports the required fields that were already unmet and that this write did not address.
/// `always_check` lists keys checked even when unchanged (the submitted read-denied keys).
pub async fn validate_update(
    schema: &Value,
    ctx: &WriteContext,
    values: Value,
    existing_values: &Value,
    always_check: &BTreeSet<String>,
) -> Result<NormalizedValues, ApiError> {
    let output = validate_update_report(schema, ctx, values, existing_values, always_check, false)
        .await?
        .map_err(FieldErrors::into_api_error)?;
    Ok(NormalizedValues {
        values: output.values,
        unmet_required_fields: output.unmet_required_fields,
    })
}

/// Update without refusing; with `report_untouched`, stored values this write does not touch are
/// checked too and their failures come back as carried (the validate endpoint).
pub async fn validate_update_report(
    schema: &Value,
    ctx: &WriteContext,
    values: Value,
    existing_values: &Value,
    always_check: &BTreeSet<String>,
    report_untouched: bool,
) -> Result<Result<PipelineOutput, FieldErrors>, ApiError> {
    let none = BTreeSet::new();
    run(
        schema,
        ctx,
        PipelineInput {
            values,
            existing: Some(existing_values),
            always_check,
            defaulted: &none,
            policy: RulePolicy::Refuse,
            report_untouched,
        },
    )
    .await
}

/// The values a recalculation stores and the rule failures it carries.
pub struct RecalculatedValues {
    pub values: Value,
    /// Derived outputs that break their field rules. They are stored as computed and reported, never
    /// refused: a recalculation is a system write (also run after a child record commits), and
    /// failing it would leave the user's own, already accepted write looking refused.
    pub carried: Vec<FieldError>,
}

/// Recalculation: type errors (and refused plugin calls) still refuse; rule failures are carried.
pub async fn validate_recalculated(
    schema: &Value,
    ctx: &WriteContext,
    values: Value,
    existing_values: &Value,
) -> Result<RecalculatedValues, ApiError> {
    let none = BTreeSet::new();
    let output = run(
        schema,
        ctx,
        PipelineInput {
            values,
            existing: Some(existing_values),
            always_check: &none,
            defaulted: &none,
            policy: RulePolicy::CarryAll,
            report_untouched: false,
        },
    )
    .await?
    .map_err(FieldErrors::into_api_error)?;
    Ok(RecalculatedValues {
        values: output.values,
        carried: output.carried,
    })
}
