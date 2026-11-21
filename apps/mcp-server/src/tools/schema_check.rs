//! Validation of tool arguments against a tool's `inputSchema`.
//!
//! `sylvode_read` and `sylvode_write` call a registry tool on behalf of a model that has only
//! read the tool's schema. The tool bodies deserialize their own arguments, but a misspelled or
//! out-of-range argument should be refused with the schema's own words before any request is
//! made, and an argument the schema does not declare must never travel further. This checker
//! implements exactly the JSON Schema keywords the registry uses ([`SUPPORTED_KEYWORDS`]).
//!
//! It fails closed: a schema that uses a keyword outside the list, or a `pattern` outside
//! [`KNOWN_PATTERNS`], makes validation fail instead of passing silently, and a test walks every
//! schema of the registry, the task tools and the entry tools to prove none does.

use serde_json::{Map, Value};
use uuid::Uuid;

/// Every keyword a schema may use: the constraints implemented below, then the annotations
/// (`description`, `default`, `title`) that constrain nothing.
pub const SUPPORTED_KEYWORDS: &[&str] = &[
    "type",
    "required",
    "properties",
    "additionalProperties",
    "enum",
    "const",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "items",
    "pattern",
    "format",
    "oneOf",
    "anyOf",
    "not",
    "description",
    "default",
    "title",
];

/// The check that implements one `pattern`.
pub type PatternCheck = fn(&str) -> bool;

/// The `pattern` values the registry uses, each with the check that implements it.
pub const KNOWN_PATTERNS: &[(&str, PatternCheck)] = &[
    (
        r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
        is_lower_uuid,
    ),
    (
        r"^(PROP-)?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
        is_proposal_id,
    ),
    (r"^[A-Za-z0-9]+-[0-9]+$", is_work_item_identifier),
    (r"^[a-z_]+(\.[a-z_]+)+$", is_dotted_tool_name),
];

fn is_lower_uuid(text: &str) -> bool {
    is_uuid_shape(text, |c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn is_proposal_id(text: &str) -> bool {
    let rest = text.strip_prefix("PROP-").unwrap_or(text);
    is_uuid_shape(rest, |c| c.is_ascii_hexdigit())
}

fn is_uuid_shape(text: &str, digit: impl Fn(char) -> bool) -> bool {
    let groups = text.split('-').collect::<Vec<_>>();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, length)| group.len() == length && group.chars().all(&digit))
}

fn is_work_item_identifier(text: &str) -> bool {
    text.split_once('-').is_some_and(|(key, number)| {
        !key.is_empty()
            && key.chars().all(|c| c.is_ascii_alphanumeric())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
    })
}

fn is_dotted_tool_name(text: &str) -> bool {
    let segments = text.split('.').collect::<Vec<_>>();
    segments.len() >= 2
        && segments
            .iter()
            .all(|segment| !segment.is_empty() && segment.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
}

/// Keywords used by `schema` (and every schema nested in it) that are not implemented, and
/// `pattern` values that have no check. Empty for a schema this module can enforce completely.
pub fn unsupported(schema: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_unsupported(schema, "", &mut found);
    found
}

fn collect_unsupported(schema: &Value, path: &str, found: &mut Vec<String>) {
    let Some(object) = schema.as_object() else {
        found.push(format!("{path}: a schema must be an object"));
        return;
    };
    for (keyword, value) in object {
        if !SUPPORTED_KEYWORDS.contains(&keyword.as_str()) {
            found.push(format!("{path}/{keyword}: unsupported keyword"));
            continue;
        }
        match keyword.as_str() {
            "pattern" => {
                if !value
                    .as_str()
                    .is_some_and(|pattern| KNOWN_PATTERNS.iter().any(|(known, _)| *known == pattern))
                {
                    found.push(format!("{path}/pattern: unsupported pattern {value}"));
                }
            }
            "format" => {
                if value.as_str() != Some("uuid") {
                    found.push(format!("{path}/format: unsupported format {value}"));
                }
            }
            "properties" => match value.as_object() {
                Some(properties) => {
                    for (name, property) in properties {
                        collect_unsupported(property, &format!("{path}/properties/{name}"), found);
                    }
                }
                None => found.push(format!("{path}/properties: must be an object")),
            },
            "items" | "not" => collect_unsupported(value, &format!("{path}/{keyword}"), found),
            "additionalProperties" => {
                if value.is_object() {
                    collect_unsupported(value, &format!("{path}/additionalProperties"), found);
                } else if !value.is_boolean() {
                    found.push(format!("{path}/additionalProperties: must be a boolean or a schema"));
                }
            }
            "oneOf" | "anyOf" => match value.as_array() {
                Some(branches) => {
                    for (index, branch) in branches.iter().enumerate() {
                        collect_unsupported(branch, &format!("{path}/{keyword}/{index}"), found);
                    }
                }
                None => found.push(format!("{path}/{keyword}: must be an array of schemas")),
            },
            "required" => {
                if !value.as_array().is_some_and(|names| names.iter().all(Value::is_string)) {
                    found.push(format!("{path}/required: must be an array of names"));
                }
            }
            "type" => {
                let known = |name: &Value| {
                    name.as_str().is_some_and(|name| {
                        matches!(
                            name,
                            "object" | "array" | "string" | "boolean" | "null" | "integer" | "number"
                        )
                    })
                };
                let valid = match value {
                    Value::Array(names) => names.iter().all(known),
                    other => known(other),
                };
                if !valid {
                    found.push(format!("{path}/type: unknown type {value}"));
                }
            }
            "enum" => {
                if !value.is_array() {
                    found.push(format!("{path}/enum: must be an array"));
                }
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" => {
                if value.as_u64().is_none() {
                    found.push(format!("{path}/{keyword}: must be a non-negative integer"));
                }
            }
            "minimum" | "maximum" if !value.is_number() => {
                found.push(format!("{path}/{keyword}: must be a number"));
            }
            _ => {}
        }
    }
}

/// Checks `value` against `schema`; the error names the first violation with its location.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(problem) = unsupported(schema).into_iter().next() {
        return Err(format!("the tool's schema cannot be enforced ({problem})"));
    }
    check(schema, value, "arguments")
}

