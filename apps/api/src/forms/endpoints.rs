//! Field engine endpoints (contract C2 §4.2, §8): the field type descriptors, a dry run of a record
//! write, and the schema check the designer and MCP tools call before a save.
//!
//! None of them stores anything, emits events or runs plugin formula hooks; values are returned only
//! for fields the caller may read.

use std::collections::BTreeSet;

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    permissions::{denied_read_field_keys, ensure_field_write_policy_allows, ensure_record_scope_allows_created_by},
    reader_context::ReaderContext,
    rules::{
        CarriedError, PipelineInput, RulePolicy, SchemaIssue, SchemaIssues, SchemaScope, WriteContext, apply_defaults,
        external::Writer, read_only_errors, run,
    },
    schema::{ensure_schema_field_ids, ensure_valid_schema, validate_schema_report},
    types::descriptor_document,
    validation::{caller, validate_create_report, validate_update_report, write_context},
};
use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::ApiResponse,
    routes::form::{
        calculate_values, calculate_values_with_existing, ensure_project_actor, find_form, find_record,
        require_form_action,
    },
};

/// `GET /api/v1/field-types`: every field type's descriptor.
#[allow(clippy::unused_async)]
pub async fn list_field_types(Extension(_claims): Extension<JwtClaims>) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(descriptor_document()))
}

#[derive(Debug, Deserialize)]
pub struct ValidateRecordRequest {
    /// `create` (default) or `update`; `update` needs `record_id`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub record_id: Option<Uuid>,
    pub values: Value,
    /// A draft schema to check the values against instead of the stored one (needs form.design).
    #[serde(default)]
    pub schema: Option<Value>,
}

