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
    /// Fields the caller may not read: no filter or sort on them.
    pub denied_read_fields: &'a BTreeSet<String>,
    /// Fields whose content may not show through the title (denied fields and restricted relations).
    pub title_excluded_fields: &'a BTreeSet<String>,
    pub title_template: &'a str,
}

impl QueryReadGuard<'_> {
    /// Refuses a filter or sort on a read-denied field.
    pub fn ensure_field_readable(&self, field_key: &str) -> Result<(), ApiError> {
        ensure_field_readable(self.denied_read_fields, field_key)
    }

    /// Refuses ordering or matching on the stored title when the title template renders an excluded
    /// field: the order (or a match) of titles would reveal that field.
    pub fn ensure_title_queryable(&self) -> Result<(), ApiError> {
        match first_excluded_template_field(self.title_template, self.title_excluded_fields) {
            Some(field_key) => Err(field_read_denied(field_key)),
            None => Ok(()),
        }
    }
}

/// The first excluded field the title template renders, if any.
pub fn first_excluded_template_field<'a>(template: &str, excluded_fields: &'a BTreeSet<String>) -> Option<&'a str> {
    excluded_fields
        .iter()
        .find(|field_key| template.contains(&placeholder(field_key)))
        .map(String::as_str)
}

/// Keys a relation value keeps for a reader its target form restricts: the reference, never a
/// label rendered from the target's values.
const RELATION_REFERENCE_KEYS: [&str; 2] = ["record_id", "form_key"];

/// Everything a record response must hide from one reader of one form, decided from one
/// exclusion set: the read-denied fields plus the relation fields whose target restricts the
/// reader. Every outlet asks this policy for titles, values and query guards.
#[derive(Debug, Clone)]
pub struct RecordReadPolicy {
    title_template: String,
    denied_read_fields: BTreeSet<String>,
    restricted_relation_fields: BTreeSet<String>,
    excluded_fields: BTreeSet<String>,
}

impl RecordReadPolicy {
    #[must_use]
    pub fn new(title_template: String, denied: BTreeSet<String>, restricted: BTreeSet<String>) -> Self {
        let excluded_fields = denied.union(&restricted).cloned().collect();
        Self {
            title_template,
            denied_read_fields: denied,
            restricted_relation_fields: restricted,
            excluded_fields,
        }
    }

    #[must_use]
    pub fn unrestricted(title_template: String) -> Self {
        Self::new(title_template, BTreeSet::new(), BTreeSet::new())
    }

    #[must_use]
    pub fn denied_read_fields(&self) -> &BTreeSet<String> {
        &self.denied_read_fields
    }

    #[must_use]
    pub fn restricted_relation_fields(&self) -> &BTreeSet<String> {
        &self.restricted_relation_fields
    }

    #[must_use]
    pub fn excluded_fields(&self) -> &BTreeSet<String> {
        &self.excluded_fields
    }

    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        self.excluded_fields.is_empty()
    }

    /// The record title for this reader. Unrestricted: the stored title. Otherwise the stored (or
    /// writer-supplied) title is discarded and rendered again without the excluded fields.
    #[must_use]
    pub fn title(&self, record_id: Uuid, stored: String, values: &Value) -> String {
        if self.is_unrestricted() {
            return stored;
        }
        render_record_title(&self.title_template, record_id, values, &self.excluded_fields)
    }

    /// Values for this reader: denied keys removed, restricted relation values reduced to
    /// `{record_id, form_key}` (an object, or each object of an array).
    #[must_use]
    pub fn values(&self, mut values: Value) -> Value {
        if self.is_unrestricted() {
            return values;
        }
        if let Some(object) = values.as_object_mut() {
            for field_key in &self.denied_read_fields {
                object.remove(field_key);
            }
            for field_key in &self.restricted_relation_fields {
                if let Some(value) = object.get_mut(field_key) {
                    reduce_relation_value(value);
                }
            }
        }
        values
    }

    #[must_use]
    pub fn query_guard(&self) -> QueryReadGuard<'_> {
        QueryReadGuard {
            denied_read_fields: &self.denied_read_fields,
            title_excluded_fields: &self.excluded_fields,
            title_template: &self.title_template,
        }
    }
}

