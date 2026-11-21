//! Regression tests for the second adversarial review of the incremental write window, and the
//! generators that extend the harness's corpus with the shapes that review found.
//!
//! Every test builds its input from raw documents and hand-encoded updates, runs the window as the
//! worker does, and compares the result with three references that share nothing with the code
//! under test:
//!
//! * `next`: the full computation with `next`'s own import and full id-cache rebuild
//!   (`next_oracle`);
//! * a fresh load of the document the window accepted, projected and checked, which is what the host
//!   does with the worker's result;
//! * the host's oracle on the report, when the window decided incrementally.
//!
//! The module uses only interfaces that existed at the reviewed revision (`run_window`,
//! `BaseFacts::compute`, `compare_with_projection`, the engine), and compares paths by name, so the
//! regression tests can be run against that revision: those that assert what the review found fail
//! there.
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
    Container, ContainerID, ContainerType, ExportMode, LoroDoc, LoroMap, LoroText, TreeID, TreeParentId,
    ValueOrContainer,
};

use super::hand_encoded::{accepted, frontier_strings, raw_edit, rewrite_deps, set_node};
use super::window_fixtures::{Case, FactsFrom};
use super::{CollabEngine, LoroCollabEngine};
use crate::editor_doc_bridge::{EditorInitialBlock, initialize_editor_doc};
use crate::facts::BaseFacts;
use crate::window_check::{compare_with_projection, run_window};

// ---------------------------------------------------------------------------------------------
// The three references
// ---------------------------------------------------------------------------------------------

/// What `next` decides: its import with the full id-cache rebuild, then projection and limits.
pub(super) fn next_decision(base: &[u8], update: &[u8]) -> Result<(), String> {
    let mut engine = LoroCollabEngine::load(base).unwrap();
    engine.next_window(update).map_err(|e| e.to_string())
}

/// What the worker's window and the references made of one update.
#[derive(Debug)]
pub(super) struct Judged {
    /// `next`.
    pub next: Result<(), String>,
    /// What an independent read of the update says `tree_refs` must refuse it with, if anything.
    pub tree_reference: Option<&'static str>,
    /// The window with facts: `Ok(path)` (`None` when the check decided, the fallback reason when the
    /// full computation did) or the refusal.
    pub window: Result<Option<crate::window_check::FallbackReason>, String>,
    /// Whether the check decided incrementally (a report).
    pub incremental: bool,
    /// The reason of the fallback, when the window fell back, whether the full computation then
    /// accepted or refused.
    pub reason: Option<String>,
    /// The host's oracle on the report, when there is one.
    pub oracle: Option<String>,
    /// A fresh load of the accepted document, projected and checked.
    pub fresh: Option<Result<(), String>>,
    /// The window without facts (the full path of the switch off): the refusal, if any.
    pub full_window: Result<(), String>,
}

pub(super) fn judge(base: &[u8], update: &[u8]) -> Judged {
    let next = next_decision(base, update);
    let mut engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    let facts = BaseFacts::compute(&engine);
    crate::walk_counter::reset();
    let outcome = run_window(&mut engine, facts.as_ref(), update);
    let reason = crate::walk_counter::fallback_reasons().last().cloned();
    let (window, incremental, oracle, fresh) = match outcome {
        Ok(accepted) => {
            let oracle = accepted.report.as_ref().map(|report| {
                let host = LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
                match host.semantic_snapshot() {
                    Ok(projection) => format!("{:?}", compare_with_projection(report, &projection)),
                    Err(e) => format!("host projection error {e:?}"),
                }
            });
            let fresh = engine.fresh_load_decision().map_err(|e| e.to_string());
            (Ok(accepted.fallback), accepted.report.is_some(), oracle, Some(fresh))
        }
        Err(e) => (Err(e.to_string()), false, None, None),
    };
    let mut plain = LoroCollabEngine::load(base).unwrap();
    plain.prewarm_for_writes();
    let full_window = run_window(&mut plain, None, update)
        .map(|_| ())
        .map_err(|e| e.to_string());
    Judged {
        next,
        tree_reference: super::window_differential_tests::unresolved_tree_reference(base, update),
        window,
        incremental,
        reason,
        oracle,
        fresh,
        full_window,
    }
}

impl Judged {
    /// The window decided what `next` decides, the document it accepted is within every limit for a
    /// fresh load, and the report of an incremental decision holds; the switched-off window is
    /// `next`'s as well.
    pub(super) fn assert_sound(&self, label: &str) {
        // A tree op naming a parent that does not exist when it runs: `next` accepts, the window
        // refuses with or without facts, and the update explains the refusal.
        if let Some(reason) = self.tree_reference {
            let refusal = Err(format!("operation failed: {reason}"));
            assert_eq!(self.window.clone().map(|_| ()), refusal, "{label}: {self:?}");
            assert_eq!(self.full_window, refusal, "{label}: {self:?}");
            return;
        }
        assert_eq!(self.next.is_ok(), self.window.is_ok(), "{label}: {self:?}");
        if let (Err(next), Err(window)) = (&self.next, &self.window) {
            assert_eq!(next, window, "{label}: the refusals differ");
        }
        assert_eq!(
            self.next, self.full_window,
            "{label}: the switched-off window differs from next"
        );
        if let Some(fresh) = &self.fresh {
            assert!(
                fresh.is_ok(),
                "{label}: the accepted document is refused by a fresh load: {self:?}"
            );
        }
        if let Some(oracle) = &self.oracle {
            assert_eq!(oracle, "Ok(())", "{label}: the oracle disagrees: {self:?}");
        }
    }

