//! Turns a [`collab_core`] document state into the JSON shapes `flow_object_projections` stores.
//!
//! `ADR-0002`: the projection is a rebuildable read replica of the CRDT document, never a second
//! writer.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use collab_core::{Frontier, SemanticSnapshot};
use serde_json::Value;
use std::fmt::Write as _;

/// Current version of derived editor-CID and canonical navigator-order projections.
pub const PROJECTION_MODEL_VERSION: u64 = 5;

/// Base64 encoding for an opaque [`Frontier`] on the wire. `Bootstrap.snapshot_base64`
/// elsewhere in the contract sets the precedent that opaque CRDT bytes are base64 in JSON.
#[must_use]
pub fn encode_frontier(frontier: &Frontier) -> String {
    BASE64.encode(frontier.as_bytes())
}

/// The `flow_object_projections.state` JSON for a snapshot: the canonical `{"nodes": {...}}` shape
/// `SemanticSnapshot` already serializes to.
///
/// Empty (`{"nodes":{}}`) for every object this package creates, since no content command exists
/// yet to populate blocks.
///
/// # Errors
/// Only if `serde_json` itself cannot encode the snapshot, which cannot happen for the field
/// types `SemanticSnapshot` carries (see its own `canonical_json` doc comment).
pub fn state_json(snapshot: &SemanticSnapshot) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(snapshot)?;
    if let Value::Object(fields) = &mut value {
        fields.insert(
            "__flow_projection_model".into(),
            serde_json::json!(PROJECTION_MODEL_VERSION),
        );
    }
    Ok(value)
}

