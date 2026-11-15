//! Canonical flat editor prose, using the installed loro-prosemirror 0.4.3 layout.
//! This bridge never recreates an existing block/text container and never writes tree prose.
use crate::CollabError;
use loro::{Container, ContainerTrait, LoroDoc, LoroList, LoroMap, LoroText, LoroValue, ToJson, ValueOrContainer};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct EditorBlock {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub attributes: Value,
    pub logical_id: Option<String>,
}
fn invalid(reason: &str) -> CollabError {
    CollabError::OperationFailed {
        reason: reason.to_owned(),
    }
}
fn string(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}
fn children(map: &LoroMap) -> Option<LoroList> {
    match map.get("children") {
        Some(ValueOrContainer::Container(Container::List(l))) => Some(l),
        _ => None,
    }
}
fn root_children(doc: &LoroDoc) -> Option<LoroList> {
    children(&doc.get_map("doc"))
}
fn supported(kind: &str) -> bool {
    matches!(kind, "paragraph" | "heading" | "bulletItem" | "codeBlock")
}
fn text_children(map: &LoroMap) -> Result<Vec<LoroText>, CollabError> {
    let list = children(map).ok_or_else(|| invalid("editor_block_missing_children"))?;
    let mut texts = Vec::new();
    for i in 0..list.len() {
        match list.get(i) {
            Some(ValueOrContainer::Container(Container::Text(t))) => texts.push(t),
            _ => return Err(invalid("editor_block_unsupported_inline_container")),
        }
    }
    Ok(texts)
}
/// None means a legacy tree-only document. IDs are actual live map CIDs, not ordinals.
pub fn editor_blocks(doc: &LoroDoc) -> Result<Option<Vec<EditorBlock>>, CollabError> {
    let Some(list) = root_children(doc) else {
        return Ok(None);
    };
    let aliases = doc.get_map("flow_editor_aliases");
    if aliases.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX {
        return Err(invalid("editor_block_alias_limit_exceeded"));
    }
    let mut reverse_aliases = std::collections::BTreeMap::new();
    for key in aliases.keys() {
        if let Some(cid) = string(&aliases, key.as_ref()) {
            reverse_aliases.entry(cid).or_insert_with(|| key.to_string());
        }
    }
    let mut blocks = Vec::new();
    for i in 0..list.len() {
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i) else {
            return Err(invalid("editor_doc_unsupported_block_container"));
        };
        let kind = string(&map, "nodeName").ok_or_else(|| invalid("editor_block_missing_node_name"))?;
        // Non-prose dependency nodes must not expand private content into search/export.
        if !supported(&kind) {
            continue;
        }
        let attributes = match map.get("attributes") {
            Some(ValueOrContainer::Container(Container::Map(attrs))) => attrs.get_deep_value().to_json_value(),
            _ => Value::Object(serde_json::Map::new()),
        };
        if ["embed_type", "embed_object_id", "child_page_id", "reference_id"]
            .iter()
            .any(|key| attributes.get(*key).is_some())
        {
            continue;
        }
        let text = text_children(&map)?.iter().map(LoroText::to_string).collect::<String>();
        let logical_id = reverse_aliases.get(&map.id().to_string()).cloned();
        blocks.push(EditorBlock {
            id: map.id().to_string(),
            kind,
            text,
            attributes,
            logical_id,
        });
    }
    Ok(Some(blocks))
}
/// Resolve only live top-level editor maps. Tombstoned aliases cannot resurrect blocks.
pub fn resolve_editor_block_id(doc: &LoroDoc, id: &str) -> Option<String> {
    let list = root_children(doc)?;
    let canonical = string(&doc.get_map("flow_editor_aliases"), id).unwrap_or_else(|| id.to_owned());
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical
        {
            return Some(canonical);
        }
    }
    None
}

