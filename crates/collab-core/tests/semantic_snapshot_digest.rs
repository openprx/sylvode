//! Pins the exact output of every semantic reader on a fixed corpus.
//!
//! The Flow write path's CPU budget work (`decode_apply_cpu_ms_max`) rewrites how the editor
//! document is read; it must not change what is read. For every corpus document this test records
//! the result (value or typed error) of `semantic_snapshot`, the frozen `legacy_package_*` readers,
//! and `minimum_editor_model`, then compares a SHA-256 digest per document with
//! `tests/fixtures/semantic-digest.txt`, which was generated from the implementation as it stood
//! before that work.
//!
//! The corpus is every `.snapshot` fixture in this crate and in `frontend/tests/fixtures`, every
//! `.update` fixture imported onto every snapshot in its directory, and documents generated here:
//! legacy tree pages, flat model-1 pages, structured model-4 and model-5 pages (including deleted
//! placements and atoms), and deliberately malformed documents (bad containers, unknown marks,
//! invalid atoms, duplicate or unindexed placements, colliding ids).
//!
//! Regenerate the pinned file only for an intentional output change:
//! `SEMANTIC_DIGEST_WRITE=1 cargo test -p collab-core --test semantic_snapshot_digest`.
//! `SEMANTIC_DIGEST_DUMP=<path>` additionally writes the full recorded text for review.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use collab_core::{CollabEngine, LoroCollabEngine, NodeKind, Operation};
use loro::{Container, ContainerTrait, LoroDoc, LoroList, LoroMap, LoroText, LoroValue, ValueOrContainer};
use sha2::{Digest, Sha256};

const PINNED: &str = "tests/fixtures/semantic-digest.txt";

fn record<T: serde::Serialize>(out: &mut String, label: &str, result: Result<T, collab_core::CollabError>) {
    match result {
        Ok(value) => {
            let _ = writeln!(out, "{label} ok {}", serde_json::to_string(&value).unwrap());
        }
        Err(error) => {
            let _ = writeln!(out, "{label} err {error:?}");
        }
    }
}

/// Everything the readers report about one loaded document.
fn describe(engine: &LoroCollabEngine) -> String {
    let mut out = String::new();
    record(&mut out, "semantic", engine.semantic_snapshot());
    record(&mut out, "v1", engine.legacy_package_semantic_snapshot_v1());
    record(&mut out, "v2", engine.legacy_package_semantic_snapshot_v2());
    record(&mut out, "v3", engine.legacy_package_semantic_snapshot_v3());
    record(&mut out, "v4", engine.legacy_package_semantic_snapshot_v4());
    record(&mut out, "floor", engine.minimum_editor_model());
    out
}

fn describe_bytes(snapshot: &[u8]) -> String {
    match LoroCollabEngine::load(snapshot) {
        Ok(engine) => describe(&engine),
        Err(error) => format!("load err {error:?}\n"),
    }
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn fixture_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut dirs = Vec::new();
    for base in [root.join("tests/fixtures"), root.join("../../frontend/tests/fixtures")] {
        dirs.push(base.clone());
        let mut children: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        children.sort();
        dirs.extend(children);
    }
    dirs
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect();
    files.sort();
    files
}

fn display_name(path: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let relative = path
        .strip_prefix(root.join("../../frontend/tests/fixtures"))
        .map(|rest| format!("frontend/{}", rest.display()))
        .or_else(|_| {
            path.strip_prefix(root.join("tests/fixtures"))
                .map(|rest| format!("collab-core/{}", rest.display()))
        });
    relative.unwrap_or_else(|_| path.display().to_string())
}

