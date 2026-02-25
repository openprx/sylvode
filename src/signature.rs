use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn verify_signature(payload: &[u8], signature: &str, secrets: &[String]) -> bool {
    let sig_hex = signature.strip_prefix("sha256=").unwrap_or(signature);

    for secret in secrets {
        if let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) {
            mac.update(payload);
            let result = hex::encode(mac.finalize().into_bytes());
            if result == sig_hex {
                return true;
            }
        }
    }
    false
}
