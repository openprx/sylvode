use base64::{Engine as _, engine::general_purpose::STANDARD};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
pub struct Manifest {
    pub schema_version: String,
    pub key: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub host_protocol: Protocol,
    pub wasm_abi: String,
    pub wasm: Asset,
    pub ui: Ui,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    pub min: u32,
    pub max: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub path: String,
    pub size: usize,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ui {
    pub entry: Asset,
    pub slot: String,
    pub bridge_version: u32,
    pub form_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributions: Option<Vec<Contribution>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub id: String,
    pub slot: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub id: String,
    pub enabled: bool,
    pub position: u8,
}

#[derive(Serialize)]
pub struct EnabledContribution {
    pub id: String,
    pub slot: String,
    pub label: String,
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

impl Manifest {
    pub fn validate(&self) -> Result<(), ApiError> {
        let required_host_protocol = if self.ui.contributions.is_some() { 2 } else { 1 };
        if self.schema_version != "openpr.plugin.v2"
            || self.wasm_abi != "openpr.plugin.v1"
            || self.host_protocol.min != required_host_protocol
            || self.host_protocol.max != required_host_protocol
            || self.ui.bridge_version != 1
            || self.ui.slot != "form.record.detail"
        {
            return Err(bad("unsupported plugin package protocol, ABI, or slot"));
        }
        validate_key(&self.key)?;
        validate_key(&self.ui.form_key)?;
        if let Some(contributions) = &self.ui.contributions {
            if !matches!(contributions.as_slice(), [first, second] if first.id != second.id) {
                return Err(bad("layout packages require two distinct contribution IDs"));
            }
            for slot in ["form.record.detail", "form.record.actions"] {
                let contribution = contributions
                    .iter()
                    .find(|contribution| contribution.slot == slot)
                    .ok_or_else(|| bad("layout packages must declare one detail and one action contribution"))?;
                validate_key(&contribution.id)?;
                if !valid_label(&contribution.label) {
                    return Err(bad("invalid plugin contribution label"));
                }
            }
        }

        if self.version.len() > 64 || semver::Version::parse(&self.version).is_err() {
            return Err(bad("plugin version must be a SemVer release identifier"));
        }
        for value in [&self.name, &self.publisher] {
            if !valid_label(value) {
                return Err(bad("plugin name and publisher must be nonempty display labels"));
            }
        }
        if self.capabilities.len() != 2 {
            return Err(bad("the order amount profile requires preview and apply capabilities"));
        }
        for (name, permission) in [(PREVIEW, "record.read"), (APPLY, "record.update")] {
            let capability = self
                .capabilities
                .iter()
                .find(|item| item.name == name)
                .ok_or_else(|| bad("the order amount profile requires preview and apply capabilities"))?;
            if capability.permission != permission
                || capability.input_schema != json!({"type":"object","additionalProperties":false})
                || capability.output_schema
                    != json!({
                        "type":"object","additionalProperties":false,"required":["amount_cents"],
                        "properties":{"amount_cents":{"type":"integer","minimum":0,"maximum":MAX_AMOUNT_CENTS}}
                    })
            {
                return Err(bad(
                    "capability schema or permission is incompatible with the order amount profile",
                ));
            }
        }
        Ok(())
    }
}

fn valid_label(label: &str) -> bool {
    !label.is_empty() && label.len() <= 120 && label.trim() == label && !label.chars().any(char::is_control)
}

pub fn normalize_placements(contributions: &[Contribution], placements: &mut [Placement]) -> Result<(), ApiError> {
    placements.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    if placements.len() != contributions.len()
        || !contributions.iter().all(|contribution| {
            placements
                .iter()
                .any(|placement| placement.id == contribution.id && placement.position <= 99)
        })
    {
        return Err(bad(
            "layout must configure each declared contribution exactly once with position between 0 and 99",
        ));
    }
    Ok(())
}

pub fn default_placements(contributions: &[Contribution]) -> Vec<Placement> {
    let mut placements: Vec<_> = contributions
        .iter()
        .map(|contribution| Placement {
            id: contribution.id.clone(),
            enabled: true,
            position: 50,
        })
        .collect();
    placements.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    placements
}

pub fn enabled_contributions(contributions: Vec<Contribution>, placements: &[Placement]) -> Vec<EnabledContribution> {
    let mut enabled: Vec<_> = contributions
        .into_iter()
        .filter_map(|contribution| {
            let placement = placements
                .iter()
                .find(|placement| placement.id == contribution.id && placement.enabled)?;
            Some(EnabledContribution {
                id: contribution.id,
                slot: contribution.slot,
                label: contribution.label,
                position: placement.position,
            })
        })
        .collect();
    enabled.sort_unstable_by(|left, right| left.position.cmp(&right.position).then(left.id.cmp(&right.id)));
    enabled
}

pub fn validate_key(key: &str) -> Result<(), ApiError> {
    if key.len() > 64 || normalize_key(key).as_deref() != Ok(key) {
        return Err(bad(
            "plugin and form keys must use lowercase letters, digits, and underscores",
        ));
    }
    Ok(())
}

pub fn decode_asset(asset: &Asset, path: &str, encoded: &str, limit: usize) -> Result<Vec<u8>, ApiError> {
    if asset.path != path || asset.size == 0 || asset.size > limit || encoded.len() > limit.div_ceil(3) * 4 {
        return Err(bad("invalid plugin asset path or size"));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| bad("invalid plugin asset base64"))?;
    if bytes.len() != asset.size || sha256(&bytes) != asset.sha256 {
        return Err(bad("plugin asset size or SHA-256 does not match the manifest"));
    }
    Ok(bytes)
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
