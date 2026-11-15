//! Checked access to JSON DTO fields. Reads retain `serde_json`'s missing-field null semantics;
//! writes return a typed internal error for scalar/array shapes instead of panicking.
use crate::error::ApiError;
use serde_json::{Map, Value};
pub trait FlowJson {
    fn field(&self, key: &str) -> &Value;
    fn field_mut(&mut self, key: &str) -> Result<&mut Value, ApiError>;
}
impl FlowJson for Value {
    fn field(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&Self::Null)
    }
    fn field_mut(&mut self, key: &str) -> Result<&mut Value, ApiError> {
        if self.is_null() {
            *self = Self::Object(Map::new());
        }
        Ok(self
            .as_object_mut()
            .ok_or_else(ApiError::internal)?
            .entry(key)
            .or_insert(Self::Null))
    }
}
impl FlowJson for Map<String, Value> {
    fn field(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&Value::Null)
    }
    fn field_mut(&mut self, key: &str) -> Result<&mut Value, ApiError> {
        Ok(self.entry(key).or_insert(Value::Null))
    }
}
