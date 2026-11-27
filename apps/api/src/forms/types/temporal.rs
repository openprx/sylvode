//! Date types. Values stay the strings clients send; rules and CEL read them as an ISO date
//! (`YYYY-MM-DD`) or a date-time (RFC 3339, or a zone-less `YYYY-MM-DDTHH:MM[:SS]` read as UTC, the
//! form a browser `datetime-local` input produces).

use serde_json::Value;
use sv_rules::{Bound, Operand, RuleKind};
use time::{
    Date as CalendarDate, OffsetDateTime, PrimitiveDateTime, format_description::well_known::Rfc3339,
    macros::format_description,
};

use super::{CelType, FieldType, ORDERED_RULES, StorageKind, TypeError};
use crate::forms::schema::FormField;

fn string_value(raw: &Value) -> Result<Value, TypeError> {
    raw.as_str()
        .map(|value| Value::String(value.to_string()))
        .ok_or(TypeError::Mismatch)
}

/// A strict `YYYY-MM-DD` date.
pub fn parse_iso_date(text: &str) -> Option<CalendarDate> {
    CalendarDate::parse(text.trim(), format_description!("[year]-[month]-[day]")).ok()
}

/// An RFC 3339 date-time, or a zone-less one read as UTC.
pub fn parse_date_time(text: &str) -> Option<OffsetDateTime> {
    let text = text.trim();
    if let Ok(value) = OffsetDateTime::parse(text, &Rfc3339) {
        return Some(value);
    }
    PrimitiveDateTime::parse(
        text,
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"),
    )
    .or_else(|_| PrimitiveDateTime::parse(text, format_description!("[year]-[month]-[day]T[hour]:[minute]")))
    .ok()
    .map(PrimitiveDateTime::assume_utc)
}

/// `date`.
pub struct Date;

impl FieldType for Date {
    fn name(&self) -> &'static str {
        "date"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::Date
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::String)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        ORDERED_RULES
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        string_value(raw)
    }
    fn accepts_bound(&self, bound: &Bound) -> bool {
        matches!(bound, Bound::Date(_))
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        value.as_str().and_then(parse_iso_date).map(Operand::Date)
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        value.as_str().map(|text| Value::String(text.to_string()))
    }
}

/// `datetime`; CEL sees Unix seconds.
pub struct DateTime;

impl FieldType for DateTime {
    fn name(&self) -> &'static str {
        "datetime"
    }
    fn storage(&self) -> StorageKind {
        StorageKind::DateTime
    }
    fn cel_type(&self) -> Option<CelType> {
        Some(CelType::Timestamp)
    }
    fn allowed_rules(&self) -> &'static [RuleKind] {
        ORDERED_RULES
    }
    fn normalize(&self, _field: &FormField, raw: &Value, _prior: Option<&Value>) -> Result<Value, TypeError> {
        string_value(raw)
    }
    fn accepts_bound(&self, bound: &Bound) -> bool {
        matches!(bound, Bound::DateTime(_))
    }
    fn operand(&self, _field: &FormField, value: &Value) -> Option<Operand> {
        value.as_str().and_then(parse_date_time).map(Operand::DateTime)
    }
    fn cel_value(&self, _field: &FormField, value: &Value) -> Option<Value> {
        value
            .as_str()
            .and_then(parse_date_time)
            .map(|moment| Value::from(moment.unix_timestamp()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_browser_and_rfc3339_date_times() {
        let local = parse_date_time("2026-01-02T10:30").map(OffsetDateTime::unix_timestamp);
        let zoned = parse_date_time("2026-01-02T10:30:00Z").map(OffsetDateTime::unix_timestamp);
        assert!(local.is_some());
        assert_eq!(local, zoned);
        assert!(parse_date_time("tomorrow").is_none());
        assert!(parse_iso_date("2026-02-30").is_none());
        assert!(parse_iso_date("2026-02-28").is_some());
    }
}
