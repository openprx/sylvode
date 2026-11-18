use collab_core::{CollabEngine, LoroCollabEngine, SemanticSnapshot};

fn bytes(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/page-cut")
            .join(format!("sv-page-cut.{name}")),
    )
}

fn order(semantic: &SemanticSnapshot) -> Vec<String> {
    let mut nodes = semantic.nodes.iter().collect::<Vec<_>>();
    nodes.sort_by_key(|(_, node)| &node.order_key);
    nodes.into_iter().map(|(id, _)| id.to_string()).collect()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    ids: Vec<String>,
    placeholder: String,
    preorder_after_paste: Vec<String>,
}

#[test]
fn actual_frontend_cut_paste_packets_keep_cids_marks_and_valid_undo_states() -> Result<(), Box<dyn std::error::Error>> {
    let manifest: Manifest = serde_json::from_slice(&bytes("manifest.json")?)?;
    let mut engine = LoroCollabEngine::load(&bytes("before.snapshot")?)?;
    let original = engine.semantic_snapshot()?;
    assert_eq!(order(&original), manifest.ids);
    engine.import_update(&bytes("cut.update")?)?;
    assert_eq!(order(&engine.semantic_snapshot()?), vec![manifest.placeholder.clone()]);
    engine.import_update(&bytes("paste.update")?)?;
    let restored = engine.semantic_snapshot()?;
    assert_eq!(order(&restored), manifest.preorder_after_paste);
    for id in &manifest.ids {
        let old = original.nodes.get(id.as_str()).unwrap();
        let new = restored.nodes.get(id.as_str()).unwrap();
        assert_eq!(old.text, new.text);
        assert_eq!(old.properties.get("inline_spans"), new.properties.get("inline_spans"));
        assert_eq!(old.properties.get("indent"), new.properties.get("indent"));
    }
    let reopened = LoroCollabEngine::load(&bytes("paste.snapshot")?)?;
    assert_eq!(order(&reopened.semantic_snapshot()?), manifest.preorder_after_paste);
    let undo_paste = LoroCollabEngine::load(&bytes("undo-paste.snapshot")?)?;
    assert_eq!(order(&undo_paste.semantic_snapshot()?), vec![manifest.placeholder]);
    let undo_cut = LoroCollabEngine::load(&bytes("undo-cut.snapshot")?)?;
    assert_eq!(order(&undo_cut.semantic_snapshot()?), manifest.ids);
    Ok(())
}
