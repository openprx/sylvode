//! Read-only block list for a public share.
//!
//! The editor's prose container is the visual source. Embeds and child pages that point at
//! another object are not expanded: an embed becomes an empty placeholder, and a child page
//! node is omitted. A page that has never been opened in the editor falls back to the semantic
//! tree's own text.

use collab_core::{NodeKind, SemanticNode, SemanticSnapshot};
use serde::Serialize;
use serde_json::Value;

const PUBLIC_BLOCK_CAP: usize = 10_000;
const PUBLIC_TEXT_CAP: usize = 100_000;
const CHILD_PAGE_NAMES: &[&str] = &["childpage", "child_page", "subpage", "page"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicBlock {
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u32>,
    pub indent: u32,
}

#[must_use]
pub fn compose_public_document(root_state: &Value, semantic: &SemanticSnapshot) -> Vec<PublicBlock> {
    let mut blocks = prose_blocks(root_state);
    if blocks.is_empty() {
        blocks = semantic_text_blocks(semantic);
    }
    for block in semantic_embed_placeholders(semantic) {
        if blocks.len() >= PUBLIC_BLOCK_CAP {
            break;
        }
        blocks.push(block);
    }
    blocks
}

#[must_use]
pub fn plain_text_of(blocks: &[PublicBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if block.kind == "embed" || block.text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&block.text);
    }
    sanitize_share_text(&out)
}

fn prose_blocks(root_state: &Value) -> Vec<PublicBlock> {
    let doc = root_state.get("doc").unwrap_or(root_state);
    let Some(children) = doc.get("children").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    for child in children {
        if blocks.len() >= PUBLIC_BLOCK_CAP {
            break;
        }
        push_prose_node(child, &mut blocks);
    }
    blocks
}

fn push_prose_node(node: &Value, blocks: &mut Vec<PublicBlock>) {
    if blocks.len() >= PUBLIC_BLOCK_CAP {
        return;
    }
    if node.is_string() {
        return;
    }
    let name = node.get("nodeName").and_then(Value::as_str).unwrap_or("");
    let normalized = name.to_ascii_lowercase();
    if CHILD_PAGE_NAMES.contains(&normalized.as_str()) {
        return;
    }
    let attributes = node.get("attributes");
    if is_embed_attributes(attributes) || normalized == "embed" {
        blocks.push(PublicBlock {
            kind: "embed".to_string(),
            text: String::new(),
            level: None,
            indent: 0,
        });
        return;
    }
    if matches!(name, "paragraph" | "heading" | "bulletItem" | "codeBlock") {
        let indent = indent_of(attributes);
        let level = (name == "heading").then(|| level_of(attributes));
        blocks.push(PublicBlock {
            kind: name.to_string(),
            text: sanitize_share_text(&text_of(node.get("children"))),
            level,
            indent,
        });
        return;
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            push_prose_node(child, blocks);
        }
    }
}

fn semantic_text_blocks(semantic: &SemanticSnapshot) -> Vec<PublicBlock> {
    let mut nodes: Vec<_> = semantic
        .nodes
        .iter()
        .filter(|(_, node)| {
            !node.deleted && node.kind == NodeKind::Block && !is_child_page(node) && !is_embed_node(node)
        })
        .collect();
    nodes
        .sort_by(|(left_id, left), (right_id, right)| left.order_key.cmp(&right.order_key).then(left_id.cmp(right_id)));
    nodes
        .into_iter()
        .take(PUBLIC_BLOCK_CAP)
        .map(|(_, node)| PublicBlock {
            kind: "paragraph".to_string(),
            text: sanitize_share_text(&node.text),
            level: None,
            indent: 0,
        })
        .filter(|block| !block.text.is_empty())
        .collect()
}

fn semantic_embed_placeholders(semantic: &SemanticSnapshot) -> Vec<PublicBlock> {
    let mut nodes: Vec<_> = semantic
        .nodes
        .iter()
        .filter(|(_, node)| !node.deleted && is_embed_node(node) && !is_child_page(node))
        .collect();
    nodes
        .sort_by(|(left_id, left), (right_id, right)| left.order_key.cmp(&right.order_key).then(left_id.cmp(right_id)));
    nodes
        .into_iter()
        .map(|_| PublicBlock {
            kind: "embed".to_string(),
            text: String::new(),
            level: None,
            indent: 0,
        })
        .collect()
}

fn is_child_page(node: &SemanticNode) -> bool {
    node.properties.keys().any(|key| {
        matches!(
            key.as_str(),
            "child_page_id" | "child_object_id" | "page_id" | "subpage_id"
        )
    })
}

