//! Canonical bytes, Ed25519 signing and verification, digests.

use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{ErrorCode, PackageError, Result};

/// Domain-separation prefix prepended to the canonical manifest before signing.
pub const SIGNING_DOMAIN: &[u8] = b"sv.package.v1\0";

/// Lower-case hex of a byte slice.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
    }
    out
}

/// `sha256:` + hex of the SHA-256 of `bytes`.
#[must_use]
pub fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex(&Sha256::digest(bytes)))
}

/// Canonical manifest bytes `M` (RFC 8785) of the manifest as parsed JSON.
///
/// # Errors
/// `manifest_invalid` when the value cannot be canonicalised.
pub fn canonical_bytes(manifest: &Value) -> Result<Vec<u8>> {
    serde_jcs::to_vec(manifest)
        .map_err(|e| PackageError::new(ErrorCode::ManifestInvalid, format!("not canonicalisable: {e}")))
}

/// The package digest: `sha256:` + hex(sha256(M)).
#[must_use]
pub fn package_digest(canonical: &[u8]) -> String {
    sha256_digest(canonical)
}

/// `ed25519:` + the first 16 hex characters of `sha256(public key)`.
#[must_use]
pub fn key_id(public_key: &[u8]) -> String {
    let h = hex(&Sha256::digest(public_key));
    format!("ed25519:{}", h.get(..16).unwrap_or(&h))
}

fn signing_input(canonical: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(SIGNING_DOMAIN.len() + canonical.len());
    m.extend_from_slice(SIGNING_DOMAIN);
    m.extend_from_slice(canonical);
    m
}

/// Generates a new Ed25519 key pair as PKCS#8 bytes (secret material: never log or print).
///
/// # Errors
/// `key_invalid` when the system random source fails.
pub fn generate_pkcs8() -> Result<Vec<u8>> {
    Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map(|d| d.as_ref().to_vec())
        .map_err(|_| PackageError::new(ErrorCode::KeyInvalid, "key generation failed"))
}

/// Parses a PKCS#8 Ed25519 key pair.
///
/// # Errors
/// `key_invalid` for malformed key material.
pub fn keypair_from_pkcs8(pkcs8: &[u8]) -> Result<Ed25519KeyPair> {
    Ed25519KeyPair::from_pkcs8(pkcs8)
        .map_err(|_| PackageError::new(ErrorCode::KeyInvalid, "not a valid Ed25519 PKCS#8 key"))
}

/// The raw 32-byte public key of a key pair.
#[must_use]
pub fn public_key(pair: &Ed25519KeyPair) -> Vec<u8> {
    pair.public_key().as_ref().to_vec()
}

/// Signs the canonical manifest bytes (with the domain prefix); returns the 64-byte signature.
#[must_use]
pub fn sign(canonical: &[u8], pair: &Ed25519KeyPair) -> Vec<u8> {
    pair.sign(&signing_input(canonical)).as_ref().to_vec()
}

/// Verifies a signature over the canonical manifest bytes.
///
/// # Errors
/// `signature_invalid` when it does not verify.
pub fn verify(canonical: &[u8], signature: &[u8], public_key: &[u8]) -> Result<()> {
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&signing_input(canonical), signature)
        .map_err(|_| PackageError::new(ErrorCode::SignatureInvalid, "signature does not verify"))
}