/// Apply one UTF-8 byte-indexed edit to an actual reachable editor block CID.
/// Returns false for non-editor IDs; caller must reject legacy shadow prose after initialization.
/// Both byte boundaries are validated before any mutation. Loro's explicit UTF-8 API performs
/// the coordinate translation; browser UTF-16 offsets must never be passed here as byte offsets.
pub fn apply_editor_text(
    doc: &LoroDoc,
    id: &str,
    index: u32,
    delete_len: Option<u32>,
    insert: Option<&str>,
) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc) else {
        return Ok(false);
    };
    let Some(canonical_id) = resolve_editor_block_id(doc, id) else {
        return Ok(false);
    };
    let mut target = None;
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical_id
        {
            target = Some(map);
            break;
        }
    }
    let Some(map) = target else { return Ok(false) };
    let kind = string(&map, "nodeName").ok_or_else(|| invalid("editor_block_missing_node_name"))?;
    if !supported(&kind) {
        return Err(invalid("editor_block_unsupported_kind"));
    }
    if let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes")
        && ["embed_type", "embed_object_id", "child_page_id", "reference_id"]
            .iter()
            .any(|key| attrs.get(key).is_some())
    {
        return Err(invalid("editor_block_dependency_not_prose"));
    }
    if delete_len.is_some() == insert.is_some() {
        return Err(invalid("editor_text_requires_exactly_one_edit"));
    }
    let texts = text_children(&map)?;
    if texts.len() > 1 {
        return Err(invalid("editor_block_multiple_text_containers_unsupported"));
    }
    let text = texts.first();
    let value = text.map(LoroText::to_string).unwrap_or_default();
    let start = index as usize;
    let end = start
        .checked_add(delete_len.unwrap_or(0) as usize)
        .ok_or_else(|| invalid("editor_text_offset_out_of_bounds"))?;
    if end > value.len() || !value.is_char_boundary(start) || !value.is_char_boundary(end) {
        return Err(invalid("editor_text_invalid_utf8_boundary"));
    }
    // The shipped schema has no marks. Do not guess inheritance or erase unknown rich text.
    if let Some(text) = text {
        let delta = text.get_richtext_value().to_json_value();
        if delta.as_array().is_some_and(|runs| {
            runs.iter().any(|run| {
                run.get("attributes")
                    .and_then(Value::as_object)
                    .is_some_and(|attrs| !attrs.is_empty())
            })
        }) {
            return Err(invalid("editor_text_marks_unsupported"));
        }
    }
    if let Some(value) = insert {
        if value.is_empty() {
            return Ok(true);
        }
        let text = match text {
            Some(text) => text.clone(),
            None => children(&map)
                .ok_or_else(|| invalid("editor_block_missing_children"))?
                .insert_container(0, LoroText::new())
                .map_err(|e| invalid(&e.to_string()))?,
        };
        text.insert_utf8(start, value).map_err(|e| invalid(&e.to_string()))?;
    } else if let Some(len) = delete_len
        && len > 0
    {
        text.ok_or_else(|| invalid("editor_text_offset_out_of_bounds"))?
            .delete_utf8(start, len as usize)
            .map_err(|e| invalid(&e.to_string()))?;
    }
    Ok(true)
}

