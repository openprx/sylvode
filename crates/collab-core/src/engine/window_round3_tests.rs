//! Regression tests for the third adversarial review of the incremental write window.
//!
//! Like the round-two tests, every test builds its input from raw documents and hand-encoded
//! updates, runs the window as the worker does and compares it with `next`, with a fresh load of the
//! accepted document and with the host's oracle (see `window_round2_tests`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::similar_names
)]

use std::sync::Arc;

use loro::{
    Container, ContainerID, ContainerType, LoroDoc, LoroText, LoroTree, TreeID, TreeParentId, ValueOrContainer,
};

use super::hand_encoded::{accepted, frontier_strings, raw_edit, rewrite_deps, set_node};
use super::window_fixtures::{Case, FactsFrom};
use super::window_round2_tests::{flat_page_of, grow_by, judge, next_decision};
use super::{CollabEngine, LoroCollabEngine};
use crate::facts::BaseFacts;

fn create_node(engine: &mut LoroCollabEngine, id: &str, parent: Option<&str>) {
    engine
        .apply_content_operation(&crate::operation::Operation::CreateNode {
            id: id.into(),
            parent: parent.map(Into::into),
            index: 0,
            kind: crate::operation::NodeKind::NavigatorNode,
        })
        .unwrap();
}

fn deps_of(snapshot: &[u8]) -> Vec<String> {
    let doc = LoroDoc::new();
    doc.import(snapshot).unwrap();
    frontier_strings(&doc)
}

fn json_of(doc: &LoroDoc, from: &loro::VersionVector) -> serde_json::Value {
    serde_json::to_value(
        doc.export_json_updates_without_peer_compression(from, &doc.oplog_vv())
            .unwrap(),
    )
    .unwrap()
}

