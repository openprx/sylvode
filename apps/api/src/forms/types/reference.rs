//! Reference and structured types: members, locations, files, record relations, child tables
//! and formula outputs. None of them is exposed to CEL or carries built-in rules in the MVP.

use serde_json::{Map, Value, json};
use sv_rules::RuleKind;

use super::{CelType, FieldType, MaskRule, Outlet, OutletRule, StorageKind, TypeError, opaque_outlet};
use crate::forms::schema::FormField;

/// `member`: `{"user_id", "name"?, "email"?}`; a bare string is a user id. CEL sees the user id.
pub struct Member;

impl FieldType for Member {
    fn name(&self) -> &'static str {
        "member"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Ref
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::String)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        &[RuleKind::Cel, RuleKind::Plugin]
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        if let Some(user_id) = raw.as_str().map(str::trim).filter(|value| !value.is_empty()) {
            return Ok(json!({ "user_id": user_id }));
        }
        let object = raw.as_object().ok_or(TypeError::Mismatch)?;
        let user_id = object
            .get("user_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(TypeError::Mismatch)?;
        let mut value = Map::new();
        value.insert("user_id".to_string(), json!(user_id));
        for key in ["name", "email"] {
            if let Some(item) = object.get(key).and_then(Value::as_str).map(str::trim)
                && !item.is_empty()
            {
                value.insert(key.to_string(), json!(item));
            }
        }
        Ok(Value::Object(value))
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        value
            .get("user_id")
            .and_then(Value::as_str)
            .map(|id| Value::String(id.to_string()))
    }
    fn outlet(&self, outlet: Outlet) -> OutletRule {
        // Staff user ids are never shown to or probed from a public site.
        if matches!(outlet, Outlet::SiteRead | Outlet::SiteSubmit) {
            OutletRule::Reject
        } else {
            OutletRule::Value
        }
    }
}

/// `location`: a string, or `{"lat", "lng", "label"?}` within range.
pub struct Location;

impl FieldType for Location {
    fn name(&self) -> &'static str {
        "location"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Json
    }
    fn cel_type(&self) -> Option<CelType> {
        None
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        &[]
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        if let Some(value) = raw.as_str() {
            return Ok(json!(value));
        }
        let object = raw.as_object().ok_or(TypeError::Mismatch)?;
        let lat = object.get("lat").and_then(Value::as_f64).ok_or(TypeError::Mismatch)?;
        let lng = object.get("lng").and_then(Value::as_f64).ok_or(TypeError::Mismatch)?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lng) {
            return Err(TypeError::Mismatch);
        }
        let mut value = Map::new();
        value.insert("lat".to_string(), json!(lat));
        value.insert("lng".to_string(), json!(lng));
        if let Some(label) = object.get("label").and_then(Value::as_str).map(str::trim)
            && !label.is_empty()
        {
            value.insert("label".to_string(), json!(label));
        }
        Ok(Value::Object(value))
    }
    fn outlet(&self, outlet: Outlet) -> OutletRule {
        opaque_outlet(outlet)
    }
    fn mask(&self, _field: &FormField, _value: &Value, _rule: &MaskRule) -> Option<Value> {
        None
    }
}

/// A type whose value is kept as sent (its own services check it: attachment claims, relation
/// targets, child rows, formula evaluation).
macro_rules! opaque_type {
    ($ty:ident, $name:literal, $storage:expr, $outlet:expr) => {
        pub struct $ty;

        impl FieldType for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn storage(&self) -> StorageKind {
                $storage
            }
            fn cel_type(&self) -> Option<CelType> {
                None
            }
            fn allowed_rules(&self) -> &'static [RuleKind] {
                &[]
            }
            fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
                Ok(raw.clone())
            }
            fn outlet(&self, outlet: Outlet) -> OutletRule {
                let rule: fn(Outlet) -> OutletRule = $outlet;
                rule(outlet)
            }
            fn server_written(&self) -> bool {
                $name == "formula"
            }
        }
    };
}

/// A relation renders its target's label in titles (FIX-L0.1); every other opaque outlet is refused.
fn relation_outlet(outlet: Outlet) -> OutletRule {
    if outlet == Outlet::TitleTemplate {
        OutletRule::Value
    } else {
        opaque_outlet(outlet)
    }
}

const fn derived_outlet(_outlet: Outlet) -> OutletRule {
    OutletRule::Value
}

opaque_type!(Attachment, "attachment", StorageKind::Blob, opaque_outlet);
opaque_type!(Image, "image", StorageKind::Blob, opaque_outlet);
opaque_type!(Relation, "relation", StorageKind::Ref, relation_outlet);
opaque_type!(ChildTable, "child_table", StorageKind::Ref, opaque_outlet);
opaque_type!(Formula, "formula", StorageKind::Json, derived_outlet);