fn fixture_corpus(corpus: &mut BTreeMap<String, String>) {
    for dir in fixture_dirs() {
        let snapshots = files_with_extension(&dir, "snapshot");
        for snapshot in &snapshots {
            corpus.insert(
                display_name(snapshot),
                describe_bytes(&std::fs::read(snapshot).unwrap()),
            );
        }
        for update in files_with_extension(&dir, "update") {
            let bytes = std::fs::read(&update).unwrap();
            for snapshot in &snapshots {
                let mut out = String::new();
                match LoroCollabEngine::load(&std::fs::read(snapshot).unwrap()) {
                    Ok(mut engine) => {
                        record(
                            &mut out,
                            "import",
                            engine.import_update(&bytes).map(|diff| diff.changed),
                        );
                        out.push_str(&describe(&engine));
                        let mut strict = LoroCollabEngine::load(&std::fs::read(snapshot).unwrap()).unwrap();
                        record(
                            &mut out,
                            "import_strict",
                            strict.import_update_causally_complete(&bytes).map(|diff| diff.changed),
                        );
                        out.push_str(&describe(&strict));
                    }
                    Err(error) => {
                        let _ = writeln!(out, "load err {error:?}");
                    }
                }
                corpus.insert(format!("{} <- {}", display_name(snapshot), display_name(&update)), out);
            }
        }
    }
}

fn create(engine: &mut LoroCollabEngine, id: &str, parent: Option<&str>, index: u32) {
    engine
        .apply_content_operation(&Operation::CreateNode {
            id: id.into(),
            parent: parent.map(Into::into),
            index,
            kind: NodeKind::Block,
        })
        .unwrap();
}

fn insert_text(engine: &mut LoroCollabEngine, id: &str, text: &str) {
    engine
        .apply_content_operation(&Operation::InsertText {
            id: id.into(),
            index: 0,
            text: text.into(),
        })
        .unwrap();
}

fn set_property(engine: &mut LoroCollabEngine, id: &str, key: &str, value: &str) {
    engine
        .apply_operation(&Operation::SetProperty {
            id: id.into(),
            key: key.into(),
            value: value.into(),
        })
        .unwrap();
}

