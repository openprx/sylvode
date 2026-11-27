//! `sv.nav.v1`: the stored navigation document, its validation and the built-in defaults.
//!
//! Everything here is pure: a document is checked against the shape rules of contract N1 section 2
//! without looking at the database. Whether the objects it names exist is `refs.rs`.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::error::ApiError;

pub const SCHEMA: &str = "sv.nav.v1";
pub const MAX_DOC_BYTES: usize = 64 * 1024;
pub const MAX_SECTIONS: usize = 20;
pub const MAX_ENTRIES: usize = 200;
const MAX_URL_BYTES: usize = 2048;
const MAX_LABEL_LOCALES: usize = 8;
const MAX_LABEL_CHARS: usize = 64;

/// Locale to text.
pub type Labels = BTreeMap<String, String>;

/// Where a document applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavScope {
    Workspace,
    Project,
}

impl NavScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Project => "project",
        }
    }
}

/// Built-in destinations of the workspace navigation.
pub const WORKSPACE_BUILTINS: &[&str] = &[
    "home.overview",
    "home.notifications",
    "home.my_work",
    "home.recent",
    "workspace.milestones",
    "workspace.market",
    "workspace.flow",
    "governance",
];

/// Built-in destinations of a project navigation.
pub const PROJECT_BUILTINS: &[&str] = &[
    "project.overview",
    "project.issues",
    "project.board",
    "project.cycles",
    "project.forms",
    "project.apps",
    "project.settings",
];

/// The work-item destinations a `custom_form` project does not have.
const WORK_ITEM_BUILTINS: &[&str] = &["project.overview", "project.issues", "project.board", "project.cycles"];

/// Glyph names an entry may use: the column glyphs plus the ones link-like entries need.
pub const ICONS: &[&str] = &[
    "house",
    "bell",
    "list-checks",
    "clock",
    "folder",
    "folder-kanban",
    "layout-list",
    "kanban",
    "repeat",
    "table-2",
    "file-text",
    "app-window",
    "settings",
    "workflow",
    "bot",
    "sparkles",
    "landmark",
    "flag",
    "users",
    "briefcase",
    "link",
    "external-link",
    "star",
    "layout-template",
    "globe",
    "puzzle",
];

/// Whether a built-in applies to a project of this type.
#[must_use]
pub fn builtin_applies(page: &str, project_type: Option<&str>) -> bool {
    !(project_type == Some("custom_form") && WORK_ITEM_BUILTINS.contains(&page))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavDoc {
    pub schema: String,
    pub scope: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub id: String,
    #[serde(default)]
    pub label: Option<Labels>,
    #[serde(default)]
    pub collapsed: bool,
    pub entries: Vec<EntryDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryDoc {
    pub id: String,
    pub target: Target,
    #[serde(default)]
    pub label: Option<Labels>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Builtin { page: String },
    Project { project_id: Uuid },
    Form { form_id: Uuid },
    FlowPage { object_id: Uuid },
    LayoutPage { layout_id: Uuid },
    PluginApp { package_key: String, route: String },
    External { url: String },
}

impl Target {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Builtin { .. } => "builtin",
            Self::Project { .. } => "project",
            Self::Form { .. } => "form",
            Self::FlowPage { .. } => "flow_page",
            Self::LayoutPage { .. } => "layout_page",
            Self::PluginApp { .. } => "plugin_app",
            Self::External { .. } => "external",
        }
    }

    /// The id of the stored object the target names, for the kinds that name one.
    pub const fn object_id(&self) -> Option<Uuid> {
        match self {
            Self::Project { project_id } => Some(*project_id),
            Self::Form { form_id } => Some(*form_id),
            Self::FlowPage { object_id } => Some(*object_id),
            Self::LayoutPage { layout_id } => Some(*layout_id),
            Self::Builtin { .. } | Self::PluginApp { .. } | Self::External { .. } => None,
        }
    }
}

impl NavDoc {
    pub fn entries(&self) -> impl Iterator<Item = (&Section, &EntryDoc)> {
        self.sections
            .iter()
            .flat_map(|section| section.entries.iter().map(move |entry| (section, entry)))
    }
}

/// A refusal of a document or of what it names: a stable `code`, the JSON pointer `path` of the
/// offending node and a `reason` from the contract's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavError {
    pub code: &'static str,
    pub path: String,
    pub reason: &'static str,
}

impl NavError {
    pub fn invalid(path: impl Into<String>, reason: &'static str) -> Self {
        Self {
            code: "nav_invalid",
            path: path.into(),
            reason,
        }
    }

    fn unavailable(path: impl Into<String>) -> Self {
        Self {
            code: "target_kind_unavailable",
            path: path.into(),
            reason: "target_kind_unavailable",
        }
    }
}

