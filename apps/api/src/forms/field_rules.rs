//! Field rules a schema declares and the server enforces on every write.
//!
//! The designer saves `validation.{min,max,pattern,max_length}`, `default_value` and
//! `policy.read_only` on a field. They used to be HTML attributes only; this module makes them
//! server rules:
//!
//! - `validation.min` / `validation.max`: numeric bounds for `number`, `integer`, `amount`,
//!   `rating` and `progress`; ISO string bounds (compared as text) for `date` and `datetime`.
//! - `validation.pattern`: the whole string value must match (anchored, RE2-class syntax).
//! - `validation.max_length`: the string value may hold at most this many characters.
//! - `default_value`: written on create when the caller leaves the field empty.
//! - `policy.read_only`: no caller may set or change the value; only the default, formulas and
//!   other server computations write it.
//!
//! Violations are collected per field and reported with a stable code; the value, the bound and
//! the pattern are never echoed. A rule is checked when a write sets or changes a value, so a
//! stored value that predates a rule does not block unrelated writes to the same record.

use std::collections::BTreeMap;

use regex_automata::meta::Regex;
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};

use super::{
    schema::{FormField, normalize_key, parse_fields},
    values::{is_missing, normalize_field_value},
};
use crate::error::ApiError;

/// Stable error code of a write refused by one or more field rules.
pub const FIELD_RULES_FAILED: &str = "field_rules_failed";

const MAX_PATTERN_LENGTH: usize = 512;
const PATTERN_NFA_SIZE_LIMIT: usize = 1 << 20;
const MAX_MAX_LENGTH: u64 = 1_000_000;

/// Why a field refused a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRuleCode {
    BelowMin,
    AboveMax,
    PatternMismatch,
    TooLong,
    ReadOnly,
}

impl FieldRuleCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BelowMin => "below_min",
            Self::AboveMax => "above_max",
            Self::PatternMismatch => "pattern_mismatch",
            Self::TooLong => "too_long",
            Self::ReadOnly => "read_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRuleViolation {
    pub field_key: String,
    pub code: FieldRuleCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Bound {
    Number(Decimal),
    Text(String),
}

#[derive(Debug, Clone, Default)]
struct FieldRules {
    min: Option<Bound>,
    max: Option<Bound>,
    pattern: Option<Regex>,
    max_length: Option<usize>,
    default_value: Option<Value>,
    read_only: bool,
}

impl FieldRules {
    fn is_empty(&self) -> bool {
        self.min.is_none()
            && self.max.is_none()
            && self.pattern.is_none()
            && self.max_length.is_none()
            && self.default_value.is_none()
            && !self.read_only
    }
}