/// Like `rewrite_deps`, but a change that depends on an earlier change of the same `peer` keeps
/// that dependency.
fn rewrite_first_deps(
    base: &[u8],
    author: &LoroDoc,
    from: &loro::VersionVector,
    deps: &[String],
    peer: u64,
) -> Vec<u8> {
    use serde_json::Value;
    let mut json = json_of(author, from);
    let own = format!("@{peer}");
    for change in json["changes"].as_array_mut().unwrap() {
        let kept: Vec<Value> = change["deps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d.as_str().is_some_and(|d| d.ends_with(&own)))
            .cloned()
            .collect();
        change["deps"] = if kept.is_empty() {
            Value::from(deps.iter().map(|d| Value::String(d.clone())).collect::<Vec<_>>())
        } else {
            Value::from(kept)
        };
    }
    let doc = LoroDoc::new();
    doc.import(base).unwrap();
    let before = doc.oplog_vv();
    doc.import_json_updates(serde_json::to_string(&json).unwrap()).unwrap();
    doc.export(loro::ExportMode::updates(&before)).unwrap()
}

// ---------------------------------------------------------------------------------------------
// R3-1: a path through a second tree node under the engine tree
// ---------------------------------------------------------------------------------------------

const PX: u64 = 5_252;

/// A page whose live engine node `j` has its metadata map registered under a child tree nested in
/// the metadata of another engine node `n` (round two's trick, one level deeper). Returns the
/// base, `j`, the base before the nesting, and `anchor`.
fn nested_meta_base() -> (Vec<u8>, TreeID, Vec<u8>, TreeID) {
    let mut engine = LoroCollabEngine::new_empty(41);
    create_node(&mut engine, "anchor", None);
    create_node(&mut engine, "n", Some("anchor"));
    let anchor = *engine.id_to_tree.get("anchor").unwrap();
    let n = *engine.id_to_tree.get("n").unwrap();
    let base0 = engine.export_snapshot().unwrap();
    let a = raw_edit(&base0, PX, |raw| {
        let sub = raw
            .get_tree("tree")
            .get_meta(n)
            .unwrap()
            .insert_container("sub", LoroTree::new())
            .unwrap();
        sub.enable_fractional_index(0);
        let j = sub.create(TreeParentId::Root).unwrap();
        assert_eq!(j, TreeID::new(PX, 1));
    });
    let base1 = accepted(&base0, &a);
    let author = scratch_author(&base0, anchor, false);
    let j = TreeID::new(PX, 1);
    author.set_peer_id(99).unwrap();
    let from = author.oplog_vv();
    author.get_tree("tree").mov(j, TreeParentId::Node(anchor)).unwrap();
    set_node(&author, j, "j", "collection_field");
    author.commit();
    let mov = rewrite_deps(&base1, &author, &from, &deps_of(&base1));
    let base2 = accepted(&base1, &mov).as_ref().clone();
    (base2, j, base0, anchor)
}

/// An author holding `base0` plus `j = (PX, 1)` as a node of the engine tree (under `anchor` when
/// `under`, a root otherwise), so that the library lets it write `j`'s metadata.
fn scratch_author(base0: &[u8], anchor: TreeID, under: bool) -> LoroDoc {
    let author = LoroDoc::new();
    author.import(base0).unwrap();
    author.set_peer_id(PX).unwrap();
    author.get_map("scratch").insert("x", 1).unwrap();
    author.commit();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let parent = if under {
        TreeParentId::Node(anchor)
    } else {
        TreeParentId::Root
    };
    assert_eq!(tree.create(parent).unwrap(), TreeID::new(PX, 1));
    author.commit();
    author
}

#[test]
fn r3_1_kind_change_on_a_node_whose_meta_is_nested_under_another_node_is_not_decided_incrementally() {
    let (base, j, base0, anchor) = nested_meta_base();
    // The nesting is real: the path of `j`'s metadata map runs through the other node first.
    let engine = LoroCollabEngine::load(&base).unwrap();
    let path = engine
        .doc()
        .get_path_to_container(&ContainerID::new_normal(j.id(), ContainerType::Map))
        .unwrap();
    println!("{path:?}");
    let author = scratch_author(&base0, anchor, true);
    author.set_peer_id(779).unwrap();
    let from = author.oplog_vv();
    author
        .get_tree("tree")
        .get_meta(j)
        .unwrap()
        .insert("kind", "navigator_node")
        .unwrap();
    author.commit();
    let update = rewrite_first_deps(&base, &author, &from, &deps_of(&base), 779);
    let judged = judge(&base, &update);
    println!("{judged:?}");
    judged.assert_sound("nested kind change");
    assert!(!judged.incremental, "{judged:?}");
}

#[test]
fn r3_1_text_over_the_block_limit_on_a_node_whose_meta_is_nested_is_refused() {
    let (base, j, base0, anchor) = nested_meta_base();
    let author = scratch_author(&base0, anchor, true);
    let deliver = |target: &[u8], peer: u64, edit: &dyn Fn(&LoroDoc)| {
        author.set_peer_id(peer).unwrap();
        let from = author.oplog_vv();
        edit(&author);
        author.commit();
        rewrite_first_deps(target, &author, &from, &deps_of(target), peer)
    };
    let grow = deliver(&base, 600, &|a: &LoroDoc| {
        let meta = a.get_tree("tree").get_meta(j).unwrap();
        meta.insert_container("text", LoroText::new())
            .unwrap()
            .insert(0, &"t".repeat(60_000))
            .unwrap();
    });
    let base = accepted(&base, &grow).as_ref().clone();
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_some());
    let update = deliver(&base, 779, &|a: &LoroDoc| {
        let meta = a.get_tree("tree").get_meta(j).unwrap();
        let Some(ValueOrContainer::Container(Container::Text(text))) = meta.get("text") else {
            panic!()
        };
        text.insert(0, &"u".repeat(50_000)).unwrap();
    });
    let judged = judge(&base, &update);
    println!("{judged:?}");
    assert!(
        judged.next.as_ref().is_err_and(|e| e.contains("text_block_chars")),
        "{judged:?}"
    );
    judged.assert_sound("nested text over the limit");
    assert!(!judged.incremental, "{judged:?}");
}

// ---------------------------------------------------------------------------------------------
// The corpus extension
// ---------------------------------------------------------------------------------------------

fn case(label: String, kind: &'static str, base: &[u8], update: Vec<u8>) -> Case {
    Case {
        label,
        family: "round3",
        kind,
        base: Arc::new(base.to_vec()),
        update,
        expect: None,
        facts: FactsFrom::Base,
    }
}

/// An edit of a document.
type Edit<'a> = dyn Fn(&LoroDoc) + 'a;

