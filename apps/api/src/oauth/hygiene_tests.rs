//! Secrets stay out of `Debug` and `Display` output, and out of error bodies.
//!
//! The secret-bearing types are impossible to print by construction: their `Debug` is a fixed
//! string and they implement no `Display`. These tests render every one of them, in compact and
//! pretty form, inside a struct that holds them, and check that neither the secret nor its hash
//! shows up; and they check that `Display` is not implemented at all.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::assertions_on_constants
)]

use std::marker::PhantomData;

use platform::config::{OAuthConfig, Secret};

use super::audit::{AuditEvent, AuditRecord, DetailValue};
use super::error::{OAuthBearerError, OAuthError, OAuthErrorKind};
use super::tokens::{MintedToken, RawToken, TokenHash, TokenKind, hash_secret, mint};
use super::validate::{AudienceRule, BearerFailure, ForwardCredential, RejectReason};

/// Resolves to `true` only for types that implement `Display`.
struct Probe<T>(PhantomData<T>);
trait NotDisplay {
    const IMPLEMENTS: bool = false;
}
impl<T> NotDisplay for Probe<T> {}
impl<T: std::fmt::Display> Probe<T> {
    const IMPLEMENTS: bool = true;
}

macro_rules! assert_not_display {
    ($($ty:ty),+ $(,)?) => {
        $(assert!(!Probe::<$ty>::IMPLEMENTS, concat!(stringify!($ty), " must not implement Display"));)+
    };
}

#[derive(Debug)]
#[allow(dead_code)]
struct Holder {
    minted: MintedToken,
    hash: TokenHash,
    config: OAuthConfig,
}

#[test]
fn secret_bearing_types_do_not_implement_display() {
    assert_not_display!(RawToken, TokenHash, MintedToken, Secret, ForwardCredential<'static>);
    // The probe does detect Display when it is there.
    assert!(Probe::<String>::IMPLEMENTS);
}

#[test]
fn debug_output_never_contains_a_secret_or_its_hash() {
    for kind in [
        TokenKind::AuthorizationCode,
        TokenKind::Access,
        TokenKind::Refresh,
        TokenKind::Consent,
    ] {
        let minted = mint(kind).expect("mint");
        let raw = minted.raw.expose().to_owned();
        let hash = minted.hash.as_hex().to_owned();
        let forward = "forward-secret-for-tests-0123456789abcdef";
        let mut config = OAuthConfig::with_defaults("https://sv.example.test");
        config.internal_forward_secret = Some(Secret::new(forward));
        let holder = Holder {
            hash: hash_secret(&raw),
            minted,
            config,
        };
        let credential = Secret::new(forward);
        let cred = ForwardCredential::new(Some(&credential), Some("mcp-server"), Some(forward));
        let rendered = [
            format!("{holder:?}"),
            format!("{holder:#?}"),
            format!("{:?}", holder.minted.raw),
            format!("{:?}", holder.hash),
            format!("{cred:?}"),
            format!("{:#?}", AudienceRule::McpForwarded(cred)),
        ]
        .join("\n");
        for needle in [raw.as_str(), hash.as_str(), forward] {
            assert!(!rendered.contains(needle), "{kind:?}: secret material in {rendered}");
        }
        assert!(rendered.contains("[REDACTED]") || rendered.contains("[redacted]"));
    }
}

#[test]
fn errors_and_audit_records_cannot_carry_a_presented_value() {
    // Descriptions are `&'static str`: nothing from a request can be formatted into them.
    let presented = mint(TokenKind::Access).expect("mint");
    let raw = presented.raw.expose();
    let failure = BearerFailure::bare(RejectReason::UnknownToken);
    let mut texts = vec![
        format!("{:?}", failure.to_error(None)),
        format!("{:?}", failure.audit_record()),
        format!(
            "{:?}",
            OAuthError::new(OAuthErrorKind::InvalidGrant, "the grant is invalid")
        ),
        format!(
            "{:?}",
            OAuthBearerError::invalid_token("the access token is invalid or has expired")
        ),
        format!(
            "{:?}",
            AuditRecord::new(AuditEvent::CodeIssued).with_detail("k", DetailValue::Code("v"))
        ),
    ];
    texts.push(format!("{:?}", failure.audit_record().detail()));
    for text in texts {
        assert!(!text.contains(raw), "{text}");
        assert!(!text.contains(presented.hash.as_hex()), "{text}");
    }
}
