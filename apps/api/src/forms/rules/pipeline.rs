//! The write pipeline after type normalisation (contract C2 §6): default (create), read-only,
//! normalize ops, validate rules, transform ops. The callers run permission, merge, formulas and
//! store around it; every writer goes through the same functions, so the same violation is refused
//! with the same code from the console, the API, an import or an MCP tool.
//!
//! Stages 5-7 run for the values this write sets or changes (every value on create). A key in
//! `always_check` runs even when unchanged: the update path passes the submitted read-denied keys,
//! so a refusal never reveals whether a guess matched the hidden stored value. A stored value that
//! breaks a rule added later and that the write does not touch never blocks it.

use std::{borrow::Cow, collections::BTreeSet};

use serde_json::{Map, Value, json};
use sv_rules::{BuiltinRule, Operand, PluginRuleRef, RuleSpecKind, Vars, check_builtin};
use uuid::Uuid;

use super::{
    external::{
        ExternalRuleCall, ExternalRuleOutcome, ExternalRuleRef, ExternalStage, RuleCaller, Writer, is_violation_code,
        resolver,
    },
    plan::{DefaultSpec, FieldPlan, FormPlan, OpStep, ValidateStep, write_plan},
    report::{FieldError, FieldErrors, Stage},
};
use crate::{
    error::ApiError,
    forms::{
        types::{FieldType, Outlet, OutletRule},
        values::{NormalizedValues, is_missing, normalize_field_value, normalize_record_values_report},
    },
};

/// Who writes where, for CEL `user`/`now`, plugin calls and the `record` binding.
#[derive(Clone, Debug)]
pub struct WriteContext {
    pub writer: Writer,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub record_id: Option<Uuid>,
    pub caller: RuleCaller,
    /// Request time, Unix seconds.
    pub now: i64,
    /// Fields the caller may not read: left out of CEL `record` and plugin deps.
    pub denied_read_fields: BTreeSet<String>,
}

impl WriteContext {
    #[must_use]
    pub fn now_seconds() -> i64 {
        time::OffsetDateTime::now_utc().unix_timestamp()
    }
}

/// What happens to a rule failure (stage 6 and plugin violations).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulePolicy {
    /// A user write: every failure refuses.
    Refuse,
    /// A recalculation preview: failures of server-written fields are carried, others refuse.
    CarryDerived,
    /// A recalculation: every rule failure is carried (stored as computed). Type errors still refuse.
    CarryAll,
}

/// The input of [`run`].
pub struct PipelineInput<'a> {
    pub values: Value,
    pub existing: Option<&'a Value>,
    pub always_check: &'a BTreeSet<String>,
    /// Keys whose value came from the field's `default`: their errors are `default_invalid`.
    pub defaulted: &'a BTreeSet<String>,
    pub policy: RulePolicy,
    /// Also check stored values this write does not touch, reporting failures as carried (the
    /// validate endpoint).
    pub report_untouched: bool,
}

/// The values to store, the required fields carried over, and the carried rule failures.
#[derive(Debug)]
pub struct PipelineOutput {
    pub values: Value,
    pub unmet_required_fields: Vec<String>,
    pub carried: Vec<FieldError>,
}