/// Plain-text projection: every live (non-deleted) node's text, in a stable `(order_key, id)`
/// order, space-joined. Trivially empty for a just-created document.
#[must_use]
pub fn plain_text(snapshot: &SemanticSnapshot) -> String {
    let mut nodes: Vec<_> = snapshot.nodes.iter().filter(|(_, node)| !node.deleted).collect();
    nodes.sort_by(|(id_a, node_a), (id_b, node_b)| node_a.order_key.cmp(&node_b.order_key).then(id_a.cmp(id_b)));
    nodes
        .into_iter()
        .map(|(_, node)| node.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Markdown rendering for `render=markdown`.
///
/// The title is followed by every live node's text in deterministic document order. The
/// engine-independent snapshot is the source here: rendering only the title would silently drop
/// content that is already present in the same accepted document.
#[must_use]
pub fn render_markdown(title: &str, snapshot: &SemanticSnapshot) -> String {
    render_markdown_context(title, snapshot, None)
}
pub fn render_markdown_for_object(title: &str, snapshot: &SemanticSnapshot, object: uuid::Uuid) -> String {
    render_markdown_context(title, snapshot, Some(object))
}
fn render_markdown_context(title: &str, snapshot: &SemanticSnapshot, object: Option<uuid::Uuid>) -> String {
    let mut rendered = String::new();
    if !title.is_empty() {
        rendered.push_str("# ");
        rendered.push_str(&collab_core::page_inline::markdown(title, &[]));
        rendered.push('\n');
    }

    let mut nodes: Vec<_> = snapshot.nodes.iter().filter(|(_, node)| !node.deleted).collect();
    nodes.sort_by(|(id_a, node_a), (id_b, node_b)| node_a.order_key.cmp(&node_b.order_key).then(id_a.cmp(id_b)));
    for (_, node) in nodes {
        let kind = node.properties.get("block_type").map(String::as_str);
        if matches!(kind, Some("image" | "attachment")) {
            if !rendered.is_empty() && !rendered.ends_with("\n\n") {
                rendered.push('\n');
            }
            let label = node
                .properties
                .get(if kind == Some("image") { "alt" } else { "name" })
                .filter(|value| !value.is_empty())
                .map_or_else(
                    || if kind == Some("image") { "Image" } else { "Attachment" },
                    String::as_str,
                );
            let label = collab_core::page_inline::markdown(label, &[]);
            if let Some(asset) = node
                .properties
                .get("asset_id")
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
            {
                if let Some(object) = object {
                    let prefix = if kind == Some("image") { "!" } else { "" };
                    let _ = writeln!(
                        rendered,
                        "{prefix}[{label}](/api/v2/flow/objects/{object}/body-assets/{asset})"
                    );
                } else {
                    let _ = writeln!(rendered, "[{label}; asset reference {asset}]");
                }
            }
            if let Some(caption) = node.properties.get("caption").filter(|caption| !caption.is_empty()) {
                rendered.push_str(&collab_core::page_inline::markdown(caption, &[]));
                rendered.push('\n');
            }
            continue;
        }
        if kind == Some("childPage") {
            if let Some(child) = node.properties.get("child_page_id") {
                let _ = writeln!(rendered, "\n[Linked Page reference {child}]");
            }
            continue;
        }
        if node.text.is_empty() && node.properties.get("block_type").is_none_or(|v| v != "divider") {
            continue;
        }
        if !rendered.is_empty() && !rendered.ends_with("\n\n") {
            rendered.push('\n');
        }
        let spans = node
            .properties
            .get("inline_spans")
            .and_then(|raw| serde_json::from_str::<Vec<collab_core::page_inline::InlineSpan>>(raw).ok())
            .unwrap_or_default();
        let code = node
            .properties
            .get("block_type")
            .is_some_and(|kind| kind == "codeBlock");
        let fence = if code {
            "`".repeat(
                node.text
                    .split(|ch| ch != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .max(3),
            )
        } else {
            String::new()
        };
        let text = if code {
            node.text.clone()
        } else {
            collab_core::page_inline::markdown(&node.text, &spans)
        };
        let indent = node
            .properties
            .get("indent")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
            .min(7);
        rendered.push_str(&"  ".repeat(indent));
        match node.properties.get("block_type").map(String::as_str) {
            Some("heading") => {
                let level = node
                    .properties
                    .get("level")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 3);
                rendered.push_str(&"#".repeat(level));
                rendered.push(' ');
            }
            Some("divider") => rendered.push_str("---"),
            Some("bulletItem") => rendered.push_str("- "),
            Some("orderedItem") => {
                rendered.push_str(node.properties.get("order").map_or("1", String::as_str));
                rendered.push_str(". ");
            }
            Some("todoItem") => rendered.push_str(if node.properties.get("checked").is_some_and(|v| v == "true") {
                "- [x] "
            } else {
                "- [ ] "
            }),
            Some("blockquote" | "toggle") => rendered.push_str("> "),
            Some("codeBlock") => {
                rendered.push_str(&fence);
                rendered.push('\n');
            }
            _ => {}
        }
        rendered.push_str(&text);
        if code {
            rendered.push('\n');
            rendered.push_str(&fence);
        }
        rendered.push('\n');
    }
    rendered
}

/// Bounded, read-only refresh for projections emitted before canonical editor CID support.
///
/// The marker belongs only to the derived JSON, never the accepted snapshot/frontier/hash.
/// Replacing the entire projection from an older writer also removes its marker and is detected.
pub async fn refresh_editor_state(
    db: &sea_orm::DatabaseConnection,
    document: uuid::Uuid,
    expected_frontier: &str,
    stored: &mut Value,
) -> Result<Option<collab_core::LoroCollabEngine>, crate::error::ApiError> {
    use collab_core::CollabEngine;
    let ordinal = stored
        .get("nodes")
        .and_then(Value::as_object)
        .is_some_and(|nodes| nodes.keys().any(|id| id.starts_with("__editor_prose__:")));
    if stored.get("__flow_projection_model").and_then(Value::as_u64) == Some(PROJECTION_MODEL_VERSION) && !ordinal {
        return Ok(None);
    }
    let engine = super::document::load_head_engine(db, document).await?;
    // `expected_frontier` is the stored projection frontier and may use a legacy byte order.
    let expected = BASE64
        .decode(expected_frontier)
        .map_err(|_| crate::error::ApiError::internal())?;
    if !super::frontier::same(engine.frontier().as_bytes(), &expected)? {
        return Err(crate::error::ApiError::resync_required(
            "document changed during projection refresh",
            None,
        ));
    }
    *stored = state_json(
        &engine
            .semantic_snapshot()
            .map_err(|e| super::command::map_collab_error(&e))?,
    )
    .map_err(|_| crate::error::ApiError::internal())?;
    Ok(Some(engine))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use collab_core::{NodeKind, SemanticNode};

    use super::*;

    #[test]
    fn fenced_code_is_raw_and_uses_a_delimiter_longer_than_its_body_runs() {
        let mut snapshot = SemanticSnapshot::default();
        let text = "* _ ` [ ] < &amp;\n```\n````";
        snapshot.nodes.insert(
            "code".into(),
            SemanticNode {
                parent: None,
                order_key: "0".into(),
                kind: NodeKind::Block,
                text: text.into(),
                properties: BTreeMap::from([("block_type".into(), "codeBlock".into())]),
                deleted: false,
            },
        );
        assert_eq!(render_markdown("", &snapshot), format!("`````\n{text}\n`````\n"));
    }
    #[test]
    fn markdown_contains_live_block_text_in_document_order() {
        let mut snapshot = SemanticSnapshot::default();
        for (id, order_key, text, deleted) in [
            ("later", "0000000001", "second", false),
            ("earlier", "0000000000", "first", false),
            ("deleted", "0000000002", "must not render", true),
        ] {
            snapshot.nodes.insert(
                Arc::<str>::from(id),
                SemanticNode {
                    parent: None,
                    order_key: order_key.to_string(),
                    kind: NodeKind::Block,
                    text: text.to_string(),
                    properties: BTreeMap::new(),
                    deleted,
                },
            );
        }

        assert_eq!(render_markdown("Title", &snapshot), "# Title\n\nfirst\n\nsecond\n");
    }
}
