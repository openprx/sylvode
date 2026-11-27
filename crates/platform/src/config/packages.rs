//! `[packages]` — the package store's built-in publisher key and built-in package directory.
//!
//! Both keys are optional. Without `builtin_public_key` no `sv` publisher key is active, so no
//! built-in package verifies. A build that pins a release key at compile time ignores this
//! section's key (see the `packages` module of the API).

use std::path::PathBuf;

use serde::Deserialize;

use super::raw::Issues;

/// Length of a base64url (no padding) encoded 32-byte Ed25519 public key.
const PUBLIC_KEY_TEXT_LEN: usize = 43;

/// The validated `[packages]` section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackagesConfig {
    /// Public key of the built-in publisher `sv`: 32 bytes, base64url without padding.
    pub builtin_public_key: Option<String>,
    /// Directory holding `*.svpkg` built-in packages, verified and stored at boot.
    pub builtin_dir: Option<PathBuf>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPackages {
    builtin_public_key: Option<String>,
    builtin_dir: Option<String>,
}

impl RawPackages {
    pub(super) fn validate(self, issues: &mut Issues) -> PackagesConfig {
        let builtin_public_key = self.builtin_public_key.and_then(|key| {
            let key = key.trim().to_string();
            issues.record(
                if key.len() == PUBLIC_KEY_TEXT_LEN
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    Ok(key)
                } else {
                    Err(
                        "packages.builtin_public_key must be a 32-byte Ed25519 public key in base64url without padding"
                            .to_string(),
                    )
                },
            )
        });
        let builtin_dir = self.builtin_dir.and_then(|dir| {
            let dir = dir.trim().to_string();
            issues.record(if dir.is_empty() {
                Err("packages.builtin_dir must not be empty".to_string())
            } else {
                Ok(PathBuf::from(dir))
            })
        });
        PackagesConfig {
            builtin_public_key,
            builtin_dir,
        }
    }
}
