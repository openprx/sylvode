//! Field read policy for everything derived from record values: titles, filters, sorts.
//!
//! A field marked `read:false` for a role is stripped from `values`, but a record also carries a
//! stored `title` rendered from **all** fields, and the query surface (filter, sort, aggregate) can
//! be used as an oracle on a field whose value is never shown. Every outlet that is built from
//! record values goes through this module so a denied field leaves no fragment behind.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;

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

/// Everything a record response must hide from one reader of one form.
#[derive(Debug, Clone, Default)]
pub struct RecordReadPolicy {
    pub title_template: String,
    pub denied_read_fields: BTreeSet<String>,
    /// Relation fields whose target form hides at least one field from this reader. Their stored
    /// snapshot (`title`, `display`) was rendered from the target's full values and is dropped.
    pub restricted_relation_fields: BTreeSet<String>,
}

impl RecordReadPolicy {
    /// Drops the `title`/`display` snapshot from relation values whose target form restricts this
    /// reader. The record id stays, so the client resolves a label through the filtered
    /// relation-target listing.
    #[must_use]
    pub fn redact_relation_snapshots(&self, mut values: Value) -> Value {
        if self.restricted_relation_fields.is_empty() {
            return values;
        }
        let Some(object) = values.as_object_mut() else {
            return values;
        };
        for field_key in &self.restricted_relation_fields {
            if let Some(value) = object.get_mut(field_key) {
                strip_relation_snapshot(value);
            }
        }
        values
    }
}

fn strip_relation_snapshot(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("title");
            object.remove("display");
        }
        Value::Array(items) => items.iter_mut().for_each(strip_relation_snapshot),
        _ => {}
    }
}

fn placeholder(field_key: &str) -> String {
    format!("{{{field_key}}}")
}

/// Longest title the renderer produces, in characters.
pub const MAX_TITLE_CHARS: usize = 1_024;

/// Renders a record title from its template in one pass.
///
/// The template is scanned once, left to right. `{id}` becomes the record id; `{key}` becomes the
/// display value of `key`, or nothing when the key is read-denied or has no value. Inserted text
/// is never scanned again, so a value that itself contains `{other}` stays literal and the output
/// cannot grow beyond the template plus each value once. The result is capped at
/// [`MAX_TITLE_CHARS`]; an empty result falls back to the record id.
pub fn render_record_title(
    template: &str,
    record_id: Uuid,
    values: &Value,
    denied_read_fields: &BTreeSet<String>,
) -> String {
    let mut title = String::new();
    let mut chars = 0usize;
    let mut rest = template;
    while chars < MAX_TITLE_CHARS && !rest.is_empty() {
        let token = rest
            .strip_prefix('{')
            .and_then(|after| after.find('}').map(|end| (after.get(..end), end)))
            .and_then(|(key, end)| key.filter(|key| is_placeholder_key(key)).map(|key| (key, end)));
        if let Some((key, end)) = token {
            if key == "id" {
                push_capped(&mut title, &mut chars, &record_id.to_string());
            } else if !denied_read_fields.contains(key)
                && let Some(value) = values.get(key)
            {
                push_capped(&mut title, &mut chars, &title_display_value(value));
            }
            rest = rest.get(end + 2..).unwrap_or_default();
            continue;
        }
        let mut literal = rest.char_indices();
        let next = literal
            .find(|(index, ch)| *index > 0 && *ch == '{')
            .map_or(rest.len(), |(index, _)| index);
        push_capped(&mut title, &mut chars, rest.get(..next).unwrap_or_default());
        rest = rest.get(next..).unwrap_or_default();
    }
    if title.trim().is_empty() {
        record_id.to_string()
    } else {
        title
    }
}

/// A writer-supplied title, cut to [`MAX_TITLE_CHARS`] on a character boundary, so stored titles
/// share the bound of rendered ones.
#[must_use]
pub fn cap_title(title: String) -> String {
    match title.char_indices().nth(MAX_TITLE_CHARS) {
        Some((end, _)) => title.get(..end).unwrap_or_default().to_string(),
        None => title,
    }
}

