//! The strict `sylvode.inbound_event.v1` envelope.
//!
//! Validation is written by hand over a parsed JSON value rather than derived, so every refusal
//! names the field path an integrator has to fix, an unknown top-level field is refused (a typo
//! must be visible), and the limits of the contract are the only place the rules live. Every text
//! field of the result is untrusted data and stays that way: nothing here makes it safe to follow.

use platform::timestamp::Timestamp;
use serde_json::{Map, Value};
use time::Duration;

use super::severity::Severity;

/// The only accepted value of `spec`.
pub const SPEC: &str = "sylvode.inbound_event.v1";
const ALLOWED_FIELDS: [&str; 10] = [
    "spec",
    "event_id",
    "occurred_at",
    "kind",
    "severity",
    "dedup_key",
    "title",
    "summary",
    "subject",
    "payload",
];

const EVENT_ID_MAX: usize = 128;
const KIND_MAX: usize = 64;
const KIND_SEGMENT_MAX: usize = 32;
const KIND_SEGMENTS_MAX: usize = 4;
const DEDUP_KEY_MAX: usize = 200;
const TITLE_MAX: usize = 200;
const SUMMARY_MAX: usize = 2_000;
const SUBJECT_KEYS_MAX: usize = 8;
const SUBJECT_KEY_MAX: usize = 32;
const SUBJECT_VALUE_MAX: usize = 200;
/// Largest serialised `payload`, in bytes.
pub const PAYLOAD_MAX_BYTES: usize = 32 * 1024;
/// Deepest nesting of `payload`.
pub const PAYLOAD_MAX_DEPTH: usize = 16;
const FUTURE_TOLERANCE: Duration = Duration::minutes(5);
const MAX_AGE: Duration = Duration::days(7);

/// A validated event. Free text fields have had control characters removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub event_id: String,
    pub occurred_at: Timestamp,
    pub kind: String,
    pub severity: Severity,
    pub dedup_key: String,
    pub title: String,
    pub summary: String,
    pub subject: Map<String, Value>,
    pub payload: Value,
}

/// Why an envelope was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    /// 400: the shape or a limit of the contract is violated.
    Invalid { path: String, reason: String },
    /// 422: `occurred_at` is in the future or too old.
    OccurredAtOutOfRange,
}

fn invalid(path: &str, reason: &str) -> EnvelopeError {
    EnvelopeError::Invalid {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

/// Removes control characters; `\n` survives when `keep_newline` is set.
pub fn strip_controls(text: &str, keep_newline: bool) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || (keep_newline && *ch == '\n'))
        .collect()
}

fn required_str<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, EnvelopeError> {
    match object.get(key) {
        None | Some(Value::Null) => Err(invalid(key, "is required")),
        Some(Value::String(text)) => Ok(text),
        Some(_) => Err(invalid(key, "must be a string")),
    }
}

fn optional_str<'a>(object: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, EnvelopeError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(invalid(key, "must be a string")),
    }
}

/// `[A-Za-z0-9._:@/-]{1,128}`.
fn valid_event_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= EVENT_ID_MAX
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'@' | b'/' | b'-'))
}

/// `^[a-z0-9_]{1,32}(\.[a-z0-9_]{1,32}){0,3}$`, at most 64 characters.
pub fn valid_kind(text: &str) -> bool {
    if text.len() > KIND_MAX {
        return false;
    }
    let mut segments = 0_usize;
    for segment in text.split('.') {
        segments += 1;
        if segments > KIND_SEGMENTS_MAX
            || segment.is_empty()
            || segment.len() > KIND_SEGMENT_MAX
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return false;
        }
    }
    true
}

