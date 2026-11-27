//! What the old plugin system keeps of an invocation: digests, never the values.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// `{"digest": "sha256:<hex>", "size": <bytes>}` of the canonical JSON bytes of `value`.
#[must_use]
pub fn digest_object(value: &Value) -> Value {
    let bytes = serde_jcs::to_vec(value).unwrap_or_default();
    json!({
        "digest": format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
        "size": bytes.len(),
    })
}

/// The fixed error code stored in `plugin_invocations.error_message`; never guest text.
pub const ERROR_FAILED: &str = "plugin_failed";
pub const ERROR_TIMEOUT: &str = "plugin_timeout";
pub const ERROR_NO_MODULE: &str = "plugin_no_module";
pub const ERROR_VALIDATION_FAILED: &str = "validation_failed";

#[cfg(test)]
mod tests {
    use super::digest_object;
    use serde_json::json;

    /// The digest is over the canonical bytes, so key order does not matter, and it carries no value.
    #[test]
    fn the_digest_is_canonical_and_holds_no_value() {
        let a = digest_object(&json!({"b": 1, "a": "secret"}));
        let b = digest_object(&json!({"a": "secret", "b": 1}));
        assert_eq!(a, b);
        assert!(
            a["digest"]
                .as_str()
                .is_some_and(|d| d.starts_with("sha256:") && d.len() == 71)
        );
        assert_eq!(a["size"], 20);
        assert!(!a.to_string().contains("secret"));
        assert_ne!(a, digest_object(&json!({"a": "other", "b": 1})));
    }
}