/// Runs type normalisation and the rule stages. The outer error is a server failure; the inner one
/// the field errors of a refused write.
pub async fn run(
    schema: &Value,
    ctx: &WriteContext,
    input: PipelineInput<'_>,
) -> Result<Result<PipelineOutput, FieldErrors>, ApiError> {
    let plan = write_plan(schema).map_err(ApiError::BadRequest)?;
    let submitted: BTreeSet<String> = input
        .values
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    let NormalizedValues {
        values,
        unmet_required_fields,
    } = match normalize_record_values_report(schema, input.values, input.existing) {
        Ok(report) => report,
        Err(errors) => return Ok(Err(relabel_defaulted(errors, input.defaulted))),
    };
    let Value::Object(mut values) = values else {
        return Ok(Ok(PipelineOutput {
            values,
            unmet_required_fields,
            carried: Vec::new(),
        }));
    };
    let creating = input.existing.is_none();
    let record = cel_record(&plan, &values, &ctx.denied_read_fields);
    let mut errors = FieldErrors::default();
    let mut carried = Vec::new();
    for field_plan in plan.fields.iter().filter(|plan| plan.has_rules()) {
        let key = field_plan.field.key.as_str();
        let stored = input.existing.and_then(|existing| existing.get(key));
        let Some(value) = values.get(key).cloned() else {
            if creating || submitted.contains(key) {
                let outcome = run_on_empty(field_plan, ctx, &record, &values).await;
                route(
                    field_plan,
                    outcome,
                    input.policy,
                    input.defaulted,
                    &mut errors,
                    &mut carried,
                );
            }
            continue;
        };
        if is_missing(Some(&value)) {
            continue;
        }
        let touched = input.always_check.contains(key) || stored != Some(&value);
        if !touched {
            if input.report_untouched {
                let failures = validate_value(field_plan, ctx, &record, &values, &value).await;
                carried.extend(failures.errors);
            }
            continue;
        }
        match field_stages(
            field_plan,
            ctx,
            &record,
            &values,
            value,
            stored,
            input.always_check.contains(key),
        )
        .await
        {
            FieldOutcome::Value(next) => {
                values.insert(key.to_string(), next);
            }
            FieldOutcome::Cleared => {
                values.remove(key);
                if field_plan.field.required {
                    errors.push(error(field_plan, Stage::Validate, "required"));
                }
            }
            FieldOutcome::Refused(failures) => errors.extend(relabel_defaulted(failures, input.defaulted)),
            FieldOutcome::RuleFailures(value, failures) => {
                if carries(field_plan, input.policy) {
                    values.insert(key.to_string(), value);
                    carried.extend(failures.errors);
                } else {
                    errors.extend(relabel_defaulted(failures, input.defaulted));
                }
            }
        }
    }
    if !errors.is_empty() {
        return Ok(Err(errors));
    }
    Ok(Ok(PipelineOutput {
        values: Value::Object(values),
        unmet_required_fields,
        carried,
    }))
}

const fn carries(plan: &FieldPlan, policy: RulePolicy) -> bool {
    match policy {
        RulePolicy::Refuse => false,
        RulePolicy::CarryDerived => plan.derived,
        RulePolicy::CarryAll => true,
    }
}

fn route(
    plan: &FieldPlan,
    failures: FieldErrors,
    policy: RulePolicy,
    defaulted: &BTreeSet<String>,
    errors: &mut FieldErrors,
    carried: &mut Vec<FieldError>,
) {
    if failures.is_empty() {
        return;
    }
    if carries(plan, policy) {
        carried.extend(failures.errors);
    } else {
        errors.extend(relabel_defaulted(failures, defaulted));
    }
}

/// An error on a value that came from the field's default is the schema's fault: `default_invalid`.
fn relabel_defaulted(mut errors: FieldErrors, defaulted: &BTreeSet<String>) -> FieldErrors {
    for error in &mut errors.errors {
        if defaulted.contains(&error.key) {
            error.stage = Stage::Default;
            error.code = Cow::Borrowed("default_invalid");
            error.rule = None;
            error.params = None;
        }
    }
    errors
}

fn error(plan: &FieldPlan, stage: Stage, code: impl Into<Cow<'static, str>>) -> FieldError {
    FieldError::new(&plan.field.key, &plan.field.field_id, stage, code)
}

enum FieldOutcome {
    Value(Value),
    Cleared,
    /// A type error or a refused plugin call: refuses even a system writer.
    Refused(FieldErrors),
    /// Stage 6 failures, with the value as normalised (stored as computed when carried).
    RuleFailures(Value, FieldErrors),
}