    /// The name of the fallback reason, when the window fell back.
    pub(super) fn fallback(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

fn meta_map(doc: &LoroDoc, node: TreeID) -> LoroMap {
    match doc.get_container(ContainerID::new_normal(node.id(), ContainerType::Map)) {
        Some(Container::Map(map)) => map,
        other => panic!("no meta map for {node:?}: {other:?}"),
    }
}

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

fn json_of(doc: &LoroDoc, from: &loro::VersionVector) -> serde_json::Value {
    serde_json::to_value(
        doc.export_json_updates_without_peer_compression(from, &doc.oplog_vv())
            .unwrap(),
    )
    .unwrap()
}

fn import_json(base: &[u8], json: &serde_json::Value) -> Vec<u8> {
    let doc = LoroDoc::new();
    doc.import(base).unwrap();
    let before = doc.oplog_vv();
    doc.import_json_updates(serde_json::to_string(json).unwrap()).unwrap();
    doc.export(ExportMode::updates(&before)).unwrap()
}

/// Like `rewrite_deps`, but a change that depends on an earlier change of the same `peer` keeps
/// that dependency (a long text insert is split into several changes).
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
    import_json(base, &json)
}

fn deps_of(snapshot: &[u8]) -> Vec<String> {
    let doc = LoroDoc::new();
    doc.import(snapshot).unwrap();
    frontier_strings(&doc)
}

// ---------------------------------------------------------------------------------------------
// R2-1: the metadata of a deleted tree node has no path
// ---------------------------------------------------------------------------------------------

/// A mixed page (editor document plus legacy tree). Tree: d -> a1 .. a20 -> b1 .. b20, all
/// navigators, d deleted, a20 marked `content_source = editor_doc` so the projection drops it and
/// b1 .. b20 count their depth from 1: deepest 20. Returns the snapshot before `d` was deleted, the
/// snapshot after, and a20.
fn deleted_chain_base(chain: usize) -> (Vec<u8>, Vec<u8>, TreeID) {
    let engine = LoroCollabEngine::new_empty(31);
    initialize_editor_doc(
        &engine.doc,
        &[EditorInitialBlock {
            id: "p0".into(),
            kind: "paragraph".into(),
            text: "hello".into(),
            indent: 0,
            level: None,
        }],
    )
    .unwrap();
    let tree = engine.doc.get_tree("tree");
    tree.enable_fractional_index(0);
    let d = tree.create(TreeParentId::Root).unwrap();
    set_node(&engine.doc, d, "d", "navigator_node");
    let mut parent = d;
    let mut dropped = d;
    for i in 1..=chain {
        let n = tree.create(TreeParentId::Node(parent)).unwrap();
        set_node(&engine.doc, n, &format!("a{i}"), "navigator_node");
        parent = n;
        dropped = n;
    }
    tree.get_meta(dropped)
        .unwrap()
        .insert("prop:content_source", "editor_doc")
        .unwrap();
    for i in 1..=chain {
        let n = tree.create(TreeParentId::Node(parent)).unwrap();
        set_node(&engine.doc, n, &format!("b{i}"), "navigator_node");
        parent = n;
    }
    engine.doc.commit();
    let before_delete = engine.export_snapshot().unwrap();
    tree.delete(d).unwrap();
    engine.doc.commit();
    (before_delete, engine.export_snapshot().unwrap(), dropped)
}

/// The offline client of the review: it edits on the version before `d` was deleted, so the update
/// is concurrent with the base.
#[test]
fn r2_1_offline_edit_of_a_deleted_nodes_metadata_is_decided_as_next_does() {
    let (before_delete, base, a20) = deleted_chain_base(20);
    let update = raw_edit(&before_delete, 777, |raw| {
        meta_map(raw, a20).delete("prop:content_source").unwrap();
    });
    let judged = judge(&base, &update);
    println!("{judged:?}");
    assert!(
        judged.next.as_ref().is_err_and(|e| e.contains("tree_depth")),
        "next refuses: {judged:?}"
    );
    judged.assert_sound("offline edit");
    assert!(!judged.incremental, "{judged:?}");
}

/// The same edit delivered as a strict extension of the base (hand-encoded: the library refuses to
/// author it on a document where the node is deleted): no concurrency, so only the rule for a
/// container without a path stops the incremental check.
#[test]
fn r2_1_sequential_edit_of_a_deleted_nodes_metadata_is_not_decided_incrementally() {
    let (before_delete, base, a20) = deleted_chain_base(20);
    let author = LoroDoc::new();
    author.import(&before_delete).unwrap();
    author.set_peer_id(778).unwrap();
    let from = author.oplog_vv();
    meta_map(&author, a20).delete("prop:content_source").unwrap();
    author.commit();
    let update = rewrite_deps(&base, &author, &from, &deps_of(&base));
    let judged = judge(&base, &update);
    println!("{judged:?}");
    assert!(
        judged.next.as_ref().is_err_and(|e| e.contains("tree_depth")),
        "next refuses: {judged:?}"
    );
    judged.assert_sound("sequential edit");
    assert_eq!(judged.fallback(), Some("Unclassified"), "{judged:?}");
}

// ---------------------------------------------------------------------------------------------
// R2-2: a live node whose metadata map is registered under another tree
// ---------------------------------------------------------------------------------------------

const PX: u64 = 5_151;

/// Base: a navigator `anchor`; a node J = (PX, 0) created and deleted in a root tree `aux`; J moved
/// into the engine tree by a hand-encoded move (its create op is the `aux` one) and named `n`, kind
/// `collection_field` (counted by no total).
fn foreign_meta_base() -> (Vec<u8>, TreeID, Vec<u8>, TreeID) {
    let mut engine = LoroCollabEngine::new_empty(41);
    create_node(&mut engine, "anchor", None);
    let anchor = *engine.id_to_tree.get("anchor").unwrap();
    let base0 = engine.export_snapshot().unwrap();
    let aux_ops = raw_edit(&base0, PX, |raw| {
        let aux = raw.get_tree("aux");
        aux.enable_fractional_index(0);
        let j = aux.create(TreeParentId::Root).unwrap();
        assert_eq!(j, TreeID::new(PX, 0));
        aux.delete(j).unwrap();
    });
    let base1 = accepted(&base0, &aux_ops);
    let author = LoroDoc::new();
    author.import(&base0).unwrap();
    author.set_peer_id(PX).unwrap();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let j = tree.create(TreeParentId::Root).unwrap();
    assert_eq!(j, TreeID::new(PX, 0));
    author.commit();
    author.set_peer_id(99).unwrap();
    let from = author.oplog_vv();
    tree.mov(j, TreeParentId::Node(anchor)).unwrap();
    set_node(&author, j, "n", "collection_field");
    author.commit();
    let mov = rewrite_deps(&base1, &author, &from, &deps_of(&base1));
    let base = accepted(&base1, &mov).as_ref().clone();
    (base, j, base0, anchor)
}

/// An edit of J's metadata, authored where J is a plain engine-tree node (so the library lets a
/// local transaction write its metadata) and delivered against `target` with `target`'s frontier as
/// its only dependency.
fn foreign_edit(
    base0: &[u8],
    target: &[u8],
    anchor: TreeID,
    peer: u64,
    edit: impl FnOnce(&LoroDoc, TreeID),
) -> Vec<u8> {
    let author = LoroDoc::new();
    author.import(base0).unwrap();
    author.set_peer_id(PX).unwrap();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let j = tree.create(TreeParentId::Node(anchor)).unwrap();
    author.commit();
    author.set_peer_id(peer).unwrap();
    let from = author.oplog_vv();
    edit(&author, j);
    author.commit();
    rewrite_first_deps(target, &author, &from, &deps_of(target), peer)
}

#[test]
fn r2_2a_kind_change_on_a_live_node_with_a_foreign_meta_parent_is_decided_as_next_does() {
    let (base, j, base0, anchor) = foreign_meta_base();
    let update = foreign_edit(&base0, &base, anchor, 778, |author, j2| {
        assert_eq!(j2, j);
        author
            .get_tree("tree")
            .get_meta(j2)
            .unwrap()
            .insert("kind", "navigator_node")
            .unwrap();
    });
    let judged = judge(&base, &update);
    println!("{judged:?}");
    judged.assert_sound("kind change");
    assert!(!judged.incremental, "{judged:?}");
}

/// J's text goes from 60,000 to 110,000 characters: over `text_block_chars`. One scratch author
/// keeps J as a plain engine-tree node, so the library lets it write J's metadata; each of its
/// changes is delivered against the server's base with the base's frontier as its dependency (the
/// first change of each peer) and the same peer's earlier change otherwise.
#[test]
fn r2_2b_text_over_the_block_limit_on_a_live_node_with_a_foreign_meta_parent_is_refused() {
    let (base, j, base0, anchor) = foreign_meta_base();
    let author = LoroDoc::new();
    author.import(&base0).unwrap();
    author.set_peer_id(PX).unwrap();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    assert_eq!(tree.create(TreeParentId::Node(anchor)).unwrap(), j);
    author.commit();
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
    judged.assert_sound("text over the limit");
    assert!(!judged.incremental, "{judged:?}");
}

// ---------------------------------------------------------------------------------------------
// R2-4: nodes a concurrent import materialises are never named
// ---------------------------------------------------------------------------------------------

/// Peer 6000 creates C under `Y = (6001, 0)` and a chain of `hidden` named navigators under C;
/// peer 6001 creates Y under `anchor`; both depend on the base alone. The library applies C's
/// create before Y exists (equal lamports, peer order) and drops it from the state. Returns the
/// base before and after, and C.
fn orphan_base(hidden: usize) -> (Vec<u8>, Vec<u8>, TreeID) {
    const PB: u64 = 6_001;
    const PA: u64 = 6_000;
    let mut engine = LoroCollabEngine::new_empty(71);
    create_node(&mut engine, "anchor", None);
    let anchor = *engine.id_to_tree.get("anchor").unwrap();
    let base0 = engine.export_snapshot().unwrap();
    let author = LoroDoc::new();
    author.import(&base0).unwrap();
    let base_deps = frontier_strings(&author);
    let from = author.oplog_vv();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    author.set_peer_id(PB).unwrap();
    let y = tree.create(TreeParentId::Node(anchor)).unwrap();
    set_node(&author, y, "y", "navigator_node");
    author.commit();
    author.set_peer_id(PA).unwrap();
    let c = tree.create(TreeParentId::Node(y)).unwrap();
    set_node(&author, c, "c", "navigator_node");
    let mut parent = c;
    for i in 0..hidden {
        let n = tree.create(TreeParentId::Node(parent)).unwrap();
        set_node(&author, n, &format!("h{i}"), "navigator_node");
        parent = n;
    }
    author.commit();
    let mut json = json_of(&author, &from);
    let own = format!("@{PA}");
    for change in json["changes"].as_array_mut().unwrap() {
        let id = change["id"].as_str().unwrap().to_owned();
        if id.ends_with(&format!("@{PB}")) {
            change["deps"] = serde_json::Value::from(base_deps.clone());
        } else {
            let kept: Vec<serde_json::Value> = change["deps"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d.as_str().is_some_and(|d| d.ends_with(&own)))
                .cloned()
                .collect();
            change["deps"] = if kept.is_empty() {
                serde_json::Value::from(base_deps.clone())
            } else {
                serde_json::Value::from(kept)
            };
        }
    }
    let u1 = import_json(&base0, &json);
    let judged = judge(&base0, &u1);
    assert!(
        judged.next.is_ok(),
        "the building update is accepted by next: {judged:?}"
    );
    judged.assert_sound("orphan base: building update");
    let base1 = accepted(&base0, &u1).as_ref().clone();
    (base0, base1, c)
}

/// A plain create of `z` from a client that has seen only `stale` (the version before U1).
fn stale_create(base1: &[u8], stale: &[u8], peer: u64) -> Vec<u8> {
    let author = LoroDoc::new();
    author.import(base1).unwrap();
    author.set_peer_id(peer).unwrap();
    let from = author.oplog_vv();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let anchor = *LoroCollabEngine::load(base1).unwrap().id_to_tree.get("anchor").unwrap();
    let z = tree.create(TreeParentId::Node(anchor)).unwrap();
    set_node(&author, z, "z", "navigator_node");
    author.commit();
    let mut json = json_of(&author, &from);
    let stale_deps = deps_of(stale);
    for change in json["changes"].as_array_mut().unwrap() {
        change["deps"] = serde_json::Value::from(stale_deps.clone());
    }
    import_json(base1, &json)
}

#[test]
fn r2_4_a_dropped_node_materialised_by_a_concurrent_import_is_decided_as_next_does() {
    for hidden in [3, 40] {
        let (base0, base1, c) = orphan_base(hidden);
        let loaded = LoroCollabEngine::load(&base1).unwrap();
        assert!(!loaded.tree.contains(c), "C is dropped from the state of the base");
        let update = stale_create(&base1, &base0, 6_003);
        let judged = judge(&base1, &update);
        println!("hidden {hidden}: {judged:?}");
        if hidden == 40 {
            assert!(
                judged.next.as_ref().is_err_and(|e| e.contains("tree_depth")),
                "next refuses on depth: {judged:?}"
            );
        }
        judged.assert_sound(&format!("stale create, hidden {hidden}"));
        assert!(!judged.incremental, "{judged:?}");
        assert_eq!(judged.fallback(), Some("NotSequential"), "{judged:?}");
        // With the switch off the worker is `next` on this input, caches included.
        let mut worker = LoroCollabEngine::load(&base1).unwrap();
        let outcome = run_window(&mut worker, None, &update)
            .map(|_| ())
            .map_err(|e| e.to_string());
        assert_eq!(outcome, judged.next);
        let mut next = LoroCollabEngine::load(&base1).unwrap();
        let _ = next.next_window(&update);
        assert_eq!(worker.id_to_tree, next.id_to_tree);
        assert_eq!(worker.tree_to_id, next.tree_to_id);
    }
}

/// The base move of `m` (with a chain of 5 below it) under Y ran before Y existed and was dropped;
/// a stale client's plain create replays Y's create and, with it, the move.
#[test]
fn r2_4b_a_dropped_move_replayed_by_a_concurrent_import_is_decided_as_next_does() {
    const PB: u64 = 6_101;
    const PA: u64 = 6_100;
    let mut engine = LoroCollabEngine::new_empty(81);
    create_node(&mut engine, "a0", None);
    for i in 1..30 {
        create_node(&mut engine, &format!("a{i}"), Some(&format!("a{}", i - 1)));
    }
    create_node(&mut engine, "m", None);
    for i in 1..=5 {
        let parent = if i == 1 { "m".to_owned() } else { format!("m{}", i - 1) };
        create_node(&mut engine, &format!("m{i}"), Some(&parent));
    }
    let base0 = engine.export_snapshot().unwrap();
    let a29 = *engine.id_to_tree.get("a29").unwrap();
    let m = *engine.id_to_tree.get("m").unwrap();
    let author = LoroDoc::new();
    author.import(&base0).unwrap();
    let base_deps = frontier_strings(&author);
    let from = author.oplog_vv();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    author.set_peer_id(PB).unwrap();
    let y = tree.create(TreeParentId::Node(a29)).unwrap();
    set_node(&author, y, "y", "navigator_node");
    author.commit();
    author.set_peer_id(PA).unwrap();
    tree.mov(m, TreeParentId::Node(y)).unwrap();
    author.commit();
    let mut json = json_of(&author, &from);
    for change in json["changes"].as_array_mut().unwrap() {
        change["deps"] = serde_json::Value::from(base_deps.clone());
    }
    let u1 = import_json(&base0, &json);
    let base1 = accepted(&base0, &u1).as_ref().clone();
    let author = LoroDoc::new();
    author.import(&base1).unwrap();
    author.set_peer_id(6_102).unwrap();
    let from = author.oplog_vv();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let z = tree.create(TreeParentId::Root).unwrap();
    set_node(&author, z, "z", "navigator_node");
    author.commit();
    let mut json = json_of(&author, &from);
    for change in json["changes"].as_array_mut().unwrap() {
        change["deps"] = serde_json::Value::from(base_deps.clone());
    }
    let u2 = import_json(&base1, &json);
    let judged = judge(&base1, &u2);
    println!("{judged:?}");
    judged.assert_sound("dropped move replayed");
    assert!(!judged.incremental, "{judged:?}");
}

/// The full computation after a fallback reads the id caches, so a window that rebuilt them from
/// the update's ops (a sequential import) completes them first: the caches are then what `next`'s
/// full rebuild gives, even where the incremental rebuild could not know a node (simulated here by
/// a node the caches lost).
#[test]
fn a_fallback_after_an_incremental_cache_rebuild_completes_the_caches() {
    let (_, base1, _) = orphan_base(3);
    let anchor = *LoroCollabEngine::load(&base1)
        .unwrap()
        .id_to_tree
        .get("anchor")
        .unwrap();
    let update = raw_edit(&base1, 6_200, |raw| {
        let tree = raw.get_tree("tree");
        tree.enable_fractional_index(0);
        let n = tree.create(TreeParentId::Node(anchor)).unwrap();
        set_node(raw, n, "w", "navigator_node");
    });
    let mut engine = LoroCollabEngine::load(&base1).unwrap();
    engine.prewarm_for_writes();
    let facts = BaseFacts::compute(&engine).expect("facts");
    let trace = engine
        .import_client_update_traced(&update, Some(&facts.version))
        .unwrap();
    assert!(
        trace.sequential,
        "an update on top of the whole base is strictly sequential"
    );
    assert!(
        engine.id_cache_incremental,
        "its id caches were rebuilt from its own ops"
    );
    // A node the incremental rebuild could not have named.
    let lost = *engine.id_to_tree.get("y").unwrap();
    engine.id_to_tree.remove("y");
    engine.tree_to_id.remove(&lost);
    engine.complete_id_cache().unwrap();
    let mut next = LoroCollabEngine::load(&base1).unwrap();
    next.next_import_client_update(&update).unwrap();
    assert_eq!(engine.id_to_tree, next.id_to_tree);
    assert_eq!(engine.tree_to_id, next.tree_to_id);
    assert!(!engine.id_cache_incremental);
}

/// What strictly sequential means, case by case.
#[test]
fn the_sequential_gate_accepts_only_a_strict_causal_extension_of_the_base() {
    let (base0, base1, _) = orphan_base(2);
    let anchor = *LoroCollabEngine::load(&base1)
        .unwrap()
        .id_to_tree
        .get("anchor")
        .unwrap();
    let gate = |base: &[u8], update: &[u8]| {
        let mut engine = LoroCollabEngine::load(base).unwrap();
        engine.prewarm_for_writes();
        let facts = BaseFacts::compute(&engine).expect("facts");
        engine
            .import_client_update_traced(update, Some(&facts.version))
            .unwrap()
            .sequential
    };
    // On top of the whole base: yes, whether it is one change or several.
    let one = raw_edit(&base1, 6_400, |raw| {
        let tree = raw.get_tree("tree");
        tree.enable_fractional_index(0);
        let n = tree.create(TreeParentId::Node(anchor)).unwrap();
        set_node(raw, n, "q", "navigator_node");
    });
    assert!(gate(&base1, &one));
    let several = raw_edit(&base1, 6_401, |raw| {
        let tree = raw.get_tree("tree");
        tree.enable_fractional_index(0);
        for i in 0..3 {
            let n = tree.create(TreeParentId::Node(anchor)).unwrap();
            set_node(raw, n, &format!("q{i}"), "navigator_node");
            raw.commit();
        }
    });
    assert!(gate(&base1, &several));
    // From a client that has not seen the last accepted update: concurrent, so no.
    assert!(!gate(&base1, &stale_create(&base1, &base0, 6_402)));
    // A change that depends only on a part of the base's frontier: no.
    let partial = {
        let author = LoroDoc::new();
        author.import(&base1).unwrap();
        author.set_peer_id(6_403).unwrap();
        let from = author.oplog_vv();
        let tree = author.get_tree("tree");
        tree.enable_fractional_index(0);
        let n = tree.create(TreeParentId::Node(anchor)).unwrap();
        set_node(&author, n, "p", "navigator_node");
        author.commit();
        let frontier = deps_of(&base1);
        assert!(frontier.len() >= 2, "the base has a two-head frontier: {frontier:?}");
        rewrite_deps(&base1, &author, &from, &frontier[..1])
    };
    assert!(!gate(&base1, &partial));
    // An update the base already holds changes nothing: vacuously sequential.
    assert!(gate(&base1, &one));
    let again = {
        let mut engine = LoroCollabEngine::load(&base1).unwrap();
        engine.import_update(&one).unwrap();
        let facts = BaseFacts::compute(&engine).expect("facts");
        engine
            .import_client_update_traced(&one, Some(&facts.version))
            .unwrap()
            .sequential
    };
    assert!(again);
}

// ---------------------------------------------------------------------------------------------
// The corpus extension: shapes the generators could not produce
// ---------------------------------------------------------------------------------------------

fn case(label: String, kind: &'static str, base: &[u8], update: Vec<u8>) -> Case {
    Case {
        label,
        family: "round2",
        kind,
        base: Arc::new(base.to_vec()),
        update,
        expect: None,
        facts: FactsFrom::Base,
    }
}

/// Concurrent imports against a base whose history holds dropped ops, in several shapes.
fn dropped_history_cases(scale: u64) -> Vec<Case> {
    let mut cases = Vec::new();
    for hidden in [2, 12, 40] {
        let (base0, base1, _) = orphan_base(hidden);
        for k in 0..scale {
            let peer = 6_300 + k;
            cases.push(case(
                format!("round2/orphan-{hidden}/{k} :: stale create"),
                "stale_create",
                &base1,
                stale_create(&base1, &base0, peer),
            ));
            // A stale text edit on the anchor's metadata.
            let anchor = *LoroCollabEngine::load(&base1)
                .unwrap()
                .id_to_tree
                .get("anchor")
                .unwrap();
            let stale_edit = {
                let author = LoroDoc::new();
                author.import(&base1).unwrap();
                author.set_peer_id(peer + 100).unwrap();
                let from = author.oplog_vv();
                meta_map(&author, anchor).insert("prop:note", "x").unwrap();
                author.commit();
                let mut json = json_of(&author, &from);
                let stale = deps_of(&base0);
                for change in json["changes"].as_array_mut().unwrap() {
                    change["deps"] = serde_json::Value::from(stale.clone());
                }
                import_json(&base1, &json)
            };
            cases.push(case(
                format!("round2/orphan-{hidden}/{k} :: stale property"),
                "stale_property",
                &base1,
                stale_edit,
            ));
            // A sequential create on the same base: the incremental path stays.
            let sequential = raw_edit(&base1, peer + 200, |raw| {
                let tree = raw.get_tree("tree");
                tree.enable_fractional_index(0);
                let n = tree.create(TreeParentId::Node(anchor)).unwrap();
                set_node(raw, n, &format!("s{k}"), "navigator_node");
            });
            cases.push(case(
                format!("round2/orphan-{hidden}/{k} :: sequential create"),
                "sequential_create",
                &base1,
                sequential,
            ));
        }
    }
    cases
}

/// Offline-client edits of the metadata of deleted nodes, and the same edits delivered in sequence.
fn deleted_metadata_cases(scale: u64) -> Vec<Case> {
    let mut cases = Vec::new();
    for chain in [20, 26] {
        let (before_delete, base, a_dropped) = deleted_chain_base(chain);
        for k in 0..scale {
            let peer = 7_000 + k;
            cases.push(case(
                format!("round2/deleted-{chain}/{k} :: offline content_source removal"),
                "offline_property",
                &base,
                raw_edit(&before_delete, peer, |raw| {
                    meta_map(raw, a_dropped).delete("prop:content_source").unwrap();
                }),
            ));
            cases.push(case(
                format!("round2/deleted-{chain}/{k} :: offline kind change"),
                "offline_kind",
                &base,
                raw_edit(&before_delete, peer + 100, |raw| {
                    meta_map(raw, a_dropped).insert("kind", "collection_field").unwrap();
                }),
            ));
            let author = LoroDoc::new();
            author.import(&before_delete).unwrap();
            author.set_peer_id(peer + 200).unwrap();
            let from = author.oplog_vv();
            meta_map(&author, a_dropped).delete("prop:content_source").unwrap();
            author.commit();
            cases.push(case(
                format!("round2/deleted-{chain}/{k} :: sequential content_source removal"),
                "sequential_deleted_property",
                &base,
                rewrite_deps(&base, &author, &from, &deps_of(&base)),
            ));
        }
    }
    cases
}

/// Edits of live nodes whose metadata map is registered under another tree.
fn cross_tree_cases(scale: u64) -> Vec<Case> {
    let (base, j, base0, anchor) = foreign_meta_base();
    let mut cases = Vec::new();
    for k in 0..scale {
        let peer = 7_500 + k;
        cases.push(case(
            format!("round2/cross-tree/{k} :: kind change"),
            "cross_tree_kind",
            &base,
            foreign_edit(&base0, &base, anchor, peer, |author, j2| {
                assert_eq!(j2, j);
                author
                    .get_tree("tree")
                    .get_meta(j2)
                    .unwrap()
                    .insert("kind", "navigator_node")
                    .unwrap();
            }),
        ));
        cases.push(case(
            format!("round2/cross-tree/{k} :: property"),
            "cross_tree_property",
            &base,
            foreign_edit(&base0, &base, anchor, peer + 100, |author, j2| {
                author
                    .get_tree("tree")
                    .get_meta(j2)
                    .unwrap()
                    .insert("prop:note", "x")
                    .unwrap();
            }),
        ));
        cases.push(case(
            format!("round2/cross-tree/{k} :: text"),
            "cross_tree_text",
            &base,
            foreign_edit(&base0, &base, anchor, peer + 200, |author, j2| {
                let meta = author.get_tree("tree").get_meta(j2).unwrap();
                meta.insert_container("text", LoroText::new())
                    .unwrap()
                    .insert(0, &"t".repeat(50))
                    .unwrap();
            }),
        ));
    }
    cases
}

/// Growth shapes, on both sides of the work budget.
fn growth_cases(scale: u64) -> Vec<Case> {
    let mut cases = Vec::new();
    for (kind, under_deleted, per) in [
        ("collection_field", false, 600),
        ("navigator_node", true, 600),
        ("collection_field", false, 1_000),
    ] {
        for (step, existing) in [0usize, 2_000, 3_300, 3_900].into_iter().enumerate() {
            for k in 0..scale {
                let (base, update) = grow_step(kind, under_deleted, existing, per, 8_000 + k);
                cases.push(case(
                    format!("round2/grow-{kind}-{under_deleted}-{per}/{step}/{k} :: {per} nodes after {existing}"),
                    "growth",
                    &base,
                    update,
                ));
            }
        }
    }
    cases
}

/// Every new family of cases of this round.
pub(super) fn round2_cases(scale: u64) -> Vec<Case> {
    let mut cases = dropped_history_cases(scale);
    cases.extend(deleted_metadata_cases(scale));
    cases.extend(cross_tree_cases(scale));
    cases.extend(growth_cases(scale));
    cases.extend(super::window_round3_tests::round3_cases(scale));
    cases.extend(super::window_round4_tests::round4_cases(scale));
    cases.extend(super::window_round6_tests::round6_cases(scale));
    cases.extend(super::window_setup_guard_tests::round7_cases(scale));
    cases
}

// ---------------------------------------------------------------------------------------------
// R2-3: growth through content no limit counts
// ---------------------------------------------------------------------------------------------

/// A legacy page holding `existing` zero-count (or deleted-parent) nodes, and an update of `per`
/// more, from a client that has seen the whole base.
fn grow_step(kind: &str, under_deleted: bool, existing: usize, per: usize, peer: u64) -> (Vec<u8>, Vec<u8>) {
    let engine = LoroCollabEngine::new_empty(51);
    let tree = engine.doc.get_tree("tree");
    tree.enable_fractional_index(0);
    let trash = tree.create(TreeParentId::Root).unwrap();
    set_node(&engine.doc, trash, "trash", "navigator_node");
    let parent = if under_deleted {
        TreeParentId::Node(trash)
    } else {
        TreeParentId::Root
    };
    for i in 0..existing {
        let n = tree.create(parent).unwrap();
        set_node(&engine.doc, n, &format!("g{i}"), kind);
    }
    engine.doc.commit();
    let before_delete = engine.export_snapshot().unwrap();
    if under_deleted {
        tree.delete(trash).unwrap();
        engine.doc.commit();
    }
    let base = engine.export_snapshot().unwrap();
    let update = if under_deleted {
        // A client that went offline before the delete names more nodes under the parent; its
        // update is delivered on top of the base's frontier.
        let client = LoroDoc::new();
        client.import(&before_delete).unwrap();
        client.set_peer_id(peer).unwrap();
        let from = client.oplog_vv();
        let client_tree = client.get_tree("tree");
        client_tree.enable_fractional_index(0);
        for i in 0..per {
            let n = client_tree.create(TreeParentId::Node(trash)).unwrap();
            set_node(&client, n, &format!("h{i}"), kind);
        }
        client.commit();
        rewrite_first_deps(&base, &client, &from, &deps_of(&base), peer)
    } else {
        raw_edit(&base, peer, |raw| {
            let raw_tree = raw.get_tree("tree");
            raw_tree.enable_fractional_index(0);
            for i in 0..per {
                let n = raw_tree.create(TreeParentId::Root).unwrap();
                set_node(raw, n, &format!("h{i}"), kind);
            }
        })
    };
    assert!(update.len() <= 65_536, "update {} bytes", update.len());
    (base, update)
}

/// Writes of 600 zero-count nodes each, as a client does: the incremental path decides them while
/// the page is within the work budget, and stops once the post-import items would pass it; the
/// full path then decides (next's behaviour), and the page's base takes no facts any more.
#[test]
fn zero_count_nodes_cannot_grow_a_page_through_the_incremental_window() {
    let mut nodes = 0usize;
    let mut last_incremental = 0usize;
    let mut first_fallback = None;
    for step in 0..12usize {
        let (base, update) = grow_step("collection_field", false, nodes, 600, 9_000 + step as u64);
        let judged = judge(&base, &update);
        judged.assert_sound(&format!("collection_field step {step}"));
        if judged.incremental {
            last_incremental = nodes + 600;
        } else if first_fallback.is_none() {
            first_fallback = Some((nodes + 600, judged.reason.clone()));
        }
        nodes += 600;
    }
    let (fell_back_at, reason) = first_fallback.expect("the incremental path stops");
    println!(
        "collection_field: incremental up to {last_incremental} nodes, first fallback at {fell_back_at}: {reason:?}"
    );
    assert_eq!(reason.as_deref(), Some("WorkBudget"));
    // Four items per node, 16,000 items of budget beyond what the live totals allow.
    assert!((3_000..=4_500).contains(&last_incremental), "{last_incremental}");
    let (base, _) = grow_step("collection_field", false, 5_400, 1, 1);
    let engine = LoroCollabEngine::load(&base).unwrap();
    assert!(
        BaseFacts::compute(&engine).is_none(),
        "a base over the budget has no facts"
    );
}

/// Nodes created under a deleted parent: their metadata maps have no path, so no such update is
/// decided incrementally at all, and the base that already holds them counts them in its items.
#[test]
fn nodes_under_a_deleted_parent_are_never_decided_incrementally_and_are_counted() {
    let mut nodes = 0usize;
    for step in 0..8usize {
        let (base, update) = grow_step("navigator_node", true, nodes, 600, 9_500 + step as u64);
        let judged = judge(&base, &update);
        judged.assert_sound(&format!("under a deleted parent, step {step}"));
        assert!(!judged.incremental, "{judged:?}");
        nodes += 600;
    }
    let (base, _) = grow_step("navigator_node", true, 5_400, 1, 1);
    let engine = LoroCollabEngine::load(&base).unwrap();
    assert!(
        BaseFacts::compute(&engine).is_none(),
        "a base over the budget has no facts"
    );
    let (small, _) = grow_step("navigator_node", true, 600, 1, 1);
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&small).unwrap()).is_some());
}

