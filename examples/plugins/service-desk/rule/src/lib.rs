//! Sample `field-rule` plugin: a service-desk reference code is `SD-` and six digits.
//!
//! `normalize` trims and upper-cases the value; `validate` refuses a value that is not a reference
//! code with the violation `invalid_reference`; `transform` keeps the value. A rule has no imports.

wit_bindgen::generate!({
    path: "../../../../wit/sv-plugin.wit",
    world: "field-rule",
});

use sv::plugin::rule_types::Stage;

struct Plugin;

/// `SD-` and exactly six ASCII digits.
fn is_reference(code: &str) -> bool {
    code.strip_prefix("SD-")
        .is_some_and(|digits| digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn violation(code: &str) -> RuleViolation {
    RuleViolation { code: code.to_string() }
}

impl Guest for Plugin {
    fn apply(input: RuleInput) -> Result<RuleOutcome, RuleViolation> {
        let value: serde_json::Value =
            serde_json::from_str(&input.value).map_err(|_| violation("invalid_reference"))?;
        match input.stage {
            Stage::Normalize => match value.as_str() {
                Some(text) => {
                    let normalized = text.trim().to_uppercase();
                    if normalized == text {
                        Ok(RuleOutcome::Keep)
                    } else {
                        Ok(RuleOutcome::Replace(serde_json::Value::String(normalized).to_string()))
                    }
                }
                None => Ok(RuleOutcome::Keep),
            },
            Stage::Validate => match value.as_str() {
                // An empty value is the form's own `required` rule to judge.
                Some("") | None if value.is_null() => Ok(RuleOutcome::Keep),
                Some(text) if is_reference(text) => Ok(RuleOutcome::Keep),
                _ => Err(violation("invalid_reference")),
            },
            Stage::Transform => Ok(RuleOutcome::Keep),
        }
    }
}

export!(Plugin);
