use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::fmt;

type HmacSha256 = Hmac<Sha256>;

/// Canonical Sylvode signature header.
pub const SYLVODE_SIGNATURE_HEADER: &str = "x-sylvode-signature";
/// Generic signature header; the one the Sylvode API sends today.
pub const WEBHOOK_SIGNATURE_HEADER: &str = "x-webhook-signature";
/// Legacy header, accepted with a deprecation notice for the whole Sylvode 1.x line.
pub const LEGACY_SIGNATURE_HEADER: &str = "x-openpr-signature";

pub const INBOUND_SIGNATURE_HEADERS: [&str; 3] = [
    SYLVODE_SIGNATURE_HEADER,
    WEBHOOK_SIGNATURE_HEADER,
    LEGACY_SIGNATURE_HEADER,
];
pub const OUTBOUND_SIGNATURE_HEADER: &str = "x-webhook-signature";

/// The signature a request carries, read from all accepted signature headers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct InboundSignature {
    /// The signature hex with any `sha256=` prefix removed; `None` when no header is present.
    pub value: Option<String>,
    /// Whether the legacy `x-openpr-signature` header was among the headers present.
    pub used_legacy_header: bool,
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
            if name == LEGACY_SIGNATURE_HEADER {
                found.used_legacy_header = true;
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

pub fn verify_signature(payload: &[u8], signature: &str, secrets: &[String]) -> bool {
    let sig_hex = signature.strip_prefix("sha256=").unwrap_or(signature);

    for secret in secrets {
        if let Ok(computed) = sign_payload(payload, secret)
            && computed == sig_hex
        {
            return true;
        }
    }
    false
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
        assert!(!sig.used_legacy_header);
    }

    #[test]
    fn canonical_sylvode_header_works_and_is_not_legacy() {
        let sig = extract_signature_from_headers(&headers(&[("x-sylvode-signature", "sha256=fed987")])).unwrap();
        assert_eq!(sig.value.as_deref(), Some("fed987"));
        assert!(
            !sig.used_legacy_header,
            "the canonical header must not trigger the deprecation notice"
        );
    }

    #[test]
    fn legacy_openpr_header_works_and_is_flagged_for_the_notice() {
        let sig = extract_signature_from_headers(&headers(&[("x-openpr-signature", "sha256=def456")])).unwrap();
        assert_eq!(sig.value.as_deref(), Some("def456"));
        assert!(
            sig.used_legacy_header,
            "the legacy header must trigger the deprecation notice"
        );
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
        let map = headers(&[
            (SYLVODE_SIGNATURE_HEADER, &good()),
            (WEBHOOK_SIGNATURE_HEADER, &hex),
            (LEGACY_SIGNATURE_HEADER, &good()),
        ]);
        let sig = extract_signature_from_headers(&map).unwrap();
        assert_eq!(sig.value.as_deref(), Some(hex.as_str()));
        assert!(sig.used_legacy_header);
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
            LEGACY_SIGNATURE_HEADER,
            HeaderValue::from_bytes(b"sha256=\xff").unwrap(),
        );
        assert_eq!(
            extract_signature_from_headers(&map),
            Err(SignatureHeaderError::Malformed {
                header: LEGACY_SIGNATURE_HEADER
            })
        );
    }
}
