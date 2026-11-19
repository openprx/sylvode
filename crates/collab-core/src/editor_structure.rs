//! Native tree placement index over immutable physical editor block containers.
use crate::CollabError;
use loro::{Container, ContainerTrait, LoroDoc, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer};
use std::collections::BTreeSet;
pub const TREE: &str = "flow_editor_structure";
pub const META: &str = "flow_editor_structure_meta";
fn invalid(reason: &str) -> CollabError {
    CollabError::OperationFailed { reason: reason.into() }
}
fn value(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(value))) => Some(value.to_string()),
        _ => None,
    }
}
pub fn physical(doc: &LoroDoc) -> Result<Vec<LoroMap>, CollabError> {
    let Some(ValueOrContainer::Container(Container::Map(root))) = doc.get_by_str_path("doc") else {
        return Err(invalid("editor_doc_required"));
    };
    let Some(ValueOrContainer::Container(Container::List(list))) = root.get("children") else {
        return Err(invalid("editor_doc_required"));
    };
    (0..list.len())
        .map(|index| match list.get(index) {
            Some(ValueOrContainer::Container(Container::Map(map))) => Ok(map),
            _ => Err(invalid("editor_doc_unsupported_block_container")),
        })
        .collect()
}
pub fn ready(doc: &LoroDoc) -> bool {
    let Some(ValueOrContainer::Container(Container::Map(meta))) = doc.get_by_str_path(META) else {
        return false;
    };
    matches!(
        meta.get("initialized"),
        Some(ValueOrContainer::Value(LoroValue::Bool(true)))
    )
}
fn tree(doc: &LoroDoc) -> Result<LoroTree, CollabError> {
    let Some(ValueOrContainer::Container(Container::Tree(tree))) = doc.get_by_str_path(TREE) else {
        return Err(invalid("editor_structure_missing_tree"));
    };
    tree.enable_fractional_index(0);
    Ok(tree)
}
#[derive(Debug, Clone)]
pub struct Placement {
    pub cid: String,
    pub parent: Option<String>,
    pub depth: u32,
    pub tree_id: TreeID,
    pub position: usize,
}
pub fn placements(doc: &LoroDoc) -> Result<Option<Vec<Placement>>, CollabError> {
    if !ready(doc) {
        return Ok(None);
    }
    let meta = doc.get_map(META);
    if !matches!(
        meta.get("version"),
        Some(ValueOrContainer::Value(LoroValue::I64(1) | LoroValue::Double(1.0)))
    ) || !matches!(value(&meta, "model").as_deref(), Some("rich_page_v4" | "rich_page_v5"))
    {
        return Err(invalid("editor_structure_model_unsupported"));
    }
    let tree = tree(doc)?;
    let maps = physical(doc)?;
    let physical: BTreeSet<_> = maps.iter().map(|map| map.id().to_string()).collect();
    let mut all = BTreeSet::new();
    // Every node's `block_cid`, read once here and reused for the active traversal below.
    let mut cids = std::collections::HashMap::new();
    for node in tree.nodes() {
        let Some(cid) = value(&tree.get_meta(node).map_err(|e| invalid(&e.to_string()))?, "block_cid") else {
            continue;
        };
        cids.insert(node, cid.clone());
        let known = physical.contains(&cid)
            || loro::ContainerID::try_from(cid.as_str())
                .ok()
                .and_then(|id| doc.get_container(id))
                .is_some_and(|container| matches!(container, Container::Map(_)));
        if !known {
            return Err(invalid("editor_structure_invalid_block_cid"));
        }
        all.insert(cid);
    }
    if physical.iter().any(|cid| !all.contains(cid)) {
        return Err(invalid("editor_structure_unindexed_physical_block"));
    }
    let output = active_placements_with(&tree, |node| {
        cids.get(&node)
            .map_or_else(|| block_cid(&tree, node), |cid| Ok(Some(cid.clone())))
    })?;
    if output.iter().any(|entry| !physical.contains(&entry.cid)) {
        return Err(invalid("editor_structure_active_block_not_in_storage"));
    }
    Ok(Some(output))
}
fn block_cid(tree: &LoroTree, node: TreeID) -> Result<Option<String>, CollabError> {
    Ok(value(
        &tree.get_meta(node).map_err(|e| invalid(&e.to_string()))?,
        "block_cid",
    ))
}
fn active_placements(tree: &LoroTree) -> Result<Vec<Placement>, CollabError> {
    active_placements_with(tree, |node| block_cid(tree, node))
}
/// The active (non-deleted) placements in document order. `cid_of` returns a node's
/// `block_cid` exactly as [`block_cid`] would (it may answer from values already read).
fn active_placements_with(
    tree: &LoroTree,
    cid_of: impl Fn(TreeID) -> Result<Option<String>, CollabError>,
) -> Result<Vec<Placement>, CollabError> {
    let mut output = Vec::new();
    let mut active = BTreeSet::new();
    let mut stack: Vec<_> = tree
        .roots()
        .into_iter()
        .enumerate()
        .rev()
        .map(|(index, node)| (node, None, 0, index))
        .collect();
    while let Some((node, parent, depth, position)) = stack.pop() {
        if depth > 7 {
            return Err(invalid("editor_structure_depth_limit"));
        }
        let cid = cid_of(node)?.ok_or_else(|| invalid("editor_structure_missing_block_cid"))?;
        if !active.insert(cid.clone()) {
            return Err(invalid("editor_structure_duplicate_active_block_cid"));
        }
        output.push(Placement {
            cid: cid.clone(),
            parent,
            depth,
            tree_id: node,
            position,
        });
        for (index, child) in tree.children(node).unwrap_or_default().into_iter().enumerate().rev() {
            stack.push((child, Some(cid.clone()), depth + 1, index));
        }
    }
    if output.is_empty() {
        return Err(invalid("editor_doc_requires_one_block"));
    }
    Ok(output)
}
pub fn initialize(doc: &LoroDoc) -> Result<bool, CollabError> {
    if ready(doc) {
        placements(doc)?;
        return Ok(false);
    }
    if doc.get_by_str_path(TREE).is_some_and(|value| match value {
        ValueOrContainer::Container(Container::Tree(tree)) => !tree.roots().is_empty(),
        _ => true,
    }) {
        return Err(invalid("editor_structure_partial_initialization"));
    }
    let maps = physical(doc)?;
    let readable = crate::editor_doc_bridge::editor_blocks(doc)?.ok_or_else(|| invalid("editor_doc_required"))?;
    if readable.len() != maps.len() || maps.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX {
        return Err(invalid("editor_structure_unsupported_physical_block"));
    }
    let mut depths = Vec::new();
    for map in &maps {
        let depth = match map.get("attributes") {
            Some(ValueOrContainer::Container(Container::Map(attrs))) => match attrs.get("indent") {
                Some(ValueOrContainer::Value(LoroValue::I64(value))) => {
                    u32::try_from(value).map_err(|_| invalid("editor_structure_invalid_indent"))?
                }
                Some(ValueOrContainer::Value(LoroValue::Double(value)))
                    if value.fract() == 0.0 && (0.0..=7.0).contains(&value) =>
                {
                    (0..=7)
                        .find(|depth| f64::from(*depth).to_bits() == value.to_bits())
                        .ok_or_else(|| invalid("editor_structure_invalid_indent"))?
                }
                _ => 0,
            },
            _ => 0,
        };
        if depth > 7 || depths.last().is_none_or(|previous| depth > previous + 1) && depth != 0 {
            return Err(invalid("editor_structure_invalid_indent"));
        }
        depths.push(depth);
    }
    if maps.is_empty() {
        return Err(invalid("editor_doc_requires_one_block"));
    }
    let tree = doc.get_tree(TREE);
    tree.enable_fractional_index(0);
    let mut ancestors = Vec::new();
    for (map, depth) in maps.iter().zip(depths) {
        ancestors.truncate(depth as usize);
        let node = tree
            .create(ancestors.last().copied())
            .map_err(|e| invalid(&e.to_string()))?;
        tree.get_meta(node)
            .map_err(|e| invalid(&e.to_string()))?
            .insert("block_cid", map.id().to_string())
            .map_err(|e| invalid(&e.to_string()))?;
        ancestors.push(node);
    }
    let meta = doc.get_map(META);
    meta.insert("version", 1).map_err(|e| invalid(&e.to_string()))?;
    meta.insert("initialized", true).map_err(|e| invalid(&e.to_string()))?;
    meta.insert("model", "rich_page_v4")
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(true)
}
pub fn insert(doc: &LoroDoc, cid: &str, parent: Option<&str>, index: usize) -> Result<(), CollabError> {
    let tree = tree(doc)?;
    let placements = active_placements(&tree)?;
    let parent = parent
        .map(|cid| {
            placements
                .iter()
                .find(|entry| entry.cid == cid)
                .map(|entry| entry.tree_id)
                .ok_or_else(|| invalid("editor_structure_parent_not_found"))
        })
        .transpose()?;
    let node = tree.create_at(parent, index).map_err(|e| invalid(&e.to_string()))?;
    tree.get_meta(node)
        .map_err(|e| invalid(&e.to_string()))?
        .insert("block_cid", cid)
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(())
}
pub fn move_block(doc: &LoroDoc, cid: &str, parent: Option<&str>, index: usize) -> Result<(), CollabError> {
    let placements = placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
    let node = placements
        .iter()
        .find(|entry| entry.cid == cid)
        .ok_or_else(|| invalid("editor_block_not_found"))?;
    let parent = parent
        .map(|cid| {
            placements
                .iter()
                .find(|entry| entry.cid == cid)
                .map(|entry| entry.tree_id)
                .ok_or_else(|| invalid("editor_structure_parent_not_found"))
        })
        .transpose()?;
    let parent_depth = parent
        .and_then(|id| {
            placements
                .iter()
                .find(|entry| entry.tree_id == id)
                .map(|entry| entry.depth + 1)
        })
        .unwrap_or(0);
    let height = placements
        .iter()
        .skip_while(|entry| entry.tree_id != node.tree_id)
        .skip(1)
        .take_while(|entry| entry.depth > node.depth)
        .map(|entry| entry.depth - node.depth)
        .max()
        .unwrap_or(0);
    if parent_depth + height > 7 {
        return Err(invalid("editor_structure_depth_limit"));
    }
    let mut ancestor = parent;
    while let Some(id) = ancestor {
        if id == node.tree_id {
            return Err(invalid("editor_structure_cycle"));
        }
        ancestor = match tree(doc)?.parent(id) {
            Some(TreeParentId::Node(parent)) => Some(parent),
            _ => None,
        };
    }
    tree(doc)?
        .mov_to(node.tree_id, parent, index)
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(())
}
pub fn delete(doc: &LoroDoc, cid: &str) -> Result<(), CollabError> {
    let placements = placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
    let node = placements
        .iter()
        .find(|entry| entry.cid == cid)
        .ok_or_else(|| invalid("editor_block_not_found"))?;
    let mut remaining = 0;
    for placement in &placements {
        let mut ancestor = Some(placement.tree_id);
        let mut in_subtree = false;
        while let Some(id) = ancestor {
            if id == node.tree_id {
                in_subtree = true;
                break;
            }
            ancestor = match tree(doc)?.parent(id) {
                Some(TreeParentId::Node(parent)) => Some(parent),
                _ => None,
            };
        }
        if !in_subtree {
            remaining += 1;
        }
    }
    if remaining == 0 {
        return Err(invalid("editor_doc_requires_one_block"));
    }
    tree(doc)?.delete(node.tree_id).map_err(|e| invalid(&e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CollabEngine, LoroCollabEngine, NodeKind, Operation};
    const SNAPSHOT: &[u8] = include_bytes!("../../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot");
    #[test]
    fn historical_fork_retains_deleted_placement_metadata_and_ignores_later_ops() {
        let mut engine = LoroCollabEngine::new_empty(97);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure_model(5).unwrap();
        let placeholder = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: "survivor".into(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::DeleteNode { id: placeholder })
            .unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let historical = engine.frontier();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: "survivor".into(),
                index: 0,
                text: "future".into(),
            })
            .unwrap();
        let fork = engine.fork_at_frontier(&historical).unwrap();
        assert_eq!(fork.semantic_snapshot().unwrap(), before);
        assert_eq!(fork.frontier(), historical);
        let loaded = LoroCollabEngine::load(&fork.export_snapshot().unwrap()).unwrap();
        assert_eq!(loaded.semantic_snapshot().unwrap(), before);
        assert_eq!(loaded.frontier(), historical);
    }

    #[test]
    fn native_tree_move_nest_delete_create_preserve_content_cids_and_roundtrip() {
        let mut engine = LoroCollabEngine::load(SNAPSHOT).unwrap();
        let before = engine.semantic_snapshot().unwrap();
        assert!(!before.nodes.is_empty());
        assert!(engine.initialize_page_structure().unwrap());
        assert!(!engine.initialize_page_structure().unwrap());
        let indexed = engine.semantic_snapshot().unwrap();
        let roots: Vec<_> = indexed
            .nodes
            .iter()
            .filter(|(_, node)| node.parent.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        let first = roots.first().unwrap().clone();
        let second = roots.get(1).unwrap().clone();
        engine
            .apply_content_operation(&Operation::MoveNode {
                id: first.clone(),
                new_parent: Some(second.clone()),
                index: 0,
            })
            .unwrap();
        let nested = engine.semantic_snapshot().unwrap();
        assert_eq!(nested.nodes.get(&first).unwrap().parent.as_ref(), Some(&second));
        engine
            .apply_content_operation(&Operation::InsertText {
                id: first.clone(),
                index: 0,
                text: "Moved ".into(),
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: "new-indexed".into(),
                parent: Some(second.clone()),
                index: 1,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: "new-indexed".into(),
                index: 0,
                text: "New".into(),
            })
            .unwrap();
        let created = engine.resolve_content_node_id(&"new-indexed".into());
        assert_eq!(
            engine
                .semantic_snapshot()
                .unwrap()
                .nodes
                .get(&created)
                .unwrap()
                .parent
                .as_ref(),
            Some(&second)
        );
        engine
            .apply_content_operation(&Operation::DeleteNode { id: first.clone() })
            .unwrap();
        let after = engine.semantic_snapshot().unwrap();
        assert!(!after.nodes.contains_key(&first));
        assert!(after.nodes.contains_key(&second));
        let loaded = LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
        assert_eq!(loaded.semantic_snapshot().unwrap(), after);
        assert_eq!(loaded.minimum_editor_model().unwrap(), 4);
        if std::env::var("WRITE_PAGE_V4_FIXTURE").as_deref() == Ok("1") {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/page-v4");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("rust-structured.snapshot"), engine.export_snapshot().unwrap()).unwrap();
            std::fs::write(
                root.join("rust-structured.semantic.json"),
                serde_json::to_vec_pretty(&after).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn concurrent_uncoordinated_initializers_are_detected_not_silently_merged() {
        let source = LoroDoc::new();
        source.import(SNAPSHOT).unwrap();
        let a = source.fork();
        let b = source.fork();
        a.set_peer_id(9001).unwrap();
        b.set_peer_id(9002).unwrap();
        initialize(&a).unwrap();
        initialize(&b).unwrap();
        a.commit();
        b.commit();
        a.import(&b.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
        assert!(placements(&a).is_err());
    }
}

#[cfg(test)]
mod js_interop_tests {
    use crate::{CollabEngine, LoroCollabEngine, Operation};
    #[test]
    fn actual_js_rich_tree_snapshot_accepts_rust_cid_edit_and_move() {
        let bytes = include_bytes!("../tests/fixtures/page-v4/js-rich.snapshot");
        let mut engine = LoroCollabEngine::load(bytes).unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let unicode = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "o中文😀ne")
            .unwrap()
            .0
            .clone();
        let three = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "three")
            .unwrap()
            .0
            .clone();
        let two = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "two")
            .unwrap()
            .0
            .clone();
        assert_eq!(before.nodes.get(&two).unwrap().parent.as_ref(), Some(&unicode));
        let spans = before
            .nodes
            .get(&unicode)
            .unwrap()
            .properties
            .get("inline_spans")
            .unwrap()
            .clone();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: unicode.clone(),
                index: u32::try_from(before.nodes.get(&unicode).unwrap().text.len()).unwrap(),
                text: " Rust".into(),
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::MoveNode {
                id: three.clone(),
                new_parent: None,
                index: 1,
            })
            .unwrap();
        let after = engine.semantic_snapshot().unwrap();
        assert_eq!(after.nodes.get(&unicode).unwrap().text, "o中文😀ne Rust");
        assert_eq!(
            after
                .nodes
                .get(&unicode)
                .unwrap()
                .properties
                .get("inline_spans")
                .unwrap(),
            &spans
        );
        let mut order: Vec<_> = after
            .nodes
            .iter()
            .filter(|(_, node)| node.properties.get("content_source").is_some_and(|v| v == "editor_doc"))
            .collect();
        order.sort_by_key(|(_, node)| &node.order_key);
        assert_eq!(
            order.iter().map(|(id, _)| (*id).clone()).collect::<Vec<_>>(),
            [unicode, two, three]
        );
        if std::env::var("WRITE_PAGE_V4_FIXTURE").as_deref() == Ok("1") {
            std::fs::write("/tmp/sv-page-rich-v4-rust.snapshot", engine.export_snapshot().unwrap()).unwrap();
        }
    }
}

