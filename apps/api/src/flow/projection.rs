//! Turns a [`collab_core`] document state into the JSON shapes `flow_object_projections` stores.
//!
//! `ADR-0002`: the projection is a rebuildable read replica of the CRDT document, never a second
//! writer.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use collab_core::{Frontier, InlineMark, InlineSpan, SemanticNode, SemanticSnapshot};
use serde_json::Value;

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
    serde_json::to_value(snapshot)
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

/// Markdown rendering for `render=markdown` and object Markdown export.
///
/// A node with no `block_type` and no inline spans is written as its plain text, which is what
/// documents stored before block types and marks produced. Typed blocks (`paragraph`, `heading`,
/// `bulletItem`, `codeBlock`) and spans become Markdown. The visible characters are the same
/// string [`plain_text`] joins for search: marks never leak into that projection.
#[must_use]
pub fn render_markdown(title: &str, snapshot: &SemanticSnapshot) -> String {
    let mut rendered = String::new();
    if !title.is_empty() {
        rendered.push_str("# ");
        rendered.push_str(title);
        rendered.push('\n');
    }

    let mut nodes: Vec<_> = snapshot.nodes.iter().filter(|(_, node)| !node.deleted).collect();
    nodes.sort_by(|(id_a, node_a), (id_b, node_b)| {
        node_a
            .parent
            .cmp(&node_b.parent)
            .then(node_a.order_key.cmp(&node_b.order_key))
            .then(id_a.cmp(id_b))
    });
    for (_, node) in nodes {
        if node.text.is_empty() {
            continue;
        }
        let body = render_block(node);
        if body.is_empty() {
            continue;
        }
        if !rendered.is_empty() && !rendered.ends_with("\n\n") {
            rendered.push('\n');
        }
        rendered.push_str(&body);
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
    }
    rendered
}

fn render_block(node: &SemanticNode) -> String {
    let Some(block_type) = node.properties.get("block_type") else {
        if node.spans.is_empty() {
            return node.text.clone();
        }
        return render_inline(&node.text, &node.spans);
    };
    match block_type.as_str() {
        "heading" => {
            let level = node
                .properties
                .get("level")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(1)
                .clamp(1, 3);
            let hashes = "#".repeat(usize::from(level));
            format!("{hashes} {}", render_inline(&node.text, &node.spans))
        }
        "bulletItem" | "bullet_item" => {
            let indent = node
                .properties
                .get("indent")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(0)
                .min(7);
            format!(
                "{}- {}",
                "  ".repeat(usize::from(indent)),
                render_inline(&node.text, &node.spans)
            )
        }
        "codeBlock" | "code_block" => {
            let fence = "`".repeat(longest_backtick_run(&node.text).max(3));
            format!("{fence}\n{}\n{fence}", node.text)
        }
        _ => render_inline(&node.text, &node.spans),
    }
}

fn render_inline(text: &str, spans: &[InlineSpan]) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    let mut points = std::collections::BTreeSet::new();
    points.insert(0usize);
    points.insert(text.len());
    let valid: Vec<&InlineSpan> = spans
        .iter()
        .filter(|span| {
            let Ok(start) = usize::try_from(span.start) else {
                return false;
            };
            let Ok(end) = usize::try_from(span.end) else {
                return false;
            };
            start < end && text.get(start..end).is_some()
        })
        .collect();
    for span in &valid {
        if let Ok(start) = usize::try_from(span.start) {
            points.insert(start);
        }
        if let Ok(end) = usize::try_from(span.end) {
            points.insert(end);
        }
    }
    let points: Vec<usize> = points.into_iter().collect();
    let mut open: Vec<&InlineSpan> = Vec::new();
    let mut out = String::new();
    for pair in points.windows(2) {
        let Some(&start) = pair.first() else {
            continue;
        };
        let Some(&end) = pair.get(1) else {
            continue;
        };
        if start == end {
            continue;
        }
        let Some(slice) = text.get(start..end) else {
            continue;
        };
        let mut desired: Vec<&InlineSpan> = valid
            .iter()
            .copied()
            .filter(|span| {
                usize::try_from(span.start).is_ok_and(|span_start| span_start <= start)
                    && usize::try_from(span.end).is_ok_and(|span_end| span_end >= end)
            })
            .collect();
        desired.sort_by_key(|span| (mark_rank(span.kind), span.href.as_deref().unwrap_or("")));
        desired.dedup_by_key(|span| span.kind);
        let prefix = open
            .iter()
            .zip(desired.iter())
            .take_while(|(current, next)| same_span(current, next))
            .count();
        while open.len() > prefix {
            if let Some(span) = open.pop() {
                out.push_str(close_token(span));
            }
        }
        for span in desired.iter().skip(prefix) {
            out.push_str(open_token(span));
            open.push(span);
        }
        out.push_str(&escape_markdown(slice));
    }
    while let Some(span) = open.pop() {
        out.push_str(close_token(span));
    }
    out
}

