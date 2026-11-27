//! Built-in `rules.normalize` and `rules.transform` ops (contract C2 §3.1, §3.3).

use std::str::FromStr;

use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization as _;

use super::report::SchemaIssue;

const MAX_ROUND_SCALE: u64 = 12;
const MAX_TRUNCATE: u64 = 1_048_576;

/// A built-in op of either stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinOp {
    Trim,
    CollapseSpaces,
    Lower,
    Upper,
    Nfc,
    Round { scale: u32, half_even: bool },
    Slug,
    Truncate { max: usize },
}

impl BuiltinOp {
    /// Parses `{"op": ...}` at `path`; `allowed` lists the op names the field type accepts in
    /// this stage.
    pub fn parse(path: &str, object: &Map<String, Value>, allowed: &[&str]) -> Result<Self, SchemaIssue> {
        let name = object
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| SchemaIssue::new(format!("{path}/op"), "rule_unknown", "op must be a string"))?;
        let known = [
            "trim",
            "collapse_spaces",
            "lower",
            "upper",
            "nfc",
            "round",
            "slug",
            "truncate",
        ];
        if !known.contains(&name) {
            return Err(SchemaIssue::new(format!("{path}/op"), "rule_unknown", "unknown op"));
        }
        if !allowed.contains(&name) {
            return Err(SchemaIssue::new(
                format!("{path}/op"),
                "rule_not_allowed_for_type",
                "op not allowed for this field type or stage",
            ));
        }
        let param_err =
            |key: &str| SchemaIssue::new(format!("{path}/{key}"), "rule_param_invalid", "invalid op parameter");
        let params: &[&str] = match name {
            "round" => &["scale", "mode"],
            "truncate" => &["max"],
            _ => &[],
        };
        if let Some(key) = object
            .keys()
            .find(|key| *key != "op" && !params.contains(&key.as_str()))
        {
            return Err(param_err(key));
        }
        Ok(match name {
            "trim" => Self::Trim,
            "collapse_spaces" => Self::CollapseSpaces,
            "lower" => Self::Lower,
            "upper" => Self::Upper,
            "nfc" => Self::Nfc,
            "slug" => Self::Slug,
            "round" => {
                let scale = object
                    .get("scale")
                    .and_then(Value::as_u64)
                    .filter(|scale| *scale <= MAX_ROUND_SCALE)
                    .and_then(|scale| u32::try_from(scale).ok())
                    .ok_or_else(|| param_err("scale"))?;
                let half_even = match object.get("mode").map(|mode| mode.as_str()) {
                    None | Some(Some("half_even")) => true,
                    Some(Some("half_up")) => false,
                    Some(_) => return Err(param_err("mode")),
                };
                Self::Round { scale, half_even }
            }
            _ => {
                let max = object
                    .get("max")
                    .and_then(Value::as_u64)
                    .filter(|max| (1..=MAX_TRUNCATE).contains(max))
                    .and_then(|max| usize::try_from(max).ok())
                    .ok_or_else(|| param_err("max"))?;
                Self::Truncate { max }
            }
        })
    }

    /// Applies the op to a type-normalised value. The caller re-normalises the result by type;
    /// a value of a shape the op does not read is returned unchanged.
    #[must_use]
    pub fn apply(self, value: &Value) -> Value {
        match self {
            Self::Round { scale, half_even } => round(value, scale, half_even),
            _ => value
                .as_str()
                .map_or_else(|| value.clone(), |text| Value::String(self.apply_text(text))),
        }
    }

    fn apply_text(self, text: &str) -> String {
        match self {
            Self::Trim => text.trim().to_string(),
            Self::CollapseSpaces => collapse_spaces(text),
            Self::Lower => text.to_lowercase(),
            Self::Upper => text.to_uppercase(),
            Self::Nfc => text.nfc().collect(),
            Self::Slug => slug(text),
            Self::Truncate { max } => text.chars().take(max).collect(),
            Self::Round { .. } => text.to_string(),
        }
    }
}

fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
}

fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dash = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Rounds a stored decimal (`{"decimal": "..."}`) to `scale`; the result is the decimal string the
/// type re-normalises (an amount then applies its own scale).
fn round(value: &Value, scale: u32, half_even: bool) -> Value {
    let Some(decimal) = value
        .get("decimal")
        .and_then(Value::as_str)
        .and_then(|text| Decimal::from_str(text).ok())
    else {
        return value.clone();
    };
    let strategy = if half_even {
        RoundingStrategy::MidpointNearestEven
    } else {
        RoundingStrategy::MidpointAwayFromZero
    };
    let rounded = decimal.round_dp_with_strategy(scale, strategy);
    let mut object = value.as_object().cloned().unwrap_or_default();
    object.insert("decimal".to_string(), Value::String(rounded.to_string()));
    Value::Object(object)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use serde_json::json;

    use super::*;

    fn op(raw: &Value, allowed: &[&str]) -> Result<BuiltinOp, &'static str> {
        let empty = Map::new();
        BuiltinOp::parse("/x", raw.as_object().unwrap_or(&empty), allowed).map_err(|issue| issue.code)
    }

    #[test]
    fn parses_ops_against_the_type() {
        assert_eq!(op(&json!({"op": "trim"}), &["trim"]), Ok(BuiltinOp::Trim));
        assert_eq!(op(&json!({"op": "trim"}), &["lower"]), Err("rule_not_allowed_for_type"));
        assert_eq!(op(&json!({"op": "nope"}), &["trim"]), Err("rule_unknown"));
        assert_eq!(
            op(&json!({"op": "truncate", "max": 0}), &["truncate"]),
            Err("rule_param_invalid")
        );
        assert_eq!(op(&json!({"op": "trim", "x": 1}), &["trim"]), Err("rule_param_invalid"));
        assert_eq!(
            op(&json!({"op": "round", "scale": 1, "mode": "half_up"}), &["round"]),
            Ok(BuiltinOp::Round {
                scale: 1,
                half_even: false
            })
        );
    }

    #[test]
    fn applies_text_and_decimal_ops() {
        assert_eq!(BuiltinOp::Trim.apply(&json!("  a b ")), json!("a b"));
        assert_eq!(BuiltinOp::CollapseSpaces.apply(&json!("a  \t b")), json!("a b"));
        assert_eq!(BuiltinOp::Slug.apply(&json!(" Hello, World! ")), json!("hello-world"));
        assert_eq!(BuiltinOp::Truncate { max: 2 }.apply(&json!("héllo")), json!("hé"));
        let rounded = BuiltinOp::Round {
            scale: 1,
            half_even: true,
        }
        .apply(&json!({"type": "decimal", "decimal": "2.25"}));
        assert_eq!(rounded["decimal"], "2.2");
    }
}