impl From<NavError> for ApiError {
    fn from(error: NavError) -> Self {
        Self::Coded {
            status: 400,
            code: error.code,
            message: format!("the navigation document is refused: {}", error.reason),
            details: Some(json!({ "path": error.path, "reason": error.reason })),
        }
    }
}

/// An id of a section or an entry: `^[a-z0-9][a-z0-9_-]{0,63}$`. Ids starting `_` are reserved.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    id.len() <= 64
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn valid_locale(locale: &str) -> bool {
    let bytes = locale.as_bytes();
    match bytes.len() {
        2 => bytes.iter().all(u8::is_ascii_lowercase),
        5 => {
            bytes.get(..2).is_some_and(|b| b.iter().all(u8::is_ascii_lowercase))
                && bytes.get(2) == Some(&b'-')
                && bytes.get(3..).is_some_and(|b| b.iter().all(u8::is_ascii_uppercase))
        }
        _ => false,
    }
}

fn check_labels(labels: Option<&Labels>, path: &str) -> Result<(), NavError> {
    let Some(labels) = labels else {
        return Ok(());
    };
    if labels.is_empty() || labels.len() > MAX_LABEL_LOCALES {
        return Err(NavError::invalid(path, "schema"));
    }
    for (locale, text) in labels {
        let chars = text.chars().count();
        if !valid_locale(locale) || chars == 0 || chars > MAX_LABEL_CHARS {
            return Err(NavError::invalid(format!("{path}/{locale}"), "schema"));
        }
    }
    Ok(())
}

/// Whether `raw` is an external link the console may open: `http` or `https`, a host, no userinfo.
#[must_use]
pub fn external_url_ok(raw: &str) -> bool {
    if raw.len() > MAX_URL_BYTES || raw.trim() != raw {
        return false;
    }
    Url::parse(raw).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.host_str().is_some_and(|host| !host.is_empty())
    })
}

fn reserved_kind(raw: &Value) -> Option<String> {
    let sections = raw.get("sections")?.as_array()?;
    for (s, section) in sections.iter().enumerate() {
        let Some(entries) = section.get("entries").and_then(Value::as_array) else {
            continue;
        };
        for (e, entry) in entries.iter().enumerate() {
            let kind = entry.pointer("/target/kind").and_then(Value::as_str);
            if matches!(kind, Some("site_page" | "plugin_action")) {
                return Some(format!("/sections/{s}/entries/{e}/target"));
            }
        }
    }
    None
}

fn check_target(target: &Target, scope: NavScope, path: &str) -> Result<(), NavError> {
    match target {
        Target::Builtin { page } => {
            let own = match scope {
                NavScope::Workspace => WORKSPACE_BUILTINS,
                NavScope::Project => PROJECT_BUILTINS,
            };
            if own.contains(&page.as_str()) {
                Ok(())
            } else if WORKSPACE_BUILTINS.contains(&page.as_str()) || PROJECT_BUILTINS.contains(&page.as_str()) {
                Err(NavError::invalid(format!("{path}/page"), "builtin_wrong_scope"))
            } else {
                Err(NavError::invalid(format!("{path}/page"), "unknown_builtin"))
            }
        }
        Target::Project { .. } if scope == NavScope::Project => Err(NavError::invalid(path, "target_out_of_scope")),
        Target::PluginApp { package_key, route } => {
            if scope == NavScope::Workspace {
                return Err(NavError::invalid(path, "target_out_of_scope"));
            }
            let ok = |text: &str| !text.is_empty() && text.len() <= 128 && !text.chars().any(char::is_control);
            if ok(package_key) && ok(route) {
                Ok(())
            } else {
                Err(NavError::invalid(path, "schema"))
            }
        }
        Target::External { url } => {
            if external_url_ok(url) {
                Ok(())
            } else {
                Err(NavError::invalid(format!("{path}/url"), "external_url_invalid"))
            }
        }
        Target::Project { .. } | Target::Form { .. } | Target::FlowPage { .. } | Target::LayoutPage { .. } => Ok(()),
    }
}