fn nested_meta_cases(scale: u64) -> Vec<Case> {
    let (base, j, base0, anchor) = nested_meta_base();
    let mut cases = Vec::new();
    for k in 0..scale {
        let peer = 9_500 + k;
        let edits: [(&'static str, &Edit<'_>); 3] = [
            ("nested_kind", &|a: &LoroDoc| {
                a.get_tree("tree")
                    .get_meta(j)
                    .unwrap()
                    .insert("kind", "navigator_node")
                    .unwrap();
            }),
            ("nested_property", &|a: &LoroDoc| {
                a.get_tree("tree")
                    .get_meta(j)
                    .unwrap()
                    .insert("prop:note", "x")
                    .unwrap();
            }),
            ("nested_text", &|a: &LoroDoc| {
                a.get_tree("tree")
                    .get_meta(j)
                    .unwrap()
                    .insert_container("text", LoroText::new())
                    .unwrap()
                    .insert(0, &"t".repeat(50))
                    .unwrap();
            }),
        ];
        for (offset, (kind, edit)) in (0u64..).zip(edits) {
            let author = scratch_author(&base0, anchor, true);
            author.set_peer_id(peer + offset * 100).unwrap();
            let from = author.oplog_vv();
            edit(&author);
            author.commit();
            let update = rewrite_first_deps(&base, &author, &from, &deps_of(&base), peer + offset * 100);
            cases.push(case(format!("round3/{kind}/{k}"), kind, &base, update));
        }
    }
    cases
}

// ---------------------------------------------------------------------------------------------
// R3-4: the incremental window is O(created nodes)
// ---------------------------------------------------------------------------------------------

/// A legacy page with `unnamed` tree nodes that have no logical id, 1,000 per update.
fn unnamed_page(unnamed: usize) -> Vec<u8> {
    let mut engine = LoroCollabEngine::new_empty(51);
    create_node(&mut engine, "anchor", None);
    let mut base = engine.export_snapshot().unwrap();
    let mut left = unnamed;
    let mut peer = 52_000;
    while left > 0 {
        let step = left.min(1_000);
        base = accepted(
            &base,
            &raw_edit(&base, peer, |raw| {
                let tree = raw.get_tree("tree");
                tree.enable_fractional_index(0);
                for _ in 0..step {
                    tree.create(TreeParentId::Root).unwrap();
                }
            }),
        )
        .as_ref()
        .clone();
        left -= step;
        peer += 1;
    }
    base
}

#[test]
fn r3_4_an_incremental_window_does_not_read_the_unnamed_nodes_of_the_page() {
    let mut reads = Vec::new();
    for unnamed in [500usize, 2_000, 4_000] {
        let base = unnamed_page(unnamed);
        let update = raw_edit(&base, 53_000, |raw| {
            let tree = raw.get_tree("tree");
            tree.enable_fractional_index(0);
            let n = tree.create(TreeParentId::Root).unwrap();
            set_node(raw, n, "fresh", "navigator_node");
        });
        let mut engine = LoroCollabEngine::load(&base).unwrap();
        engine.prewarm_for_writes();
        let facts = BaseFacts::compute(&engine);
        assert!(facts.is_some());
        crate::walk_counter::reset();
        let accepted = crate::window_check::run_window(&mut engine, facts.as_ref(), &update).unwrap();
        assert!(accepted.report.is_some(), "{accepted:?}");
        reads.push(crate::walk_counter::id_reads());
        judge(&base, &update).assert_sound("one create on a page of unnamed nodes");
    }
    println!("id reads for 500, 2,000 and 4,000 unnamed nodes: {reads:?}");
    assert_eq!(reads, [1, 1, 1], "the window reads the created node only");
}

// ---------------------------------------------------------------------------------------------
// R3-3: a write that rewrites the logical id of an existing node
// ---------------------------------------------------------------------------------------------

/// Two chains: `a0 .. a20` (depth 20) and `b0 .. b30` (depth 30), all navigators.
fn two_chains() -> Vec<u8> {
    let mut engine = LoroCollabEngine::new_empty(81);
    for (name, length) in [("a", 21), ("b", 31)] {
        for index in 0..length {
            create_node(
                &mut engine,
                &format!("{name}{index}"),
                (index > 0).then(|| format!("{name}{}", index - 1)).as_deref(),
            );
        }
    }
    engine.export_snapshot().unwrap()
}

fn rename_to(base: &[u8], node: &str, value: &str) -> Vec<u8> {
    let node = super::hand_encoded::tree_id_of(base, node);
    raw_edit(base, 8_100, |raw| {
        raw.get_tree("tree")
            .get_meta(node)
            .unwrap()
            .insert("logical_id", value)
            .unwrap();
    })
}