#[derive(Debug, Clone)]
pub struct EditorInitialBlock {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub indent: u32,
    pub level: Option<u32>,
}
/// One-time initialization only. Callers supply a validated legacy flat read model and
/// commit the private candidate atomically; existing editor state is never replaced.
pub fn initialize_editor_doc(doc: &LoroDoc, blocks: &[EditorInitialBlock]) -> Result<bool, CollabError> {
    let root = doc.get_map("doc");
    if root.get("children").is_some() {
        return Ok(false);
    }
    if string(&root, "nodeName").is_some_and(|kind| kind != "doc") {
        return Err(invalid("editor_doc_invalid_root"));
    }
    if root
        .get("attributes")
        .is_some_and(|value| !matches!(value, ValueOrContainer::Container(Container::Map(_))))
    {
        return Err(invalid("editor_doc_invalid_attributes"));
    }
    let aliases = doc.get_map("flow_editor_aliases");
    if blocks.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX
        || aliases.len().saturating_add(blocks.len()) > crate::limits::DOCUMENT_BLOCK_COUNT_MAX
    {
        return Err(invalid("editor_block_alias_limit_exceeded"));
    }
    let mut ids = std::collections::BTreeSet::new();
    for block in blocks {
        if block.id.is_empty()
            || block.id.len() > 256
            || block.id.starts_with("cid:")
            || !ids.insert(block.id.as_str())
            || aliases.get(&block.id).is_some()
        {
            return Err(invalid("editor_block_invalid_or_duplicate_logical_id"));
        }
        if !supported(&block.kind)
            || block.indent > 8
            || block.text.chars().count() > crate::limits::TEXT_BLOCK_CHARS_MAX
        {
            return Err(invalid("editor_initial_block_unsupported"));
        }
        if (block.kind == "heading" && !block.level.is_some_and(|level| (1..=3).contains(&level)))
            || (block.kind != "heading" && block.level.is_some())
        {
            return Err(invalid("editor_block_invalid_attribute"));
        }
    }
    // Build detached containers so invalid input cannot leave an initialized half-document.
    let list = LoroList::new();
    let defaults = [EditorInitialBlock {
        id: String::new(),
        kind: "paragraph".into(),
        text: String::new(),
        indent: 0,
        level: None,
    }];
    let source = if blocks.is_empty() { defaults.as_slice() } else { blocks };
    for block in source {
        let map = LoroMap::new();
        map.insert("nodeName", block.kind.as_str())
            .map_err(|e| invalid(&e.to_string()))?;
        let attrs = map
            .insert_container("attributes", LoroMap::new())
            .map_err(|e| invalid(&e.to_string()))?;
        attrs
            .insert("indent", block.indent)
            .map_err(|e| invalid(&e.to_string()))?;
        if let Some(level) = block.level {
            attrs.insert("level", level).map_err(|e| invalid(&e.to_string()))?;
        }
        let inline = map
            .insert_container("children", LoroList::new())
            .map_err(|e| invalid(&e.to_string()))?;
        if !block.text.is_empty() {
            inline
                .insert_container(0, LoroText::new())
                .map_err(|e| invalid(&e.to_string()))?
                .insert_utf8(0, &block.text)
                .map_err(|e| invalid(&e.to_string()))?;
        }
        list.insert_container(list.len(), map)
            .map_err(|e| invalid(&e.to_string()))?;
    }
    let attached = root
        .insert_container("children", list)
        .map_err(|e| invalid(&e.to_string()))?;
    root.insert("nodeName", "doc").map_err(|e| invalid(&e.to_string()))?;
    if root.get("attributes").is_none() {
        root.insert_container("attributes", LoroMap::new())
            .map_err(|e| invalid(&e.to_string()))?;
    }
    for (index, block) in blocks.iter().enumerate() {
        let Some(ValueOrContainer::Container(Container::Map(map))) = attached.get(index) else {
            return Err(invalid("editor_initial_block_attach_failed"));
        };
        aliases
            .insert(&block.id, map.id().to_string())
            .map_err(|e| invalid(&e.to_string()))?;
    }
    Ok(true)
}

