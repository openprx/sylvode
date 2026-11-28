//! The layout validator: walks the raw JSON, reports every error with a JSON-pointer path.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::analysis::{Analysis, analyse};
use crate::components::{self, ChildrenRule, ComponentProps, typed_props};
use crate::model::{Action, DataSource, Layout, SCHEMA_ID};
use crate::refs::{FormRef, RefRoot, is_ident, parse_form_ref, parse_ref};
use crate::slots::SlotDescriptor;

/// Maximum serialised spec size.
pub const MAX_SPEC_BYTES: usize = 256 * 1024;
/// Maximum number of nodes.
pub const MAX_NODES: usize = 500;
/// Maximum tree depth (the root has depth 1).
pub const MAX_DEPTH: usize = 24;
/// Maximum number of data sources.
pub const MAX_DATA_SOURCES: usize = 16;
/// Maximum number of actions.
pub const MAX_ACTIONS: usize = 16;
/// Maximum `page_size` of a query source.
pub const MAX_PAGE_SIZE: u32 = 200;
const MAX_MARKDOWN_BYTES: usize = 32 * 1024;
const MAX_EXPR_CHARS: usize = 1024;

/// Machine-readable error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// `schema` is not `sv.layout.v1`.
    SchemaUnsupported,
    /// Structurally wrong JSON (wrong type, unknown or missing member). Not in C3's list.
    ShapeInvalid,
    /// `key` does not match the key grammar.
    KeyInvalid,
    /// A param, source or action name is not a valid identifier. Not in C3's list.
    NameInvalid,
    /// Unknown component type.
    NodeTypeUnknown,
    /// Props do not fit the component.
    PropsInvalid,
    /// Children do not fit the component.
    ChildrenInvalid,
    /// Malformed reference.
    RefInvalid,
    /// Reference to an undeclared data source or parameter.
    RefUnknownSource,
    /// Reference to an undeclared action.
    ActionUnknown,
    /// Plugin sources are not linked in the MVP.
    SourceNotAvailable,
    /// A size, count, depth or page-size limit is exceeded.
    LimitExceeded,
    /// The slot does not allow this node or action.
    SlotNodeNotAllowed,
    /// The action kind is not allowed in a site layout (`invoke`; non-route navigation).
    ActionNotAllowedOnSite,
    /// A data source does not resolve to a binding of the site.
    BindingMissing,
    /// A CEL expression was refused by `sv-rules` (`cel_syntax`, `cel_unknown_variable`, ...).
    Cel(&'static str),
}

impl ErrorCode {
    /// The wire name (`snake_case`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaUnsupported => "schema_unsupported",
            Self::ShapeInvalid => "shape_invalid",
            Self::KeyInvalid => "key_invalid",
            Self::NameInvalid => "name_invalid",
            Self::NodeTypeUnknown => "node_type_unknown",
            Self::PropsInvalid => "props_invalid",
            Self::ChildrenInvalid => "children_invalid",
            Self::RefInvalid => "ref_invalid",
            Self::RefUnknownSource => "ref_unknown_source",
            Self::ActionUnknown => "action_unknown",
            Self::SourceNotAvailable => "source_not_available",
            Self::LimitExceeded => "limit_exceeded",
            Self::SlotNodeNotAllowed => "slot_node_not_allowed",
            Self::ActionNotAllowedOnSite => "action_not_allowed_on_site",
            Self::BindingMissing => "binding_missing",
            Self::Cel(code) => code,
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One validation error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayoutError {
    /// JSON pointer to the offending value.
    pub path: String,
    /// Error code.
    pub code: ErrorCode,
    /// Human-readable detail (English).
    pub message: String,
}

/// A layout that passed validation, with its static analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    /// The typed layout.
    pub layout: Layout,
    /// Capabilities, packages and forms it uses.
    pub analysis: Analysis,
}

/// Where the layout will run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Context {
    /// The console: all action kinds, `$bind`/`form:` form references.
    #[default]
    Console,
    /// A public site: only `submit` and route `navigate`; every data source must name one of
    /// these binding keys (as `$bind.<key>` after resolution, or `form:<key>`).
    Site {
        /// Binding keys of the site.
        bindings: BTreeSet<String>,
    },
}

