//! The incremental check of the write window.
//!
//! The window imports a client update into the accepted base and checks the structural limits of
//! the result inside the isolated worker's CPU ceiling. Projecting the whole page (`semantic_snapshot`)
//! to do that is linear in the page, so for the pages this module models it works from the
//! [`BaseFacts`] of the base and re-evaluates only the units the update touched, on the document
//! *after* the import:
//!
//! * the totals (live blocks, live navigators, live characters) are the base's plus the sum, over the
//!   touched units, of what the unit contributes now minus what the base says it contributed;
//! * the two maxima (depth, longest block) can only be violated by a touched unit, since the base
//!   is within both limits (a precondition of the facts);
//! * every touched unit is evaluated by the same code the full projection uses (`evaluate_block`,
//!   `evaluate_tree_unit`), and one that the reader rejects is not decided here.
//!
//! Incremental evaluation *accepts*, and refuses only what its exact totals prove: a total (live
//! blocks, live navigators, live characters) over its limit is refused here with the limit kind,
//! `observed` and `limit` the full computation reports (see `finish`). Anything else, whether a
//! depth or a longest text may be over its limit (those two are only bounds), a unit is invalid, a
//! structure changes or the check cannot classify what the update did with certainty, returns a
//! [`FallbackReason`] and the caller runs today's full computation (`semantic_snapshot` and
//! `check_snapshot`), so every refusal carries exactly the reason, limit kind and numbers it
//! carries today.
//!
//! # Which units an update touched
//!
//! The update is hostile input, so "touched" is read from the import itself, never from what the
//! client claims: the ops the library added to the op log (`floor_guard::scan_imported_ops_visiting`,
//! one pass shared with the downgrade guard). Each op names its container, and a container id is the
//! id of the op that created it, so it cannot be faked. A container is mapped to its unit by its
//! path from the root in the document after the import (`LoroDoc::get_path_to_container`), which is
//! state, not a claim: a container that is not reachable has no path and can change nothing a
//! reader sees.
//!
//! * Editor block (E-unit): the element of `doc.children` that is the ancestor of the container.
//!   An element that the update inserted is named by the insert op.
//! * Legacy tree node (L-unit): the node whose metadata map is the ancestor of the container, or a
//!   node the update created.
//!
//! A delete on `doc.children` is applied by the library by position, so the ids its op names say
//! nothing about the elements it removed. The removed blocks are read from the document after the
//! import instead: the list's value names every element by its container id, and the list must be
//! the base's list (kept in the facts) without the removed blocks and with the inserted ones,
//! element by element, or the check does not decide.
//!
//! # The soundness condition
//!
//! The incremental path is taken only if all of the following hold. Each is enforced by the check
//! named, and the check falls back (the reason in brackets) when it does not hold or cannot be
//! established.
//!
//! 1. The facts exist and describe the state the import started from (`FactsAbsent`, `StaleFacts`):
//!    the base has no placement structure, no duplicate logical id, projects, is within every limit,
//!    and its root list is not longer than [`ROOT_LIST_FALLBACK_LEN`] (`BaseFacts::compute`); the
//!    facts' version vector equals the import's `before_state`.
//! 2. The import left nothing pending, and the scan visited every op it added (`Unclassified`).
//! 3. No op touched the `doc` map, the structure meta or tree, a shadow root, or a tree other than
//!    the engine's (`GlobalStructure`, `TreeStructure`); no op on the engine tree is anything but
//!    a create whose target is its own op id (`TreeStructure`); no map op writes or deletes the key
//!    `logical_id` except in the metadata map of a node the update created (`IdRewritten`); no
//!    container under a tree node is reached through any other root (`Unclassified`).
//! 4. Every element inserted into the root list is a map container created by the inserting op
//!    itself, and no element of another kind was inserted (`UnreadableInsert`).
//! 5. Every created target is a node the tree did not hold before the import. The two id caches
//!    together list every node of the base tree, live or deleted, named or not (the load-time
//!    rebuild visits all of `LoroTree::nodes()`, deleted nodes included, and every later rebuild
//!    adds what it visits), so a created target found in either is not new, however its ops look: a
//!    hand-encoded move of a target whose create op the base never saw makes the node exist
//!    without it (`ResolvedNodeId`). Read from the other side, on the document after the import,
//!    no created node has a child that the update did not create (`ResolvedNodeId`). No existing
//!    node got its logical id from the update, and no two nodes claim one id (`ResolvedNodeId`,
//!    `DuplicateLogicalId`).
//! 6. The root list is not longer than [`ROOT_LIST_FALLBACK_LEN`] after the import
//!    (`OversizedList`), so a page cannot grow through this path past what the full path allows.
//! 7. Every touched container maps to a unit or is unreachable; every touched unit is valid for the
//!    reader and no key crosses between the two kinds of unit; no existing tree node enters or
//!    leaves the projection; the alias map is within its limit; no deepest or longest unit is over
//!    its limit (`Unclassified`, `InvalidUnit`, `KeyCollision`, `PresenceChange`, `AliasLimit`,
//!    `LimitReached`); and the facts and the delta agree (`Inconsistent`).
//!
//! Under that condition, every unit whose evaluation or whose contribution to a total or maximum
//! can differ from the base is in the touched set, because:
//!
//! * **Editor blocks.** A block's contribution (live, characters) and its validity depend only on
//!   the containers reachable from its own map and on global state that conditions 1, 3 and 7 keep
//!   fixed (the editor document exists; the alias map is within its limit). A block
//!   exists only as an element of the root list, a plain list that has no move and no set, whose
//!   elements are containers created by the op that inserted them (condition 4): an element that
//!   no insert op names is a base element with its identity unchanged, no removed element comes
//!   back, and two peers cannot create one id. The elements removed are derived exactly from the
//!   list after the import against the base's order, or the check falls back. A change to a block
//!   is an op on a container reachable from it, whose path from the root in the document after the
//!   import runs through the block (condition 7). So the blocks whose evaluation can differ are
//!   the inserted ones, the removed ones and the ones some op's container maps to.
//! * **Legacy tree nodes.** A node's contribution depends on its kind, its text, its
//!   `content_source`, its key, its deleted state (its own and its ancestors') and its depth (its
//!   parent chain). Under conditions 3 and 5 the update moves and deletes nothing and writes no
//!   existing node's key, so no existing node changes parent, ancestors, key or depth; an existing
//!   node can change only through an op on its own metadata map, which maps to it by path. A created
//!   node is new, exists only because of its own create op and has no child but created ones
//!   (condition 5), so it has no earlier state to differ from and nothing existing hangs under it;
//!   its depth is its parent chain through created nodes and then the base's depth of an existing
//!   parent, which has not changed. So the nodes whose evaluation can differ are the created ones
//!   and the ones some op's container maps to. A node without a logical id is not in the projection
//!   and counts toward nothing.
//! * **Totals.** Each total is the sum of per-unit contributions, so the total after the update is
//!   the base's plus the sum, over the touched units, of the contribution now minus the base's
//!   contribution: exact. Every other unit contributes what it did.
//! * **Maxima.** The base is within the depth and longest-text limits (a precondition of the
//!   facts), and every unit that is not touched has the depth and text it had, so a maximum can only
//!   be over its limit in a touched unit, and each touched unit is checked, the check stopping at
//!   the first one over. The depth and longest bounds in the report are the larger of the base's
//!   maximum and the touched units' values, so they are at least the exact maxima.
//!
//! # Shapes that take the full path, and the cost of those that do not
//!
//! The full computation decides: pages with a placement structure (editor models 4 and 5); a base
//! with duplicate logical ids or one the projection or the limits do not accept; every legacy tree
//! move or delete; any op on the `doc` map, the structure containers or a tree other than the
//! engine's; a created node that already existed; a rewrite of an existing node's logical id; a
//! root list longer than [`ROOT_LIST_FALLBACK_LEN`]; and every shape condition 7 names.
//!
//! "One window does not grow with the page" has one exception that predates this module and is
//! not changed by it: tree nodes without a logical id. `rebuild_id_cache_for` re-reads every node
//! the last rebuild saw without an id on every import (an import can still give one), the full
//! path did the same, and no limit counts such nodes, so a page that holds many of them makes every
//! window O(those nodes) on either path.