fn legacy_tree_page() -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(101);
    engine.set_title("legacy").unwrap();
    for (index, id) in ["a", "b", "c"].iter().enumerate() {
        engine
            .apply_operation(&Operation::CreateNode {
                id: (*id).into(),
                parent: None,
                index: u32::try_from(index).unwrap(),
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_operation(&Operation::InsertText {
                id: (*id).into(),
                index: 0,
                text: format!("text {id} 😀"),
            })
            .unwrap();
    }
    set_property(&mut engine, "a", "block_type", "heading");
    set_property(&mut engine, "a", "level", "2");
    set_property(&mut engine, "b", "indent", "1");
    engine
        .apply_operation(&Operation::CreateNode {
            id: "child".into(),
            parent: Some("c".into()),
            index: 0,
            kind: NodeKind::Block,
        })
        .unwrap();
    engine
        .apply_operation(&Operation::DeleteNode { id: "b".into() })
        .unwrap();
    let navigator = "0f8fad5b-d9cb-469f-a165-70867728950e";
    engine
        .apply_operation(&Operation::CreateNode {
            id: format!("{navigator}#2").as_str().into(),
            parent: None,
            index: 0,
            kind: NodeKind::NavigatorNode,
        })
        .unwrap();
    engine.set_navigator_order(navigator, "a1").unwrap();
    engine
}

fn flat_page(seed: u64, blocks: usize) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(seed);
    engine
        .apply_operation(&Operation::CreateNode {
            id: "legacy-heading".into(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .unwrap();
    engine
        .apply_operation(&Operation::InsertText {
            id: "legacy-heading".into(),
            index: 0,
            text: "Heading é".into(),
        })
        .unwrap();
    set_property(&mut engine, "legacy-heading", "block_type", "heading");
    set_property(&mut engine, "legacy-heading", "level", "3");
    engine.initialize_page_editor().unwrap();
    for index in 0..blocks {
        let id = format!("b{index:04}");
        create(&mut engine, &id, None, u32::try_from(index % 3).unwrap());
        insert_text(&mut engine, &id, &format!("block {index} ✓"));
    }
    engine
}

fn structured_page(seed: u64, blocks: usize, model5: bool) -> LoroCollabEngine {
    let mut engine = flat_page(seed, 0);
    engine
        .initialize_page_structure_model(if model5 { 5 } else { 4 })
        .unwrap();
    for index in 0..blocks {
        let id = format!("s{index:04}");
        let parent = if index % 4 == 3 {
            Some(format!("s{:04}", index - 1))
        } else {
            None
        };
        create(&mut engine, &id, parent.as_deref(), 0);
        insert_text(&mut engine, &id, &format!("nested {index}"));
    }
    if blocks > 6 {
        engine
            .apply_content_operation(&Operation::DeleteNode { id: "s0002".into() })
            .unwrap();
        engine
            .apply_content_operation(&Operation::MoveNode {
                id: "s0005".into(),
                new_parent: Some("s0004".into()),
                index: 0,
            })
            .unwrap();
    }
    engine
        .apply_page_marks(
            "s0000",
            &[
                collab_core::page_inline::MarkEdit {
                    start: 0,
                    end: 3,
                    mark: collab_core::page_inline::InlineMark::Bold,
                    clear: false,
                    href: None,
                },
                collab_core::page_inline::MarkEdit {
                    start: 2,
                    end: 5,
                    mark: collab_core::page_inline::InlineMark::Link,
                    clear: false,
                    href: Some("https://example.test/a".into()),
                },
            ],
        )
        .unwrap();
    if model5 {
        let image = serde_json::json!({"asset_id": "00000000-0000-4000-8000-000000000001", "alt": "alt text", "caption": "cap", "indent": 0});
        let attachment = serde_json::json!({"asset_id": "00000000-0000-4000-8000-000000000002", "name": "f.pdf", "mime": "application/pdf", "caption": "", "indent": 0});
        let child = serde_json::json!({"child_page_id": "00000000-0000-4000-8000-000000000003", "indent": 0});
        engine.insert_page_atom("img", None, 1, "image", &image).unwrap();
        engine
            .insert_page_atom("att", None, 2, "attachment", &attachment)
            .unwrap();
        let removed = engine.insert_page_atom("kid", None, 3, "childPage", &child).unwrap();
        engine
            .apply_content_operation(&Operation::DeleteNode {
                id: removed.as_str().into(),
            })
            .unwrap();
    }
    engine
}

/// Opens `engine`'s snapshot as a raw Loro document for hand-made (possibly malformed) edits.
fn raw(engine: &LoroCollabEngine) -> LoroDoc {
    let doc = LoroDoc::new();
    collab_core::page_inline::configure(&doc);
    doc.import(&engine.export_snapshot().unwrap()).unwrap();
    doc.set_peer_id(4_242).unwrap();
    doc
}

fn raw_snapshot(doc: &LoroDoc) -> Vec<u8> {
    doc.commit();
    doc.export(loro::ExportMode::Snapshot).unwrap()
}

fn editor_list(doc: &LoroDoc) -> LoroList {
    match doc.get_map("doc").get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        _ => panic!("editor list"),
    }
}

fn block_at(doc: &LoroDoc, index: usize) -> LoroMap {
    match editor_list(doc).get(index) {
        Some(ValueOrContainer::Container(Container::Map(map))) => map,
        _ => panic!("editor block"),
    }
}

fn first_text(map: &LoroMap) -> LoroText {
    match map.get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => match list.get(0) {
            Some(ValueOrContainer::Container(Container::Text(text))) => text,
            _ => panic!("inline text"),
        },
        _ => panic!("inline list"),
    }
}

fn structure_tree(doc: &LoroDoc) -> loro::LoroTree {
    let tree = doc.get_tree(collab_core::editor_structure::TREE);
    tree.enable_fractional_index(0);
    tree
}

fn malformed_corpus(corpus: &mut BTreeMap<String, String>) {
    let flat = flat_page(301, 6);
    let structured = structured_page(302, 10, false);
    let model5 = structured_page(303, 10, true);
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();

    let doc = raw(&flat);
    editor_list(&doc).insert_container(1, LoroText::new()).unwrap();
    cases.push(("flat non-map block", raw_snapshot(&doc)));

    let doc = raw(&flat);
    block_at(&doc, 2).delete("nodeName").unwrap();
    cases.push(("flat missing nodeName", raw_snapshot(&doc)));

    let doc = raw(&flat);
    match block_at(&doc, 3).get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => {
            list.insert_container(0, LoroMap::new()).unwrap();
        }
        _ => panic!("inline list"),
    }
    cases.push(("flat map inside inline children", raw_snapshot(&doc)));

    let doc = raw(&flat);
    block_at(&doc, 1).delete("children").unwrap();
    cases.push(("flat block without children", raw_snapshot(&doc)));

    let doc = raw(&flat);
    doc.config_default_text_style(Some(loro::StyleConfig {
        expand: loro::ExpandType::After,
    }));
    first_text(&block_at(&doc, 2)).mark(0..2, "underline", true).unwrap();
    cases.push(("flat unknown mark", raw_snapshot(&doc)));

    let doc = raw(&flat);
    first_text(&block_at(&doc, 2))
        .mark(0..2, "link", "javascript:alert(1)")
        .unwrap();
    cases.push(("flat unsafe link", raw_snapshot(&doc)));

    let doc = raw(&flat);
    first_text(&block_at(&doc, 4)).mark(0..1, "bold", true).unwrap();
    first_text(&block_at(&doc, 4)).mark(1..3, "italic", true).unwrap();
    cases.push(("flat marks raise floor", raw_snapshot(&doc)));

    let doc = raw(&flat);
    block_at(&doc, 1).insert("nodeName", "todoItem").unwrap();
    block_at(&doc, 2).insert("nodeName", "mermaid").unwrap();
    block_at(&doc, 3).insert("nodeName", "divider").unwrap();
    cases.push(("flat other kinds", raw_snapshot(&doc)));

    let doc = raw(&flat);
    match block_at(&doc, 2).get("attributes") {
        Some(ValueOrContainer::Container(Container::Map(attrs))) => {
            attrs.insert("embed_object_id", LoroValue::Null).unwrap();
            attrs.insert("checked", true).unwrap();
        }
        _ => panic!("attributes"),
    }
    match block_at(&doc, 3).get("attributes") {
        Some(ValueOrContainer::Container(Container::Map(attrs))) => {
            attrs.insert("level", 2).unwrap();
            attrs.insert("order", 3.5).unwrap();
        }
        _ => panic!("attributes"),
    }
    block_at(&doc, 4).insert("attributes", "not a map").unwrap();
    cases.push(("flat dependency and scalar attributes", raw_snapshot(&doc)));

    let doc = raw(&flat);
    let image = block_at(&doc, 1);
    image.insert("nodeName", "image").unwrap();
    cases.push(("flat atom without valid attributes", raw_snapshot(&doc)));

    let doc = raw(&flat);
    let aliases = doc.get_map("flow_editor_aliases");
    let cid = block_at(&doc, 2).id().to_string();
    aliases.insert("second-alias", cid.as_str()).unwrap();
    aliases.insert("tombstone", "__deleted__").unwrap();
    cases.push(("flat duplicate alias", raw_snapshot(&doc)));

    // Legacy tree nodes whose logical ids equal editor block CIDs -- live, unplaced (deleted
    // placement), atoms, and non-model-1 kinds -- with and without a `content_source` property,
    // and a tree node that claims to be editor prose through its own properties.
    let structured_for_collision = structured_page(305, 10, false);
    let model5_for_collision = structured_page(306, 10, true);
    for (label, source) in [
        ("flat", &flat),
        ("structured", &structured_for_collision),
        ("model5", &model5_for_collision),
    ] {
        let doc = raw(source);
        if label == "flat" {
            block_at(&doc, 4).insert("nodeName", "todoItem").unwrap();
        }
        let tree = doc.get_tree("tree");
        tree.enable_fractional_index(0);
        for index in 0..editor_list(&doc).len() {
            let cid = block_at(&doc, index).id().to_string();
            let node = tree.create(None).unwrap();
            let meta = tree.get_meta(node).unwrap();
            meta.insert("logical_id", cid.as_str()).unwrap();
            meta.insert("kind", if index % 4 == 3 { "navigator_node" } else { "block" })
                .unwrap();
            meta.insert_container("text", LoroText::new())
                .unwrap()
                .insert(0, "tree shadow")
                .unwrap();
            match index % 3 {
                1 => meta.insert("prop:content_source", "editor_doc").unwrap(),
                2 => meta.insert("prop:content_source", "tree").unwrap(),
                _ => {}
            }
        }
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        meta.insert("logical_id", "plain-tree-node").unwrap();
        meta.insert("prop:content_source", "editor_doc").unwrap();
        corpus.insert(
            format!("malformed/{label} tree ids collide with block cids"),
            describe_bytes(&raw_snapshot(&doc)),
        );
    }

    let doc = raw(&structured);
    doc.get_map(collab_core::editor_structure::META)
        .insert("model", "rich_page_v9")
        .unwrap();
    cases.push(("structured unknown model", raw_snapshot(&doc)));

    let doc = raw(&structured);
    let tree = structure_tree(&doc);
    let cid = block_at(&doc, 1).id().to_string();
    let duplicate = tree.create(None).unwrap();
    tree.get_meta(duplicate)
        .unwrap()
        .insert("block_cid", cid.as_str())
        .unwrap();
    cases.push(("structured duplicate active placement", raw_snapshot(&doc)));

    let doc = raw(&structured);
    let tree = structure_tree(&doc);
    let cid = block_at(&doc, 1).id().to_string();
    let duplicate = tree.create(None).unwrap();
    tree.get_meta(duplicate)
        .unwrap()
        .insert("block_cid", cid.as_str())
        .unwrap();
    tree.delete(duplicate).unwrap();
    cases.push(("structured duplicate deleted placement", raw_snapshot(&doc)));

    let doc = raw(&structured);
    let list = editor_list(&doc);
    let orphan = list.insert_container(list.len(), LoroMap::new()).unwrap();
    orphan.insert("nodeName", "paragraph").unwrap();
    orphan.insert_container("children", LoroList::new()).unwrap();
    cases.push(("structured unindexed physical block", raw_snapshot(&doc)));

    let doc = raw(&structured);
    let tree = structure_tree(&doc);
    let dangling = tree.create(None).unwrap();
    tree.get_meta(dangling)
        .unwrap()
        .insert("block_cid", "cid:root-nothing:Map")
        .unwrap();
    cases.push(("structured invalid block cid", raw_snapshot(&doc)));

    // A deleted placement whose block is malformed: only some readers visit it.
    let doc = raw(&structured);
    for index in 0..editor_list(&doc).len() {
        let map = block_at(&doc, index);
        let placed = structure_tree(&doc).nodes().into_iter().any(|node| {
            structure_tree(&doc).is_node_deleted(&node).unwrap_or(false)
                && matches!(structure_tree(&doc).get_meta(node).unwrap().get("block_cid"),
                    Some(ValueOrContainer::Value(LoroValue::String(cid))) if cid.as_ref() == map.id().to_string())
        });
        if placed {
            map.delete("children").unwrap();
        }
    }
    cases.push(("structured deleted placement malformed", raw_snapshot(&doc)));

    let doc = raw(&model5);
    let tree = structure_tree(&doc);
    if let Some(root) = tree.roots().first().copied() {
        let mut parent = root;
        for _ in 0..8 {
            let node = tree.create(parent).unwrap();
            let map = editor_list(&doc)
                .insert_container(editor_list(&doc).len(), LoroMap::new())
                .unwrap();
            map.insert("nodeName", "paragraph").unwrap();
            map.insert_container("children", LoroList::new()).unwrap();
            tree.get_meta(node)
                .unwrap()
                .insert("block_cid", map.id().to_string())
                .unwrap();
            parent = node;
        }
    }
    cases.push(("model5 depth limit", raw_snapshot(&doc)));

    let doc = raw(&model5);
    doc.get_map(collab_core::editor_structure::META)
        .insert("model", "rich_page_v4")
        .unwrap();
    cases.push(("model5 atoms with v4 meta", raw_snapshot(&doc)));

    let doc = raw(&model5);
    for index in 0..editor_list(&doc).len() {
        let map = block_at(&doc, index);
        if matches!(map.get("nodeName"), Some(ValueOrContainer::Value(LoroValue::String(kind))) if kind.as_ref() == "image")
        {
            match map.get("attributes") {
                Some(ValueOrContainer::Container(Container::Map(attrs))) => {
                    attrs.insert("src", "https://example.test/x.png").unwrap();
                }
                _ => panic!("atom attributes"),
            }
        }
    }
    cases.push(("model5 atom unknown attribute", raw_snapshot(&doc)));

    for (name, bytes) in cases {
        corpus.insert(format!("malformed/{name}"), describe_bytes(&bytes));
    }
}

