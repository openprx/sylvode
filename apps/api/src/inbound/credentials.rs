//! Minting of the two secrets of an inbound source.
//!
//! The source token (`svi_` plus 64 hex characters) is stored only as a SHA-256 hash and shown
//! once. The signing secret (`svs_` plus 64 hex characters) is stored as text because the server
//! needs the key to verify a signature, like `webhooks.secret`; it is never returned after the
//! response that creates or rotates it and never logged.

use rand::RngCore;
use sha2::{Digest, Sha256};

/// Prefix of a source token.
pub const TOKEN_PREFIX: &str = "svi_";
/// Prefix of a signing secret.
pub const SIGNING_SECRET_PREFIX: &str = "svs_";
const RANDOM_BYTES: usize = 32;
/// Length of a source token: the prefix and 64 hex characters.
pub const TOKEN_LEN: usize = TOKEN_PREFIX.len() + RANDOM_BYTES * 2;
/// Characters of a token kept for display: the prefix and 8 hex characters.
const DISPLAY_PREFIX_LEN: usize = TOKEN_PREFIX.len() + 8;

/// The operating system's random source could not supply bytes. No secret is derived from
/// anything else, so the caller must fail the request.
#[derive(Debug, thiserror::Error)]
#[error("the operating system random source is unavailable")]
pub struct EntropyUnavailable;

/// A freshly minted source token, its hash and its display prefix.
pub struct MintedToken {
    pub raw: String,
    pub hash: String,
    pub display_prefix: String,
}

fn random_hex() -> Result<String, EntropyUnavailable> {
    let mut bytes = [0_u8; RANDOM_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| EntropyUnavailable)?;
    Ok(hex::encode(bytes))
}

/// SHA-256 of a source token as 64 lowercase hex characters, the form stored and looked up.
pub fn hash_token(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}

/// Mints a source token.
pub fn mint_token() -> Result<MintedToken, EntropyUnavailable> {
    let raw = format!("{TOKEN_PREFIX}{}", random_hex()?);
    let hash = hash_token(&raw);
    let display_prefix = raw.chars().take(DISPLAY_PREFIX_LEN).collect();
    Ok(MintedToken {
        raw,
        hash,
        display_prefix,
    })
}

/// Mints a signing secret.
pub fn mint_signing_secret() -> Result<String, EntropyUnavailable> {
    Ok(format!("{SIGNING_SECRET_PREFIX}{}", random_hex()?))
}

/// Whether `raw` has exactly the shape [`mint_token`] produces, so a value of any other shape is
/// refused before it reaches the database.
pub fn is_token_shape(raw: &str) -> bool {
    raw.len() == TOKEN_LEN
        && raw.strip_prefix(TOKEN_PREFIX).is_some_and(|rest| {
            rest.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_token_has_the_documented_shape_and_hash() {
        let minted = mint_token().expect("entropy");
        assert!(is_token_shape(&minted.raw));
        assert_eq!(minted.raw.len(), 68);
        assert_eq!(minted.hash, hash_token(&minted.raw));
        assert_eq!(minted.hash.len(), 64);
        assert_eq!(minted.display_prefix.len(), 12);
        assert!(minted.raw.starts_with(&minted.display_prefix));
        assert_ne!(minted.raw, mint_token().expect("entropy").raw);
    }

    #[test]
    fn shapes_other_than_a_token_are_refused() {
        assert!(!is_token_shape(""));
        assert!(!is_token_shape("svi_"));
        assert!(!is_token_shape(&format!("svi_{}", "A".repeat(64))));
        assert!(!is_token_shape(&format!("opr_{}", "a".repeat(64))));
        assert!(!is_token_shape(&format!("svi_{}", "a".repeat(63))));
        assert!(!is_token_shape(&format!("svi_{}g", "a".repeat(63))));
        assert!(is_token_shape(&format!("svi_{}", "a".repeat(64))));
    }

    #[test]
    fn a_signing_secret_is_prefixed_and_distinct_from_the_token() {
        let secret = mint_signing_secret().expect("entropy");
        assert!(secret.starts_with("svs_"));
        assert_eq!(secret.len(), 68);
        assert!(!is_token_shape(&secret));
    }
}
