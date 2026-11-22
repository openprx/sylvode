//! Minting, hashing and comparing the secrets of the authorization server.
//!
//! Every secret is 32 bytes from the operating system's random source, rendered as 64 lowercase
//! hex characters behind a fixed prefix. Only the SHA-256 of the whole string is stored. The
//! secret itself exists in the response that returns it and in the client, nowhere else.
//!
//! The types that carry a secret ([`RawToken`], [`TokenHash`], [`MintedToken`]) print as
//! `[REDACTED]` in `Debug`, implement neither `Display` nor `Serialize`, and expose the text only
//! through a method whose name says so, so a stray `{}` or `{:?}` in a log line does not compile
//! or does not leak.

use std::fmt;

use platform::config::OAuthConfig;
use platform::timestamp::Timestamp;
use rand::RngCore;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::Duration;

/// Entropy of every secret, in bytes.
pub const RANDOM_BYTES: usize = 32;
/// Length of the hex part of every secret.
pub const HEX_LEN: usize = RANDOM_BYTES * 2;
/// Length of a stored hash (SHA-256 as hex).
pub const HASH_LEN: usize = 64;

const REDACTED: &str = "[REDACTED]";

/// The kinds of secret the server issues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// `svc_`, single use, 60 seconds by default.
    AuthorizationCode,
    /// `sva_`, bound to one resource.
    Access,
    /// `svr_`, rotated on every use.
    Refresh,
    /// `sve_`, single use, 60 seconds.
    EmbedTicket,
    /// 64 hex characters without a prefix, single use, bound to an authorization request.
    Consent,
}

impl TokenKind {
    /// The kinds that carry a prefix, in the order [`kind_of`] tests them.
    pub const PREFIXED: [Self; 4] = [Self::AuthorizationCode, Self::Access, Self::Refresh, Self::EmbedTicket];

    pub const fn prefix(self) -> &'static str {
        match self {
            Self::AuthorizationCode => "svc_",
            Self::Access => "sva_",
            Self::Refresh => "svr_",
            Self::EmbedTicket => "sve_",
            Self::Consent => "",
        }
    }
}

/// Why a secret could not be minted.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenError {
    /// The operating system could not supply random bytes. No secret is ever derived from
    /// anything else, so the caller must fail the request.
    #[error("the operating system random source is unavailable")]
    EntropyUnavailable,
}

/// A freshly minted secret, in the form handed to the client.
pub struct RawToken(String);

impl RawToken {
    /// The secret text. Every call site is a deliberate decision to put the secret on the wire.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RawToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

/// SHA-256 of a secret, as 64 lowercase hex characters. A hash of a 256-bit random value is not
/// recoverable, but it is still the lookup key of the live credential, so it is kept out of logs
/// the same way.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenHash(String);

impl TokenHash {
    /// The hex text, for binding into a statement.
    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

/// A secret and the hash to store for it.
#[derive(Debug)]
pub struct MintedToken {
    pub raw: RawToken,
    pub hash: TokenHash,
}

/// Mints a secret of `kind` from the operating system's random source.
pub fn mint(kind: TokenKind) -> Result<MintedToken, TokenError> {
    let mut bytes = [0_u8; RANDOM_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| TokenError::EntropyUnavailable)?;
    let raw = format!("{}{}", kind.prefix(), hex::encode(bytes));
    let hash = hash_secret(&raw);
    Ok(MintedToken {
        raw: RawToken(raw),
        hash,
    })
}

/// SHA-256 of `raw`, as stored in the `*_hash` columns.
pub fn hash_secret(raw: &str) -> TokenHash {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    TokenHash(hex::encode(hasher.finalize()))
}

/// The kind a presented value claims by its prefix. A value without a known prefix is `None`;
/// consent tokens have no prefix and are never recognised here.
pub fn kind_of(raw: &str) -> Option<TokenKind> {
    TokenKind::PREFIXED
        .into_iter()
        .find(|kind| raw.starts_with(kind.prefix()))
}

/// Whether `raw` is exactly the shape [`mint`] produces for `kind`.
///
/// That is the prefix, then 64 lowercase hex characters. A value that fails this can not be a secret of ours, so it is refused before
/// any database work.
pub fn is_well_formed(kind: TokenKind, raw: &str) -> bool {
    raw.strip_prefix(kind.prefix())
        .is_some_and(|rest| rest.len() == HEX_LEN && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// Constant-time equality of two secrets. The lengths of ours are public (fixed formats), so a
/// length mismatch returns early; the contents are compared without an early exit.
pub fn secrets_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let difference = a
        .iter()
        .zip(b)
        .fold(0_u8, |accumulated, (left, right)| accumulated | (left ^ right));
    // `black_box` keeps the optimiser from turning the fold back into an early-exit comparison.
    std::hint::black_box(difference) == 0
}

/// How long each artefact lives, resolved from configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifetimes {
    pub access: Duration,
    pub code: Duration,
    pub refresh_idle: Duration,
    pub grant_max: Duration,
}

impl Lifetimes {
    pub const fn from_config(config: &OAuthConfig) -> Self {
        Self {
            access: Duration::seconds(config.access_token_ttl_seconds),
            code: Duration::seconds(config.authorization_code_ttl_seconds),
            refresh_idle: Duration::days(config.refresh_token_idle_days),
            grant_max: Duration::days(config.grant_max_lifetime_days),
        }
    }

