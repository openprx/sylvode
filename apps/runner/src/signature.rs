use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::fmt;

type HmacSha256 = Hmac<Sha256>;

/// Canonical Sylvode signature header.
pub const SYLVODE_SIGNATURE_HEADER: &str = "x-sylvode-signature";
/// Generic signature header; the one the Sylvode API sends today.
pub const WEBHOOK_SIGNATURE_HEADER: &str = "x-webhook-signature";
/// Timestamp header that goes with [`SYLVODE_SIGNATURE_HEADER`].
pub const SYLVODE_TIMESTAMP_HEADER: &str = "x-sylvode-timestamp";
/// Timestamp header that goes with [`WEBHOOK_SIGNATURE_HEADER`]; the one the Sylvode API sends.
pub const WEBHOOK_TIMESTAMP_HEADER: &str = "x-webhook-timestamp";

pub const INBOUND_SIGNATURE_HEADERS: [&str; 2] = [SYLVODE_SIGNATURE_HEADER, WEBHOOK_SIGNATURE_HEADER];
pub const INBOUND_TIMESTAMP_HEADERS: [&str; 2] = [SYLVODE_TIMESTAMP_HEADER, WEBHOOK_TIMESTAMP_HEADER];
pub const OUTBOUND_SIGNATURE_HEADER: &str = "x-webhook-signature";
pub const OUTBOUND_TIMESTAMP_HEADER: &str = "x-webhook-timestamp";

/// The prefix of an HTTP request signature: `v1=<hex>`, an HMAC-SHA256 over `<timestamp>.<body>`.
const SIGNATURE_PREFIX: &str = "v1=";

/// The signature a request carries, read from all accepted signature headers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct InboundSignature {
    /// The signature hex with its `v1=` prefix removed; `None` when no header is present.
    pub value: Option<String>,
}

/// Why the signature headers of a request were refused before any HMAC was computed.
#[derive(Debug, PartialEq, Eq)]
pub enum SignatureHeaderError {
    /// Two signature headers (or two values of one header) carry different signatures.
    Conflicting,
    /// A signature header value is not visible ASCII.
    Malformed { header: &'static str },
    /// A signature header value does not start with `v1=`.
    UnknownFormat { header: &'static str },
}

impl fmt::Display for SignatureHeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflicting => f.write_str("signature headers carry different signatures"),
            Self::Malformed { header } => write!(f, "signature header {header} is not visible ASCII"),
            Self::UnknownFormat { header } => write!(f, "signature header {header} does not start with v1="),
        }
    }
}

/// Reads the request signature from every accepted signature header.
///
/// All present values must start with `v1=` and agree once it is removed. Any disagreement is refused
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
            let normalized = text
                .strip_prefix(SIGNATURE_PREFIX)
                .ok_or(SignatureHeaderError::UnknownFormat { header: name })?;
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
/// The digests are compared in constant time by `Mac::verify_slice`. Both the HTTP receiver
/// ([`verify_request`]) and the tunnel verify through this function.
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

/// Reads the request timestamp from every accepted timestamp header. All present values must
/// agree, for the same reason as the signatures.
pub fn extract_timestamp_from_headers(headers: &HeaderMap) -> Result<Option<String>, SignatureHeaderError> {
    let mut found: Option<String> = None;
    for name in INBOUND_TIMESTAMP_HEADERS {
        for raw in headers.get_all(name) {
            let text = raw
                .to_str()
                .map_err(|_| SignatureHeaderError::Malformed { header: name })?;
            match &found {
                Some(existing) if existing != text => return Err(SignatureHeaderError::Conflicting),
                Some(_) => {}
                None => found = Some(text.to_string()),
            }
        }
    }
    Ok(found)
}