use std::collections::{HashMap, HashSet};

use loro::{
    Container, ContainerID, ContainerType, Index, JsonListOp, JsonMapOp, JsonOp, JsonOpContent, JsonTreeOp, TreeID,
};

use crate::engine::{ImportTrace, LoroCollabEngine, TreeUnit};
use crate::error::CollabError;
use crate::facts::{BaseFacts, Contribution, UnitFact};
use crate::limits::{DocumentLimits, LimitViolation, check_snapshot, check_snapshot_with_depth, max_depth_capped};
use crate::operation::NodeKind;
use crate::reserved_roots::{ALIASES, DOC, ENGINE_TREE, META, TREE};
use crate::semantic::SemanticSnapshot;

/// Deliberate faults of the check, for the mutants of the differential harness: each is a way an
/// incremental check goes wrong, and the harness must catch every one.
#[cfg(test)]
pub mod fault {
    use std::cell::Cell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Fault {
        /// Decides incrementally even when the update changed a global structure, moved or
        /// deleted a tree node, or gave an existing node its id.
        StructuralFallbackSkipped,
        /// Does not subtract the blocks the update removed from `doc.children`.
        RemovedBlocksIgnored,
        /// Forgets the units the update changed: only inserted blocks and created nodes count.
        TouchedUnitMissed,
        /// Adds each unit's change to the totals twice.
        DeltaAppliedTwice,
        /// Takes a created tree node for new although it already existed, and does not look at
        /// the nodes that hang under a created one.
        CreatedTargetChecksSkipped,
        /// Does not fall back for a root list longer than [`super::ROOT_LIST_FALLBACK_LEN`].
        ListThresholdSkipped,
        /// Does not fall back when an update writes the logical id of an existing tree node.
        IdRewriteSkipped,
        /// Reports a limit refusal with an `observed` value one too high.
        RefusalOffByOne,
    }

    thread_local! {
        static ACTIVE: Cell<Option<Fault>> = const { Cell::new(None) };
    }

    pub fn active() -> Option<Fault> {
        ACTIVE.with(Cell::get)
    }

    /// Holds a fault active on this thread until it is dropped.
    pub struct Injected;