/// `POST /api/v1/forms/{form_id}/validate`: runs a create or update through permission, merge,
/// default, read-only, normalize, validate, transform and the formulas, without storing it.
pub async fn validate_form_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Json(req): Json<ValidateRecordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    crate::middleware::validate_limit::check(&claims.sub)?;
    let form = find_form(&state, form_id).await?;
    let bot = bot.as_ref().map(|b| &b.0);
    let updating = match req.mode.as_deref() {
        None => req.record_id.is_some(),
        Some("create") => false,
        Some("update") => true,
        Some(_) => return Err(ApiError::BadRequest("mode must be create or update".to_string())),
    };
    let action = if updating { "record.update" } else { "record.create" };
    let (actor_id, role, is_bot) = require_form_action(&state, &claims, bot, &form, action).await?;
    let scope = SchemaScope {
        workspace_id: form.workspace_id,
        project_id: form.project_id,
    };
    let schema = match req.schema {
        Some(draft) => {
            require_form_action(&state, &claims, bot, &form, "form.design").await?;
            let draft = ensure_schema_field_ids(draft).map_err(ApiError::BadRequest)?;
            ensure_valid_schema(&draft, Some(scope))?;
            draft
        }
        None => form.schema.clone(),
    };
    ensure_field_write_policy_allows(&state, form.id, &role, &req.values).await?;
    let denied_read_fields = denied_read_field_keys(&state, form.id, &role).await?;
    let existing = if updating {
        let record_id = req
            .record_id
            .ok_or_else(|| ApiError::BadRequest("record_id is required for update".to_string()))?;
        let record = find_record(&state, record_id).await?;
        if record.form_id != form.id {
            return Err(ApiError::NotFound("record not found".to_string()));
        }
        ensure_record_scope_allows_created_by(&state, form.id, &role, actor_id, record.created_by).await?;
        Some(record.values)
    } else {
        None
    };
    let mut errors = read_only_errors(&schema, &req.values, existing.as_ref(), &denied_read_fields)?;
    let mut form_for_rules = form.clone();
    form_for_rules.schema.clone_from(&schema);
    let ctx = write_context(
        &form_for_rules,
        req.record_id.filter(|_| updating),
        caller(actor_id, &role, is_bot),
        if updating { Writer::Update } else { Writer::Create },
        denied_read_fields.clone(),
    );
    // The plugin rules are loaded first; the checks below need no connection of their own for them.
    let rules = crate::extensions::rules::PreparedRules::prepare(
        &state,
        form_for_rules.workspace_id,
        form_for_rules.project_id,
        &schema,
    )
    .await?;
    let outcome = if let Some(existing) = existing.as_ref() {
        let submitted_denied = req
            .values
            .as_object()
            .map(|submitted| {
                submitted
                    .keys()
                    .filter(|key| denied_read_fields.contains(*key))
                    .cloned()
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let calculated =
            calculate_values_with_existing(&state, &form_for_rules, req.record_id, Some(existing), req.values).await?;
        rules
            .scope(validate_update_report(
                &schema,
                &ctx,
                calculated,
                existing,
                &submitted_denied,
                true,
            ))
            .await?
    } else {
        let (with_defaults, defaulted) = apply_defaults(&schema, req.values, &caller(actor_id, &role, is_bot))?;
        let calculated = calculate_values(&state, &form_for_rules, None, with_defaults).await?;
        rules
            .scope(validate_create_report(
                &schema,
                &ctx,
                calculated,
                &defaulted,
                RulePolicy::Refuse,
            ))
            .await?
    };
    let read_policy = ReaderContext::new(&role).policy(&state.db, (&form).into()).await?;
    let (normalized, carried) = match outcome {
        Ok(output) => (
            read_policy.values(output.values),
            CarriedError::for_reader(output.carried, ctx.record_id, &denied_read_fields),
        ),
        Err(failures) => {
            errors.extend(failures);
            (Value::Null, Vec::new())
        }
    };
    Ok(ApiResponse::success(json!({
        "valid": errors.is_empty(),
        "normalized": if errors.is_empty() { normalized } else { Value::Null },
        "errors": errors.errors.iter().map(|error| error.to_json(None)).collect::<Vec<_>>(),
        "truncated": errors.truncated,
        "carried": carried,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ValidateSchemaRequest {
    pub project_id: Uuid,
    pub schema: Value,
}

/// `POST /api/v1/forms/schema/validate`: the save-time check of a schema, with field ids filled.
pub async fn validate_form_schema(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<ValidateSchemaRequest>,
) -> Result<impl IntoResponse, ApiError> {
    crate::middleware::validate_limit::check(&claims.sub)?;
    let (workspace_id, _) = ensure_project_actor(&state, &claims, bot.as_ref().map(|b| &b.0), req.project_id).await?;
    let (schema, issues) = match ensure_schema_field_ids(req.schema.clone()) {
        Ok(schema) => {
            let issues = validate_schema_report(
                &schema,
                Some(SchemaScope {
                    workspace_id,
                    project_id: req.project_id,
                }),
            );
            (schema, issues)
        }
        Err(detail) => (
            req.schema,
            SchemaIssues {
                errors: vec![SchemaIssue::new("", "field_invalid", detail)],
                warnings: Vec::new(),
            },
        ),
    };
    Ok(ApiResponse::success(json!({
        "valid": issues.is_valid(),
        "schema": schema,
        "errors": issues.errors,
        "warnings": issues.warnings,
    })))
}

/// The most samples one rules check runs.
const MAX_RULE_SAMPLES: usize = 20;

#[derive(Debug, Deserialize)]
pub struct ValidateRulesRequest {
    pub project_id: Uuid,
    pub field_type: String,
    #[serde(default)]
    pub rules: Option<Value>,
    /// The field's `default`, checked like a schema save does.
    #[serde(default)]
    pub default: Option<Value>,
    /// Options of a choice type, which its definition needs.
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub samples: Vec<Value>,
}

/// The one-field schema a rule set is checked in: the key is fixed, the field is optional.
fn rules_probe_schema(req: &ValidateRulesRequest) -> Value {
    let mut field = serde_json::Map::new();
    field.insert("field_id".to_string(), json!("fld_rules_probe"));
    field.insert("key".to_string(), json!("probe"));
    field.insert("type".to_string(), json!(req.field_type));
    for (name, value) in [
        ("rules", &req.rules),
        ("default", &req.default),
        ("options", &req.options),
    ] {
        if let Some(value) = value {
            field.insert(name.to_string(), value.clone());
        }
    }
    json!({ "version": super::rules::SCHEMA_VERSION, "fields": [Value::Object(field)] })
}

/// The probe schema without plugin entries: plugin rules are checked for installation only, never run.
fn without_plugin_rules(mut schema: Value) -> Value {
    if let Some(Value::Object(rules)) = schema.pointer_mut("/fields/0/rules") {
        for stage in rules.values_mut() {
            if let Value::Array(items) = stage {
                items.retain(|item| item.get("plugin").is_none());
            }
        }
    }
    schema
}

/// `POST /api/v1/forms/rules/validate`: checks a rule set against a field type without a form, and
/// optionally runs sample values through it (plugin rules are checked, not run). Stores nothing.
pub async fn validate_field_rules(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<ValidateRulesRequest>,
) -> Result<impl IntoResponse, ApiError> {
    crate::middleware::validate_limit::check(&claims.sub)?;
    if req.samples.len() > MAX_RULE_SAMPLES {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_RULE_SAMPLES} samples per request"
        )));
    }
    let (workspace_id, _) = ensure_project_actor(&state, &claims, bot.as_ref().map(|b| &b.0), req.project_id).await?;
    let probe = rules_probe_schema(&req);
    let issues = validate_schema_report(
        &probe,
        Some(SchemaScope {
            workspace_id,
            project_id: req.project_id,
        }),
    );
    let errors: Vec<Value> = issues
        .errors
        .iter()
        .map(|issue| {
            let path = issue.path.strip_prefix("/fields/0").unwrap_or(&issue.path);
            json!({ "path": path, "code": issue.code })
        })
        .collect();
    let mut samples = Vec::new();
    if issues.is_valid() {
        let runnable = without_plugin_rules(probe);
        let ctx = WriteContext {
            writer: Writer::Create,
            workspace_id,
            project_id: req.project_id,
            form_id: Uuid::nil(),
            record_id: None,
            caller: caller(claims.sub.parse().unwrap_or_default(), "", bot.is_some()),
            now: WriteContext::now_seconds(),
            denied_read_fields: BTreeSet::new(),
        };
        let none = BTreeSet::new();
        for (index, sample) in req.samples.iter().enumerate() {
            let outcome = run(
                &runnable,
                &ctx,
                PipelineInput {
                    values: json!({ "probe": sample }),
                    existing: None,
                    always_check: &none,
                    defaulted: &none,
                    policy: RulePolicy::Refuse,
                    report_untouched: false,
                },
            )
            .await?;
            samples.push(match outcome {
                Ok(output) => json!({
                    "index": index,
                    "valid": true,
                    "normalized": output.values.get("probe").cloned().unwrap_or(Value::Null),
                    "errors": [],
                }),
                Err(failures) => json!({
                    "index": index,
                    "valid": false,
                    "normalized": Value::Null,
                    "errors": failures
                        .errors
                        .iter()
                        .map(|error| json!({ "code": error.code, "rule": error.rule }))
                        .collect::<Vec<_>>(),
                }),
            });
        }
    }
    Ok(ApiResponse::success(json!({
        "valid": issues.is_valid(),
        "errors": errors,
        "samples": samples,
    })))
}

#[cfg(test)]
mod rules_probe_tests {
    use super::*;
    use crate::forms::rules::check_schema;

    fn request(field_type: &str, rules: Value) -> ValidateRulesRequest {
        ValidateRulesRequest {
            project_id: Uuid::nil(),
            field_type: field_type.to_string(),
            rules: Some(rules),
            default: None,
            options: None,
            samples: Vec::new(),
        }
    }

    #[test]
    fn a_known_rule_set_passes_the_schema_check_of_the_probe() {
        let req = request("text", json!({ "validate": [{ "rule": "max_length", "value": 5 }] }));
        assert!(check_schema(&rules_probe_schema(&req), None).is_valid());
    }

    #[test]
    fn an_unknown_rule_is_reported_by_the_probe() {
        let req = request("text", json!({ "validate": [{ "rule": "no_such_rule" }] }));
        let issues = check_schema(&rules_probe_schema(&req), None);
        assert!(!issues.is_valid());
        assert!(
            issues
                .errors
                .iter()
                .all(|issue| issue.path.starts_with("/fields/0/rules"))
        );
    }

    #[test]
    fn plugin_entries_are_left_out_of_the_runnable_probe() {
        let req = request(
            "text",
            json!({ "validate": [
                { "plugin": { "package": "acme/x", "rule": "r" } },
                { "rule": "max_length", "value": 5 }
            ] }),
        );
        let runnable = without_plugin_rules(rules_probe_schema(&req));
        assert_eq!(
            runnable.pointer("/fields/0/rules/validate"),
            Some(&json!([{ "rule": "max_length", "value": 5 }]))
        );
    }
}