/// Structural edits are deliberately limited to the flat shipped schema. Existing map IDs
/// are retained; requested creation IDs are bounded aliases, never guessed from text/order.
pub fn apply_editor_structure(doc: &LoroDoc, operation: &crate::Operation) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc) else {
        return Ok(false);
    };
    if let crate::Operation::CreateNode {
        id,
        parent,
        index,
        kind,
    } = operation
    {
        if *kind != crate::NodeKind::Block {
            return Ok(false);
        }
        if parent.is_some() {
            return Err(invalid("editor_block_parent_must_be_root"));
        }
        if id.is_empty() || id.len() > 256 || id.starts_with("cid:") {
            return Err(invalid("editor_block_invalid_logical_id"));
        }
        let aliases = doc.get_map("flow_editor_aliases");
        if aliases.get(id.as_ref()).is_some() {
            return Err(CollabError::DuplicateNode { id: id.to_string() });
        }
        if list.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX
            || aliases.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX
        {
            return Err(invalid("editor_block_alias_limit_exceeded"));
        }
        if *index as usize > list.len() {
            return Err(invalid("editor_block_index_out_of_bounds"));
        }
        let map = LoroMap::new();
        map.insert("nodeName", "paragraph")
            .map_err(|e| invalid(&e.to_string()))?;
        let attrs = map
            .insert_container("attributes", LoroMap::new())
            .map_err(|e| invalid(&e.to_string()))?;
        attrs.insert("indent", 0).map_err(|e| invalid(&e.to_string()))?;
        map.insert_container("children", LoroList::new())
            .map_err(|e| invalid(&e.to_string()))?;
        let attached = list
            .insert_container(*index as usize, map)
            .map_err(|e| invalid(&e.to_string()))?;
        aliases
            .insert(id.as_ref(), attached.id().to_string())
            .map_err(|e| invalid(&e.to_string()))?;
        return Ok(true);
    }
    let (crate::Operation::DeleteNode { id }
    | crate::Operation::MoveNode { id, .. }
    | crate::Operation::SetProperty { id, .. }) = operation
    else {
        return Ok(false);
    };
    let canonical = string(&doc.get_map("flow_editor_aliases"), id.as_ref()).unwrap_or_else(|| id.to_string());
    let mut target = None;
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical
        {
            target = Some((i, map));
            break;
        }
    }
    let Some((position, map)) = target else {
        if doc.get_map("flow_editor_aliases").get(id.as_ref()).is_some() {
            return Err(CollabError::UnknownNode { id: id.to_string() });
        }
        return Ok(false);
    };
    if !string(&map, "nodeName").is_some_and(|kind| supported(&kind)) {
        return Err(invalid("editor_block_unsupported_kind"));
    }
    match operation {
        crate::Operation::DeleteNode { .. } => {
            if list.len() == 1 {
                return Err(invalid("editor_doc_requires_one_block"));
            }
            list.delete(position, 1).map_err(|e| invalid(&e.to_string()))?;
        }
        crate::Operation::MoveNode { new_parent, index, .. } => {
            if new_parent.is_some() {
                return Err(invalid("editor_block_parent_must_be_root"));
            }
            if *index as usize != position {
                return Err(invalid("editor_block_identity_preserving_move_unsupported"));
            }
        }
        crate::Operation::SetProperty { key, value, .. } => {
            let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes") else {
                return Err(invalid("editor_block_missing_attributes"));
            };
            match key.as_str() {
                "block_type" => {
                    if !supported(value) {
                        return Err(invalid("editor_block_unsupported_kind"));
                    }
                    // Schema attrs vary by type: setting paragraph cannot retain heading-only attrs.
                    if value == "heading" {
                        attrs.insert("level", 1).map_err(|e| invalid(&e.to_string()))?;
                    } else {
                        attrs.delete("level").map_err(|e| invalid(&e.to_string()))?;
                    }
                    map.insert("nodeName", value.as_str())
                        .map_err(|e| invalid(&e.to_string()))?;
                }
                "indent" | "level" => {
                    let number = value
                        .parse::<i32>()
                        .map_err(|_| invalid("editor_block_invalid_attribute"))?;
                    if (key == "indent" && !(0..=8).contains(&number))
                        || (key == "level"
                            && (!(1..=3).contains(&number) || string(&map, "nodeName").as_deref() != Some("heading")))
                    {
                        return Err(invalid("editor_block_invalid_attribute"));
                    }
                    attrs.insert(key, number).map_err(|e| invalid(&e.to_string()))?;
                }
                _ => return Err(invalid("editor_block_unsupported_property")),
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SNAPSHOT: &[u8] = include_bytes!("../../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot");
    fn fixture() -> LoroDoc {
        let doc = LoroDoc::new();
        doc.import(SNAPSHOT).expect("real JS library snapshot");
        doc
    }
    #[test]
    fn real_js_snapshot_reader_and_utf8_edits_preserve_cids() {
        let doc = fixture();
        let before = editor_blocks(&doc).unwrap().unwrap();
        let first = before.first().unwrap();
        assert_eq!(first.text, "Browser 😀 prose");
        assert!(apply_editor_text(&doc, &first.id, 12, None, Some(" agent")).unwrap());
        assert_eq!(
            editor_blocks(&doc).unwrap().unwrap().first().unwrap().text,
            "Browser 😀 agent prose"
        );
        assert!(apply_editor_text(&doc, &first.id, 12, Some(6), None).unwrap());
        let after = editor_blocks(&doc).unwrap().unwrap();
        assert_eq!(after.first().unwrap().text, first.text);
        assert_eq!(
            before.iter().map(|b| &b.id).collect::<Vec<_>>(),
            after.iter().map(|b| &b.id).collect::<Vec<_>>()
        );
    }
    #[test]
    fn rust_output_fixture_preserves_js_ids_and_carries_agent_edit() {
        let doc = fixture();
        doc.set_peer_id(72001).unwrap();
        let vv = doc.oplog_vv();
        let id = editor_blocks(&doc).unwrap().unwrap().first().unwrap().id.clone();
        assert!(apply_editor_text(&doc, &id, 12, None, Some(" + Rust agent")).unwrap());
        doc.commit();
        if std::env::var("WRITE_EDITOR_INTEROP_FIXTURE").as_deref() == Ok("1") {
            let base =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/tests/fixtures/editor-doc-v1");
            std::fs::write(
                base.join("rust-agent.update"),
                doc.export(loro::ExportMode::updates(&vv)).unwrap(),
            )
            .unwrap();
            std::fs::write(
                base.join("rust-agent.snapshot"),
                doc.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
        }
        assert_eq!(
            editor_blocks(&doc).unwrap().unwrap().first().unwrap().text,
            "Browser 😀 + Rust agent prose"
        );
    }
    #[test]
    fn structural_creation_alias_and_deletion_keep_existing_identity() {
        let doc = fixture();
        let before = editor_blocks(&doc).unwrap().unwrap();
        let create = crate::Operation::CreateNode {
            id: "agent-block".into(),
            parent: None,
            index: 1,
            kind: crate::NodeKind::Block,
        };
        assert!(apply_editor_structure(&doc, &create).unwrap());
        assert!(apply_editor_text(&doc, "agent-block", 0, None, Some("Agent created 😀")).unwrap());
        let blocks = editor_blocks(&doc).unwrap().unwrap();
        let created = blocks
            .iter()
            .find(|b| b.logical_id.as_deref() == Some("agent-block"))
            .unwrap();
        assert_eq!(created.text, "Agent created 😀");
        assert!(created.id.starts_with("cid:"));
        assert_eq!(blocks.first().unwrap().id, before.first().unwrap().id);
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::SetProperty {
                    id: "agent-block".into(),
                    key: "block_type".into(),
                    value: "heading".into()
                }
            )
            .unwrap()
        );
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::SetProperty {
                    id: "agent-block".into(),
                    key: "level".into(),
                    value: "2".into()
                }
            )
            .unwrap()
        );
        let frontier = doc.oplog_vv();
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::MoveNode {
                    id: "agent-block".into(),
                    new_parent: None,
                    index: 0
                }
            )
            .is_err()
        );
        assert_eq!(doc.oplog_vv(), frontier);
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::DeleteNode {
                    id: "agent-block".into()
                }
            )
            .unwrap()
        );
        assert_eq!(editor_blocks(&doc).unwrap().unwrap().len(), before.len());
        assert!(apply_editor_structure(&doc, &create).is_err());
        assert!(!apply_editor_text(&doc, "agent-block", 0, None, Some("resurrect")).unwrap());
        if std::env::var("WRITE_EDITOR_INTEROP_FIXTURE").as_deref() == Ok("1") {
            // A second fresh fixture captures the positive create/alias/edit shape for JS.
            let fresh = fixture();
            apply_editor_structure(&fresh, &create).unwrap();
            apply_editor_text(&fresh, "agent-block", 0, None, Some("Agent created 😀")).unwrap();
            fresh.commit();
            let base =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/tests/fixtures/editor-doc-v1");
            std::fs::write(
                base.join("rust-created.snapshot"),
                fresh.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn unknown_marks_and_private_dependency_attrs_fail_before_text_mutation() {
        let doc = fixture();
        let list = root_children(&doc).unwrap();
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(0) else {
            panic!("fixture map")
        };
        let text = text_children(&map).unwrap().remove(0);
        text.mark(0..1, "bold", true).unwrap();
        doc.commit();
        let before = doc.oplog_vv();
        assert!(apply_editor_text(&doc, &map.id().to_string(), 0, None, Some("bad")).is_err());
        assert_eq!(doc.oplog_vv(), before);
        let doc = fixture();
        let list = root_children(&doc).unwrap();
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(0) else {
            panic!("fixture map")
        };
        let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes") else {
            panic!("fixture attrs")
        };
        attrs.insert("embed_object_id", "private").unwrap();
        doc.commit();
        let before = doc.oplog_vv();
        assert!(apply_editor_text(&doc, &map.id().to_string(), 0, None, Some("bad")).is_err());
        assert_eq!(doc.oplog_vv(), before);
        assert_eq!(editor_blocks(&doc).unwrap().unwrap().len(), 3);
    }
    #[test]
    fn initialization_validates_all_input_and_never_overwrites_an_existing_doc() {
        let doc = LoroDoc::new();
        let bad = EditorInitialBlock {
            id: "legacy-id".into(),
            kind: "unknown".into(),
            text: "legacy".into(),
            indent: 0,
            level: None,
        };
        let frontier = doc.oplog_vv();
        assert!(initialize_editor_doc(&doc, &[bad.clone()]).is_err());
        assert_eq!(doc.oplog_vv(), frontier);
        assert!(root_children(&doc).is_none());
        let good = EditorInitialBlock {
            kind: "paragraph".into(),
            ..bad
        };
        assert!(initialize_editor_doc(&doc, &[good]).unwrap());
        let blocks = editor_blocks(&doc).unwrap().unwrap();
        assert_eq!(blocks.first().unwrap().text, "legacy");
        assert_eq!(blocks.first().unwrap().logical_id.as_deref(), Some("legacy-id"));
        let frontier = doc.oplog_vv();
        assert!(!initialize_editor_doc(&doc, &[]).unwrap());
        assert_eq!(doc.oplog_vv(), frontier);
        assert!(apply_editor_text(&doc, "legacy-id", 6, None, Some(" + agent")).unwrap());
        assert_eq!(
            editor_blocks(&doc).unwrap().unwrap().first().unwrap().text,
            "legacy + agent"
        );
        let fresh = LoroDoc::new();
        assert!(initialize_editor_doc(&fresh, &[]).unwrap());
        assert_eq!(editor_blocks(&fresh).unwrap().unwrap().len(), 1);
        if std::env::var("WRITE_EDITOR_INTEROP_FIXTURE").as_deref() == Ok("1") {
            let base =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/tests/fixtures/editor-doc-v1");
            std::fs::write(
                base.join("rust-initialized.snapshot"),
                doc.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
            std::fs::write(
                base.join("rust-empty.snapshot"),
                fresh.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn invalid_unicode_boundaries_and_unknown_ids_leave_frontier_unchanged() {
        let doc = fixture();
        let blocks = editor_blocks(&doc).unwrap().unwrap();
        let id = &blocks.first().unwrap().id;
        let before = doc.oplog_vv();
        assert!(apply_editor_text(&doc, id, 9, None, Some("bad")).is_err());
        assert!(apply_editor_text(&doc, id, 8, Some(1), None).is_err());
        assert!(!apply_editor_text(&doc, "tree-block", 0, None, Some("bad")).unwrap());
        assert_eq!(doc.oplog_vv(), before);
    }
}