/// Items (`crate::work`) of a page and the budget of its live totals, from a fresh load.
pub(super) fn items_and_budget(base: &[u8]) -> (u64, u64) {
    let engine = LoroCollabEngine::load(base).unwrap();
    let items = engine.projection_items(u64::MAX).unwrap().unwrap();
    let semantic = engine.semantic_snapshot().unwrap();
    let live = || semantic.nodes.values().filter(|node| !node.deleted);
    let count = |kind: crate::operation::NodeKind| live().filter(|node| node.kind == kind).count() as u64;
    let chars: usize = live().map(|node| node.text.chars().count()).sum();
    (
        items,
        crate::work::item_budget_with_marks(
            count(crate::operation::NodeKind::Block),
            count(crate::operation::NodeKind::NavigatorNode),
            chars as u64,
            engine.unit_mark_items().unwrap().values().sum(),
        ),
    )
}

/// Applies `edit` as a client write, repeatedly, to `base`. Every write is judged against the
/// references; an incremental accept must leave the page within its budget. Returns how many
/// writes were decided incrementally, the reason of the first that was not, and the final base.
pub(super) fn grow_by(
    mut base: Vec<u8>,
    writes: u64,
    edit: &dyn Fn(&LoroDoc, u64),
) -> (usize, Option<String>, Vec<u8>) {
    let mut incremental = 0;
    let mut first_fallback = None;
    for write in 0..writes {
        let update = raw_edit(&base, 30_000 + write, |raw| edit(raw, write));
        assert!(update.len() <= 65_536, "update {} bytes", update.len());
        let judged = judge(&base, &update);
        judged.assert_sound(&format!("growth write {write}"));
        base = accepted(&base, &update).as_ref().clone();
        if judged.incremental {
            incremental += 1;
            let (items, budget) = items_and_budget(&base);
            assert!(
                items <= budget,
                "write {write}: an incremental accept left {items} items over the budget {budget}"
            );
        } else if first_fallback.is_none() {
            first_fallback = Some(judged.reason.clone().unwrap_or_default());
        }
    }
    (incremental, first_fallback, base)
}

