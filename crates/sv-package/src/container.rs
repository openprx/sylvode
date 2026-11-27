//! The zip container: pack with signing, unpack with every guard of C3 §4.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::ops::Deref;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::Ed25519KeyPair;
use serde_json::Value;
use zip::CompressionMethod;
use zip::ZipArchive;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::error::{ErrorCode, PackageError, Result};
use crate::manifest::Manifest;
use crate::sign;

/// Name of the manifest entry.
pub const MANIFEST_NAME: &str = "sv-package.json";
/// Name of the signature entry.
pub const SIGNATURE_NAME: &str = "SIGNATURE";
const MAX_PATH_LEN: usize = 200;
/// Entries smaller than this are exempt from the compression-ratio check (a tiny entry of
/// repeated bytes legitimately compresses far beyond 100:1).
const RATIO_MIN_BYTES: u64 = 64 * 1024;
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;

/// Size limits applied when unpacking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Zip file size.
    pub max_zip_bytes: u64,
    /// Sum of uncompressed sizes.
    pub max_total_bytes: u64,
    /// One uncompressed entry.
    pub max_entry_bytes: u64,
    /// Compression ratio.
    pub max_ratio: u64,
    /// Entry count.
    pub max_entries: usize,
    /// Manifest size.
    pub max_manifest_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        const MIB: u64 = 1024 * 1024;
        Self {
            max_zip_bytes: 32 * MIB,
            max_total_bytes: 64 * MIB,
            max_entry_bytes: 16 * MIB,
            max_ratio: 100,
            max_entries: 512,
            max_manifest_bytes: 256 * 1024,
        }
    }
}

fn err(code: ErrorCode, msg: impl Into<String>) -> PackageError {
    PackageError::new(code, msg)
}

/// Checks an entry or `files` path: relative, no `..` or empty segments, characters limited to
/// `[A-Za-z0-9._/-]` (which also excludes backslashes, drive letters, control characters and
/// anything not in NFC), at most 200 bytes.
///
/// # Errors
/// `path_invalid`.
pub fn validate_entry_path(path: &str) -> Result<()> {
    let bad = |m: &str| Err(err(ErrorCode::PathInvalid, format!("`{path}`: {m}")));
    if path.is_empty() || path.len() > MAX_PATH_LEN {
        return bad("empty or longer than 200 bytes");
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
    {
        return bad("characters outside [A-Za-z0-9._/-]");
    }
    if path.starts_with('/') {
        return bad("absolute path");
    }
    if path.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return bad("empty, `.` or `..` segment");
    }
    Ok(())
}

/// The parsed contents of a package, shared by the unverified and verified forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageData {
    /// Typed manifest.
    pub manifest: Manifest,
    /// The manifest as parsed JSON (what was canonicalised).
    pub manifest_value: Value,
    /// Canonical manifest bytes `M`.
    pub canonical: Vec<u8>,
    /// The 64-byte signature.
    pub signature: Vec<u8>,
    /// Content files by path (all listed in the manifest, digests checked).
    pub files: BTreeMap<String, Vec<u8>>,
    /// Package digest: `sha256:` + hex(sha256(M)).
    pub digest: String,
    /// Digest of the zip bytes.
    pub blob_digest: String,
}

/// A package whose structure and file digests are checked but whose signature is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedPackage(PackageData);

/// A package whose signature verified against a registered, active key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPackage(PackageData);

impl Deref for UnverifiedPackage {
    type Target = PackageData;
    fn deref(&self) -> &PackageData {
        &self.0
    }
}

impl Deref for VerifiedPackage {
    type Target = PackageData;
    fn deref(&self) -> &PackageData {
        &self.0
    }
}

/// What the registry knows about a key id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyLookup {
    /// Active key with its raw 32-byte public key.
    Active(Vec<u8>),
    /// Revoked key.
    Revoked,
    /// Not registered.
    Unknown,
}

impl UnverifiedPackage {
    /// Verifies the signature with the key the registry returns for the manifest's `key_id`.
    ///
    /// # Errors
    /// `key_unknown`, `key_revoked` or `signature_invalid`.
    pub fn verify(self, lookup: impl FnOnce(&str) -> KeyLookup) -> Result<VerifiedPackage> {
        let key_id = &self.0.manifest.publisher.key_id;
        match lookup(key_id) {
            KeyLookup::Unknown => Err(err(ErrorCode::KeyUnknown, format!("no key {key_id}"))),
            KeyLookup::Revoked => Err(err(ErrorCode::KeyRevoked, format!("key {key_id} is revoked"))),
            KeyLookup::Active(public) => {
                if sign::key_id(&public) != *key_id {
                    return Err(err(ErrorCode::SignatureInvalid, "key does not match key_id"));
                }
                sign::verify(&self.0.canonical, &self.0.signature, &public)?;
                Ok(VerifiedPackage(self.0))
            }
        }
    }

