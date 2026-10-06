//! Holds `scripts/lib/sylvode_config_schema.json` — the key table `scripts/start.sh
//! --check-config` validates against — equal to the keys the binaries actually accept.
//!
//! The script cannot run the Rust parser, so it carries its own copy of the key set, and that
//! copy drifted: it rejected `[flow]`, `[audit]` and `auth.allow_insecure_cookies`, all of which
//! the binaries accept, so `--check-config` refused valid files. This test derives the accepted
//! key set from the serde types themselves, not from a hand-written list: [`Recorder`] is a
//! deserializer that answers every `deserialize_struct` call by writing down the field names the
//! derive hands it and then walking into each field. A key added to (or removed from) any `Raw*`
//! struct changes what it records, and the comparison below fails until the JSON file says the
//! same.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde::de::value::{Error, SeqDeserializer, StrDeserializer};
use serde::de::{self, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, Visitor};
use serde::forward_to_deserialize_any;

use super::raw::RawConfig;

/// Field names per struct path: `""` is the file itself (its fields are the sections),
/// `"storage"` is `[storage]`, `"storage.s3"` is `[storage.s3]`.
type Fields = RefCell<BTreeMap<String, BTreeSet<String>>>;

struct Recorder<'a> {
    path: String,
    fields: &'a Fields,
}

impl<'de> Deserializer<'de> for Recorder<'_> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Error> {
        Err(de::Error::custom(format!(
            "{}: a config field type the recorder does not know; teach it before trusting this test",
            self.path
        )))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.fields
            .borrow_mut()
            .entry(self.path.clone())
            .or_default()
            .extend(fields.iter().map(|field| (*field).to_string()));
        visitor.visit_map(StructFields {
            path: self.path,
            remaining: fields.iter(),
            current: None,
            fields: self.fields,
        })
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_some(self)
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_bool(false)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_i32(1)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_i64(1)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_u32(1)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_u64(1)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_str("value")
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_str("value")
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_seq(SeqDeserializer::<_, Error>::new(std::iter::empty::<&str>()))
    }

    forward_to_deserialize_any! {
        i8 i16 i128 u8 u16 u128 f32 f64 char bytes byte_buf unit unit_struct newtype_struct
        tuple tuple_struct map enum identifier ignored_any
    }
}

/// Presents every field of one struct, each with a [`Recorder`] for its value.
struct StructFields<'a> {
    path: String,
    remaining: std::slice::Iter<'static, &'static str>,
    current: Option<&'static str>,
    fields: &'a Fields,
}

impl<'de> MapAccess<'de> for StructFields<'_> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        let Some(field) = self.remaining.next() else {
            return Ok(None);
        };
        self.current = Some(field);
        let key: StrDeserializer<'_, Error> = field.into_deserializer();
        seed.deserialize(key).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let field = self
            .current
            .take()
            .ok_or_else(|| <Error as de::Error>::custom("value requested before its key"))?;
        let path = if self.path.is_empty() {
            field.to_string()
        } else {
            format!("{}.{field}", self.path)
        };
        seed.deserialize(Recorder {
            path,
            fields: self.fields,
        })
    }
}

fn accepted_by_the_binaries() -> BTreeMap<String, BTreeSet<String>> {
    let fields = Fields::default();
    let parsed: Result<RawConfig, Error> = serde::Deserialize::deserialize(Recorder {
        path: String::new(),
        fields: &fields,
    });
    if let Err(error) = parsed {
        panic!("the recorder could not walk RawConfig: {error}");
    }
    fields.into_inner()
}

fn listed_by_the_script() -> BTreeMap<String, BTreeSet<String>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/lib/sylvode_config_schema.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let schema: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path} is not JSON: {error}"));
    let mut listed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let sections = schema
        .get("sections")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("{path} has no `sections` object"));
    for (section, keys) in sections {
        let keys = keys
            .as_array()
            .unwrap_or_else(|| panic!("{path}: sections.{section} is not an array"));
        let entry = listed.entry(section.clone()).or_default();
        for key in keys {
            entry.insert(
                key.as_str()
                    .unwrap_or_else(|| panic!("{path}: sections.{section} holds a non-string"))
                    .to_string(),
            );
        }
        let (parent, name) = section.rsplit_once('.').unwrap_or(("", section.as_str()));
        listed.entry(parent.to_string()).or_default().insert(name.to_string());
    }
    // A retired key is still a field of the Rust struct (so the binary can explain the removal
    // instead of reporting a bare unknown key) and the script reports it the same way.
    let retired = schema
        .get("retired")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("{path} has no `retired` object"));
    for dotted in retired.keys() {
        let (section, key) = dotted
            .rsplit_once('.')
            .unwrap_or_else(|| panic!("{path}: retired key {dotted} is not section.key"));
        listed.entry(section.to_string()).or_default().insert(key.to_string());
    }
    listed
}

#[test]
fn start_script_schema_lists_exactly_the_keys_the_binaries_accept() {
    let accepted = accepted_by_the_binaries();
    assert!(
        accepted
            .get("flow")
            .is_some_and(|keys| keys.contains("collab_allowed_origins")),
        "the recorder did not reach [flow]; it is not walking the real types: {accepted:?}"
    );
    assert_eq!(
        listed_by_the_script(),
        accepted,
        "scripts/lib/sylvode_config_schema.json disagrees with crates/platform/src/config/raw.rs \
         (left: the script's table, right: what the binaries accept)"
    );
}

/// The annotated example is the reference operators copy from: every section header in it,
/// commented or not, has to be a section the binaries accept, and every key under it a key of that
/// section. (This compares names; the values are covered by the configuration loader's tests.)
#[test]
fn every_key_in_the_example_configuration_is_known_to_the_script() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/sylvode.example.toml");
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let listed = listed_by_the_script();
    let mut section = String::new();
    let mut unknown = Vec::new();
    for line in text.lines() {
        let line = line.trim_start_matches('#').trim();
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
            {
                if !listed.contains_key(name) {
                    unknown.push(format!("[{name}]"));
                }
                name.clone_into(&mut section);
            }
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if section.is_empty() || key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue;
        }
        if !listed.get(&section).is_some_and(|keys| keys.contains(key)) {
            unknown.push(format!("{section}.{key}"));
        }
    }
    assert!(
        unknown.is_empty(),
        "config/sylvode.example.toml names keys the binaries would reject: {unknown:?}"
    );
}