/// The bytes a request signature covers: `<timestamp>.<body>`.
fn signed_message(timestamp: &str, payload: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(timestamp.len() + 1 + payload.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.push(b'.');
    message.extend_from_slice(payload);
    message
}

/// Signs a request body at `unix_seconds`. Returns the timestamp header value and the signature
/// header value (`v1=<hex>`).
pub fn sign_request(payload: &[u8], secret: &str, unix_seconds: i64) -> Result<(String, String), String> {
    let timestamp = unix_seconds.to_string();
    let digest = sign_payload(&signed_message(&timestamp, payload), secret)?;
    Ok((timestamp, format!("{SIGNATURE_PREFIX}{digest}")))
}

/// Verifies an HTTP request signature: `sig_hex` (the `v1=` prefix already removed) must be the
/// HMAC-SHA256 of `<timestamp>.<payload>` under one of `secrets`, and `timestamp` must be within
/// `tolerance_secs` of `now_unix` either way. A missing or unparsable timestamp is refused. The
/// digests are compared in constant time.
///
/// `tolerance_secs = 0` turns the window off (the timestamp is still signed and required).
pub fn verify_request<'a>(
    payload: &[u8],
    timestamp: Option<&str>,
    sig_hex: &str,
    secrets: impl IntoIterator<Item = &'a str>,
    now_unix: i64,
    tolerance_secs: u64,
) -> bool {
    let Some(timestamp) = timestamp else {
        return false;
    };
    let Ok(sent_at) = timestamp.parse::<i64>() else {
        return false;
    };
    if tolerance_secs > 0 && sent_at.abs_diff(now_unix) > tolerance_secs {
        return false;
    }
    verify_digest_hex(&signed_message(timestamp, payload), sig_hex, secrets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const BODY: &[u8] = br#"{"event":"issue.created"}"#;
    const SECRET: &str = "s3cret";
    const NOW: i64 = 1_700_000_000;
    const WINDOW: u64 = 300;

    fn good() -> String {
        sign_request(BODY, SECRET, NOW).unwrap().1
    }

    fn bad() -> String {
        sign_request(BODY, "wrong-secret", NOW).unwrap().1
    }

    fn good_hex() -> String {
        good().trim_start_matches(SIGNATURE_PREFIX).to_string()
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    /// The decision the handler makes: refused headers, or the HMAC over what was extracted.
    fn accepted_at(map: &HeaderMap, now: i64) -> bool {
        let Ok(sig) = extract_signature_from_headers(map) else {
            return false;
        };
        let Ok(timestamp) = extract_timestamp_from_headers(map) else {
            return false;
        };
        verify_request(
            BODY,
            timestamp.as_deref(),
            sig.value.as_deref().unwrap_or_default(),
            [SECRET],
            now,
            WINDOW,
        )
    }

    fn accepted(map: &HeaderMap) -> bool {
        accepted_at(map, NOW)
    }

    fn timestamp() -> String {
        NOW.to_string()
    }

    #[test]
    fn the_good_and_bad_fixtures_really_differ() {
        assert_ne!(good(), bad());
        assert!(accepted(&headers(&[
            (WEBHOOK_SIGNATURE_HEADER, &good()),
            (WEBHOOK_TIMESTAMP_HEADER, &timestamp())
        ])));
        assert!(!accepted(&headers(&[
            (WEBHOOK_SIGNATURE_HEADER, &bad()),
            (WEBHOOK_TIMESTAMP_HEADER, &timestamp())
        ])));
    }

    #[test]
    fn every_accepted_header_pair_verifies_a_good_signature_and_refuses_a_bad_one() {
        for (sig_header, ts_header) in INBOUND_SIGNATURE_HEADERS.into_iter().zip(INBOUND_TIMESTAMP_HEADERS) {
            assert!(
                accepted(&headers(&[(sig_header, &good()), (ts_header, &timestamp())])),
                "{sig_header} with a good signature"
            );
            assert!(
                !accepted(&headers(&[(sig_header, &bad()), (ts_header, &timestamp())])),
                "{sig_header} with a bad signature"
            );
        }
    }

    #[test]
    fn a_fresh_timestamp_is_accepted_and_a_stale_or_future_one_is_refused() {
        let map = headers(&[
            (WEBHOOK_SIGNATURE_HEADER, &good()),
            (WEBHOOK_TIMESTAMP_HEADER, &timestamp()),
        ]);
        assert!(accepted_at(&map, NOW));
        assert!(accepted_at(&map, NOW + 300));
        assert!(accepted_at(&map, NOW - 300));
        assert!(!accepted_at(&map, NOW + 301), "stale");
        assert!(!accepted_at(&map, NOW - 301), "from the future");
    }

    #[test]
    fn a_missing_or_unparsable_timestamp_is_refused() {
        assert!(!accepted(&headers(&[(WEBHOOK_SIGNATURE_HEADER, &good())])));
        for bad_ts in ["", "abc", " 1700000000", "1700000000 ", "17e8"] {
            let map = headers(&[(WEBHOOK_SIGNATURE_HEADER, &good()), (WEBHOOK_TIMESTAMP_HEADER, bad_ts)]);
            assert!(!accepted(&map), "{bad_ts:?}");
        }
    }

    #[test]
    fn the_timestamp_is_signed() {
        // A captured signature presented with a refreshed timestamp does not verify.
        let refreshed = (NOW + 1_000).to_string();
        let map = headers(&[
            (WEBHOOK_SIGNATURE_HEADER, &good()),
            (WEBHOOK_TIMESTAMP_HEADER, &refreshed),
        ]);
        assert!(!accepted_at(&map, NOW + 1_000));
    }

    #[test]
    fn a_zero_window_turns_the_freshness_check_off_but_not_the_signature() {
        let hex = good_hex();
        assert!(verify_request(
            BODY,
            Some(&timestamp()),
            &hex,
            [SECRET],
            NOW + 10_000_000,
            0
        ));
        assert!(!verify_request(BODY, None, &hex, [SECRET], NOW, 0));
        assert!(!verify_request(
            BODY,
            Some(&timestamp()),
            bad().trim_start_matches(SIGNATURE_PREFIX),
            [SECRET],
            NOW,
            0
        ));
    }

    #[test]
    fn a_changed_body_is_refused() {
        assert!(!verify_request(
            b"another body",
            Some(&timestamp()),
            &good_hex(),
            [SECRET],
            NOW,
            WINDOW
        ));
    }

    #[test]
    fn extracts_the_signature_without_its_prefix() {
        let sig = extract_signature_from_headers(&headers(&[("x-webhook-signature", "v1=abc123")])).unwrap();
        assert_eq!(sig.value.as_deref(), Some("abc123"));
        let sig = extract_signature_from_headers(&headers(&[("x-sylvode-signature", "v1=fed987")])).unwrap();
        assert_eq!(sig.value.as_deref(), Some("fed987"));
    }

    #[test]
    fn the_old_prefix_and_bare_hex_are_refused() {
        for name in INBOUND_SIGNATURE_HEADERS {
            for value in [format!("sha256={}", good_hex()), good_hex()] {
                assert_eq!(
                    extract_signature_from_headers(&headers(&[(name, &value)])),
                    Err(SignatureHeaderError::UnknownFormat { header: name }),
                    "{name}: {value}"
                );
            }
        }
    }

    #[test]
    fn no_signature_header_yields_no_value() {
        assert_eq!(
            extract_signature_from_headers(&HeaderMap::new()),
            Ok(InboundSignature::default())
        );
        assert_eq!(extract_timestamp_from_headers(&HeaderMap::new()), Ok(None));
    }

    #[test]
    fn agreeing_headers_are_accepted() {
        let map = headers(&[
            (SYLVODE_SIGNATURE_HEADER, &good()),
            (WEBHOOK_SIGNATURE_HEADER, &good()),
            (SYLVODE_TIMESTAMP_HEADER, &timestamp()),
            (WEBHOOK_TIMESTAMP_HEADER, &timestamp()),
        ]);
        assert!(accepted(&map));
    }

    /// A bad signature must not ride along a good one, whichever header carries which and
    /// whichever header a "take one" rule would prefer.
    #[test]
    fn a_bad_signature_beside_a_good_one_is_refused_in_every_combination() {
        for good_header in INBOUND_SIGNATURE_HEADERS {
            for bad_header in INBOUND_SIGNATURE_HEADERS {
                let map = headers(&[
                    (good_header, &good()),
                    (bad_header, &bad()),
                    (WEBHOOK_TIMESTAMP_HEADER, &timestamp()),
                ]);
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
    fn disagreeing_timestamps_are_refused() {
        let other = (NOW + 1).to_string();
        let map = headers(&[
            (WEBHOOK_SIGNATURE_HEADER, &good()),
            (SYLVODE_TIMESTAMP_HEADER, &timestamp()),
            (WEBHOOK_TIMESTAMP_HEADER, &other),
        ]);
        assert_eq!(
            extract_timestamp_from_headers(&map),
            Err(SignatureHeaderError::Conflicting)
        );
        assert!(!accepted(&map));
    }

    #[test]
    fn a_non_ascii_header_value_is_refused() {
        let mut map = headers(&[(WEBHOOK_SIGNATURE_HEADER, &good())]);
        map.append(SYLVODE_SIGNATURE_HEADER, HeaderValue::from_bytes(b"v1=\xff").unwrap());
        assert_eq!(
            extract_signature_from_headers(&map),
            Err(SignatureHeaderError::Malformed {
                header: SYLVODE_SIGNATURE_HEADER
            })
        );
        let mut map = headers(&[(WEBHOOK_TIMESTAMP_HEADER, &timestamp())]);
        map.append(SYLVODE_TIMESTAMP_HEADER, HeaderValue::from_bytes(b"\xff").unwrap());
        assert_eq!(
            extract_timestamp_from_headers(&map),
            Err(SignatureHeaderError::Malformed {
                header: SYLVODE_TIMESTAMP_HEADER
            })
        );
    }

    fn verifies(signature: &str) -> bool {
        verify_request(BODY, Some(&timestamp()), signature, [SECRET], NOW, WINDOW)
    }

    #[test]
    fn lowercase_hex_is_accepted() {
        let hex = good_hex();
        assert!(
            hex.bytes().any(|b| b.is_ascii_lowercase()),
            "fixture must contain letters"
        );
        assert!(verifies(&hex));
    }

    /// Uppercase hex was never accepted: the comparison was against lowercase output.
    #[test]
    fn uppercase_and_mixed_case_hex_are_refused() {
        let hex = good_hex();
        let upper = hex.to_ascii_uppercase();
        assert_ne!(upper, hex);
        assert!(!verifies(&upper));
        let pos = hex.find(|c: char| c.is_ascii_lowercase()).unwrap();
        let mut mixed = hex.clone();
        mixed.replace_range(pos..=pos, &hex[pos..=pos].to_ascii_uppercase());
        assert!(!verifies(&mixed));
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
            "v1=".to_string(),
            String::new(),
        ] {
            assert!(!verifies(&sig), "{sig:?} must be refused");
        }
    }

    #[test]
    fn a_digest_of_the_wrong_length_is_refused() {
        let hex = good_hex();
        assert!(!verifies(&hex[..63]));
        assert!(!verifies(&hex[..62]));
        assert!(!verifies(&hex[..32]));
        assert!(!verifies(&format!("{hex}0")));
        assert!(!verifies(&format!("{hex}00")));
        assert!(!verifies(&format!("{hex}{hex}")));
        assert!(!verifies("a"));
    }

    #[test]
    fn a_well_formed_digest_of_another_body_is_refused() {
        assert!(!verifies(&"0".repeat(64)));
    }

    #[test]
    fn any_configured_secret_verifies_and_none_configured_verifies_nothing() {
        let hex = good_hex();
        assert!(verify_request(
            BODY,
            Some(&timestamp()),
            &hex,
            ["other", SECRET],
            NOW,
            WINDOW
        ));
        assert!(!verify_request(BODY, Some(&timestamp()), &hex, [], NOW, WINDOW));
    }

    /// Defence in depth behind config validation: an empty or blank key never verifies, even
    /// for a signature computed with exactly that key.
    #[test]
    fn a_blank_secret_verifies_nothing() {
        for blank in ["", " ", "\t\n"] {
            let (ts, forged) = sign_request(BODY, blank, NOW).unwrap();
            let hex = forged.trim_start_matches(SIGNATURE_PREFIX);
            assert!(!verify_request(BODY, Some(&ts), hex, [blank], NOW, WINDOW), "{blank:?}");
            assert!(
                !verify_request(BODY, Some(&ts), hex, [SECRET, blank], NOW, WINDOW),
                "{blank:?} beside a real secret"
            );
        }
    }

    #[test]
    fn the_signature_covers_timestamp_dot_body() {
        let (ts, sig) = sign_request(BODY, SECRET, NOW).unwrap();
        assert_eq!(ts, "1700000000");
        let mut message = b"1700000000.".to_vec();
        message.extend_from_slice(BODY);
        assert_eq!(sig, format!("v1={}", sign_payload(&message, SECRET).unwrap()));
    }
}