/// Setting `b30`'s id to the id of a shallow node: the worker's stale caches keep the old id and
/// accept, while the host's fresh load keeps one of the two nodes with the duplicate id and, for
/// some targets, reads `b30`'s children at the shallow node's depth (`tree_depth`).
#[test]
fn r3_3_an_existing_nodes_logical_id_is_never_rewritten_inside_the_window() {
    let base = two_chains();
    let mut over_the_limit_for_a_fresh_load = 0;
    for target in 1..=10 {
        let update = rename_to(&base, "b30", &format!("a{target}"));
        // With the switch off (no facts) the window is `next`'s: it accepts.
        let mut plain = LoroCollabEngine::load(&base).unwrap();
        plain.prewarm_for_writes();
        let next = crate::window_check::run_window(&mut plain, None, &update);
        assert!(next.is_ok(), "a{target}: {next:?}");
        over_the_limit_for_a_fresh_load += usize::from(plain.fresh_load_decision().is_err());
        // With facts the write is refused, as a plain refusal and before the host.
        let mut engine = LoroCollabEngine::load(&base).unwrap();
        engine.prewarm_for_writes();
        let facts = BaseFacts::compute(&engine);
        assert!(facts.is_some());
        let refused = crate::window_check::run_window(&mut engine, facts.as_ref(), &update);
        assert_eq!(
            refused.as_ref().err(),
            Some(&crate::error::CollabError::OperationFailed {
                reason: "logical_id_rewritten".into()
            }),
            "a{target}: {refused:?}"
        );
    }
    println!("fresh loads over the limit: {over_the_limit_for_a_fresh_load} of 10");
    assert!(
        over_the_limit_for_a_fresh_load >= 1,
        "the hole of `next` exists for at least one target"
    );
}

/// A node that the same update creates may be named, and a block map with a key of that name is
/// not a tree node's metadata.
#[test]
fn r3_3_naming_a_node_the_update_creates_is_not_a_rewrite() {
    let base = two_chains();
    let update = raw_edit(&base, 8_101, |raw| {
        let tree = raw.get_tree("tree");
        tree.enable_fractional_index(0);
        let n = tree.create(TreeParentId::Root).unwrap();
        set_node(raw, n, "brand_new", "navigator_node");
    });
    let judged = judge(&base, &update);
    judged.assert_sound("a create with a name");
    assert!(judged.incremental, "{judged:?}");
}

// ---------------------------------------------------------------------------------------------
// R3-2: the work budget charges the payload of a value
// ---------------------------------------------------------------------------------------------

fn block_attributes(raw: &LoroDoc, index: usize) -> loro::LoroMap {
    let list = super::floor_equivalence_tests::children(raw);
    let Some(ValueOrContainer::Container(Container::Map(block))) = list.get(index) else {
        panic!()
    };
    match block.get("attributes") {
        Some(ValueOrContainer::Container(Container::Map(attributes))) => attributes,
        _ => block.insert_container("attributes", loro::LoroMap::new()).unwrap(),
    }
}

/// Each write puts one value of about 64 KB into the `attributes` of some block: a list of 20,000
/// small integers. Without a charge for the payload each write adds two items and the page grows
/// without a work bound (each write is decided incrementally and the payload is re-read by every
/// full projection and by the host).
#[test]
fn r3_2_a_list_value_in_block_attributes_cannot_grow_a_flat_page_without_a_bound() {
    let (incremental, first_fallback, base) = grow_by(flat_page_of(400), 40, &|raw, write| {
        let values: Vec<i64> = (0..20_000).map(|i| i % 7).collect();
        block_attributes(raw, usize::try_from(write).unwrap() % 400)
            .insert(&format!("p{write}"), values)
            .unwrap();
    });
    println!("list values: {incremental} incremental writes, then {first_fallback:?}");
    assert!(incremental >= 1);
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 12, "{incremental} writes of 160 KB of payload each");
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none());
}

/// The same with a 60,000-character string per write.
#[test]
fn r3_2_a_string_value_in_block_attributes_cannot_grow_a_flat_page_without_a_bound() {
    let (incremental, first_fallback, base) = grow_by(flat_page_of(400), 60, &|raw, write| {
        block_attributes(raw, usize::try_from(write).unwrap() % 400)
            .insert(&format!("p{write}"), "s".repeat(60_000))
            .unwrap();
    });
    println!("string values: {incremental} incremental writes, then {first_fallback:?}");
    assert!(incremental >= 1);
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 30, "{incremental} writes of 60 KB each");
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none());
}

