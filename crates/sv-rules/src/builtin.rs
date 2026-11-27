//! Rule objects of `rules.validate` (contract C2 §2, §3.2) and the built-in rule checks.

use std::cmp::Ordering;
use std::str::FromStr;

use regex::Regex;
use rust_decimal::Decimal;
use serde_json::{Map, Value};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::decimal::is_canonical_decimal;
use crate::error::RuleDefError;
use crate::expr::{Env, Program, compile};
use crate::limits::{MAX_ENUM_VALUES, MAX_LENGTH_BOUND, MAX_SAFE_INTEGER};
use crate::regex_subset::compile_pattern;
use crate::text::{is_email, is_url, parse_iso_date, scalar_len};

const MAX_CODE_LEN: usize = 40;
const MAX_DEPS: usize = 16;
const URL_SCHEMES: &[&str] = &["https", "http"];

/// Rule kinds a field type may allow (contract C2 §4.1 `RuleKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuleKind {
    Min,
    Max,
    MinLength,
    MaxLength,
    Pattern,
    Enum,
    Email,
    Url,
    Cel,
    Plugin,
}

impl RuleKind {
    /// Every kind, in schema order.
    pub const ALL: &'static [Self] = &[
        Self::Min,
        Self::Max,
        Self::MinLength,
        Self::MaxLength,
        Self::Pattern,
        Self::Enum,
        Self::Email,
        Self::Url,
        Self::Cel,
        Self::Plugin,
    ];

    /// The schema name (`min_length`, `cel`, ...).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::Max => "max",
            Self::MinLength => "min_length",
            Self::MaxLength => "max_length",
            Self::Pattern => "pattern",
            Self::Enum => "enum",
            Self::Email => "email",
            Self::Url => "url",
            Self::Cel => "cel",
            Self::Plugin => "plugin",
        }
    }

    /// Parses a schema name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.name() == name)
    }
}

/// A `min` / `max` bound: an integer, a decimal string, an ISO date or an RFC 3339 date-time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bound {
    Int(i64),
    Decimal(Decimal),
    Date(Date),
    DateTime(OffsetDateTime),
}

/// The value a rule is checked against, produced by the field type (contract C2 §4.1 `operand`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    Int(i64),
    Decimal(Decimal),
    Date(Date),
    DateTime(OffsetDateTime),
    /// Text; lengths count Unicode scalar values.
    Str(String),
    /// Item count (multi-select) for `min_length` / `max_length`.
    Count(usize),
}

/// A parsed built-in rule.
#[derive(Clone, Debug)]
pub enum BuiltinRule {
    Min {
        bound: Bound,
        raw: Value,
    },
    Max {
        bound: Bound,
        raw: Value,
    },
    MinLength(u64),
    MaxLength(u64),
    Pattern {
        source: String,
        case_insensitive: bool,
        regex: Regex,
    },
    Enum(Vec<String>),
    Email,
    Url {
        schemes: Vec<&'static str>,
    },
}

impl BuiltinRule {
    /// The rule kind.
    #[must_use]
    pub const fn kind(&self) -> RuleKind {
        match self {
            Self::Min { .. } => RuleKind::Min,
            Self::Max { .. } => RuleKind::Max,
            Self::MinLength(_) => RuleKind::MinLength,
            Self::MaxLength(_) => RuleKind::MaxLength,
            Self::Pattern { .. } => RuleKind::Pattern,
            Self::Enum(_) => RuleKind::Enum,
            Self::Email => RuleKind::Email,
            Self::Url { .. } => RuleKind::Url,
        }
    }

    /// Error `params` for a failure: schema constants only, never the submitted value.
    #[must_use]
    pub fn params(&self) -> Option<Map<String, Value>> {
        let bound = match self {
            Self::Min { raw, .. } | Self::Max { raw, .. } => raw.clone(),
            Self::MinLength(n) | Self::MaxLength(n) => Value::from(*n),
            _ => return None,
        };
        let mut m = Map::new();
        m.insert("bound".to_owned(), bound);
        Some(m)
    }
}

/// A plugin rule reference (`{"plugin": {"package", "rule"}, "deps", "config"}`); checked against
/// installations by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRuleRef {
    pub package: String,
    pub rule: String,
    pub deps: Vec<String>,
    pub config: Value,
}

/// What a rule object asks for.
#[derive(Debug)]
pub enum RuleSpecKind {
    Builtin(BuiltinRule),
    /// A `field_rule` CEL predicate.
    Cel(Program),
    Plugin(PluginRuleRef),
}

