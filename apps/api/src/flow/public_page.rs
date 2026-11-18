//! Read-only block list for a public share.
//!
//! The editor's prose container is the visual source. Embeds and child pages that point at
//! another object are not expanded: an embed becomes an empty placeholder, and a child page
//! node is omitted. A page that has never been opened in the editor falls back to the semantic
//! tree's own text.

use collab_core::{NodeKind, SemanticNode, SemanticSnapshot};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

const PUBLIC_BLOCK_CAP: usize = 10_000;
const PUBLIC_TEXT_CAP: usize = 100_000;
const CHILD_PAGE_NAMES: &[&str] = &["childpage", "child_page", "subpage", "page"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PublicBlock {
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u32>,
    pub indent: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inline_spans: Vec<collab_core::page_inline::InlineSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

#[must_use]
pub fn compose_public_document(root_state: &Value, semantic: &SemanticSnapshot) -> Vec<PublicBlock> {
    let canonical = root_state
        .get("flow_editor_structure_meta")
        .and_then(|meta| meta.get("initialized"))
        .and_then(Value::as_bool)
        == Some(true)
        || semantic.nodes.values().any(|node| {
            node.properties
                .get("content_source")
                .is_some_and(|source| source == "editor_doc")
        });
    let mut blocks = if canonical {
        semantic_text_blocks(semantic)
    } else {
        prose_blocks(root_state)
    };
    if blocks.is_empty() && !canonical {
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
            ..PublicBlock::default()
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
            ..PublicBlock::default()
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
            !node.deleted
                && node.kind == NodeKind::Block
                && (!is_child_page(node)
                    || (node
                        .properties
                        .get("content_source")
                        .is_some_and(|source| source == "editor_doc")
                        && node
                            .properties
                            .get("block_type")
                            .is_some_and(|kind| kind == "childPage")))
                && !is_embed_node(node)
        })
        .collect();
    nodes
        .sort_by(|(left_id, left), (right_id, right)| left.order_key.cmp(&right.order_key).then(left_id.cmp(right_id)));
    nodes
        .into_iter()
        .take(PUBLIC_BLOCK_CAP)
        .map(|(_, node)| public_semantic_node(node))
        .filter(|block| {
            !block.text.is_empty() || matches!(block.kind.as_str(), "divider" | "image" | "attachment" | "childPage")
        })
        .collect()
}

fn public_semantic_node(node: &SemanticNode) -> PublicBlock {
    let canonical = node
        .properties
        .get("content_source")
        .is_some_and(|source| source == "editor_doc");
    let (text, offsets) = if canonical {
        sanitize_editor_text(&node.text)
    } else {
        (sanitize_share_text(&node.text), std::collections::BTreeMap::new())
    };
    let kind = node
        .properties
        .get("block_type")
        .filter(|kind| {
            matches!(
                kind.as_str(),
                "paragraph"
                    | "heading"
                    | "bulletItem"
                    | "codeBlock"
                    | "orderedItem"
                    | "todoItem"
                    | "blockquote"
                    | "toggle"
                    | "divider"
                    | "image"
                    | "attachment"
                    | "childPage"
            )
        })
        .cloned()
        .unwrap_or_else(|| "paragraph".into());
    if canonical && kind == "childPage" {
        return PublicBlock {
            kind,
            indent: node
                .properties
                .get("indent")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0)
                .min(7),
            ..PublicBlock::default()
        };
    }
    let number = |key: &str, default: u32, max: u32| {
        node.properties
            .get(key)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(default)
            .min(max)
    };
    let mut spans = Vec::new();
    if canonical
        && let Some(raw) = node.properties.get("inline_spans")
        && let Ok(source) = serde_json::from_str::<Vec<collab_core::page_inline::InlineSpan>>(raw)
    {
        let retained = offsets.last_key_value().map_or(0, |(offset, _)| *offset);
        for mut span in source {
            if span.start >= span.end
                || span.end > node.text.len()
                || !node.text.is_char_boundary(span.start)
                || !node.text.is_char_boundary(span.end)
            {
                continue;
            }
            if span.mark == collab_core::page_inline::InlineMark::Link
                && !span.href.as_deref().is_some_and(collab_core::page_inline::safe_href)
            {
                continue;
            }
            let (Some(start), Some(end)) = (offsets.get(&span.start), offsets.get(&span.end.min(retained))) else {
                continue;
            };
            if start >= end {
                continue;
            }
            span.start = *start;
            span.end = *end;
            spans.push(span);
        }
    }
    PublicBlock {
        text,
        level: (kind == "heading").then(|| number("level", 1, 3).max(1)),
        indent: number("indent", 0, 7),
        checked: (kind == "todoItem").then(|| node.properties.get("checked").is_some_and(|v| v == "true")),
        order: (kind == "orderedItem").then(|| number("order", 1, 1_000_000).max(1)),
        collapsed: (kind == "toggle").then(|| node.properties.get("collapsed").is_some_and(|v| v == "true")),
        inline_spans: spans,
        asset_id: if canonical && matches!(kind.as_str(), "image" | "attachment") {
            node.properties
                .get("asset_id")
                .and_then(|value| Uuid::parse_str(value).ok())
        } else {
            None
        },
        alt: if canonical {
            node.properties.get("alt").cloned()
        } else {
            None
        },
        caption: if canonical {
            node.properties.get("caption").cloned()
        } else {
            None
        },
        name: if canonical {
            node.properties.get("name").cloned()
        } else {
            None
        },
        mime: if canonical {
            node.properties.get("mime").cloned()
        } else {
            None
        },
        kind,
    }
}
fn sanitize_editor_text(input: &str) -> (String, std::collections::BTreeMap<usize, usize>) {
    let mut text = String::new();
    let mut offsets = std::collections::BTreeMap::new();
    let mut retained = 0;
    offsets.insert(0, 0);
    for (index, ch) in input.char_indices() {
        offsets.insert(index, text.len());
        if !ch.is_control() || matches!(ch, '\n' | '\t') {
            text.push(ch);
            retained += 1;
        }
        offsets.insert(index + ch.len_utf8(), text.len());
        if retained >= PUBLIC_TEXT_CAP {
            break;
        }
    }
    (text, offsets)
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
            ..PublicBlock::default()
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
    fn model5_public_fixture_redacts_child_and_keeps_typed_media_unicode() {
        use collab_core::{LoroCollabEngine, Operation};
        let mut engine = LoroCollabEngine::new_empty(8701);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure().unwrap();
        let id = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: id.clone(),
                index: 0,
                text: "Public 中文😀".into(),
            })
            .unwrap();
        engine
            .apply_page_marks(
                &id,
                &[collab_core::page_inline::MarkEdit {
                    start: 7,
                    end: 17,
                    mark: collab_core::page_inline::InlineMark::Bold,
                    clear: false,
                    href: None,
                }],
            )
            .unwrap();
        engine.insert_page_atom("image",None,1,"image",&json!({"asset_id":"00000000-0000-4000-8000-000000000101","alt":"图片😀","caption":"Caption <literal>","indent":0})).unwrap();
        engine.insert_page_atom("attachment",None,2,"attachment",&json!({"asset_id":"00000000-0000-4000-8000-000000000102","name":"说明.json","mime":"application/json","caption":"File & note","indent":0})).unwrap();
        engine
            .insert_page_atom(
                "child",
                None,
                3,
                "childPage",
                &json!({"child_page_id":"00000000-0000-4000-8000-000000000103","indent":0}),
            )
            .unwrap();
        let blocks = compose_public_document(&engine.root_state_json(), &engine.semantic_snapshot().unwrap());
        let value = json!({"synthetic":true,"blocks":blocks});
        let encoded = value.to_string();
        assert!(!encoded.contains("00000000-0000-4000-8000-000000000103"));
        assert!(
            blocks
                .iter()
                .any(|block| block.kind == "childPage" && block.text.is_empty())
        );
        if std::env::var("WRITE_PUBLIC_V5_FIXTURE").as_deref() == Ok("1") {
            std::fs::write(
                "/tmp/sv-page-public-v5.fixture.json",
                serde_json::to_vec_pretty(&value).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn canonical_public_rich_attrs_keep_literal_text_and_filter_unsafe_links() {
        let text = "a< &😀";
        let mut properties = BTreeMap::from([
            ("content_source".into(), "editor_doc".into()),
            ("block_type".into(), "todoItem".into()),
            ("checked".into(), "true".into()),
            ("indent".into(), "2".into()),
        ]);
        properties.insert("inline_spans".into(),json!([{"start":0,"end":text.len(),"mark":"bold"},{"start":0,"end":text.len(),"mark":"link","href":"javascript:bad"}]).to_string());
        let mut semantic = SemanticSnapshot::default();
        semantic.nodes.insert(
            "synthetic".into(),
            SemanticNode {
                parent: None,
                order_key: "0".into(),
                kind: NodeKind::Block,
                text: text.into(),
                properties,
                deleted: false,
            },
        );
        let blocks = compose_public_document(&json!({}), &semantic);
        let block = blocks.first().unwrap();
        assert_eq!(block.text, text);
        assert_eq!(block.kind, "todoItem");
        assert_eq!(block.checked, Some(true));
        assert_eq!(block.indent, 2);
        assert_eq!(block.inline_spans.len(), 1);
        assert_eq!(
            block.inline_spans.first().unwrap().mark,
            collab_core::page_inline::InlineMark::Bold
        );
    }

    #[test]
    fn indexed_public_read_has_no_physical_fallback_when_semantic_is_empty() {
        let root = serde_json::json!({"flow_editor_structure_meta":{"initialized":true},"doc":{"children":[{"nodeName":"paragraph","children":["physical stale content"]}]}});
        assert!(compose_public_document(&root, &SemanticSnapshot::default()).is_empty());
    }

    #[test]
    fn indexed_public_read_uses_live_tree_order_and_never_storage_tombstones() {
        use collab_core::{CollabEngine, LoroCollabEngine, Operation};
        let mut engine = LoroCollabEngine::load(include_bytes!(
            "../../../../crates/collab-core/tests/fixtures/page-v4/js-rich.snapshot"
        ))
        .unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let three = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "three")
            .unwrap()
            .0
            .clone();
        engine
            .apply_content_operation(&Operation::MoveNode {
                id: three.clone(),
                new_parent: None,
                index: 1,
            })
            .unwrap();
        let moved = compose_public_document(&engine.root_state_json(), &engine.semantic_snapshot().unwrap());
        assert_eq!(
            moved.iter().map(|block| block.text.as_str()).collect::<Vec<_>>(),
            ["o中文😀ne", "two", "three"]
        );
        engine
            .apply_content_operation(&Operation::DeleteNode { id: three })
            .unwrap();
        let snapshot = engine.semantic_snapshot().unwrap();
        let blocks = compose_public_document(&engine.root_state_json(), &snapshot);
        assert_eq!(
            blocks.iter().map(|block| block.text.as_str()).collect::<Vec<_>>(),
            ["o中文😀ne", "two"]
        );
        assert_eq!(blocks.get(1).unwrap().indent, 1);
        assert!(!blocks.first().unwrap().inline_spans.is_empty());
    }

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
