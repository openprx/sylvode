//! Hand-encoded updates for tests: tree ops that the library's own API cannot author, such as a
//! move of a node whose create op the receiver never saw.
//!
//! Depends only on the library and the engine, so a regression test can use it against any
//! revision of the window check.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::sync::Arc;

use loro::{ExportMode, LoroDoc, TreeID, TreeParentId};
use serde_json::Value;

use super::{CollabEngine, LoroCollabEngine};

/// The frontier of `doc` as `counter@peer` strings, the form a JSON update names its deps in.
pub(super) fn frontier_strings(doc: &LoroDoc) -> Vec<String> {
    doc.oplog_frontiers()
        .iter()
        .map(|id| format!("{}@{}", id.counter, id.peer))
        .collect()
}

/// The changes `author` made after `from`, re-encoded through JSON with every change's deps
/// replaced by `deps` (`counter@peer` strings), as a binary update against `base`. The ops keep
/// their ids and lamport stamps, so an op that the author made on top of ops the base does not
/// have is delivered without them.
pub(super) fn rewrite_deps(base: &[u8], author: &LoroDoc, from: &loro::VersionVector, deps: &[String]) -> Vec<u8> {
    let mut json = serde_json::to_value(
        author
            .export_json_updates_without_peer_compression(from, &author.oplog_vv())
            .unwrap(),
    )
    .unwrap();
    for change in json["changes"].as_array_mut().unwrap() {
        change["deps"] = Value::from(deps.iter().map(|d| Value::String(d.clone())).collect::<Vec<_>>());
    }
    let rewritten = LoroDoc::new();
    rewritten.import(base).unwrap();
    let before = rewritten.oplog_vv();
    rewritten
        .import_json_updates(serde_json::to_string(&json).unwrap())
        .unwrap();
    rewritten.export(ExportMode::updates(&before)).unwrap()
}

/// Names a tree node and gives it a kind, the way the engine does.
pub(super) fn set_node(doc: &LoroDoc, node: TreeID, logical: &str, kind: &str) {
    let meta = doc.get_tree("tree").get_meta(node).unwrap();
    meta.insert("logical_id", logical).unwrap();
    meta.insert("kind", kind).unwrap();
}

/// A tree node that the base holds only through a hand-encoded move: its create op, the first op
/// of peer `peer_x`, is not in the base. `edit` runs as peer `peer` against a document that holds
/// the base and that create op, and the update it returns carries `edit`'s ops with the base's
/// frontier as their only dependency, so the base takes the move of a node it never saw created.
pub(super) fn unborn_update(
    base: &[u8],
    peer_x: u64,
    peer: u64,
    edit: impl FnOnce(&LoroDoc, TreeID),
) -> (Vec<u8>, TreeID) {
    let author = LoroDoc::new();
    author.import(base).unwrap();
    let deps = frontier_strings(&author);
    author.set_peer_id(peer_x).unwrap();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let x = tree.create(TreeParentId::Root).unwrap();
    assert_eq!(x, TreeID::new(peer_x, 0), "peer {peer_x} must have no ops in the base");
    author.commit();
    author.set_peer_id(peer).unwrap();
    let from = author.oplog_vv();
    edit(&author, x);
    author.commit();
    (rewrite_deps(base, &author, &from, &deps), x)
}

/// The first op of `peer_x`: a plain create of the tree node `x` under `parent`, named `logical`.
pub(super) fn create_unborn(base: &[u8], peer_x: u64, parent: Option<TreeID>, logical: &str, x: TreeID) -> Vec<u8> {
    raw_edit(base, peer_x, |raw| {
        let tree = raw.get_tree("tree");
        tree.enable_fractional_index(0);
        let created = tree
            .create(parent.map_or(TreeParentId::Root, TreeParentId::Node))
            .unwrap();
        assert_eq!(created, x, "the create names the node the base holds");
        set_node(raw, created, logical, "navigator_node");
    })
}

/// `base` with `update` imported through the replay import (accepted history), as a snapshot.
pub(super) fn accepted(base: &[u8], update: &[u8]) -> Arc<Vec<u8>> {
    let mut engine = LoroCollabEngine::load(base).unwrap();
    engine.import_update(update).unwrap();
    Arc::new(engine.export_snapshot().unwrap())
}

/// The engine tree node a logical id names in `base`.
pub(super) fn tree_id_of(base: &[u8], logical: &str) -> TreeID {
    *LoroCollabEngine::load(base).unwrap().id_to_tree.get(logical).unwrap()
}

/// The update produced by `edit` on a raw copy of `base`, as peer `peer`.
pub(super) fn raw_edit(base: &[u8], peer: u64, edit: impl FnOnce(&LoroDoc)) -> Vec<u8> {
    let raw = LoroDoc::new();
    raw.import(base).unwrap();
    raw.set_peer_id(peer).unwrap();
    let before = raw.oplog_vv();
    edit(&raw);
    raw.commit();
    raw.export(ExportMode::updates(&before)).unwrap()
}
