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

/// Publisher reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publisher {
    /// Publisher id (prefix of the package id).
    pub id: String,
    /// `ed25519:` + 32 hex.
    pub key_id: String,
}

/// Compatibility block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// Minimum Sylvode version (semver).
    pub min: String,
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

/// A requested capability: an explicit id, optionally with the bind its grant selector names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Explicit capability id (C1 grammar; no wildcards or bundles).
    pub id: String,
    /// `$bind.<name>`: the proposed grant selector `{"form": <bound form id>}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
}

/// An action a component exports (world `action`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionEntry {
    /// `^[a-z][a-z0-9_]{0,63}$`, unique among the actions.
    pub name: String,
    /// Always `action`.
    pub world: String,
    /// Package path of the component.
    pub component: String,
    /// Display label (i18n map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<BTreeMap<String, String>>,
}

/// A field rule a component implements (world `field-rule`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleEntry {
    /// `^[a-z][a-z0-9_]{0,63}$`, unique among the rules.
    pub name: String,
    /// Always `field-rule`.
    pub world: String,
    /// Package path of the component.
    pub component: String,
    /// Stages the rule runs in: `normalize`, `validate`, `transform`.
    pub stages: Vec<String>,
    /// C2 field type names the rule applies to.
    pub field_types: Vec<String>,
    /// JSON schema of the rule's `config`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
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
    /// Layout key (one of `layouts`).
    pub layout: String,
    /// `visitor` or `operator`.
    pub audience: String,
    /// Navigation label (i18n map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav: Option<BTreeMap<String, String>>,
}

/// The package manifest.
///
/// `custom_code` is deliberately not a field: the server computes it
/// ([`Manifest::computed_custom_code`]) and a manifest that carries it is refused as an unknown
/// field.
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
    /// WIT package of the components, `sv:plugin@4.x.y`; required with actions or rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit: Option<String>,
    /// Summary (i18n map, `en` required).
    pub summary: BTreeMap<String, String>,
    /// SPDX licence id.
    pub license: String,
    /// Binds by name.
    #[serde(default)]
    pub binds: BTreeMap<String, Bind>,
    /// Requested capabilities.
    #[serde(default)]
    pub requests: Vec<Request>,
    /// Actions.
    #[serde(default)]
    pub actions: Vec<ActionEntry>,
    /// Field rules.
    #[serde(default)]
    pub rules: Vec<RuleEntry>,
    /// Guest message codes to i18n text.
    #[serde(default)]
    pub messages: BTreeMap<String, BTreeMap<String, String>>,
    /// Layout files.
    #[serde(default)]
    pub layouts: Vec<LayoutEntry>,
    /// Site pages.
    #[serde(default)]
    pub pages: Vec<PageEntry>,
    /// Required package ids.
    #[serde(default)]
    pub depends: Vec<String>,
    /// Content path to `sha256:<64 hex>`.
    pub files: BTreeMap<String, String>,
}

const MAX_LIST: usize = 128;
const MAX_TEXT: usize = 512;

fn bad(msg: impl Into<String>) -> PackageError {
    PackageError::new(ErrorCode::ManifestInvalid, msg)
}

/// `[a-z0-9][a-z0-9-]*` within a length range.
fn name_ok(s: &str, min: usize, max: usize) -> bool {
    let b = s.as_bytes();
    (min..=max).contains(&b.len())
        && b.first().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// C1's `package_key` grammar `^[a-z0-9][a-z0-9-]{0,38}/[a-z0-9][a-z0-9-]{0,63}$`; returns the
/// publisher part.
fn package_id_publisher(id: &str) -> Option<&str> {
    let (p, n) = id.split_once('/')?;
    (name_ok(p, 1, 39) && name_ok(n, 1, 64)).then_some(p)
}

fn hex_tail(s: &str, prefix: &str, len: usize) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|h| h.len() == len && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn is_digest(s: &str) -> bool {
    hex_tail(s, "sha256:", 64)
}

fn is_key_id(s: &str) -> bool {
    hex_tail(s, "ed25519:", 32)
}

fn ident(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase())
        && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
}

/// `^[a-z][a-z0-9_]{0,63}$`.
fn entry_name(s: &str) -> bool {
    s.len() <= 64 && ident(s)
}

/// Guest message codes `[a-z0-9_]{1,64}`.
fn message_code(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Layout keys and slot ids: dotted lower-case names.
fn dotted(s: &str, max: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

fn route_ok(s: &str) -> bool {
    s.starts_with('/')
        && s.len() <= 200
        && !s.contains("//")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b':' | b'.'))
        && s.split('/').all(|seg| seg != "." && seg != "..")
}

/// An i18n map: `en` present and non-empty, language keys of two to eight letters, bounded text.
fn check_i18n(what: &str, map: &BTreeMap<String, String>) -> Result<()> {
    if !map.get("en").is_some_and(|s| !s.trim().is_empty()) {
        return Err(bad(format!("{what} needs a non-empty `en`")));
    }
    for (lang, text) in map {
        if !(2..=8).contains(&lang.len()) || !lang.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-') {
            return Err(bad(format!("{what}: bad language key `{lang}`")));
        }
        if text.len() > MAX_TEXT || text.chars().any(char::is_control) {
            return Err(bad(format!("{what}[{lang}] is too long or has control characters")));
        }
    }
    Ok(())
}

