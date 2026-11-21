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

use loro::{ContainerID, ID, LoroDoc, VersionVector};

use crate::engine::{LoroCollabEngine, ProjectionCount};
use crate::limits::{DocumentLimits, check_snapshot};
use crate::operation::{NodeId, NodeKind};
use crate::semantic::SemanticSnapshot;
use crate::setup_guard::{Fired, SetupGuard};

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

/// The causal position of the base: what the import of an update is measured against to decide
/// whether it is a strict causal extension of the base (`OpSummary::strictly_sequential`).
///
/// A pure function of the base's op log, computed in setup: its version, its frontier, and the
/// highest lamport of any op in it. A lamport is never taken from the wire: the library derives
/// the lamport of every change it imports from the change's dependencies
/// (`import_changes_to_oplog`, `try_get_change_lamport_from_deps`), and a lamport grows along every
/// dependency edge, so the highest lamport of the base is the one of a head.
#[derive(Debug, Clone)]
pub struct BaseVersion {
    pub(crate) oplog_vv: VersionVector,
    pub(crate) frontier: Vec<ID>,
    /// The highest lamport of an op of the base; `None` for a base without ops.
    pub(crate) max_lamport: Option<u32>,
}

impl BaseVersion {
    /// The position of `doc`'s op log, or `None` when a head's change cannot be read.
    #[must_use]
    pub fn of(doc: &LoroDoc) -> Option<Self> {
        let frontier: Vec<ID> = doc.oplog_frontiers().iter().collect();
        let mut max_lamport: Option<u32> = None;
        for head in &frontier {
            let change = doc.get_change(*head)?;
            let offset = u32::try_from(head.counter.checked_sub(change.id.counter)?).ok()?;
            let lamport = change.lamport.checked_add(offset)?;
            max_lamport = Some(max_lamport.map_or(lamport, |current| current.max(lamport)));
        }
        Some(Self {
            oplog_vv: doc.oplog_vv(),
            frontier,
            max_lamport,
        })
    }
}

/// The facts of one accepted base.
#[derive(Debug, Clone)]
pub struct BaseFacts {
    /// The state the facts describe. The incremental check applies them only to an import that
    /// starts from exactly this state.
    pub(crate) state: VersionVector,
    /// The base's causal position, for the sequential gate of the import.
    pub(crate) version: BaseVersion,
    /// The items (see [`crate::work`]) the full projection of the base reads.
    pub(crate) items: u64,
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
    /// The mark items (see [`crate::work::text_mark_items`]) of the editor blocks that have any, by
    /// key: the part of `items` that a text's segments multiply, which the window re-counts for the
    /// blocks an update touched.
    pub(crate) marks: HashMap<NodeId, u64>,
    /// The sum of `marks`: the formatting the budget's allowance applies to.
    pub(crate) mark_items: u64,
}

/// What the setup computed: the facts, or none and which setup guard is the reason, if one is.
#[derive(Debug, Clone)]
pub struct SetupFacts {
    pub facts: Option<BaseFacts>,
    /// The setup has cost more than [`SetupGuard`] allows, so there are no facts. Telemetry: the
    /// page takes the full window, as it does for any base without facts.
    pub guard: Option<Fired>,
}

impl SetupFacts {
    /// No facts, with or without a guard being the reason.
    #[must_use]
    pub const fn none(guard: Option<Fired>) -> Self {
        Self { facts: None, guard }
    }
}

impl BaseFacts {
    /// The facts of `engine`'s current state, or `None` when the incremental check does not
    /// apply to this base and every update takes the full computation: a page with a placement
    /// structure (models 4 and 5, until the structured path exists), a tree that names a logical
    /// id twice, a base that the full projection or the limits do not accept (the facts
    /// presuppose a valid base), or a setup that has cost more than the guard allows
    /// ([`crate::setup_guard`]).
    ///
    /// Costs one full projection, so it belongs to the unmetered setup. The guard counts from this
    /// call; the worker uses [`Self::compute_guarded`] with a guard started before the load.
    #[must_use]
    pub fn compute(engine: &LoroCollabEngine) -> Option<Self> {
        Self::compute_guarded(engine, &SetupGuard::start()).facts
    }

