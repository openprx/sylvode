//! What the incremental window check relies on in the library, pinned so that a Loro upgrade that
//! changes any of it fails here and not in production:
//!
//! * the path of a container from the root has the shape the check maps units from (a block under
//!   `doc.children`, a legacy tree node's metadata and its mergeable text under the node);
//! * a container that the document no longer reaches has no path;
//! * asking for a path registers nothing: the exported value, the roots and the history are the
//!   same afterwards;
//! * the list value of `doc.children` names every element by its container id and equals the
//!   elements read one by one;
//! * the ids the check keys on agree with the projection (a block's snapshot key is its container
//!   id's string).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use loro::{Container, ContainerID, ContainerTrait, ContainerType, ExportMode, Index, LoroDoc, ValueOrContainer};

use super::floor_equivalence_tests::{children, page, raw_update};
use super::{CollabEngine, LoroCollabEngine};
use crate::operation::{NodeId, NodeKind, Operation};

fn block_at(doc: &LoroDoc, index: usize) -> loro::LoroMap {
    match children(doc).get(index) {
        Some(ValueOrContainer::Container(Container::Map(map))) => map,
        _ => panic!("no block at {index}"),
    }
}

fn inline_text(block: &loro::LoroMap) -> loro::LoroText {
    let Some(ValueOrContainer::Container(Container::List(list))) = block.get("children") else {
        panic!("no inline list");
    };
    match list.get(0) {
        Some(ValueOrContainer::Container(Container::Text(text))) => text,
        _ => panic!("no inline text"),
    }
}

/// A page whose first block has an inline text.
fn typed_page() -> LoroCollabEngine {
    let mut engine = page(1, 4, 5);
    engine
        .apply_content_operation(&Operation::InsertText {
            id: NodeId::from("s2"),
            index: 0,
            text: "typed".into(),
        })
        .unwrap();
    engine
}

#[test]
fn paths_of_editor_containers_go_through_their_block() {
    let engine = typed_page();
    let doc = &engine.doc;
    let list = children(doc);
    let mut found_text = false;
    for index in 0..list.len() {
        let block = block_at(doc, index);
        let block_id = block.id();
        let path = doc.get_path_to_container(&block_id).unwrap();
        assert_eq!(path.len(), 3, "{path:?}");
        assert!(
            matches!(&path[0], (ContainerID::Root { name, container_type: ContainerType::Map }, _) if name.as_ref() == "doc")
        );
        assert!(matches!(&path[1].1, Index::Key(key) if key.as_ref() == "children"));
        assert_eq!(path[2].0, block_id);
        assert!(matches!(path[2].1, Index::Seq(_)));
        if let Some(ValueOrContainer::Container(Container::List(inline))) = block.get("children")
            && !inline.is_empty()
        {
            let text = inline_text(&block);
            let path = doc.get_path_to_container(&text.id()).unwrap();
            assert_eq!(path[2].0, block_id, "a text is mapped to its block");
            assert!(path.len() >= 4);
            found_text = true;
        }
        if let Some(ValueOrContainer::Container(Container::Map(attributes))) = block.get("attributes") {
            let path = doc.get_path_to_container(&attributes.id()).unwrap();
            assert_eq!(path[2].0, block_id, "attributes are mapped to their block");
        }
    }
    assert!(found_text, "the page has no inline text to check");
}