    /// A grant lasts at most `grant_max` from the moment of consent.
    pub fn grant_expires_at(&self, consented_at: Timestamp) -> Timestamp {
        consented_at + self.grant_max
    }

    pub fn code_expires_at(&self, now: Timestamp) -> Timestamp {
        now + self.code
    }

    /// An access token never outlives its grant.
    pub fn access_expires_at(&self, now: Timestamp, grant_expires_at: Timestamp) -> Timestamp {
        (now + self.access).min(grant_expires_at)
    }

    /// A refresh token expires after the idle period, reset by every rotation, and never past the
    /// grant's own end.
    pub fn refresh_expires_at(&self, now: Timestamp, grant_expires_at: Timestamp) -> Timestamp {
        (now + self.refresh_idle).min(grant_expires_at)
    }
}

/// Whether something that expires at `expires_at` is no longer usable at `now`. The instant of
/// expiry itself is already expired, which matches the `expires_at > now()` test in SQL.
pub fn is_expired(expires_at: Timestamp, now: Timestamp) -> bool {
    expires_at <= now
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_unix_timestamp(seconds, 0).expect("valid instant")
    }

    #[test]
    fn minted_secrets_have_the_documented_shape() {
        for (kind, prefix) in [
            (TokenKind::AuthorizationCode, "svc_"),
            (TokenKind::Access, "sva_"),
            (TokenKind::Refresh, "svr_"),
            (TokenKind::EmbedTicket, "sve_"),
            (TokenKind::Consent, ""),
        ] {
            let minted = mint(kind).expect("mint");
            let raw = minted.raw.expose();
            assert_eq!(kind.prefix(), prefix);
            assert!(raw.starts_with(prefix));
            assert_eq!(raw.len(), prefix.len() + 64, "{kind:?}");
            assert!(
                raw[prefix.len()..]
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            );
            assert!(is_well_formed(kind, raw));
            assert_eq!(minted.hash, hash_secret(raw));
            assert_eq!(minted.hash.as_hex().len(), HASH_LEN);
            assert_ne!(minted.hash.as_hex(), raw);
        }
    }

    #[test]
    fn no_prefix_can_enter_the_bot_or_jwt_branches() {
        for kind in TokenKind::PREFIXED {
            let raw = mint(kind).expect("mint");
            assert!(!raw.raw.expose().starts_with("opr_"));
            assert!(!raw.raw.expose().contains('.'), "a JWT has dots, ours never do");
        }
    }

    #[test]
    fn secrets_are_unique_and_drawn_from_the_whole_byte_range() {
        let mut seen = HashSet::new();
        let mut nibbles = HashSet::new();
        for _ in 0..200 {
            let minted = mint(TokenKind::Access).expect("mint");
            nibbles.extend(minted.raw.expose().bytes().skip(4));
            assert!(seen.insert(minted.raw.expose().to_owned()), "a secret repeated");
        }
        assert_eq!(nibbles.len(), 16, "every hex digit appears");
    }

