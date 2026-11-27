//! String types: the text-like types (text, textarea, phone, email, address, scan), rich text,
//! and the system or media strings (`signature`, `autonumber`, `ai_summary`).

use serde_json::{Map, Value};
use sv_rules::{Operand, RuleKind};

use super::{
    CelType, DefError, FieldType, MaskRule, Outlet, OutletRule, StorageKind, TEXT_NORMALIZE_OPS, TEXT_RULES,
    TEXT_TRANSFORM_OPS, TypeError, TypeSettings, opaque_outlet,
};
use crate::forms::schema::{FormField, SignatureFieldConfig, normalize_key};

fn string_value(raw: &Value) -> Result<Value, TypeError> {
    raw.as_str()
        .map(|value| Value::String(value.to_string()))
        .ok_or(TypeError::Mismatch)
}

fn text_operand(value: &Value) -> Option<Operand> {
    value.as_str().map(|text| Operand::Str(text.to_string()))
}

fn text_cel(value: &Value) -> Option<Value> {
    value.as_str().map(|text| Value::String(text.to_string()))
}

/// A text-like type: a plain string with the full text rule set.
macro_rules! text_like {
    ($ty:ident, $name:literal, $transform:expr) => {
        pub struct $ty;

        impl FieldType for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn storage(&self) -> StorageKind {
                StorageKind::Text
            }
            fn cel_type(&self) -> Option<CelType> {
                Some(CelType::String)
            }
            fn allowed_rules(&self) -> &'static [RuleKind] {
                TEXT_RULES
            }
            fn normalize_ops(&self) -> &'static [&'static str] {
                TEXT_NORMALIZE_OPS
            }
            fn transform_ops(&self) -> &'static [&'static str] {
                $transform
            }
            fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
                string_value(raw)
            }
            fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
                text_operand(value)
            }
            fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
                text_cel(value)
            }
            fn maskable(&self) -> bool {
                true
            }
        }
    };
}

text_like!(Text, "text", &["slug", "truncate"]);
text_like!(Textarea, "textarea", TEXT_TRANSFORM_OPS);
text_like!(Phone, "phone", TEXT_TRANSFORM_OPS);
text_like!(Address, "address", TEXT_TRANSFORM_OPS);
text_like!(Scan, "scan", TEXT_TRANSFORM_OPS);

/// `email`: a trimmed string with one `@` that is neither first nor last.
pub struct Email;

impl FieldType for Email {
    fn name(&self) -> &'static str {
        "email"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Text
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::String)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        TEXT_RULES
    }
    fn normalize_ops(&self) -> &'static [&'static str] {
        TEXT_NORMALIZE_OPS
    }
    fn transform_ops(&self) -> &'static [&'static str] {
        TEXT_TRANSFORM_OPS
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        let value = raw.as_str().ok_or(TypeError::Mismatch)?.trim();
        if !value.contains('@') || value.starts_with('@') || value.ends_with('@') {
            return Err(TypeError::Mismatch);
        }
        Ok(Value::String(value.to_string()))
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        text_operand(value)
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        text_cel(value)
    }
    fn maskable(&self) -> bool {
        true
    }
}

/// `rich_text`: a string; lengths count the plain text the client sends.
pub struct RichText;

impl FieldType for RichText {
    fn name(&self) -> &'static str {
        "rich_text"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Text
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::String)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        &[
            RuleKind::MinLength,
            RuleKind::MaxLength,
            RuleKind::Cel,
            RuleKind::Plugin,
        ]
    }
    fn normalize_ops(&self) -> &'static [&'static str] {
        &["nfc"]
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        string_value(raw)
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        text_operand(value)
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        text_cel(value)
    }
}

/// A server-written or media string without rules (`autonumber`, `ai_summary`).
macro_rules! system_string {
    ($ty:ident, $name:literal) => {
        pub struct $ty;

        impl FieldType for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn storage(&self) -> StorageKind {
                StorageKind::Text
            }
            fn cel_type(&self) -> Option<CelType> {
                None
            }
            fn allowed_rules(&self) -> &'static [RuleKind] {
                &[]
            }
            fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
                string_value(raw)
            }
        }
    };
}

system_string!(Autonumber, "autonumber");
system_string!(AiSummary, "ai_summary");

/// `signature`: the signature media string; its settings carry retention and consent.
pub struct Signature;

impl FieldType for Signature {
    fn name(&self) -> &'static str {
        "signature"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Blob
    }
    fn cel_type(&self) -> Option<CelType> {
        None
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        &[]
    }
    fn parse_settings(&self, raw: &Map<String, Value>) -> Result<TypeSettings, DefError> {
        parse_signature_config(raw.get("signature")).map(TypeSettings::Signature)
    }
    fn validate_definition(&self, field: &FormField, all: &[FormField]) -> Result<(), DefError> {
        let Some(reason_field) = field
            .signature
            .as_ref()
            .and_then(|signature| signature.reason_field.as_deref())
        else {
            return Ok(());
        };
        if reason_field == field.key {
            return Err(DefError::invalid(
                "/signature/reason_field",
                "signature.reason_field cannot point to the signature field itself",
            ));
        }
        if !all.iter().any(|other| other.key == reason_field) {
            return Err(DefError::invalid(
                "/signature/reason_field",
                format!("signature.reason_field must reference an existing field key: {reason_field}"),
            ));
        }
        Ok(())
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        string_value(raw)
    }
    fn outlet(&self, outlet: Outlet) -> OutletRule {
        opaque_outlet(outlet)
    }
    fn mask(&self, _field: &FormField, _value: &Value, _rule: &MaskRule) -> Option<Value> {
        None
    }
}

fn parse_signature_config(value: Option<&Value>) -> Result<SignatureFieldConfig, DefError> {
    let object = value.and_then(Value::as_object);
    let raw_retention = object.and_then(|item| item.get("retention_days"));
    let retention_days = raw_retention.and_then(signature_retention_days_value);
    if raw_retention.is_some_and(|value| !value.is_null()) && retention_days.is_none() {
        return Err(DefError::invalid(
            "/signature/retention_days",
            "signature.retention_days must be between 1 and 3650",
        ));
    }
    let reason_field = object
        .and_then(|item| item.get("reason_field"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_key)
        .transpose()
        .map_err(|detail| DefError::invalid("/signature/reason_field", detail))?;
    let reason_required = object
        .and_then(|item| item.get("reason_required"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| reason_field.is_some());
    if reason_required && reason_field.is_none() {
        return Err(DefError::invalid(
            "/signature/reason_required",
            "signature.reason_field is required when signature.reason_required is true",
        ));
    }
    let consent_statement = object
        .and_then(|item| item.get("consent_statement"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if consent_statement
        .as_ref()
        .is_some_and(|statement| statement.chars().count() > 1000)
    {
        return Err(DefError::invalid(
            "/signature/consent_statement",
            "signature.consent_statement must be 1000 characters or less",
        ));
    }
    Ok(SignatureFieldConfig {
        retention_days,
        reason_field,
        reason_required,
        consent_statement,
    })
}

fn signature_retention_days_value(value: &Value) -> Option<i64> {
    let days = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|item| item.trim().parse::<i64>().ok()))?;
    (1..=3650).contains(&days).then_some(days)
}
