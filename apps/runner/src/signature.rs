use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::fmt;

type HmacSha256 = Hmac<Sha256>;

/// Canonical Sylvode signature header.
pub const SYLVODE_SIGNATURE_HEADER: &str = "x-sylvode-signature";
/// Generic signature header; the one the Sylvode API sends today.
pub const WEBHOOK_SIGNATURE_HEADER: &str = "x-webhook-signature";

pub const INBOUND_SIGNATURE_HEADERS: [&str; 2] = [SYLVODE_SIGNATURE_HEADER, WEBHOOK_SIGNATURE_HEADER];
pub const OUTBOUND_SIGNATURE_HEADER: &str = "x-webhook-signature";

/// The signature a request carries, read from all accepted signature headers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct InboundSignature {
    /// The signature hex with any `sha256=` prefix removed; `None` when no header is present.
    pub value: Option<String>,
}

/// Why the signature headers of a request were refused before any HMAC was computed.
#[derive(Debug, PartialEq, Eq)]
pub enum SignatureHeaderError {
    /// Two signature headers (or two values of one header) carry different signatures.
    Conflicting,
    /// A signature header value is not visible ASCII.
    Malformed { header: &'static str },
}

impl fmt::Display for SignatureHeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflicting => f.write_str("signature headers carry different signatures"),
            Self::Malformed { header } => write!(f, "signature header {header} is not visible ASCII"),
        }
    }
}

/// Reads the request signature from every accepted signature header.
///
/// All present values must agree once a `sha256=` prefix is removed. Any disagreement is refused
/// as a whole rather than resolved by picking one header: picking one would let an invalid
/// signature ride along a valid one, and a header the sender did not mean to be authoritative
/// would silently decide whether the request is trusted.
pub fn extract_signature_from_headers(headers: &HeaderMap) -> Result<InboundSignature, SignatureHeaderError> {
    let mut found = InboundSignature::default();
    for name in INBOUND_SIGNATURE_HEADERS {
        for raw in headers.get_all(name) {
            let text = raw
                .to_str()
                .map_err(|_| SignatureHeaderError::Malformed { header: name })?;
            let normalized = text.strip_prefix("sha256=").unwrap_or(text);
            match &found.value {
                Some(existing) if existing != normalized => return Err(SignatureHeaderError::Conflicting),
                Some(_) => {}
                None => found.value = Some(normalized.to_string()),
            }
        }
    }
    Ok(found)
}

pub fn sign_payload(payload: &[u8], secret: &str) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| format!("HMAC init failed: {e}"))?;
    mac.update(payload);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Length of an HMAC-SHA256 digest in bytes.
const DIGEST_LEN: usize = 32;

/// Decodes a presented signature: exactly 64 lowercase hex digits, without any prefix.
///
/// Uppercase hex is refused, as it always was: the signature used to be compared as text against
/// the lowercase hex of the computed digest. Anything else that is not exactly one digest is
/// refused here too, so it never reaches the HMAC comparison.
fn decode_digest_hex(sig_hex: &str) -> Option<[u8; DIGEST_LEN]> {
    let well_formed =
        sig_hex.len() == DIGEST_LEN * 2 && sig_hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return None;
    }
    let mut digest = [0_u8; DIGEST_LEN];
    hex::decode_to_slice(sig_hex, &mut digest).ok()?;
    Some(digest)
}

/// Whether `sig_hex` (a signature with any prefix already removed) is the HMAC-SHA256 of
/// `payload` under one of `secrets`.
///
/// The digests are compared in constant time by `Mac::verify_slice`. Both the HTTP receiver and
/// the tunnel verify through this function.
///
/// An empty or blank secret never verifies anything. Start-up refuses such a secret, but an empty
/// HMAC key is one every sender knows, so this function does not rely on that check.
pub fn verify_digest_hex<'a>(payload: &[u8], sig_hex: &str, secrets: impl IntoIterator<Item = &'a str>) -> bool {
    let Some(presented) = decode_digest_hex(sig_hex) else {
        return false;
    };
    secrets
        .into_iter()
        .filter(|secret| !secret.trim().is_empty())
        .any(|secret| {
            HmacSha256::new_from_slice(secret.as_bytes()).is_ok_and(|mut mac| {
                mac.update(payload);
                mac.verify_slice(&presented).is_ok()
            })
        })
}

