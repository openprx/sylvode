//! Reading documents from bytes: size limit, JSON, typed decoding with JSON paths, unknown-member
//! detection (strict mode) and semantic validation.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_path_to_error::Segment;

use crate::error::{Issues, ValidationError, element, member};
use crate::limits;
use crate::model::{IndexDoc, IndexMetadata, PluginDoc, PublisherDoc};
use crate::rules::{AnyHost, HostPolicy};
use crate::validate;

/// How unknown members are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// The index repository's CI: an unknown member is a violation.
    #[default]
    Strict,
    /// Sylvode reading a fetched index: unknown members are ignored.
    Lenient,
}

/// Parse and validation options.
#[derive(Clone, Copy)]
pub struct Options<'a> {
    /// Unknown-member handling.
    pub mode: Mode,
    /// Which download hosts are acceptable.
    pub hosts: &'a dyn HostPolicy,
}

impl Options<'_> {
    /// Strict mode, any public https host.
    #[must_use]
    pub const fn strict() -> Options<'static> {
        Options {
            mode: Mode::Strict,
            hosts: &AnyHost,
        }
    }

    /// Lenient mode, any public https host.
    #[must_use]
    pub const fn lenient() -> Options<'static> {
        Options {
            mode: Mode::Lenient,
            hosts: &AnyHost,
        }
    }
}

impl std::fmt::Debug for Options<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

fn one(path: &str, message: impl Into<String>) -> ValidationError {
    let mut issues = Issues::new();
    issues.push(path, message);
    ValidationError {
        issues: issues.finish(),
    }
}

/// `$.a[3].b` from a `serde_path_to_error` path.
fn json_path(path: &serde_path_to_error::Path) -> String {
    let mut out = "$".to_owned();
    for seg in path {
        match seg {
            Segment::Seq { index } => out = element(&out, *index),
            Segment::Map { key } => out = member(&out, key),
            Segment::Enum { variant } => out = member(&out, variant),
            Segment::Unknown => {}
        }
    }
    out
}

/// Members present in `original` but not in the re-serialised typed document: unknown to the schema.
fn unknown_members(issues: &mut Issues, path: &str, original: &Value, known: &Value) {
    match (original, known) {
        (Value::Object(o), Value::Object(k)) => {
            for (name, value) in o {
                let p = member(path, name);
                match k.get(name) {
                    Some(kv) => unknown_members(issues, &p, value, kv),
                    None if value.is_null() => issues.push(&p, "null is not allowed; omit the member"),
                    None => issues.push(&p, "unknown member"),
                }
            }
        }
        (Value::Array(o), Value::Array(k)) => {
            for (i, (ov, kv)) in o.iter().zip(k).enumerate() {
                unknown_members(issues, &element(path, i), ov, kv);
            }
        }
        _ => {}
    }
}

fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8], mode: Mode) -> Result<T, ValidationError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| one("$", format!("not valid JSON: {e}")))?;
    let doc: T =
        serde_path_to_error::deserialize(&value).map_err(|e| one(&json_path(e.path()), e.inner().to_string()))?;
    if mode == Mode::Strict {
        let mut names = Issues::new();
        crate::keys::check_member_names(bytes, &mut names);
        names.into_result()?;
        let known = serde_json::to_value(&doc).map_err(|e| one("$", format!("cannot re-encode: {e}")))?;
        let mut issues = Issues::new();
        unknown_members(&mut issues, "$", &value, &known);
        issues.into_result()?;
    }
    Ok(doc)
}

/// Parses and fully validates `index.json`: at most 8 MiB, schema, every rule, and in strict mode no
/// unknown members.
pub fn parse_index(bytes: &[u8], opts: &Options<'_>) -> Result<IndexDoc, ValidationError> {
    if bytes.len() as u64 > limits::MAX_INDEX_BYTES {
        return Err(one(
            "$",
            format!(
                "index is {} bytes, at most {} allowed",
                bytes.len(),
                limits::MAX_INDEX_BYTES
            ),
        ));
    }
    let doc: IndexDoc = decode(bytes, opts.mode)?;
    validate::validate_index(&doc, opts.hosts).into_result()?;
    Ok(doc)
}

/// Parses and validates `publishers/<id>.json`.
pub fn parse_publisher(bytes: &[u8], opts: &Options<'_>) -> Result<PublisherDoc, ValidationError> {
    check_file_size(bytes)?;
    let doc: PublisherDoc = decode(bytes, opts.mode)?;
    validate::validate_publisher(&doc).into_result()?;
    Ok(doc)
}

/// Parses and validates `plugins/<publisher>/<name>/plugin.json`; `declared` is the index metadata
/// the category and labels must appear in (skipped when `None`).
pub fn parse_plugin(
    bytes: &[u8],
    declared: Option<&IndexMetadata>,
    opts: &Options<'_>,
) -> Result<PluginDoc, ValidationError> {
    check_file_size(bytes)?;
    let doc: PluginDoc = decode(bytes, opts.mode)?;
    validate::validate_plugin(&doc, declared, opts.hosts).into_result()?;
    Ok(doc)
}

fn check_file_size(bytes: &[u8]) -> Result<(), ValidationError> {
    if bytes.len() as u64 > limits::MAX_REPO_FILE_BYTES {
        return Err(one(
            "$",
            format!(
                "file is {} bytes, at most {} allowed",
                bytes.len(),
                limits::MAX_REPO_FILE_BYTES
            ),
        ));
    }
    Ok(())
}
