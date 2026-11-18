//! Accepted editor prose projected with stable actual block container identities.
use crate::{CollabError, NodeKind, SemanticNode, SemanticSnapshot};
use loro::LoroDoc;
use std::collections::BTreeMap;

pub fn project_editor_prose(doc: &LoroDoc, semantic: &mut SemanticSnapshot) -> Result<(), CollabError> {
    project(doc, semantic, false)
}
pub fn project_legacy_editor_prose(doc: &LoroDoc, semantic: &mut SemanticSnapshot) -> Result<(), CollabError> {
    project(doc, semantic, true)
}
fn project(doc: &LoroDoc, semantic: &mut SemanticSnapshot, legacy: bool) -> Result<(), CollabError> {
    let Some(blocks) = (if legacy {
        crate::editor_doc_bridge::legacy_editor_blocks(doc)?
    } else {
        crate::editor_doc_bridge::editor_blocks(doc)?
    }) else {
        return Ok(());
    };
    if !legacy {
        semantic.nodes.retain(|_, node| {
            node.properties
                .get("content_source")
                .is_none_or(|source| source != "editor_doc")
        });
    }
    // An initialized doc is authoritative, including an intentionally empty page.
    for node in semantic.nodes.values_mut() {
        if node.kind == NodeKind::Block && !node.properties.contains_key("embed_object_id") {
            node.text.clear();
        }
    }
    for (index, block) in blocks.into_iter().enumerate() {
        let mut properties = BTreeMap::new();
        properties.insert("content_source".to_owned(), "editor_doc".to_owned());
        if !legacy && crate::editor_structure::ready(doc) {
            let root = doc.get_map(crate::editor_structure::META);
            let model = match root.get("model") {
                Some(loro::ValueOrContainer::Value(loro::LoroValue::String(value))) => value.to_string(),
                _ => "rich_page_v4".into(),
            };
            properties.insert("structure_model".into(), model);
        }
        properties.insert("block_type".to_owned(), block.kind);
        if let Some(logical_id) = block.logical_id {
            properties.insert("logical_id".to_owned(), logical_id);
        }
        if !block.inline_spans.is_empty() {
            properties.insert(
                "inline_spans".into(),
                serde_json::to_string(&block.inline_spans)
                    .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?,
            );
        }
        for key in ["indent", "level", "checked", "order", "collapsed"] {
            if legacy && !matches!(key, "indent" | "level") {
                continue;
            }
            if let Some(value) = block.attributes.get(key) {
                properties.insert(key.to_owned(), value.to_string());
            }
        }
        if !legacy {
            for key in [
                "asset_id",
                "alt",
                "caption",
                "name",
                "mime",
                "child_page_id",
                "reservation_id",
            ] {
                if let Some(value) = block.attributes.get(key).and_then(serde_json::Value::as_str) {
                    properties.insert(key.into(), value.into());
                }
            }
        }
        semantic.nodes.insert(
            block.id.into(),
            SemanticNode {
                parent: block.parent.map(Into::into),
                order_key: format!("{index:08}"),
                kind: NodeKind::Block,
                text: block.text,
                properties,
                deleted: false,
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_editor_prose_uses_stable_container_ids() {
        let doc = LoroDoc::new();
        doc.import(include_bytes!(
            "../../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot"
        ))
        .unwrap();
        let mut semantic = SemanticSnapshot::default();
        project_editor_prose(&doc, &mut semantic).unwrap();
        assert_eq!(semantic.nodes.len(), 4);
        let (id, _node) = semantic
            .nodes
            .iter()
            .find(|(_, node)| node.text == "Browser 😀 prose")
            .unwrap();
        assert!(id.as_ref().starts_with("cid:"));
        let id = id.clone();
        crate::editor_doc_bridge::apply_editor_text(&doc, id.as_ref(), 0, None, Some("Agent ")).unwrap();
        project_editor_prose(&doc, &mut semantic).unwrap();
        assert_eq!(semantic.nodes.get(&id).unwrap().text, "Agent Browser 😀 prose");
    }
}