fn check(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    if let Some(expected) = schema.get("type") {
        check_type(expected, value, at)?;
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        return Err(format!("{at} must be one of {}", join_values(options)));
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        return Err(format!("{at} must be {expected}"));
    }
    check_number(schema, value, at)?;
    check_string(schema, value, at)?;
    check_array(schema, value, at)?;
    if let Some(object) = value.as_object() {
        check_object(schema, object, at)?;
    }
    check_combinators(schema, value, at)
}

fn join_values(values: &[Value]) -> String {
    values.iter().map(Value::to_string).collect::<Vec<_>>().join(", ")
}

fn check_type(expected: &Value, value: &Value, at: &str) -> Result<(), String> {
    let matches = |name: &str| match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    };
    let allowed = match expected {
        Value::String(name) => vec![name.as_str()],
        Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if allowed.iter().any(|name| matches(name)) {
        Ok(())
    } else {
        Err(format!("{at} must be of type {}", allowed.join(" or ")))
    }
}

fn check_number(schema: &Map<String, Value>, value: &Value, at: &str) -> Result<(), String> {
    let Some(number) = value.as_f64().filter(|_| value.is_number()) else {
        return Ok(());
    };
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && number < minimum
    {
        return Err(format!("{at} must be at least {minimum}"));
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && number > maximum
    {
        return Err(format!("{at} must be at most {maximum}"));
    }
    Ok(())
}

fn check_string(schema: &Map<String, Value>, value: &Value, at: &str) -> Result<(), String> {
    let Some(text) = value.as_str() else {
        return Ok(());
    };
    let length = text.chars().count() as u64;
    if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64)
        && length < minimum
    {
        return Err(format!("{at} must have at least {minimum} characters"));
    }
    if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64)
        && length > maximum
    {
        return Err(format!("{at} must have at most {maximum} characters"));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        let accepted = KNOWN_PATTERNS
            .iter()
            .find(|(known, _)| *known == pattern)
            .is_some_and(|(_, matches)| matches(text));
        if !accepted {
            return Err(format!("{at} has the wrong format"));
        }
    }
    // The canonical 8-4-4-4-12 form only: the braced, urn and simple spellings denote the same id
    // but the policy chain refuses them, so the checker must not accept what the chain refuses.
    if schema.get("format").and_then(Value::as_str) == Some("uuid")
        && (text.len() != 36 || Uuid::parse_str(text).is_err())
    {
        return Err(format!("{at} must be a UUID"));
    }
    Ok(())
}

fn check_array(schema: &Map<String, Value>, value: &Value, at: &str) -> Result<(), String> {
    let Some(items) = value.as_array() else {
        return Ok(());
    };
    let length = items.len() as u64;
    if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64)
        && length < minimum
    {
        return Err(format!("{at} must have at least {minimum} items"));
    }
    if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64)
        && length > maximum
    {
        return Err(format!("{at} must have at most {maximum} items"));
    }
    if let Some(item_schema) = schema.get("items") {
        for (index, item) in items.iter().enumerate() {
            check(item_schema, item, &format!("{at}[{index}]"))?;
        }
    }
    Ok(())
}