#[test]
fn paths_of_legacy_tree_containers_go_through_their_node() {
    let mut engine = LoroCollabEngine::new_empty(11);
    for id in ["a", "b"] {
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: NodeId::from(id),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
    }
    engine
        .apply_content_operation(&Operation::InsertText {
            id: NodeId::from("a"),
            index: 0,
            text: "typed".into(),
        })
        .unwrap();
    let doc = &engine.doc;
    let tree = doc.get_tree("tree");
    let mut mergeable_seen = false;
    for node in tree.nodes() {
        let meta = tree.get_meta(node).unwrap();
        let path = doc.get_path_to_container(&meta.id()).unwrap();
        assert!(
            matches!(&path[0], (ContainerID::Root { name, container_type: ContainerType::Tree }, _) if name.as_ref() == "tree")
        );
        assert!(matches!(path[1].1, Index::Node(found) if found == node), "{path:?}");
        if let Some(ValueOrContainer::Container(Container::Text(text))) = meta.get("text") {
            // A mergeable child is named like a root container but belongs to its map.
            assert!(
                text.id().is_mergeable(),
                "the text of a tree node is a mergeable container"
            );
            let path = doc.get_path_to_container(&text.id()).unwrap();
            assert!(matches!(path[1].1, Index::Node(found) if found == node), "{path:?}");
            mergeable_seen = true;
        }
    }
    assert!(mergeable_seen);
}

#[test]
fn a_container_the_document_no_longer_reaches_has_no_path() {
    let engine = typed_page();
    let doc = &engine.doc;
    let block = block_at(doc, 0);
    let text = block.get("children").and_then(|inline| match inline {
        ValueOrContainer::Container(Container::List(list)) => list.get(0),
        _ => None,
    });
    let block_id = block.id();
    let text_id = match text {
        Some(ValueOrContainer::Container(container)) => Some(container.id()),
        _ => None,
    };
    children(doc).delete(0, 1).unwrap();
    doc.commit();
    assert!(
        doc.get_path_to_container(&block_id).is_none(),
        "a deleted block has a path"
    );
    if let Some(text_id) = text_id {
        assert!(
            doc.get_path_to_container(&text_id).is_none(),
            "a text under a deleted block has a path"
        );
    }
}

/// Asking for a path registers nothing: every exported surface is the same afterwards.
#[test]
fn path_queries_register_nothing() {
    let author = typed_page();
    let base = author.export_snapshot().unwrap();
    let update = raw_update(&author, 4242, |doc| {
        let block = block_at(doc, 1);
        inline_text(&block);
        children(doc)
            .insert_container(0, loro::LoroMap::new())
            .unwrap()
            .insert("nodeName", "paragraph")
            .unwrap();
    });
    let mut engine = LoroCollabEngine::load(&base).unwrap();
    engine.import_client_update(&update).unwrap();
    let doc = &engine.doc;
    let observe = || {
        (
            format!("{:?}", doc.get_deep_value()),
            format!("{:?}", doc.get_value()),
            doc.export(ExportMode::all_updates()).unwrap(),
        )
    };
    let before = observe();
    let list = children(doc);
    for index in 0..list.len() {
        let block = block_at(doc, index);
        let _ = doc.get_path_to_container(&block.id());
        for key in ["attributes", "children"] {
            if let Some(ValueOrContainer::Container(container)) = block.get(key) {
                let _ = doc.get_path_to_container(&container.id());
            }
        }
    }
    let _ = doc.get_path_to_container(&ContainerID::new_root("doc", ContainerType::Map));
    assert_eq!(before, observe(), "a path query changed what the document exports");
}

/// The value of the root list names every element by its container id, in order, exactly as the
/// elements read one by one do.
#[test]
fn the_list_value_names_the_blocks_in_order() {
    let engine = typed_page();
    let ids = engine.root_block_ids().unwrap().unwrap();
    let list = children(&engine.doc);
    assert_eq!(ids.len(), list.len());
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(*id, block_at(&engine.doc, index).id());
    }
    // And the projection's key of a block is the string of that id.
    let snapshot = engine.semantic_snapshot().unwrap();
    for id in &ids {
        assert!(snapshot.nodes.contains_key(id.to_string().as_str()), "{id}");
    }
    // An update that deletes by position changes the value as it changes the elements.
    let engine = typed_page();
    let before = engine.root_block_ids().unwrap().unwrap();
    children(&engine.doc).delete(1, 2).unwrap();
    engine.doc.commit();
    let after = engine.root_block_ids().unwrap().unwrap();
    let mut expected = before;
    expected.drain(1..3);
    assert_eq!(after, expected);
}
