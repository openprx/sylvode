//! The write window's import as `next` has it, kept as the oracle of the tests.
//!
//! `next` rebuilds the id caches in full after every import (`next_rebuild_id_cache`, a copy of its
//! `rebuild_id_cache`) and decides with `semantic_snapshot` and `check_snapshot`. The branch must
//! behave exactly so whenever it takes the full path, with the switch off for every input, and for
//! every update that is not strictly sequential. These functions are that behaviour, written down
//! without the helpers the branch added (`store_logical_id`, `read_logical_id`, the incremental
//! rebuild), so that a mistake in the helpers shows against them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use super::{CollabEngine, CollabError, Diff, InputLimits, LoroCollabEngine, NodeId, model5_marker};
use crate::limits::{DocumentLimits, check_snapshot};

impl LoroCollabEngine {
    /// `rebuild_id_cache` of `next`, verbatim.
    pub(super) fn next_rebuild_id_cache(&mut self) -> Result<(), CollabError> {
        for tree_id in self.tree.nodes() {
            if self.tree_to_id.contains_key(&tree_id) {
                continue;
            }
            let meta = self
                .tree
                .get_meta(tree_id)
                .map_err(|e| super::map_loro_error(&e, "<rebuild>"))?;
            let Some(raw) = meta.get(super::META_LOGICAL_ID) else {
                continue;
            };
            if let Some(logical) = super::read_string_value(&raw) {
                let id = NodeId::from(logical);
                self.id_to_tree.insert(id.clone(), tree_id);
                self.tree_to_id.insert(tree_id, id);
            }
        }
        Ok(())
    }

    /// `import_client_update` of `next`: the causal import, the client scan, the full id-cache
    /// rebuild, the downgrade guard.
    pub(super) fn next_import_client_update(&mut self, update: &[u8]) -> Result<Diff, CollabError> {
        InputLimits::default().validate_update(update)?;
        let prior_m5 = model5_marker(&self.doc)?;
        let before_frontiers = self.doc.state_frontiers();
        let before_ops = self.doc.oplog_vv();
        self.invalidate_editor_model_floor();
        let before = self.doc.state_vv();
        let status = self.doc.import(update).map_err(|e| CollabError::DecodeFailed {
            input: "update",
            reason: e.to_string(),
        })?;
        if status.pending.is_some() {
            return Err(CollabError::CausalDependencyMissing);
        }
        let after = self.doc.state_vv();
        let scanned = Some(crate::floor_guard::scan_imported_ops(&self.doc, &before_ops, true)?);
        self.next_rebuild_id_cache()?;
        self.enforce_floor_ratchet(prior_m5, &before_ops, &before_frontiers, scanned)?;
        Ok(Diff {
            changed: before != after,
        })
    }

    /// The whole window of `next` on a loaded base: import, projection, limits.
    pub(super) fn next_window(&mut self, update: &[u8]) -> Result<(), CollabError> {
        self.next_import_client_update(update)?;
        let semantic = self.semantic_snapshot()?;
        check_snapshot(&semantic, &DocumentLimits::default()).map_err(CollabError::from)
    }

    /// What the host does with the document a worker returned: export it, load the snapshot into a
    /// fresh engine, project and check. Independent of every id cache of `self`.
    pub(super) fn fresh_load_decision(&self) -> Result<(), CollabError> {
        let fresh = Self::load(&self.export_snapshot()?)?;
        let semantic = fresh.semantic_snapshot()?;
        check_snapshot(&semantic, &DocumentLimits::default()).map_err(CollabError::from)
    }
}
