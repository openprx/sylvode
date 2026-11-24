//! A snapshot that holds an API tree block and an initialized editor document side by side.
use collab_core::{CollabEngine, LoroCollabEngine};

#[test]
fn the_projection_keeps_editor_text_and_clears_shadow_tree_prose() {
    let bytes = include_bytes!("fixtures/tree-block-with-editor-doc.snapshot");
    let engine = LoroCollabEngine::load(bytes).expect("snapshot loads");
    let current = engine.semantic_snapshot().expect("current projection");
    assert!(
        current
            .nodes
            .values()
            .any(|node| node.text == "Legacy browser text stays in the snapshot")
    );
    assert!(
        current.nodes.get("api-block").unwrap().text.is_empty(),
        "the initialized editor is authoritative over shadow API tree prose"
    );
}
