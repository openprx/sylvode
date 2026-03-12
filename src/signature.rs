use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub const INBOUND_SIGNATURE_HEADERS: [&str; 2] = ["x-webhook-signature", "x-openpr-signature"];
pub const OUTBOUND_SIGNATURE_HEADER: &str = "x-webhook-signature";

pub fn extract_signature_from_headers(headers: &HeaderMap) -> Option<String> {
    INBOUND_SIGNATURE_HEADERS
        .iter()
        .find_map(|name| headers.get(*name).and_then(|v| v.to_str().ok()))
        .map(|value| value.strip_prefix("sha256=").unwrap_or(value).to_string())
}

pub fn sign_payload(payload: &[u8], secret: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC-SHA256 accepts any key length");
    mac.update(payload);
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_signature(payload: &[u8], signature: &str, secrets: &[String]) -> bool {
    let sig_hex = signature.strip_prefix("sha256=").unwrap_or(signature);

    for secret in secrets {
        if sign_payload(payload, secret) == sig_hex {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn extracts_signature_from_x_webhook_signature() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-webhook-signature",
            HeaderValue::from_static("sha256=abc123"),
        );

        let sig = extract_signature_from_headers(&headers);
        assert_eq!(sig.as_deref(), Some("abc123"));
    }

    #[test]
    fn extracts_signature_from_x_openpr_signature() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-openpr-signature",
            HeaderValue::from_static("sha256=def456"),
        );

        let sig = extract_signature_from_headers(&headers);
        assert_eq!(sig.as_deref(), Some("def456"));
    }
}