fn is_embed_node(node: &SemanticNode) -> bool {
    node.properties.contains_key("embed_type")
        || node.properties.contains_key("embed_object_id")
        || node.properties.contains_key("reference_id")
}

fn is_embed_attributes(attributes: Option<&Value>) -> bool {
    let Some(attributes) = attributes else {
        return false;
    };
    attributes.get("embed_type").is_some()
        || attributes.get("embed_object_id").is_some()
        || attributes.get("reference_id").is_some()
        || attributes.get("child_page_id").is_some()
}

fn indent_of(attributes: Option<&Value>) -> u32 {
    let raw = attributes
        .and_then(|value| value.get("indent"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    u32::try_from(raw).unwrap_or(0).min(7)
}

fn level_of(attributes: Option<&Value>) -> u32 {
    let raw = attributes
        .and_then(|value| value.get("level"))
        .and_then(Value::as_u64)
        .unwrap_or(1);
    u32::try_from(raw).unwrap_or(1).clamp(1, 3)
}

/// Tag-strips text before it is returned on an anonymous link.
#[must_use]
pub fn sanitize_share_text(input: &str) -> String {
    let decoded = decode_entities(input);
    let mut out = String::with_capacity(decoded.len());
    let mut in_tag = false;
    for ch in decoded.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        out.push(ch);
        if out.chars().count() >= PUBLIC_TEXT_CAP {
            break;
        }
    }
    out
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn text_of(children: Option<&Value>) -> String {
    let Some(children) = children.and_then(Value::as_array) else {
        return String::new();
    };
    let mut text = String::new();
    for child in children {
        if let Some(piece) = child.as_str() {
            text.push_str(piece);
            continue;
        }
        if child.get("nodeName").and_then(Value::as_str) == Some("text") {
            text.push_str(&text_of(child.get("children")));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use collab_core::{NodeKind, SemanticNode, SemanticSnapshot};
    use serde_json::json;
    use uuid::Uuid;

    use super::compose_public_document;

    #[test]
    fn public_page_keeps_rich_blocks_and_drops_child_pages_and_embed_ids() {
        let secret = Uuid::from_u128(42);
        let root = json!({
            "doc": {
                "nodeName": "doc",
                "children": [
                    {
                        "nodeName": "heading",
                        "attributes": { "level": 2, "indent": 0 },
                        "children": ["Hello"]
                    },
                    {
                        "nodeName": "bulletItem",
                        "attributes": { "indent": 1 },
                        "children": ["<b>item</b>"]
                    },
                    {
                        "nodeName": "childPage",
                        "attributes": { "page_id": secret.to_string() },
                        "children": ["Hidden child"]
                    },
                    {
                        "nodeName": "paragraph",
                        "attributes": { "embed_object_id": secret.to_string(), "embed_type": "collection" },
                        "children": ["Should not leak"]
                    }
                ]
            }
        });
        let mut semantic = SemanticSnapshot::default();
        semantic.nodes.insert(
            Arc::<str>::from("child"),
            SemanticNode {
                parent: None,
                order_key: "00000000".to_string(),
                kind: NodeKind::Block,
                text: format!("child {secret}"),
                properties: BTreeMap::from([("child_page_id".to_string(), secret.to_string())]),
                deleted: false,
            },
        );
        semantic.nodes.insert(
            Arc::<str>::from("embed"),
            SemanticNode {
                parent: None,
                order_key: "00000001".to_string(),
                kind: NodeKind::Block,
                text: secret.to_string(),
                properties: BTreeMap::from([("embed_object_id".to_string(), secret.to_string())]),
                deleted: false,
            },
        );

        let blocks = compose_public_document(&root, &semantic);
        let encoded = serde_json::to_string(&blocks).unwrap_or_default();
        assert!(!encoded.is_empty());
        assert!(!encoded.contains(&secret.to_string()), "{encoded}");
        assert!(!encoded.contains("Hidden child"), "{encoded}");
        assert_eq!(blocks.first().map(|block| block.kind.as_str()), Some("heading"));
        assert_eq!(blocks.first().and_then(|block| block.level), Some(2));
        assert_eq!(blocks.first().map(|block| block.text.as_str()), Some("Hello"));
        assert_eq!(blocks.get(1).map(|block| block.kind.as_str()), Some("bulletItem"));
        assert_eq!(blocks.get(1).map(|block| block.text.as_str()), Some("item"));
        assert_eq!(blocks.get(1).map(|block| block.indent), Some(1));
        assert!(blocks.iter().any(|block| block.kind == "embed"));
        assert!(blocks.iter().all(|block| block.text != format!("child {secret}")));
    }
}