/// Validation options.
#[derive(Debug, Clone, Default)]
pub struct Options<'a> {
    /// Slot the layout will be placed in.
    pub slot: Option<&'a SlotDescriptor>,
    /// Console or site.
    pub context: Context,
}

/// Validates raw bytes (size limit first, then JSON syntax, then [`validate`]).
///
/// # Errors
/// Returns every error found.
pub fn validate_bytes(bytes: &[u8], opts: &Options<'_>) -> Result<Validated, Vec<LayoutError>> {
    if bytes.len() > MAX_SPEC_BYTES {
        return Err(vec![err("", ErrorCode::LimitExceeded, "spec larger than 256 KiB")]);
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => validate(&v, opts),
        Err(e) => Err(vec![err("", ErrorCode::ShapeInvalid, &format!("invalid JSON: {e}"))]),
    }
}

/// Validates a parsed spec, optionally against the slot it will be placed in.
///
/// # Errors
/// Returns every error found, each with a JSON-pointer path.
pub fn validate(spec: &Value, opts: &Options<'_>) -> Result<Validated, Vec<LayoutError>> {
    let mut cx = Cx {
        context: opts.context.clone(),
        ..Cx::default()
    };
    cx.run(spec, opts.slot);
    if !cx.errors.is_empty() {
        return Err(cx.errors);
    }
    match serde_json::from_value::<Layout>(spec.clone()) {
        Ok(layout) => {
            let analysis = analyse(&layout);
            Ok(Validated { layout, analysis })
        }
        Err(e) => Err(vec![err("", ErrorCode::ShapeInvalid, &e.to_string())]),
    }
}

fn err(path: &str, code: ErrorCode, message: &str) -> LayoutError {
    LayoutError {
        path: path.to_owned(),
        code,
        message: message.to_owned(),
    }
}

fn esc(seg: &str) -> String {
    seg.replace('~', "~0").replace('/', "~1")
}

fn join(base: &str, seg: &str) -> String {
    format!("{base}/{}", esc(seg))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Record,
    Query,
    Form,
    Plugin,
}

impl Default for Cx {
    fn default() -> Self {
        Self {
            context: Context::Console,
            errors: Vec::new(),
            sources: BTreeMap::new(),
            actions: BTreeMap::new(),
            params: BTreeSet::new(),
            nodes: 0,
            node_limit_reported: false,
            depth_reported: false,
        }
    }
}

struct Cx {
    context: Context,
    errors: Vec<LayoutError>,
    sources: BTreeMap<String, (SourceKind, Vec<String>)>,
    actions: BTreeMap<String, &'static str>,
    params: BTreeSet<String>,
    nodes: usize,
    node_limit_reported: bool,
    depth_reported: bool,
}

impl Cx {
    fn push(&mut self, path: &str, code: ErrorCode, message: &str) {
        self.errors.push(err(path, code, message));
    }

    fn run(&mut self, spec: &Value, slot: Option<&SlotDescriptor>) {
        let Some(obj) = spec.as_object() else {
            self.push("", ErrorCode::ShapeInvalid, "layout must be a JSON object");
            return;
        };
        match serde_json::to_vec(spec) {
            Ok(b) if b.len() > MAX_SPEC_BYTES => {
                self.push("", ErrorCode::LimitExceeded, "spec larger than 256 KiB");
                return;
            }
            _ => {}
        }
        if obj.get("schema").and_then(Value::as_str) != Some(SCHEMA_ID) {
            self.push(
                "/schema",
                ErrorCode::SchemaUnsupported,
                "schema must be \"sv.layout.v1\"",
            );
            return;
        }
        self.keys(
            "",
            obj,
            &["schema", "key", "title", "params", "data", "actions", "root"],
        );
        self.key(obj);
        self.title(obj);
        self.params(obj);
        self.collect_names(obj);
        self.data(obj);
        self.actions(obj, slot);
        match obj.get("root") {
            Some(root) => {
                self.node(root, "/root", 1, None);
                if let Some(slot) = slot {
                    self.slot_root(root, slot);
                }
            }
            None => self.push("/root", ErrorCode::ShapeInvalid, "root is required"),
        }
    }