/// The refusal for a write that broke field rules: one `{field_key, code}` per violation.
pub fn field_rules_error(violations: &[FieldRuleViolation]) -> ApiError {
    let summary = violations
        .iter()
        .map(|violation| format!("{} ({})", violation.field_key, violation.code.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let fields = violations
        .iter()
        .map(|violation| json!({ "field_key": violation.field_key, "code": violation.code.as_str() }))
        .collect::<Vec<_>>();
    ApiError::Coded {
        status: 400,
        code: FIELD_RULES_FAILED,
        message: format!("form field rules failed: {summary}"),
        details: Some(json!({ "fields": fields })),
    }
}

/// Checks the rule declarations of a schema: bounds parse, the pattern compiles within its
/// limits, `max_length` is a positive integer and each default value is a valid value of its
/// field that passes the field's own rules.
pub fn validate_schema_field_rules(schema: &Value) -> Result<(), String> {
    let fields = parse_fields(schema)?;
    let rules = parse_field_rules(schema)?;
    for field in &fields {
        let Some(field_rules) = rules.get(&field.key) else {
            continue;
        };
        if let (Some(Bound::Number(min)), Some(Bound::Number(max))) = (&field_rules.min, &field_rules.max)
            && min > max
        {
            return Err(format!("field '{}' validation.min exceeds validation.max", field.key));
        }
        if let Some(default_value) = &field_rules.default_value {
            let normalized = normalize_default_value(field, default_value)
                .map_err(|_| format!("field '{}' default_value is not a valid value", field.key))?;
            if !value_violations(field, field_rules, &normalized).is_empty() {
                return Err(format!("field '{}' default_value breaks the field rules", field.key));
            }
        }
    }
    Ok(())
}

/// Fills fields the caller left empty on create with their declared default value.
pub fn apply_default_values(schema: &Value, values: Value) -> Result<Value, ApiError> {
    let Value::Object(mut object) = values else {
        return Ok(values);
    };
    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    let rules = parse_field_rules(schema).map_err(ApiError::BadRequest)?;
    for field in &fields {
        let Some(default_value) = rules.get(&field.key).and_then(|rules| rules.default_value.as_ref()) else {
            continue;
        };
        if is_missing(object.get(&field.key)) {
            object.insert(field.key.clone(), default_for_input(field, default_value));
        }
    }
    Ok(Value::Object(object))
}

/// Refuses a write that sets or changes a `policy.read_only` field.
///
/// `submitted` is the caller's own value map, before formulas and defaults. On create a value equal
/// to the field's default is accepted (clients prefill it); on update a value equal to the stored
/// one is accepted (clients send back every field). An empty value is a clear, accepted only when
/// nothing is stored.
pub fn ensure_read_only_fields_untouched(
    schema: &Value,
    submitted: &Value,
    existing: Option<&Value>,
) -> Result<(), ApiError> {
    let Some(submitted) = submitted.as_object() else {
        return Ok(());
    };
    if submitted.is_empty() {
        return Ok(());
    }
    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    let rules = parse_field_rules(schema).map_err(ApiError::BadRequest)?;
    let mut violations = Vec::new();
    for field in &fields {
        let Some(field_rules) = rules.get(&field.key).filter(|rules| rules.read_only) else {
            continue;
        };
        let Some(raw) = submitted.get(&field.key) else {
            continue;
        };
        if !read_only_write_allowed(field, field_rules, raw, existing) {
            violations.push(FieldRuleViolation {
                field_key: field.key.clone(),
                code: FieldRuleCode::ReadOnly,
            });
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(field_rules_error(&violations))
    }
}

/// Checks `validation.*` on the normalized values of a write.
///
/// A field is checked when it holds a value that this write set or changed (every value on create,
/// `existing` is `None`).
pub fn check_field_rules(schema: &Value, normalized: &Value, existing: Option<&Value>) -> Result<(), ApiError> {
    let Some(values) = normalized.as_object() else {
        return Ok(());
    };
    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    let rules = parse_field_rules(schema).map_err(ApiError::BadRequest)?;
    let mut violations = Vec::new();
    for field in &fields {
        let Some(field_rules) = rules.get(&field.key) else {
            continue;
        };
        let Some(value) = values.get(&field.key) else {
            continue;
        };
        if is_missing(Some(value)) {
            continue;
        }
        if existing.and_then(|existing| existing.get(&field.key)) == Some(value) {
            continue;
        }
        violations.extend(
            value_violations(field, field_rules, value)
                .into_iter()
                .map(|code| FieldRuleViolation {
                    field_key: field.key.clone(),
                    code,
                }),
        );
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(field_rules_error(&violations))
    }
}

fn read_only_write_allowed(field: &FormField, rules: &FieldRules, raw: &Value, existing: Option<&Value>) -> bool {
    let Some(existing) = existing else {
        if is_missing(Some(raw)) {
            return true;
        }
        return rules.default_value.as_ref().is_some_and(|default_value| {
            match (
                normalize_default_value(field, default_value),
                normalize_field_value(field, raw, None),
            ) {
                (Ok(default_value), Ok(value)) => default_value == value,
                _ => false,
            }
        });
    };
    let stored = existing.get(&field.key);
    if is_missing(Some(raw)) {
        return is_missing(stored);
    }
    let Some(stored) = stored else {
        return false;
    };
    normalize_field_value(field, raw, Some(stored)).is_ok_and(|value| &value == stored)
}

fn value_violations(field: &FormField, rules: &FieldRules, value: &Value) -> Vec<FieldRuleCode> {
    let mut violations = Vec::new();
    if let Some(number) = numeric_value(field, value) {
        if let Some(Bound::Number(min)) = &rules.min
            && number < *min
        {
            violations.push(FieldRuleCode::BelowMin);
        }
        if let Some(Bound::Number(max)) = &rules.max
            && number > *max
        {
            violations.push(FieldRuleCode::AboveMax);
        }
    }
    if matches!(field.field_type.as_str(), "date" | "datetime")
        && let Some(text) = value.as_str()
    {
        if let Some(Bound::Text(min)) = &rules.min
            && text < min.as_str()
        {
            violations.push(FieldRuleCode::BelowMin);
        }
        if let Some(Bound::Text(max)) = &rules.max
            && text > max.as_str()
        {
            violations.push(FieldRuleCode::AboveMax);
        }
    }
    if let Some(text) = value.as_str() {
        if let Some(max_length) = rules.max_length
            && text.chars().count() > max_length
        {
            violations.push(FieldRuleCode::TooLong);
        }
        if let Some(pattern) = &rules.pattern
            && !pattern.is_match(text)
        {
            violations.push(FieldRuleCode::PatternMismatch);
        }
    }
    violations
}

fn numeric_value(field: &FormField, value: &Value) -> Option<Decimal> {
    match field.field_type.as_str() {
        "integer" | "rating" | "progress" => value.get("value").and_then(Value::as_i64).map(Decimal::from),
        "number" | "amount" => value
            .get("decimal")
            .and_then(Value::as_str)
            .and_then(|decimal| decimal.parse::<Decimal>().ok()),
        _ => None,
    }
}

/// Decimal fields refuse JSON numbers from callers; a designer default may still be one.
fn default_for_input(field: &FormField, default_value: &Value) -> Value {
    match (field.field_type.as_str(), default_value) {
        ("number" | "amount", Value::Number(number)) => Value::String(number.to_string()),
        _ => default_value.clone(),
    }
}

fn normalize_default_value(field: &FormField, default_value: &Value) -> Result<Value, String> {
    normalize_field_value(field, &default_for_input(field, default_value), None)
}

fn parse_field_rules(schema: &Value) -> Result<BTreeMap<String, FieldRules>, String> {
    let mut rules = BTreeMap::new();
    let Some(fields) = schema.get("fields").and_then(Value::as_array) else {
        return Ok(rules);
    };
    for field in fields {
        let Some(object) = field.as_object() else {
            continue;
        };
        let Some(key) = object.get("key").and_then(Value::as_str) else {
            continue;
        };
        let key = normalize_key(key)?;
        let field_rules = parse_rules_object(&key, object)?;
        if !field_rules.is_empty() {
            rules.insert(key, field_rules);
        }
    }
    Ok(rules)
}

fn parse_rules_object(key: &str, object: &Map<String, Value>) -> Result<FieldRules, String> {
    let mut rules = FieldRules {
        default_value: object.get("default_value").filter(|value| !value.is_null()).cloned(),
        read_only: object
            .get("policy")
            .and_then(|policy| policy.get("read_only"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..FieldRules::default()
    };
    let Some(validation) = object.get("validation") else {
        return Ok(rules);
    };
    if validation.is_null() {
        return Ok(rules);
    }
    let validation = validation
        .as_object()
        .ok_or_else(|| format!("field '{key}' validation must be an object"))?;
    rules.min = parse_bound(key, "min", validation.get("min"))?;
    rules.max = parse_bound(key, "max", validation.get("max"))?;
    rules.max_length = match validation.get("max_length") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let max_length = value
                .as_u64()
                .filter(|max_length| (1..=MAX_MAX_LENGTH).contains(max_length))
                .ok_or_else(|| format!("field '{key}' validation.max_length must be a positive integer"))?;
            Some(usize::try_from(max_length).map_err(|_| format!("field '{key}' validation.max_length is too large"))?)
        }
    };
    rules.pattern = match validation.get("pattern") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let pattern = value
                .as_str()
                .ok_or_else(|| format!("field '{key}' validation.pattern must be a string"))?;
            if pattern.is_empty() {
                None
            } else {
                Some(compile_pattern(key, pattern)?)
            }
        }
    };
    Ok(rules)
}

fn parse_bound(key: &str, name: &str, value: Option<&Value>) -> Result<Option<Bound>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .to_string()
            .parse::<Decimal>()
            .map(|number| Some(Bound::Number(number)))
            .map_err(|_| format!("field '{key}' validation.{name} is not a valid number")),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            Ok(Some(
                text.parse::<Decimal>()
                    .map_or_else(|_| Bound::Text(text.to_string()), Bound::Number),
            ))
        }
        Some(_) => Err(format!("field '{key}' validation.{name} must be a number or a string")),
    }
}

