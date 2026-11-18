#[path = "../src/flow/page_transfer_ir.rs"]
#[allow(dead_code)]
mod ir;
use collab_core::{LoroCollabEngine, NodeKind, Operation};
#[test]
fn selected_ir_keeps_native_marks_and_never_private_siblings_or_metadata() {
    let mut doc = LoroCollabEngine::new_empty(992_441);
    doc.initialize_page_editor().unwrap();
    doc.initialize_page_structure().unwrap();
    let initial = doc.semantic_snapshot().unwrap();
    let root = initial.nodes.keys().next().unwrap().clone();
    doc.apply_content_operation(&Operation::InsertText {
        id: root.clone(),
        index: 0,
        text: "中文😀".into(),
    })
    .unwrap();
    doc.apply_page_marks(
        root.as_ref(),
        &[collab_core::page_inline::MarkEdit {
            start: 0,
            end: 10,
            mark: collab_core::page_inline::InlineMark::Bold,
            clear: false,
            href: None,
        }],
    )
    .unwrap();
    doc.apply_content_operation(&Operation::CreateNode {
        id: "private-other".into(),
        parent: None,
        index: 1,
        kind: NodeKind::Block,
    })
    .unwrap();
    doc.apply_content_operation(&Operation::InsertText {
        id: "private-other".into(),
        index: 0,
        text: "PRIVATE_SIBLING".into(),
    })
    .unwrap();
    let all = ir::read(&doc.semantic_snapshot().unwrap()).unwrap();
    let selected = ir::select(&all, root.as_ref(), true).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected.first().expect("fixture has item 1").text, "中文😀");
    assert_eq!(selected.first().expect("fixture has item 1").spans.len(), 1);
    let json = serde_json::to_string(&selected).unwrap();
    assert!(!json.contains("PRIVATE_SIBLING"));
    assert!(!json.contains("properties"));
    assert!(!json.contains("logical_id"));
    assert!(!json.contains("source_page"));
}
