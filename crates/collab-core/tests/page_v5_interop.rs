use collab_core::{CollabEngine, LoroCollabEngine};
#[test]
fn actual_frontend_model5_packets_preserve_floor_and_identity_through_undo_redo() {
    for kind in ["image", "attachment", "childPage"] {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/page-v5");
        let before = std::fs::read(base.join(format!("{kind}.before.snapshot"))).unwrap();
        let mut engine = LoroCollabEngine::load(&before).unwrap();
        assert_eq!(engine.minimum_editor_model().unwrap(), 4);
        let v4 = engine.package_semantic_snapshot_v4().unwrap();
        engine
            .import_update(&std::fs::read(base.join(format!("{kind}.update"))).unwrap())
            .unwrap();
        assert_eq!(engine.minimum_editor_model().unwrap(), 5);
        assert!(engine.package_semantic_snapshot_v4().is_err());
        let active = engine.semantic_snapshot().unwrap();
        let cid = active
            .nodes
            .iter()
            .find(|(_, node)| node.properties.get("block_type").is_some_and(|value| value == kind))
            .unwrap()
            .0
            .clone();
        let undo = LoroCollabEngine::load(&std::fs::read(base.join(format!("{kind}.undo.snapshot"))).unwrap()).unwrap();
        assert_eq!(undo.minimum_editor_model().unwrap(), 5);
        assert!(!undo.semantic_snapshot().unwrap().nodes.contains_key(&cid));
        let redo = LoroCollabEngine::load(&std::fs::read(base.join(format!("{kind}.redo.snapshot"))).unwrap()).unwrap();
        assert_eq!(redo.minimum_editor_model().unwrap(), 5);
        assert_eq!(
            redo.semantic_snapshot()
                .unwrap()
                .nodes
                .get(&cid)
                .unwrap()
                .properties
                .get("block_type")
                .unwrap(),
            kind
        );
        let original = LoroCollabEngine::load(&before).unwrap();
        assert_eq!(original.package_semantic_snapshot_v4().unwrap(), v4);
    }
}
