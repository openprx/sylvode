//! The refusal of tree operations that name a parent the tree does not hold at that point.
//!
//! A client's Loro document subscribes to its own events. When it imports an update whose change
//! continues the document's head (the library's `Linear` import mode), the tree state applies each
//! `create` and `move` without checking the parent and then unwraps the result
//! (`tree_state.rs`, `apply_diff_and_convert`): a parent that does not exist when the op runs, a
//! node made its own parent, or a move under one of its own descendants panics inside the WASM
//! module, and the document object is dead for good. The server's engine never subscribes, so it
//! applies the same ops without panicking and used to accept, store and fan out such an update.
//!
//! Every shape the check refuses is one a client library cannot author: the editor and the engine
//! create a node under a node that exists, and move one to a place that is not below itself.
//!
//! # What is refused
//!
//! For every `create` or `move` op of the update, in any tree container of any root or nesting
//! (every tree the update names, not only the engine's), whose parent is a node `p`:
//!
//! 1. `p` is the target itself (`tree_parent_missing`: a node does not exist before its own create).
//! 2. `p` is created by an op of the same update in the same tree, and that op is not applied
//!    before this one (in `(lamport, peer)` order, the order the library applies ops in). This is
//!    the forward reference: the final state is well formed, but a client that applies the ops one
//!    by one meets the parent before it exists (`tree_parent_missing`).
//! 3. `p` is not created by the update and is not a node the tree held before it: its own id is not
//!    in the version the document had before the import (a legitimate node is created by the op
//!    whose id it carries) and no earlier move of the update brought it into the tree, or the tree
//!    does not hold it after the import (the parent is in another
//!    tree, or was never created). A node that was **deleted** is still held by the tree (the
//!    library keeps it under the deleted root), so a parent that a concurrent update deleted is not
//!    refused: that is a node that existed, not one that never did (`tree_parent_missing`).
//! 4. The last move of a node in a change that was made on the head would put it below itself
//!    (`tree_parent_cycle`): the library refuses such a move when it checks, and unwraps the
//!    refusal when it does not. The state after the import has no cycle, so a refused move shows as
//!    the target being an ancestor of the new parent. Only changes that depend on the whole of the
//!    previous head are examined: a move that closes a cycle only together with an op of a
//!    concurrent change is Loro's own conflict resolution (the move is dropped), not an error of
//!    the client.
//!
//! A parent that is the deleted root is a deleted node and is not refused.
//!
//! # Cost
//!
//! One pass over the ops the import added (the pass the write window makes anyway), a hash lookup
//! per tree op and, for the last move of each node, a walk up from its new parent. Nothing reads the
//! document beyond the nodes the ops name, so the cost is O(nodes the update creates or moves) plus
//! the depth of the moves' new parents (the base is within the depth limit), and it does not depend
//! on the size of the document. It does not depend on the shape of the import either: a concurrent
//! update is checked by the same rules, because they read only the update's own ops, the version
//! before the import and whether the tree holds a node, and none of those is a function of how the
//! library orders the import.
//!
//! The check runs in the window (`LoroCollabEngine::import_client_update`), so a worker that
//! skipped it would be caught by the host, which runs [`refuse_unresolved_tree_parents`] over the
//! document the worker returned.

use std::collections::{HashMap, HashSet};

use loro::{
    ContainerID, ID, JsonChange, JsonOp, JsonOpContent, JsonTreeOp, LoroDoc, LoroTree, TreeID, TreeParentId,
    VersionVector,
};

use crate::error::CollabError;

/// The reason of the refusal of an update that names a tree parent that does not exist when the op
/// runs.
pub const TREE_PARENT_MISSING_REASON: &str = "tree_parent_missing";
/// The reason of the refusal of an update that moves a node below itself.
pub const TREE_PARENT_CYCLE_REASON: &str = "tree_parent_cycle";

/// Whether `reason` is the reason of a refusal of this module.
#[must_use]
pub fn is_tree_reference_reason(reason: &str) -> bool {
    reason == TREE_PARENT_MISSING_REASON || reason == TREE_PARENT_CYCLE_REASON
}

