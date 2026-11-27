//! Record value validation as every writer runs it: type normalization, then the field rules the
//! schema declares (`forms::field_rules`).

use serde_json::Value;

use super::{
    field_rules::{apply_default_values, check_field_rules},
    values::{NormalizedValues, normalize_record_values_report, normalize_record_values_with_existing},
};
use crate::error::ApiError;

/// Create: fills declared defaults for empty fields, normalizes and checks every field rule.
pub fn validate_and_normalize_values(schema: &Value, values: Value) -> Result<Value, ApiError> {
    let values = apply_default_values(schema, values)?;
    let normalized = normalize_record_values_with_existing(schema, values, None).map_err(ApiError::BadRequest)?;
    check_field_rules(schema, &normalized, None)?;
    Ok(normalized)
}

/// Update: normalizes against the stored values and checks the rules of every value this write
/// sets or changes.
pub fn validate_and_normalize_values_with_existing(
    schema: &Value,
    values: Value,
    existing_values: &Value,
) -> Result<Value, ApiError> {
    validate_and_normalize_values_with_existing_report(schema, values, existing_values).map(|report| report.values)
}

/// Same as [`validate_and_normalize_values_with_existing`], but also reports the required fields
/// that were already unmet on the stored record and that this write did not address.
pub fn validate_and_normalize_values_with_existing_report(
    schema: &Value,
    values: Value,
    existing_values: &Value,
) -> Result<NormalizedValues, ApiError> {
    let report = normalize_record_values_report(schema, values, Some(existing_values)).map_err(ApiError::BadRequest)?;
    check_field_rules(schema, &report.values, Some(existing_values))?;
    Ok(report)
}
