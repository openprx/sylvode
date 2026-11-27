//! Form schema fields as the server reads them. Each field's type-specific settings come from its
//! descriptor ([`super::types`]); the v2 rule members are read by [`super::rules`].

// Public and framework-facing signatures take owned JSON like their callers hold it.
#![allow(clippy::needless_pass_by_value)]

use serde_json::Value;
use uuid::Uuid;

use super::{
    rules::{SchemaIssues, SchemaScope, check_schema},
    types::{DefError, TypeSettings, field_type},
};
use crate::error::ApiError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub field_id: String,
    pub key: String,
    pub label: String,
    pub field_type: String,
    pub required: bool,
    pub options: Vec<String>,
    pub option_disabled: Vec<String>,
    pub option_archived: Vec<String>,
    pub conditional: Option<Value>,
    pub amount: Option<AmountFieldConfig>,
    pub signature: Option<SignatureFieldConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmountFieldConfig {
    pub currency: String,
    pub scale: u32,
    pub allow_negative: bool,
    pub rounding: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureFieldConfig {
    pub retention_days: Option<i64>,
    pub reason_field: Option<String>,
    pub reason_required: bool,
    pub consent_statement: Option<String>,
}

pub fn parse_fields(schema: &Value) -> Result<Vec<FormField>, String> {
    let Some(fields) = schema.get("fields").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    fields
        .iter()
        .map(|field| parse_field_at(field).map_err(|error| error.detail))
        .collect::<Result<Vec<_>, _>>()
}

/// Checks a schema for a save and returns every problem (contract C2 §8, §9).
pub fn validate_schema_report(schema: &Value, scope: Option<SchemaScope>) -> SchemaIssues {
    check_schema(schema, scope)
}

/// Checks a schema for a save; the refusal is `schema_invalid` with the issue list.
pub fn ensure_valid_schema(schema: &Value, scope: Option<SchemaScope>) -> Result<(), ApiError> {
    let issues = validate_schema_report(schema, scope);
    if issues.is_valid() {
        Ok(())
    } else {
        Err(issues.into_api_error())
    }
}

/// Checks a schema for a save where no project scope is known (so plugin rules are refused), as a
/// plain message.
pub fn validate_schema(schema: &Value) -> Result<(), String> {
    let issues = validate_schema_report(schema, None);
    if issues.is_valid() {
        Ok(())
    } else {
        Err(issues.message())
    }
}

pub fn ensure_schema_field_ids(schema: Value) -> Result<Value, String> {
    let mut object = schema
        .as_object()
        .cloned()
        .ok_or_else(|| "schema must be a JSON object".to_string())?;
    let fields = object.remove("fields").unwrap_or_else(|| Value::Array(Vec::new()));
    let fields = fields
        .as_array()
        .ok_or_else(|| "schema.fields must be an array".to_string())?;
    let mut next_fields = Vec::with_capacity(fields.len());

    for field in fields {
        let mut field_object = field
            .as_object()
            .cloned()
            .ok_or_else(|| "field definition must be an object".to_string())?;
        let field_id = field_object
            .get("field_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map_or_else(new_field_id, str::to_string);
        validate_field_id(&field_id).map_err(|error| error.detail)?;
        field_object.insert("field_id".to_string(), Value::String(field_id));
        next_fields.push(Value::Object(field_object));
    }

    object.insert("fields".to_string(), Value::Array(next_fields));
    Ok(Value::Object(object))
}

/// Parses one raw field object. Error paths are relative to the field object.
pub fn parse_field_at(value: &Value) -> Result<FormField, DefError> {
    let object = value
        .as_object()
        .ok_or_else(|| DefError::invalid("", "field definition must be an object"))?;
    let field_id = object
        .get("field_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DefError::invalid("/field_id", "field_id is required"))?
        .to_string();
    validate_field_id(&field_id)?;
    let key = object
        .get("key")
        .and_then(Value::as_str)
        .map(normalize_key)
        .transpose()
        .map_err(|detail| DefError::invalid("/key", detail))?
        .ok_or_else(|| DefError::invalid("/key", "field key is required"))?;
    let label = object
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&key)
        .to_string();
    let field_type_name = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("text")
        .to_string();
    let descriptor = field_type(&field_type_name).ok_or_else(|| {
        DefError::new(
            "/type",
            "field_type_unknown",
            format!("unsupported field type: {field_type_name}"),
        )
    })?;
    let mut field = FormField {
        field_id,
        key,
        label,
        field_type: field_type_name,
        required: object.get("required").and_then(Value::as_bool).unwrap_or(false),
        options: Vec::new(),
        option_disabled: Vec::new(),
        option_archived: Vec::new(),
        conditional: object.get("conditional").cloned(),
        amount: None,
        signature: None,
    };
    match descriptor.parse_settings(object)? {
        TypeSettings::None => {}
        TypeSettings::Options {
            options,
            disabled,
            archived,
        } => {
            field.options = options;
            field.option_disabled = disabled;
            field.option_archived = archived;
        }
        TypeSettings::Amount(amount) => field.amount = Some(amount),
        TypeSettings::Signature(signature) => field.signature = Some(signature),
    }
    Ok(field)
}

fn new_field_id() -> String {
    format!("fld_{}", Uuid::new_v4().simple())
}

fn validate_field_id(field_id: &str) -> Result<(), DefError> {
    if field_id.len() < 8 || field_id.len() > 80 {
        return Err(DefError::invalid(
            "/field_id",
            "field_id length must be between 8 and 80",
        ));
    }
    if !field_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(DefError::invalid(
            "/field_id",
            "field_id must contain lowercase letters, digits, or underscores",
        ));
    }
    Ok(())
}

pub fn normalize_key(raw: &str) -> Result<String, String> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Err("key is required".to_string());
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err("key must contain lowercase letters, digits, or underscores".to_string());
    }
    let Some(first) = key.chars().next() else {
        return Err("key is required".to_string());
    };
    if !first.is_ascii_lowercase() {
        return Err("key must start with a lowercase letter".to_string());
    }
    Ok(key)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{ensure_schema_field_ids, normalize_key, parse_fields, validate_schema};
    use serde_json::json;

    #[test]
    fn validates_form_schema_fields() {
        let schema = json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {"field_id": "fld_counterparty", "key": "counterparty", "label": "Counterparty", "type": "text", "required": true},
                {"field_id": "fld_amount", "key": "amount", "label": "Amount", "type": "amount", "amount": {"currency": "CNY", "scale": 2}}
            ]
        });
        let fields = parse_fields(&schema).unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(
            fields
                .get(1)
                .and_then(|field| field.amount.as_ref())
                .map(|amount| amount.currency.as_str()),
            Some("CNY")
        );
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn validates_signature_retention_policy() {
        let schema = json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 90}}
            ]
        });
        let fields = parse_fields(&schema).unwrap();
        assert_eq!(
            fields
                .first()
                .and_then(|field| field.signature.as_ref())
                .and_then(|signature| signature.retention_days),
            Some(90)
        );
        assert!(validate_schema(&schema).is_ok());

        let invalid = json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 0}}
            ]
        });
        assert!(validate_schema(&invalid).is_err());
    }

    #[test]
    fn validates_signature_reason_and_consent_policy() {
        let schema = json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {"field_id": "fld_reason", "key": "signature_reason", "type": "text"},
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "type": "signature",
                    "signature": {
                        "reason_field": "signature_reason",
                        "reason_required": true,
                        "consent_statement": "I confirm this signature is intentional."
                    }
                }
            ]
        });
        let fields = parse_fields(&schema).unwrap();
        let signature = fields
            .iter()
            .find(|field| field.key == "guest_signature")
            .and_then(|field| field.signature.as_ref())
            .expect("signature config should parse");
        assert_eq!(signature.reason_field.as_deref(), Some("signature_reason"));
        assert!(signature.reason_required);
        assert_eq!(
            signature.consent_statement.as_deref(),
            Some("I confirm this signature is intentional.")
        );
        assert!(validate_schema(&schema).is_ok());

        let missing_reason_field = json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "type": "signature",
                    "signature": {"reason_field": "signature_reason"}
                }
            ]
        });
        assert!(validate_schema(&missing_reason_field).is_err());

        let impossible_required_reason = json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "type": "signature",
                    "signature": {"reason_required": true}
                }
            ]
        });
        assert!(validate_schema(&impossible_required_reason).is_err());
    }

    #[test]
    fn rejects_duplicate_keys() {
        let schema = json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {"field_id": "fld_amount_1", "key": "amount", "type": "amount"},
                {"field_id": "fld_amount_2", "key": "amount", "type": "text"}
            ]
        });
        assert!(validate_schema(&schema).is_err());
    }

    #[test]
    fn fills_missing_field_ids() {
        let schema = ensure_schema_field_ids(json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {"key": "amount", "type": "amount"}
            ]
        }))
        .unwrap();
        let field_id = schema["fields"][0]["field_id"].as_str().unwrap();
        assert!(field_id.starts_with("fld_"));
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn normalizes_safe_keys() {
        assert_eq!(normalize_key("Order_Line").unwrap(), "order_line");
        assert!(normalize_key("order-line").is_err());
        assert!(normalize_key("1order").is_err());
    }
}