async fn field_stages(
    plan: &FieldPlan,
    ctx: &WriteContext,
    record: &Map<String, Value>,
    values: &Map<String, Value>,
    value: Value,
    stored: Option<&Value>,
    always_check: bool,
) -> FieldOutcome {
    let value = match run_ops(
        plan,
        ctx,
        values,
        value,
        stored,
        &plan.normalize,
        ExternalStage::Normalize,
    )
    .await
    {
        Ok(value) => value,
        Err(failures) => return FieldOutcome::Refused(failures),
    };
    if is_missing(Some(&value)) {
        return FieldOutcome::Cleared;
    }
    if !always_check && stored == Some(&value) {
        return FieldOutcome::Value(value);
    }
    let failures = validate_value(plan, ctx, record, values, &value).await;
    if !failures.is_empty() {
        return FieldOutcome::RuleFailures(value, failures);
    }
    match run_ops(
        plan,
        ctx,
        values,
        value,
        stored,
        &plan.transform,
        ExternalStage::Transform,
    )
    .await
    {
        Ok(value) if is_missing(Some(&value)) => FieldOutcome::Cleared,
        Ok(value) => FieldOutcome::Value(value),
        Err(failures) => FieldOutcome::Refused(failures),
    }
}

/// Applies one op stage and re-normalises the result by type. A type error after a transform is
/// `transform_invalid`; after a normalize op it is the type's own code.
async fn run_ops(
    plan: &FieldPlan,
    ctx: &WriteContext,
    values: &Map<String, Value>,
    mut value: Value,
    stored: Option<&Value>,
    steps: &[OpStep],
    stage: ExternalStage,
) -> Result<Value, FieldErrors> {
    if steps.is_empty() {
        return Ok(value);
    }
    let report_stage = match stage {
        ExternalStage::Transform => Stage::Transform,
        _ => Stage::Normalize,
    };
    for step in steps {
        match step {
            OpStep::Builtin { op, .. } => value = op.apply(&value),
            OpStep::Plugin {
                index,
                rule,
                run_on_empty,
            } => {
                if is_missing(Some(&value)) && !run_on_empty {
                    continue;
                }
                let label = format!("{}[{index}]", stage.name());
                match call_plugin(plan, ctx, values, rule, &value, stage).await {
                    Ok(ExternalRuleOutcome::Keep) => {}
                    Ok(ExternalRuleOutcome::Replace(next)) => value = next,
                    Ok(ExternalRuleOutcome::Violation { code }) => {
                        return Err(FieldErrors::single(
                            error(plan, report_stage, plugin_code(&rule.package, &code)).with_rule(label, None),
                        ));
                    }
                    Err(code) => {
                        return Err(FieldErrors::single(
                            error(plan, report_stage, code).with_rule(label, None),
                        ));
                    }
                }
            }
        }
    }
    if is_missing(Some(&value)) {
        return Ok(value);
    }
    normalize_field_value(&plan.field, &value, stored).map_err(|type_error| {
        let code = if stage == ExternalStage::Transform {
            "transform_invalid"
        } else {
            type_error.code()
        };
        FieldErrors::single(error(plan, report_stage, code))
    })
}

/// Stage 6 for a non-empty value: built-in rules, CEL, plugin rules; every failure collected.
async fn validate_value(
    plan: &FieldPlan,
    ctx: &WriteContext,
    record: &Map<String, Value>,
    values: &Map<String, Value>,
    value: &Value,
) -> FieldErrors {
    let mut failures = FieldErrors::default();
    for step in &plan.validate {
        if let Some(failure) = validate_step(plan, ctx, record, values, step, Some(value)).await {
            failures.push(failure);
        }
    }
    failures
}

/// Stage 6 for an empty value: only `cel` and `plugin` rules with `run_on_empty`.
async fn run_on_empty(
    plan: &FieldPlan,
    ctx: &WriteContext,
    record: &Map<String, Value>,
    values: &Map<String, Value>,
) -> FieldErrors {
    let mut failures = FieldErrors::default();
    for step in plan.validate.iter().filter(|step| step.spec.run_on_empty) {
        if let Some(failure) = validate_step(plan, ctx, record, values, step, None).await {
            failures.push(failure);
        }
    }
    failures
}

