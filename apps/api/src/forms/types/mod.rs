//! Field type descriptors: one unit struct per form field type, and the registry every schema
//! parse, value normalisation and outlet check dispatches through (contract C2 §4.1).
//!
//! A descriptor owns what is specific to its type: the type settings read from the raw field
//! object, the type check and canonical form of a value, the operand built-in rules compare, the
//! CEL binding of a value, the outlet table and the display mask. Everything else (merge, defaults,
//! read-only, the rule stages) is the rule pipeline in [`super::rules`], which never matches on a
//! type name.
//!
//! The remaining type-name matches outside this module (projections, export, import cell parsing,
//! calculation, the Flow bridge, plugin field filters) are listed in the batch report and move here
//! in P-next.

mod choice;
mod numeric;
mod reference;
mod temporal;
mod text;

use serde::Serialize;
use serde_json::{Map, Value, json};
use sv_rules::{Bound, Operand, RuleKind};

use super::schema::{AmountFieldConfig, FormField, SignatureFieldConfig};

/// How a type's value is stored and indexed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Text,
    Number,
    Bool,
    Date,
    DateTime,
    Json,
    Ref,
    Blob,
}

/// The CEL type of `value` / `record.<key>` (contract C2 §5.3). Timestamps are Unix seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CelType {
    String,
    Int,
    Bool,
    Timestamp,
    ListString,
}

/// Every place a field value can flow to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outlet {
    RecordRead,
    List,
    Filter,
    Sort,
    Group,
    Formula,
    ConditionSource,
    TitleTemplate,
    DefaultValue,
    Export,
    Import,
    EventPayload,
    PluginInput,
    FlowConversion,
    FlowSummary,
    SiteRead,
    SiteSubmit,
    Log,
}

impl Outlet {
    pub const ALL: &'static [Self] = &[
        Self::RecordRead,
        Self::List,
        Self::Filter,
        Self::Sort,
        Self::Group,
        Self::Formula,
        Self::ConditionSource,
        Self::TitleTemplate,
        Self::DefaultValue,
        Self::Export,
        Self::Import,
        Self::EventPayload,
        Self::PluginInput,
        Self::FlowConversion,
        Self::FlowSummary,
        Self::SiteRead,
        Self::SiteSubmit,
        Self::Log,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RecordRead => "record_read",
            Self::List => "list",
            Self::Filter => "filter",
            Self::Sort => "sort",
            Self::Group => "group",
            Self::Formula => "formula",
            Self::ConditionSource => "condition_source",
            Self::TitleTemplate => "title_template",
            Self::DefaultValue => "default_value",
            Self::Export => "export",
            Self::Import => "import",
            Self::EventPayload => "event_payload",
            Self::PluginInput => "plugin_input",
            Self::FlowConversion => "flow_conversion",
            Self::FlowSummary => "flow_summary",
            Self::SiteRead => "site_read",
            Self::SiteSubmit => "site_submit",
            Self::Log => "log",
        }
    }
}

/// What a type does at an outlet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutletRule {
    /// The value flows.
    Value,
    /// A presence marker flows instead of the value.
    Marker,
    /// The field is left out silently.
    Omit,
    /// Naming the field there is refused.
    Reject,
}

/// Type settings read from the raw field object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeSettings {
    None,
    Options {
        options: Vec<String>,
        disabled: Vec<String>,
        archived: Vec<String>,
    },
    Amount(AmountFieldConfig),
    Signature(SignatureFieldConfig),
}

/// A field definition refused at schema save. `path` is relative to the field object (`/amount`);
/// the caller prefixes `/fields/<index>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefError {
    pub path: String,
    pub code: &'static str,
    /// Fixed English detail for logs and the legacy string surface; never a submitted value.
    pub detail: String,
}

impl DefError {
    pub fn new(path: impl Into<String>, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            code,
            detail: detail.into(),
        }
    }

    /// `field_invalid` at `path`.
    pub fn invalid(path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(path, "field_invalid", detail)
    }
}

impl std::fmt::Display for DefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Why a value failed its type check. Codes only; the value is never echoed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeError {
    Mismatch,
    OptionUnsupported,
    OptionDisabled,
    OptionArchived,
}

impl TypeError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Mismatch => "type_mismatch",
            Self::OptionUnsupported => "option_unsupported",
            Self::OptionDisabled => "option_disabled",
            Self::OptionArchived => "option_archived",
        }
    }
}

/// A display mask (`mask` in schema v2): keeps the first/last characters and replaces the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskRule {
    pub keep_first: usize,
    pub keep_last: usize,
    pub mask_char: char,
    pub outlets: Vec<Outlet>,
}

