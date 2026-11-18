//! UTC instants stored as `PostgreSQL` `TIMESTAMPTZ` and exposed as RFC 3339 strings.

use std::fmt::{self, Display, Formatter};
use std::ops::{Add, AddAssign, Sub, SubAssign};

use sea_orm::DeriveValueType;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime, UtcOffset};
use time::{format_description::well_known::Rfc3339, formatting::Formattable};

#[derive(Clone, Copy, Debug, DeriveValueType, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[sea_orm(column_type = "TimestampWithTimeZone", array_type = "TimeDateTimeWithTimeZone")]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    pub const UNIX_EPOCH: Self = Self(OffsetDateTime::UNIX_EPOCH);

    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }

    pub const fn from_utc(value: OffsetDateTime) -> Self {
        Self(value.to_offset(UtcOffset::UTC))
    }

    pub const fn from_unix_timestamp(seconds: i64, nanoseconds: u32) -> Option<Self> {
        let nanos = seconds as i128 * 1_000_000_000 + nanoseconds as i128;
        match OffsetDateTime::from_unix_timestamp_nanos(nanos) {
            Ok(value) if nanoseconds < 1_000_000_000 => Some(Self(value)),
            _ => None,
        }
    }

    pub const fn from_unix_timestamp_millis(milliseconds: i64) -> Option<Self> {
        match OffsetDateTime::from_unix_timestamp_nanos(milliseconds as i128 * 1_000_000) {
            Ok(value) => Some(Self(value)),
            Err(_) => None,
        }
    }

    pub const fn date(self) -> Date {
        self.0.to_offset(UtcOffset::UTC).date()
    }

    pub const fn unix_timestamp(self) -> i64 {
        self.0.unix_timestamp()
    }

    pub const fn unix_timestamp_micros(self) -> i128 {
        self.0.unix_timestamp_nanos().div_euclid(1_000)
    }

    pub const fn unix_timestamp_millis(self) -> i128 {
        self.0.unix_timestamp_nanos().div_euclid(1_000_000)
    }

    pub const fn unix_timestamp_nanos(self) -> i128 {
        self.0.unix_timestamp_nanos()
    }

    pub fn parse_rfc3339(value: &str) -> Result<Self, time::error::Parse> {
        Self::parse_standard(value).map(|instant| Self::from_utc(instant.0))
    }

    fn parse_standard(value: &str) -> Result<Self, time::error::Parse> {
        // The time crate also accepts nonstandard separators. Keep the API's original grammar.
        if !matches!(value.as_bytes().get(10), Some(b'T' | b't' | b' ')) {
            return Err(time::error::ParseFromDescription::InvalidComponent("separator").into());
        }
        OffsetDateTime::parse(value, &Rfc3339).map(Self)
    }

    fn parse_wire(value: &str) -> Result<Self, time::error::Parse> {
        match Self::parse_standard(value) {
            Ok(instant) => Ok(instant),
            Err(error) if !value.starts_with(['+', '-']) => Err(error),
            // JSON historically supported signed ISO 8601 years outside RFC 3339's four digits.
            Err(_) => {
                if let Some(utc) = value.strip_suffix(['Z', 'z']) {
                    let date_time = PrimitiveDateTime::parse(
                        utc,
                        time::macros::format_description!(
                            version = 2,
                            "[year sign:mandatory]-[month]-[day]T[hour]:[minute]:[second][optional [.[subsecond]]]"
                        ),
                    )?;
                    Ok(Self(date_time.assume_utc()))
                } else {
                    OffsetDateTime::parse(
                        value,
                        time::macros::format_description!(
                            version = 2, "[year sign:mandatory]-[month]-[day]T[hour]:[minute]:[second][optional [.[subsecond]]][offset_hour sign:mandatory]:[offset_minute]"
                        ),
                    ).map(Self)
                }
            }
        }
    }

    pub fn format(self, format: &(impl Formattable + ?Sized)) -> Result<String, time::error::Format> {
        self.0.to_offset(UtcOffset::UTC).format(format)
    }

    pub fn to_rfc3339(self) -> String {
        let offset = self.0.offset();
        let sign = if offset.is_negative() { '-' } else { '+' };
        let hour = offset.whole_hours().unsigned_abs();
        let minute = offset.minutes_past_hour().unsigned_abs();
        format!("{}{sign}{hour:02}:{minute:02}", Self::calendar_fields(self.0))
    }

    fn calendar_fields(value: OffsetDateTime) -> String {
        // Keep the prior API's 0/3/6/9 fractional-digit representations, including trailing zeros.
        let (year, month, day) = value.to_calendar_date();
        let (hour, minute, second, nanos) = value.to_hms_nano();
        let year = if (0..=9999).contains(&year) {
            format!("{year:04}")
        } else {
            format!("{year:+05}")
        };
        let fields = format!("{year}-{:02}-{day:02}T{hour:02}:{minute:02}:{second:02}", month as u8);
        if nanos == 0 {
            fields
        } else if nanos % 1_000_000 == 0 {
            format!("{fields}.{:03}", nanos / 1_000_000)
        } else if nanos % 1_000 == 0 {
            format!("{fields}.{:06}", nanos / 1_000)
        } else {
            format!("{fields}.{nanos:09}")
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Self::parse_wire(&value)
            .map(|time| Self::from_utc(time.0))
            .map_err(serde::de::Error::custom)
    }
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}Z", Self::calendar_fields(self.0.to_offset(UtcOffset::UTC)))
    }
}