/// Verifies an HTTP request signature, `sha256=<hex>` or bare `<hex>`, against `secrets`.
pub fn verify_signature(payload: &[u8], signature: &str, secrets: &[String]) -> bool {
    let sig_hex = signature.strip_prefix("sha256=").unwrap_or(signature);
    verify_digest_hex(payload, sig_hex, secrets.iter().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const BODY: &[u8] = br#"{"event":"issue.created"}"#;
    const SECRET: &str = "s3cret";

    fn good() -> String {
        format!("sha256={}", sign_payload(BODY, SECRET).unwrap())
    }

    fn bad() -> String {
        format!("sha256={}", sign_payload(BODY, "wrong-secret").unwrap())
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    /// The decision the handler makes: refused headers, or the HMAC over what was extracted.
    fn accepted(map: &HeaderMap) -> bool {
        extract_signature_from_headers(map)
            .is_ok_and(|sig| verify_signature(BODY, sig.value.as_deref().unwrap_or_default(), &[SECRET.to_string()]))
    }

    #[test]
    fn the_good_and_bad_fixtures_really_differ() {
        assert_ne!(good(), bad());
        assert!(accepted(&headers(&[(WEBHOOK_SIGNATURE_HEADER, &good())])));
        assert!(!accepted(&headers(&[(WEBHOOK_SIGNATURE_HEADER, &bad())])));
    }

    #[test]
    fn every_accepted_header_verifies_a_good_signature_and_refuses_a_bad_one() {
        for name in INBOUND_SIGNATURE_HEADERS {
            assert!(accepted(&headers(&[(name, &good())])), "{name} with a good signature");
            assert!(!accepted(&headers(&[(name, &bad())])), "{name} with a bad signature");
        }
    }

    #[test]
    fn extracts_signature_from_x_webhook_signature() {
        let sig = extract_signature_from_headers(&headers(&[("x-webhook-signature", "sha256=abc123")])).unwrap();
        assert_eq!(sig.value.as_deref(), Some("abc123"));
    }

    #[test]
    fn sylvode_header_works() {
        let sig = extract_signature_from_headers(&headers(&[("x-sylvode-signature", "sha256=fed987")])).unwrap();
        assert_eq!(sig.value.as_deref(), Some("fed987"));
    }

    #[test]
    fn no_signature_header_yields_no_value() {
        assert_eq!(
            extract_signature_from_headers(&HeaderMap::new()),
            Ok(InboundSignature::default())
        );
    }

    #[test]
    fn agreeing_headers_are_accepted_with_or_without_prefix() {
        let hex = good().trim_start_matches("sha256=").to_string();
        let map = headers(&[(SYLVODE_SIGNATURE_HEADER, &good()), (WEBHOOK_SIGNATURE_HEADER, &hex)]);
        let sig = extract_signature_from_headers(&map).unwrap();
        assert_eq!(sig.value.as_deref(), Some(hex.as_str()));
        assert!(accepted(&map));
    }

    /// A bad signature must not ride along a good one, whichever header carries which and
    /// whichever header a "take one" rule would prefer.
    #[test]
    fn a_bad_signature_beside_a_good_one_is_refused_in_every_combination() {
        for good_header in INBOUND_SIGNATURE_HEADERS {
            for bad_header in INBOUND_SIGNATURE_HEADERS {
                let map = headers(&[(good_header, &good()), (bad_header, &bad())]);
                assert_eq!(
                    extract_signature_from_headers(&map),
                    Err(SignatureHeaderError::Conflicting),
                    "good {good_header} + bad {bad_header}"
                );
                assert!(
                    !accepted(&map),
                    "good {good_header} + bad {bad_header} must not be accepted"
                );
            }
        }
    }

    #[test]
    fn a_non_ascii_signature_value_is_refused() {
        let mut map = headers(&[(WEBHOOK_SIGNATURE_HEADER, &good())]);
        map.append(
            SYLVODE_SIGNATURE_HEADER,
            HeaderValue::from_bytes(b"sha256=\xff").unwrap(),
        );
        assert_eq!(
            extract_signature_from_headers(&map),
            Err(SignatureHeaderError::Malformed {
                header: SYLVODE_SIGNATURE_HEADER
            })
        );
    }

    fn good_hex() -> String {
        sign_payload(BODY, SECRET).unwrap()
    }

    fn verifies(signature: &str) -> bool {
        verify_signature(BODY, signature, &[SECRET.to_string()])
    }

    #[test]
    fn lowercase_hex_is_accepted_with_or_without_the_prefix() {
        let hex = good_hex();
        assert!(
            hex.bytes().any(|b| b.is_ascii_lowercase()),
            "fixture must contain letters"
        );
        assert!(verifies(&hex));
        assert!(verifies(&format!("sha256={hex}")));
    }

    /// Uppercase hex was never accepted: the old text comparison was against lowercase output.
    #[test]
    fn uppercase_and_mixed_case_hex_are_refused() {
        let hex = good_hex();
        let upper = hex.to_ascii_uppercase();
        assert_ne!(upper, hex);
        assert!(!verifies(&upper));
        assert!(!verifies(&format!("sha256={upper}")));
        let pos = hex.find(|c: char| c.is_ascii_lowercase()).unwrap();
        let mut mixed = hex.clone();
        mixed.replace_range(pos..=pos, &hex[pos..=pos].to_ascii_uppercase());
        assert!(!verifies(&mixed));
    }

    #[test]
    fn an_uppercase_prefix_is_not_stripped() {
        assert!(!verifies(&format!("SHA256={}", good_hex())));
    }

    #[test]
    fn malformed_hex_is_refused_without_panicking() {
        let hex = good_hex();
        let mut non_hex = hex.clone();
        non_hex.replace_range(0..1, "g");
        for sig in [
            non_hex,
            format!(" {}", &hex[1..]),
            format!("{} ", &hex[..63]),
            format!("{}\t", &hex[..63]),
            "z".repeat(64),
            "sha256=".to_string(),
            String::new(),
        ] {
            assert!(!verifies(&sig), "{sig:?} must be refused");
        }
    }

    #[test]
    fn odd_length_hex_is_refused() {
        let hex = good_hex();
        assert!(!verifies(&hex[..63]));
        assert!(!verifies(&format!("{hex}0")));
        assert!(!verifies("a"));
    }

    #[test]
    fn a_digest_of_the_wrong_length_is_refused() {
        let hex = good_hex();
        // A prefix of the right digest, and the right digest with a byte appended.
        assert!(!verifies(&hex[..62]));
        assert!(!verifies(&hex[..32]));
        assert!(!verifies(&format!("{hex}00")));
        assert!(!verifies(&format!("{hex}{hex}")));
    }

    #[test]
    fn a_well_formed_digest_of_another_body_is_refused() {
        assert!(!verifies(&"0".repeat(64)));
        assert!(!verifies(&sign_payload(b"another body", SECRET).unwrap()));
    }

    #[test]
    fn any_configured_secret_verifies_and_none_configured_verifies_nothing() {
        let hex = good_hex();
        assert!(verify_signature(BODY, &hex, &["other".to_string(), SECRET.to_string()]));
        assert!(!verify_signature(BODY, &hex, &[]));
    }

    /// Defence in depth behind config validation: an empty or blank key never verifies, even
    /// for a signature computed with exactly that key.
    #[test]
    fn a_blank_secret_verifies_nothing() {
        for blank in ["", " ", "\t\n"] {
            let forged = sign_payload(BODY, blank).unwrap();
            assert!(!verify_signature(BODY, &forged, &[blank.to_string()]), "{blank:?}");
            assert!(
                !verify_signature(BODY, &forged, &[SECRET.to_string(), blank.to_string()]),
                "{blank:?} beside a real secret"
            );
            assert!(!verify_digest_hex(BODY, &forged, std::iter::once(blank)), "{blank:?}");
        }
    }

    /// The handler strips one `sha256=` while extracting and `verify_signature` strips another.
    #[test]
    fn a_doubled_prefix_through_the_handler_path_is_still_accepted() {
        let map = headers(&[(WEBHOOK_SIGNATURE_HEADER, &format!("sha256=sha256={}", good_hex()))]);
        assert!(accepted(&map));
        let tripled = headers(&[(
            WEBHOOK_SIGNATURE_HEADER,
            &format!("sha256=sha256=sha256={}", good_hex()),
        )]);
        assert!(!accepted(&tripled));
    }
}