async fn validate_step(
    plan: &FieldPlan,
    ctx: &WriteContext,
    record: &Map<String, Value>,
    values: &Map<String, Value>,
    step: &ValidateStep,
    value: Option<&Value>,
) -> Option<FieldError> {
    let label = format!("validate[{}]", step.index);
    let override_code = step.spec.code.clone();
    match &step.spec.kind {
        RuleSpecKind::Builtin(rule) => {
            let value = value?;
            let code = check_builtin_value(plan.ty, &plan.field, rule, value).err()?;
            let code: Cow<'static, str> = match override_code {
                Some(custom) if code != "rule_invalid" => Cow::Owned(custom),
                _ => Cow::Borrowed(code),
            };
            Some(error(plan, Stage::Validate, code).with_rule(label, rule.params()))
        }
        RuleSpecKind::Cel(program) => {
            let bound = value
                .and_then(|value| plan.ty.cel_value(&plan.field, value))
                .unwrap_or(Value::Null);
            let vars = Vars::new()
                .with("value", bound)
                .with("record", Value::Object(record.clone()))
                .with("now", json!(ctx.now))
                .with("user", cel_user(&ctx.caller));
            let code: Cow<'static, str> = match program.eval_bool(&vars) {
                Ok(true) => return None,
                Ok(false) => override_code.map_or(Cow::Borrowed("cel_failed"), Cow::Owned),
                Err(eval_error) => Cow::Borrowed(eval_error.code),
            };
            Some(error(plan, Stage::Validate, code).with_rule(label, None))
        }
        RuleSpecKind::Plugin(rule) => {
            let empty = Value::Null;
            let code: Cow<'static, str> = match call_plugin(
                plan,
                ctx,
                values,
                rule,
                value.unwrap_or(&empty),
                ExternalStage::Validate,
            )
            .await
            {
                Ok(ExternalRuleOutcome::Keep) => return None,
                Ok(ExternalRuleOutcome::Replace(_)) => Cow::Borrowed("plugin_failed"),
                Ok(ExternalRuleOutcome::Violation { code }) => plugin_code(&rule.package, &code),
                Err(code) => Cow::Borrowed(code),
            };
            Some(error(plan, Stage::Validate, code).with_rule(label, None))
        }
    }
}

/// A built-in rule against a value; `enum` on a list checks every item.
fn check_builtin_value(
    ty: &dyn FieldType,
    field: &crate::forms::schema::FormField,
    rule: &BuiltinRule,
    value: &Value,
) -> Result<(), &'static str> {
    if let (BuiltinRule::Enum(_), Value::Array(items)) = (rule, value) {
        for item in items {
            let text = item.as_str().ok_or("rule_invalid")?;
            check_builtin(rule, &Operand::Str(text.to_string()))?;
        }
        return Ok(());
    }
    // A value the rule cannot compare (an unparsable date) fails the rule closed.
    let operand = ty.operand(field, value).ok_or_else(|| rule.kind().name())?;
    check_builtin(rule, &operand)
}

fn plugin_code(package: &str, code: &str) -> Cow<'static, str> {
    if is_violation_code(code) {
        Cow::Owned(format!("plugin.{package}.{code}"))
    } else {
        Cow::Borrowed("plugin_failed")
    }
}