    #[test]
    fn hashing_is_sha256_of_the_whole_string() {
        // SHA-256("abc"), the standard test vector.
        assert_eq!(
            hash_secret("abc").as_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(hash_secret("sva_a"), hash_secret("sva_b"));
    }

    #[test]
    fn prefixes_are_recognised_and_shapes_are_checked() {
        let access = mint(TokenKind::Access).unwrap();
        let raw = access.raw.expose();
        assert_eq!(kind_of(raw), Some(TokenKind::Access));
        assert_eq!(kind_of("svr_x"), Some(TokenKind::Refresh));
        assert_eq!(kind_of("svc_x"), Some(TokenKind::AuthorizationCode));
        assert_eq!(kind_of("sve_x"), Some(TokenKind::EmbedTicket));
        assert_eq!(kind_of("opr_x"), None);
        assert_eq!(kind_of(""), None);
        assert!(!is_well_formed(TokenKind::Refresh, raw));
        assert!(!is_well_formed(TokenKind::Access, &raw[..raw.len() - 1]));
        assert!(!is_well_formed(TokenKind::Access, &format!("{raw}0")));
        assert!(!is_well_formed(TokenKind::Access, &raw.to_uppercase()));
        assert!(!is_well_formed(TokenKind::Access, &format!("sva_{}", "g".repeat(64))));
        assert!(!is_well_formed(TokenKind::Access, ""));
        assert!(is_well_formed(TokenKind::Consent, &"0".repeat(64)));
        assert!(!is_well_formed(TokenKind::Consent, &"0".repeat(63)));
    }

    #[test]
    fn comparison_is_exact() {
        assert!(secrets_equal("sva_abc", "sva_abc"));
        assert!(!secrets_equal("sva_abc", "sva_abd"));
        assert!(!secrets_equal("sva_abc", "sva_ab"));
        assert!(!secrets_equal("", "x"));
        assert!(secrets_equal("", ""));
        let a = "a".repeat(68);
        let mut b = a.clone();
        b.replace_range(67.., "b");
        assert!(!secrets_equal(&a, &b), "a difference in the last byte is seen");
    }

    #[test]
    fn lifetimes_follow_the_configuration_defaults() {
        let config = OAuthConfig::with_defaults("https://sv.example.test");
        let life = Lifetimes::from_config(&config);
        assert_eq!(life.access, Duration::minutes(10));
        assert_eq!(life.code, Duration::seconds(60));
        assert_eq!(life.refresh_idle, Duration::days(30));
        assert_eq!(life.grant_max, Duration::days(365));
    }

    #[test]
    fn expiry_arithmetic_for_codes_access_and_the_grant() {
        let life = Lifetimes::from_config(&OAuthConfig::with_defaults("https://sv.example.test"));
        let now = ts(1_800_000_000);
        let grant_end = life.grant_expires_at(now);
        assert_eq!(grant_end, ts(1_800_000_000 + 365 * 86_400));
        assert_eq!(life.code_expires_at(now), ts(1_800_000_060));
        assert_eq!(life.access_expires_at(now, grant_end), ts(1_800_000_600));
        // An access token is cut off at the end of its grant.
        let nearly_over = ts(1_800_000_300);
        assert_eq!(life.access_expires_at(now, nearly_over), nearly_over);
        assert!(is_expired(life.access_expires_at(grant_end, grant_end), grant_end));
    }

    #[test]
    fn refresh_tokens_expire_after_thirty_idle_days_and_never_past_the_grant() {
        let life = Lifetimes::from_config(&OAuthConfig::with_defaults("https://sv.example.test"));
        let consented = ts(1_800_000_000);
        let grant_end = life.grant_expires_at(consented);
        // Issued at consent: 30 days.
        assert_eq!(
            life.refresh_expires_at(consented, grant_end),
            ts(1_800_000_000 + 30 * 86_400)
        );
        // Each rotation resets the idle clock from the rotation instant.
        let rotated_at = ts(1_800_000_000 + 20 * 86_400);
        assert_eq!(
            life.refresh_expires_at(rotated_at, grant_end),
            ts(1_800_000_000 + 50 * 86_400)
        );
        // A chain of rotations every 29 days survives until the grant's own end, and not one day
        // beyond it.
        let mut at = consented;
        let mut last = life.refresh_expires_at(at, grant_end);
        while last < grant_end {
            at += Duration::days(29);
            last = life.refresh_expires_at(at, grant_end);
            assert!(last <= grant_end);
        }
        assert_eq!(last, grant_end, "the 365 day cap wins over the 30 day idle period");
        // Once the grant has ended nothing issued is usable.
        assert!(is_expired(
            life.refresh_expires_at(grant_end + Duration::days(1), grant_end),
            grant_end + Duration::days(1)
        ));
    }

    #[test]
    fn the_instant_of_expiry_is_already_expired() {
        let t = ts(1_800_000_000);
        assert!(!is_expired(t + Duration::seconds(1), t));
        assert!(is_expired(t, t));
        assert!(is_expired(t - Duration::seconds(1), t));
    }

    #[test]
    fn lifetimes_honour_configured_values() {
        let mut config = OAuthConfig::with_defaults("https://sv.example.test");
        config.access_token_ttl_seconds = 120;
        config.refresh_token_idle_days = 7;
        config.grant_max_lifetime_days = 90;
        config.authorization_code_ttl_seconds = 30;
        let life = Lifetimes::from_config(&config);
        let now = ts(1_800_000_000);
        let end = life.grant_expires_at(now);
        assert_eq!(end, now + Duration::days(90));
        assert_eq!(life.access_expires_at(now, end), now + Duration::seconds(120));
        assert_eq!(life.refresh_expires_at(now, end), now + Duration::days(7));
        assert_eq!(life.code_expires_at(now), now + Duration::seconds(30));
    }
}
