//! Layout schema `sv.layout.v1`: serde types, the component catalogue, the spec validator, slot
//! descriptors and static capability analysis.
//!
//! The entry points are [`validate`] / [`validate_bytes`] (all errors with JSON-pointer node paths)
//! and [`analyse`] (capabilities implied by data sources and actions). The catalogue served to
//! editors is [`descriptors`]; slot descriptors are [`slots`].
//!
//! Text values that start with `$` are always references; a literal that must begin with a dollar
//! sign cannot be expressed in v1.

mod analysis;
mod components;
mod model;
mod refs;
mod slots;
mod validate;

pub use analysis::{
    Analysis, CAP_ACTION_INVOKE, CAP_FORMS_READ, CAP_RECORDS_CREATE, CAP_RECORDS_READ, CAP_RECORDS_UPDATE,
    CapabilityUse, analyse,
};
pub use components::{
    Align, BadgeProps, BadgeTone, ButtonProps, ButtonVariant, CardProps, ChildrenRule, ColumnDef, ComponentDescriptor,
    ComponentProps, Direction, FieldDisplayProps, FormProps, Format, Gap, GridProps, MarkdownProps, StackProps,
    StatProps, TabDef, TableProps, TabsProps, TextProps, TextStyle, ToneMap, ToneSpec, descriptor, descriptors,
    typed_props,
};
pub use model::{
    Action, CelText, DataSource, Filter, FilterOp, FormSource, Layout, NavTarget, Node, ParamDef, ParamType,
    PluginSource, QuerySource, RecordSource, RecordTarget, SCHEMA_ID, SortDir, SortSpec, SubmitMode, Text, ViewTarget,
};
pub use slots::{SlotDescriptor, SlotKind, slot, slots};
pub use validate::{
    ErrorCode, LayoutError, MAX_ACTIONS, MAX_DATA_SOURCES, MAX_DEPTH, MAX_NODES, MAX_PAGE_SIZE, MAX_SPEC_BYTES,
    Validated, validate, validate_bytes,
};
