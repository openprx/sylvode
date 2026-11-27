//! Serde types of `sv.layout.v1`. They mirror the JSON exactly; semantic checks live in the
//! validator.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `schema` member every layout must carry.
pub const SCHEMA_ID: &str = "sv.layout.v1";

/// A layout specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    /// Always [`SCHEMA_ID`].
    pub schema: String,
    /// Dotted key, unique per scope.
    pub key: String,
    /// Human title (literal or i18n map).
    pub title: Text,
    /// Declared parameters, filled from slot context or route params.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamDef>,
    /// Named data sources.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, DataSource>,
    /// Named actions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, Action>,
    /// Root node of the component tree.
    pub root: Node,
}

/// A text value: literal, i18n map (`en` required) or computed CEL expression.
///
/// A literal or map value starting with `$` is a reference resolved by the renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Text {
    /// Plain literal or reference string.
    Literal(String),
    /// `{"cel": "<expr>"}`, evaluated in the `layout_when` environment.
    Computed(CelText),
    /// Locale to text.
    I18n(BTreeMap<String, String>),
}

/// Computed text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CelText {
    /// The CEL expression source.
    pub cel: String,
}

/// Parameter value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    /// String.
    String,
    /// Integer.
    Int,
    /// Boolean.
    Bool,
}

/// Declaration of one parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamDef {
    /// Value type.
    #[serde(rename = "type")]
    pub kind: ParamType,
    /// Whether the host must supply it.
    #[serde(default)]
    pub required: bool,
}

/// A node of the component tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Component type name.
    #[serde(rename = "type")]
    pub kind: String,
    /// Raw props; typed access through [`crate::typed_props`].
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub props: Value,
    /// CEL expression (`layout_when`); false hides the node and its children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Child nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Self>,
    /// Action name for the `click` event (buttons).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_click: Option<String>,
}

/// Filter operator of a `query` source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    /// Equal.
    Eq,
    /// Not equal.
    Neq,
    /// Contains.
    Contains,
    /// Greater than.
    Gt,
    /// Greater or equal.
    Gte,
    /// Less than.
    Lt,
    /// Less or equal.
    Lte,
    /// Not empty.
    NotEmpty,
}

/// One filter condition; `value` may be a reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    /// Field key.
    pub field: String,
    /// Operator.
    pub op: FilterOp,
    /// Comparison value (absent for `not_empty`).
    #[serde(default)]
    pub value: Value,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}

/// One sort key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SortSpec {
    /// Field key.
    pub field: String,
    /// Direction.
    pub dir: SortDir,
}

/// Single-record read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordSource {
    /// Form reference (`$bind.<name>` or `form:<key>`).
    pub form: String,
    /// Record id reference.
    pub record: String,
    /// Fields to read.
    #[serde(default)]
    pub fields: Vec<String>,
}

/// List query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuerySource {
    /// Form reference.
    pub form: String,
    /// Filters.
    #[serde(default)]
    pub filter: Vec<Filter>,
    /// Sort keys.
    #[serde(default)]
    pub sort: Vec<SortSpec>,
    /// Fields to read.
    #[serde(default)]
    pub fields: Vec<String>,
    /// Page size, at most 200.
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// Form descriptor source (input target of `form` nodes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FormSource {
    /// Form reference.
    pub form: String,
    /// Fields exposed.
    #[serde(default)]
    pub fields: Vec<String>,
}

/// Reserved plugin source (refused in the MVP).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginSource {
    /// Package id.
    pub package: String,
    /// Export name.
    pub export: String,
    /// Input.
    #[serde(default)]
    pub input: BTreeMap<String, Value>,
}

/// A named data source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum DataSource {
    /// One record.
    Record(RecordSource),
    /// A list of records.
    Query(QuerySource),
    /// A form descriptor.
    Form(FormSource),
    /// Reserved.
    Plugin(PluginSource),
}

/// `submit` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SubmitMode {
    /// Create a record.
    Create,
    /// Update a record.
    Update,
}

/// Target of a record navigation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordTarget {
    /// Form reference.
    pub form: String,
    /// Record id reference.
    pub id: String,
}

/// Target of a view navigation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewTarget {
    /// Form reference.
    pub form: String,
    /// View key.
    pub view: String,
}

/// Navigation target; external URLs are not allowed in v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NavTarget {
    /// A record.
    Record(RecordTarget),
    /// A form.
    Form(String),
    /// A site route (site only).
    Route(String),
    /// A form view.
    View(ViewTarget),
}

/// A named action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    /// Invoke a package action export.
    Invoke {
        /// Package id.
        package: String,
        /// Export name.
        export: String,
        /// Input (values may be references).
        #[serde(default)]
        input: BTreeMap<String, Value>,
        /// Confirmation text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirm: Option<Text>,
    },
    /// Create or update a record from a `form` source.
    Submit {
        /// Name of a `form` source.
        data: String,
        /// Create or update.
        mode: SubmitMode,
        /// Record reference (update).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record: Option<String>,
        /// Confirmation text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirm: Option<Text>,
    },
    /// Client-side navigation.
    Navigate {
        /// Target.
        to: NavTarget,
    },
}

impl Action {
    /// The `kind` string of the action.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Invoke { .. } => "invoke",
            Self::Submit { .. } => "submit",
            Self::Navigate { .. } => "navigate",
        }
    }
}