    fn keys(&mut self, path: &str, obj: &Map<String, Value>, allowed: &[&str]) {
        for k in obj.keys() {
            if !allowed.contains(&k.as_str()) {
                self.push(&join(path, k), ErrorCode::ShapeInvalid, "unknown member");
            }
        }
    }

    fn key(&mut self, obj: &Map<String, Value>) {
        match obj.get("key").and_then(Value::as_str) {
            Some(k) if key_ok(k) => {}
            Some(_) => self.push("/key", ErrorCode::KeyInvalid, "key does not match the key grammar"),
            None => self.push("/key", ErrorCode::KeyInvalid, "key is required and must be a string"),
        }
    }

    fn title(&mut self, obj: &Map<String, Value>) {
        match obj.get("title") {
            Some(v) => self.text(v, "/title", false),
            None => self.push("/title", ErrorCode::ShapeInvalid, "title is required"),
        }
    }

    /// Checks a text value: literal, i18n map with `en`, or (when allowed) `{cel}`. References
    /// are checked for syntax and, when `refs` is true, resolved against the declarations later.
    fn text(&mut self, v: &Value, path: &str, cel_allowed: bool) {
        match v {
            Value::String(s) => self.ref_syntax(s, path),
            Value::Object(m) => {
                if let Some(expr) = m.get("cel") {
                    if !cel_allowed || m.len() != 1 {
                        self.push(path, ErrorCode::ShapeInvalid, "computed text is not allowed here");
                    } else {
                        self.expr(expr, &join(path, "cel"));
                    }
                    return;
                }
                if !m.get("en").is_some_and(Value::is_string) {
                    self.push(path, ErrorCode::ShapeInvalid, "i18n map needs a string `en`");
                }
                for (k, t) in m {
                    match t.as_str() {
                        Some(s) => self.ref_syntax(s, &join(path, k)),
                        None => self.push(&join(path, k), ErrorCode::ShapeInvalid, "text must be a string"),
                    }
                }
            }
            _ => self.push(path, ErrorCode::ShapeInvalid, "text must be a string or an i18n map"),
        }
    }