fn valid_subject_key(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= SUBJECT_KEY_MAX
        && text.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn parse_subject(object: &Map<String, Value>) -> Result<Map<String, Value>, EnvelopeError> {
    let Some(value) = object.get("subject").filter(|value| !value.is_null()) else {
        return Ok(Map::new());
    };
    let Value::Object(entries) = value else {
        return Err(invalid("subject", "must be a flat object"));
    };
    if entries.len() > SUBJECT_KEYS_MAX {
        return Err(invalid("subject", "has more than 8 keys"));
    }
    let mut subject = Map::new();
    for (key, entry) in entries {
        let path = format!("subject.{key}");
        if !valid_subject_key(key) {
            return Err(invalid(&path, "key must be 1-32 characters of a-z and _"));
        }
        let Value::String(text) = entry else {
            return Err(invalid(&path, "must be a string"));
        };
        let text = strip_controls(text, false);
        if text.chars().count() > SUBJECT_VALUE_MAX {
            return Err(invalid(&path, "is longer than 200 characters"));
        }
        subject.insert(key.clone(), Value::String(text));
    }
    Ok(subject)
}

/// Validates a request body against the envelope contract at `now`.
pub fn parse_envelope(body: &[u8], now: Timestamp) -> Result<Envelope, EnvelopeError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| invalid("", "body is not valid JSON"))?;
    let Value::Object(object) = value else {
        return Err(invalid("", "body must be a JSON object"));
    };
    if let Some(unknown) = object.keys().find(|key| !ALLOWED_FIELDS.contains(&key.as_str())) {
        return Err(invalid(
            &unknown.chars().take(64).collect::<String>(),
            "is not a known field",
        ));
    }

    if required_str(&object, "spec")? != SPEC {
        return Err(invalid("spec", "must be sylvode.inbound_event.v1"));
    }

    let event_id = required_str(&object, "event_id")?;
    if !valid_event_id(event_id) {
        return Err(invalid(
            "event_id",
            "must be 1-128 characters of A-Z a-z 0-9 . _ : @ / -",
        ));
    }

    let occurred_at = Timestamp::parse_rfc3339(required_str(&object, "occurred_at")?)
        .map_err(|_| invalid("occurred_at", "must be an RFC 3339 timestamp"))?;

    let kind = required_str(&object, "kind")?;
    if !valid_kind(kind) {
        return Err(invalid(
            "kind",
            "must be dot separated lower case segments, at most 64 characters",
        ));
    }

    let severity = Severity::parse(required_str(&object, "severity")?)
        .ok_or_else(|| invalid("severity", "must be one of info, low, medium, high, critical"))?;

    let title = strip_controls(required_str(&object, "title")?, false);
    if title.is_empty() || title.chars().count() > TITLE_MAX {
        return Err(invalid("title", "must be 1-200 characters"));
    }

    let summary = match optional_str(&object, "summary")? {
        Some(text) => {
            let text = strip_controls(text, true);
            if text.chars().count() > SUMMARY_MAX {
                return Err(invalid("summary", "is longer than 2000 characters"));
            }
            text
        }
        None => String::new(),
    };

    let subject = parse_subject(&object)?;

    let payload = match object.get("payload") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(value) => {
            if depth(value) > PAYLOAD_MAX_DEPTH {
                return Err(invalid("payload", "is nested deeper than 16 levels"));
            }
            let size = serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len());
            if size > PAYLOAD_MAX_BYTES {
                return Err(invalid("payload", "is larger than 32 KiB"));
            }
            value.clone()
        }
    };

    let dedup_key = match optional_str(&object, "dedup_key")? {
        Some(key) => {
            if key.is_empty() || key.chars().count() > DEDUP_KEY_MAX || key.chars().any(char::is_control) {
                return Err(invalid("dedup_key", "must be 1-200 printable characters"));
            }
            key.to_owned()
        }
        None => default_dedup_key(kind, &subject),
    };

    if occurred_at > now + FUTURE_TOLERANCE || occurred_at < now - MAX_AGE {
        return Err(EnvelopeError::OccurredAtOutOfRange);
    }

    Ok(Envelope {
        event_id: event_id.to_owned(),
        occurred_at,
        kind: kind.to_owned(),
        severity,
        dedup_key,
        title,
        summary,
        subject,
        payload,
    })
}

