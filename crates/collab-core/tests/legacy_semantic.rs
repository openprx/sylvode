//! A golden fixture produced by the unmodified d523d7c collab-core implementation.
use collab_core::{CollabEngine, LoroCollabEngine};

#[test]
fn old_package_fingerprint_is_exact_while_live_projection_keeps_editor_text() {
    let bytes = include_bytes!("fixtures/pre-inline-marks-d523d7c.snapshot");
    let old_json = include_bytes!("fixtures/pre-inline-marks-d523d7c.semantic.json");
    let old_hash = include_str!("fixtures/pre-inline-marks-d523d7c.sha256").trim();
    let engine = LoroCollabEngine::load(bytes).expect("legacy snapshot loads");
    let legacy = engine.legacy_package_semantic_snapshot_v1().expect("legacy projection");
    assert_eq!(legacy.canonical_json().expect("canonical JSON"), old_json);
    assert_eq!(legacy.semantic_hash(), old_hash);
    let current = engine.semantic_snapshot().expect("current projection");
    assert_ne!(
        current.semantic_hash(),
        old_hash,
        "modern projection must not silently drop editor data"
    );
    assert!(
        current
            .nodes
            .values()
            .any(|node| node.text == "Legacy browser text stays in the snapshot")
    );
    assert_eq!(legacy.nodes.get("api-block").unwrap().text, "Legacy API text");
    assert!(
        current.nodes.get("api-block").unwrap().text.is_empty(),
        "the initialized editor is authoritative over shadow API tree prose"
    );
}