async fn call_plugin(
    plan: &FieldPlan,
    ctx: &WriteContext,
    values: &Map<String, Value>,
    rule: &PluginRuleRef,
    value: &Value,
    stage: ExternalStage,
) -> Result<ExternalRuleOutcome, &'static str> {
    let reference = ExternalRuleRef {
        package_key: rule.package.clone(),
        rule: rule.rule.clone(),
    };
    let external = resolver()
        .resolve(ctx.workspace_id, ctx.project_id, &reference)
        .map_err(|refusal| refusal.code())?;
    let deps = rule
        .deps
        .iter()
        .filter(|dep| !ctx.denied_read_fields.contains(*dep))
        .filter_map(|dep| values.get(dep).map(|value| (dep.clone(), value.clone())))
        .collect::<Map<String, Value>>();
    let outcome = external
        .apply(ExternalRuleCall {
            stage,
            writer: ctx.writer,
            workspace_id: ctx.workspace_id,
            project_id: ctx.project_id,
            form_id: ctx.form_id,
            record_id: ctx.record_id,
            field_key: &plan.field.key,
            field_type: plan.ty.name(),
            value,
            deps: &deps,
            config: &rule.config,
            caller: &ctx.caller,
        })
        .await
        .map_err(|refusal| refusal.code())?;
    if matches!(outcome, ExternalRuleOutcome::Replace(_)) && stage == ExternalStage::Validate {
        return Err("plugin_failed");
    }
    Ok(outcome)
}

/// CEL `record`: the merged values of fields with a CEL type that the caller may read.
fn cel_record(plan: &FormPlan, values: &Map<String, Value>, denied: &BTreeSet<String>) -> Map<String, Value> {
    plan.fields
        .iter()
        .filter(|field| !denied.contains(&field.field.key))
        .filter(|field| field.ty.cel_type().is_some())
        .filter_map(|field| {
            let value = values.get(&field.field.key)?;
            field
                .ty
                .cel_value(&field.field, value)
                .map(|bound| (field.field.key.clone(), bound))
        })
        .collect()
}

fn cel_user(caller: &RuleCaller) -> Value {
    json!({
        "id": caller.actor_id.map(|id| id.to_string()).unwrap_or_default(),
        "role": caller.role,
    })
}

/// Fills absent keys with their field's default on create. An explicit `null` stays empty.
///
/// A CEL default is evaluated with `now` and `user`; a failing one refuses the create with
/// `default_invalid`. Returns the values and the keys that were filled.
pub fn apply_defaults(
    schema: &Value,
    values: Value,
    caller: &RuleCaller,
) -> Result<(Value, BTreeSet<String>), ApiError> {
    let Value::Object(mut object) = values else {
        return Ok((values, BTreeSet::new()));
    };
    let plan = write_plan(schema).map_err(ApiError::BadRequest)?;
    let mut defaulted = BTreeSet::new();
    let mut errors = FieldErrors::default();
    let vars = Vars::new()
        .with("now", json!(WriteContext::now_seconds()))
        .with("user", cel_user(caller));
    for field in &plan.fields {
        if object.contains_key(&field.field.key) {
            continue;
        }
        let input = match &field.default {
            None => continue,
            Some(DefaultSpec::Value(default)) => default_input(field.ty, default),
            Some(DefaultSpec::Cel(program)) => {
                if let Ok(Some(value)) = program.eval_json(&vars).map(|value| cel_default_input(field.ty, value)) {
                    value
                } else {
                    errors.push(error(field, Stage::Default, "default_invalid"));
                    continue;
                }
            }
        };
        object.insert(field.field.key.clone(), input);
        defaulted.insert(field.field.key.clone());
    }
    if !errors.is_empty() {
        return Err(errors.into_api_error());
    }
    Ok((Value::Object(object), defaulted))
}

/// A CEL default result as type input: a timestamp field's Unix seconds become RFC 3339 text.
fn cel_default_input(ty: &dyn FieldType, value: Value) -> Option<Value> {
    if ty.cel_type() == Some(crate::forms::types::CelType::Timestamp) {
        let seconds = value.as_i64()?;
        let moment = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?;
        return moment
            .format(&time::format_description::well_known::Rfc3339)
            .ok()
            .map(Value::String);
    }
    Some(value)
}

/// Decimal fields refuse JSON numbers from callers; a designer default may still be one.
fn default_input(ty: &dyn FieldType, default: &Value) -> Value {
    match default {
        Value::Number(number) if ty.number_default_as_text() => Value::String(number.to_string()),
        _ => default.clone(),
    }
}

