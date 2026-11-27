//! Manifest types and rules of `sv.package.v1`.

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, PackageError, Result};

/// The `schema` member every manifest must carry.
pub const SCHEMA_ID: &str = "sv.package.v1";

/// Package kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Form templates.
    FormTemplate,
    /// Layouts and pages bound to existing forms.
    PageTemplate,
    /// Forms, layouts, pages and components.
    App,
    /// Components, layouts and binds.
    Plugin,
}

impl Kind {
    const fn has_components(self) -> bool {
        matches!(self, Self::App | Self::Plugin)
    }
}

/// Publisher reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publisher {
    /// Publisher id (prefix of the package id).
    pub id: String,
    /// `ed25519:` + 16 hex.
    pub key_id: String,
}

/// Compatibility block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// Minimum Sylvode version (semver).
    pub min: String,
    /// Plugin ABI, e.g. `sv:plugin@4`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_abi: Option<String>,
    /// Layout schema id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Form schema id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form_schema: Option<String>,
}

/// A bind: a form the installer resolves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bind {
    /// Always `form` in v1.
    pub kind: String,
    /// Package path of a form schema to create the form from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_from: Option<String>,
    /// Field key to C2 field type name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires_fields: BTreeMap<String, String>,
}

/// A form file bound to a bind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormEntry {
    /// Bind name.
    pub bind: String,
    /// Package path.
    pub path: String,
}

/// A layout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutEntry {
    /// Layout key.
    pub key: String,
    /// Package path.
    pub path: String,
    /// Slots it is placed in.
    #[serde(default)]
    pub slots: Vec<String>,
}

/// A site page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEntry {
    /// Route.
    pub route: String,
    /// Layout key.
    pub layout: String,
    /// Navigation label (i18n map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav: Option<BTreeMap<String, String>>,
}

/// A WebAssembly component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentEntry {
    /// WIT world (`action`, `field-rule`, ...); validated by the host.
    pub world: String,
    /// Package path.
    pub path: String,
    /// Export names.
    #[serde(default)]
    pub exports: Vec<String>,
}

/// The package manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Always [`SCHEMA_ID`].
    pub schema: String,
    /// Kind.
    pub kind: Kind,
    /// `<publisher>/<name>`.
    pub id: String,
    /// Semver version.
    pub version: String,
    /// Publisher.
    pub publisher: Publisher,
    /// Compatibility.
    pub sv: Compat,
    /// Summary (i18n map, `en` required).
    pub summary: BTreeMap<String, String>,
    /// SPDX licence id.
    pub license: String,
    /// Binds by name.
    #[serde(default)]
    pub binds: BTreeMap<String, Bind>,
    /// Requested grants (C1 grammar).
    #[serde(default)]
    pub requests: Vec<String>,
    /// Form files.
    #[serde(default)]
    pub forms: Vec<FormEntry>,
    /// Layout files.
    #[serde(default)]
    pub layouts: Vec<LayoutEntry>,
    /// Site pages.
    #[serde(default)]
    pub pages: Vec<PageEntry>,
    /// Components.
    #[serde(default)]
    pub components: Vec<ComponentEntry>,
    /// Required package ids (shape not fixed by C3; plain ids for now).
    #[serde(default)]
    pub depends: Vec<String>,
    /// Content path to `sha256:<64 hex>`.
    pub files: BTreeMap<String, String>,
    /// Not trusted: the server computes custom code from the components (see
    /// [`Manifest::computed_custom_code`]). When present it must agree, otherwise the manifest is
    /// refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_code: Option<bool>,
}

fn bad(msg: impl Into<String>) -> PackageError {
    PackageError::new(ErrorCode::ManifestInvalid, msg)
}