    pub fn inject(fault: Fault) -> Injected {
        ACTIVE.with(|active| active.set(Some(fault)));
        Injected
    }

    impl Drop for Injected {
        fn drop(&mut self) {
            ACTIVE.with(|active| active.set(None));
        }
    }
}

/// Whether the harness injected the named fault on this thread. Always false outside tests.
macro_rules! injected {
    ($fault:ident) => {{
        #[cfg(test)]
        {
            fault::active() == Some(fault::Fault::$fault)
        }
        #[cfg(not(test))]
        {
            false
        }
    }};
}

/// What the incremental check established about an accepted update.
///
/// The totals are exact and the two maxima are bounds. The host compares the report with its own
/// full projection of the accepted document (`compare_with_projection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowReport {
    pub blocks: u64,
    pub navigators: u64,
    pub chars: u64,
    /// At least the deepest node of the accepted document.
    pub depth_bound: u64,
    /// At least the longest live text of the accepted document.
    pub longest_bound: u64,
}

/// Declares [`FallbackReason`] together with the list of all its variants, so a test can demand
/// that every reason is exercised and a new reason cannot be added without being in the list.
macro_rules! fallback_reasons {
    ($($(#[$meta:meta])* $name:ident,)*) => {
        /// Why the incremental check did not decide, so that the full computation did.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum FallbackReason {
            $($(#[$meta])* $name,)*
        }

        impl FallbackReason {
            /// Every reason, for the test that each one is reached.
            pub const ALL: &'static [Self] = &[$(Self::$name,)*];
        }
    };
}

fallback_reasons! {
    /// No facts: a base the incremental check does not describe (structured page, duplicate ids,
    /// or a base the projection does not accept).
    FactsAbsent,
    /// The facts describe another state than the one the import started from.
    StaleFacts,
    /// An op touched the `doc` map, the structure meta or the structure tree.
    GlobalStructure,
    /// An element other than a map container was inserted into `doc.children`.
    UnreadableInsert,
    /// A legacy tree op that is not a plain create.
    TreeStructure,
    /// Two tree nodes claim one logical id.
    DuplicateLogicalId,
    /// An existing tree node got a logical id from this update, a created target already existed
    /// in the tree (live, deleted or unnamed), a created node has children the update did not
    /// create, or the id caches were rebuilt in full.
    ResolvedNodeId,
    /// An update wrote the logical id of a tree node that already existed.
    IdRewritten,
    /// The root list is longer than [`ROOT_LIST_FALLBACK_LEN`] elements.
    OversizedList,
    /// A container could not be mapped to a unit with certainty.
    Unclassified,
    /// A unit's key is, or looks like, a key of the other kind of unit.
    KeyCollision,
    /// A touched unit is invalid for a reader, or the reader failed on it.
    InvalidUnit,
    /// The alias map is over the limit of the reader.
    AliasLimit,
    /// A tree node that the base projection kept is not kept now, or the reverse.
    PresenceChange,
    /// The facts and the update disagree (a count would go negative, or a create names a node the
    /// base already has).
    Inconsistent,
    /// The deepest node or the longest text may be over its limit. Those two values are only
    /// bounds, so the full computation reports the violation with its exact numbers.
    LimitReached,
}

/// The outcome of one window: the report when the incremental check decided, the reason when the
/// full computation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowAccepted {
    pub report: Option<WindowReport>,
    pub fallback: Option<FallbackReason>,
}

/// Imports `update` into `engine`, which holds the trusted base the `facts` describe, and decides
/// the structural limits of the result.
///
/// The decision is incremental when the check can be certain and the full computation otherwise.
/// This is the whole metered window.
///
/// # Errors
/// Everything `import_client_update` refuses, and the refusal of the full computation (a projection
/// error or a `LimitExceeded`), identical to what the full computation alone reports.
pub fn run_window(
    engine: &mut LoroCollabEngine,
    facts: Option<&BaseFacts>,
    update: &[u8],
) -> Result<WindowAccepted, CollabError> {
    let trace = engine.import_client_update_traced(update)?;
    match check(engine, facts, &trace) {
        Ok(report) => Ok(WindowAccepted {
            report: Some(report),
            fallback: None,
        }),
        // A total over its limit is exact (the base's fact plus the exact change of the touched
        // units), and no limit that precedes it can be violated, so the refusal is the full
        // computation's own, produced without running it.
        Err(Undecided::Refused(violation)) => {
            #[cfg(test)]
            crate::walk_counter::record_direct_refusal();
            Err(CollabError::from(violation))
        }
        Err(Undecided::Fallback(reason)) => {
            #[cfg(test)]
            {
                crate::walk_counter::record_fallback();
                crate::walk_counter::record_fallback_reason(&format!("{reason:?}"));
            }
            let semantic = engine.semantic_snapshot()?;
            check_snapshot(&semantic, &DocumentLimits::default()).map_err(CollabError::from)?;
            Ok(WindowAccepted {
                report: None,
                fallback: Some(reason),
            })
        }
    }
}

/// The ops of one client import, reduced to what the incremental check needs. Filled by
/// [`Self::visit`], one call per op of the imported span.
#[derive(Debug, Default)]
pub(crate) struct OpSummary {
    /// What kinds of op the update carried.
    seen: Seen,
    /// The containers of the maps that the update inserted into `doc.children`.
    inserted_blocks: Vec<ContainerID>,
    /// Tree nodes the update created in the legacy tree.
    pub(crate) tree_created: Vec<TreeID>,
    /// Every other container an op named (not roots, not `doc.children`).
    containers: HashSet<ContainerID>,
    /// The containers in which an op wrote or deleted the key `logical_id` of a map: for a tree
    /// node, its metadata map.
    logical_id_writes: Vec<ContainerID>,
}

/// The kinds of op an update carried, as a set.
#[derive(Debug, Default, Clone, Copy)]
struct Seen(u8);

impl Seen {
    /// An op touched the `doc` map, the structure meta or the structure tree, or an op the summary
    /// cannot read.
    const GLOBAL: u8 = 1;
    /// An op deleted from `doc.children`.
    const ROOT_LIST_DELETED: u8 = 2;
    /// An element other than a map container was inserted into `doc.children`.
    const UNREADABLE_INSERT: u8 = 4;
    /// An op on the alias map.
    const ALIASES: u8 = 8;
    /// A legacy tree op other than a create of the node it names.
    const TREE_OTHER: u8 = 16;

    const fn set(&mut self, kind: u8) {
        self.0 |= kind;
    }

    const fn has(self, kind: u8) -> bool {
        self.0 & kind != 0
    }
}

impl OpSummary {
    /// Whether `tree_created` is every node the import added to the legacy tree.
    pub(crate) const fn tree_ids_known(&self) -> bool {
        !self.seen.has(Seen::TREE_OTHER)
    }

    /// Reads one imported op. `peer` is the peer of the change that carries it and `root_list` the
    /// id of `doc.children` after the import.
    pub(crate) fn visit(&mut self, peer: u64, op: &JsonOp, root_list: Option<&ContainerID>) {
        match &op.container {
            // A mergeable child of a map (the text of a legacy tree node, for one) is named like a
            // root container but belongs to the map that created it: it is mapped to its unit by
            // its path, like any other child.
            ContainerID::Root { .. } if op.container.is_mergeable() => {
                self.containers.insert(op.container.clone());
            }
            ContainerID::Root { name, container_type } => self.visit_root(peer, op, name, *container_type),
            ContainerID::Normal { .. } if Some(&op.container) == root_list => self.visit_root_list(peer, op),
            ContainerID::Normal { .. } => {
                if let JsonOpContent::Map(JsonMapOp::Insert { key, .. } | JsonMapOp::Delete { key }) = &op.content
                    && key == LOGICAL_ID_KEY
                {
                    self.logical_id_writes.push(op.container.clone());
                }
                self.containers.insert(op.container.clone());
            }
        }
    }

    fn visit_root(&mut self, peer: u64, op: &JsonOp, name: &str, kind: ContainerType) {
        if crate::reserved_roots::is_shadow_root(name, kind) {
            self.seen.set(Seen::GLOBAL);
            return;
        }
        match name {
            DOC | META | TREE => self.seen.set(Seen::GLOBAL),
            ALIASES => self.seen.set(Seen::ALIASES),
            // The navigator order and the title never reach a limit; any other root is not read,
            // except a tree: a tree node's metadata map is named by the node's id alone, so a
            // node that two trees hold (one of them through a move of a node it never created)
            // has one metadata map, whichever tree the library reaches it through.
            _ if kind == ContainerType::Tree && name != ENGINE_TREE => self.seen.set(Seen::TREE_OTHER),
            ENGINE_TREE => match &op.content {
                JsonOpContent::Tree(JsonTreeOp::Create { target, .. }) if *target == TreeID::new(peer, op.counter) => {
                    self.tree_created.push(*target);
                }
                _ => self.seen.set(Seen::TREE_OTHER),
            },
            _ => {}
        }
    }

    fn visit_root_list(&mut self, peer: u64, op: &JsonOp) {
        match &op.content {
            JsonOpContent::List(JsonListOp::Insert { value, .. }) => {
                for (offset, element) in value.iter().enumerate() {
                    match element {
                        // A container inserted into a list is created by the op that inserts it:
                        // its id is that op's id. An element that names any other container
                        // would be an existing one, which no unit accounts for.
                        loro::LoroValue::Container(
                            id @ ContainerID::Normal {
                                peer: owner,
                                counter,
                                container_type: ContainerType::Map,
                            },
                        ) if *owner == peer
                            && i64::from(*counter)
                                == i64::from(op.counter).saturating_add(i64::try_from(offset).unwrap_or(i64::MAX)) =>
                        {
                            self.inserted_blocks.push(id.clone());
                        }
                        _ => self.seen.set(Seen::UNREADABLE_INSERT),
                    }
                }
            }
            JsonOpContent::List(JsonListOp::Delete { .. }) => self.seen.set(Seen::ROOT_LIST_DELETED),
            _ => self.seen.set(Seen::GLOBAL),
        }
    }
}

/// The signed change of the three summed aggregates, and the largest depth and longest text among
/// the units evaluated.
#[derive(Debug, Default)]
struct Delta {
    share: Contribution,
    max_depth: usize,
    longest: usize,
}

impl Delta {
    fn add(&mut self, post: Option<&UnitFact>, pre: Option<&UnitFact>) {
        let after = post.map(UnitFact::contribution).unwrap_or_default();
        let before = pre.map(UnitFact::contribution).unwrap_or_default();
        let times = if injected!(DeltaAppliedTwice) { 2 } else { 1 };
        self.share.blocks += times * (after.blocks - before.blocks);
        self.share.navigators += times * (after.navigators - before.navigators);
        self.share.chars += times * (after.chars - before.chars);
        if let Some(unit) = post {
            self.max_depth = self.max_depth.max(unit.depth);
            if unit.live {
                self.longest = self.longest.max(unit.chars);
            }
        }
    }
}

impl Delta {
    /// Whether a unit evaluated so far is deeper or longer than its limit. Both values are only
    /// bounds (the full computation reports their exact numbers), so the check stops at the first
    /// such unit and leaves the decision to the full computation, before it evaluates any more
    /// units.
    const fn over_a_bound(&self, limits: &DocumentLimits) -> bool {
        self.max_depth > limits.tree_depth_max || self.longest > limits.text_block_chars_max
    }
}

/// Why [`check`] did not accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Undecided {
    /// The check does not decide; the full computation does.
    Fallback(FallbackReason),
    /// A total is over its limit: the refusal, with the numbers the full computation reports.
    Refused(LimitViolation),
}

impl From<FallbackReason> for Undecided {
    fn from(reason: FallbackReason) -> Self {
        Self::Fallback(reason)
    }
}

/// The units an update touched, mapped from its ops.
#[derive(Debug, Default)]
struct Touched {
    blocks: HashSet<ContainerID>,
    tree_nodes: HashSet<TreeID>,
    aliases: bool,
}

const CID_PREFIX: &str = "cid:";
/// The key of a tree node's metadata map that holds its logical id.
const LOGICAL_ID_KEY: &str = "logical_id";

/// The root list is longer than this many elements, whatever the elements are, and the check does
/// not decide.
///
/// Elements of a kind no reader supports (and prose blocks that are dependency nodes) count toward
/// no limit and cost O(1) here, but every full projection (the setup's facts, the host's) visits
/// each of them, and on the full path so did the metered window, which is what stopped such a page
/// from growing: the window ran out of its CPU budget at about 16,000 elements on the reference
/// runner. Above this length the update takes that full path again, so the stop is kept. It is
/// one and a half times `document_block_count_max`: a page at its block limit still has room for
/// unsupported elements beside its blocks and stays incremental, and the length is below where the
/// full path stops, so no page that today's window accepts is made to fail. The post-import length
/// decides, so an update cannot step over it; a base already longer takes no facts at all
/// ([`BaseFacts::compute`]).
pub const ROOT_LIST_FALLBACK_LEN: usize = crate::limits::DOCUMENT_BLOCK_COUNT_MAX / 2 * 3;

/// `Ok(report)` when the update is certainly accepted by the full computation too, and otherwise
/// either the reason the check did not decide or the refusal of a total.
fn check(engine: &LoroCollabEngine, facts: Option<&BaseFacts>, trace: &ImportTrace) -> Result<WindowReport, Undecided> {
    let facts = facts.ok_or(FallbackReason::FactsAbsent)?;
    // A causal import never leaves anything pending; if it did, the ops seen are not the update.
    if trace.status.pending.is_some() {
        return Err(FallbackReason::Unclassified.into());
    }
    if facts.state != trace.before_state {
        return Err(FallbackReason::StaleFacts.into());
    }
    let summary = &trace.summary;
    if !injected!(StructuralFallbackSkipped) {
        if summary.seen.has(Seen::GLOBAL) {
            return Err(FallbackReason::GlobalStructure.into());
        }
        if summary.seen.has(Seen::TREE_OTHER) {
            return Err(FallbackReason::TreeStructure.into());
        }
        if engine.has_duplicate_logical_ids() {
            return Err(FallbackReason::DuplicateLogicalId.into());
        }
        if trace.resolved_existing_id {
            return Err(FallbackReason::ResolvedNodeId.into());
        }
    }
    if summary.seen.has(Seen::UNREADABLE_INSERT) {
        return Err(FallbackReason::UnreadableInsert.into());
    }
    if !injected!(ListThresholdSkipped) && engine.root_list_len().is_some_and(|len| len > ROOT_LIST_FALLBACK_LEN) {
        return Err(FallbackReason::OversizedList.into());
    }
    let created: HashSet<TreeID> = summary.tree_created.iter().copied().collect();
    // The id caches never re-read a node they hold, so a rewritten logical id of an existing node
    // is not seen by either path. The full computation decides what it always decided.
    if !injected!(IdRewriteSkipped)
        && summary
            .logical_id_writes
            .iter()
            .any(|container| !is_meta_of_created(container, &created))
    {
        return Err(FallbackReason::IdRewritten.into());
    }

    let removed = if summary.seen.has(Seen::ROOT_LIST_DELETED) {
        removed_blocks(engine, facts, summary)?
    } else {
        Vec::new()
    };
    let touched = touched_units(engine, summary)?;
    if touched.aliases || summary.seen.has(Seen::ALIASES) {
        alias_count_within_limit(engine)?;
    }

    let limits = DocumentLimits::default();
    let mut delta = Delta::default();
    for id in removed.iter().filter(|_| !injected!(RemovedBlocksIgnored)) {
        // A block that left the list contributes nothing now. A tree node with its container id
        // as logical id would be dropped by the projection beside it: not modelled.
        let key = id.to_string();
        if engine.has_logical_id(&key) {
            return Err(FallbackReason::KeyCollision.into());
        }
        delta.add(None, facts.units.get(key.as_str()));
    }
    evaluate_blocks(engine, facts, &touched, &limits, &mut delta)?;
    evaluate_tree_nodes(engine, facts, &created, &touched, &limits, &mut delta)?;
    finish(facts, &delta, &limits)
}

/// Whether `container` is the metadata map of a node in `created`.
fn is_meta_of_created(container: &ContainerID, created: &HashSet<TreeID>) -> bool {
    matches!(
        container,
        ContainerID::Normal { peer, counter, container_type: ContainerType::Map }
            if created.contains(&TreeID::new(*peer, *counter))
    )
}

/// The blocks of the base that the update removed from `doc.children`.
///
/// A list delete is applied by position, so the ids its op names say nothing about the elements it
/// removed; the document after the import says it exactly. The list's value names every element
/// by its container id (a cheap read), and the list now is the base's list without the removed
/// blocks and with the inserted ones. That is checked, not assumed: every element is either one the
/// update inserted or the next one of the base's remaining blocks in order, or the check does not
/// decide.
fn removed_blocks(
    engine: &LoroCollabEngine,
    facts: &BaseFacts,
    summary: &OpSummary,
) -> Result<Vec<ContainerID>, FallbackReason> {
    let current = engine
        .root_block_ids()
        .ok()
        .flatten()
        .ok_or(FallbackReason::Unclassified)?;
    let inserted: HashSet<&ContainerID> = summary.inserted_blocks.iter().collect();
    let base = &facts.block_order;
    let mut removed = Vec::new();
    let mut next = 0usize;
    let mut seen_inserted = 0usize;
    for id in &current {
        if inserted.contains(id) {
            seen_inserted += 1;
            continue;
        }
        while base.get(next).is_some_and(|candidate| candidate != id) {
            removed.extend(base.get(next).cloned());
            next += 1;
        }
        if next >= base.len() {
            return Err(FallbackReason::Unclassified);
        }
        next += 1;
    }
    removed.extend(base.get(next..).unwrap_or_default().iter().cloned());
    if seen_inserted != inserted.len() {
        return Err(FallbackReason::Unclassified);
    }
    Ok(removed)
}

/// Maps every container the update named to its unit, from the document after the import.
fn touched_units(engine: &LoroCollabEngine, summary: &OpSummary) -> Result<Touched, FallbackReason> {
    let doc = engine.doc();
    let mut touched = Touched::default();
    touched.tree_nodes.extend(summary.tree_created.iter().copied());
    for id in &summary.inserted_blocks {
        // An element the update inserted is visible in the list, or something is wrong.
        let path = doc.get_path_to_container(id).ok_or(FallbackReason::Unclassified)?;
        match block_of(&path) {
            Some(block) if &block == id => {
                touched.blocks.insert(block);
            }
            _ => return Err(FallbackReason::Unclassified),
        }
    }
    for id in &summary.containers {
        if injected!(TouchedUnitMissed) {
            break;
        }
        // A container without a path is not reachable from any root: no reader sees it.
        let Some(path) = doc.get_path_to_container(id) else {
            continue;
        };
        let Some((ContainerID::Root { name, container_type }, _)) = path.first() else {
            return Err(FallbackReason::Unclassified);
        };
        if crate::reserved_roots::is_shadow_root(name, *container_type) {
            return Err(FallbackReason::GlobalStructure);
        }
        match name.as_str() {
            DOC => match block_of(&path) {
                Some(block) => {
                    touched.blocks.insert(block);
                }
                // The list itself, which the summary handles by id, would be the only other
                // legitimate path; anything else under `doc` is not modelled.
                _ => return Err(FallbackReason::GlobalStructure),
            },
            ENGINE_TREE => match path.get(1) {
                Some((_, Index::Node(node))) => {
                    touched.tree_nodes.insert(*node);
                }
                _ => return Err(FallbackReason::Unclassified),
            },
            META | TREE => return Err(FallbackReason::GlobalStructure),
            ALIASES => touched.aliases = true,
            // The navigator order and the title never reach a limit; any other root is not read.
            // A container below a tree node is the metadata map of a node of some tree: if the
            // library reaches it through a tree this check does not model, it is not mapped.
            _ => {
                if path.iter().any(|(_, index)| matches!(index, Index::Node(_))) {
                    return Err(FallbackReason::Unclassified);
                }
            }
        }
    }
    Ok(touched)
}

/// The block map under `doc.children` that a path from the root goes through (a path
/// `doc / children / <element> / ...`), or `None` for any other shape.
fn block_of(path: &[(ContainerID, Index)]) -> Option<ContainerID> {
    let [(root, _), (_, Index::Key(children)), (block, Index::Seq(_)), ..] = path else {
        return None;
    };
    if !matches!(root, ContainerID::Root { name, container_type: ContainerType::Map } if name.as_ref() == DOC)
        || children.as_ref() != "children"
        || !matches!(
            block,
            ContainerID::Normal {
                container_type: ContainerType::Map,
                ..
            }
        )
    {
        return None;
    }
    Some(block.clone())
}

/// The alias map, as the full reader counts it, is within the limit.
fn alias_count_within_limit(engine: &LoroCollabEngine) -> Result<(), FallbackReason> {
    match crate::editor_structure::root_map(engine.doc(), ALIASES) {
        Ok(Some(aliases)) if aliases.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX => Err(FallbackReason::AliasLimit),
        Ok(_) => Ok(()),
        Err(_) => Err(FallbackReason::InvalidUnit),
    }
}

/// Evaluates the touched editor blocks on the document after the import, with the code of the full
/// projection, and adds each one's change to `delta`.
fn evaluate_blocks(
    engine: &LoroCollabEngine,
    facts: &BaseFacts,
    touched: &Touched,
    limits: &DocumentLimits,
    delta: &mut Delta,
) -> Result<(), FallbackReason> {
    let no_aliases = std::collections::BTreeMap::new();
    for id in &touched.blocks {
        #[cfg(test)]
        crate::walk_counter::record_unit_evaluation();
        let Some(Container::Map(map)) = engine.doc().get_container(id.clone()) else {
            return Err(FallbackReason::Unclassified);
        };
        let key = id.to_string();
        // A tree node with this logical id is dropped by the projection when the block is a
        // legacy block: not modelled.
        if engine.has_logical_id(&key) {
            return Err(FallbackReason::KeyCollision);
        }
        let visit = crate::editor_doc_bridge::evaluate_block(&map, &key, true, &no_aliases)
            .map_err(|_| FallbackReason::InvalidUnit)?;
        let post = match visit.current {
            crate::editor_doc_bridge::CurrentRead::Skipped => None,
            crate::editor_doc_bridge::CurrentRead::Block(block) => Some(UnitFact {
                live: true,
                kind: NodeKind::Block,
                chars: block.text.chars().count(),
                depth: 0,
            }),
            crate::editor_doc_bridge::CurrentRead::Invalid(_) => return Err(FallbackReason::InvalidUnit),
        };
        let pre = facts.units.get(key.as_str());
        delta.add(post.as_ref(), pre);
        if delta.over_a_bound(limits) {
            return Err(FallbackReason::LimitReached);
        }
    }
    Ok(())
}

/// Evaluates the touched legacy tree nodes and adds each one's change to `delta`.
///
/// No node moved or was deleted (those updates fall back), so an existing node keeps its depth, and
/// only its liveness, kind and text can have changed. A created node's depth is its parent's plus
/// one, through the parents the update created and, beyond them, the base's facts.
fn evaluate_tree_nodes(
    engine: &LoroCollabEngine,
    facts: &BaseFacts,
    created: &HashSet<TreeID>,
    touched: &Touched,
    limits: &DocumentLimits,
    delta: &mut Delta,
) -> Result<(), FallbackReason> {
    // A created node is new, so it has no children but the ones this update created. The caches
    // already refused a created target that existed (`rebuild_id_cache_for`); this is the same
    // condition read from the other side, from the document after the import, and it also covers
    // nodes that the library only now attaches to a created parent (an earlier op of theirs that
    // could not apply because the parent was missing).
    if !injected!(CreatedTargetChecksSkipped) && !engine.created_nodes_adopt_nothing(created) {
        return Err(FallbackReason::ResolvedNodeId);
    }
    let mut units: Vec<(TreeUnit, bool)> = Vec::with_capacity(touched.tree_nodes.len());
    for node in &touched.tree_nodes {
        #[cfg(test)]
        crate::walk_counter::record_unit_evaluation();
        let unit = engine
            .evaluate_tree_unit(*node, facts.editor_doc)
            .map_err(|_| FallbackReason::InvalidUnit)?;
        // A node without a logical id is not in the projection.
        let Some(unit) = unit else { continue };
        let looks_like_block = |key: &str| key.starts_with(CID_PREFIX);
        if looks_like_block(&unit.key) || unit.parent_key.as_deref().is_some_and(looks_like_block) {
            return Err(FallbackReason::KeyCollision);
        }
        units.push((unit, created.contains(node)));
    }
    let fresh: HashMap<&str, &TreeUnit> = units
        .iter()
        .filter(|(_, is_new)| *is_new)
        .map(|(unit, _)| (&*unit.key, unit))
        .collect();
    for (unit, is_new) in &units {
        let pre = facts.units.get(&*unit.key);
        let post = if *is_new {
            if pre.is_some() {
                return Err(FallbackReason::Inconsistent);
            }
            unit.present
                .then(|| created_unit_fact(unit, facts, &fresh))
                .transpose()?
        } else {
            if unit.present != pre.is_some() {
                return Err(FallbackReason::PresenceChange);
            }
            pre.map(|pre| UnitFact {
                live: !unit.deleted,
                kind: unit.kind,
                chars: unit.chars,
                depth: pre.depth,
            })
        };
        delta.add(post.as_ref(), pre);
        if delta.over_a_bound(limits) {
            return Err(FallbackReason::LimitReached);
        }
    }
    Ok(())
}

/// The fact of a node the update created and the projection keeps: its depth is the number of
/// parent hops through nodes the projection keeps, counted the way `limits::depth_of` counts them.
fn created_unit_fact(
    unit: &TreeUnit,
    facts: &BaseFacts,
    fresh: &HashMap<&str, &TreeUnit>,
) -> Result<UnitFact, FallbackReason> {
    let limit = DocumentLimits::default().tree_depth_max;
    let mut hops = 0usize;
    let mut parent = unit.parent_key.clone();
    while let Some(key) = parent.take() {
        hops += 1;
        if hops > limit + 1 {
            return Err(FallbackReason::LimitReached);
        }
        match fresh.get(&*key) {
            Some(next) if next.present => parent.clone_from(&next.parent_key),
            // A created parent the projection drops ends the chain at the hop to it.
            Some(_) => {}
            // An existing parent: the base's depth of it, if the projection keeps it.
            None => hops += facts.units.get(&*key).map_or(0, |fact| fact.depth),
        }
    }
    Ok(UnitFact {
        live: !unit.deleted,
        kind: unit.kind,
        chars: unit.chars,
        depth: hops,
    })
}

/// Base facts plus the delta, against the limits.
///
/// The three totals are exact, so a total over its limit is refused here with the limit kind and
/// the `observed` value the full computation reports, in its precedence (`container_count`, then
/// `document_block_count`, then `document_text_chars`). The two maxima are only bounds: a unit
/// beyond either of them has already ended the check (see [`Delta::over_a_bound`]), so when this
/// runs neither is violated, which is what makes the precedence of the three totals the full
/// computation's.
fn finish(facts: &BaseFacts, delta: &Delta, limits: &DocumentLimits) -> Result<WindowReport, Undecided> {
    let total = |base: usize, change: i64| {
        i64::try_from(base)
            .ok()
            .and_then(|base| base.checked_add(change))
            .and_then(|total| usize::try_from(total).ok())
            .ok_or(FallbackReason::Inconsistent)
    };
    let blocks = total(facts.blocks, delta.share.blocks)?;
    let navigators = total(facts.navigators, delta.share.navigators)?;
    let chars = total(facts.chars, delta.share.chars)?;
    if delta.over_a_bound(limits) {
        return Err(FallbackReason::LimitReached.into());
    }
    let refuse = |limit_kind: &'static str, limit: usize, observed: usize| {
        let skew = u64::from(injected!(RefusalOffByOne));
        Undecided::Refused(LimitViolation {
            limit_kind,
            limit: limit as u64,
            observed: observed as u64 + skew,
        })
    };
    if navigators > limits.container_count_max {
        return Err(refuse("container_count", limits.container_count_max, navigators));
    }
    if blocks > limits.document_block_count_max {
        return Err(refuse("document_block_count", limits.document_block_count_max, blocks));
    }
    if chars > limits.document_text_chars_max {
        return Err(refuse("document_text_chars", limits.document_text_chars_max, chars));
    }
    Ok(WindowReport {
        blocks: blocks as u64,
        navigators: navigators as u64,
        chars: chars as u64,
        depth_bound: facts.max_depth.max(delta.max_depth) as u64,
        longest_bound: facts.longest.max(delta.longest) as u64,
    })
}