/// A parsed rule object with its common members.
#[derive(Debug)]
pub struct RuleSpec {
    pub kind: RuleSpecKind,
    /// Error code override (`^[a-z][a-z0-9_]{0,39}$`).
    pub code: Option<String>,
    /// Client message texts by language; the server never echoes them.
    pub message: Option<Map<String, Value>>,
    /// Run on an empty value (`cel` and `plugin` only).
    pub run_on_empty: bool,
}

impl RuleSpec {
    /// The rule kind.
    #[must_use]
    pub const fn rule_kind(&self) -> RuleKind {
        match &self.kind {
            RuleSpecKind::Builtin(b) => b.kind(),
            RuleSpecKind::Cel(_) => RuleKind::Cel,
            RuleSpecKind::Plugin(_) => RuleKind::Plugin,
        }
    }
}

fn pointer(key: &str) -> String {
    format!("/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn param_err(key: &str) -> RuleDefError {
    RuleDefError::new(pointer(key), "rule_param_invalid")
}

fn is_code(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= MAX_CODE_LEN
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A JSON integer within ±(2^53 - 1), written as an integer or an integral float, so the browser
/// (which reads every JSON number as a double) parses the same value.
fn json_int(v: &Value) -> Option<i64> {
    let n = v.as_number()?;
    let i = if let Some(i) = n.as_i64() {
        i
    } else {
        let f = n.as_f64()?;
        #[allow(clippy::cast_precision_loss)]
        let in_range = f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER as f64;
        if !in_range {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let i = f as i64;
        i
    };
    (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&i).then_some(i)
}

fn parse_bound(v: &Value) -> Option<Bound> {
    match v {
        Value::Number(_) => json_int(v).map(Bound::Int),
        Value::String(s) if is_canonical_decimal(s) => Decimal::from_str(s).ok().map(Bound::Decimal),
        Value::String(s) if s.len() == 10 => parse_iso_date(s).map(Bound::Date),
        Value::String(s) => OffsetDateTime::parse(s, &Rfc3339).ok().map(Bound::DateTime),
        _ => None,
    }
}

fn length_bound(v: Option<&Value>) -> Option<u64> {
    v.and_then(json_int)
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n <= MAX_LENGTH_BOUND)
}

fn parse_builtin(name: &str, obj: &Map<String, Value>) -> Result<(BuiltinRule, &'static [&'static str]), RuleDefError> {
    let value = obj.get("value");
    Ok(match name {
        "min" | "max" => {
            let raw = value.ok_or_else(|| param_err("value"))?;
            let bound = parse_bound(raw).ok_or_else(|| param_err("value"))?;
            let raw = raw.clone();
            let rule = if name == "min" {
                BuiltinRule::Min { bound, raw }
            } else {
                BuiltinRule::Max { bound, raw }
            };
            (rule, &["value"])
        }
        "min_length" | "max_length" => {
            let n = length_bound(value).ok_or_else(|| param_err("value"))?;
            let rule = if name == "min_length" {
                BuiltinRule::MinLength(n)
            } else {
                BuiltinRule::MaxLength(n)
            };
            (rule, &["value"])
        }
        "pattern" => {
            let source = value.and_then(Value::as_str).ok_or_else(|| param_err("value"))?;
            let case_insensitive = match obj.get("case_insensitive") {
                None => false,
                Some(v) => v.as_bool().ok_or_else(|| param_err("case_insensitive"))?,
            };
            let regex = compile_pattern(source, case_insensitive).map_err(|code| RuleDefError::new("/value", code))?;
            (
                BuiltinRule::Pattern {
                    source: source.to_owned(),
                    case_insensitive,
                    regex,
                },
                &["value", "case_insensitive"],
            )
        }
        "enum" => {
            let values = obj
                .get("values")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty() && a.len() <= MAX_ENUM_VALUES)
                .ok_or_else(|| param_err("values"))?;
            let values = values
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| param_err("values"))?;
            (BuiltinRule::Enum(values), &["values"])
        }
        "email" => (BuiltinRule::Email, &[]),
        "url" => {
            let schemes = match obj.get("schemes") {
                None => vec!["https"],
                Some(v) => {
                    let items = v
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .ok_or_else(|| param_err("schemes"))?;
                    let mut out: Vec<&'static str> = Vec::new();
                    for item in items {
                        let s = item.as_str().ok_or_else(|| param_err("schemes"))?;
                        let known = URL_SCHEMES
                            .iter()
                            .copied()
                            .find(|k| *k == s)
                            .ok_or_else(|| param_err("schemes"))?;
                        if out.contains(&known) {
                            return Err(param_err("schemes"));
                        }
                        out.push(known);
                    }
                    out
                }
            };
            (BuiltinRule::Url { schemes }, &["schemes"])
        }
        _ => return Err(RuleDefError::new("/rule", "rule_unknown")),
    })
}

