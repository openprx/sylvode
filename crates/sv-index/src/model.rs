//! The documents of `sv.plugin-index.v1`, as Rust types.
//!
//! The types ignore unknown members when deserialised (the lenient reader of Sylvode); the strict
//! mode of [`crate::parse`] finds them separately. Optional members are omitted when serialised.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
pub use sv_package::Kind;

/// Schema id of `index.json`.
pub const INDEX_SCHEMA: &str = "sv.plugin-index.v1";
/// Schema id of `publishers/<id>.json`.
pub const PUBLISHER_SCHEMA: &str = "sv.plugin-index.publisher.v1";
/// Schema id of `plugins/<publisher>/<name>/plugin.json`.
pub const PLUGIN_SCHEMA: &str = "sv.plugin-index.plugin.v1";

/// Localised text: language key (`en`, `zh-CN`) to text. `en` is required.
pub type I18nText = BTreeMap<String, String>;

/// `index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDoc {
    /// Always [`INDEX_SCHEMA`].
    pub schema: String,
    /// Generation time, Unix seconds.
    pub generated_at: u64,
    /// Index title.
    pub title: I18nText,
    /// Index description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<I18nText>,
    /// Categories and labels plugins may use (owner-curated).
    pub metadata: IndexMetadata,
    /// Publishers by id.
    pub publishers: BTreeMap<String, PublisherDoc>,
    /// Plugins in display order.
    pub plugins: Vec<PluginDoc>,
}

/// Declared categories and labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMetadata {
    /// Category id to entry.
    pub categories: BTreeMap<String, MetaEntry>,
    /// Label id to entry.
    pub labels: BTreeMap<String, MetaEntry>,
}

/// A declared category or label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaEntry {
    /// Display title.
    pub title: I18nText,
}

/// `publishers/<id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherDoc {
    /// Always [`PUBLISHER_SCHEMA`].
    pub schema: String,
    /// Publisher id (`publisher_keys` grammar, `sv` reserved).
    pub id: String,
    /// Display name.
    pub name: I18nText,
    /// Home page (https).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// GitHub logins allowed to open pull requests for this publisher.
    pub maintainers: Vec<String>,
    /// Signing keys, oldest first.
    pub keys: Vec<PublisherKey>,
}

/// State of a publisher key. The only allowed transition is `Active` to `Revoked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    /// Usable.
    Active,
    /// Final.
    Revoked,
}

/// A publisher signing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherKey {
    /// `ed25519:` + 32 hex; equals `sv_package::key_id(public_key)`.
    pub key_id: String,
    /// Base64url (no padding) of the 32-byte Ed25519 public key.
    pub public_key: String,
    /// Key state.
    pub status: KeyStatus,
    /// Time the key was listed, Unix seconds.
    pub added_at: u64,
    /// Revocation time; present exactly when revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
    /// Revocation reason; present exactly when revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `plugins/<publisher>/<name>/plugin.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDoc {
    /// Always [`PLUGIN_SCHEMA`].
    pub schema: String,
    /// `<publisher>/<name>`.
    pub id: String,
    /// Package kind, equal to the manifest kind.
    pub kind: Kind,
    /// Display title.
    pub title: I18nText,
    /// One-line summary.
    pub summary: I18nText,
    /// `CommonMark` description.
    pub description: I18nText,
    /// Icon path inside the plugin directory.
    pub icon: String,
    /// Screenshots, at most eight.
    #[serde(default)]
    pub screenshots: Vec<Screenshot>,
    /// Home page (https).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Source repository (https).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// SPDX license expression.
    pub license: String,
    /// Category, labels and keywords.
    pub metadata: PluginMetadata,
    /// Named versions.
    pub channels: Channels,
    /// Versions, append-only, oldest first.
    pub versions: Vec<VersionEntry>,
}

/// A screenshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screenshot {
    /// Path inside the plugin directory, under `screenshots/`.
    pub path: String,
    /// Caption.
    pub caption: I18nText,
}

/// Plugin metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMetadata {
    /// Category id declared in the index metadata.
    pub category: String,
    /// Label ids declared in the index metadata.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Free search keywords.
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// Channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channels {
    /// A `listed` version; absent while no version is listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
}

/// Version state. Transitions are one-way: `Listed` to `Yanked` to `Revoked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionStatus {
    /// Importable.
    Listed,
    /// Not importable; imported copies keep working.
    Yanked,
    /// Security takedown.
    Revoked,
}

impl VersionStatus {
    /// Position in the one-way order.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Listed => 0,
            Self::Yanked => 1,
            Self::Revoked => 2,
        }
    }
}

/// One published version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    /// Exact semver, no build metadata.
    pub version: String,
    /// Publication time, Unix seconds.
    pub published_at: u64,
    /// Version state.
    pub status: VersionStatus,
    /// Required unless `listed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    /// Download URL of the package file.
    pub url: String,
    /// Package file size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 of the file bytes.
    pub sha256: String,
    /// Package digest (`sha256:` + 64 hex, C3 §4.2).
    pub digest: String,
    /// Publisher id; the prefix of the plugin id.
    pub publisher: String,
    /// Signing key; one of the publisher's keys.
    pub key_id: String,
    /// `semver::VersionReq` of the Sylvode versions this runs on.
    pub sylvode: String,
    /// Display projection of the manifest.
    pub contents: Contents,
    /// Changelog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<I18nText>,
}

/// Display-only projection of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contents {
    /// Requested capability ids.
    pub requests: Vec<String>,
    /// The package carries custom code.
    pub custom_code: bool,
    /// Action names.
    pub actions: Vec<String>,
    /// Field-rule names.
    pub rules: Vec<String>,
    /// Bind names.
    pub binds: Vec<String>,
    /// Layouts.
    pub layouts: Vec<ContentLayout>,
    /// Site pages.
    pub pages: Vec<ContentPage>,
    /// Form keys.
    pub forms: Vec<String>,
    /// The package carries a project block.
    pub project_block: bool,
}

/// A layout in `contents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentLayout {
    /// Layout key.
    pub key: String,
    /// Slots it is placed in.
    pub slots: Vec<String>,
}

/// A page in `contents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPage {
    /// Route.
    pub route: String,
    /// `visitor` or `operator`.
    pub audience: String,
}