pub(super) fn flat_page_of(blocks: usize) -> Vec<u8> {
    let engine = LoroCollabEngine::new_empty(61);
    let list: Vec<EditorInitialBlock> = (0..blocks)
        .map(|index| EditorInitialBlock {
            id: format!("b{index}"),
            kind: "paragraph".into(),
            text: "x".into(),
            indent: 0,
            level: None,
        })
        .collect();
    initialize_editor_doc(&engine.doc, &list).unwrap();
    engine.doc.commit();
    engine.export_snapshot().unwrap()
}

/// Small keys in one block's `attributes`, which the reader takes whole for every block and no
/// limit counts.
#[test]
fn block_attribute_keys_cannot_grow_a_flat_page_through_the_incremental_window() {
    let (incremental, first_fallback, base) = grow_by(flat_page_of(400), 8, &|raw, write| {
        let list = super::floor_equivalence_tests::children(raw);
        let Some(ValueOrContainer::Container(Container::Map(block))) = list.get(0) else {
            panic!()
        };
        let attrs = match block.get("attributes") {
            Some(ValueOrContainer::Container(Container::Map(a))) => a,
            _ => block.insert_container("attributes", LoroMap::new()).unwrap(),
        };
        for i in 0..4_500 {
            attrs.insert(&format!("{write}.{i}"), i64::from(i)).unwrap();
        }
    });
    println!("attribute keys: {incremental} incremental writes of 4,500 keys, then {first_fallback:?}");
    assert!(incremental >= 1, "the first write is incremental");
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(
        incremental <= 4,
        "it stops after about 18,000 keys: {incremental} writes"
    );
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none());
}