    /// [`Self::compute`] under `guard`, and whether the guard is why there are no facts.
    ///
    /// A fired guard is an ordinary outcome: no facts, so the page takes the full window.
    #[must_use]
    pub fn compute_guarded(engine: &LoroCollabEngine, guard: &SetupGuard) -> SetupFacts {
        // A guard that cannot see the allocation would be silently off: no facts then, the same as
        // for a fired guard, but not reported as one.
        if !crate::alloc_meter::is_installed() {
            return SetupFacts::none(None);
        }
        if let Some(fired) = guard.fired() {
            return SetupFacts::none(Some(fired));
        }
        match Self::compute_within(engine, guard) {
            Ok(facts) => SetupFacts {
                facts: Some(facts),
                guard: None,
            },
            Err(guard) => SetupFacts::none(guard),
        }
    }

    /// The facts, or the guard that stopped the setup (`Err(Some(_))`), or `Err(None)` when the base
    /// has no facts for another reason.
    fn compute_within(engine: &LoroCollabEngine, guard: &SetupGuard) -> Result<Self, Option<Fired>> {
        if engine.has_duplicate_logical_ids() || engine.has_editor_structure().map_err(|_| None)? {
            return Err(None);
        }
        // The work of the full projection, counted before the projection itself runs: a page far
        // over any budget is refused after the ceiling's worth of items, so the walk stops there
        // (`crate::work`). The count converts a text's delta only where the projection converts it
        // (the inline texts of the blocks it reads, `work::block_mark_items`), and a delta cannot be
        // stopped early, so what this setup reads is what the projection reads of the same page.
        // The count also yields the mark items of every block, so no text is converted a third time
        // for them. The guard stops it between texts.
        let (bytes_before, cpu_before) = (guard.spent(), guard.cpu_spent());
        let counted = engine
            .projection_count_guarded(crate::work::item_budget_ceiling(), *guard)
            .map_err(|_| None)?;
        let ProjectionCount::Within { items, marks } = counted else {
            return Err(match counted {
                ProjectionCount::GuardFired(fired) => Some(fired),
                ProjectionCount::OverCeiling | ProjectionCount::Within { .. } => None,
            });
        };
        // The projection that follows converts the texts the count has just converted, so it
        // costs about what the count did, in allocation and in CPU time: both limits must hold
        // for the two.
        let counted_bytes = guard.spent().saturating_sub(bytes_before);
        let counted_nanos = guard.cpu_spent().saturating_sub(cpu_before);
        if let Some(fired) = guard.fired_by(counted_bytes, counted_nanos) {
            return Err(Some(fired));
        }
        let editor_doc = engine.has_editor_doc().map_err(|_| None)?;
        let snapshot = engine.semantic_snapshot().map_err(|_| None)?;
        if let Some(fired) = guard.fired() {
            return Err(Some(fired));
        }
        check_snapshot(&snapshot, &DocumentLimits::default()).map_err(|_| None)?;
        let block_order = engine.root_block_ids().map_err(|_| None)?.unwrap_or_default();
        let facts = Self::from_snapshot(
            engine.doc().state_vv(),
            BaseVersion::of(engine.doc()).ok_or(None)?,
            items,
            editor_doc,
            block_order,
            marks,
            &snapshot,
        );
        // With the exact totals the budget is known: a page above it takes the full computation.
        if facts.items <= facts.item_budget() {
            Ok(facts)
        } else {
            Err(None)
        }
    }

    /// The budget of the base itself (see [`crate::work::item_budget`]).
    pub(crate) const fn item_budget(&self) -> u64 {
        crate::work::item_budget_with_marks(
            self.blocks as u64,
            self.navigators as u64,
            self.chars as u64,
            self.mark_items,
        )
    }

    fn from_snapshot(
        state: VersionVector,
        version: BaseVersion,
        items: u64,
        editor_doc: bool,
        block_order: Vec<ContainerID>,
        marks: HashMap<NodeId, u64>,
        snapshot: &SemanticSnapshot,
    ) -> Self {
        let mark_items = marks.values().fold(0_u64, |sum, items| sum.saturating_add(*items));
        let mut facts = Self {
            state,
            version,
            items,
            editor_doc,
            blocks: 0,
            navigators: 0,
            chars: 0,
            max_depth: 0,
            longest: 0,
            block_order,
            units: HashMap::with_capacity(snapshot.nodes.len()),
            marks,
            mark_items,
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