/// Wire adapter for models whose existing contract preserves offsets from UTC.
pub mod with_offset {
    use super::{Deserialize, Deserializer, Serializer, Timestamp};

    pub fn serialize<S: Serializer>(value: &Timestamp, serializer: S) -> Result<S::Ok, S::Error> {
        if value.0.offset().is_utc() {
            serializer.collect_str(value)
        } else {
            serializer.serialize_str(&value.to_rfc3339())
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Timestamp, D::Error> {
        let value = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Timestamp::parse_wire(&value).map_err(serde::de::Error::custom)
    }

    pub mod option {
        use super::{Deserialize, Deserializer, Serializer, Timestamp};

        pub fn serialize<S: Serializer>(value: &Option<Timestamp>, serializer: S) -> Result<S::Ok, S::Error> {
            match value {
                Some(value) if value.0.offset().is_utc() => serializer.serialize_some(value),
                Some(value) => serializer.serialize_some(&value.to_rfc3339()),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Timestamp>, D::Error> {
            Option::<std::borrow::Cow<'de, str>>::deserialize(deserializer)?
                .as_deref()
                .map(Timestamp::parse_wire)
                .transpose()
                .map_err(serde::de::Error::custom)
        }
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self {
        Self(self.0 + rhs)
    }
}

impl AddAssign<Duration> for Timestamp {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self {
        Self(self.0 - rhs)
    }
}

impl SubAssign<Duration> for Timestamp {
    fn sub_assign(&mut self, rhs: Duration) {
        *self = *self - rhs;
    }
}

impl Sub for Timestamp {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Duration {
        self.0 - rhs.0
    }
}

#[cfg(test)]
mod tests {
    use super::Timestamp;
    use serde_json::{from_value, json};

    #[test]
    fn fixed_offset_wire_adapter_retains_offsets_and_optional_values() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Record {
            #[serde(with = "super::with_offset")]
            required: Timestamp,
            #[serde(default, with = "super::with_offset::option")]
            optional: Option<Timestamp>,
        }

        let original = json!({
            "required": "2025-12-23T12:00:11.123400Z",
            "optional": "2025-12-23T04:30:00.010-03:30"
        });
        let record = from_value::<Record>(original.clone()).unwrap();
        assert_eq!(json!(record), original);

        let record = from_value::<Record>(json!({"required": "2025-12-23T12:00:11+00:00"})).unwrap();
        assert_eq!(
            json!(record),
            json!({"required": "2025-12-23T12:00:11Z", "optional": null})
        );
    }

    #[test]
    fn utc_timestamp_preserves_json_and_explicit_rfc3339_contracts() {
        for (input, utc) in [
            ("2025-12-23T17:00:11+05:00", "2025-12-23T12:00:11"),
            ("2025-12-23T12:00:11.001Z", "2025-12-23T12:00:11.001"),
            ("2025-12-23T12:00:11.123400Z", "2025-12-23T12:00:11.123400"),
            ("2025-12-23T12:00:11.123456789Z", "2025-12-23T12:00:11.123456789"),
            ("+10000-12-23T12:00:11.123400Z", "+10000-12-23T12:00:11.123400"),
            ("-0100-12-23T12:00:11.123400Z", "-0100-12-23T12:00:11.123400"),
        ] {
            let value: Timestamp = from_value(json!(input)).unwrap();
            assert_eq!(json!(value), json!(format!("{utc}Z")));
            assert_eq!(value.to_rfc3339(), format!("{utc}+00:00"));
        }
        assert_eq!(from_value::<Option<Timestamp>>(json!(null)).unwrap(), None);
        assert_eq!(json!(None::<Timestamp>), json!(null));
        assert!(Timestamp::parse_rfc3339("2025-12-23 12:00:11Z").is_ok());
        assert!(Timestamp::parse_rfc3339("2025-12-23t12:00:11z").is_ok());
        assert!(Timestamp::parse_rfc3339("2025-12-23x12:00:11Z").is_err());
    }
}
