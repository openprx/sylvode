//! Choice types: single and multi select over the field's options, and boolean.
//!
//! A disabled or archived option is refused unless the stored value already holds it, so editing
//! a record never forces a change of a retired choice.

use serde_json::{Map, Value};
use sv_rules::{Operand, RuleKind};

use super::{CelType, DefError, FieldType, StorageKind, TypeError, TypeSettings, string_list};
use crate::forms::schema::FormField;

fn parse_options(raw: &Map<String, Value>) -> TypeSettings {
    TypeSettings::Options {
        options: string_list(raw, "options"),
        disabled: string_list(raw, "option_disabled"),
        archived: string_list(raw, "option_archived"),
    }
}

fn check_option(field: &FormField, value: &str, prior: Option<&Value>) -> Result<(), TypeError> {
    if !field.options.is_empty() && !field.options.iter().any(|option| option == value) {
        return Err(TypeError::OptionUnsupported);
    }
    let disabled = field.option_disabled.iter().any(|option| option == value);
    let archived = field.option_archived.iter().any(|option| option == value);
    if !disabled && !archived {
        return Ok(());
    }
    let existed = match prior {
        Some(Value::String(stored)) => stored == value,
        Some(Value::Array(items)) => items.iter().any(|item| item.as_str() == Some(value)),
        _ => false,
    };
    if existed {
        Ok(())
    } else if archived {
        Err(TypeError::OptionArchived)
    } else {
        Err(TypeError::OptionDisabled)
    }
}

/// `single_select`.
pub struct SingleSelect;

impl FieldType for SingleSelect {
    fn name(&self) -> &'static str {
        "single_select"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Text
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::String)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        &[RuleKind::Enum, RuleKind::Cel, RuleKind::Plugin]
    }
    fn parse_settings(&self, raw: &Map<String, Value>) -> Result<TypeSettings, DefError> {
        Ok(parse_options(raw))
    }
    fn normalize(&self, field: &FormField, raw: &Value, prior: Option<&Value>) -> Result<Value, TypeError> {
        let value = raw.as_str().ok_or(TypeError::Mismatch)?;
        check_option(field, value, prior)?;
        Ok(Value::String(value.to_string()))
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        value.as_str().map(|text| Operand::Str(text.to_string()))
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        value.as_str().map(|text| Value::String(text.to_string()))
    }
}

/// `multi_select`: an array of option strings; length rules count items.
pub struct MultiSelect;

impl FieldType for MultiSelect {
    fn name(&self) -> &'static str {
        "multi_select"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Json
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::ListString)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        &[
            RuleKind::MinLength,
            RuleKind::MaxLength,
            RuleKind::Enum,
            RuleKind::Cel,
            RuleKind::Plugin,
        ]
    }
    fn parse_settings(&self, raw: &Map<String, Value>) -> Result<TypeSettings, DefError> {
        Ok(parse_options(raw))
    }
    fn normalize(&self, field: &FormField, raw: &Value, prior: Option<&Value>) -> Result<Value, TypeError> {
        let items = raw.as_array().ok_or(TypeError::Mismatch)?;
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            let value = item.as_str().ok_or(TypeError::Mismatch)?;
            check_option(field, value, prior)?;
            values.push(Value::String(value.to_string()));
        }
        Ok(Value::Array(values))
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        value.as_array().map(|items| Operand::Count(items.len()))
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        value
            .as_array()
            .filter(|items| items.iter().all(Value::is_string))
            .map(|items| Value::Array(items.clone()))
    }
}

/// `boolean`.
pub struct Boolean;

impl FieldType for Boolean {
    fn name(&self) -> &'static str {
        "boolean"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Bool
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::Bool)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        &[RuleKind::Cel, RuleKind::Plugin]
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        raw.as_bool().map(Value::Bool).ok_or(TypeError::Mismatch)
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        value.as_bool().map(Value::Bool)
    }
}