fn refusal(reason: &str) -> CollabError {
    CollabError::OperationFailed { reason: reason.into() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Create,
    Move,
}

/// One `create` or `move` of the update.
#[derive(Debug)]
struct TreeOp {
    container: ContainerID,
    target: TreeID,
    /// `None` for the root.
    parent: Option<TreeID>,
    kind: Kind,
    /// The library applies ops in `(lamport, peer)` order.
    position: (u32, u64),
    /// Index into [`TreeRefs::deps`].
    change: usize,
}

/// The tree ops of one import, collected by [`Self::visit_change`] and [`Self::visit`] during the
/// scan of the ops the import added.
#[derive(Debug, Default)]
pub(crate) struct TreeRefs {
    ops: Vec<TreeOp>,
    /// The deps of every change that carries a tree op.
    deps: Vec<Vec<ID>>,
    /// The change being read: its first counter, lamport and deps, until a tree op needs a slot.
    current: Option<CurrentChange>,
}

#[derive(Debug)]
struct CurrentChange {
    start: i32,
    lamport: u32,
    deps: Vec<ID>,
    slot: Option<usize>,
}

impl TreeRefs {
    /// A change of the import, before its ops.
    pub(crate) fn visit_change(&mut self, change: &JsonChange) {
        self.current = Some(CurrentChange {
            start: change.id.counter,
            lamport: change.lamport,
            deps: change.deps.clone(),
            slot: None,
        });
    }

    /// An op of the change last given to [`Self::visit_change`]; `peer` is the change's peer.
    pub(crate) fn visit(&mut self, peer: u64, op: &JsonOp) {
        let JsonOpContent::Tree(tree) = &op.content else {
            return;
        };
        let (target, parent, kind) = match tree {
            JsonTreeOp::Create { target, parent, .. } => (*target, *parent, Kind::Create),
            JsonTreeOp::Move { target, parent, .. } => (*target, *parent, Kind::Move),
            JsonTreeOp::Delete { .. } => return,
        };
        let Some(current) = self.current.as_mut() else {
            return;
        };
        let change = *current.slot.get_or_insert_with(|| {
            self.deps.push(std::mem::take(&mut current.deps));
            self.deps.len() - 1
        });
        let offset = u32::try_from(i64::from(op.counter).saturating_sub(i64::from(current.start))).unwrap_or(0);
        self.ops.push(TreeOp {
            container: op.container.clone(),
            target,
            parent,
            kind,
            position: (current.lamport.saturating_add(offset), peer),
            change,
        });
        #[cfg(test)]
        crate::walk_counter::record_tree_op();
    }

    /// Decides the import: `doc` is the document after it and `before` the op log version before it.
    ///
    /// # Errors
    /// `tree_parent_missing` or `tree_parent_cycle`.
    pub(crate) fn verdict(&self, doc: &LoroDoc, before: &VersionVector) -> Result<(), CollabError> {
        if self.ops.is_empty() {
            return Ok(());
        }
        // The earliest create of every node of the update, per tree: a parent the update creates
        // must be created before the op that names it.
        let mut created: HashMap<(&ContainerID, TreeID), (u32, u64)> = HashMap::new();
        // The earliest move of every node of the update, per tree: a node the base never had is
        // brought into the tree by a move (hand-encoded input), and from then on it can be a parent.
        let mut first_move: HashMap<(&ContainerID, TreeID), (u32, u64)> = HashMap::new();
        // The last move of every node of the update, per tree, for the cycle rule.
        let mut last_move: HashMap<(&ContainerID, TreeID), &TreeOp> = HashMap::new();
        for op in &self.ops {
            match op.kind {
                Kind::Create => {
                    let slot = created.entry((&op.container, op.target)).or_insert(op.position);
                    *slot = (*slot).min(op.position);
                }
                Kind::Move => {
                    let slot = first_move.entry((&op.container, op.target)).or_insert(op.position);
                    *slot = (*slot).min(op.position);
                    let slot = last_move.entry((&op.container, op.target)).or_insert(op);
                    if slot.position < op.position {
                        *slot = op;
                    }
                }
            }
        }
        // The trees the update names, by container. A container that is not a reachable tree has
        // no state to consult (`None`).
        let mut trees: HashMap<&ContainerID, Option<LoroTree>> = HashMap::new();
        for op in &self.ops {
            trees
                .entry(&op.container)
                .or_insert_with(|| match doc.get_container(op.container.clone()) {
                    Some(loro::Container::Tree(tree)) => Some(tree),
                    _ => None,
                });
        }
        for op in &self.ops {
            let Some(parent) = op.parent else { continue };
            if parent.is_deleted_root() {
                continue;
            }
            if parent == op.target {
                return Err(refusal(match op.kind {
                    Kind::Create => TREE_PARENT_MISSING_REASON,
                    Kind::Move => TREE_PARENT_CYCLE_REASON,
                }));
            }
            if let Some(position) = created.get(&(&op.container, parent)) {
                if *position >= op.position {
                    return Err(refusal(TREE_PARENT_MISSING_REASON));
                }
            } else {
                #[cfg(test)]
                crate::walk_counter::record_tree_lookup();
                // A node the base never had exists once an earlier move of the update has
                // brought it into the tree.
                let moved_in = first_move
                    .get(&(&op.container, parent))
                    .is_some_and(|position| *position < op.position);
                let known_before = moved_in || before.includes_id(ID::new(parent.peer, parent.counter));
                let held = trees
                    .get(&op.container)
                    .and_then(Option::as_ref)
                    .is_none_or(|tree| tree.contains(parent));
                if !known_before || !held {
                    return Err(refusal(TREE_PARENT_MISSING_REASON));
                }
            }
        }
        self.refuse_cycles(doc, before, &last_move, &trees)
    }

    /// Rule 4: the last move of each node, in a change made on the head, whose new parent has the
    /// node among its ancestors in the state after the import.
    fn refuse_cycles(
        &self,
        doc: &LoroDoc,
        before: &VersionVector,
        last_move: &HashMap<(&ContainerID, TreeID), &TreeOp>,
        trees: &HashMap<&ContainerID, Option<LoroTree>>,
    ) -> Result<(), CollabError> {
        if last_move.is_empty() {
            return Ok(());
        }
        let head = doc.vv_to_frontiers(before);
        for op in last_move.values() {
            let Some(parent) = op.parent else { continue };
            if parent.is_deleted_root() {
                continue;
            }
            let deps = self.deps.get(op.change).map_or(&[][..], Vec::as_slice);
            let on_head = head.iter().all(|id| deps.contains(&id));
            if !on_head {
                continue;
            }
            let Some(tree) = trees.get(&op.container).and_then(Option::as_ref) else {
                continue;
            };
            let mut seen: HashSet<TreeID> = HashSet::new();
            let mut at = parent;
            loop {
                if at == op.target {
                    return Err(refusal(TREE_PARENT_CYCLE_REASON));
                }
                if !seen.insert(at) {
                    break;
                }
                #[cfg(test)]
                crate::walk_counter::record_tree_lookup();
                match tree.parent(at) {
                    Some(TreeParentId::Node(next)) => at = next,
                    _ => break,
                }
            }
        }
        Ok(())
    }
}

/// The host's check of the document a worker returned.
///
/// Scans the ops of `doc` that are not in `before` (the op log version of the base the worker was
/// given) with the rules of this module. A wrong worker that accepted an update the window refuses
/// is refused here.
///
/// # Errors
/// `tree_parent_missing`, `tree_parent_cycle`, or an update whose ops cannot all be read.
pub fn refuse_unresolved_tree_parents(doc: &LoroDoc, before: &VersionVector) -> Result<(), CollabError> {
    let mut refs = TreeRefs::default();
    crate::floor_guard::scan_imported_ops_visiting(doc, before, true, &mut |scanned| match scanned {
        crate::floor_guard::Scanned::Change(change) => refs.visit_change(change),
        crate::floor_guard::Scanned::Op(peer, op, _) => refs.visit(peer, op),
    })?;
    refs.verdict(doc, before)
}

/// [`refuse_unresolved_tree_parents`] for a base given as snapshot bytes: its version is read from
/// the blob header, so the base is not loaded.
///
/// # Errors
/// As [`refuse_unresolved_tree_parents`], and `decode_failed` for a header that cannot be read.
pub fn refuse_unresolved_tree_parents_over(doc: &LoroDoc, base_snapshot: &[u8]) -> Result<(), CollabError> {
    let meta = LoroDoc::decode_import_blob_meta(base_snapshot, false).map_err(|e| CollabError::DecodeFailed {
        input: "snapshot",
        reason: e.to_string(),
    })?;
    refuse_unresolved_tree_parents(doc, &meta.partial_end_vv)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]
mod tests {
    use crate::LoroCollabEngine;
    use crate::engine::CollabEngine;

    fn snapshots(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                snapshots(&path, out);
            } else if path.extension().is_some_and(|e| e == "snapshot") {
                out.push(path);
            }
        }
    }

    /// The history of every snapshot a real client library produced (undo, redo, paste, cut,
    /// split) passes the rules: nothing a client authors is refused. The one fixture that records
    /// a dangling reference is a block reference of the projection, not a tree parent.
    #[test]
    fn the_history_of_every_client_fixture_is_accepted() {
        let mut files = Vec::new();
        snapshots(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
            &mut files,
        );
        assert!(files.len() > 20, "{} fixtures", files.len());
        for file in files {
            let engine = LoroCollabEngine::load(&std::fs::read(&file).unwrap()).unwrap();
            let verdict = engine.refuse_unresolved_tree_parents_in_history();
            assert!(verdict.is_ok(), "{}: {verdict:?}", file.display());
        }
    }

    /// The base's version in the blob header of a snapshot is the version of the document the
    /// snapshot loads to (the host relies on it).
    #[test]
    fn the_snapshot_header_names_the_version_of_the_document() {
        let mut engine = LoroCollabEngine::new_empty(5);
        engine
            .apply_content_operation(&crate::operation::Operation::CreateNode {
                id: "a".into(),
                parent: None,
                index: 0,
                kind: crate::operation::NodeKind::NavigatorNode,
            })
            .unwrap();
        let snapshot = engine.export_snapshot().unwrap();
        let meta = loro::LoroDoc::decode_import_blob_meta(&snapshot, false).unwrap();
        assert_eq!(meta.partial_end_vv, engine.doc().oplog_vv());
    }
}