/// Reduces a relation value to its reference keys (allow-list, so `label`, `name`, `title` and
/// `display` all go).
pub fn reduce_relation_value(value: &mut Value) {
    match value {
        Value::Object(object) => object.retain(|key, _| RELATION_REFERENCE_KEYS.contains(&key.as_str())),
        Value::Array(items) => items.iter_mut().for_each(reduce_relation_value),
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
/// display value of `key`, or nothing when the key is excluded or has no value. Inserted text
/// is never scanned again, so a value that itself contains `{other}` stays literal and the output
/// cannot grow beyond the template plus each value once. The result is capped at
/// [`MAX_TITLE_CHARS`]; an empty result falls back to the record id.
pub fn render_record_title(
    template: &str,
    record_id: Uuid,
    values: &Value,
    excluded_fields: &BTreeSet<String>,
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
            } else if !excluded_fields.contains(key)
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
///
/// A relation object (it has `record_id`) renders its `title`, else its `display`, else nothing;
/// an array renders its items joined by `", "`, empties skipped. Never raw JSON.
pub fn title_display_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items
            .iter()
            .map(title_display_value)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(object) => {
            if object.contains_key("record_id") {
                return ["title", "display"]
                    .into_iter()
                    .find_map(|key| object.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()))
                    .unwrap_or_default()
                    .to_string();
            }
            if let Some(decimal) = object.get("decimal").and_then(Value::as_str) {
                if let Some(currency) = object.get("currency").and_then(Value::as_str) {
                    return format!("{decimal} {currency}");
                }
                return decimal.to_string();
            }
            if let Some(number) = object.get("value").and_then(Value::as_i64) {
                return number.to_string();
            }
            value.to_string()
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{
        FIELD_READ_DENIED, MAX_TITLE_CHARS, RecordReadPolicy, cap_title, ensure_field_readable, render_record_title,
        title_display_value,
    };
    use crate::error::ApiError;
    use serde_json::json;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn set(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    fn policy(template: &str, denied: &[&str], restricted: &[&str]) -> RecordReadPolicy {
        RecordReadPolicy::new(template.to_string(), set(denied), set(restricted))
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
    fn relation_values_render_their_label_and_arrays_join() {
        assert_eq!(
            title_display_value(&json!({"record_id": "r1", "title": "ACME"})),
            "ACME"
        );
        assert_eq!(title_display_value(&json!({"record_id": "r1", "display": "D"})), "D");
        assert_eq!(title_display_value(&json!({"record_id": "r1"})), "");
        assert_eq!(
            title_display_value(&json!([{"record_id": "r1", "title": "A"}, {"record_id": "r2"}, "b"])),
            "A, b"
        );
    }

    #[test]
    fn a_restricted_relation_alone_rerenders_the_title_without_it() {
        let record_id = Uuid::new_v4();
        let values = json!({"name": "Ada", "customer": {"record_id": "r1", "title": "Ada 9000"}});
        let reader = policy("{name} {customer}", &[], &["customer"]);

        for stored in ["Ada Ada 9000", "custom Ada 9000"] {
            let title = reader.title(record_id, stored.to_string(), &values);
            assert!(title.contains("Ada"));
            assert!(!title.contains("9000"));
            assert!(!title.contains("r1"));
        }
    }

    #[test]
    fn the_exclusion_set_is_the_union_and_drives_unrestricted() {
        let reader = policy("{name}", &["salary"], &["customer"]);
        assert_eq!(reader.excluded_fields(), &set(&["salary", "customer"]));
        assert!(!reader.is_unrestricted());
        assert!(!policy("{name}", &["salary"], &[]).is_unrestricted());
        assert!(!policy("{name}", &[], &["customer"]).is_unrestricted());
        assert!(RecordReadPolicy::unrestricted("{name}".to_string()).is_unrestricted());
        assert_eq!(
            RecordReadPolicy::unrestricted("{name}".to_string()).title(
                Uuid::new_v4(),
                "stored".to_string(),
                &json!({})
            ),
            "stored"
        );
    }

    #[test]
    fn values_drop_denied_keys_and_reduce_relation_values_to_their_reference() {
        let reader = policy("{name}", &["salary"], &["customer", "lines"]);
        let values = json!({
            "name": "Ada",
            "salary": "9000",
            "customer": {"record_id": "r1", "form_key": "c", "title": "x", "label": "y", "name": "z"},
            "lines": [{"record_id": "r2", "display": "secret"}],
            "other": {"record_id": "r3", "title": "kept"}
        });

        let filtered = reader.values(values);

        assert!(filtered.get("salary").is_none());
        assert_eq!(filtered["customer"], json!({"record_id": "r1", "form_key": "c"}));
        assert_eq!(filtered["lines"], json!([{"record_id": "r2"}]));
        assert_eq!(filtered["other"]["title"], "kept");
    }

    #[test]
    fn title_queries_are_refused_when_the_template_renders_a_restricted_relation() {
        let restricted = policy("{name} {customer}", &[], &["customer"]);
        let clean = policy("{name}", &[], &["customer"]);

        assert!(restricted.query_guard().ensure_title_queryable().is_err());
        assert!(clean.query_guard().ensure_title_queryable().is_ok());
        assert!(restricted.query_guard().ensure_field_readable("customer").is_ok());
        assert!(
            policy("{name}", &["salary"], &[])
                .query_guard()
                .ensure_field_readable("salary")
                .is_err()
        );
    }

    #[test]
    fn a_denied_field_leaves_no_fragment_in_the_title() {
        let record_id = Uuid::new_v4();
        let reader = policy("{name} {salary}", &["salary"], &[]);
        let values = json!({"name": "Ada", "salary": "9000"});

        assert_eq!(reader.title(record_id, "Ada 9000".to_string(), &values).trim(), "Ada");
        assert_eq!(
            policy("{salary}", &["salary"], &[]).title(record_id, "9000".to_string(), &values),
            record_id.to_string()
        );
    }

    #[test]
    fn queries_on_denied_fields_are_refused_with_a_typed_code_and_no_value() {
        let denied = set(&["salary"]);
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
}
