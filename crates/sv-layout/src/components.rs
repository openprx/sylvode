//! The 12 MVP components: typed props, JSON Schemas and descriptors.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Node, Text};

/// Spacing scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Gap {
    /// No gap.
    None,
    /// Extra small.
    Xs,
    /// Small.
    Sm,
    /// Medium.
    Md,
    /// Large.
    Lg,
}

/// Stack direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Top to bottom.
    Vertical,
    /// Left to right.
    Horizontal,
}

/// Cross-axis alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    /// Start.
    Start,
    /// Center.
    Center,
    /// End.
    End,
    /// Stretch.
    Stretch,
}

/// Props of `stack`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackProps {
    /// Direction.
    #[serde(default)]
    pub direction: Option<Direction>,
    /// Gap.
    #[serde(default)]
    pub gap: Option<Gap>,
    /// Alignment.
    #[serde(default)]
    pub align: Option<Align>,
}

/// Props of `grid`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GridProps {
    /// Columns, 1..=4 (collapses to 1 on narrow screens).
    #[serde(default)]
    #[schemars(range(min = 1, max = 4))]
    pub columns: Option<u8>,
    /// Gap.
    #[serde(default)]
    pub gap: Option<Gap>,
}

/// Props of `card`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardProps {
    /// Title.
    #[serde(default)]
    pub title: Option<Text>,
    /// Subtitle.
    #[serde(default)]
    pub subtitle: Option<Text>,
    /// Whether the card can be collapsed.
    #[serde(default)]
    pub collapsible: Option<bool>,
}

/// One tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabDef {
    /// Tab key.
    pub key: String,
    /// Label.
    pub label: Text,
}

/// Props of `tabs`; child `i` belongs to tab `i`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabsProps {
    /// Tabs, at most 8.
    #[schemars(length(max = 8))]
    pub tabs: Vec<TabDef>,
}

/// Text style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TextStyle {
    /// Body text.
    Body,
    /// Muted text.
    Muted,
    /// Heading.
    Heading,
    /// Caption.
    Caption,
}

/// Props of `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextProps {
    /// The text.
    pub value: Text,
    /// Style.
    #[serde(default)]
    pub style: Option<TextStyle>,
}

/// Props of `markdown` (`CommonMark`, sanitised; value at most 32 KiB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkdownProps {
    /// Markdown source.
    pub value: Text,
}

/// Value format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Number.
    Number,
    /// Amount.
    Amount,
    /// Percent.
    Percent,
    /// Date.
    Date,
    /// Date and time.
    Datetime,
}

/// Props of `stat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatProps {
    /// Label.
    pub label: Text,
    /// Value (text or reference).
    pub value: Text,
    /// Format.
    #[serde(default)]
    pub format: Option<Format>,
    /// Trend reference.
    #[serde(default)]
    pub trend: Option<String>,
}

/// Badge tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BadgeTone {
    /// Neutral.
    Neutral,
    /// Info.
    Info,
    /// Success.
    Success,
    /// Warning.
    Warning,
    /// Danger.
    Danger,
}

/// Tone, fixed or mapped from the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ToneSpec {
    /// One tone.
    Fixed(BadgeTone),
    /// `{"map": {"open": "info"}}`.
    Mapped(ToneMap),
}

/// Value-to-tone map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToneMap {
    /// Value to tone.
    pub map: BTreeMap<String, BadgeTone>,
}

/// Props of `badge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BadgeProps {
    /// Value (text or reference).
    pub value: Text,
    /// Tone.
    #[serde(default)]
    pub tone: Option<ToneSpec>,
}

/// One table column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColumnDef {
    /// Field key.
    pub field: String,
    /// Header label.
    #[serde(default)]
    pub label: Option<Text>,
    /// Format.
    #[serde(default)]
    pub format: Option<Format>,
}

/// Props of `table`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableProps {
    /// Reference to a list.
    pub rows: String,
    /// Columns, at most 12.
    #[schemars(length(max = 12))]
    pub columns: Vec<ColumnDef>,
    /// Text shown when empty.
    #[serde(default)]
    pub empty: Option<Text>,
    /// Action name for row clicks.
    #[serde(default)]
    pub row_click: Option<String>,
}

/// Props of `field-display`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldDisplayProps {
    /// Reference to a record result.
    pub record: String,
    /// Field key.
    pub field: String,
    /// Whether to show the label.
    #[serde(default)]
    pub label: Option<bool>,
}

/// Props of `form`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormProps {
    /// Name of a `form` data source.
    pub data: String,
    /// Subset of the source's fields (default all).
    #[serde(default)]
    pub fields: Vec<String>,
    /// Name of a `submit` action.
    pub submit: String,
    /// Submit button label.
    #[serde(default)]
    pub submit_label: Option<Text>,
}

/// Button variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ButtonVariant {
    /// Primary.
    Primary,
    /// Secondary.
    Secondary,
    /// Danger.
    Danger,
    /// Ghost.
    Ghost,
}

/// Props of `button`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ButtonProps {
    /// Label.
    pub label: Text,
    /// Variant.
    #[serde(default)]
    pub variant: Option<ButtonVariant>,
    /// Icon name (lowercase, digits and `-`, at most 40 characters).
    #[serde(default)]
    #[schemars(length(max = 40))]
    pub icon: Option<String>,
}