/// How a window's report differs from the full projection of the accepted document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowDivergence {
    pub field: &'static str,
    pub reported: u64,
    pub projected: u64,
}

/// Compares what the incremental check established with the full projection of the same accepted
/// document.
///
/// The totals must be equal, the bounds must hold, and the full `check_snapshot` must accept. This
/// is the oracle that keeps a wrong fact from becoming an accepted write.
///
/// # Errors
/// The first field in which the two disagree.
pub fn compare_with_projection(report: &WindowReport, projection: &SemanticSnapshot) -> Result<(), WindowDivergence> {
    let live = || projection.nodes.values().filter(|node| !node.deleted);
    let count = |kind: NodeKind| live().filter(|node| node.kind == kind).count() as u64;
    let chars = |node: &crate::semantic::SemanticNode| node.text.chars().count() as u64;
    let projected = [
        ("blocks", report.blocks, count(NodeKind::Block)),
        ("navigators", report.navigators, count(NodeKind::NavigatorNode)),
        ("chars", report.chars, live().map(chars).sum()),
    ];
    for (field, reported, projected) in projected {
        if reported != projected {
            return Err(WindowDivergence {
                field,
                reported,
                projected,
            });
        }
    }
    // The depth is computed once, in one linear pass that stops one past the limit, and shared by
    // the bound check and the limit check below: what the oracle costs does not depend on how
    // deep a hostile document is.
    let limits = DocumentLimits::default();
    let deepest = max_depth_capped(projection, limits.tree_depth_max);
    if deepest as u64 > report.depth_bound {
        return Err(WindowDivergence {
            field: "depth_bound",
            reported: report.depth_bound,
            projected: deepest as u64,
        });
    }
    let longest = live().map(chars).max().unwrap_or(0);
    if longest > report.longest_bound {
        return Err(WindowDivergence {
            field: "longest_bound",
            reported: report.longest_bound,
            projected: longest,
        });
    }
    if let Err(violation) = check_snapshot_with_depth(projection, &limits, deepest) {
        return Err(WindowDivergence {
            field: violation.limit_kind,
            reported: violation.limit,
            projected: violation.observed,
        });
    }
    Ok(())
}
