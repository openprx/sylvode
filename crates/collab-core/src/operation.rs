use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A logical, engine-agnostic node identifier chosen by the fixture generator. Never an engine
/// peer id, tree id, or client id — those stay inside each adapter.
pub type NodeId = Arc<str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A Page's block-tree node (paragraph/heading/list/code carry rich text).
    Block,
    /// A Collection's field definition, ordered as a child of the fields root.
    CollectionField,
    /// A Collection's saved view, ordered as a child of the views root.
    CollectionView,
    /// A Record's scalar/relation property, keyed by name in `properties`.
    RecordProperty,
    /// A Navigator tree node (workspace/project/page shortcut entries).
    NavigatorNode,
}

/// One entry in a deterministic, replayable operation log.
///
/// `Operation` is the single vocabulary both `collab-loro` and `collab-yrs-yjs` translate into
/// their own engine calls — the fixture generator and corpus runner never know which engine is
/// underneath.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    CreateNode {
        id: NodeId,
        parent: Option<NodeId>,
        index: u32,
        kind: NodeKind,
    },
    MoveNode {
        id: NodeId,
        new_parent: Option<NodeId>,
        index: u32,
    },
    DeleteNode {
        id: NodeId,
    },
    InsertText {
        id: NodeId,
        index: u32,
        text: String,
    },
    DeleteText {
        id: NodeId,
        index: u32,
        len: u32,
    },
    SetProperty {
        id: NodeId,
        key: String,
        value: String,
    },
    /// Applies or clears one inline mark. `index` and `len` are UTF-8 byte offsets, the same
    /// unit as [`Self::InsertText`]. `value: null` (or JSON `false`) clears the mark; `true` or
    /// `"true"` sets a toggle mark (`bold`, `italic`, `code`, `strike`); a string sets a `link`
    /// href.
    MarkText {
        id: NodeId,
        index: u32,
        len: u32,
        mark: String,
        #[serde(default, deserialize_with = "deserialize_mark_value")]
        value: Option<String>,
    },
}

fn deserialize_mark_value<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null | serde_json::Value::Bool(false)) => Ok(None),
        Some(serde_json::Value::Bool(true)) => Ok(Some("true".to_string())),
        Some(serde_json::Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(serde::de::Error::custom(
            "mark value must be a string, a boolean, or null",
        )),
    }
}

impl Operation {
    #[must_use]
    pub const fn target(&self) -> &NodeId {
        match self {
            Self::CreateNode { id, .. }
            | Self::MoveNode { id, .. }
            | Self::DeleteNode { id }
            | Self::InsertText { id, .. }
            | Self::DeleteText { id, .. }
            | Self::SetProperty { id, .. }
            | Self::MarkText { id, .. } => id,
        }
    }
}