    fn expr(&mut self, v: &Value, path: &str) {
        match v.as_str() {
            Some(s) if !s.trim().is_empty() && s.chars().count() <= MAX_EXPR_CHARS => {}
            Some(_) => self.push(
                path,
                ErrorCode::LimitExceeded,
                "expression empty or longer than 1024 characters",
            ),
            None => {
                self.push(path, ErrorCode::ShapeInvalid, "expression must be a string");
                return;
            }
        }
        if let Some(src) = v
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.chars().count() <= MAX_EXPR_CHARS)
            && let Err(e) = sv_rules::compile(src, sv_rules::Env::LayoutWhen)
        {
            self.push(path, ErrorCode::Cel(e.code), &format!("expression refused: {}", e.code));
        }
    }

    /// Strings starting with `$` must be valid references, then resolved.
    fn ref_syntax(&mut self, s: &str, path: &str) {
        if !s.starts_with('$') {
            return;
        }
        let Some(r) = parse_ref(s) else {
            self.push(path, ErrorCode::RefInvalid, "malformed reference");
            return;
        };
        match (r.root, r.first) {
            (RefRoot::Data, Some(name)) if !self.sources.contains_key(&name) => {
                self.push(
                    path,
                    ErrorCode::RefUnknownSource,
                    "reference to an undeclared data source",
                );
            }
            (RefRoot::Data, None) => {
                self.push(path, ErrorCode::RefInvalid, "$data needs a source name");
            }
            (RefRoot::Params, Some(name)) if !self.params.contains(&name) => {
                self.push(
                    path,
                    ErrorCode::RefUnknownSource,
                    "reference to an undeclared parameter",
                );
            }
            (RefRoot::Actions, Some(name)) if !self.actions.contains_key(&name) => {
                self.push(path, ErrorCode::ActionUnknown, "reference to an undeclared action");
            }
            (RefRoot::Actions, None) => {
                self.push(path, ErrorCode::RefInvalid, "$actions needs an action name");
            }
            _ => {}
        }
    }

    fn form_ref(&mut self, v: Option<&Value>, path: &str) {
        match v.and_then(Value::as_str) {
            Some(s) if parse_form_ref(s).is_some() => {}
            Some(_) => self.push(
                path,
                ErrorCode::RefInvalid,
                "form must be `$bind.<name>` or `form:<key>`",
            ),
            None => self.push(path, ErrorCode::ShapeInvalid, "form reference is required"),
        }
    }

    fn params(&mut self, obj: &Map<String, Value>) {
        let Some(v) = obj.get("params") else { return };
        let Some(m) = v.as_object() else {
            self.push("/params", ErrorCode::ShapeInvalid, "params must be an object");
            return;
        };
        for (name, def) in m {
            let p = join("/params", name);
            if !is_ident(name) {
                self.push(&p, ErrorCode::NameInvalid, "parameter name must match [a-z_][a-z0-9_]*");
            }
            if serde_json::from_value::<crate::model::ParamDef>(def.clone()).is_err() {
                self.push(
                    &p,
                    ErrorCode::ShapeInvalid,
                    "expected {\"type\": string|int|bool, \"required\": bool}",
                );
            }
        }
    }

    /// Records declared names first so references resolve regardless of member order.
    fn collect_names(&mut self, obj: &Map<String, Value>) {
        if let Some(m) = obj.get("params").and_then(Value::as_object) {
            self.params = m.keys().cloned().collect();
        }
        if let Some(m) = obj.get("data").and_then(Value::as_object) {
            for (name, v) in m {
                let kind = match v.get("source").and_then(Value::as_str) {
                    Some("record") => SourceKind::Record,
                    Some("query") => SourceKind::Query,
                    Some("form") => SourceKind::Form,
                    Some("plugin") => SourceKind::Plugin,
                    _ => continue,
                };
                let fields = v
                    .get("fields")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                self.sources.insert(name.clone(), (kind, fields));
            }
        }
        if let Some(m) = obj.get("actions").and_then(Value::as_object) {
            for (name, v) in m {
                let kind = match v.get("kind").and_then(Value::as_str) {
                    Some("invoke") => "invoke",
                    Some("submit") => "submit",
                    Some("navigate") => "navigate",
                    _ => continue,
                };
                self.actions.insert(name.clone(), kind);
            }
        }
    }

    fn data(&mut self, obj: &Map<String, Value>) {
        let Some(v) = obj.get("data") else { return };
        let Some(m) = v.as_object() else {
            self.push("/data", ErrorCode::ShapeInvalid, "data must be an object");
            return;
        };
        if m.len() > MAX_DATA_SOURCES {
            self.push("/data", ErrorCode::LimitExceeded, "more than 16 data sources");
        }
        for (name, src) in m {
            let p = join("/data", name);
            if !is_ident(name) {
                self.push(&p, ErrorCode::NameInvalid, "source name must match [a-z_][a-z0-9_]*");
            }
            self.source(src, &p);
        }
    }

    fn source(&mut self, src: &Value, p: &str) {
        let Some(m) = src.as_object() else {
            self.push(p, ErrorCode::ShapeInvalid, "data source must be an object");
            return;
        };
        let allowed: &[&str] = match m.get("source").and_then(Value::as_str) {
            Some("record") => &["source", "form", "record", "fields"],
            Some("query") => &["source", "form", "filter", "sort", "fields", "page_size"],
            Some("form") => &["source", "form", "fields"],
            Some("plugin") => {
                self.push(
                    p,
                    ErrorCode::SourceNotAvailable,
                    "plugin sources are not available in this version",
                );
                return;
            }
            _ => {
                self.push(
                    &join(p, "source"),
                    ErrorCode::ShapeInvalid,
                    "source must be record, query, form or plugin",
                );
                return;
            }
        };
        self.keys(p, m, allowed);
        if serde_json::from_value::<DataSource>(src.clone()).is_err() {
            self.push(p, ErrorCode::ShapeInvalid, "data source members have the wrong shape");
        }
        self.form_ref(m.get("form"), &join(p, "form"));
        self.site_binding(m.get("form"), &join(p, "form"));
        if let Some(r) = m.get("record") {
            self.ref_member(r, &join(p, "record"));
        }
        if let Some(f) = m.get("filter").and_then(Value::as_array) {
            for (i, item) in f.iter().enumerate() {
                if let Some(val) = item.get("value") {
                    self.value_refs(val, &format!("{}/{i}/value", join(p, "filter")));
                }
            }
        }
        if let Some(n) = m.get("page_size").and_then(Value::as_u64)
            && (n == 0 || n > u64::from(MAX_PAGE_SIZE))
        {
            self.push(
                &join(p, "page_size"),
                ErrorCode::LimitExceeded,
                "page_size must be 1..=200",
            );
        }
    }

    /// In a site context the form reference must name a binding of the site.
    fn site_binding(&mut self, v: Option<&Value>, path: &str) {
        let Context::Site { bindings } = &self.context else {
            return;
        };
        let Some(FormRef::Bind(name) | FormRef::Key(name)) = v.and_then(Value::as_str).and_then(parse_form_ref) else {
            return;
        };
        if !bindings.contains(&name) {
            self.push(
                path,
                ErrorCode::BindingMissing,
                "data source does not resolve to a binding of the site",
            );
        }
    }

    /// A member that must be a reference.
    fn ref_member(&mut self, v: &Value, path: &str) {
        match v.as_str() {
            Some(s) if s.starts_with('$') => self.ref_syntax(s, path),
            _ => self.push(path, ErrorCode::RefInvalid, "a reference starting with `$` is required"),
        }
    }

    /// Syntax is checked with the other string leaves; this only demands a reference.
    fn require_ref(&mut self, s: &str, path: &str) {
        if !s.starts_with('$') {
            self.push(path, ErrorCode::RefInvalid, "a reference starting with `$` is required");
        }
    }

    fn value_refs(&mut self, v: &Value, path: &str) {
        match v {
            Value::String(s) => self.ref_syntax(s, path),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    self.value_refs(x, &format!("{path}/{i}"));
                }
            }
            Value::Object(m) => {
                for (k, x) in m {
                    self.value_refs(x, &join(path, k));
                }
            }
            _ => {}
        }
    }

    fn actions(&mut self, obj: &Map<String, Value>, slot: Option<&SlotDescriptor>) {
        let Some(v) = obj.get("actions") else { return };
        let Some(m) = v.as_object() else {
            self.push("/actions", ErrorCode::ShapeInvalid, "actions must be an object");
            return;
        };
        if m.len() > MAX_ACTIONS {
            self.push("/actions", ErrorCode::LimitExceeded, "more than 16 actions");
        }
        for (name, a) in m {
            let p = join("/actions", name);
            if !is_ident(name) {
                self.push(&p, ErrorCode::NameInvalid, "action name must match [a-z_][a-z0-9_]*");
            }
            self.action(a, &p, slot);
        }
    }

    fn action(&mut self, a: &Value, p: &str, slot: Option<&SlotDescriptor>) {
        let Some(m) = a.as_object() else {
            self.push(p, ErrorCode::ShapeInvalid, "action must be an object");
            return;
        };
        let allowed: &[&str] = match m.get("kind").and_then(Value::as_str) {
            Some("invoke") => &["kind", "package", "export", "input", "confirm"],
            Some("submit") => &["kind", "data", "mode", "record", "confirm"],
            Some("navigate") => &["kind", "to"],
            _ => {
                self.push(
                    &join(p, "kind"),
                    ErrorCode::ShapeInvalid,
                    "kind must be invoke, submit or navigate",
                );
                return;
            }
        };
        self.keys(p, m, allowed);
        let parsed = serde_json::from_value::<Action>(a.clone());
        let Ok(action) = parsed else {
            self.push(p, ErrorCode::ShapeInvalid, "action members have the wrong shape");
            return;
        };
        if let Some(slot) = slot
            && !slot.allowed_actions.contains(&action.kind_name())
        {
            self.push(
                &join(p, "kind"),
                ErrorCode::SlotNodeNotAllowed,
                "action kind not allowed in this slot",
            );
        }
        if matches!(self.context, Context::Site { .. }) {
            match &action {
                Action::Invoke { .. } => self.push(
                    &join(p, "kind"),
                    ErrorCode::ActionNotAllowedOnSite,
                    "invoke is not allowed in a site layout",
                ),
                Action::Navigate { to } if !matches!(to, crate::model::NavTarget::Route(_)) => self.push(
                    &join(p, "to"),
                    ErrorCode::ActionNotAllowedOnSite,
                    "only route navigation is allowed in a site layout",
                ),
                _ => {}
            }
        }
        if let Some(c) = m.get("confirm") {
            self.text(c, &join(p, "confirm"), false);
        }
        match action {
            Action::Invoke { input, .. } => {
                for (k, v) in &input {
                    self.value_refs(v, &join(&join(p, "input"), k));
                }
            }
            Action::Submit { data, record, .. } => {
                match self.sources.get(&data).map(|s| s.0) {
                    Some(SourceKind::Form) => {}
                    Some(_) => self.push(
                        &join(p, "data"),
                        ErrorCode::RefUnknownSource,
                        "submit data must name a `form` source",
                    ),
                    None => self.push(
                        &join(p, "data"),
                        ErrorCode::RefUnknownSource,
                        "submit data names an undeclared source",
                    ),
                }
                if let Some(r) = record {
                    self.ref_member(&Value::String(r), &join(p, "record"));
                }
            }
            Action::Navigate { to } => self.nav(&to, p),
        }
    }

    fn nav(&mut self, to: &crate::model::NavTarget, p: &str) {
        use crate::model::NavTarget as N;
        let tp = join(p, "to");
        match to {
            N::Record(t) => {
                self.form_ref(Some(&Value::String(t.form.clone())), &join(&tp, "form"));
                self.ref_member(&Value::String(t.id.clone()), &join(&tp, "id"));
            }
            N::Form(f) => self.form_ref(Some(&Value::String(f.clone())), &tp),
            N::View(t) => {
                self.form_ref(Some(&Value::String(t.form.clone())), &join(&tp, "form"));
            }
            N::Route(r) => {
                let ok = r.starts_with('/') && !r.starts_with("//") && !r.contains("://");
                if !ok {
                    self.push(&tp, ErrorCode::RefInvalid, "route must be an absolute site path");
                }
            }
        }
    }

    fn node(&mut self, v: &Value, path: &str, depth: usize, grid_columns: Option<u64>) {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            if !self.node_limit_reported {
                self.node_limit_reported = true;
                self.push(path, ErrorCode::LimitExceeded, "more than 500 nodes");
            }
            return;
        }
        if depth > MAX_DEPTH {
            if !self.depth_reported {
                self.depth_reported = true;
                self.push(path, ErrorCode::LimitExceeded, "tree deeper than 24 levels");
            }
            return;
        }
        let Some(m) = v.as_object() else {
            self.push(path, ErrorCode::ShapeInvalid, "node must be an object");
            return;
        };
        self.keys(path, m, &["type", "props", "when", "children", "on_click", "col_span"]);
        self.col_span(m, grid_columns, path);
        if let Some(w) = m.get("when") {
            self.expr(w, &join(path, "when"));
        }
        let Some(kind) = m.get("type").and_then(Value::as_str) else {
            self.push(
                &join(path, "type"),
                ErrorCode::NodeTypeUnknown,
                "node needs a string `type`",
            );
            return;
        };
        let children = match m.get("children") {
            None => &[][..],
            Some(Value::Array(a)) => a.as_slice(),
            Some(_) => {
                self.push(
                    &join(path, "children"),
                    ErrorCode::ChildrenInvalid,
                    "children must be an array",
                );
                &[][..]
            }
        };
        let Some(entry) = components::entry(kind) else {
            self.push(
                &join(path, "type"),
                ErrorCode::NodeTypeUnknown,
                "unknown component type",
            );
            return;
        };
        let props_path = join(path, "props");
        self.props(entry, m.get("props"), &props_path, kind);
        self.children_rule(entry.descriptor.children, kind, m.get("props"), children.len(), path);
        self.events(m, kind, path);
        let columns = (kind == "grid").then(|| grid_columns_of(m.get("props")));
        for (i, c) in children.iter().enumerate() {
            self.node(c, &format!("{}/{i}", join(path, "children")), depth + 1, columns);
        }
    }

    fn props(&mut self, entry: &components::Entry, props: Option<&Value>, path: &str, kind: &str) {
        let empty = Value::Object(Map::new());
        let props = match props {
            Some(p) => p,
            None if components::props_optional(kind) => &empty,
            None => {
                self.push(path, ErrorCode::PropsInvalid, "props are required for this component");
                return;
            }
        };
        let mut schema_errors = 0usize;
        if let Some(validator) = &entry.validator {
            for e in validator.iter_errors(props) {
                schema_errors += 1;
                let sub = e.instance_path().to_string();
                self.push(&format!("{path}{sub}"), ErrorCode::PropsInvalid, &e.to_string());
            }
        }
        if schema_errors > 0 {
            return;
        }
        let node = crate::model::Node {
            kind: kind.to_owned(),
            props: props.clone(),
            when: None,
            children: Vec::new(),
            on_click: None,
            col_span: None,
        };
        match typed_props(&node) {
            Ok(Some(typed)) => self.props_semantics(&typed, path),
            Ok(None) => {}
            Err(e) => self.push(path, ErrorCode::PropsInvalid, &e.to_string()),
        }
        self.prop_texts(props, path);
    }

    /// Reference syntax and resolution for every string leaf of the props.
    fn prop_texts(&mut self, v: &Value, path: &str) {
        match v {
            Value::String(s) => self.ref_syntax(s, path),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    self.prop_texts(x, &format!("{path}/{i}"));
                }
            }
            Value::Object(m) => {
                if m.len() == 1
                    && let Some(expr) = m.get("cel")
                {
                    self.expr(expr, &join(path, "cel"));
                    return;
                }
                for (k, x) in m {
                    self.prop_texts(x, &join(path, k));
                }
            }
            _ => {}
        }
    }

    fn props_semantics(&mut self, typed: &ComponentProps, path: &str) {
        match typed {
            ComponentProps::Markdown(p) => {
                let size = match &p.value {
                    crate::model::Text::Literal(s) => s.len(),
                    crate::model::Text::I18n(m) => m.values().map(String::len).max().unwrap_or(0),
                    crate::model::Text::Computed(_) => 0,
                };
                if size > MAX_MARKDOWN_BYTES {
                    self.push(
                        &join(path, "value"),
                        ErrorCode::LimitExceeded,
                        "markdown larger than 32 KiB",
                    );
                }
            }
            ComponentProps::Stat(p) => {
                if let Some(t) = &p.trend {
                    self.require_ref(t, &join(path, "trend"));
                }
            }
            ComponentProps::Table(p) => {
                self.require_ref(&p.rows, &join(path, "rows"));
                if let Some(a) = &p.row_click {
                    self.action_name(a, None, &join(path, "row_click"));
                }
            }
            ComponentProps::FieldDisplay(p) => {
                self.require_ref(&p.record, &join(path, "record"));
            }
            ComponentProps::Form(p) => {
                match self.sources.get(&p.data) {
                    Some((SourceKind::Form, fields)) => {
                        let fields = fields.clone();
                        self.form_fields(&p.fields, &fields, path);
                    }
                    _ => self.push(
                        &join(path, "data"),
                        ErrorCode::RefUnknownSource,
                        "form data must name a `form` source",
                    ),
                }
                self.action_name(&p.submit, Some("submit"), &join(path, "submit"));
            }
            ComponentProps::Button(p) => {
                if let Some(icon) = &p.icon {
                    let ok = !icon.is_empty()
                        && icon
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
                    if !ok {
                        self.push(
                            &join(path, "icon"),
                            ErrorCode::PropsInvalid,
                            "icon must be a lowercase icon name",
                        );
                    }
                }
            }
            ComponentProps::Tabs(p) => {
                let mut seen = BTreeSet::new();
                for (i, t) in p.tabs.iter().enumerate() {
                    if !seen.insert(t.key.as_str()) {
                        self.push(
                            &format!("{}/{i}/key", join(path, "tabs")),
                            ErrorCode::PropsInvalid,
                            "duplicate tab key",
                        );
                    }
                }
            }
            ComponentProps::Stack(_)
            | ComponentProps::Grid(_)
            | ComponentProps::Card(_)
            | ComponentProps::Text(_)
            | ComponentProps::Badge(_) => {}
        }
    }

    fn form_fields(&mut self, wanted: &[String], source_fields: &[String], path: &str) {
        if source_fields.is_empty() {
            return;
        }
        for (i, f) in wanted.iter().enumerate() {
            if !source_fields.contains(f) {
                self.push(
                    &format!("{}/{i}", join(path, "fields")),
                    ErrorCode::PropsInvalid,
                    "field is not in the form source",
                );
            }
        }
    }

    fn action_name(&mut self, name: &str, want_kind: Option<&str>, path: &str) {
        match self.actions.get(name) {
            None => self.push(path, ErrorCode::ActionUnknown, "undeclared action"),
            Some(kind) => {
                if let Some(w) = want_kind
                    && *kind != w
                {
                    self.push(path, ErrorCode::ActionUnknown, &format!("action must be of kind {w}"));
                }
            }
        }
    }

    fn children_rule(&mut self, rule: ChildrenRule, kind: &str, props: Option<&Value>, n: usize, path: &str) {
        let p = join(path, "children");
        match rule {
            ChildrenRule::None if n > 0 => {
                self.push(&p, ErrorCode::ChildrenInvalid, &format!("{kind} takes no children"));
            }
            ChildrenRule::Any | ChildrenRule::None => {}
            ChildrenRule::Fixed => {
                let tabs = props
                    .and_then(|p| p.get("tabs"))
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if n != tabs {
                    self.push(&p, ErrorCode::ChildrenInvalid, "tabs needs exactly one child per tab");
                }
            }
        }
    }

    /// `col_span` is only valid on a direct child of a `grid` and at most the grid's columns.
    fn col_span(&mut self, m: &Map<String, Value>, grid_columns: Option<u64>, path: &str) {
        let Some(v) = m.get("col_span") else {
            return;
        };
        let p = join(path, "col_span");
        let Some(columns) = grid_columns else {
            self.push(
                &p,
                ErrorCode::ShapeInvalid,
                "col_span is only valid on a child of a grid",
            );
            return;
        };
        match v.as_u64() {
            Some(span) if (1..=columns).contains(&span) => {}
            Some(_) => self.push(
                &p,
                ErrorCode::ShapeInvalid,
                &format!("col_span must be between 1 and the grid's {columns} columns"),
            ),
            None => self.push(&p, ErrorCode::ShapeInvalid, "col_span must be a positive integer"),
        }
    }

    fn events(&mut self, m: &Map<String, Value>, kind: &str, path: &str) {
        if let Some(v) = m.get("on_click") {
            let p = join(path, "on_click");
            if kind != "button" {
                self.push(&p, ErrorCode::ShapeInvalid, "on_click is only valid on a button");
            } else if let Some(name) = v.as_str() {
                self.action_name(name, None, &p);
            } else {
                self.push(&p, ErrorCode::ShapeInvalid, "on_click must be an action name");
            }
        }
    }

    fn slot_root(&mut self, root: &Value, slot: &SlotDescriptor) {
        let Some(kind) = root.get("type").and_then(Value::as_str) else {
            return;
        };
        if !slot.allowed_roots.contains(&kind) {
            self.push(
                "/root/type",
                ErrorCode::SlotNodeNotAllowed,
                "root component not allowed in this slot",
            );
            return;
        }
        if kind == "stack"
            && !slot.stack_children.is_empty()
            && let Some(children) = root.get("children").and_then(Value::as_array)
        {
            for (i, c) in children.iter().enumerate() {
                let ck = c.get("type").and_then(Value::as_str).unwrap_or("");
                if !slot.stack_children.contains(&ck) {
                    self.push(
                        &format!("/root/children/{i}/type"),
                        ErrorCode::SlotNodeNotAllowed,
                        "child component not allowed in this slot",
                    );
                }
            }
        }
    }
}

/// Columns of a grid as the renderers lay it out: `columns` 1..=4, 2 when absent.
fn grid_columns_of(props: Option<&Value>) -> u64 {
    props
        .and_then(|p| p.get("columns"))
        .and_then(Value::as_u64)
        .filter(|c| (1..=4).contains(c))
        .unwrap_or(2)
}

/// `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*){0,4}$`
fn key_ok(k: &str) -> bool {
    let segs: Vec<&str> = k.split('.').collect();
    segs.len() <= 5
        && segs.iter().all(|s| {
            let mut c = s.chars();
            matches!(c.next(), Some(f) if f.is_ascii_lowercase())
                && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
        })
}