/// Parses and checks a document received for `scope`. The order is: size, reserved target kinds,
/// shape, then the rules that need the whole document (ids, counts, built-ins, links).
///
/// # Errors
/// The first [`NavError`] found.
pub fn parse(raw: &Value, scope: NavScope) -> Result<NavDoc, NavError> {
    let size = serde_json::to_vec(raw).map_or(usize::MAX, |bytes| bytes.len());
    if size > MAX_DOC_BYTES {
        return Err(NavError::invalid("", "too_large"));
    }
    if let Some(path) = reserved_kind(raw) {
        return Err(NavError::unavailable(path));
    }
    let doc: NavDoc = serde_json::from_value(raw.clone()).map_err(|_| NavError::invalid("", "schema"))?;
    if doc.schema != SCHEMA {
        return Err(NavError::invalid("/schema", "schema"));
    }
    if doc.scope != scope.as_str() {
        return Err(NavError::invalid("/scope", "schema"));
    }
    if doc.sections.len() > MAX_SECTIONS || doc.entries().count() > MAX_ENTRIES {
        return Err(NavError::invalid("/sections", "too_large"));
    }
    let mut ids = HashSet::new();
    let mut builtins = HashSet::new();
    for (s, section) in doc.sections.iter().enumerate() {
        let path = format!("/sections/{s}");
        if !valid_id(&section.id) {
            return Err(NavError::invalid(format!("{path}/id"), "schema"));
        }
        if !ids.insert(section.id.as_str()) {
            return Err(NavError::invalid(format!("{path}/id"), "duplicate_id"));
        }
        check_labels(section.label.as_ref(), &format!("{path}/label"))?;
        for (e, entry) in section.entries.iter().enumerate() {
            let path = format!("{path}/entries/{e}");
            if !valid_id(&entry.id) {
                return Err(NavError::invalid(format!("{path}/id"), "schema"));
            }
            if !ids.insert(entry.id.as_str()) {
                return Err(NavError::invalid(format!("{path}/id"), "duplicate_id"));
            }
            check_labels(entry.label.as_ref(), &format!("{path}/label"))?;
            if entry.icon.as_deref().is_some_and(|icon| !ICONS.contains(&icon)) {
                return Err(NavError::invalid(format!("{path}/icon"), "schema"));
            }
            check_target(&entry.target, scope, &format!("{path}/target"))?;
            if let Target::Builtin { page } = &entry.target
                && !builtins.insert(page.as_str())
            {
                return Err(NavError::invalid(format!("{path}/target"), "builtin_repeated"));
            }
        }
    }
    Ok(doc)
}

fn builtin_entry(id: &str, page: &str) -> EntryDoc {
    EntryDoc {
        id: id.to_string(),
        target: Target::Builtin { page: page.to_string() },
        label: None,
        icon: None,
        hidden: false,
        pinned: false,
        locked: false,
    }
}