fn too_many(what: &str, n: usize) -> Result<()> {
    if n > MAX_LIST {
        Err(bad(format!("{what} lists more than {MAX_LIST} entries")))
    } else {
        Ok(())
    }
}

/// `sv:plugin@<major>.<minor>.<patch>` with major 4.
fn wit_ok(wit: &str) -> bool {
    wit.strip_prefix("sv:plugin@")
        .and_then(|v| Version::parse(v).ok())
        .is_some_and(|v| v.major == 4 && v.pre.is_empty() && v.build.is_empty())
}

/// Checks one request: an explicit capability id (dotted lower-case, no wildcards) and an
/// optional `$bind.<name>` hint that names a bind. C1 owns the catalogue and the `PLUGIN`
/// exposure check; the id is treated as an opaque string here.
fn check_request(req: &Request, binds: &BTreeMap<String, Bind>) -> Result<()> {
    let segs: Vec<&str> = req.id.split('.').collect();
    if segs.len() < 2 || segs.len() > 6 || !segs.iter().all(|s| ident(s)) {
        return Err(bad(format!("request `{}`: bad capability id", req.id)));
    }
    if let Some(form) = &req.form {
        let name = form
            .strip_prefix("$bind.")
            .ok_or_else(|| bad(format!("request `{}`: form must be `$bind.<name>`", req.id)))?;
        if !binds.contains_key(name) {
            return Err(bad(format!("request `{}`: `$bind.{name}` is not a bind", req.id)));
        }
    }
    Ok(())
}

impl Manifest {
    /// Whether the package carries custom code through its manifest: any action or rule. The
    /// server also counts a `custom-frame` layout node, which only the layout files show.
    #[must_use]
    pub fn computed_custom_code(&self) -> bool {
        !self.actions.is_empty() || !self.rules.is_empty()
    }