fn name_ok(s: &str, min: usize, max: usize) -> bool {
    let b = s.as_bytes();
    (min..=max).contains(&b.len())
        && b.first().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `^[a-z0-9][a-z0-9-]{1,31}/[a-z0-9][a-z0-9-]{1,63}$`; returns the publisher part.
fn package_id_publisher(id: &str) -> Option<&str> {
    let (p, n) = id.split_once('/')?;
    (name_ok(p, 2, 32) && name_ok(n, 2, 64)).then_some(p)
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn is_key_id(s: &str) -> bool {
    s.strip_prefix("ed25519:")
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn ident(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase())
        && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
}

/// Checks one request against the grant grammar: `<capability>[@<key>=<value>]`, where the
/// capability is a dotted lower-case id and the value is `$bind.<name>` (a key of `binds`),
/// `$install` or a literal of `[A-Za-z0-9_.:-]`. C1 owns the capability catalogue; ids are
/// treated as opaque strings here.
fn check_request(req: &str, binds: &BTreeMap<String, Bind>) -> Result<()> {
    let (id, selector) = match req.split_once('@') {
        Some((i, s)) => (i, Some(s)),
        None => (req, None),
    };
    let segs: Vec<&str> = id.split('.').collect();
    if segs.len() < 2 || segs.len() > 6 || !segs.iter().all(|s| ident(s)) {
        return Err(bad(format!("request `{req}`: bad capability id")));
    }
    let Some(sel) = selector else { return Ok(()) };
    let Some((key, value)) = sel.split_once('=') else {
        return Err(bad(format!("request `{req}`: selector must be key=value")));
    };
    if !ident(key) {
        return Err(bad(format!("request `{req}`: bad selector key")));
    }
    if let Some(name) = value.strip_prefix("$bind.") {
        if !binds.contains_key(name) {
            return Err(bad(format!("request `{req}`: `$bind.{name}` is not a bind")));
        }
    } else if value != "$install"
        && (value.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-')))
    {
        return Err(bad(format!("request `{req}`: bad selector value")));
    }
    Ok(())
}

impl Manifest {
    /// Whether the package carries custom code: any component is present.
    #[must_use]
    pub fn computed_custom_code(&self) -> bool {
        !self.components.is_empty()
    }

    /// Checks every rule of C3 §4 that needs only the manifest.
    ///
    /// # Errors
    /// `manifest_invalid` with the first rule broken.
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA_ID {
            return Err(bad("schema must be sv.package.v1"));
        }
        let publisher = package_id_publisher(&self.id).ok_or_else(|| bad("id does not match <publisher>/<name>"))?;
        if publisher != self.publisher.id {
            return Err(bad("id prefix must equal publisher.id"));
        }
        if !is_key_id(&self.publisher.key_id) {
            return Err(bad("publisher.key_id must be ed25519:<16 hex>"));
        }
        Version::parse(&self.version).map_err(|e| bad(format!("version is not semver: {e}")))?;
        Version::parse(&self.sv.min).map_err(|e| bad(format!("sv.min is not semver: {e}")))?;
        if !self.summary.get("en").is_some_and(|s| !s.is_empty()) {
            return Err(bad("summary needs a non-empty `en`"));
        }
        if self.license.trim().is_empty() {
            return Err(bad("license is required"));
        }
        for (path, digest) in &self.files {
            crate::container::validate_entry_path(path)
                .map_err(|e| bad(format!("files key `{path}`: {}", e.message)))?;
            if !path.starts_with("contents/") {
                return Err(bad(format!("files key `{path}` must be under contents/")));
            }
            if !is_digest(digest) {
                return Err(bad(format!("files digest of `{path}` must be sha256:<64 hex>")));
            }
        }
        if self.custom_code.is_some_and(|c| c != self.computed_custom_code()) {
            return Err(bad("custom_code disagrees with the components"));
        }
        self.check_kind()?;
        self.check_references()?;
        for r in &self.requests {
            check_request(r, &self.binds)?;
        }
        Ok(())
    }

    fn check_kind(&self) -> Result<()> {
        let k = self.kind;
        if !self.requests.is_empty() && !k.has_components() {
            return Err(bad("only kinds with components may carry requests"));
        }
        let create_from = self.binds.values().any(|b| b.create_from.is_some());
        let forbid = |present: bool, what: &str| {
            if present {
                Err(bad(format!("{what} is not allowed in this kind")))
            } else {
                Ok(())
            }
        };
        match k {
            Kind::FormTemplate => {
                forbid(!self.layouts.is_empty(), "layouts")?;
                forbid(!self.pages.is_empty(), "pages")?;
                forbid(!self.components.is_empty(), "components")?;
                if self.forms.is_empty() {
                    return Err(bad("a form-template needs forms"));
                }
            }
            Kind::PageTemplate => {
                forbid(!self.forms.is_empty(), "forms")?;
                forbid(!self.components.is_empty(), "components")?;
                forbid(create_from, "create_from")?;
                if self.layouts.is_empty() && self.pages.is_empty() {
                    return Err(bad("a page-template needs layouts or pages"));
                }
            }
            Kind::Plugin => {
                forbid(!self.forms.is_empty(), "forms")?;
                forbid(!self.pages.is_empty(), "pages")?;
                forbid(create_from, "create_from")?;
                if self.components.is_empty() {
                    return Err(bad("a plugin needs components"));
                }
            }
            Kind::App => {}
        }
        Ok(())
    }

    fn check_references(&self) -> Result<()> {
        let listed = |path: &str, what: &str| {
            if self.files.contains_key(path) {
                Ok(())
            } else {
                Err(bad(format!("{what} `{path}` is not in files")))
            }
        };
        for (name, b) in &self.binds {
            if !ident(name) {
                return Err(bad(format!("bind name `{name}` is not an identifier")));
            }
            if b.kind != "form" {
                return Err(bad(format!("bind `{name}`: kind must be form")));
            }
            if let Some(p) = &b.create_from {
                listed(p, "create_from")?;
            }
        }
        for f in &self.forms {
            if !self.binds.contains_key(&f.bind) {
                return Err(bad(format!("form entry names unknown bind `{}`", f.bind)));
            }
            listed(&f.path, "form")?;
        }
        for l in &self.layouts {
            listed(&l.path, "layout")?;
        }
        for c in &self.components {
            listed(&c.path, "component")?;
        }
        Ok(())
    }
}