fn parse_plugin(obj: &Map<String, Value>) -> Result<PluginRuleRef, RuleDefError> {
    let plugin = obj
        .get("plugin")
        .and_then(Value::as_object)
        .ok_or_else(|| param_err("plugin"))?;
    let text = |key: &str| -> Result<String, RuleDefError> {
        plugin
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| RuleDefError::new(format!("/plugin{}", pointer(key)), "rule_param_invalid"))
    };
    let package = text("package")?;
    let rule = text("rule")?;
    if plugin.keys().any(|k| k != "package" && k != "rule") {
        return Err(param_err("plugin"));
    }
    let deps = match obj.get("deps") {
        None => Vec::new(),
        Some(v) => v
            .as_array()
            .filter(|a| a.len() <= MAX_DEPS)
            .and_then(|a| {
                a.iter()
                    .map(|d| d.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| param_err("deps"))?,
    };
    let config = match obj.get("config") {
        None => Value::Object(Map::new()),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(param_err("config")),
    };
    Ok(PluginRuleRef {
        package,
        rule,
        deps,
        config,
    })
}

/// Parses one `rules.validate` entry. `allowed` is the field type's rule list; a kind outside it is
/// `rule_not_allowed_for_type`. Unknown members are refused.
///
/// # Errors
/// [`RuleDefError`] with a JSON Pointer relative to the rule object and one of `rule_unknown`,
/// `rule_not_allowed_for_type`, `rule_param_invalid`, `pattern_invalid`, `pattern_unsupported`, or a
/// CEL compile code from [`compile`].
pub fn parse_rule(v: &Value, allowed: &[RuleKind]) -> Result<RuleSpec, RuleDefError> {
    let obj = v.as_object().ok_or_else(|| RuleDefError::root("rule_param_invalid"))?;
    let present: Vec<&str> = ["rule", "cel", "plugin"]
        .into_iter()
        .filter(|k| obj.contains_key(*k))
        .collect();
    let (selector, kind, spec, params): (&str, RuleKind, RuleSpecKind, &[&str]) = match present.as_slice() {
        [] => return Err(RuleDefError::root("rule_unknown")),
        ["rule"] => {
            let name = obj
                .get("rule")
                .and_then(Value::as_str)
                .ok_or_else(|| RuleDefError::new("/rule", "rule_unknown"))?;
            let kind = RuleKind::from_name(name)
                .filter(|k| !matches!(k, RuleKind::Cel | RuleKind::Plugin))
                .ok_or_else(|| RuleDefError::new("/rule", "rule_unknown"))?;
            if !allowed.contains(&kind) {
                return Err(RuleDefError::new("/rule", "rule_not_allowed_for_type"));
            }
            let (rule, params) = parse_builtin(name, obj)?;
            ("rule", kind, RuleSpecKind::Builtin(rule), params)
        }
        ["cel"] => {
            if !allowed.contains(&RuleKind::Cel) {
                return Err(RuleDefError::new("/cel", "rule_not_allowed_for_type"));
            }
            let src = obj.get("cel").and_then(Value::as_str).ok_or_else(|| param_err("cel"))?;
            let program = compile(src, Env::FieldRule).map_err(|e| RuleDefError::new("/cel", e.code))?;
            ("cel", RuleKind::Cel, RuleSpecKind::Cel(program), &[])
        }
        ["plugin"] => {
            if !allowed.contains(&RuleKind::Plugin) {
                return Err(RuleDefError::new("/plugin", "rule_not_allowed_for_type"));
            }
            let plugin = parse_plugin(obj)?;
            (
                "plugin",
                RuleKind::Plugin,
                RuleSpecKind::Plugin(plugin),
                &["deps", "config"],
            )
        }
        _ => return Err(RuleDefError::root("rule_param_invalid")),
    };
    let cel_like = matches!(kind, RuleKind::Cel | RuleKind::Plugin);
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for key in keys {
        let known = key == selector
            || params.contains(&key.as_str())
            || key == "code"
            || key == "message"
            || (key == "run_on_empty" && cel_like);
        if !known {
            return Err(param_err(key));
        }
    }
    let code = match obj.get("code") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .filter(|s| is_code(s))
                .map(str::to_owned)
                .ok_or_else(|| param_err("code"))?,
        ),
    };
    let message = match obj.get("message") {
        None => None,
        Some(Value::Object(m)) if m.values().all(Value::is_string) => Some(m.clone()),
        Some(_) => return Err(param_err("message")),
    };
    let run_on_empty = match obj.get("run_on_empty") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| param_err("run_on_empty"))?,
    };
    Ok(RuleSpec {
        kind: spec,
        code,
        message,
        run_on_empty,
    })
}