fn same_span(left: &InlineSpan, right: &InlineSpan) -> bool {
    left.kind == right.kind && left.href == right.href && left.start == right.start && left.end == right.end
}

fn mark_rank(kind: InlineMark) -> u8 {
    match kind {
        InlineMark::Code => 0,
        InlineMark::Italic => 1,
        InlineMark::Bold => 2,
        InlineMark::Strike => 3,
        InlineMark::Link => 4,
    }
}

fn open_token(span: &InlineSpan) -> &'static str {
    match span.kind {
        InlineMark::Code => "`",
        InlineMark::Italic => "*",
        InlineMark::Bold => "**",
        InlineMark::Strike => "~~",
        InlineMark::Link => "[",
    }
}

fn close_token(span: &InlineSpan) -> String {
    match span.kind {
        InlineMark::Code => "`".to_string(),
        InlineMark::Italic => "*".to_string(),
        InlineMark::Bold => "**".to_string(),
        InlineMark::Strike => "~~".to_string(),
        InlineMark::Link => {
            let href = span.href.as_deref().unwrap_or("");
            format!("]({})", href.replace(')', "%29"))
        }
    }
}

fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '*' | '_' | '`' | '~' | '[' | ']' | '<') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn longest_backtick_run(text: &str) -> usize {
    let mut best = 0usize;
    let mut current = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            current = current.saturating_add(1);
            if current > best {
                best = current;
            }
        } else {
            current = 0;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use collab_core::{NodeKind, SemanticNode};

    use super::*;

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
                    spans: Vec::new(),
                },
            );
        }

        assert_eq!(render_markdown("Title", &snapshot), "# Title\n\nfirst\n\nsecond\n");
        assert_eq!(plain_text(&snapshot), "first second");
    }

    #[test]
    fn marks_render_as_markdown_and_stay_out_of_plain_text() {
        let mut snapshot = SemanticSnapshot::default();
        let mut properties = BTreeMap::new();
        properties.insert("block_type".to_string(), "paragraph".to_string());
        snapshot.nodes.insert(
            Arc::<str>::from("body"),
            SemanticNode {
                parent: None,
                order_key: "00000000".to_string(),
                kind: NodeKind::Block,
                text: "Hello bold".to_string(),
                properties,
                deleted: false,
                spans: vec![InlineSpan {
                    start: 6,
                    end: 10,
                    kind: InlineMark::Bold,
                    href: None,
                }],
            },
        );
        assert_eq!(plain_text(&snapshot), "Hello bold");
        assert_eq!(render_markdown("Note", &snapshot), "# Note\n\nHello **bold**\n");
    }

    #[test]
    fn heading_bullet_code_and_link_render_from_block_properties() {
        let mut snapshot = SemanticSnapshot::default();
        for (id, order_key, block_type, level, indent, text, spans) in [
            ("h", "00000000", "heading", Some("2"), None, "Section", Vec::new()),
            (
                "b",
                "00000001",
                "bulletItem",
                None,
                Some("1"),
                "item",
                vec![InlineSpan {
                    start: 0,
                    end: 4,
                    kind: InlineMark::Link,
                    href: Some("https://example.com/x".to_string()),
                }],
            ),
            ("c", "00000002", "codeBlock", None, None, "x = 1", Vec::new()),
        ] {
            let mut properties = BTreeMap::new();
            properties.insert("block_type".to_string(), block_type.to_string());
            if let Some(level) = level {
                properties.insert("level".to_string(), level.to_string());
            }
            if let Some(indent) = indent {
                properties.insert("indent".to_string(), indent.to_string());
            }
            snapshot.nodes.insert(
                Arc::<str>::from(id),
                SemanticNode {
                    parent: None,
                    order_key: order_key.to_string(),
                    kind: NodeKind::Block,
                    text: text.to_string(),
                    properties,
                    deleted: false,
                    spans,
                },
            );
        }
        assert_eq!(
            render_markdown("Page", &snapshot),
            "# Page\n\n## Section\n\n  - [item](https://example.com/x)\n\n```\nx = 1\n```\n"
        );
    }
}
