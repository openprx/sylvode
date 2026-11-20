//! Facts about the trusted base of a write: what the structural limits read, per unit and in total.
//!
//! The write window imports a client update into the accepted base and checks the structural
//! limits of the result. Re-reading every block to do that costs O(page), so the incremental check
//! (`window_check`) starts from facts about the base and corrects them by what the update touched.
//! The facts are a pure function of the accepted base: they are computed from the document the
//! host supplied and the worker loaded, before the update is looked at, in the worker's unmetered
//! setup (the owner's decision D2b). Nothing that reaches a verdict is decided here: every check
//! still runs in the window, on the document after the import.
//!
//! A wrong fact would turn into a wrong decision, which is why the facts are built by the full
//! projection itself (`semantic_snapshot`) and not by a second reader, and why the host compares
//! the result of every incremental window with its own full projection of the accepted document.

use std::collections::HashMap;

use loro::{ContainerID, VersionVector};

use crate::engine::LoroCollabEngine;
use crate::limits::{DocumentLimits, check_snapshot};
use crate::operation::{NodeId, NodeKind};
use crate::semantic::SemanticSnapshot;

/// What one projected node contributes to the limits: the base's pre-image of a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnitFact {
    pub live: bool,
    pub kind: NodeKind,
    /// Characters of the node's text, counted only while the node is live.
    pub chars: usize,
    /// Parent hops to the root through nodes the projection keeps.
    pub depth: usize,
}

impl UnitFact {
    /// Live blocks, live navigators and live characters this unit adds to the totals.
    pub(crate) fn contribution(&self) -> Contribution {
        Contribution {
            blocks: i64::from(self.live && matches!(self.kind, NodeKind::Block)),
            navigators: i64::from(self.live && matches!(self.kind, NodeKind::NavigatorNode)),
            chars: if self.live {
                i64::try_from(self.chars).unwrap_or(i64::MAX)
            } else {
                0
            },
        }
    }
}

/// A unit's share of the three summed aggregates. Signed, so that a delta is a difference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Contribution {
    pub blocks: i64,
    pub navigators: i64,
    pub chars: i64,
}

/// The facts of one accepted base.
#[derive(Debug, Clone)]
pub struct BaseFacts {
    /// The state the facts describe. The incremental check applies them only to an import that
    /// starts from exactly this state.
    pub(crate) state: VersionVector,
    /// The page has an editor document (`doc.children`), possibly empty.
    pub(crate) editor_doc: bool,
    pub(crate) blocks: usize,
    pub(crate) navigators: usize,
    pub(crate) chars: usize,
    /// The deepest and the longest unit of the base: both are within their limits, a precondition
    /// of the facts, so a maximum can only be violated by a unit the update touched.
    pub(crate) max_depth: usize,
    pub(crate) longest: usize,
    /// The block maps of `doc.children` in list order (empty without an editor document): what an
    /// update that deletes from the list is measured against.
    pub(crate) block_order: Vec<ContainerID>,
    /// Every node of the base's projection by its snapshot key: editor blocks by container id,
    /// tree nodes by logical id.
    pub(crate) units: HashMap<NodeId, UnitFact>,
}

impl BaseFacts {
    /// The facts of `engine`'s current state, or `None` when the incremental check does not
    /// apply to this base and every update takes the full computation: a page with a placement
    /// structure (models 4 and 5, until the structured path exists), a tree that names a logical
    /// id twice, or a base that the full projection or the limits do not accept (the facts
    /// presuppose a valid base).
    ///
    /// Costs one full projection, so it belongs to the unmetered setup.
    #[must_use]
    pub fn compute(engine: &LoroCollabEngine) -> Option<Self> {
        if engine.has_duplicate_logical_ids() || engine.has_editor_structure().ok()? {
            return None;
        }
        let editor_doc = engine.has_editor_doc().ok()?;
        let snapshot = engine.semantic_snapshot().ok()?;
        check_snapshot(&snapshot, &DocumentLimits::default()).ok()?;
        let block_order = engine.root_block_ids().ok()?.unwrap_or_default();
        Some(Self::from_snapshot(
            engine.doc().state_vv(),
            editor_doc,
            block_order,
            &snapshot,
        ))
    }

    fn from_snapshot(
        state: VersionVector,
        editor_doc: bool,
        block_order: Vec<ContainerID>,
        snapshot: &SemanticSnapshot,
    ) -> Self {
        let mut facts = Self {
            state,
            editor_doc,
            blocks: 0,
            navigators: 0,
            chars: 0,
            max_depth: 0,
            longest: 0,
            block_order,
            units: HashMap::with_capacity(snapshot.nodes.len()),
        };
        for (key, node) in &snapshot.nodes {
            let unit = UnitFact {
                live: !node.deleted,
                kind: node.kind,
                chars: node.text.chars().count(),
                depth: crate::limits::depth_of(snapshot, key),
            };
            let share = unit.contribution();
            facts.blocks += usize::try_from(share.blocks).unwrap_or(0);
            facts.navigators += usize::try_from(share.navigators).unwrap_or(0);
            facts.chars += usize::try_from(share.chars).unwrap_or(0);
            facts.max_depth = facts.max_depth.max(unit.depth);
            if unit.live {
                facts.longest = facts.longest.max(unit.chars);
            }
            facts.units.insert(key.clone(), unit);
        }
        facts
    }
}