/// Normalize ops of `rules.normalize` (contract C2 §3.1).
pub const TEXT_NORMALIZE_OPS: &[&str] = &["trim", "collapse_spaces", "lower", "upper", "nfc"];
/// Transform ops of `rules.transform` for text-like types other than `text` (contract C2 §3.3).
pub const TEXT_TRANSFORM_OPS: &[&str] = &["truncate"];
/// Validate rules of text-like types (contract C2 §3.2).
pub const TEXT_RULES: &[RuleKind] = &[
    RuleKind::MinLength,
    RuleKind::MaxLength,
    RuleKind::Pattern,
    RuleKind::Enum,
    RuleKind::Email,
    RuleKind::Url,
    RuleKind::Cel,
    RuleKind::Plugin,
];
/// Validate rules of ordered scalar types (numbers, dates).
pub const ORDERED_RULES: &[RuleKind] = &[RuleKind::Min, RuleKind::Max, RuleKind::Cel, RuleKind::Plugin];

/// One form field type.
pub trait FieldType: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn storage(&self) -> StorageKind;
    fn cel_type(&self) -> Option<CelType>;
    fn allowed_rules(&self) -> &'static [RuleKind];
    fn normalize_ops(&self) -> &'static [&'static str] {
        &[]
    }
    fn transform_ops(&self) -> &'static [&'static str] {
        &[]
    }
    /// Type-specific settings (options, amount, signature) from the raw field object.
    fn parse_settings(&self, _raw: &Map<String, Value>) -> Result<TypeSettings, DefError> {
        Ok(TypeSettings::None)
    }
    /// Checks a parsed field against the whole field list (references between fields).
    fn validate_definition(&self, _field: &FormField, _all: &[FormField]) -> Result<(), DefError> {
        Ok(())
    }
    /// Type check and canonical form; `prior` is the stored value (option carry-over).
    fn normalize(&self, field: &FormField, raw: &Value, prior: Option<&Value>) -> Result<Value, TypeError>;
    /// Whether a `min` / `max` bound has the form this type compares against.
    fn accepts_bound(&self, _bound: &Bound) -> bool {
        false
    }
    /// Whether a JSON number default is given to the type as its decimal text (decimal types
    /// refuse JSON numbers from callers; a designer default may still be one).
    fn number_default_as_text(&self) -> bool {
        false
    }
    /// Whether the server writes the value (formula output, autonumber, AI summary).
    fn server_written(&self) -> bool {
        false
    }
    /// Operand for `min` / `max` / length rules; `None` = not comparable.
    fn operand(&self, _field: &FormField, _value: &Value) -> Option<Operand> {
        None
    }
    /// The CEL binding of a stored value (contract C2 §5.3); `None` = not exposed.
    fn cel_value(&self, _field: &FormField, _value: &Value) -> Option<Value> {
        None
    }
    fn outlet(&self, outlet: Outlet) -> OutletRule {
        let _ = outlet;
        OutletRule::Value
    }
    /// Whether `mask` may be declared on this type (text-like types only).
    fn maskable(&self) -> bool {
        false
    }
    /// Display mask; only called when the field's `mask` lists the outlet. `None` = no mask.
    fn mask(&self, _field: &FormField, value: &Value, rule: &MaskRule) -> Option<Value> {
        if !self.maskable() {
            return None;
        }
        value.as_str().map(|text| Value::String(mask_text(text, rule)))
    }
}

/// Outlets where the file, reference, signature and location types are refused in the MVP
/// (contract C2 §4.1).
const OPAQUE_REJECTED: &[Outlet] = &[
    Outlet::Filter,
    Outlet::Sort,
    Outlet::Group,
    Outlet::ConditionSource,
    Outlet::SiteRead,
    Outlet::SiteSubmit,
];

/// The outlet rule of a type that never flows into queries, conditions or a public site.
fn opaque_outlet(outlet: Outlet) -> OutletRule {
    if OPAQUE_REJECTED.contains(&outlet) || outlet == Outlet::TitleTemplate {
        OutletRule::Reject
    } else {
        OutletRule::Value
    }
}

/// Keeps `keep_first` and `keep_last` characters and replaces the rest with `mask_char`.
fn mask_text(text: &str, rule: &MaskRule) -> String {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let tail_start = total.saturating_sub(rule.keep_last);
    chars
        .iter()
        .enumerate()
        .map(|(index, ch)| {
            if index < rule.keep_first || index >= tail_start {
                *ch
            } else {
                rule.mask_char
            }
        })
        .collect()
}

