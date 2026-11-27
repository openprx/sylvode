//! Slot descriptors: parent-declared typed regions that placements fill.

use std::collections::BTreeMap;

use serde::Serialize;

/// Slot kind. The DSH `chain` and `hoc` kinds are not adopted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotKind {
    /// One occupant per scope.
    Single,
    /// Ordered list of occupants, up to `max_entries`.
    List,
    /// One occupant per key; the id ends in `.<key>`.
    Keyed,
}

/// Descriptor of a slot, as served by `GET /api/v1/slots`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotDescriptor {
    /// Slot id.
    pub id: &'static str,
    /// Kind.
    pub kind: SlotKind,
    /// Context member to type name.
    pub context_schema: BTreeMap<&'static str, &'static str>,
    /// Layout parameter to context member.
    pub params_from_context: BTreeMap<&'static str, &'static str>,
    /// Component types allowed at the root of a layout placed here.
    pub allowed_roots: Vec<&'static str>,
    /// When the root is a `stack`, the component types its direct children may have
    /// (empty: unrestricted).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stack_children: Vec<&'static str>,
    /// Allowed action kinds.
    pub allowed_actions: Vec<&'static str>,
    /// Maximum number of entries.
    pub max_entries: u32,
    /// Scope kinds a placement may use.
    pub scope_kinds: Vec<&'static str>,
}

fn map(pairs: &[(&'static str, &'static str)]) -> BTreeMap<&'static str, &'static str> {
    pairs.iter().copied().collect()
}

/// The static slot table (MVP slots).
#[must_use]
pub fn slots() -> Vec<SlotDescriptor> {
    let record_ctx = || map(&[("form_id", "string"), ("record_id", "string")]);
    vec![
        SlotDescriptor {
            id: "form.record.detail",
            kind: SlotKind::List,
            context_schema: record_ctx(),
            params_from_context: map(&[("record_id", "record_id")]),
            allowed_roots: vec!["stack", "card", "grid", "tabs"],
            stack_children: vec![],
            allowed_actions: vec!["invoke", "submit", "navigate"],
            max_entries: 8,
            scope_kinds: vec!["workspace", "project", "form"],
        },
        SlotDescriptor {
            id: "form.record.actions",
            kind: SlotKind::List,
            context_schema: record_ctx(),
            params_from_context: map(&[("record_id", "record_id")]),
            allowed_roots: vec!["button", "stack"],
            stack_children: vec!["button"],
            allowed_actions: vec!["invoke", "submit", "navigate"],
            max_entries: 8,
            scope_kinds: vec!["workspace", "project", "form"],
        },
        SlotDescriptor {
            id: "project.home.widgets",
            kind: SlotKind::List,
            context_schema: map(&[("project_id", "string")]),
            params_from_context: map(&[("project_id", "project_id")]),
            allowed_roots: vec!["card", "stat", "table", "text", "markdown"],
            stack_children: vec![],
            allowed_actions: vec!["invoke", "submit", "navigate"],
            max_entries: 8,
            scope_kinds: vec!["workspace", "project"],
        },
        // A full console page reached from a navigation entry. Never placed (`max_entries` 0): it
        // exists so a layout can be validated for a page and so the page route knows its context.
        SlotDescriptor {
            id: "page",
            kind: SlotKind::Single,
            context_schema: map(&[("workspace_id", "string"), ("project_id", "string?")]),
            params_from_context: map(&[("project_id", "project_id")]),
            allowed_roots: vec![
                "stack",
                "grid",
                "card",
                "tabs",
                "text",
                "markdown",
                "stat",
                "badge",
                "table",
                "field-display",
                "form",
                "button",
            ],
            stack_children: vec![],
            allowed_actions: vec!["invoke", "submit", "navigate"],
            max_entries: 0,
            scope_kinds: vec!["workspace", "project"],
        },
    ]
}

/// Looks up a slot by id.
#[must_use]
pub fn slot(id: &str) -> Option<SlotDescriptor> {
    slots().into_iter().find(|s| s.id == id)
}
