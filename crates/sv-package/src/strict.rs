//! Strict JSON parsing for manifests: duplicate keys refused, numbers must be integers within
//! +-2^53, no U+0000 in strings or keys (PostgreSQL `jsonb` cannot store it) (so every parser and the canonicaliser see the same document).

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::error::{ErrorCode, PackageError, Result};

const MAX_SAFE: i64 = 1 << 53;

struct Strict(Value);

struct V;

impl<'de> Visitor<'de> for V {
    type Value = Strict;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value without duplicate keys")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
        Ok(Strict(Value::Bool(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
        if !(-MAX_SAFE..=MAX_SAFE).contains(&v) {
            return Err(E::custom("integer outside +-2^53"));
        }
        Ok(Strict(Value::from(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
        if v > MAX_SAFE.unsigned_abs() {
            return Err(E::custom("integer outside +-2^53"));
        }
        Ok(Strict(Value::from(v)))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Strict, E> {
        Err(E::custom("numbers must be integers"))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
        if v.contains('\0') {
            return Err(E::custom("U+0000 in a string"));
        }
        Ok(Strict(Value::String(v.to_owned())))
    }

    fn visit_unit<E: de::Error>(self) -> std::result::Result<Strict, E> {
        Ok(Strict(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Strict, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Strict(Value::Array(out)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Strict, A::Error> {
        let mut out = Map::new();
        while let Some((k, Strict(v))) = map.next_entry::<String, Strict>()? {
            if k.contains('\0') {
                return Err(de::Error::custom("U+0000 in a key"));
            }
            if out.contains_key(&k) {
                return Err(de::Error::custom(format!("duplicate key `{k}`")));
            }
            out.insert(k, v);
        }
        Ok(Strict(Value::Object(out)))
    }
}

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(V)
    }
}

/// Parses manifest bytes strictly.
///
/// # Errors
/// `manifest_invalid` for syntax errors, duplicate keys, floats and out-of-range integers.
pub fn parse_manifest(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice::<Strict>(bytes)
        .map(|s| s.0)
        .map_err(|e| PackageError::new(ErrorCode::ManifestInvalid, format!("manifest JSON: {e}")))
}