/// The text a single field value contributes to a title or a relation label.
pub fn title_display_value(value: &Value) -> String {
    if let Some(value) = value.as_str() {
        return value.to_string();
    }
    if let Some(decimal) = value.get("decimal").and_then(Value::as_str) {
        if let Some(currency) = value.get("currency").and_then(Value::as_str) {
            return format!("{decimal} {currency}");
        }
        return decimal.to_string();
    }
    if let Some(value) = value.get("value").and_then(Value::as_i64) {
        return value.to_string();
    }
    value.to_string()
}

fn is_placeholder_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn push_capped(title: &mut String, chars: &mut usize, text: &str) {
    for ch in text.chars() {
        if *chars >= MAX_TITLE_CHARS {
            return;
        }
        title.push(ch);
        *chars += 1;
    }
}

/// The record title as this reader may see it.
///
/// With no denied field the stored title is returned unchanged. Otherwise the stored title is
/// discarded (it was rendered from every field, or supplied by a writer who could read them) and
/// the title is rendered again from the template, skipping the denied fields.
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
    render_record_title(template, record_id, values, denied_read_fields)
}

#[cfg(test)]
mod tests {
    use super::{
        FIELD_READ_DENIED, MAX_TITLE_CHARS, QueryReadGuard, RecordReadPolicy, cap_title, ensure_field_readable,
        reader_title, render_record_title,
    };
    use crate::error::ApiError;
    use serde_json::json;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn denied(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    #[test]
    fn the_renderer_never_rescans_inserted_text_and_is_bounded() {
        let record_id = Uuid::new_v4();
        let chain = "{notes}".repeat(1_000);
        let values = json!({"name": chain, "notes": "x".repeat(10_000)});

        let title = render_record_title("{name}", record_id, &values, &BTreeSet::new());

        assert!(title.starts_with("{notes}{notes}"));
        assert!(!title.contains('x'));
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        let long = render_record_title("{notes} {notes}", record_id, &values, &BTreeSet::new());
        assert_eq!(long.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn writer_titles_are_capped_on_a_char_boundary() {
        let capped = cap_title("é".repeat(MAX_TITLE_CHARS + 10));
        assert_eq!(capped.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(cap_title("short".to_string()), "short");
    }

    #[test]
    fn the_renderer_keeps_literals_and_substitutes_known_tokens() {
        let record_id = Uuid::new_v4();
        let values = json!({"name": "Ada", "qty": {"type": "integer", "value": 3}});

        assert_eq!(
            render_record_title(
                "#{id} {name} x{qty} {missing} {not a key}",
                record_id,
                &values,
                &BTreeSet::new()
            ),
            format!("#{record_id} Ada x3  {{not a key}}")
        );
        assert_eq!(render_record_title("{", record_id, &values, &BTreeSet::new()), "{");
        assert_eq!(
            render_record_title("", record_id, &values, &BTreeSet::new()),
            record_id.to_string()
        );
    }

    #[test]
    fn relation_snapshots_lose_their_title_when_the_target_restricts_the_reader() {
        let policy = RecordReadPolicy {
            restricted_relation_fields: denied(&["customer"]),
            ..RecordReadPolicy::default()
        };
        let values = json!({
            "customer": {"record_id": "r1", "title": "ACME 9000", "display": "9000", "form_key": "c"},
            "lines": [{"record_id": "r2", "title": "secret"}],
            "other": {"record_id": "r3", "title": "kept"}
        });

        let redacted = policy.redact_relation_snapshots(values);

        assert_eq!(redacted["customer"], json!({"record_id": "r1", "form_key": "c"}));
        assert_eq!(redacted["other"]["title"], "kept");
        assert_eq!(redacted["lines"][0]["title"], "secret");
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