fn check_object(schema: &Map<String, Value>, object: &Map<String, Value>, at: &str) -> Result<(), String> {
    for required in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
        if let Some(name) = required.as_str()
            && object.get(name).is_none_or(Value::is_null)
        {
            return Err(format!("{at}.{name} is required"));
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    let required = |name: &str| {
        schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|names| names.iter().any(|candidate| candidate.as_str() == Some(name)))
    };
    for (name, value) in object {
        let declared = properties.and_then(|properties| properties.get(name));
        match (declared, schema.get("additionalProperties")) {
            // An optional argument sent as null is "not given", as the tools have always read it.
            (Some(_), _) if value.is_null() && !required(name) => {}
            (Some(property), _) => check(property, value, &format!("{at}.{name}"))?,
            (None, Some(Value::Bool(false))) => return Err(format!("{at}.{name} is not an argument of this tool")),
            (None, Some(extra @ Value::Object(_))) => check(extra, value, &format!("{at}.{name}"))?,
            (None, _) => {}
        }
    }
    Ok(())
}

fn check_combinators(schema: &Map<String, Value>, value: &Value, at: &str) -> Result<(), String> {
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let passing = branches
            .iter()
            .filter(|branch| check(branch, value, at).is_ok())
            .count();
        if passing != 1 {
            return Err(format!("{at} must match exactly one of the allowed argument shapes"));
        }
    }
    if let Some(branches) = schema.get("anyOf").and_then(Value::as_array)
        && !branches.iter().any(|branch| check(branch, value, at).is_ok())
    {
        return Err(format!("{at} must match one of the allowed argument shapes"));
    }
    if let Some(inner) = schema.get("not")
        && check(inner, value, at).is_ok()
    {
        return Err(format!("{at} must not match the excluded argument shape"));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    fn accepts(schema: &Value, value: &Value) -> bool {
        validate(schema, value).is_ok()
    }

    #[test]
    fn types_required_and_additional_properties() {
        let schema = json!({
            "type": "object", "additionalProperties": false, "required": ["id"],
            "properties": { "id": { "type": "string" }, "n": { "type": "integer", "minimum": 1, "maximum": 5 } }
        });
        assert!(accepts(&schema, &json!({ "id": "x", "n": 3 })));
        assert!(
            accepts(&schema, &json!({ "id": "x", "n": null })),
            "an optional null is not given"
        );
        assert!(!accepts(&schema, &json!({ "n": 3 })), "required");
        assert!(!accepts(&schema, &json!({ "id": null })), "a required null is missing");
        assert!(!accepts(&schema, &json!({ "id": "x", "extra": 1 })), "additional");
        assert!(!accepts(&schema, &json!({ "id": 1 })), "type");
        assert!(!accepts(&schema, &json!({ "id": "x", "n": 6 })), "maximum");
        assert!(!accepts(&schema, &json!({ "id": "x", "n": 1.5 })), "integer");
        assert!(!accepts(&schema, &json!([])), "object");
    }

    #[test]
    fn strings_arrays_enums_constants_and_formats() {
        let schema = json!({
            "type": "object",
            "properties": {
                "s": { "type": "string", "minLength": 2, "maxLength": 3 },
                "a": { "type": "array", "minItems": 1, "maxItems": 2, "items": { "type": "string", "enum": ["x", "y"] } },
                "c": { "const": true },
                "u": { "type": "string", "format": "uuid" },
                "p": { "type": "string", "pattern": "^[A-Za-z0-9]+-[0-9]+$" }
            }
        });
        assert!(accepts(
            &schema,
            &json!({ "s": "ab", "a": ["x"], "c": true, "p": "PRX-42" })
        ));
        assert!(!accepts(&schema, &json!({ "s": "a" })));
        assert!(!accepts(&schema, &json!({ "s": "abcd" })));
        assert!(!accepts(&schema, &json!({ "a": [] })));
        assert!(!accepts(&schema, &json!({ "a": ["x", "y", "x"] })));
        assert!(!accepts(&schema, &json!({ "a": ["z"] })));
        assert!(!accepts(&schema, &json!({ "c": false })));
        assert!(!accepts(&schema, &json!({ "u": "not-a-uuid" })));
        assert!(accepts(
            &schema,
            &json!({ "u": "11111111-1111-4111-8111-111111111111" })
        ));
        assert!(!accepts(&schema, &json!({ "p": "PRX42" })));
        assert!(!accepts(&schema, &json!({ "p": "../x-1" })));
    }

    #[test]
    fn one_of_any_of_and_not() {
        let schema = json!({
            "type": "object",
            "oneOf": [
                { "required": ["a"], "not": { "required": ["b"] } },
                { "required": ["b"], "not": { "required": ["a"] } }
            ],
            "anyOf": [{ "required": ["a"] }, { "required": ["b"] }]
        });
        assert!(accepts(&schema, &json!({ "a": 1 })));
        assert!(accepts(&schema, &json!({ "b": 1 })));
        assert!(!accepts(&schema, &json!({ "a": 1, "b": 1 })));
        assert!(!accepts(&schema, &json!({})));
    }

    #[test]
    fn an_unsupported_keyword_or_pattern_fails_closed() {
        for schema in [
            json!({ "type": "object", "patternProperties": {} }),
            json!({ "type": "object", "properties": { "a": { "type": "string", "pattern": "^x$" } } }),
            json!({ "type": "string", "format": "email" }),
            json!({ "type": "object", "properties": { "a": { "$ref": "#/x" } } }),
            json!({ "allOf": [] }),
        ] {
            assert!(!unsupported(&schema).is_empty(), "{schema}");
            assert!(validate(&schema, &json!({})).is_err(), "{schema}");
        }
    }

    /// A `uuid` format is the canonical 8-4-4-4-12 form, like the policy chain reads it: the braced,
    /// `urn:` and 32 digit spellings denote the same id but are refused by the chain, so the checker
    /// refuses them too instead of disagreeing with it.
    #[test]
    fn a_uuid_format_is_the_canonical_hyphenated_form() {
        let schema = json!({ "type": "string", "format": "uuid" });
        for good in [
            "11111111-1111-4111-8111-111111111111",
            "AAAAAAAA-1111-4111-8111-111111111111",
        ] {
            assert!(accepts(&schema, &json!(good)), "{good}");
        }
        for bad in [
            "{11111111-1111-4111-8111-111111111111}",
            "urn:uuid:11111111-1111-4111-8111-111111111111",
            "11111111111141118111111111111111",
            " 11111111-1111-4111-8111-111111111111",
            "11111111-1111-4111-8111-11111111111",
        ] {
            assert!(!accepts(&schema, &json!(bad)), "{bad}");
        }
    }

    /// A schema whose keyword values are malformed is not enforceable: `properties` that is not an
    /// object would otherwise be skipped, leaving an open schema where a closed one was meant.
    #[test]
    fn malformed_keyword_values_are_unsupported() {
        for schema in [
            json!({ "type": "object", "properties": [] }),
            json!({ "type": "object", "properties": { "a": 1 } }),
            json!({ "type": "object", "required": "a" }),
            json!({ "type": "object", "required": [1] }),
            json!({ "type": "nonsense" }),
            json!({ "type": ["string", 5] }),
            json!({ "type": "object", "additionalProperties": "no" }),
            json!({ "enum": "a" }),
            json!({ "type": "string", "minLength": "1" }),
            json!({ "type": "array", "items": [] }),
            json!({ "oneOf": {} }),
            json!({ "oneOf": [1] }),
        ] {
            assert!(!unsupported(&schema).is_empty(), "{schema}");
            assert!(validate(&schema, &json!({})).is_err(), "{schema}");
        }
        for schema in [
            json!({ "type": ["object", "null"], "properties": {}, "required": [], "additionalProperties": true }),
            json!({ "type": "string", "minLength": 1, "maxLength": 3, "enum": ["a"] }),
        ] {
            assert!(unsupported(&schema).is_empty(), "{schema}");
        }
    }

    #[test]
    fn the_known_patterns_match_their_regular_expressions() {
        let lower = KNOWN_PATTERNS[0].1;
        assert!(lower("11111111-1111-4111-8111-111111111111"));
        assert!(!lower("11111111-1111-4111-8111-11111111111G"));
        assert!(!lower("AAAAAAAA-1111-4111-8111-111111111111"));
        assert!(!lower("11111111-1111-4111-8111-1111111111111"));
        let proposal = KNOWN_PATTERNS[1].1;
        assert!(proposal("PROP-AAAAAAAA-1111-4111-8111-111111111111"));
        assert!(proposal("aaaaaaaa-1111-4111-8111-111111111111"));
        assert!(!proposal("PROP-PROP-aaaaaaaa-1111-4111-8111-111111111111"));
        let identifier = KNOWN_PATTERNS[2].1;
        assert!(identifier("PRX-42") && identifier("a1-0"));
        assert!(!identifier("PRX-") && !identifier("-1") && !identifier("PRX-4-2") && !identifier("PR_X-1"));
        let dotted = KNOWN_PATTERNS[3].1;
        assert!(dotted("work_items.update") && dotted("a.b.c"));
        assert!(!dotted("sylvode_read") && !dotted("a..b") && !dotted(".a") && !dotted("a.B") && !dotted("a.b-c"));
    }
}
