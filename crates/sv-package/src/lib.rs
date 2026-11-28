//! Package format `sv.package.v1`: manifest types, canonical bytes (RFC 8785) with a
//! domain-separated Ed25519 signature, package digest, and the zip container with its guards.
//!
//! Flow for a consumer: [`unpack`] (structure, limits, file digests) then
//! [`UnverifiedPackage::verify`] with the registry's key lookup, which yields a
//! [`VerifiedPackage`]. Producers use [`pack`] (or the `sv-pack` command).

mod container;
mod error;
mod manifest;
mod sign;
mod strict;
mod zipcheck;

pub use container::{
    KeyLookup, Limits, MANIFEST_NAME, PackageData, SIGNATURE_NAME, UnverifiedPackage, VerifiedPackage,
    has_custom_frame, pack, unpack, validate_entry_path,
};
pub use error::{ErrorCode, PackageError, Result};
pub use manifest::{
    ActionEntry, Bind, Compat, Kind, LayoutEntry, Manifest, PageEntry, ProjectBlock, ProjectLabel, ProjectState,
    Publisher, Request, RuleEntry, SCHEMA_ID,
};
pub use sign::{
    SIGNING_DOMAIN, canonical_bytes, generate_pkcs8, hex, key_id, keypair_from_pkcs8, package_digest, public_key,
    sha256_digest, sign, verify,
};
pub use strict::parse_manifest;