    /// Checks every rule of C3 §4.1 that needs only the manifest.
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
            return Err(bad("publisher.key_id must be ed25519:<32 hex>"));
        }
        Version::parse(&self.version).map_err(|e| bad(format!("version is not semver: {e}")))?;
        Version::parse(&self.sv.min).map_err(|e| bad(format!("sv.min is not semver: {e}")))?;
        check_i18n("summary", &self.summary)?;
        if self.license.trim().is_empty() || self.license.len() > 128 {
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
        too_many("binds", self.binds.len())?;
        too_many("requests", self.requests.len())?;
        too_many("actions", self.actions.len())?;
        too_many("rules", self.rules.len())?;
        too_many("messages", self.messages.len())?;
        too_many("layouts", self.layouts.len())?;
        too_many("pages", self.pages.len())?;
        too_many("depends", self.depends.len())?;
        self.check_kind()?;
        self.check_binds()?;
        self.check_components()?;
        self.check_layouts_and_pages()?;
        self.check_messages_and_depends()?;
        for r in &self.requests {
            check_request(r, &self.binds)?;
        }
        let mut seen = std::collections::BTreeSet::new();
        for r in &self.requests {
            if !seen.insert((&r.id, &r.form)) {
                return Err(bad(format!("request `{}` is listed twice", r.id)));
            }
        }
        Ok(())
    }

    fn check_kind(&self) -> Result<()> {
        let create_from = self.binds.values().any(|b| b.create_from.is_some());
        let code = !self.actions.is_empty() || !self.rules.is_empty();
        let forbid = |present: bool, what: &str| {
            if present {
                Err(bad(format!("{what} is not allowed in this kind")))
            } else {
                Ok(())
            }
        };
        if self.wit.is_some() || code {
            let wit = self
                .wit
                .as_deref()
                .ok_or_else(|| bad("wit is required with actions or rules"))?;
            if !wit_ok(wit) {
                return Err(bad("wit must be sv:plugin@4.<minor>.<patch>"));
            }
        }
        match self.kind {
            Kind::FormTemplate => {
                forbid(!self.layouts.is_empty(), "layouts")?;
                forbid(!self.pages.is_empty(), "pages")?;
                forbid(code, "actions or rules")?;
                forbid(!self.requests.is_empty(), "requests")?;
                forbid(!self.messages.is_empty(), "messages")?;
                if !create_from {
                    return Err(bad("a form-template needs a bind with create_from"));
                }
            }
            Kind::PageTemplate => {
                forbid(code, "actions or rules")?;
                forbid(!self.requests.is_empty(), "requests")?;
                forbid(!self.messages.is_empty(), "messages")?;
                forbid(create_from, "create_from")?;
                if self.layouts.is_empty() && self.pages.is_empty() {
                    return Err(bad("a page-template needs layouts or pages"));
                }
            }
            Kind::Plugin => {
                forbid(!self.pages.is_empty(), "pages")?;
                forbid(create_from, "create_from")?;
                if !code {
                    return Err(bad("a plugin needs actions or rules"));
                }
            }
            Kind::App => {}
        }
        if !self.requests.is_empty() && !code {
            return Err(bad("requests need actions or rules"));
        }
        Ok(())
    }

    fn listed(&self, path: &str, what: &str) -> Result<()> {
        if self.files.contains_key(path) {
            Ok(())
        } else {
            Err(bad(format!("{what} `{path}` is not in files")))
        }
    }

    fn check_binds(&self) -> Result<()> {
        for (name, b) in &self.binds {
            if !ident(name) {
                return Err(bad(format!("bind name `{name}` is not an identifier")));
            }
            if b.kind != "form" {
                return Err(bad(format!("bind `{name}`: kind must be form")));
            }
            if let Some(p) = &b.create_from {
                self.listed(p, "create_from")?;
            }
            if b.requires_fields.len() > 256 {
                return Err(bad(format!("bind `{name}` requires too many fields")));
            }
            for (field, ty) in &b.requires_fields {
                if !ident(ty) || !entry_name(field) {
                    return Err(bad(format!("bind `{name}`: bad requires_fields entry `{field}`")));
                }
            }
        }
        Ok(())
    }

    fn check_components(&self) -> Result<()> {
        let mut worlds: BTreeMap<&str, &str> = BTreeMap::new();
        let claim = |worlds: &BTreeMap<&'_ str, &'static str>, component: &'_ str, world: &'static str| -> Result<()> {
            self.listed(component, "component")?;
            match worlds.get(component) {
                Some(w) if *w != world => Err(bad(format!("component `{component}` serves two worlds"))),
                _ => Ok(()),
            }
        };
        let mut names = std::collections::BTreeSet::new();
        for a in &self.actions {
            if !entry_name(&a.name) || !names.insert(a.name.as_str()) {
                return Err(bad(format!("action name `{}` is invalid or repeated", a.name)));
            }
            if a.world != "action" {
                return Err(bad(format!("action `{}`: world must be `action`", a.name)));
            }
            if let Some(label) = &a.label {
                check_i18n("action label", label)?;
            }
            claim(&worlds, &a.component, "action")?;
            worlds.insert(&a.component, "action");
        }
        let mut names = std::collections::BTreeSet::new();
        for r in &self.rules {
            if !entry_name(&r.name) || !names.insert(r.name.as_str()) {
                return Err(bad(format!("rule name `{}` is invalid or repeated", r.name)));
            }
            if r.world != "field-rule" {
                return Err(bad(format!("rule `{}`: world must be `field-rule`", r.name)));
            }
            if r.stages.is_empty()
                || r.stages.len() > 3
                || r.stages
                    .iter()
                    .any(|s| !matches!(s.as_str(), "normalize" | "validate" | "transform"))
                || r.stages.iter().collect::<std::collections::BTreeSet<_>>().len() != r.stages.len()
            {
                return Err(bad(format!(
                    "rule `{}`: stages must be distinct values of normalize, validate, transform",
                    r.name
                )));
            }
            if r.field_types.is_empty() || r.field_types.len() > 32 || r.field_types.iter().any(|t| !ident(t)) {
                return Err(bad(format!("rule `{}`: bad field_types", r.name)));
            }
            claim(&worlds, &r.component, "field-rule")?;
            worlds.insert(&r.component, "field-rule");
        }
        Ok(())
    }

    fn check_layouts_and_pages(&self) -> Result<()> {
        let mut keys = std::collections::BTreeSet::new();
        for l in &self.layouts {
            if !dotted(&l.key, 128) || !keys.insert(l.key.as_str()) {
                return Err(bad(format!("layout key `{}` is invalid or repeated", l.key)));
            }
            self.listed(&l.path, "layout")?;
            if l.slots.len() > 16 || l.slots.iter().any(|s| !dotted(s, 64)) {
                return Err(bad(format!("layout `{}`: bad slots", l.key)));
            }
        }
        let mut routes = std::collections::BTreeSet::new();
        for p in &self.pages {
            if !route_ok(&p.route) || !routes.insert((p.route.as_str(), p.audience.as_str())) {
                return Err(bad(format!("page route `{}` is invalid or repeated", p.route)));
            }
            if !matches!(p.audience.as_str(), "visitor" | "operator") {
                return Err(bad(format!("page `{}`: audience must be visitor or operator", p.route)));
            }
            if !keys.contains(p.layout.as_str()) {
                return Err(bad(format!("page `{}` names unknown layout `{}`", p.route, p.layout)));
            }
            if let Some(nav) = &p.nav {
                check_i18n("page nav", nav)?;
            }
        }
        Ok(())
    }

    fn check_messages_and_depends(&self) -> Result<()> {
        for (code, text) in &self.messages {
            if !message_code(code) {
                return Err(bad(format!("message code `{code}` must be [a-z0-9_]{{1,64}}")));
            }
            check_i18n("message", text)?;
        }
        for d in &self.depends {
            if package_id_publisher(d).is_none() {
                return Err(bad(format!("depends entry `{d}` is not a package id")));
            }
        }
        Ok(())
    }
}