    /// Checks the signature against one known public key (for example the built-in key).
    ///
    /// # Errors
    /// As [`UnverifiedPackage::verify`].
    pub fn verify_with_key(self, public_key: &[u8]) -> Result<VerifiedPackage> {
        let known = public_key.to_vec();
        let id = sign::key_id(&known);
        self.verify(|k| {
            if k == id {
                KeyLookup::Active(known)
            } else {
                KeyLookup::Unknown
            }
        })
    }
}

fn zip_err(e: &zip::result::ZipError) -> PackageError {
    err(ErrorCode::ZipInvalid, format!("zip: {e}"))
}

fn read_limited<R: Read>(mut r: R, limit: u64, what: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let n = r
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| err(ErrorCode::ZipInvalid, format!("{what}: {e}")))?;
    if n as u64 > limit {
        return Err(err(ErrorCode::TooLarge, format!("{what} exceeds {limit} bytes")));
    }
    Ok(out)
}

struct EntryInfo {
    index: usize,
    name: String,
}

/// Unpacks and structurally verifies a package. The signature is not checked; call
/// [`UnverifiedPackage::verify`]. Nothing is written to disk.
///
/// # Errors
/// Any [`ErrorCode`] except the key and signature ones.
pub fn unpack(bytes: &[u8], limits: &Limits) -> Result<UnverifiedPackage> {
    if bytes.len() as u64 > limits.max_zip_bytes {
        return Err(err(ErrorCode::TooLarge, "zip larger than the limit"));
    }
    crate::zipcheck::check(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| zip_err(&e))?;
    if archive.has_duplicate_names() {
        return Err(err(ErrorCode::PathInvalid, "duplicate entry names"));
    }
    if archive.len() > limits.max_entries {
        return Err(err(ErrorCode::TooManyEntries, "too many entries"));
    }
    let entries = scan_entries(&mut archive, limits)?;
    let find = |name: &str| entries.iter().find(|e| e.name == name);
    let manifest_idx = find(MANIFEST_NAME)
        .ok_or_else(|| err(ErrorCode::FileMissing, "sv-package.json is missing"))?
        .index;
    let sig_idx = find(SIGNATURE_NAME)
        .ok_or_else(|| err(ErrorCode::FileMissing, "SIGNATURE is missing"))?
        .index;

    let mut total: u64 = 0;
    let manifest_bytes = {
        let f = archive.by_index(manifest_idx).map_err(|e| zip_err(&e))?;
        read_limited(f, limits.max_manifest_bytes, MANIFEST_NAME)?
    };
    total += manifest_bytes.len() as u64;
    let sig_text = {
        let f = archive.by_index(sig_idx).map_err(|e| zip_err(&e))?;
        read_limited(f, 256, SIGNATURE_NAME)?
    };
    total += sig_text.len() as u64;

    let manifest_value = crate::strict::parse_manifest(&manifest_bytes)?;
    let manifest: Manifest = serde_json::from_value(manifest_value.clone())
        .map_err(|e| err(ErrorCode::ManifestInvalid, format!("manifest shape: {e}")))?;
    manifest.validate()?;
    let canonical = sign::canonical_bytes(&manifest_value)?;
    let signature = decode_signature(&sig_text)?;

    let listed: BTreeSet<&str> = manifest.files.keys().map(String::as_str).collect();
    for e in &entries {
        if e.name != MANIFEST_NAME && e.name != SIGNATURE_NAME && !listed.contains(e.name.as_str()) {
            return Err(err(ErrorCode::FileUnlisted, format!("`{}` is not in files", e.name)));
        }
    }
    let present: BTreeSet<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    if let Some(missing) = listed.iter().find(|p| !present.contains(**p)) {
        return Err(err(ErrorCode::FileMissing, format!("`{missing}` is listed but absent")));
    }

    let mut files = BTreeMap::new();
    for e in entries.iter().filter(|e| listed.contains(e.name.as_str())) {
        let (data, compressed) = {
            let f = archive.by_index(e.index).map_err(|er| zip_err(&er))?;
            let compressed = f.compressed_size();
            (read_limited(f, limits.max_entry_bytes, &e.name)?, compressed)
        };
        let size = data.len() as u64;
        total += size;
        if total > limits.max_total_bytes {
            return Err(err(ErrorCode::TooLarge, "uncompressed total exceeds the limit"));
        }
        if size >= RATIO_MIN_BYTES && size / compressed.max(1) > limits.max_ratio {
            return Err(err(
                ErrorCode::TooLarge,
                format!("`{}` compression ratio too high", e.name),
            ));
        }
        let want = manifest.files.get(&e.name).map_or("", String::as_str);
        if sign::sha256_digest(&data) != want {
            return Err(err(
                ErrorCode::DigestMismatch,
                format!("`{}` does not match its digest", e.name),
            ));
        }
        files.insert(e.name.clone(), data);
    }

    let digest = sign::package_digest(&canonical);
    Ok(UnverifiedPackage(PackageData {
        manifest,
        manifest_value,
        canonical,
        signature,
        files,
        digest,
        blob_digest: sign::sha256_digest(bytes),
    }))
}

