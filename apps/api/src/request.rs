//! Helpers for reading request bodies.

use serde::{Deserialize, Deserializer};

/// A key that is present with `null` clears the value; an absent key leaves it alone.
///
/// Use as `#[serde(default, deserialize_with = "nullable")]` on an `Option<Option<T>>` field: an
/// absent key is `None`, `null` is `Some(None)` and a value is `Some(Some(value))`.
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::nullable;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[allow(clippy::option_option)]
    struct Body {
        #[serde(default, deserialize_with = "nullable")]
        field: Option<Option<String>>,
    }

    #[test]
    fn absent_null_and_a_value_are_three_different_requests() {
        let absent: Body = serde_json::from_str("{}").expect("absent");
        assert_eq!(absent.field, None);
        let null: Body = serde_json::from_str(r#"{"field": null}"#).expect("null");
        assert_eq!(null.field, Some(None));
        let value: Body = serde_json::from_str(r#"{"field": "x"}"#).expect("value");
        assert_eq!(value.field, Some(Some("x".to_string())));
    }
}