/// The navigation a scope has until somebody configures it: today's Home column and project tree.
#[must_use]
pub fn defaults(scope: NavScope, project_type: Option<&str>) -> NavDoc {
    let section = |id: &str, entries: Vec<EntryDoc>| Section {
        id: id.to_string(),
        label: None,
        collapsed: false,
        entries,
    };
    let sections = match scope {
        NavScope::Workspace => vec![
            section(
                "main",
                vec![
                    builtin_entry("overview", "home.overview"),
                    builtin_entry("notifications", "home.notifications"),
                    builtin_entry("my_work", "home.my_work"),
                    builtin_entry("market", "workspace.market"),
                ],
            ),
            section("recent", vec![builtin_entry("recent_items", "home.recent")]),
        ],
        NavScope::Project => {
            let mut entries = Vec::new();
            for (id, page) in [
                ("overview", "project.overview"),
                ("issues", "project.issues"),
                ("board", "project.board"),
                ("cycles", "project.cycles"),
                ("forms", "project.forms"),
                ("apps", "project.apps"),
                ("settings", "project.settings"),
            ] {
                if builtin_applies(page, project_type) {
                    entries.push(builtin_entry(id, page));
                }
            }
            vec![section("main", entries)]
        }
    };
    NavDoc {
        schema: SCHEMA.to_string(),
        scope: scope.as_str().to_string(),
        sections,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[allow(clippy::needless_pass_by_value)]
    fn doc(entries: Value) -> Value {
        json!({"schema": "sv.nav.v1", "scope": "project", "sections": [{"id": "main", "entries": entries}]})
    }

    #[allow(clippy::needless_pass_by_value)]
    fn entry(id: &str, target: Value) -> Value {
        json!({"id": id, "target": target})
    }

    fn reason(raw: &Value) -> (&'static str, &'static str) {
        let error = parse(raw, NavScope::Project).expect_err("the document is refused");
        (error.code, error.reason)
    }

    #[test]
    fn the_defaults_validate_for_their_scope() {
        for scope in [NavScope::Workspace, NavScope::Project] {
            let value = serde_json::to_value(defaults(scope, None)).expect("serialises");
            assert!(parse(&value, scope).is_ok(), "{scope:?}");
        }
    }

    #[test]
    fn a_custom_form_project_has_no_work_item_defaults() {
        let doc = defaults(NavScope::Project, Some("custom_form"));
        let pages: Vec<_> = doc
            .entries()
            .filter_map(|(_, e)| match &e.target {
                Target::Builtin { page } => Some(page.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(pages, ["project.forms", "project.apps", "project.settings"]);
    }

    #[test]
    fn external_links_need_http_or_https_a_host_and_no_userinfo() {
        for good in ["https://wiki.example.com/handbook", "http://intranet.local:8080/x?a=b"] {
            assert!(external_url_ok(good), "{good}");
        }
        for bad in [
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://example.com/",
            "https://user:pw@example.com/",
            "https://user@example.com/",
            "//example.com",
            " https://example.com",
            "example.com",
        ] {
            assert!(!external_url_ok(bad), "{bad}");
        }
        assert!(!external_url_ok(&format!("https://example.com/{}", "a".repeat(2048))));
    }

    #[test]
    fn invalid_documents_name_their_reason() {
        let ext = |url: &str| entry("a", json!({"kind": "external", "url": url}));
        assert_eq!(
            reason(&doc(json!([ext("javascript:x")]))),
            ("nav_invalid", "external_url_invalid")
        );
        let dup = doc(json!([
            entry("a", json!({"kind": "builtin", "page": "project.issues"})),
            entry("a", json!({"kind": "builtin", "page": "project.board"}))
        ]));
        assert_eq!(reason(&dup), ("nav_invalid", "duplicate_id"));
        let site = doc(json!([entry("a", json!({"kind": "site_page", "route": "/x"}))]));
        assert_eq!(reason(&site), ("target_kind_unavailable", "target_kind_unavailable"));
        let action = doc(json!([entry("a", json!({"kind": "plugin_action", "action": "x"}))]));
        assert_eq!(reason(&action).0, "target_kind_unavailable");
        let many: Vec<Value> = (0..201)
            .map(|i| entry(&format!("e{i}"), json!({"kind": "external", "url": "https://x.test/"})))
            .collect();
        assert_eq!(reason(&doc(Value::Array(many))), ("nav_invalid", "too_large"));
        let unknown = doc(json!([entry("a", json!({"kind": "builtin", "page": "project.nope"}))]));
        assert_eq!(reason(&unknown), ("nav_invalid", "unknown_builtin"));
        let wrong = doc(json!([entry("a", json!({"kind": "builtin", "page": "home.overview"}))]));
        assert_eq!(reason(&wrong), ("nav_invalid", "builtin_wrong_scope"));
        let twice = doc(json!([
            entry("a", json!({"kind": "builtin", "page": "project.board"})),
            entry("b", json!({"kind": "builtin", "page": "project.board"}))
        ]));
        assert_eq!(reason(&twice), ("nav_invalid", "builtin_repeated"));
    }

    #[test]
    fn ids_labels_icons_and_scope_are_checked() {
        let target = json!({"kind": "builtin", "page": "project.board"});
        let with = |extra: Value| {
            let mut e = entry("a", target.clone());
            e.as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            doc(json!([e]))
        };
        assert_eq!(
            reason(&doc(json!([entry("_pinned", target.clone())]))),
            ("nav_invalid", "schema")
        );
        assert_eq!(
            reason(&doc(json!([entry("Upper", target.clone())]))),
            ("nav_invalid", "schema")
        );
        assert_eq!(
            reason(&with(json!({"label": {"english": "x"}}))),
            ("nav_invalid", "schema")
        );
        assert_eq!(reason(&with(json!({"label": {"en": ""}}))), ("nav_invalid", "schema"));
        assert_eq!(
            reason(&with(json!({"icon": "no-such-glyph"}))),
            ("nav_invalid", "schema")
        );
        assert!(
            parse(
                &with(json!({"label": {"en": "Board", "zh-CN": "看板"}, "icon": "kanban"})),
                NavScope::Project
            )
            .is_ok()
        );
        let workspace = json!({"schema": "sv.nav.v1", "scope": "workspace", "sections": []});
        assert_eq!(reason(&workspace), ("nav_invalid", "schema"));
        let plugin = doc(json!([entry(
            "a",
            json!({"kind": "plugin_app", "package_key": "acme/desk", "route": "board"})
        )]));
        assert!(parse(&plugin, NavScope::Project).is_ok());
        assert_eq!(
            parse(&plugin, NavScope::Workspace).expect_err("refused").code,
            "nav_invalid"
        );
    }

    #[test]
    fn a_document_over_the_size_limit_is_too_large() {
        let big = doc(json!([entry(
            "a",
            json!({"kind": "external", "url": format!("https://x.test/{}", "a".repeat(70_000))})
        )]));
        assert_eq!(reason(&big), ("nav_invalid", "too_large"));
    }
}
