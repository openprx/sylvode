//! Field read policy for everything derived from record values: titles, filters, sorts.
//!
//! A field marked `read:false` for a role is stripped from `values`, but a record also carries a
//! stored `title` rendered from **all** fields, and the query surface (filter, sort, aggregate) can
//! be used as an oracle on a field whose value is never shown. Every outlet that is built from
//! record values goes through this module so a denied field leaves no fragment behind.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::{error::ApiError, routes::form::render_title};

/// Stable error code for a query (filter, sort, aggregate) on a field the caller may not read.
pub const FIELD_READ_DENIED: &str = "field_read_denied";

/// The refusal for a query on a read-denied field. Names the field key, never a value.
pub fn field_read_denied(field_key: &str) -> ApiError {
    ApiError::Coded {
        status: 403,
        code: FIELD_READ_DENIED,
        message: format!("form field is not readable: {field_key}"),
        details: Some(json!({ "field_key": field_key })),
    }
}

/// Refuses a query on `field_key` when the caller may not read it.
pub fn ensure_field_readable(denied_read_fields: &BTreeSet<String>, field_key: &str) -> Result<(), ApiError> {
    if denied_read_fields.contains(field_key) {
        Err(field_read_denied(field_key))
    } else {
        Ok(())
    }
}

/// What a record query may touch for one caller of one form.
#[derive(Debug, Clone, Copy)]
pub struct QueryReadGuard<'a> {
    pub denied_read_fields: &'a BTreeSet<String>,
    pub title_template: &'a str,
}

impl QueryReadGuard<'_> {
    /// Refuses a filter or sort on a read-denied field.
    pub fn ensure_field_readable(&self, field_key: &str) -> Result<(), ApiError> {
        ensure_field_readable(self.denied_read_fields, field_key)
    }

    /// Refuses ordering or matching on the stored title when the title template renders a field
    /// the caller may not read: the order (or a match) of titles would reveal that field.
    pub fn ensure_title_queryable(&self) -> Result<(), ApiError> {
        match first_denied_template_field(self.title_template, self.denied_read_fields) {
            Some(field_key) => Err(field_read_denied(field_key)),
            None => Ok(()),
        }
    }
}

/// The first read-denied field the title template renders, if any.
pub fn first_denied_template_field<'a>(template: &str, denied_read_fields: &'a BTreeSet<String>) -> Option<&'a str> {
    denied_read_fields
        .iter()
        .find(|field_key| template.contains(&placeholder(field_key)))
        .map(String::as_str)
}

/// The record title as this reader may see it.
///
/// With no denied field the stored title is returned unchanged. Otherwise the stored title is
/// discarded (it was rendered from every field, or supplied by a writer who could read them) and
/// the title is rendered again from the template with the denied placeholders removed and only the
/// readable values substituted.
pub fn reader_title(
    template: &str,
    record_id: Uuid,
    stored_title: String,
    values: &Value,
    denied_read_fields: &BTreeSet<String>,
) -> String {
    if denied_read_fields.is_empty() {
        return stored_title;
    }
    let mut readable_template = template.to_string();
    for field_key in denied_read_fields {
        readable_template = readable_template.replace(&placeholder(field_key), "");
    }
    let mut readable_values = values.clone();
    if let Some(object) = readable_values.as_object_mut() {
        for field_key in denied_read_fields {
            object.remove(field_key);
        }
    }
    render_title(&readable_template, record_id, &readable_values)
}

fn placeholder(field_key: &str) -> String {
    format!("{{{field_key}}}")
}

#[cfg(test)]
mod tests {
    use super::{FIELD_READ_DENIED, QueryReadGuard, ensure_field_readable, reader_title};
    use crate::error::ApiError;
    use serde_json::json;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn denied(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    #[test]
    fn reader_title_drops_every_fragment_of_a_denied_field() {
        let record_id = Uuid::new_v4();
        let values = json!({"name": "Ada", "salary": "9000"});
        let stored = "Ada 9000".to_string();

        let title = reader_title("{name} {salary}", record_id, stored, &values, &denied(&["salary"]));

        assert_eq!(title.trim(), "Ada");
        assert!(!title.contains("9000"));
        assert!(!title.contains("salary"));
    }

    #[test]
    fn reader_title_ignores_a_writer_supplied_title_when_fields_are_denied() {
        let record_id = Uuid::new_v4();
        let values = json!({"name": "Ada", "salary": "9000"});

        let title = reader_title(
            "{name}",
            record_id,
            "custom 9000".to_string(),
            &values,
            &denied(&["salary"]),
        );

        assert_eq!(title, "Ada");
    }

    #[test]
    fn reader_title_keeps_the_stored_title_without_denied_fields() {
        let title = reader_title(
            "{name}",
            Uuid::new_v4(),
            "stored".to_string(),
            &json!({"name": "Ada"}),
            &BTreeSet::new(),
        );

        assert_eq!(title, "stored");
    }

    #[test]
    fn reader_title_falls_back_to_the_record_id_when_nothing_readable_remains() {
        let record_id = Uuid::new_v4();

        let title = reader_title(
            "{salary}",
            record_id,
            "9000".to_string(),
            &json!({"salary": "9000"}),
            &denied(&["salary"]),
        );

        assert_eq!(title, record_id.to_string());
    }

    #[test]
    fn queries_on_denied_fields_are_refused_with_a_typed_code_and_no_value() {
        let denied = denied(&["salary"]);
        let error = ensure_field_readable(&denied, "salary").expect_err("denied field must be refused");

        match error {
            ApiError::Coded {
                status, code, message, ..
            } => {
                assert_eq!(status, 403);
                assert_eq!(code, FIELD_READ_DENIED);
                assert!(message.contains("salary"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(ensure_field_readable(&denied, "name").is_ok());
    }

    #[test]
    fn title_queries_are_refused_only_when_the_template_renders_a_denied_field() {
        let denied = denied(&["salary"]);
        let leaking = QueryReadGuard {
            denied_read_fields: &denied,
            title_template: "{name} {salary}",
        };
        let safe = QueryReadGuard {
            denied_read_fields: &denied,
            title_template: "{name}",
        };

        assert!(leaking.ensure_title_queryable().is_err());
        assert!(safe.ensure_title_queryable().is_ok());
    }
}
