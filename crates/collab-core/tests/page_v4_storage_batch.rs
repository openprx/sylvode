//! Real JS native Undo groups and atomic local-update batching, using synthetic Page data.
use collab_core::{CollabEngine, LoroCollabEngine};
const BASE: &[u8] = include_bytes!("fixtures/page-v4/js-rich.snapshot");
#[test]
fn aggregate_packet_and_storage_excluded_redo_preserve_the_original_carrier() {
    let mut engine = LoroCollabEngine::load(BASE).unwrap();
    engine
        .import_update(include_bytes!("fixtures/page-v4/js-storage-batched-create.update"))
        .unwrap();
    let cid = std::sync::Arc::<str>::from("cid:0@991010:Map");
    assert_eq!(
        engine.semantic_snapshot().unwrap().nodes.get(&cid).unwrap().text,
        "carrier synthetic"
    );
    let redo = LoroCollabEngine::load(include_bytes!("fixtures/page-v4/js-storage-batched-redo.snapshot")).unwrap();
    assert_eq!(
        redo.semantic_snapshot().unwrap().nodes.get(&cid).unwrap().text,
        "carrier synthetic"
    );
}
#[test]
fn intermediate_unindexed_storage_chunk_is_not_an_admissible_page_update() {
    let mut engine = LoroCollabEngine::load(BASE).unwrap();
    let imported = engine.import_update(include_bytes!("fixtures/page-v4/js-storage-carrier-only.update"));
    assert!(imported.is_err() || engine.semantic_snapshot().is_err());
}

#[test]
fn actual_production_vendor_split_packet_and_redo_keep_storage_index_consistent() {
    let mut engine =
        LoroCollabEngine::load(include_bytes!("fixtures/page-v4/js-storage-actual.before.snapshot")).unwrap();
    engine
        .import_update(include_bytes!("fixtures/page-v4/js-storage-actual.split.update"))
        .unwrap();
    let ordered = |engine: &LoroCollabEngine| {
        let snapshot = engine.semantic_snapshot().unwrap();
        let mut nodes: Vec<_> = snapshot
            .nodes
            .values()
            .filter(|node| {
                node.properties
                    .get("content_source")
                    .is_some_and(|source| source == "editor_doc")
            })
            .collect();
        nodes.sort_by_key(|node| &node.order_key);
        nodes.iter().map(|node| node.text.clone()).collect::<Vec<_>>()
    };
    assert_eq!(ordered(&engine), ["o", "ne", "two", "three"]);
    let carrier = std::sync::Arc::<str>::from("cid:41@991001:Map");
    assert_eq!(
        engine.semantic_snapshot().unwrap().nodes.get(&carrier).unwrap().text,
        "ne"
    );
    let redo = LoroCollabEngine::load(include_bytes!("fixtures/page-v4/js-storage-actual.redo.snapshot")).unwrap();
    assert_eq!(ordered(&redo), ["o", "ne!", "two", "three"]);
    assert_eq!(
        redo.semantic_snapshot().unwrap().nodes.get(&carrier).unwrap().text,
        "ne!"
    );
}

#[test]
fn actual_creation_undo_redo_outbound_packets_are_each_valid_and_keep_the_physical_carrier() {
    let mut engine =
        LoroCollabEngine::load(include_bytes!("fixtures/page-v4/js-storage-actual.before.snapshot")).unwrap();
    let texts = |engine: &LoroCollabEngine| {
        let snapshot = engine.semantic_snapshot().unwrap();
        let mut nodes: Vec<_> = snapshot
            .nodes
            .values()
            .filter(|node| {
                node.properties
                    .get("content_source")
                    .is_some_and(|source| source == "editor_doc")
            })
            .collect();
        nodes.sort_by_key(|node| &node.order_key);
        nodes.iter().map(|node| node.text.clone()).collect::<Vec<_>>()
    };
    engine
        .import_update(include_bytes!("fixtures/page-v4/js-storage-actual.split.update"))
        .unwrap();
    assert_eq!(texts(&engine), ["o", "ne", "two", "three"]);
    engine
        .import_update(include_bytes!("fixtures/page-v4/js-storage-actual.undo.update"))
        .unwrap();
    assert_eq!(texts(&engine), ["one", "two", "three"]);
    assert_eq!(
        engine
            .root_state_json()
            .get("doc")
            .and_then(|doc| doc.get("children"))
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(4)
    );
    let undo = LoroCollabEngine::load(include_bytes!("fixtures/page-v4/js-storage-actual.undo.snapshot")).unwrap();
    assert_eq!(engine.semantic_snapshot().unwrap(), undo.semantic_snapshot().unwrap());
    engine
        .import_update(include_bytes!("fixtures/page-v4/js-storage-actual.redo.update"))
        .unwrap();
    assert_eq!(texts(&engine), ["o", "ne", "two", "three"]);
    let carrier = std::sync::Arc::<str>::from("cid:41@991001:Map");
    assert_eq!(
        engine.semantic_snapshot().unwrap().nodes.get(&carrier).unwrap().text,
        "ne"
    );
    let redo = LoroCollabEngine::load(include_bytes!(
        "fixtures/page-v4/js-storage-actual.redo-before-edit.snapshot"
    ))
    .unwrap();
    assert_eq!(engine.semantic_snapshot().unwrap(), redo.semantic_snapshot().unwrap());
}
