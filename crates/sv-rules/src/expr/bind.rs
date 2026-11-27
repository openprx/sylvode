//! JSON to CEL values, with the runtime limits applied to every bound value.

use std::collections::HashMap;
use std::sync::Arc;

use cel::objects::{Key, Map, Value};

use crate::error::RuleEvalError;
use crate::limits::{MAX_LIST_LEN, MAX_SAFE_INTEGER, MAX_STRING_BYTES, MAX_VALUE_DEPTH};

pub fn to_value(v: &serde_json::Value) -> Result<Value, RuleEvalError> {
    convert(v, 0)
}

#[allow(clippy::cast_precision_loss)]
const MAX_SAFE_FLOAT: f64 = MAX_SAFE_INTEGER as f64;

fn number(n: &serde_json::Number) -> Result<Value, RuleEvalError> {
    if let Some(i) = n.as_i64() {
        if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&i) {
            return Ok(Value::Int(i));
        }
        return Err(RuleEvalError::limit());
    }
    if n.is_u64() {
        return Err(RuleEvalError::limit());
    }
    let f = n.as_f64().ok_or_else(RuleEvalError::limit)?;
    if f.fract() == 0.0 && f.abs() <= MAX_SAFE_FLOAT {
        // Integral and within ±(2^53 - 1), so the cast is exact.
        #[allow(clippy::cast_possible_truncation)]
        return Ok(Value::Int(f as i64));
    }
    if f.fract() == 0.0 {
        return Err(RuleEvalError::limit());
    }
    Ok(Value::Float(f))
}

fn convert(v: &serde_json::Value, depth: usize) -> Result<Value, RuleEvalError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(RuleEvalError::limit());
    }
    Ok(match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => number(n)?,
        serde_json::Value::String(s) => {
            if s.len() > MAX_STRING_BYTES {
                return Err(RuleEvalError::limit());
            }
            Value::String(Arc::new(s.clone()))
        }
        serde_json::Value::Array(items) => {
            if items.len() > MAX_LIST_LEN {
                return Err(RuleEvalError::limit());
            }
            let list = items
                .iter()
                .map(|i| convert(i, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            Value::List(Arc::new(list))
        }
        serde_json::Value::Object(entries) => {
            if entries.len() > MAX_LIST_LEN {
                return Err(RuleEvalError::limit());
            }
            let mut map = HashMap::with_capacity(entries.len());
            for (k, val) in entries {
                if k.len() > MAX_STRING_BYTES {
                    return Err(RuleEvalError::limit());
                }
                map.insert(Key::String(Arc::new(k.clone())), convert(val, depth + 1)?);
            }
            Value::Map(Map { map: Arc::new(map) })
        }
    })
}