fn compare(op: &Operand, bound: &Bound) -> Option<Ordering> {
    Some(match (op, bound) {
        (Operand::Int(a), Bound::Int(b)) => a.cmp(b),
        (Operand::Int(a), Bound::Decimal(b)) => Decimal::from(*a).cmp(b),
        (Operand::Decimal(a), Bound::Decimal(b)) => a.cmp(b),
        (Operand::Decimal(a), Bound::Int(b)) => a.cmp(&Decimal::from(*b)),
        (Operand::Date(a), Bound::Date(b)) => a.cmp(b),
        (Operand::DateTime(a), Bound::DateTime(b)) => a.cmp(b),
        _ => return None,
    })
}

fn text(op: &Operand) -> Result<&str, &'static str> {
    match op {
        Operand::Str(s) => Ok(s),
        _ => Err("rule_invalid"),
    }
}

fn length(op: &Operand) -> Result<u64, &'static str> {
    let n = match op {
        Operand::Str(s) => scalar_len(s),
        Operand::Count(n) => *n,
        _ => return Err("rule_invalid"),
    };
    u64::try_from(n).map_err(|_| "rule_invalid")
}

/// Checks a non-empty operand against a built-in rule.
///
/// # Errors
/// The rule's error code (`min`, `max`, `min_length`, `max_length`, `pattern`, `enum`, `email`,
/// `url`) when the value fails, or `rule_invalid` when the operand kind does not fit the rule. The
/// caller applies a `code` override.
pub fn check_builtin(rule: &BuiltinRule, op: &Operand) -> Result<(), &'static str> {
    let pass = match rule {
        BuiltinRule::Min { bound, .. } => compare(op, bound).ok_or("rule_invalid")? != Ordering::Less,
        BuiltinRule::Max { bound, .. } => compare(op, bound).ok_or("rule_invalid")? != Ordering::Greater,
        BuiltinRule::MinLength(n) => length(op)? >= *n,
        BuiltinRule::MaxLength(n) => length(op)? <= *n,
        BuiltinRule::Pattern { regex, .. } => regex.is_match(text(op)?),
        BuiltinRule::Enum(values) => {
            let s = text(op)?;
            values.iter().any(|v| v == s)
        }
        BuiltinRule::Email => is_email(text(op)?),
        BuiltinRule::Url { schemes } => is_url(text(op)?, schemes),
    };
    if pass { Ok(()) } else { Err(rule.kind().name()) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn check(rule: &Value, op: &Operand) -> Result<(), &'static str> {
        match parse_rule(rule, RuleKind::ALL).map(|s| s.kind) {
            Ok(RuleSpecKind::Builtin(b)) => check_builtin(&b, op),
            Ok(_) => Err("not builtin"),
            Err(e) => Err(e.code),
        }
    }

    #[test]
    fn bounds() {
        assert_eq!(
            check(&json!({"rule": "max", "value": 100}), &Operand::Int(101)),
            Err("max")
        );
        assert_eq!(
            check(
                &json!({"rule": "min", "value": "1.50"}),
                &Operand::Decimal(Decimal::new(15, 1))
            ),
            Ok(())
        );
        assert_eq!(
            check(
                &json!({"rule": "min", "value": "2026-01-01"}),
                &Operand::Str("x".into())
            ),
            Err("rule_invalid")
        );
        assert_eq!(
            check(&json!({"rule": "max_length", "value": 2}), &Operand::Str("äöü".into())),
            Err("max_length")
        );
    }

    #[test]
    fn rule_objects() {
        let code = |v: Value, allowed: &[RuleKind]| parse_rule(&v, allowed).err().map(|e| (e.path, e.code));
        assert_eq!(
            code(json!({"rule": "nope"}), RuleKind::ALL),
            Some(("/rule".into(), "rule_unknown"))
        );
        assert_eq!(
            code(json!({"rule": "email"}), &[RuleKind::Min]),
            Some(("/rule".into(), "rule_not_allowed_for_type"))
        );
        assert_eq!(
            code(json!({"rule": "email", "run_on_empty": true}), RuleKind::ALL),
            Some(("/run_on_empty".into(), "rule_param_invalid"))
        );
        assert_eq!(
            code(json!({"cel": "secret"}), RuleKind::ALL),
            Some(("/cel".into(), "cel_unknown_variable"))
        );
        assert_eq!(
            code(json!({"rule": "pattern", "value": "(?i)a"}), RuleKind::ALL),
            Some(("/value".into(), "pattern_unsupported"))
        );
        assert_eq!(
            code(json!({"rule": "email", "code": "Bad"}), RuleKind::ALL),
            Some(("/code".into(), "rule_param_invalid"))
        );
        assert!(
            parse_rule(
                &json!({"plugin": {"package": "acme/phone", "rule": "phone"}, "deps": ["country"]}),
                RuleKind::ALL
            )
            .is_ok()
        );
    }
}
