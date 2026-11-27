//! Numeric types. Integers are stored as `{"type": <name>, "value": <i64>}`; decimals as
//! `{"type": "decimal" | "amount", "decimal": "<canonical>"...}`. Callers never send JSON numbers
//! for decimals, so no float rounding reaches a stored value.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use sv_rules::{Bound, Operand, RuleKind};

use super::{CelType, DefError, FieldType, ORDERED_RULES, StorageKind, TypeError, TypeSettings};
use crate::forms::{
    decimal::normalize_amount_decimal,
    schema::{AmountFieldConfig, FormField},
};

const DECIMAL_TRANSFORM_OPS: &[&str] = &["round"];

fn integer_input(raw: &Value) -> Result<i64, TypeError> {
    if let Some(value) = raw.get("value").and_then(Value::as_i64) {
        return Ok(value);
    }
    if let Some(value) = raw.as_i64() {
        return Ok(value);
    }
    raw.as_str()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .ok_or(TypeError::Mismatch)
}

fn stored_integer(value: &Value) -> Option<i64> {
    value.get("value").and_then(Value::as_i64)
}

fn stored_decimal(value: &Value) -> Option<Decimal> {
    value
        .get("decimal")
        .and_then(Value::as_str)
        .and_then(|decimal| Decimal::from_str(decimal).ok())
}

/// An integer type with an optional fixed range (`rating` 1..5, `progress` 0..100).
macro_rules! integer_type {
    ($ty:ident, $name:literal, $range:expr) => {
        pub struct $ty;

        impl FieldType for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn storage(&self) -> StorageKind {
                StorageKind::Number
            }
            fn cel_type(&self) -> Option<CelType> {
                Some(CelType::Int)
            }
            fn allowed_rules(&self) -> &'static [RuleKind] {
                ORDERED_RULES
            }
            fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
                let value = integer_input(raw)?;
                let range: Option<(i64, i64)> = $range;
                if range.is_some_and(|(min, max)| value < min || value > max) {
                    return Err(TypeError::Mismatch);
                }
                Ok(json!({ "type": $name, "value": value }))
            }
            fn accepts_bound(&self, bound: &Bound) -> bool {
                matches!(bound, Bound::Int(_))
            }
            fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
                stored_integer(value).map(Operand::Int)
            }
            fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
                stored_integer(value).map(Value::from)
            }
        }
    };
}

integer_type!(Integer, "integer", None);
integer_type!(Rating, "rating", Some((1, 5)));
integer_type!(Progress, "progress", Some((0, 100)));

fn decimal_cel(value: &Value) -> Option<Value> {
    stored_decimal(value).map(|decimal| Value::String(decimal.normalize().to_string()))
}

/// `number`: an exact decimal.
pub struct Number;

impl FieldType for Number {
    fn name(&self) -> &'static str {
        "number"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Number
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::String)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        ORDERED_RULES
    }
    fn transform_ops(&self) -> &'static [&'static str] {
        DECIMAL_TRANSFORM_OPS
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        if raw.is_number() {
            return Err(TypeError::Mismatch);
        }
        let decimal = raw
            .as_str()
            .or_else(|| raw.get("decimal").and_then(Value::as_str))
            .ok_or(TypeError::Mismatch)?;
        let parsed = Decimal::from_str(decimal.trim()).map_err(|_| TypeError::Mismatch)?;
        Ok(json!({ "type": "decimal", "decimal": parsed.to_string() }))
    }
    fn accepts_bound(&self, bound: &Bound) -> bool {
        matches!(bound, Bound::Int(_) | Bound::Decimal(_))
    }
    fn number_default_as_text(&self) -> bool {
        true
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        stored_decimal(value).map(Operand::Decimal)
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        decimal_cel(value)
    }
}

/// `amount`: a decimal in the field's currency, rounded to its scale.
pub struct Amount;

impl FieldType for Amount {
    fn name(&self) -> &'static str {
        "amount"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Number
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::String)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        ORDERED_RULES
    }
    fn transform_ops(&self) -> &'static [&'static str] {
        DECIMAL_TRANSFORM_OPS
    }
    fn parse_settings(&self, raw: &Map<String, Value>) -> Result<TypeSettings, DefError> {
        parse_amount_config(raw.get("amount")).map(TypeSettings::Amount)
    }
    fn normalize(&self, field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        if raw.is_number() {
            return Err(TypeError::Mismatch);
        }
        let config = field.amount.as_ref().ok_or(TypeError::Mismatch)?;
        let (decimal, currency) = if let Some(value) = raw.as_str() {
            (value, config.currency.as_str())
        } else if let Some(object) = raw.as_object() {
            let decimal = object
                .get("decimal")
                .and_then(Value::as_str)
                .ok_or(TypeError::Mismatch)?;
            let currency = object
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or(&config.currency);
            (decimal, currency)
        } else {
            return Err(TypeError::Mismatch);
        };
        if currency.to_ascii_uppercase() != config.currency {
            return Err(TypeError::Mismatch);
        }
        let normalized = normalize_amount_decimal(decimal, config).map_err(|_| TypeError::Mismatch)?;
        Ok(json!({
            "type": "amount",
            "decimal": normalized,
            "currency": config.currency,
            "scale": config.scale
        }))
    }
    fn accepts_bound(&self, bound: &Bound) -> bool {
        matches!(bound, Bound::Int(_) | Bound::Decimal(_))
    }
    fn number_default_as_text(&self) -> bool {
        true
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        stored_decimal(value).map(Operand::Decimal)
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        decimal_cel(value)
    }
}

fn parse_amount_config(value: Option<&Value>) -> Result<AmountFieldConfig, DefError> {
    let object = value.and_then(Value::as_object);
    let currency = object
        .and_then(|item| item.get("currency"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("CNY")
        .to_ascii_uppercase();
    if !currency.chars().all(|c| c.is_ascii_uppercase()) || currency.len() != 3 {
        return Err(DefError::invalid(
            "/amount/currency",
            "amount.currency must be an ISO-like 3 letter code",
        ));
    }
    let scale = object
        .and_then(|item| item.get("scale"))
        .and_then(Value::as_u64)
        .unwrap_or(2);
    let scale = u32::try_from(scale)
        .ok()
        .filter(|scale| *scale <= 8)
        .ok_or_else(|| DefError::invalid("/amount/scale", "amount.scale must be between 0 and 8"))?;
    let rounding = object
        .and_then(|item| item.get("rounding"))
        .and_then(Value::as_str)
        .unwrap_or("half_up")
        .to_string();
    if !matches!(rounding.as_str(), "half_up" | "half_even" | "down" | "up") {
        return Err(DefError::invalid("/amount/rounding", "amount.rounding is unsupported"));
    }
    Ok(AmountFieldConfig {
        currency,
        scale,
        allow_negative: object
            .and_then(|item| item.get("allow_negative"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        rounding,
    })
}