fn generated_corpus(corpus: &mut BTreeMap<String, String>) {
    let docs = [
        ("legacy tree page", legacy_tree_page()),
        ("empty", LoroCollabEngine::new_empty(5)),
        ("flat model1 small", flat_page(201, 5)),
        ("flat model1 400", flat_page(202, 400)),
        ("structured model4 small", structured_page(203, 8, false)),
        ("structured model4 160", structured_page(204, 160, false)),
        ("structured model5 small", structured_page(205, 8, true)),
        ("structured model5 160", structured_page(206, 160, true)),
    ];
    for (name, engine) in docs {
        corpus.insert(format!("generated/{name}"), describe(&engine));
        corpus.insert(
            format!("generated/{name} reloaded"),
            describe_bytes(&engine.export_snapshot().unwrap()),
        );
    }
}

#[test]
fn semantic_readers_match_the_pinned_digests() {
    let mut corpus = BTreeMap::new();
    fixture_corpus(&mut corpus);
    generated_corpus(&mut corpus);
    malformed_corpus(&mut corpus);

    let actual: String = corpus.iter().fold(String::new(), |mut out, (name, text)| {
        let _ = writeln!(out, "{} {name}", digest(text));
        out
    });
    if let Some(dump) = std::env::var_os("SEMANTIC_DIGEST_DUMP") {
        let full: String = corpus.iter().fold(String::new(), |mut out, (name, text)| {
            let _ = writeln!(out, "=== {name}\n{text}");
            out
        });
        std::fs::write(dump, full).unwrap();
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(PINNED);
    if std::env::var_os("SEMANTIC_DIGEST_WRITE").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let pinned = std::fs::read_to_string(&path).unwrap();
    let pinned: BTreeMap<&str, &str> = pinned
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(hash, name)| (name, hash)))
        .collect();
    let mut mismatches = Vec::new();
    for (name, text) in &corpus {
        let hash = digest(text);
        if pinned.get(name.as_str()) != Some(&hash.as_str()) {
            mismatches.push(format!("{name}:\n{text}"));
        }
    }
    for name in pinned.keys() {
        if !corpus.contains_key(*name) {
            mismatches.push(format!("{name}: missing from the corpus"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} corpus documents changed semantic output:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