fn compile_pattern(key: &str, pattern: &str) -> Result<Regex, String> {
    if pattern.len() > MAX_PATTERN_LENGTH {
        return Err(format!("field '{key}' validation.pattern is too long"));
    }
    Regex::builder()
        .configure(Regex::config().nfa_size_limit(Some(PATTERN_NFA_SIZE_LIMIT)))
        .build(&format!("^(?:{pattern})$"))
        .map_err(|_| format!("field '{key}' validation.pattern is not a valid pattern"))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{
        FIELD_RULES_FAILED, apply_default_values, check_field_rules, ensure_read_only_fields_untouched,
        validate_schema_field_rules,
    };
    use crate::error::ApiError;
    use serde_json::{Value, json};

    fn schema() -> Value {
        json!({
            "fields": [
                {"field_id": "fld_qty_0001", "key": "quantity", "type": "integer",
                 "validation": {"min": 1, "max": 10}},
                {"field_id": "fld_price_001", "key": "price", "type": "number",
                 "validation": {"min": "0.5", "max": "99.5"}, "default_value": 1.5},
                {"field_id": "fld_code_0001", "key": "code", "type": "text",
                 "validation": {"pattern": "[A-Z]{3}-[0-9]+", "max_length": 8}},
                {"field_id": "fld_day_00001", "key": "day", "type": "date",
                 "validation": {"min": "2026-01-01", "max": "2026-12-31"}},
                {"field_id": "fld_state_001", "key": "state", "type": "text",
                 "default_value": "new", "policy": {"read_only": true}}
            ]
        })
    }

    fn codes(error: ApiError) -> Vec<(String, String)> {
        match error {
            ApiError::Coded {
                status, code, details, ..
            } => {
                assert_eq!(status, 400);
                assert_eq!(code, FIELD_RULES_FAILED);
                details.expect("details")["fields"]
                    .as_array()
                    .expect("fields")
                    .iter()
                    .map(|item| {
                        (
                            item["field_key"].as_str().unwrap_or_default().to_string(),
                            item["code"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn violations_are_collected_per_field_without_echoing_values() {
        let normalized = json!({
            "quantity": {"type": "integer", "value": 11},
            "price": {"type": "number", "decimal": "0.1"},
            "code": "abc-123456789",
            "day": "2025-12-31"
        });

        let error = check_field_rules(&schema(), &normalized, None).expect_err("rules must refuse");
        let message = error.to_string();
        let codes = codes(error);

        assert!(codes.contains(&("quantity".to_string(), "above_max".to_string())));
        assert!(codes.contains(&("price".to_string(), "below_min".to_string())));
        assert!(codes.contains(&("code".to_string(), "too_long".to_string())));
        assert!(codes.contains(&("code".to_string(), "pattern_mismatch".to_string())));
        assert!(codes.contains(&("day".to_string(), "below_min".to_string())));
        assert!(!message.contains("abc-123456789"));
        assert!(!message.contains("[A-Z]"));
    }

    #[test]
    fn valid_values_and_unchanged_legacy_values_pass() {
        let normalized = json!({
            "quantity": {"type": "integer", "value": 10},
            "code": "ABC-12",
            "day": "2026-06-01",
            "price": {"type": "number", "decimal": "0.1"}
        });
        let existing = json!({"price": {"type": "number", "decimal": "0.1"}});

        assert!(check_field_rules(&schema(), &normalized, Some(&existing)).is_ok());
    }

    #[test]
    fn defaults_fill_empty_fields_on_create_and_decimal_defaults_become_strings() {
        let values = apply_default_values(&schema(), json!({"quantity": 2, "state": null})).unwrap();

        assert_eq!(values["state"], "new");
        assert_eq!(values["price"], "1.5");
        assert_eq!(values["quantity"], 2);
    }

    #[test]
    fn read_only_fields_refuse_changes_but_accept_echoes_and_defaults() {
        let schema = schema();
        let existing = json!({"state": "approved"});

        assert!(ensure_read_only_fields_untouched(&schema, &json!({"state": "approved"}), Some(&existing)).is_ok());
        assert!(ensure_read_only_fields_untouched(&schema, &json!({"quantity": 3}), Some(&existing)).is_ok());
        assert!(ensure_read_only_fields_untouched(&schema, &json!({"state": "new"}), None).is_ok());
        assert!(ensure_read_only_fields_untouched(&schema, &json!({"state": null}), None).is_ok());

        for (submitted, stored) in [
            (json!({"state": "rejected"}), Some(&existing)),
            (json!({"state": null}), Some(&existing)),
            (json!({"state": "approved"}), None),
        ] {
            let error = ensure_read_only_fields_untouched(&schema, &submitted, stored).expect_err("read-only");
            assert_eq!(codes(error), vec![("state".to_string(), "read_only".to_string())]);
        }
    }

    #[test]
    fn schema_rule_declarations_are_checked() {
        assert!(validate_schema_field_rules(&schema()).is_ok());
        for field in [
            json!({"field_id": "fld_bad_0001", "key": "a", "type": "text", "validation": {"pattern": "("}}),
            json!({"field_id": "fld_bad_0001", "key": "a", "type": "text", "validation": {"max_length": 0}}),
            json!({"field_id": "fld_bad_0001", "key": "a", "type": "integer", "validation": {"min": 5, "max": 1}}),
            json!({"field_id": "fld_bad_0001", "key": "a", "type": "integer", "default_value": "x"}),
            json!({"field_id": "fld_bad_0001", "key": "a", "type": "integer",
                   "validation": {"max": 3}, "default_value": 4}),
        ] {
            assert!(validate_schema_field_rules(&json!({"fields": [field]})).is_err());
        }
    }
}