fn decode_signature(text: &[u8]) -> Result<Vec<u8>> {
    let bad = || err(ErrorCode::SignatureInvalid, "SIGNATURE is not one line of base64url");
    let s = std::str::from_utf8(text).map_err(|_| bad())?;
    let s = s.strip_suffix("\r\n").or_else(|| s.strip_suffix('\n')).unwrap_or(s);
    let raw = URL_SAFE_NO_PAD.decode(s).map_err(|_| bad())?;
    if raw.len() == 64 { Ok(raw) } else { Err(bad()) }
}

/// First pass over the central directory: names, types and declared sizes, no decompression.
fn scan_entries(archive: &mut ZipArchive<Cursor<&[u8]>>, limits: &Limits) -> Result<Vec<EntryInfo>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut declared: u64 = 0;
    for index in 0..archive.len() {
        let f = archive.by_index_raw(index).map_err(|e| zip_err(&e))?;
        let name = std::str::from_utf8(f.name_raw())
            .map_err(|_| err(ErrorCode::PathInvalid, "entry name is not UTF-8"))?
            .to_owned();
        validate_entry_path(&name)?;
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(err(
                ErrorCode::PathInvalid,
                format!("`{name}` duplicates another entry"),
            ));
        }
        if f.is_dir() || f.is_symlink() {
            return Err(err(ErrorCode::EntryType, format!("`{name}` is not a regular file")));
        }
        if let Some(mode) = f.unix_mode() {
            if mode & S_IFMT != S_IFREG && mode & S_IFMT != 0 {
                return Err(err(ErrorCode::EntryType, format!("`{name}` is not a regular file")));
            }
        }
        if f.encrypted() {
            return Err(err(ErrorCode::EntryType, format!("`{name}` is encrypted")));
        }
        if !matches!(f.compression(), CompressionMethod::Stored | CompressionMethod::Deflated) {
            return Err(err(
                ErrorCode::EntryType,
                format!("`{name}` uses an unsupported method"),
            ));
        }
        let size = f.size();
        if size > limits.max_entry_bytes {
            return Err(err(ErrorCode::TooLarge, format!("`{name}` exceeds the entry limit")));
        }
        declared = declared.saturating_add(size);
        if declared > limits.max_total_bytes {
            return Err(err(
                ErrorCode::TooLarge,
                "declared uncompressed total exceeds the limit",
            ));
        }
        if size >= RATIO_MIN_BYTES && size / f.compressed_size().max(1) > limits.max_ratio {
            return Err(err(ErrorCode::TooLarge, format!("`{name}` compression ratio too high")));
        }
        out.push(EntryInfo { index, name });
    }
    Ok(out)
}

/// Builds and signs a package.
///
/// `manifest` is the manifest as JSON; its `files` map and `publisher.key_id` are filled in from
/// `contents` and `key`. The result is unpacked again with the default limits so the packer never
/// emits what the store would refuse.
///
/// # Errors
/// `manifest_invalid`, `path_invalid`, `too_large`, `archive_invalid`.
pub fn pack(mut manifest: Value, contents: &BTreeMap<String, Vec<u8>>, key: &Ed25519KeyPair) -> Result<Vec<u8>> {
    use ring::signature::KeyPair;
    let digests: serde_json::Map<String, Value> = contents
        .iter()
        .map(|(p, d)| (p.clone(), Value::String(sign::sha256_digest(d))))
        .collect();
    let obj = manifest
        .as_object_mut()
        .ok_or_else(|| err(ErrorCode::ManifestInvalid, "manifest must be an object"))?;
    obj.insert("files".into(), Value::Object(digests));
    let key_id = sign::key_id(key.public_key().as_ref());
    match obj.get_mut("publisher").and_then(Value::as_object_mut) {
        Some(p) => {
            p.insert("key_id".into(), Value::String(key_id));
        }
        None => return Err(err(ErrorCode::ManifestInvalid, "publisher is required")),
    }
    let typed: Manifest = serde_json::from_value(manifest.clone())
        .map_err(|e| err(ErrorCode::ManifestInvalid, format!("manifest shape: {e}")))?;
    typed.validate()?;
    let canonical = sign::canonical_bytes(&manifest)?;
    let signature = sign::sign(&canonical, key);
    let manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|e| err(ErrorCode::ManifestInvalid, format!("manifest: {e}")))?;

    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut put = |name: &str, data: &[u8]| -> Result<()> {
        zip.start_file(name, opts).map_err(|e| zip_err(&e))?;
        zip.write_all(data)
            .map_err(|e| err(ErrorCode::ZipInvalid, format!("{name}: {e}")))
    };
    put(MANIFEST_NAME, &manifest_bytes)?;
    put(
        SIGNATURE_NAME,
        format!("{}\n", URL_SAFE_NO_PAD.encode(&signature)).as_bytes(),
    )?;
    for (path, data) in contents {
        put(path, data)?;
    }
    let bytes = zip.finish().map_err(|e| zip_err(&e))?.into_inner();
    unpack(&bytes, &Limits::default())?;
    Ok(bytes)
}