/// `kind:host` when the subject names a host, else `kind`.
fn default_dedup_key(kind: &str, subject: &Map<String, Value>) -> String {
    subject
        .get("host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .map_or_else(
            || kind.to_owned(),
            |host| format!("{kind}:{host}").chars().take(DEDUP_KEY_MAX).collect(),
        )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> Timestamp {
        Timestamp::parse_rfc3339("2026-11-21T10:00:00Z").unwrap()
    }

    fn base() -> Value {
        json!({
            "spec": "sylvode.inbound_event.v1",
            "event_id": "waf-edge01-1",
            "occurred_at": "2026-11-21T09:41:07Z",
            "kind": "waf.attack_detected",
            "severity": "high",
            "title": "SQL injection burst on /login",
        })
    }

    fn parse(value: &Value) -> Result<Envelope, EnvelopeError> {
        parse_envelope(&serde_json::to_vec(value).unwrap(), now())
    }

    fn refused_at(value: &Value) -> String {
        match parse(value) {
            Err(EnvelopeError::Invalid { path, .. }) => path,
            other => panic!("expected an invalid envelope, got {other:?}"),
        }
    }

    fn with(key: &str, value: Value) -> Value {
        let mut body = base();
        body[key] = value;
        body
    }

    fn without(key: &str) -> Value {
        let mut body = base();
        body.as_object_mut().unwrap().remove(key);
        body
    }

    #[test]
    fn the_minimal_envelope_is_accepted_with_defaults() {
        let envelope = parse(&base()).expect("valid");
        assert_eq!(envelope.event_id, "waf-edge01-1");
        assert_eq!(envelope.severity, Severity::High);
        assert_eq!(envelope.dedup_key, "waf.attack_detected", "no host: the kind alone");
        assert_eq!(envelope.summary, "");
        assert_eq!(envelope.payload, json!({}));
        assert!(envelope.subject.is_empty());
    }

    #[test]
    fn the_default_dedup_key_adds_the_subject_host() {
        let body = with("subject", json!({"host": "shop.example.com", "asset": "edge-01"}));
        assert_eq!(parse(&body).unwrap().dedup_key, "waf.attack_detected:shop.example.com");
        let body = with("dedup_key", json!("custom-key"));
        assert_eq!(parse(&body).unwrap().dedup_key, "custom-key");
    }

    #[test]
    fn required_fields_are_named_in_the_refusal() {
        for field in ["spec", "event_id", "occurred_at", "kind", "severity", "title"] {
            assert_eq!(refused_at(&without(field)), field);
        }
        assert_eq!(refused_at(&with("title", json!(null))), "title");
    }

    #[test]
    fn spec_must_be_exact() {
        assert_eq!(refused_at(&with("spec", json!("sylvode.inbound_event.v2"))), "spec");
        assert_eq!(refused_at(&with("spec", json!(1))), "spec");
    }

    #[test]
    fn unknown_top_level_fields_are_refused_including_source() {
        assert_eq!(refused_at(&with("source", json!("someone-else"))), "source");
        assert_eq!(refused_at(&with("titel", json!("typo"))), "titel");
    }

    #[test]
    fn event_id_rules() {
        for good in ["a", "A.b_c:d@e/f-g", &"x".repeat(128)] {
            assert!(parse(&with("event_id", json!(good))).is_ok(), "{good}");
        }
        for bad in ["", "has space", "ünï", "semi;colon", &"x".repeat(129)] {
            assert_eq!(refused_at(&with("event_id", json!(bad))), "event_id", "{bad}");
        }
    }

    #[test]
    fn kind_rules() {
        for good in ["waf", "waf.attack_detected", "a.b.c.d", "a1_b2.c3"] {
            assert!(parse(&with("kind", json!(good))).is_ok(), "{good}");
        }
        let long_segment = "a".repeat(33);
        let long_kind = format!("{}.{}.{}", "a".repeat(32), "b".repeat(32), "c");
        for bad in [
            "",
            "WAF",
            "a..b",
            ".a",
            "a.",
            "a.b.c.d.e",
            "a-b",
            long_segment.as_str(),
            long_kind.as_str(),
        ] {
            assert_eq!(refused_at(&with("kind", json!(bad))), "kind", "{bad}");
        }
    }

    #[test]
    fn severity_must_be_one_of_five() {
        for good in ["info", "low", "medium", "high", "critical"] {
            assert!(parse(&with("severity", json!(good))).is_ok());
        }
        for bad in ["HIGH", "urgent", "", "warn"] {
            assert_eq!(refused_at(&with("severity", json!(bad))), "severity");
        }
    }

    #[test]
    fn title_is_stripped_of_control_characters_and_bounded() {
        let envelope = parse(&with("title", json!("a\u{0}b\u{7}c\nd"))).unwrap();
        assert_eq!(envelope.title, "abcd");
        assert_eq!(refused_at(&with("title", json!("\u{1}\u{2}"))), "title");
        assert_eq!(refused_at(&with("title", json!(""))), "title");
        assert!(parse(&with("title", json!("t".repeat(200)))).is_ok());
        assert_eq!(refused_at(&with("title", json!("t".repeat(201)))), "title");
    }

    #[test]
    fn summary_keeps_newlines_only() {
        let envelope = parse(&with("summary", json!("line1\nline2\u{0}\t!"))).unwrap();
        assert_eq!(envelope.summary, "line1\nline2!");
        assert_eq!(refused_at(&with("summary", json!("s".repeat(2001)))), "summary");
        assert!(parse(&with("summary", json!("s".repeat(2000)))).is_ok());
    }

    #[test]
    fn subject_is_a_small_flat_object_of_strings() {
        assert!(parse(&with("subject", json!({"host": "h", "asset_id": "a"}))).is_ok());
        assert_eq!(refused_at(&with("subject", json!(["x"]))), "subject");
        assert_eq!(refused_at(&with("subject", json!({"Host": "h"}))), "subject.Host");
        assert_eq!(refused_at(&with("subject", json!({"host": 5}))), "subject.host");
        assert_eq!(
            refused_at(&with("subject", json!({"host": {"a": "b"}}))),
            "subject.host"
        );
        assert_eq!(
            refused_at(&with("subject", json!({"host": "h".repeat(201)}))),
            "subject.host"
        );
        let nine: Map<String, Value> = (0..9).map(|i| (format!("k{}", "x".repeat(i)), json!("v"))).collect();
        assert_eq!(refused_at(&with("subject", Value::Object(nine))), "subject");
    }

    #[test]
    fn payload_is_bounded_in_size_and_depth() {
        assert!(parse(&with("payload", json!({"any": ["JSON", 1, null]}))).is_ok());
        let mut nested = json!("leaf");
        for _ in 0..PAYLOAD_MAX_DEPTH {
            nested = json!([nested]);
        }
        assert!(parse(&with("payload", nested.clone())).is_ok(), "depth 16 is allowed");
        assert_eq!(refused_at(&with("payload", json!([nested]))), "payload");
        let big = json!({"blob": "x".repeat(PAYLOAD_MAX_BYTES)});
        assert_eq!(refused_at(&with("payload", big)), "payload");
    }

    #[test]
    fn dedup_key_must_be_printable_and_bounded() {
        assert_eq!(refused_at(&with("dedup_key", json!(""))), "dedup_key");
        assert_eq!(refused_at(&with("dedup_key", json!("a\nb"))), "dedup_key");
        assert_eq!(refused_at(&with("dedup_key", json!("k".repeat(201)))), "dedup_key");
        assert!(parse(&with("dedup_key", json!("k".repeat(200)))).is_ok());
    }

    #[test]
    fn occurred_at_must_be_rfc3339_and_inside_the_window() {
        assert_eq!(refused_at(&with("occurred_at", json!("yesterday"))), "occurred_at");
        assert!(parse(&with("occurred_at", json!("2026-11-21T10:04:59Z"))).is_ok());
        assert!(parse(&with("occurred_at", json!("2026-11-14T10:00:01Z"))).is_ok());
        for out_of_range in ["2026-11-21T10:05:01Z", "2026-11-14T09:59:59Z"] {
            assert_eq!(
                parse(&with("occurred_at", json!(out_of_range))),
                Err(EnvelopeError::OccurredAtOutOfRange),
                "{out_of_range}"
            );
        }
    }

    #[test]
    fn the_body_must_be_a_json_object() {
        for body in [&b""[..], b"not json", b"[]", b"\"x\"", b"null"] {
            assert!(matches!(
                parse_envelope(body, now()),
                Err(EnvelopeError::Invalid { ref path, .. }) if path.is_empty()
            ));
        }
    }
}