/// Schema save: a default value normalises under its type and built-in normalize ops and passes
/// the field's built-in rules. CEL and plugin rules are checked when a record is written.
pub fn check_default_value(plan: &FieldPlan, default: &Value) -> bool {
    let input = default_input(plan.ty, default);
    let Ok(mut value) = plan.ty.normalize(&plan.field, &input, None) else {
        return false;
    };
    for step in &plan.normalize {
        if let OpStep::Builtin { op, .. } = step {
            value = op.apply(&value);
        }
    }
    if is_missing(Some(&value)) {
        return true;
    }
    let Ok(value) = plan.ty.normalize(&plan.field, &value, None) else {
        return false;
    };
    plan.validate.iter().all(|step| match &step.spec.kind {
        RuleSpecKind::Builtin(rule) => check_builtin_value(plan.ty, &plan.field, rule, &value).is_ok(),
        _ => true,
    })
}

/// Stage 4: refuses a write that sets or changes a `read_only` field. Admins and owners are bound
/// too; only server writers (defaults, formulas, recalculation) set such a field.
///
/// `submitted` is the caller's own value map, before formulas and defaults. On create a value equal
/// to the field's default is accepted (clients prefill it). On update a value equal to the stored
/// one is accepted (clients send back every field), except for a field the caller may not read:
/// there any submitted key, `null` included, is refused, so the answer never says whether a guess
/// matched or whether the hidden value is empty. An empty value is a clear, accepted only when
/// nothing is stored.
pub fn ensure_read_only_untouched(
    schema: &Value,
    submitted: &Value,
    existing: Option<&Value>,
    denied_read_fields: &BTreeSet<String>,
) -> Result<(), ApiError> {
    let errors = read_only_errors(schema, submitted, existing, denied_read_fields)?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.into_api_error())
    }
}

/// The stage-4 errors of a write, without refusing (the validate endpoint).
pub fn read_only_errors(
    schema: &Value,
    submitted: &Value,
    existing: Option<&Value>,
    denied_read_fields: &BTreeSet<String>,
) -> Result<FieldErrors, ApiError> {
    let Some(submitted) = submitted.as_object().filter(|map| !map.is_empty()) else {
        return Ok(FieldErrors::default());
    };
    let plan = write_plan(schema).map_err(ApiError::BadRequest)?;
    let mut errors = FieldErrors::default();
    for field in plan.fields.iter().filter(|field| field.read_only) {
        let Some(raw) = submitted.get(&field.field.key) else {
            continue;
        };
        let allowed = if existing.is_some() && denied_read_fields.contains(&field.field.key) {
            false
        } else {
            read_only_write_allowed(field, raw, existing)
        };
        if !allowed {
            errors.push(error(field, Stage::ReadOnly, "read_only"));
        }
    }
    Ok(errors)
}

fn read_only_write_allowed(field: &FieldPlan, raw: &Value, existing: Option<&Value>) -> bool {
    let Some(existing) = existing else {
        if is_missing(Some(raw)) {
            return true;
        }
        let Some(DefaultSpec::Value(default)) = &field.default else {
            return false;
        };
        return match (
            normalize_field_value(&field.field, &default_input(field.ty, default), None),
            normalize_field_value(&field.field, raw, None),
        ) {
            (Ok(default), Ok(value)) => default == value,
            _ => false,
        };
    };
    let stored = existing.get(&field.field.key);
    if is_missing(Some(raw)) {
        return is_missing(stored);
    }
    let Some(stored) = stored else {
        return false;
    };
    normalize_field_value(&field.field, raw, Some(stored)).is_ok_and(|value| &value == stored)
}

/// Whether the outlet table lets a field be filtered or sorted on.
pub fn ensure_queryable(field: &crate::forms::schema::FormField, outlet: Outlet) -> Result<(), ApiError> {
    if crate::forms::types::field_outlet(field, outlet) == OutletRule::Reject {
        return Err(ApiError::Coded {
            status: 400,
            code: "field_not_filterable",
            message: format!("field '{}' cannot be used to filter or sort", field.key),
            details: Some(json!({ "field": field.key })),
        });
    }
    Ok(())
}
