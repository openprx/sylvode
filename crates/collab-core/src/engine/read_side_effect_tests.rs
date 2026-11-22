//! Reads must not change what a document exports.
//!
//! Loro registers an empty root container when a typed getter asks for a root the document does
//! not have. The registration shows in `get_deep_value`, in snapshot bytes and in the list of root
//! containers, so every read entry point of the editor model is run here on documents that lack
//! the editor roots (or hold a same-named root of another type), and the three observable
//! surfaces are compared before and after.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use loro::{ContainerType, ExportMode, LoroDoc, VersionVector};

use super::{CollabEngine, LoroCollabEngine, model5_marker};
use crate::operation::{NodeId, NodeKind, Operation};

/// Names the editor readers look up as roots.
const NAMES: [&str; 5] = [
    "doc",
    crate::editor_structure::TREE,
    crate::editor_structure::META,
    "flow_editor_aliases",
    "order",
];

/// The exported value, the snapshot bytes and the root list of one document.
fn observe(doc: &LoroDoc) -> (String, Vec<u8>, String) {
    (
        format!("{:?}", doc.get_deep_value()),
        doc.export(ExportMode::Snapshot).unwrap(),
        format!("{:?}", doc.get_value()),
    )
}

/// Every document-level read entry point.
fn read_doc(doc: &LoroDoc) {
    let _ = crate::editor_structure::physical(doc);
    let _ = crate::editor_structure::ready(doc);
    let _ = crate::editor_structure::placements(doc);
    let _ = crate::editor_doc_bridge::editor_blocks(doc);
    let _ = crate::editor_doc_bridge::resolve_editor_block_id(doc, "missing");
    let _ = model5_marker(doc);
    let _ = super::physical_atoms_present(doc);
    let _ = crate::floor_guard::removal_scan(doc, &VersionVector::default());
    let _ = crate::floor_guard::physical_atoms_at(doc, &doc.state_frontiers());
}

/// Every engine-level read entry point, on top of the document-level ones.
fn read_engine(engine: &LoroCollabEngine) {
    let _ = engine.compute_minimum_editor_model();
    let _ = engine.minimum_editor_model();
    let _ = engine.semantic_snapshot();
    let _ = engine.semantic_snapshot_with_floor();
    let _ = engine.has_editor_doc();
    let _ = engine.has_editor_alias("missing");
    let _ = engine.has_editor_structure();
    let _ = engine.navigator_order();
    read_doc(&engine.doc);
}

pub(super) fn assert_unchanged(label: &str, doc: &LoroDoc, read: impl FnOnce()) {
    let before = observe(doc);
    read();
    let after = observe(doc);
    assert_eq!(before.0, after.0, "{label}: the exported deep value changed");
    assert_eq!(before.1, after.1, "{label}: the snapshot bytes changed");
    assert_eq!(before.2, after.2, "{label}: the list of root containers changed");
}

fn shadow(doc: &LoroDoc, name: &str, kind: ContainerType) {
    match kind {
        ContainerType::Text => doc.get_text(name).insert(0, "x").unwrap(),
        ContainerType::Map => doc.get_map(name).insert("k", 1).unwrap(),
        ContainerType::List => doc.get_list(name).insert(0, 1).unwrap(),
        _ => panic!("unsupported shadow kind"),
    }
}

fn legacy_page() -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(7001);
    for (index, id) in ["a", "b"].into_iter().enumerate() {
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: NodeId::from(id),
                parent: None,
                index: u32::try_from(index).unwrap(),
                kind: NodeKind::Block,
            })
            .unwrap();
    }
    engine
}

fn navigator() -> LoroCollabEngine {
    let mut engine = legacy_page();
    engine.set_navigator_order("a", "m").unwrap();
    engine
}

fn structured_page() -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(7002);
    engine.initialize_page_editor().unwrap();
    engine.initialize_page_structure_model(5).unwrap();
    engine
}

/// Documents as raw Loro docs: roots exactly as stored, none added by an engine.
pub(super) fn documents() -> Vec<(String, LoroDoc)> {
    let from = |engine: &LoroCollabEngine| {
        let doc = LoroDoc::new();
        doc.import(&engine.export_snapshot().unwrap()).unwrap();
        doc
    };
    let mut docs = vec![
        ("empty".to_string(), LoroDoc::new()),
        ("legacy page".to_string(), from(&legacy_page())),
        ("navigator".to_string(), from(&navigator())),
    ];
    for name in NAMES {
        for kind in [ContainerType::Text, ContainerType::Map, ContainerType::List] {
            let doc = LoroDoc::new();
            shadow(&doc, name, kind);
            docs.push((format!("only a {kind:?} root {name}"), doc));
        }
    }
    // A real page whose lookup names also exist as roots of another type.
    for base in [legacy_page(), structured_page()] {
        let doc = from(&base);
        for name in NAMES {
            for kind in [
                ContainerType::Text,
                ContainerType::List,
                ContainerType::Map,
                ContainerType::Tree,
            ] {
                if kind == ContainerType::Tree {
                    continue;
                }
                let copy = LoroDoc::new();
                copy.import(&doc.export(ExportMode::Snapshot).unwrap()).unwrap();
                // Skip the combinations where the root already has this type.
                if copy.get_value().as_map().is_some_and(|roots| roots.contains_key(name)) && kind == ContainerType::Map
                {
                    continue;
                }
                shadow(&copy, name, kind);
                copy.commit();
                docs.push((format!("page with shadow {kind:?} root {name}"), copy));
            }
        }
    }
    docs
}

#[test]
fn document_reads_register_no_root() {
    for (label, doc) in documents() {
        assert_unchanged(&label, &doc, || read_doc(&doc));
    }
}

#[test]
fn engine_reads_register_no_root() {
    for (label, doc) in documents() {
        let snapshot = doc.export(ExportMode::Snapshot).unwrap();
        // The engine's own roots (`tree`, `meta`) are registered when it loads; reads come after.
        let Ok(engine) = LoroCollabEngine::load(&snapshot) else {
            continue;
        };
        assert_unchanged(&label, &engine.doc, || read_engine(&engine));
    }
    for (label, engine) in [
        ("new empty", LoroCollabEngine::new_empty(7003)),
        ("legacy page", legacy_page()),
        ("navigator", navigator()),
        ("structured page", structured_page()),
    ] {
        assert_unchanged(label, &engine.doc, || read_engine(&engine));
    }
}

/// The existence check itself: a root of the asked type is found, and an absent root, or one of
/// another type, is not, whatever sits beside it.
#[test]
fn root_container_is_exact_and_registers_nothing() {
    use crate::editor_structure::root_container;
    let doc = LoroDoc::new();
    doc.get_map("m").insert("k", 1).unwrap();
    doc.get_text("both").insert(0, "t").unwrap();
    doc.get_map("both").insert("k", 1).unwrap();
    doc.commit();
    let before = observe(&doc);
    assert!(root_container(&doc, "m", ContainerType::Map).unwrap().is_some());
    assert!(root_container(&doc, "m", ContainerType::Text).unwrap().is_none());
    assert!(root_container(&doc, "m", ContainerType::Tree).unwrap().is_none());
    assert!(root_container(&doc, "absent", ContainerType::Map).unwrap().is_none());
    assert!(root_container(&doc, "both", ContainerType::Map).unwrap().is_some());
    assert!(root_container(&doc, "both", ContainerType::Text).unwrap().is_some());
    assert!(root_container(&doc, "both", ContainerType::List).unwrap().is_none());
    assert_eq!(before, observe(&doc));
}
