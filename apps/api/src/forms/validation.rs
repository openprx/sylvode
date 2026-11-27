//! Record value validation as every writer runs it: type normalization, then the field rules the
//! schema declares (`forms::field_rules`).

use std::collections::BTreeSet;

use serde_json::Value;

use super::{
    field_rules::{FieldRuleViolation, apply_default_values, check_field_rules, field_rule_violations},
    values::{NormalizedValues, normalize_record_values_report, normalize_record_values_with_existing},
};
use crate::error::ApiError;

/// Create: fills declared defaults for empty fields, normalizes and checks every field rule.
pub fn validate_and_normalize_values(schema: &Value, values: Value) -> Result<Value, ApiError> {
    let values = apply_default_values(schema, values)?;
    let normalized = normalize_record_values_with_existing(schema, values, None).map_err(ApiError::BadRequest)?;
    check_field_rules(schema, &normalized, None, &BTreeSet::new())?;
    Ok(normalized)
}

/// Update: normalizes against the stored values, checks the rules of every value this write sets
/// or changes, and reports the required fields that were already unmet on the stored record and
/// that this write did not address. `always_check` lists keys checked even when unchanged (the
/// submitted read-denied keys; see [`check_field_rules`]).
pub fn validate_and_normalize_values_with_existing_report(
    schema: &Value,
    values: Value,
    existing_values: &Value,
    always_check: &BTreeSet<String>,
) -> Result<NormalizedValues, ApiError> {
    let report = normalize_record_values_report(schema, values, Some(existing_values)).map_err(ApiError::BadRequest)?;
    check_field_rules(schema, &report.values, Some(existing_values), always_check)?;
    Ok(report)
}

/// The values a recalculation stores and the rule violations it carries.
pub struct RecalculatedValues {
    pub values: Value,
    /// Derived outputs that break their field rules. They are stored as computed and reported, never
    /// refused: a recalculation is a system write (also run after a child record commits), and
    /// failing it would leave the user's own, already accepted write looking refused.
    pub carried: Vec<FieldRuleViolation>,
}

/// Recalculation: type normalization still refuses; field rule violations are carried.
pub fn validate_and_normalize_recalculated_values(
    schema: &Value,
    values: Value,
    existing_values: &Value,
) -> Result<RecalculatedValues, ApiError> {
    let values =
        normalize_record_values_with_existing(schema, values, Some(existing_values)).map_err(ApiError::BadRequest)?;
    let carried = field_rule_violations(schema, &values, Some(existing_values), &BTreeSet::new())?;
    Ok(RecalculatedValues { values, carried })
}