static REGISTRY: &[&dyn FieldType] = &[
    &text::Text,
    &text::Textarea,
    &text::RichText,
    &text::Phone,
    &text::Email,
    &text::Address,
    &text::Scan,
    &text::Signature,
    &text::Autonumber,
    &text::AiSummary,
    &numeric::Number,
    &numeric::Integer,
    &numeric::Amount,
    &numeric::Rating,
    &numeric::Progress,
    &temporal::Date,
    &temporal::DateTime,
    &choice::SingleSelect,
    &choice::MultiSelect,
    &choice::Boolean,
    &reference::Member,
    &reference::Location,
    &reference::Attachment,
    &reference::Image,
    &reference::Relation,
    &reference::ChildTable,
    &reference::Formula,
];

/// The descriptor of a type name.
#[must_use]
pub fn field_type(name: &str) -> Option<&'static dyn FieldType> {
    REGISTRY.iter().copied().find(|ty| ty.name() == name)
}

/// Every descriptor, in a stable order.
#[must_use]
pub fn all_field_types() -> &'static [&'static dyn FieldType] {
    REGISTRY
}

/// The outlet rule of a parsed field; an unknown type (cannot be parsed) is refused.
#[must_use]
pub fn field_outlet(field: &FormField, outlet: Outlet) -> OutletRule {
    field_type(&field.field_type).map_or(OutletRule::Reject, |ty| ty.outlet(outlet))
}

/// The `GET /api/v1/field-types` document (contract C2 §4.2).
#[must_use]
pub fn descriptor_document() -> Value {
    let types = REGISTRY
        .iter()
        .map(|ty| {
            let outlets = Outlet::ALL
                .iter()
                .map(|outlet| (outlet.name().to_string(), json!(ty.outlet(*outlet))))
                .collect::<Map<String, Value>>();
            json!({
                "name": ty.name(),
                "storage": ty.storage(),
                "cel_type": ty.cel_type(),
                "allowed_rules": ty.allowed_rules().iter().map(|kind| kind.name()).collect::<Vec<_>>(),
                "normalize_ops": ty.normalize_ops(),
                "transform_ops": ty.transform_ops(),
                "outlets": outlets,
                "maskable": ty.maskable(),
            })
        })
        .collect::<Vec<_>>();
    json!({"version": 1, "types": types})
}

/// Trimmed non-empty strings of a JSON array member (options lists).
fn string_list(raw: &Map<String, Value>, key: &str) -> Vec<String> {
    raw.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    const ALL_TYPE_NAMES: &[&str] = &[
        "text",
        "phone",
        "email",
        "address",
        "location",
        "scan",
        "signature",
        "autonumber",
        "member",
        "textarea",
        "rich_text",
        "number",
        "integer",
        "amount",
        "rating",
        "progress",
        "date",
        "datetime",
        "single_select",
        "multi_select",
        "boolean",
        "attachment",
        "image",
        "relation",
        "child_table",
        "formula",
        "ai_summary",
    ];

    #[test]
    fn the_registry_holds_every_type_once() {
        assert_eq!(all_field_types().len(), ALL_TYPE_NAMES.len());
        for name in ALL_TYPE_NAMES {
            assert!(field_type(name).is_some(), "{name} missing");
        }
        assert!(field_type("select").is_none());
    }

    #[test]
    fn opaque_types_are_refused_in_queries_and_on_the_site() {
        for name in [
            "attachment",
            "image",
            "relation",
            "child_table",
            "signature",
            "location",
        ] {
            let ty = field_type(name).unwrap_or(&text::Text);
            assert_eq!(ty.outlet(Outlet::Filter), OutletRule::Reject, "{name}");
            assert_eq!(ty.outlet(Outlet::SiteSubmit), OutletRule::Reject, "{name}");
            assert_eq!(ty.outlet(Outlet::RecordRead), OutletRule::Value, "{name}");
        }
        assert_eq!(reference::Member.outlet(Outlet::SiteRead), OutletRule::Reject);
        assert_eq!(text::Phone.outlet(Outlet::SiteRead), OutletRule::Value);
    }

    #[test]
    fn masks_keep_the_ends() {
        let rule = MaskRule {
            keep_first: 1,
            keep_last: 4,
            mask_char: '*',
            outlets: vec![Outlet::List],
        };
        assert_eq!(mask_text("13800001234", &rule), "1******1234");
        assert_eq!(mask_text("abc", &rule), "abc");
    }

    #[test]
    fn the_descriptor_document_names_rules_and_outlets() {
        let doc = descriptor_document();
        let phone = doc["types"]
            .as_array()
            .and_then(|types| types.iter().find(|ty| ty["name"] == "phone"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(phone["storage"], "text");
        assert_eq!(phone["cel_type"], "string");
        assert_eq!(phone["outlets"]["site_read"], "value");
        assert_eq!(phone["maskable"], true);
    }
}