/// A nested value counts all of its payload: maps of lists of strings.
#[test]
fn r3_2_a_nested_value_in_block_attributes_is_charged_by_its_payload() {
    let (incremental, first_fallback, _) = grow_by(flat_page_of(400), 60, &|raw, write| {
        let mut entries = std::collections::HashMap::new();
        for key in 0..40 {
            let strings: Vec<String> = (0..10).map(|_| "n".repeat(150)).collect();
            entries.insert(format!("k{key}"), loro::LoroValue::from(strings));
        }
        block_attributes(raw, usize::try_from(write).unwrap() % 400)
            .insert(&format!("p{write}"), loro::LoroValue::from(entries))
            .unwrap();
    });
    println!("nested values: {incremental} incremental writes, then {first_fallback:?}");
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 30, "{incremental} writes of 60 KB each");
}

/// `prop:*` strings on a legacy node, which the projection clones for every node.
#[test]
fn r3_2_property_strings_on_a_legacy_node_cannot_grow_a_page_without_a_bound() {
    let mut engine = LoroCollabEngine::new_empty(63);
    create_node(&mut engine, "anchor", None);
    let anchor = *engine.id_to_tree.get("anchor").unwrap();
    let (incremental, first_fallback, _) = grow_by(engine.export_snapshot().unwrap(), 60, &|raw, write| {
        let meta = raw.get_tree("tree").get_meta(anchor).unwrap();
        meta.insert(&format!("prop:big{write}"), "s".repeat(60_000)).unwrap();
    });
    println!("property strings: {incremental} incremental writes, then {first_fallback:?}");
    assert!(incremental >= 1);
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 30, "{incremental} writes of 60 KB each");
}

/// The items an update adds to a page never exceed the bound the window keeps (the sum of
/// `work::op_work` over its ops): on every shape of this round and the earlier growth shapes.
#[test]
fn r3_2_the_bound_on_the_items_an_update_adds_holds() {
    let page = flat_page_of(400);
    let shapes: Vec<(&str, Box<Edit>)> = vec![
        (
            "list",
            Box::new(|raw: &LoroDoc| {
                let values: Vec<i64> = (0..5_000).collect();
                block_attributes(raw, 3).insert("p", values).unwrap();
            }),
        ),
        (
            "string",
            Box::new(|raw: &LoroDoc| {
                block_attributes(raw, 4).insert("p", "s".repeat(10_000)).unwrap();
            }),
        ),
        (
            "many small keys",
            Box::new(|raw: &LoroDoc| {
                let attributes = block_attributes(raw, 5);
                for i in 0..3_000 {
                    attributes.insert(&format!("k{i}"), i64::from(i)).unwrap();
                }
            }),
        ),
        (
            "overwrite",
            Box::new(|raw: &LoroDoc| {
                let attributes = block_attributes(raw, 6);
                for i in 0..200 {
                    attributes.insert("same", "x".repeat(100 + i)).unwrap();
                }
            }),
        ),
        (
            "text and blocks",
            Box::new(|raw: &LoroDoc| {
                let list = super::floor_equivalence_tests::children(raw);
                for i in 0..30 {
                    let block = list.insert_container(list.len(), loro::LoroMap::new()).unwrap();
                    block.insert("nodeName", "paragraph").unwrap();
                    block
                        .insert_container("children", loro::LoroList::new())
                        .unwrap()
                        .insert_container(0, LoroText::new())
                        .unwrap()
                        .insert(0, &format!("block {i} {}", "w".repeat(600)))
                        .unwrap();
                }
            }),
        ),
    ];
    for (label, edit) in shapes {
        let update = raw_edit(&page, 61_000, |raw| edit(raw));
        let mut engine = LoroCollabEngine::load(&page).unwrap();
        let before = engine.projection_items(u64::MAX).unwrap().unwrap();
        let trace = engine.import_client_update_traced(&update, None).unwrap();
        let after = engine.projection_items(u64::MAX).unwrap().unwrap();
        let bound = trace.summary.work_added();
        println!("{label}: items {before} -> {after}, bound {bound}");
        assert!(after <= before + bound, "{label}: {after} > {before} + {bound}");
    }
}

// ---------------------------------------------------------------------------------------------
// R3-8: a create under a parent the document never had
// ---------------------------------------------------------------------------------------------