/// Typed props of a node, by component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentProps {
    /// `stack`
    Stack(StackProps),
    /// `grid`
    Grid(GridProps),
    /// `card`
    Card(CardProps),
    /// `tabs`
    Tabs(TabsProps),
    /// `text`
    Text(TextProps),
    /// `markdown`
    Markdown(MarkdownProps),
    /// `stat`
    Stat(StatProps),
    /// `badge`
    Badge(BadgeProps),
    /// `table`
    Table(TableProps),
    /// `field-display`
    FieldDisplay(FieldDisplayProps),
    /// `form`
    Form(FormProps),
    /// `button`
    Button(ButtonProps),
}

/// How many children a component takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildrenRule {
    /// Any number.
    Any,
    /// None.
    None,
    /// Exactly as many as the `tabs` prop lists.
    Fixed,
}

/// Descriptor of one component, as served by `GET /api/v1/layouts/components`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentDescriptor {
    /// Component type name.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// JSON Schema of the props.
    pub props_schema: Value,
    /// Children rule.
    pub children: ChildrenRule,
    /// Event names.
    pub events: Vec<&'static str>,
}

/// Props that may be omitted entirely (all members optional).
const OPTIONAL_PROPS: [&str; 3] = ["stack", "grid", "card"];

fn schema_value<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).unwrap_or(Value::Null)
}

fn build() -> Vec<ComponentDescriptor> {
    let d = |kind, props_schema, children, events| ComponentDescriptor {
        kind,
        props_schema,
        children,
        events,
    };
    vec![
        d("stack", schema_value::<StackProps>(), ChildrenRule::Any, vec![]),
        d("grid", schema_value::<GridProps>(), ChildrenRule::Any, vec![]),
        d("card", schema_value::<CardProps>(), ChildrenRule::Any, vec![]),
        d("tabs", schema_value::<TabsProps>(), ChildrenRule::Fixed, vec![]),
        d("text", schema_value::<TextProps>(), ChildrenRule::None, vec![]),
        d("markdown", schema_value::<MarkdownProps>(), ChildrenRule::None, vec![]),
        d("stat", schema_value::<StatProps>(), ChildrenRule::None, vec![]),
        d("badge", schema_value::<BadgeProps>(), ChildrenRule::None, vec![]),
        d(
            "table",
            schema_value::<TableProps>(),
            ChildrenRule::None,
            vec!["row_click"],
        ),
        d(
            "field-display",
            schema_value::<FieldDisplayProps>(),
            ChildrenRule::None,
            vec![],
        ),
        d("form", schema_value::<FormProps>(), ChildrenRule::None, vec!["submit"]),
        d(
            "button",
            schema_value::<ButtonProps>(),
            ChildrenRule::None,
            vec!["on_click"],
        ),
    ]
}

/// The catalogue: descriptor plus the compiled props validator.
pub struct Entry {
    pub descriptor: ComponentDescriptor,
    pub validator: Option<jsonschema::Validator>,
}

fn catalogue() -> &'static [Entry] {
    static CATALOGUE: OnceLock<Vec<Entry>> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        build()
            .into_iter()
            .map(|descriptor| Entry {
                validator: jsonschema::validator_for(&descriptor.props_schema).ok(),
                descriptor,
            })
            .collect()
    })
}

pub fn entry(kind: &str) -> Option<&'static Entry> {
    catalogue().iter().find(|e| e.descriptor.kind == kind)
}

/// Whether a node of this type may omit `props`.
pub fn props_optional(kind: &str) -> bool {
    OPTIONAL_PROPS.contains(&kind)
}

/// All 12 component descriptors, in catalogue order.
#[must_use]
pub fn descriptors() -> Vec<ComponentDescriptor> {
    catalogue().iter().map(|e| e.descriptor.clone()).collect()
}

/// The descriptor of one component type.
#[must_use]
pub fn descriptor(kind: &str) -> Option<&'static ComponentDescriptor> {
    entry(kind).map(|e| &e.descriptor)
}

/// Deserialises a node's props into the typed form. Unknown component types yield `Ok(None)`.
///
/// # Errors
/// Returns the serde error when the props do not fit the component.
pub fn typed_props(node: &Node) -> Result<Option<ComponentProps>, serde_json::Error> {
    let raw = if node.props.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        node.props.clone()
    };
    let props = match node.kind.as_str() {
        "stack" => ComponentProps::Stack(serde_json::from_value(raw)?),
        "grid" => ComponentProps::Grid(serde_json::from_value(raw)?),
        "card" => ComponentProps::Card(serde_json::from_value(raw)?),
        "tabs" => ComponentProps::Tabs(serde_json::from_value(raw)?),
        "text" => ComponentProps::Text(serde_json::from_value(raw)?),
        "markdown" => ComponentProps::Markdown(serde_json::from_value(raw)?),
        "stat" => ComponentProps::Stat(serde_json::from_value(raw)?),
        "badge" => ComponentProps::Badge(serde_json::from_value(raw)?),
        "table" => ComponentProps::Table(serde_json::from_value(raw)?),
        "field-display" => ComponentProps::FieldDisplay(serde_json::from_value(raw)?),
        "form" => ComponentProps::Form(serde_json::from_value(raw)?),
        "button" => ComponentProps::Button(serde_json::from_value(raw)?),
        _ => return Ok(None),
    };
    Ok(Some(props))
}
