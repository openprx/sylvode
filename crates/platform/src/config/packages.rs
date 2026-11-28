//! `[packages]` — the package store's built-in publisher key and built-in package directory, and
//! the public plugin index sources (contract M15).
//!
//! The built-in keys are optional. Without `builtin_public_key` no `sv` publisher key is active, so no
//! built-in package verifies. A build that pins a release key at compile time ignores this
//! section's key (see the `packages` module of the API).

use std::path::PathBuf;

use serde::Deserialize;

use super::raw::Issues;

/// Length of a base64url (no padding) encoded 32-byte Ed25519 public key.
const PUBLIC_KEY_TEXT_LEN: usize = 43;

/// Id of the index source a configuration without `index_sources` carries.
pub const OFFICIAL_INDEX_SOURCE_ID: &str = "official";
/// URL of the official index (the `openprx/sylvode-plugins` repository).
pub const OFFICIAL_INDEX_SOURCE_URL: &str = "https://raw.githubusercontent.com/openprx/sylvode-plugins/main/index.json";
/// Most index sources one instance may configure.
pub const MAX_INDEX_SOURCES: usize = 8;
/// Most exact hosts of `index_download_hosts`.
const MAX_DOWNLOAD_HOSTS: usize = 32;
/// Longest source URL, as in the index's own URL rule.
const MAX_INDEX_URL_CHARS: usize = 512;
/// Default seconds between two refreshes of a source.
pub const DEFAULT_INDEX_REFRESH_SECS: u64 = 3600;
/// Accepted range of `index_refresh_secs`.
pub const INDEX_REFRESH_SECS_RANGE: std::ops::RangeInclusive<u64> = 300..=86_400;

/// A public plugin index source: operators set them, workspaces cannot add any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexSource {
    /// `^[a-z][a-z0-9-]{0,31}$`.
    pub id: String,
    /// https URL of the source's `index.json`.
    pub url: String,
}

/// The validated `[packages]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackagesConfig {
    /// Public key of the built-in publisher `sv`: 32 bytes, base64url without padding.
    pub builtin_public_key: Option<String>,
    /// Directory holding `*.svpkg` built-in packages, verified and stored at boot.
    pub builtin_dir: Option<PathBuf>,
    /// Public plugin index sources; the official one when the key is absent, none for `[]`.
    pub index_sources: Vec<IndexSource>,
    /// Seconds between two refreshes of a source.
    pub index_refresh_secs: u64,
    /// Exact host names a package download may use; empty means any public https host.
    pub index_download_hosts: Vec<String>,
}

impl Default for PackagesConfig {
    fn default() -> Self {
        Self {
            builtin_public_key: None,
            builtin_dir: None,
            index_sources: vec![IndexSource {
                id: OFFICIAL_INDEX_SOURCE_ID.to_string(),
                url: OFFICIAL_INDEX_SOURCE_URL.to_string(),
            }],
            index_refresh_secs: DEFAULT_INDEX_REFRESH_SECS,
            index_download_hosts: Vec::new(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPackages {
    builtin_public_key: Option<String>,
    builtin_dir: Option<String>,
    index_sources: Option<Vec<RawIndexSource>>,
    index_refresh_secs: Option<u64>,
    index_download_hosts: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexSource {
    id: String,
    url: String,
}

/// `^[a-z][a-z0-9-]{0,31}$`.
fn source_id_ok(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// An https URL with a host, no credentials and no fragment; the fetch checks the address.
fn source_url_problem(url: &str) -> Option<&'static str> {
    let Some(rest) = url.strip_prefix("https://") else {
        return Some("must be an https URL");
    };
    if url.chars().count() > MAX_INDEX_URL_CHARS {
        return Some("is longer than 512 characters");
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) || url.contains('#') {
        return Some("must not contain whitespace, control characters or a fragment");
    }
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Some("must have a host and no credentials");
    }
    None
}

/// A lower-case DNS name: labels of `a-z0-9-` separated by dots.
fn download_host_ok(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|label| {
            let bytes = label.as_bytes();
            (1..=63).contains(&bytes.len())
                && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
                && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        })
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
        let index_sources = self.index_sources.map_or_else(
            || PackagesConfig::default().index_sources,
            |raw| validate_sources(raw, issues),
        );
        let index_refresh_secs = match self.index_refresh_secs {
            None => DEFAULT_INDEX_REFRESH_SECS,
            Some(secs) if INDEX_REFRESH_SECS_RANGE.contains(&secs) => secs,
            Some(_) => {
                issues.push("packages.index_refresh_secs must be between 300 and 86400");
                DEFAULT_INDEX_REFRESH_SECS
            }
        };
        let index_download_hosts = validate_hosts(self.index_download_hosts.unwrap_or_default(), issues);
        PackagesConfig {
            builtin_public_key,
            builtin_dir,
            index_sources,
            index_refresh_secs,
            index_download_hosts,
        }
    }
}

fn validate_sources(raw: Vec<RawIndexSource>, issues: &mut Issues) -> Vec<IndexSource> {
    if raw.len() > MAX_INDEX_SOURCES {
        issues.push(format!(
            "packages.index_sources lists more than {MAX_INDEX_SOURCES} sources"
        ));
    }
    let mut sources: Vec<IndexSource> = Vec::new();
    for source in raw {
        let id = source.id.trim().to_string();
        let url = source.url.trim().to_string();
        if !source_id_ok(&id) {
            issues.push("packages.index_sources: every id must match ^[a-z][a-z0-9-]{0,31}$");
            continue;
        }
        if sources.iter().any(|other| other.id == id) {
            issues.push(format!("packages.index_sources: the id {id} is used twice"));
            continue;
        }
        if let Some(problem) = source_url_problem(&url) {
            issues.push(format!("packages.index_sources: the url of {id} {problem}"));
            continue;
        }
        sources.push(IndexSource { id, url });
    }
    sources
}

fn validate_hosts(raw: Vec<String>, issues: &mut Issues) -> Vec<String> {
    if raw.len() > MAX_DOWNLOAD_HOSTS {
        issues.push(format!(
            "packages.index_download_hosts lists more than {MAX_DOWNLOAD_HOSTS} hosts"
        ));
    }
    let mut hosts: Vec<String> = Vec::new();
    for host in raw {
        let host = host.trim().to_ascii_lowercase();
        if !download_host_ok(&host) {
            issues.push(
                "packages.index_download_hosts: every entry must be a DNS host name without scheme, port or path",
            );
        } else if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}