#[cfg(test)]
mod native_undo_interop_tests {
    use crate::{CollabEngine, LoroCollabEngine};
    #[test]
    fn actual_js_native_tree_delete_undo_keeps_block_cid_with_new_placement_generation() {
        let mut engine =
            LoroCollabEngine::load(include_bytes!("../tests/fixtures/page-v4/js-tree-delete-undo.snapshot")).unwrap();
        let semantic = engine.semantic_snapshot().unwrap();
        let cid = std::sync::Arc::<str>::from("cid:21@991001:Map");
        assert_eq!(semantic.nodes.get(&cid).unwrap().text, "three");
        let parent = std::sync::Arc::<str>::from("cid:3@991001:Map");
        engine
            .apply_content_operation(&crate::Operation::MoveNode {
                id: cid.clone(),
                new_parent: Some(parent.clone()),
                index: 1,
            })
            .unwrap();
        assert_eq!(
            engine
                .semantic_snapshot()
                .unwrap()
                .nodes
                .get(&cid)
                .unwrap()
                .parent
                .as_ref(),
            Some(&parent)
        );
        engine
            .apply_content_operation(&crate::Operation::CreateNode {
                id: "restored-child".into(),
                parent: Some(cid.clone()),
                index: 0,
                kind: crate::NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&crate::Operation::InsertText {
                id: "restored-child".into(),
                index: 0,
                text: "grandchild".into(),
            })
            .unwrap();
        engine
            .apply_content_operation(&crate::Operation::InsertText {
                id: cid.clone(),
                index: 0,
                text: "Restored ".into(),
            })
            .unwrap();
        let snapshot = engine.export_snapshot().unwrap();
        if std::env::var("WRITE_PAGE_V4_FIXTURE").as_deref() == Ok("1") {
            std::fs::write("/tmp/sv-page-tree-delete-undo-rust.snapshot", &snapshot).unwrap();
        }

        let loaded = LoroCollabEngine::load(&snapshot).unwrap();
        assert_eq!(
            loaded.semantic_snapshot().unwrap().nodes.get(&cid).unwrap().text,
            "Restored three"
        );
    }
}

#[cfg(test)]
mod creation_undo_tests {
    use crate::{CollabEngine, LoroCollabEngine};
    #[test]
    fn native_creation_undo_allows_known_storage_tombstone_but_no_live_dangling_reference() {
        let dangling = LoroCollabEngine::load(include_bytes!(
            "../tests/fixtures/page-v4/js-tree-create-redo-dangling.snapshot"
        ))
        .unwrap();
        assert!(dangling.semantic_snapshot().is_err());
        let engine =
            LoroCollabEngine::load(include_bytes!("../tests/fixtures/page-v4/js-tree-create-undo.snapshot")).unwrap();
        let semantic = engine.semantic_snapshot().unwrap();
        assert!(!semantic.nodes.values().any(|node| node.text == "new synthetic"));
        assert_eq!(
            semantic
                .nodes
                .values()
                .filter(|node| node
                    .properties
                    .get("content_source")
                    .is_some_and(|value| value == "editor_doc"))
                .count(),
            3
        );
    }
}
