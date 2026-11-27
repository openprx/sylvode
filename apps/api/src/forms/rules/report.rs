//! The error report of record writes, validation and schema saves (contract C2 §9).
//!
//! A write error names the field, the stage and a stable code; `params` only carries schema
//! constants (bounds), never the submitted or stored value. The message lists field keys and codes
//! so clients that only show the message (the MCP tools) still see what failed.

use std::borrow::Cow;

use serde::Serialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::error::ApiError;

/// Code of a refused record write or validation.
pub const VALIDATION_FAILED: &str = "validation_failed";
/// Code of a refused schema save.
pub const SCHEMA_INVALID: &str = "schema_invalid";
/// Errors reported per write; later ones are dropped and `truncated` is set.
pub const MAX_REPORTED_ERRORS: usize = 100;

/// The pipeline stage that produced an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Permission,
    Default,
    ReadOnly,
    Normalize,
    Validate,
    Transform,
}

/// One field error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FieldError {
    #[serde(rename = "field")]
    pub key: String,
    pub field_id: String,
    pub stage: Stage,
    pub code: Cow<'static, str>,
    /// `validate[<index>]`, `normalize[<index>]` or `transform[<index>]`.
    pub rule: Option<String>,
    pub params: Option<Map<String, Value>>,
}

impl FieldError {
    pub fn new(key: &str, field_id: &str, stage: Stage, code: impl Into<Cow<'static, str>>) -> Self {
        Self {
            key: key.to_string(),
            field_id: field_id.to_string(),
            stage,
            code: code.into(),
            rule: None,
            params: None,
        }
    }

    #[must_use]
    pub fn with_rule(mut self, rule: String, params: Option<Map<String, Value>>) -> Self {
        self.rule = Some(rule);
        self.params = params;
        self
    }

    /// The JSON item, with `row` for import reports.
    #[must_use]
    pub fn to_json(&self, row: Option<usize>) -> Value {
        json!({
            "field": self.key,
            "field_id": self.field_id,
            "stage": self.stage,
            "code": self.code,
            "rule": self.rule,
            "params": self.params,
            "row": row,
        })
    }
}

/// A rule failure carried by a system write (recalculation) instead of refusing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CarriedError {
    #[serde(flatten)]
    pub error: FieldError,
    /// The record the failure was stored on (the parent when a child write triggered it); `None`
    /// on a preview.
    pub record_id: Option<Uuid>,
}

impl CarriedError {
    /// The carried items of one record, without fields the reader may not read (a carried failure
    /// of a hidden field would reveal something about its value).
    pub fn for_reader(
        errors: Vec<FieldError>,
        record_id: Option<Uuid>,
        denied_read_fields: &std::collections::BTreeSet<String>,
    ) -> Vec<Self> {
        errors
            .into_iter()
            .filter(|error| !denied_read_fields.contains(&error.key))
            .map(|error| Self { error, record_id })
            .collect()
    }
}

/// Field keys and codes of carried errors, for logs (never values).
pub fn carried_summary(carried: &[CarriedError]) -> String {
    carried
        .iter()
        .map(|item| format!("{}:{}", item.error.key, item.error.code))
        .collect::<Vec<_>>()
        .join(",")
}

/// The errors of one write, capped at [`MAX_REPORTED_ERRORS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldErrors {
    pub errors: Vec<FieldError>,
    pub truncated: bool,
}

impl FieldErrors {
    pub fn push(&mut self, error: FieldError) {
        if self.errors.len() < MAX_REPORTED_ERRORS {
            self.errors.push(error);
        } else {
            self.truncated = true;
        }
    }

    pub fn extend(&mut self, other: Self) {
        self.truncated |= other.truncated;
        for error in other.errors {
            self.push(error);
        }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    #[must_use]
    pub fn single(error: FieldError) -> Self {
        Self {
            errors: vec![error],
            truncated: false,
        }
    }

    /// `<key> (<code>)` items, for messages and logs.
    #[must_use]
    pub fn summary(&self) -> String {
        self.errors
            .iter()
            .map(|error| format!("{} ({})", error.key, error.code))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The `validation_failed` refusal.
    #[must_use]
    pub fn into_api_error(self) -> ApiError {
        self.into_api_error_for_row(None)
    }

    /// The `validation_failed` refusal with each item's `row` set (imports).
    #[must_use]
    pub fn into_api_error_for_row(self, row: Option<usize>) -> ApiError {
        ApiError::Coded {
            status: 400,
            code: VALIDATION_FAILED,
            message: format!("validation failed: {}", self.summary()),
            details: Some(self.details(row)),
        }
    }

    /// `{"errors": [...], "truncated": bool}`.
    #[must_use]
    pub fn details(&self, row: Option<usize>) -> Value {
        json!({
            "errors": self.errors.iter().map(|error| error.to_json(row)).collect::<Vec<_>>(),
            "truncated": self.truncated,
        })
    }
}

impl From<FieldErrors> for ApiError {
    fn from(errors: FieldErrors) -> Self {
        errors.into_api_error()
    }
}

/// One schema problem: a JSON Pointer into the schema and a code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SchemaIssue {
    pub path: String,
    pub code: &'static str,
    /// Fixed English detail for the legacy string surface and logs; never serialised.
    #[serde(skip)]
    pub detail: String,
}

impl SchemaIssue {
    pub fn new(path: impl Into<String>, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            code,
            detail: detail.into(),
        }
    }
}

/// The issues of a schema save.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaIssues {
    pub errors: Vec<SchemaIssue>,
    pub warnings: Vec<SchemaIssue>,
}

impl SchemaIssues {
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// The legacy string form used by callers that report a plain message.
    #[must_use]
    pub fn message(&self) -> String {
        let items = self
            .errors
            .iter()
            .map(|issue| {
                if issue.detail.is_empty() {
                    format!("{} at {}", issue.code, issue.path)
                } else {
                    format!("{} ({} at {})", issue.detail, issue.code, issue.path)
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!("schema invalid: {items}")
    }

    /// The `schema_invalid` refusal.
    #[must_use]
    pub fn into_api_error(self) -> ApiError {
        let message = self.message();
        ApiError::Coded {
            status: 400,
            code: SCHEMA_INVALID,
            message,
            details: Some(json!({ "errors": self.errors, "warnings": self.warnings })),
        }
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_report_caps_errors_and_never_carries_values() {
        let mut errors = FieldErrors::default();
        for index in 0..=MAX_REPORTED_ERRORS {
            errors.push(FieldError::new(&format!("f{index}"), "fld_x", Stage::Validate, "max"));
        }
        assert!(errors.truncated);
        let ApiError::Coded {
            code, details, message, ..
        } = errors.into_api_error()
        else {
            return;
        };
        assert_eq!(code, VALIDATION_FAILED);
        assert!(message.starts_with("validation failed: f0 (max)"));
        let details = details.unwrap_or_default();
        assert_eq!(details["errors"].as_array().map(Vec::len), Some(MAX_REPORTED_ERRORS));
        assert_eq!(details["truncated"], true);
        assert_eq!(details["errors"][0]["stage"], "validate");
        assert_eq!(details["errors"][0]["row"], Value::Null);
    }
}
