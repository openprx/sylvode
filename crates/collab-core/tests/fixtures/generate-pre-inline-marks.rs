use collab_core_legacy::{CollabEngine, LoroCollabEngine, Operation, NodeId, NodeKind};
use loro::{LoroDoc, LoroList, LoroMap, LoroText, ExportMode};
fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&out).unwrap();
    let mut engine = LoroCollabEngine::new_empty(701);
    engine.set_title("Legacy editor package").unwrap();
    engine.apply_operation(&Operation::CreateNode {id:NodeId::from("api-block"),parent:None,index:0,kind:NodeKind::Block}).unwrap();
    engine.apply_operation(&Operation::InsertText {id:NodeId::from("api-block"),index:0,text:"Legacy API text".to_string()}).unwrap();
    let doc=LoroDoc::new();doc.import(&engine.export_snapshot().unwrap()).unwrap();doc.set_peer_id(702).unwrap();
    // d523d7c's loro-prosemirror paragraph shape, kept alongside the API tree container.
    let root=doc.get_map("doc");root.insert("nodeName","doc").unwrap();
    let children=root.insert_container("children",LoroList::new()).unwrap();
    let block=children.insert_container(0,LoroMap::new()).unwrap();block.insert("nodeName","paragraph").unwrap();
    let attrs=block.insert_container("attributes",LoroMap::new()).unwrap();attrs.insert("indent",0_i64).unwrap();
    let children=block.insert_container("children",LoroList::new()).unwrap();
    let text=children.insert_container(0,LoroText::new()).unwrap();text.insert(0,"Legacy browser text stays in the snapshot").unwrap();doc.commit();
    let bytes=doc.export(ExportMode::Snapshot).unwrap();
    let restored=LoroCollabEngine::load(&bytes).unwrap();let semantic=restored.semantic_snapshot().unwrap();
    std::fs::write(out.join("pre-inline-marks-d523d7c.snapshot"),bytes).unwrap();
    std::fs::write(out.join("pre-inline-marks-d523d7c.semantic.json"),semantic.canonical_json().unwrap()).unwrap();
    std::fs::write(out.join("pre-inline-marks-d523d7c.sha256"),format!("{}\n",semantic.semantic_hash())).unwrap();
    println!("legacy semantic hash {}",semantic.semantic_hash());
}