/// `prop:*` keys on one tree node, which the projection collects for every node.
#[test]
fn node_property_keys_cannot_grow_a_legacy_page_through_the_incremental_window() {
    let mut engine = LoroCollabEngine::new_empty(62);
    create_node(&mut engine, "anchor", None);
    let anchor = *engine.id_to_tree.get("anchor").unwrap();
    let (incremental, first_fallback, base) = grow_by(engine.export_snapshot().unwrap(), 12, &|raw, write| {
        let meta = meta_map(raw, anchor);
        for i in 0..1_500 {
            meta.insert(&format!("prop:w{write}k{i}"), "v").unwrap();
        }
    });
    println!("node properties: {incremental} incremental writes of 1,500 keys, then {first_fallback:?}");
    assert!(incremental >= 1);
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(
        incremental <= 10,
        "it stops after about 14,000 keys: {incremental} writes"
    );
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none());
}

/// A page at the documented limits with realistic content is within the budget, and an update on it
/// is still decided incrementally: the budget does not take the feature away from legitimate pages.
#[test]
fn a_page_at_the_documented_limits_stays_within_the_budget_and_incremental() {
    use super::window_fixtures::{BaseSpec, History, Model, build_base};
    for (model, blocks) in [(Model::Flat, 9_950), (Model::LegacyTree, 10_000)] {
        let built = build_base(BaseSpec {
            model,
            blocks,
            seed: 3,
            history: History::None,
        });
        let engine = LoroCollabEngine::load(&built.snapshot).unwrap();
        let facts = BaseFacts::compute(&engine);
        let (items, budget) = items_and_budget(&built.snapshot);
        println!("{model:?} {blocks}: {items} items, budget {budget}");
        assert!(facts.is_some(), "{model:?}: the page at the limit has facts");
        assert!(items < budget, "{model:?}: {items} items against a budget of {budget}");
        let update = raw_edit(&built.snapshot, 31_000, |raw| {
            if model == Model::LegacyTree {
                let tree = raw.get_tree("tree");
                tree.enable_fractional_index(0);
                for i in 0..50 {
                    let n = tree.create(TreeParentId::Root).unwrap();
                    set_node(raw, n, &format!("chunk{i}"), "collection_field");
                }
            } else {
                let list = super::floor_equivalence_tests::children(raw);
                for i in 0..40 {
                    let block = list.insert_container(list.len(), LoroMap::new()).unwrap();
                    block.insert("nodeName", "paragraph").unwrap();
                    let inline = block.insert_container("children", loro::LoroList::new()).unwrap();
                    inline
                        .insert_container(0, LoroText::new())
                        .unwrap()
                        .insert(0, &format!("block {i}"))
                        .unwrap();
                }
            }
        });
        let judged = judge(&built.snapshot, &update);
        judged.assert_sound("update on a page at the limits");
        assert!(judged.incremental, "{model:?}: {judged:?}");
    }
}