/// A single-head base and one change, depending exactly on that head, that creates a node under a
/// parent id the base never had (hand-encoded: the library's API refuses to author it). Records
/// what the library does with it, for a change of a new peer and for one that continues the head's
/// own peer (the shape the library applies without a parent check), and that the window and `next`
/// do the same.
#[test]
fn r3_8_a_create_under_a_missing_parent_is_decided_as_next_does() {
    for (label, peer) in [("new peer", 9_100), ("the head's own peer", 91)] {
        let mut engine = LoroCollabEngine::new_empty(91);
        create_node(&mut engine, "anchor", None);
        let base = engine.export_snapshot().unwrap();
        let anchor = *engine.id_to_tree.get("anchor").unwrap();
        let author = LoroDoc::new();
        author.import(&base).unwrap();
        author.set_peer_id(peer).unwrap();
        let from = author.oplog_vv();
        let tree = author.get_tree("tree");
        tree.enable_fractional_index(0);
        let n = tree.create(TreeParentId::Node(anchor)).unwrap();
        set_node(&author, n, "orphan", "navigator_node");
        author.commit();
        let mut json = json_of(&author, &from);
        let missing = format!("{}@{}", 5, 9_999);
        let mut rewritten = 0;
        for change in json["changes"].as_array_mut().unwrap() {
            for op in change["ops"].as_array_mut().unwrap() {
                if op["content"]["type"] == "create" {
                    op["content"]["parent"] = serde_json::Value::String(missing.clone());
                    rewritten += 1;
                }
            }
        }
        assert_eq!(rewritten, 1);
        let doc = LoroDoc::new();
        doc.import(&base).unwrap();
        let before = doc.oplog_vv();
        doc.import_json_updates(serde_json::to_string(&json).unwrap()).unwrap();
        let update = doc.export(loro::ExportMode::updates(&before)).unwrap();

        // What the library does: does the import panic, and is the node in the state?
        let library = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut probe = LoroCollabEngine::load(&base).unwrap();
            probe.prewarm_for_writes();
            probe.import_update(&update).unwrap();
            probe.doc().get_tree("tree").nodes().len()
        }))
        .map_err(|_| "panic");
        let outcome = |facts: bool| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut engine = LoroCollabEngine::load(&base).unwrap();
                engine.prewarm_for_writes();
                let facts = if facts { BaseFacts::compute(&engine) } else { None };
                crate::walk_counter::reset();
                crate::window_check::run_window(&mut engine, facts.as_ref(), &update)
                    .map(|accepted| (accepted.report.is_some(), crate::walk_counter::fallback_reasons()))
                    .map_err(|e| e.to_string())
            }))
            .map_err(|_| "panic")
        };
        let next = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| next_decision(&base, &update)))
            .map_err(|_| "panic");
        let with_facts = outcome(true);
        let without = outcome(false);
        println!(
            "{label}: library {library:?} | next {next:?} | window with facts {with_facts:?} | without {without:?}"
        );
        // All three agree: they all panic or they all decide the same way, since the behaviour is
        // the library's.
        assert_eq!(
            next.is_ok(),
            with_facts.is_ok(),
            "{label}: next {next:?} vs window {with_facts:?}"
        );
        assert_eq!(next.is_ok(), without.is_ok(), "{label}");
        if let (Ok(next), Ok(with_facts)) = (&next, &with_facts) {
            assert_eq!(next.is_ok(), with_facts.is_ok(), "{label}");
        }
    }
}

/// The rewrite of an existing node's logical id to the id of a shallow node of another chain: `next`
/// accepts it on stale caches, the host's fresh load refuses it for some targets. The worker with
/// facts refuses it; the switch-off test records the host's refusal for the rest.
fn id_rewrite_cases(scale: u64) -> Vec<Case> {
    let base = two_chains();
    let mut cases = Vec::new();
    for k in 0..scale {
        for target in 1..=10u64 {
            let update = raw_edit(&base, 9_800 + k * 20 + target, |raw| {
                let node = super::hand_encoded::tree_id_of(&base, "b30");
                raw.get_tree("tree")
                    .get_meta(node)
                    .unwrap()
                    .insert("logical_id", format!("a{target}").as_str())
                    .unwrap();
            });
            cases.push(case(
                format!("round3/id_rewrite/{k}/a{target}"),
                "id_rewrite_to_shallow",
                &base,
                update,
            ));
        }
    }
    cases
}

pub(super) fn round3_cases(scale: u64) -> Vec<Case> {
    let mut cases = nested_meta_cases(scale);
    cases.extend(id_rewrite_cases(scale));
    cases
}
