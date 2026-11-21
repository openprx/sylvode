use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    error::ApiError,
    forms::{calculation::value_to_decimal, schema::normalize_key},
};

pub const WEB_BYTES: usize = 64 * 1024;
pub const WASM_BYTES: usize = 256 * 1024;
pub const PREVIEW: &str = "amount.preview";
pub const APPLY: &str = "amount.apply";
pub const MAX_AMOUNT_CENTS: u64 = 1_000_000_000_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    pub min: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub id: String,
    pub enabled: bool,
    pub position: u8,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub name: String,
    pub permission: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

pub(crate) fn valid_label(label: &str) -> bool {
    !label.is_empty() && label.len() <= 120 && label.trim() == label && !label.chars().any(char::is_control)
}

pub fn validate_key(key: &str) -> Result<(), ApiError> {
    if key.len() > 64 || normalize_key(key).as_deref() != Ok(key) {
        return Err(bad(
            "plugin and form keys must use lowercase letters, digits, and underscores",
        ));
    }
    Ok(())
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.to_string())
}

pub fn parse_guest_amount(output: &Value) -> Result<u64, ApiError> {
    output
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("amount_cents"))
        .and_then(Value::as_u64)
        .filter(|amount| *amount <= MAX_AMOUNT_CENTS)
        .ok_or_else(|| bad("plugin result does not match the amount schema"))
}

pub fn read_order_inputs(values: &Value) -> Result<(u64, u64), ApiError> {
    let quantity = read_decimal(values, "quantity")?;
    if quantity.fract() != Decimal::ZERO || quantity < Decimal::ONE || quantity > Decimal::from(1_000_000) {
        return Err(bad("quantity must be an integer between 1 and 1000000"));
    }
    let cents = read_decimal(values, "unit_price")?
        .checked_mul(Decimal::from(100))
        .ok_or_else(|| bad("unit price lies outside decimal bounds"))?;
    if cents.fract() != Decimal::ZERO || cents < Decimal::ZERO || cents > Decimal::from(1_000_000_000) {
        return Err(bad(
            "unit_price must be between 0 and 10000000 with at most two decimal places",
        ));
    }
    let quantity =
        u64::try_from(quantity.mantissa() / 10_i128.pow(quantity.scale())).map_err(|_| ApiError::Internal)?;
    let cents = u64::try_from(cents.mantissa() / 10_i128.pow(cents.scale())).map_err(|_| ApiError::Internal)?;
    Ok((quantity, cents))
}

pub fn read_decimal(values: &Value, field: &str) -> Result<Decimal, ApiError> {
    let value = values.get(field).ok_or_else(|| bad("record requires decimal values"))?;
    value_to_decimal(value).map_err(|_| bad("record value must be a valid decimal"))
}

#[cfg(test)]
mod tests {
    use super::{parse_guest_amount, read_decimal, read_order_inputs};
    use crate::forms::{schema::ensure_schema_field_ids, values::normalize_record_values};
    use serde_json::json;

    #[test]
    fn reads_native_form_numeric_values() {
        let schema = ensure_schema_field_ids(json!({"fields":[
            {"key":"quantity","label":"Quantity","type":"number"},
            {"key":"unit_price","label":"Unit price","type":"number"},
            {"key":"amount","label":"Amount","type":"number"}
        ]}))
        .expect("assign Form field IDs");
        let submitted = json!({"quantity":"2","unit_price":"12.50","amount":"25.00"});
        let stored = normalize_record_values(&schema, submitted.clone()).expect("numeric Form normalizes");
        assert_eq!(
            stored.get("unit_price"),
            Some(&json!({"type":"decimal","decimal":"12.50"}))
        );
        assert_eq!(read_order_inputs(&stored).expect("stored decimals read"), (2, 1250));
        assert_eq!(
            read_decimal(&submitted, "amount").unwrap(),
            read_decimal(&stored, "amount").unwrap()
        );
    }

    #[test]
    fn rejects_unbounded_or_wrongly_shaped_calculations() {
        assert!(read_order_inputs(&json!({"quantity":"2","unit_price":"12.505"})).is_err());
        assert!(read_order_inputs(&json!({"quantity":"2.5","unit_price":"12.50"})).is_err());
        assert!(read_order_inputs(&json!({"quantity":"2","unit_price":"79228162514264337593543950335"})).is_err());
        assert!(parse_guest_amount(&json!({"amount_cents":25,"other":true})).is_err());
        assert!(parse_guest_amount(&json!({"amount_cents":-1})).is_err());
    }
}
